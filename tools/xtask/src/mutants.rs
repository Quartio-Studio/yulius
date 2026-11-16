//! `scripts/dev mutants` (SPEC-493 MUT-001): one bounded mutation run of the Rust lines the
//! branch changed against the integration branch, with the pinned `cargo-mutants`. Each mutant
//! is tested with nextest across the workspace; the product's own test binary (package `yulius`,
//! the slowest by far) only with the tests the branch added or changed, the ones meant to hold
//! its lines, unless `--tests` names others. A bound on the whole run ends it and names the
//! mutants it did not reach; the last line is the summary for the spec's Evidence.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

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
    /// Mutants tested at once (each in a build directory of its own).
    pub jobs: u32,
    /// A nextest filter for the `yulius` package's tests in place of the changed ones.
    pub tests: Option<String>,
    /// The bound in seconds in place of `minutes` (tests).
    pub seconds: Option<u64>,
}

impl Settings {
    /// Parses `--base REF`, `--minutes N`, `--mutant-seconds N`, `--jobs N`, `--tests FILTER`.
    ///
    /// # Errors
    /// An unknown option, a missing or malformed value.
    pub fn parse(args: &[String], integration: &str) -> Result<Self, String> {
        let mut settings = Self {
            base: format!("origin/{integration}"),
            minutes: 30,
            mutant_seconds: 300,
            jobs: 2,
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
                "--jobs" => settings.jobs = u32::try_from(number()?).unwrap_or(u32::MAX),
                "--tests" => settings.tests = Some(value.clone()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(settings)
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
        self.missed.is_empty() && self.timeout.is_empty() && self.unreached.is_empty()
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

/// The cargo-mutants arguments that choose the mutants: those in `diff`, outside [`EXCLUDED`].
/// A cargo-mutants command in `root`, in an environment of its own: no shared target directory
/// (with more than one job, copies writing one target directory test each other's builds and
/// report mutants caught or missed wrongly; review round 1 of fix 8) and none of an outer
/// nextest's settings (a profile the tree may not have).
fn mutants_command(cargo: &str, root: &Path) -> Command {
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("NEXTEST") {
            command.env_remove(name);
        }
    }
    command
}

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

/// Runs the campaign in the repository at `root`: `None` when the branch changed no Rust code
/// with a mutant in it.
///
/// # Errors
/// Git or cargo-mutants could not be run.
pub fn run(root: &Path, cargo: &str, settings: &Settings) -> io::Result<Option<Outcomes>> {
    let base = git(root, &["merge-base", &settings.base, "HEAD"])?;
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
    let diff_file = dir.join("branch.diff");
    std::fs::write(&diff_file, &diff)?;
    let listed: Vec<String> = {
        let out = mutants_command(cargo, root)
            .args(selection(&diff_file))
            .arg("--list")
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "cargo mutants --list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    };
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
    println!(
        "{} mutants in the changed lines; each tested with nextest ({}), {} at once, \
         {} s each at most, {} min in all.",
        listed.len(),
        filter.as_deref().unwrap_or("all tests"),
        settings.jobs,
        settings.mutant_seconds,
        settings.minutes
    );
    let started = Instant::now();
    let mut command = mutants_command(cargo, root);
    command
        .args(selection(&diff_file))
        .args([
            "--test-tool",
            "nextest",
            "--test-workspace",
            "true",
            "--no-shuffle",
        ])
        .args(["--jobs", &settings.jobs.to_string()])
        .args(["--timeout", &settings.mutant_seconds.to_string()])
        .args(["--output", &dir.display().to_string()])
        .args(
            filter
                .iter()
                .flat_map(|filter| ["--", "-E", filter.as_str()]),
        );
    let mut child = command.spawn()?;
    let bound = settings.bound();
    let mut bounded = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= bound {
            println!(
                "The bound of {} s is reached: the run ends here.",
                bound.as_secs()
            );
            bounded = true;
            // cargo-mutants stops the builds and tests it started when it is asked to end.
            #[cfg(unix)]
            let _ = Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status();
            #[cfg(not(unix))]
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    // 0: all caught; 2: some missed; 3: some timed out. Anything else, unless the bound ended
    // it, is cargo-mutants' own failure (a baseline that does not pass), not mutants unreached.
    if !bounded && !matches!(status.code(), Some(0 | 2 | 3)) {
        return Err(io::Error::other(format!(
            "cargo-mutants failed ({status}); its log: {}",
            dir.join("mutants.out/debug.log").display()
        )));
    }
    let read = |kind: &str| {
        std::fs::read_to_string(dir.join("mutants.out").join(format!("{kind}.txt")))
            .unwrap_or_default()
    };
    let outcomes = Outcomes::of(
        &listed,
        &read("caught"),
        &read("missed"),
        &read("timeout"),
        &read("unviable"),
    );
    for (what, mutants) in [
        ("Missed (no test failed)", &outcomes.missed),
        ("Timed out", &outcomes.timeout),
        ("Not reached within the bound", &outcomes.unreached),
    ] {
        if !mutants.is_empty() {
            println!("{what}:");
            for mutant in mutants {
                println!("  {mutant}");
            }
        }
    }
    println!(
        "{}",
        outcomes.summary(listed.len(), settings.bound(), started.elapsed())
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "a.rs:1:1: m1\na.rs:2:1: m2\na.rs:3:1: m3\n",
            "",
            "",
            "a.rs:4:1: m4\n",
        );
        assert!(held.held());
        let partial = Outcomes::of(
            &listed,
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
        let dir = std::env::temp_dir().join(format!("xtask-mutants-{}-{name}", std::process::id()));
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
            jobs: 1,
            tests: None,
            seconds: None,
        };
        let lib = "pub fn one() -> u32 {\n    1\n}\n\npub fn two() -> u32 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one_is_one() {\n        assert_eq!(super::one(), 1);\n    }\n";
        let (dir, base) = fixture("missed")?;
        commit(&dir, "src/lib.rs", &format!("{lib}}}\n"))?;
        let outcomes = run(&dir, "cargo", &settings(&base))?.ok_or("nothing mutated")?;
        assert!(
            outcomes.missed.iter().any(|m| m.contains("two")),
            "{outcomes:?}"
        );
        assert!(!outcomes.held());
        let held = format!(
            "{lib}\n    #[test]\n    fn two_is_two() {{\n        assert_eq!(super::two(), 2);\n    }}\n}}\n"
        );
        commit(&dir, "src/lib.rs", &held)?;
        let outcomes = run(&dir, "cargo", &settings(&base))?.ok_or("nothing mutated")?;
        assert!(outcomes.held(), "{outcomes:?}");
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

    /// Review round 1 of fix 8: cargo-mutants runs with no shared target directory and none of
    /// an outer nextest's settings, whatever the caller's environment holds.
    #[test]
    fn cargo_mutants_runs_with_a_target_directory_of_its_own() {
        let command = mutants_command("cargo", Path::new("."));
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
            assert!(removed.iter().any(|r| r == name), "{name}: {removed:?}");
        }
        assert!(selection(Path::new("d")).contains(&"--all-features".to_owned()));
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
            ("origin/main", 5, 3, Some("all()"))
        );
        assert_eq!(Settings::parse(&[], "dev")?.base, "origin/dev");
        assert!(Settings::parse(&["--minutes".to_owned(), "0".to_owned()], "dev").is_err());
        assert!(Settings::parse(&["--other".to_owned(), "1".to_owned()], "dev").is_err());
        Ok(())
    }
}
