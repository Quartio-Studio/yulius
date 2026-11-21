//! `scripts/dev mutants` (SPEC-493 MUT-001): one bounded mutation run of the Rust lines the
//! branch changed against the integration branch, with the pinned `cargo-mutants`. Each mutant
//! is tested with nextest across the workspace; the product's own test binary (package `yulius`,
//! the slowest by far) only with the tests the branch added or changed, the ones meant to hold
//! its lines, unless `--tests` names others. A bound on the whole run ends it and names the
//! mutants it did not reach; the last line is the summary for the spec's Evidence.

mod process;

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

/// The most mutants tested at once by default: the most measured (SPEC-506, 2026-11-17).
const MOST_JOBS: u32 = 7;
/// Mutants listed per job by default: each job first builds the workspace in a copy of its own,
/// which pays only when it then tests several mutants (7 jobs tested 8 mutants no sooner than 2).
const MUTANTS_PER_JOB: usize = 8;
/// Memory a job may take by default: a copy of the workspace with its build (12 GB here, in the
/// temporary directory) and the tests it runs.
const JOB_MEMORY_MB: u64 = 12_000;

/// The process's budget, as Yulius gives it (`YULIUS_CPUS`, `YULIUS_MEMORY_MB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    /// CPUs.
    pub cpus: Option<u32>,
    /// Memory in MB.
    pub memory_mb: Option<u64>,
}

impl Budget {
    /// Reads the budget through `env` (a variable's value by name); a value that does not parse
    /// counts as none.
    pub fn read(env: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            cpus: env("YULIUS_CPUS").and_then(|v| v.trim().parse().ok()),
            memory_mb: env("YULIUS_MEMORY_MB").and_then(|v| v.trim().parse().ok()),
        }
    }
}

/// Code no mutant is made in: the test tool and test folders.
const EXCLUDED: [&str; 2] = ["tools/**", "crates/*/tests/**"];

/// The run's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// What the branch is compared with.
    pub base: String,
    /// The bound on the whole run.
    pub minutes: u64,
    /// The bound on one mutant's tests.
    pub mutant_seconds: u64,
    /// Mutants tested at once (each in a build directory of its own), when `--jobs` names it;
    /// else [`Self::jobs_for`] decides from the mutants listed and the budget.
    pub jobs: Option<u32>,
    /// The process's budget.
    pub budget: Budget,
    /// A nextest filter for the `yulius` package's tests in place of the changed ones.
    pub tests: Option<String>,
    /// The bound in seconds in place of `minutes` (tests).
    pub seconds: Option<u64>,
}

impl Settings {
    /// Parses `--base REF`, `--minutes N`, `--mutant-seconds N`, `--jobs N`, `--tests FILTER`,
    /// with the process's budget from its environment.
    ///
    /// # Errors
    /// An unknown option, a missing or malformed value.
    pub fn parse(args: &[String], integration: &str) -> Result<Self, String> {
        Self::parse_with(
            args,
            integration,
            Budget::read(|name| std::env::var(name).ok()),
        )
    }

    /// [`Self::parse`] with the budget given.
    ///
    /// # Errors
    /// As [`Self::parse`].
    pub fn parse_with(args: &[String], integration: &str, budget: Budget) -> Result<Self, String> {
        let mut settings = Self {
            base: format!("origin/{integration}"),
            minutes: 30,
            mutant_seconds: 300,
            jobs: None,
            budget,
            tests: None,
            seconds: None,
        };
        let mut args = args.iter();
        while let Some(option) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("{option} needs a value"))?;
            let number =
                || {
                    value.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| {
                        format!("{option} takes a whole number above 0, not {value}")
                    })
                };
            match option.as_str() {
                "--base" => settings.base.clone_from(value),
                "--minutes" => settings.minutes = number()?,
                "--mutant-seconds" => settings.mutant_seconds = number()?,
                "--jobs" => settings.jobs = Some(u32::try_from(number()?).unwrap_or(u32::MAX)),
                "--tests" => settings.tests = Some(value.clone()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(settings)
    }

    /// Mutants tested at once for `listed` mutants (SPEC-506 FST-005): `--jobs` when given, else
    /// one per [`MUTANTS_PER_JOB`] listed, at least 2, at most a third of the CPUs (the rest is
    /// the review's and CI's, which run beside it), one per [`JOB_MEMORY_MB`] and [`MOST_JOBS`];
    /// 2 with no budget, as before.
    #[must_use]
    pub fn jobs_for(&self, listed: usize) -> u32 {
        if let Some(jobs) = self.jobs {
            return jobs;
        }
        let by_mutants = u32::try_from(listed.div_ceil(MUTANTS_PER_JOB)).unwrap_or(u32::MAX);
        let by_cpus = self.budget.cpus.map_or(2, |cpus| cpus / 3);
        let jobs = by_mutants.min(by_cpus).clamp(2, MOST_JOBS);
        match self.budget.memory_mb {
            Some(mb) => jobs.min(u32::try_from(mb / JOB_MEMORY_MB).unwrap_or(u32::MAX).max(1)),
            None => jobs,
        }
    }

    /// The bound on the whole run.
    #[must_use]
    pub fn bound(&self) -> Duration {
        Duration::from_secs(self.seconds.unwrap_or(self.minutes * 60))
    }
}

