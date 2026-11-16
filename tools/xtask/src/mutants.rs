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
}

/// The names of the test functions a diff adds or changes in the product's test binary
/// (`crates/yulius/tests/`): a `fn name(` on an added line there.
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
        let Some(added) = line.strip_prefix('+') else {
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

/// The nextest filter each mutant is tested with: every package's tests but the product's, and
/// of the product's those named by `tests` (a filter of the caller's) or the `changed` ones.
#[must_use]
pub fn test_filter(tests: Option<&str>, changed: &[String]) -> String {
    let product = match (tests, changed) {
        (Some(filter), _) => filter.to_owned(),
        (None, []) => "none()".to_owned(),
        (None, names) => format!("test(/({})$/)", names.join("|")),
    };
    format!("not package(yulius) | (package(yulius) & ({product}))")
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
    pub fn summary(&self, listed: usize, minutes: u64, took: Duration) -> String {
        format!(
            "mutants: {listed} listed, {} caught, {} missed, {} timed out, {} unviable, {} not \
             reached (bound {minutes} min, took {} s)",
            self.caught.len(),
            self.missed.len(),
            self.timeout.len(),
            self.unviable.len(),
            self.unreached.len(),
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
fn selection(diff: &Path) -> Vec<String> {
    let mut args = vec![
        "mutants".to_owned(),
        "--in-diff".to_owned(),
        diff.display().to_string(),
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
    let diff = git(root, &["diff", base.trim(), "HEAD", "--", "*.rs"])?;
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
        let out = Command::new(cargo)
            .current_dir(root)
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
        .then(|| test_filter(settings.tests.as_deref(), &changed_tests(&diff)));
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
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
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
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    let bound = Duration::from_secs(settings.minutes * 60);
    while child.try_wait()?.is_none() {
        if started.elapsed() >= bound {
            println!(
                "The bound of {} min is reached: the run ends here.",
                settings.minutes
            );
            // The whole group: cargo-mutants and the builds and tests it started.
            let _ = Command::new("kill")
                .args(["-TERM", &format!("-{}", child.id())])
                .status();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
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
        outcomes.summary(listed.len(), settings.minutes, started.elapsed())
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
                    +++ b/crates/yulius-app/src/review.rs\n\
                    +fn not_a_test() {}\n";
        let changed = changed_tests(diff);
        assert_eq!(changed, ["a_cut_off_reviewer", "a_tight_round"]);
        assert_eq!(
            test_filter(None, &changed),
            "not package(yulius) | (package(yulius) & (test(/(a_cut_off_reviewer|a_tight_round)$/)))"
        );
        assert_eq!(
            test_filter(None, &[]),
            "not package(yulius) | (package(yulius) & (none()))"
        );
        assert_eq!(
            test_filter(Some("test(/review_rounds::/)"), &changed),
            "not package(yulius) | (package(yulius) & (test(/review_rounds::/)))"
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
            partial.summary(4, 30, Duration::from_secs(95)),
            "mutants: 4 listed, 1 caught, 1 missed, 1 timed out, 0 unviable, 1 not reached \
             (bound 30 min, took 95 s)"
        );
    }

    /// A fixture crate in a Git repository of its own, its first commit the base; `None` when
    /// the pinned cargo-mutants is not on `PATH` (run through `scripts/dev`, which puts
    /// `.tools/bin` there).
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
        Ok(())
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