/// The names of the test functions a diff adds or changes in the product's test binary
/// (`crates/yulius/tests/`): every `fn name(` in a hunk there. With `git diff -W` a hunk is the
/// whole function a change is in, so a test whose body alone changed is named too (review round
/// 1 of fix 8).
#[must_use]
pub fn changed_tests(diff: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    let mut in_tests = false;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            in_tests = path
                .trim_start_matches("b/")
                .starts_with("crates/yulius/tests/");
            continue;
        }
        let Some(added) = line.strip_prefix(['+', ' ']) else {
            continue;
        };
        if !in_tests {
            continue;
        }
        if let Some(rest) = added.trim_start().split("fn ").nth(1) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && rest[name.len()..].starts_with(['(', '<']) {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}

/// The nextest filter each mutant is tested with: every package's tests, the product's own unit
/// tests too, and of the product's integration tests (its slow binary) those named by `tests`
/// (a filter of the caller's) or the `changed` ones (review round 1 of fix 8).
#[must_use]
pub fn test_filter(tests: Option<&str>, changed: &[String]) -> String {
    let product = match (tests, changed) {
        (Some(filter), _) => filter.to_owned(),
        (None, []) => "none()".to_owned(),
        (None, names) => format!("test(/(^|::)({})$/)", names.join("|")),
    };
    format!("not package(yulius) | (package(yulius) & (not kind(test) | {product}))")
}

/// What became of the mutants listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcomes {
    /// The current campaign recorded a successful unmutated baseline.
    pub baseline_passed: bool,
    /// A test failed for it.
    pub caught: Vec<String>,
    /// No test failed for it.
    pub missed: Vec<String>,
    /// Its tests ran past the bound on one mutant.
    pub timeout: Vec<String>,
    /// It did not build.
    pub unviable: Vec<String>,
    /// The run's bound ended it first.
    pub unreached: Vec<String>,
}

impl Outcomes {
    /// The outcomes of `listed`, from cargo-mutants' files of each kind (one mutant a line).
    #[must_use]
    pub fn of(
        listed: &[String],
        baseline_passed: bool,
        caught: &str,
        missed: &str,
        timeout: &str,
        unviable: &str,
    ) -> Self {
        let lines = |text: &str| -> Vec<String> {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let (caught, missed, timeout, unviable) = (
            lines(caught),
            lines(missed),
            lines(timeout),
            lines(unviable),
        );
        let seen: BTreeSet<&String> = caught
            .iter()
            .chain(&missed)
            .chain(&timeout)
            .chain(&unviable)
            .collect();
        let unreached = listed
            .iter()
            .filter(|m| !seen.contains(m))
            .cloned()
            .collect();
        Self {
            baseline_passed,
            caught,
            missed,
            timeout,
            unviable,
            unreached,
        }
    }

    /// Whether every mutant was tested and caught (or did not build).
    #[must_use]
    pub fn held(&self) -> bool {
        self.baseline_passed
            && self.missed.is_empty()
            && self.timeout.is_empty()
            && self.unreached.is_empty()
    }

    /// The summary line for the Evidence.
    #[must_use]
    pub fn summary(&self, listed: usize, bound: Duration, took: Duration) -> String {
        format!(
            "mutants: {listed} listed, {} caught, {} missed, {} timed out, {} unviable, {} not \
             reached (bound {} min, took {} s)",
            self.caught.len(),
            self.missed.len(),
            self.timeout.len(),
            self.unviable.len(),
            self.unreached.len(),
            bound.as_secs().div_ceil(60),
            took.as_secs()
        )
    }
}

fn git(root: &Path, args: &[&str]) -> io::Result<String> {
    let out = Command::new("git").current_dir(root).args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What an outer nextest run sets for the tests it runs (its profile and its own run): not the
/// settings of the mutants' own runs. `NEXTEST_TEST_THREADS`, the sandbox's CPU budget, is
/// kept (review round 2 of fix 8).
fn outer_nextest(name: &str) -> bool {
    name == "NEXTEST"
        || name == "NEXTEST_PROFILE"
        || name == "NEXTEST_RUN_ID"
        || name == "NEXTEST_EXECUTION_MODE"
        || name.starts_with("NEXTEST_BIN_EXE_")
}

/// A cargo-mutants command in `root`, in an environment of its own: no shared target directory
/// (with more than one job, copies writing one target directory test each other's builds and
/// report mutants caught or missed wrongly; review round 1 of fix 8) and nothing an outer
/// nextest run set ([`outer_nextest`]).
fn mutants_command(cargo: &str, root: &Path, head: &str) -> Command {
    let mut command = process::command(cargo);
    command
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    // cargo-mutants builds in a copy without `.git`, where `crates/yulius/build.rs` finds no
    // commit: it is given the checkout's, as a build of the checkout gets it, unless the
    // caller names one.
    if std::env::var_os("YULIUS_BUILD_COMMIT").is_none() {
        command.env("YULIUS_BUILD_COMMIT", head);
    }
    for (name, _) in std::env::vars_os() {
        if outer_nextest(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
    command
}

/// What keeps a run of `jobs` mutants at once near the budget's CPUs (SPEC-506, review rounds 1
/// and 2 of pull request 2): the builds share cargo-mutants' jobserver, which gets the budget's
/// CPUs (by default the host's); tests take no jobserver task, so each job's tests get their share
/// as threads (else each job's nextest takes the whole budget). Builds and tests that overlap can
/// pass the budget for a while: it bounds the mean (17.1 of 22 CPUs measured), not each moment.
fn budget_caps(budget: Budget, jobs: u32) -> (Vec<String>, Vec<(String, String)>) {
    let Some(cpus) = budget.cpus else {
        return (Vec::new(), Vec::new());
    };
    let threads = (cpus / jobs.max(1)).max(1).to_string();
    (
        vec!["--jobserver-tasks".to_owned(), cpus.max(1).to_string()],
        vec![
            ("NEXTEST_TEST_THREADS".to_owned(), threads.clone()),
            ("RUST_TEST_THREADS".to_owned(), threads),
        ],
    )
}

/// The cargo-mutants arguments that choose the mutants: those in `diff`, outside [`EXCLUDED`].
fn selection(diff: &Path) -> Vec<String> {
    let mut args = vec![
        "mutants".to_owned(),
        "--in-diff".to_owned(),
        diff.display().to_string(),
        "--all-features".to_owned(),
    ];
    for glob in EXCLUDED {
        args.extend(["--exclude".to_owned(), glob.to_owned()]);
    }
    args
}

/// Interrupts request the same reaped shutdown as a campaign deadline, so
/// temporary workspace copies are removed before the command exits.
pub fn run(root: &Path, cargo: &str, settings: &Settings) -> io::Result<Option<Outcomes>> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    campaign(root, cargo, settings, &stop)
}

/// Runs the campaign in the repository at `root`: `None` when the branch changed no Rust code
/// with a mutant in it.
///
/// # Errors
/// Git or cargo-mutants could not be run.
fn campaign(
    root: &Path,
    cargo: &str,
    settings: &Settings,
    stop: &AtomicBool,
) -> io::Result<Option<Outcomes>> {
    let started = Instant::now();
    let base = git(root, &["merge-base", &settings.base, "HEAD"])?;
    let head = git(root, &["rev-parse", "HEAD"])?.trim().to_owned();
    // Read as Git itself writes it, whatever the configuration says (no external diff, no
    // colour, the usual prefixes): `--in-diff` and the test names parse it.
    let plain = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    let diff = git(
        root,
        &[&plain[..], &[base.trim(), "HEAD", "--", "*.rs"]].concat(),
    )?;
    let functions = git(
        root,
        &[&plain[..], &["-W", base.trim(), "HEAD", "--", "*.rs"]].concat(),
    )?;
    if !git(root, &["status", "--porcelain", "--", "*.rs"])?
        .trim()
        .is_empty()
    {
        println!(
            "Uncommitted Rust changes are not in the diff: commit them first, or line numbers drift."
        );
    }
    if diff.trim().is_empty() {
        println!(
            "No Rust code changed against {}: nothing to mutate.",
            settings.base
        );
        return Ok(None);
    }
    let dir = root.join("target/mutants");
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::File::create(dir.join(".lock"))?;
    lock.try_lock()
        .map_err(|e| io::Error::other(format!("cannot lock mutation reports: {e}")))?;
    let diff_file = dir.join("branch.diff");
    std::fs::write(&diff_file, &diff)?;
    let list = |bound: Duration| -> io::Result<Vec<String>> {
        let list_path = dir.join("listed.txt");
        let mut command = mutants_command(cargo, root, &head);
        command
            .args(selection(&diff_file))
            .arg("--list")
            .stdout(std::fs::File::create(&list_path)?);
        let status = process::run(&mut command, started + bound, stop)
            .map_err(process::RunError::into_io)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "deadline reached during mutation discovery",
                )
            })?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "cargo mutants --list failed ({status})"
            )));
        }
        Ok(std::fs::read_to_string(list_path)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    };
    let bound = settings.bound();
    let listed = list(bound)?;
    if listed.is_empty() {
        println!(
            "No mutant in the Rust lines changed against {}.",
            settings.base
        );
        return Ok(None);
    }
    // The product's tests are narrowed where the product is: a filter naming a package the
    // workspace does not have is refused by nextest.
    let filter = root
        .join("crates/yulius/Cargo.toml")
        .exists()
        .then(|| test_filter(settings.tests.as_deref(), &changed_tests(&functions)));
    let jobs = settings.jobs_for(listed.len());
    let (cap_args, cap_env) = budget_caps(settings.budget, jobs);
    println!(
        "{} mutants in the changed lines; each tested with nextest ({}), {} at once{}, \
         {} s per mutant test phase at most, {} s in all.",
        listed.len(),
        filter.as_deref().unwrap_or("all tests"),
        jobs,
        settings
            .budget
            .cpus
            .map(|cpus| format!(" within {cpus} CPUs"))
            .unwrap_or_default(),
        settings.mutant_seconds,
        settings.bound().as_secs()
    );
    let scratch_parent = std::env::var_os("YULIUS_BUILD_SCRATCH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dir.clone());
    let scratch = allocate_campaign_scratch(&scratch_parent)?;
    let disk = scratch.path().canonicalize()?;
    let cargo_config = disk.join("test-env.toml");
    std::fs::write(
        &cargo_config,
        "[env]\nTMPDIR = { value = \"/tmp\", force = true }\n",
    )?;
    let mut command = mutants_command(cargo, root, &head);
    command
        .env("TMPDIR", &disk)
        .env("YULIUS_BUILD_SCRATCH", &disk)
        .args(selection(&diff_file))
        .arg("--copy-target=false")
        .args([
            "--test-tool",
            "nextest",
            // 27.1.0 applies --test-workspace only to mutants. Cargo's --workspace
            // includes every member alongside the -p arguments in both scenarios.
            // Override test_workspace in .cargo/mutants.toml to avoid passing it twice.
            "--test-workspace=false",
            "--cargo-arg=--workspace",
            // Only the copy/build tree belongs in the disk scratch. Confined
            // Git fixtures and Unix sockets expect the ordinary temp root.
            "--no-shuffle",
        ])
        .arg(format!("--cargo-arg=--config={}", cargo_config.display()))
        .args(["--jobs", &jobs.to_string()])
        .args(&cap_args)
        .envs(cap_env)
        .args(["--timeout", &settings.mutant_seconds.to_string()])
        .args(["--output", &dir.display().to_string()])
        .args(
            filter
                .iter()
                .flat_map(|filter| ["--", "-E", filter.as_str()]),
        );
    // Rotate under the checkout lock before spawning. A tool killed during
    // discovery cannot leave the previous baseline or mutant verdicts at this path.
    let current = dir.join("mutants.out");
    if current.exists() {
        let old = dir.join("mutants.out.old");
        if old.exists() {
            std::fs::remove_dir_all(&old)?;
        }
        std::fs::rename(&current, old)?;
    }
    let run = process::run(&mut command, started + bound, stop);
    let status = scratch.finish(run)?;
    if let Some(status) = status {
        if status.code() == Some(4) {
            return Err(io::Error::other(format!(
                "cargo-mutants failed its unmutated baseline; see {}",
                current.join("log/baseline.log").display()
            )));
        }
        if !matches!(status.code(), Some(0 | 2 | 3)) {
            return Err(io::Error::other(format!(
                "cargo-mutants failed ({status}); its log: {}",
                current.join("debug.log").display()
            )));
        }
    }
    let baseline_passed = std::fs::read(current.join("outcomes.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|report| {
            report["outcomes"].as_array().is_some_and(|outcomes| {
                outcomes
                    .iter()
                    .any(|o| o["scenario"] == "Baseline" && o["summary"] == "Success")
            })
        });
    let read = |kind: &str| {
        std::fs::read_to_string(dir.join("mutants.out").join(format!("{kind}.txt")))
            .unwrap_or_default()
    };
    let outcomes = Outcomes::of(
        &listed,
        baseline_passed,
        &read("caught"),
        &read("missed"),
        &read("timeout"),
        &read("unviable"),
    );
    let baseline = if baseline_passed {
        "passed"
    } else {
        "incomplete"
    };
    for (what, mutants) in [
        ("Missed (no test failed)", &outcomes.missed),
        ("Timed out", &outcomes.timeout),
        ("Not reached within the bound", &outcomes.unreached),
    ] {
        if !mutants.is_empty() {
            println!("{what}:");
            if what.starts_with("Missed") && filter.is_some() {
                println!(
                    "  (each ran the tests this branch changed; a mutant held by other \
                     integration tests needs `--tests`)"
                );
            }
            for mutant in mutants {
                println!("  {mutant}");
            }
        }
    }
    println!(
        "base={} scope=lines jobs={jobs} test_timeout={}s baseline={baseline} tests=[{}] {}",
        base.trim(),
        settings.mutant_seconds,
        filter.as_deref().unwrap_or("all"),
        outcomes.summary(listed.len(), bound, started.elapsed())
    );
    Ok(Some(outcomes))
}

/// The exit code of a run: success when nothing was to mutate or every mutant was caught.
#[must_use]
pub fn exit_code(outcomes: Option<&Outcomes>) -> ExitCode {
    if outcomes.is_none_or(Outcomes::held) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

struct CampaignScratch {
    path: std::path::PathBuf,
    tree: Option<yulius_sandbox::build_tree::BuildTree>,
}

fn allocate_campaign_scratch(parent: &Path) -> io::Result<CampaignScratch> {
    let dir = tempfile::Builder::new().prefix("mut-").tempdir_in(parent)?;
    let tree = yulius_sandbox::build_tree::BuildTree::open(dir.path())?
        .ok_or_else(|| io::Error::other("campaign scratch disappeared during allocation"))?;
    Ok(CampaignScratch {
        path: dir.keep(),
        tree: Some(tree),
    })
}

impl CampaignScratch {
    fn path(&self) -> &Path {
        &self.path
    }

    fn finish(
        mut self,
        run: Result<Option<std::process::ExitStatus>, process::RunError>,
    ) -> io::Result<Option<std::process::ExitStatus>> {
        if let Err(process::RunError::Unreaped(error)) = run {
            self.tree = None;
            return Err(io::Error::other(format!(
                "{error}; retained build scratch {}",
                self.path.display()
            )));
        }
        self.reclaim()?;
        run.map_err(process::RunError::into_io)
    }

    fn reclaim(&mut self) -> io::Result<()> {
        if let Some(tree) = &self.tree {
            tree.clear()?;
            self.tree = None;
        }
        Ok(())
    }
}

impl Drop for CampaignScratch {
    fn drop(&mut self) {
        if let Err(error) = self.reclaim() {
            eprintln!("cannot reclaim campaign scratch: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root: &Path, cargo: &str, settings: &Settings) -> io::Result<Option<Outcomes>> {
        campaign(root, cargo, settings, &AtomicBool::new(false))
    }

    #[test]
    fn only_unconfirmed_worker_shutdown_retains_command_scratch() -> io::Result<()> {
        let parent = tempfile::tempdir()?;
        for unconfirmed in [true, false] {
            let scratch = allocate_campaign_scratch(parent.path())?;
            let path = scratch.path().to_owned();
            std::fs::write(path.join("artifact"), b"build")?;
            let failure = io::Error::other("worker observation failed");
            let outcome = if unconfirmed {
                process::RunError::Unreaped(failure)
            } else {
                process::RunError::Command(failure)
            };
            let error = scratch
                .finish(Err(outcome))
                .err()
                .ok_or("expected a failed command")
                .map_err(io::Error::other)?;
            assert_eq!(
                path.join("artifact").exists(),
                unconfirmed,
                "scratch lifetime"
            );
            if unconfirmed {
                assert!(error.to_string().contains(path.to_string_lossy().as_ref()));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn command_scratch_reclaims_read_only_builds_before_returning() -> io::Result<()> {
        use rustix::thread::{CapabilitySet, capabilities, set_capabilities};
        use std::fs;
        use std::os::unix::fs::{PermissionsExt, symlink};

        let parent = tempfile::tempdir()?;
        let outside = parent.path().join("outside");
        fs::create_dir(&outside)?;
        fs::write(outside.join("keep"), b"live")?;
        for completed in [true, false] {
            let scratch = allocate_campaign_scratch(parent.path())?;
            let path = scratch.path().to_owned();
            let dependency = path.join("target");
            fs::create_dir(&dependency)?;
            symlink(&outside, dependency.join("other"))?;
            fs::write(dependency.join("build"), b"artifact")?;
            fs::set_permissions(dependency, fs::Permissions::from_mode(0o555))?;
            let original = capabilities(None)?;
            let mut limited = original;
            limited
                .effective
                .remove(CapabilitySet::DAC_OVERRIDE | CapabilitySet::DAC_READ_SEARCH);
            set_capabilities(None, limited)?;
            let result = if completed {
                scratch.finish(Ok(None)).map(|_| ())
            } else {
                // An early error in command setup or supervision drops its owner.
                drop(scratch);
                Ok(())
            };
            set_capabilities(None, original)?;
            result?;
            assert!(!path.exists());
            assert!(outside.join("keep").is_file());
        }
        Ok(())
    }

    /// SPEC-493 MUT-001: the product's tests a branch added or changed are the ones each mutant
    /// is tested with; other packages' tests all run; `--tests` names others.
    #[test]
    fn each_mutant_runs_the_tests_the_branch_wrote() {
        let diff = "+++ b/crates/yulius/tests/review_rounds.rs\n\
                    +    fn a_cut_off_reviewer(second: bool) -> TestResult {\n\
                    +async fn a_tight_round() -> TestResult {\n\
                    + let x = some_fn(1);\n\
                    @@ -10,4 +10,4 @@\n\
                    \x20fn body_only() -> TestResult {\n\
                    -    assert!(x);\n\
                    +    assert!(y);\n\
                    +++ b/crates/yulius-app/src/review.rs\n\
                    +fn not_a_test() {}\n";
        let changed = changed_tests(diff);
        // A test whose body alone changed is named by its hunk (`git diff -W`).
        assert_eq!(
            changed,
            ["a_cut_off_reviewer", "a_tight_round", "body_only"]
        );
        assert_eq!(
            test_filter(None, &changed[..2]),
            "not package(yulius) | (package(yulius) & (not kind(test) | \
             test(/(^|::)(a_cut_off_reviewer|a_tight_round)$/)))"
        );
        // The product's own unit tests always run (review round 1 of fix 8).
        assert_eq!(
            test_filter(None, &[]),
            "not package(yulius) | (package(yulius) & (not kind(test) | none()))"
        );
        assert_eq!(
            test_filter(Some("test(/review_rounds::/)"), &changed),
            "not package(yulius) | (package(yulius) & (not kind(test) | test(/review_rounds::/)))"
        );
    }

    /// AC-20: missed, timed-out and not-reached mutants each keep the run from passing and are
    /// named; the summary line counts each kind.
    #[test]
    fn a_run_passes_only_when_every_mutant_was_caught() {
        let listed: Vec<String> = [
            "a.rs:1:1: m1",
            "a.rs:2:1: m2",
            "a.rs:3:1: m3",
            "a.rs:4:1: m4",
        ]
        .map(str::to_owned)
        .to_vec();
        let held = Outcomes::of(
            &listed,
            true,
            "a.rs:1:1: m1\na.rs:2:1: m2\na.rs:3:1: m3\n",
            "",
            "",
            "a.rs:4:1: m4\n",
        );
        assert!(held.held());
        let partial = Outcomes::of(
            &listed,
            true,
            "a.rs:1:1: m1\n",
            "a.rs:2:1: m2\n",
            "a.rs:3:1: m3\n",
            "",
        );
        assert!(!partial.held());
        assert_eq!(partial.unreached, ["a.rs:4:1: m4"]);
        assert_eq!(
            partial.summary(4, Duration::from_secs(1_800), Duration::from_secs(95)),
            "mutants: 4 listed, 1 caught, 1 missed, 1 timed out, 0 unviable, 1 not reached \
             (bound 30 min, took 95 s)"
        );
    }

    /// A fixture crate in a Git repository of its own, its first commit the base. The tests
    /// that use it need the pinned cargo-mutants on `PATH` (`scripts/dev` puts `.tools/bin`
    /// there).
    fn fixture(name: &str) -> Result<(std::path::PathBuf, String), Box<dyn std::error::Error>> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("xtask-mutants-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )?;
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn one() -> u32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one_is_one() {\n        assert_eq!(super::one(), 1);\n    }\n}\n",
        )?;
        let git = |args: &[&str]| -> Result<String, Box<dyn std::error::Error>> {
            Ok(super::git(&dir, args)?.trim().to_owned())
        };
        git(&["init", "-q", "-b", "main"])?;
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "add", "-A"])?;
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "base",
        ])?;
        let base = git(&["rev-parse", "HEAD"])?;
        Ok((dir, base))
    }

    fn commit(dir: &Path, path: &str, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::write(dir.join(path), text)?;
        super::git(dir, &["add", "-A"])?;
        super::git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "c",
            ],
        )?;
        Ok(())
    }

    /// AC-20 with the real cargo-mutants: a changed line no test holds is a missed mutant and
    /// the run does not pass; with its test it passes; a branch that changes no Rust code has
    /// nothing to mutate.
    #[test]
    fn a_real_run_finds_the_line_no_test_holds() -> Result<(), Box<dyn std::error::Error>> {
        let available = Command::new("cargo")
            .args(["mutants", "--version"])
            .output()
            .is_ok_and(|out| out.status.success());
        assert!(
            available,
            "cargo-mutants is not on PATH: run the tests through scripts/dev after scripts/bootstrap.py"
        );
        let settings = |base: &str| Settings {
            base: base.to_owned(),
            minutes: 10,
            mutant_seconds: 120,
            // The default jobs within a budget of 2 CPUs: the pinned tool takes the caps
            // (`--jobserver-tasks`) and the run's own call of `jobs_for` (SPEC-506, review round 2).
            jobs: None,
            budget: Budget {
                cpus: Some(2),
                memory_mb: None,
            },
            tests: None,
            seconds: None,
        };
        let lib = "pub fn one() -> u32 {\n    1\n}\n\npub fn two() -> u32 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one_is_one() {\n        assert_eq!(super::one(), 1);\n    }\n";
        let (dir, base) = fixture("missed")?;
        // Only the root package has mutations. A failure in a sibling's test must stop
        // the baseline before cargo-mutants can credit that failure to a mutant.
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml"))?;
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("{manifest}members = [\"tools/observer\"]\n"),
        )?;
        std::fs::write(dir.join(".gitignore"), "/target/\n")?;
        std::fs::create_dir_all(dir.join("tools/observer/src"))?;
        std::fs::write(
            dir.join("tools/observer/Cargo.toml"),
            "[package]\nname = \"observer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [dev-dependencies]\nfixture = { path = \"../..\" }\n",
        )?;
        std::fs::create_dir(dir.join(".cargo"))?;
        std::fs::write(dir.join(".cargo/mutants.toml"), "test_workspace = true\n")?;
        let observer = "tools/observer/src/lib.rs";
        std::fs::write(dir.join(observer), "#[test] fn peer_test() { panic!() }\n")?;
        commit(&dir, "src/lib.rs", &format!("{lib}}}\n"))?;
        let failed = run(&dir, "cargo", &settings(&base));
        assert!(
            failed
                .as_ref()
                .is_err_and(|err| err.to_string().contains("baseline")),
            "peer must fail the baseline: {failed:?}"
        );
        let log = std::fs::read_to_string(dir.join("target/mutants/mutants.out/log/baseline.log"))?;
        assert!(log.contains("peer_test"), "{log}");

        commit(
            &dir,
            observer,
            "#[test] fn peer_test() { assert_eq!(std::env::temp_dir(), std::path::Path::new(\"/tmp\")); }\n",
        )?;
        let outcomes = run(&dir, "cargo", &settings(&base))?.ok_or("nothing mutated")?;
        assert!(
            outcomes.missed.iter().any(|m| m.contains("two")),
            "{outcomes:?}"
        );
        assert!(!outcomes.held());
        commit(
            &dir,
            observer,
            "#[test] fn peer_test() { assert_eq!(fixture::two(), 2); }\n",
        )?;
        let outcomes = run(&dir, "cargo", &settings(&base))?.ok_or("nothing mutated")?;
        assert!(outcomes.held(), "{outcomes:?}");
        assert!(outcomes.caught.iter().any(|m| m.contains("two")));
        assert!(outcomes.baseline_passed);
        let (docs, docs_base) = fixture("docs")?;
        commit(&docs, "README.md", "words\n")?;
        assert_eq!(run(&docs, "cargo", &settings(&docs_base))?, None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&docs);
        // The bound ends a run and names the mutants it did not reach (review round 1 of fix 8).
        let (slow, slow_base) = fixture("bound")?;
        let mut lib = String::from("pub fn one() -> u32 {\n    1\n}\n");
        let mut tests = String::from("#[cfg(test)]\nmod tests {\n");
        for n in 2..8 {
            lib.push_str(&format!("pub fn f{n}() -> u32 {{\n    {n}\n}}\n"));
            tests.push_str(&format!(
                "    #[test]\n    fn f{n}_is() {{\n        std::thread::sleep(std::time::Duration::from_secs(1));\n        assert_eq!(super::f{n}(), {n});\n    }}\n"
            ));
        }
        commit(&slow, "src/lib.rs", &format!("{lib}\n{tests}}}\n"))?;
        let bounded = Settings {
            seconds: Some(3),
            ..settings(&slow_base)
        };
        let outcomes = run(&slow, "cargo", &bounded)?.ok_or("nothing mutated")?;
        assert!(!outcomes.unreached.is_empty(), "{outcomes:?}");
        assert!(!outcomes.held());
        assert!(
            std::fs::read_dir(slow.join("target/mutants"))?
                .all(|entry| entry
                    .is_ok_and(|e| !e.file_name().to_string_lossy().starts_with("mut-")))
        );
        let _ = std::fs::remove_dir_all(&slow);
        // A tree whose own tests fail is cargo-mutants' failure, said as such, not mutants
        // unreached (review round 1 of fix 8).
        let (broken, broken_base) = fixture("broken")?;
        commit(
            &broken,
            "src/lib.rs",
            "pub fn one() -> u32 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one_is_one() {\n        assert_eq!(super::one(), 1);\n    }\n}\n",
        )?;
        let failed = run(&broken, "cargo", &settings(&broken_base));
        assert!(
            failed
                .as_ref()
                .is_err_and(|err| err.to_string().contains("cargo-mutants failed")),
            "{failed:?}"
        );
        let _ = std::fs::remove_dir_all(&broken);
        Ok(())
    }

    #[test]
    fn a_stalled_command_cannot_reuse_the_last_campaign() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt as _;
        let (root, base) = fixture("stale")?;
        commit(&root, "src/lib.rs", "pub fn one() -> u32 { 2 }\n")?;
        let reports = root.join("target/mutants/mutants.out");
        std::fs::create_dir_all(&reports)?;
        std::fs::write(reports.join("caught.txt"), "variant\n")?;
        std::fs::write(
            reports.join("outcomes.json"),
            r#"{"outcomes":[{"scenario":"Baseline","summary":"Success"}]}"#,
        )?;
        let cargo = root.join("fake-cargo");
        // Finish on our own after a longer outer bound so the original unbounded wait also returns.
        std::fs::write(
            &cargo,
            "#!/bin/sh\ncase \" $* \" in *' --list '*) echo variant; exit 0;; esac\ntrap '' TERM\nsleep 12\ntouch overran\n",
        )?;
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o700))?;
        let mut settings = Settings::parse_with(&[], "dev", Budget::default())?;
        settings.base = base;
        settings.seconds = Some(1);
        let outcome = run(&root, cargo.to_str().ok_or("path")?, &settings)?.ok_or("no run")?;
        assert!(!outcome.held(), "previous caught.txt credited to this run");
        assert!(
            !root.join("overran").exists(),
            "TERM was ignored past shutdown grace"
        );
        std::fs::write(&cargo, "#!/bin/sh\ntrap '' TERM\nsleep 12\ntouch overran\n")?;
        assert!(run(&root, cargo.to_str().ok_or("path")?, &settings).is_err());
        assert!(
            !root.join("overran").exists(),
            "listing ignored the deadline"
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// Review round 1 of fix 8: cargo-mutants runs with no shared target directory and none of
    /// an outer nextest's settings, whatever the caller's environment holds.
    #[test]
    fn cargo_mutants_runs_with_a_target_directory_of_its_own() {
        let head = "0123456789abcdef0123456789abcdef01234567";
        let command = mutants_command("cargo", Path::new("."), head);
        // The copy cargo-mutants builds in has no `.git`: its build reports the checkout's
        // commit, as the gate's does, or the `yulius` crate's own tests that compare it fail
        // in the unmutated tree (SPEC-493 fix 10b's run).
        let commit = command
            .get_envs()
            .find(|(name, _)| *name == "YULIUS_BUILD_COMMIT")
            .and_then(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()));
        if std::env::var_os("YULIUS_BUILD_COMMIT").is_none() {
            assert_eq!(commit.as_deref(), Some(head));
        }
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
            assert!(removed.iter().any(|r| r == name), "{name}: {removed:?}");
        }
        assert!(selection(Path::new("d")).contains(&"--all-features".to_owned()));
        // What an outer nextest run set goes; the sandbox's CPU budget stays (review round 2).
        for name in [
            "NEXTEST",
            "NEXTEST_PROFILE",
            "NEXTEST_RUN_ID",
            "NEXTEST_BIN_EXE_xtask",
        ] {
            assert!(outer_nextest(name), "{name}");
        }
        assert!(!outer_nextest("NEXTEST_TEST_THREADS"));
    }

    #[test]
    fn the_settings_take_their_options() -> Result<(), String> {
        let args = [
            "--base",
            "origin/main",
            "--minutes",
            "5",
            "--jobs",
            "3",
            "--tests",
            "all()",
        ]
        .map(str::to_owned);
        let settings = Settings::parse(&args, "dev")?;
        assert_eq!(
            (
                settings.base.as_str(),
                settings.minutes,
                settings.jobs,
                settings.tests.as_deref()
            ),
            ("origin/main", 5, Some(3), Some("all()"))
        );
        assert_eq!(Settings::parse(&[], "dev")?.base, "origin/dev");
        assert!(Settings::parse(&["--minutes".to_owned(), "0".to_owned()], "dev").is_err());
        assert!(Settings::parse(&["--other".to_owned(), "1".to_owned()], "dev").is_err());
        Ok(())
    }

    /// `YULIUS_MEMORY_MB` and [`MOST_JOBS`]; `--jobs` still decides; with no budget it stays 2.
    #[test]
    fn the_jobs_follow_the_mutants_and_the_budget() -> Result<(), String> {
        let env = |cpus: Option<&'static str>, memory: Option<&'static str>| {
            move |name: &str| match name {
                "YULIUS_CPUS" => cpus.map(str::to_owned),
                "YULIUS_MEMORY_MB" => memory.map(str::to_owned),
                _ => None,
            }
        };
        let jobs = |args: &[&str], budget: Budget, listed: usize| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            Settings::parse_with(&args, "dev", budget).map(|s| s.jobs_for(listed))
        };
        let session = Budget::read(env(Some("22"), Some("232566")));
        // A fix's few mutants: no more than before (7 jobs finished 8 no sooner than 2).
        assert_eq!(jobs(&[], session, 8)?, 2);
        assert_eq!(jobs(&[], session, 36)?, 5);
        // A whole change: a third of the CPUs.
        assert_eq!(jobs(&[], session, 260)?, 7);
        assert_eq!(jobs(&[], Budget::read(env(Some("64"), None)), 260)?, 7);
        // A small budget still gets 2.
        assert_eq!(jobs(&[], Budget::read(env(Some("4"), None)), 260)?, 2);
        assert_eq!(jobs(&[], Budget::read(env(Some("12"), None)), 260)?, 4);
        // A small host's memory bounds it, down to one job.
        assert_eq!(
            jobs(&[], Budget::read(env(Some("22"), Some("30000"))), 260)?,
            2
        );
        assert_eq!(
            jobs(&[], Budget::read(env(Some("22"), Some("8000"))), 260)?,
            1
        );
        // No budget, or one that does not parse: 2, as before.
        assert_eq!(jobs(&[], Budget::read(env(None, None)), 260)?, 2);
        assert_eq!(jobs(&[], Budget::read(env(Some("lots"), None)), 260)?, 2);
        // `--jobs` decides.
        assert_eq!(jobs(&["--jobs", "3"], session, 8)?, 3);
        // The run stays within the CPUs: the jobserver gets them, each job's tests its share.
        assert_eq!(
            budget_caps(session, 7),
            (
                vec!["--jobserver-tasks".to_owned(), "22".to_owned()],
                vec![
                    ("NEXTEST_TEST_THREADS".to_owned(), "3".to_owned()),
                    ("RUST_TEST_THREADS".to_owned(), "3".to_owned()),
                ]
            )
        );
        assert_eq!(budget_caps(Budget::default(), 2), (Vec::new(), Vec::new()));
        // More jobs than CPUs: each job's tests still get a thread.
        assert_eq!(
            budget_caps(session, 30).1[0],
            ("NEXTEST_TEST_THREADS".to_owned(), "1".to_owned())
        );
        // The budget is read from those two names and no other.
        assert_eq!(
            session,
            Budget {
                cpus: Some(22),
                memory_mb: Some(232_566)
            }
        );
        Ok(())
    }
}
