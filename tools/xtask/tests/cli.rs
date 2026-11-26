//! Gate runner contracts: discoverability and failure propagation.
use std::process::Command;

#[test]
fn help_is_available_without_installed_optional_tools() -> std::io::Result<()> {
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("--help")
        .output()?;
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("ci"));
    Ok(())
}

#[test]
fn unknown_command_is_a_usage_error() -> std::io::Result<()> {
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("c1")
        .output()?;
    assert_eq!(result.status.code(), Some(2));
    Ok(())
}

#[test]
fn missing_prerequisite_fails_ci_without_running_later_gates() -> std::io::Result<()> {
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .env("PYTHON", "yulius-nonexistent-python-for-test")
        .arg("ci")
        .output()?;
    assert_eq!(result.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("clippy"));
    Ok(())
}

#[test]
fn security_is_listed_and_needs_a_vidoc_key() -> std::io::Result<()> {
    let help = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("--help")
        .output()?;
    assert!(String::from_utf8_lossy(&help.stdout).contains("security"));
    // The scanner reads committed config before checking the key. Give both commands a
    // repository of their own, including when cargo-mutants builds xtask in a Git-free copy.
    let repo = tempfile::tempdir()?;
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(repo.path())
            .status()?
            .success()
    );
    let security = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .env("GIT_DIR", repo.path().join(".git"))
            .env("GIT_WORK_TREE", repo.path())
            .env_remove("VIDOC_API_KEY")
            .env_remove("YULIUS_VIDOC")
            .arg("security")
            .args(args)
            .output()
    };
    let result = security(&[])?;
    // Exit 2 is "not run", distinct from findings (1): the gate's code passes through.
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("No Vidoc key"));
    // SPEC-195: its options reach the scan (`scripts/dev security --all` is the broad scan).
    let broad = security(&["--all"])?;
    assert_eq!(broad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&broad.stdout).contains("scripts/vidoc_scan.py --all"));
    assert!(String::from_utf8_lossy(&broad.stderr).contains("No Vidoc key"));
    Ok(())
}

/// A gate run against recording stubs of `cargo` and `python` (SPEC-506 FST-006, FST-008, review
/// round 1 of pull request 3): each stub writes its arguments to a file, prints a line, and fails
/// when its arguments contain `fail`.
#[cfg(unix)]
struct Stubs {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl Stubs {
    fn new(name: &str) -> std::io::Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("xtask-gates-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        for tool in ["cargo", "python"] {
            let path = dir.join(tool);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho \"{tool} $*\" >> \"$XT_RECORD\"\necho \"stub {tool} $*\"\n\
                     if [ -n \"$XT_HANDSHAKE\" ]; then other=''; case \"{tool} $*\" in \
                     *'-m unittest'*) other='cargo check';; 'cargo check'*) \
                     other='python -m unittest';; esac; if [ -n \"$other\" ]; then n=0; \
                     until grep -q \"^$other\" \"$XT_RECORD\"; do n=$((n+1)); \
                     if [ $n -gt 100 ]; then echo \"HANDSHAKE TIMEOUT {tool}\"; exit 3; fi; \
                     sleep 0.1; done; fi; fi\n\
                     for f in \"$XT_FAIL\" \"$XT_FAIL2\"; do if [ -n \"$f\" ]; then \
                     case \"{tool} $*\" in *\"$f\"*) echo \"STUB FAILURE of {tool} $*\"; \
                     exit 1;; esac; fi; done\nexit 0\n"
                ),
            )?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(Self { dir })
    }

    /// Runs `xtask <task>` with the stubs, failing the stub call whose words contain `fail`;
    /// returns its exit code, its output and the calls in the order made.
    fn run(&self, task: &str, fail: &str) -> std::io::Result<(Option<i32>, String, Vec<String>)> {
        self.run_with(task, &[fail], false)
    }

    /// [`Self::run`]; with `handshake`, the checker's tests and `cargo check` each wait (10 s at
    /// most) until the other has started, so they pass only when they run at the same time.
    fn run_with(
        &self,
        task: &str,
        fails: &[&str],
        handshake: bool,
    ) -> std::io::Result<(Option<i32>, String, Vec<String>)> {
        let record = self.dir.join("record");
        let _ = std::fs::remove_file(&record);
        std::fs::write(&record, "")?;
        let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .env("XT_HANDSHAKE", if handshake { "1" } else { "" })
            .env("CARGO", self.dir.join("cargo"))
            .env("PYTHON", self.dir.join("python"))
            .env("XT_RECORD", &record)
            .env("XT_FAIL", fails.first().copied().unwrap_or_default())
            .env("XT_FAIL2", fails.get(1).copied().unwrap_or_default())
            .env("YULIUS_CI", "1")
            .env_remove("YULIUS_CI_QUARANTINE")
            .arg(task)
            .output()?;
        let calls = std::fs::read_to_string(&record)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                // The tool and its first two words name a call; macOS's Clippy names its target.
                let words: Vec<&str> = line.split_whitespace().collect();
                let mut call = words.iter().take(3).copied().collect::<Vec<_>>().join(" ");
                if line.contains("--target") {
                    call.push_str(" --target");
                }
                call
            })
            .collect();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.code(), text, calls))
    }
}

#[cfg(unix)]
impl Drop for Stubs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The last line of a gate's output: the one that says why it failed.
#[cfg(unix)]
fn last_line(text: &str) -> &str {
    text.trim_end().lines().last().unwrap_or_default()
}

#[cfg(unix)]
fn sorted(mut calls: Vec<String>) -> Vec<String> {
    calls.sort();
    calls
}

/// SPEC-506 FST-006, FST-008 (review round 1 of pull request 3): `ci` and `checks` run the old
/// gate's commands, each once: `quick` (contracts, format) right after `doctor`, the checker's
/// tests beside the Rust gates; `check` and `lint` alone still run what `quick` holds.
#[cfg(unix)]
#[test]
fn the_gates_run_each_command_once_with_quick_first() -> std::io::Result<()> {
    let stubs = Stubs::new("order")?;
    let rust = [
        "cargo check --locked",
        "cargo clippy --locked",
        "cargo clippy --locked --target",
        "cargo deny --locked",
        "cargo doc --locked",
        "cargo shear --locked",
    ];
    let (code, text, calls) = stubs.run("checks", "")?;
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        calls[..3],
        [
            "python scripts/check_repository.py --doctor",
            "python scripts/check_repository.py",
            "cargo fmt --all",
        ],
        "doctor, then quick, before anything else: {calls:?}"
    );
    let mut checks: Vec<String> = calls[..3].to_vec();
    checks.extend(["python -m unittest".to_owned()]);
    checks.extend(rust.iter().map(|c| (*c).to_owned()));
    assert_eq!(sorted(calls), sorted(checks.clone()));
    let (code, text, calls) = stubs.run("ci", "")?;
    assert_eq!(code, Some(0), "{text}");
    let mut ci = checks;
    ci.extend([
        "cargo nextest run".to_owned(),
        "python scripts/doctests.py".to_owned(),
    ]);
    assert_eq!(sorted(calls), sorted(ci));
    let (code, _, calls) = stubs.run("check", "")?;
    assert_eq!(code, Some(0));
    assert_eq!(
        sorted(calls),
        sorted(vec![
            "python scripts/check_repository.py".to_owned(),
            "python -m unittest".to_owned(),
            "cargo check --locked".to_owned(),
        ])
    );
    let (code, _, calls) = stubs.run("lint", "")?;
    assert_eq!(code, Some(0));
    assert_eq!(calls, ["cargo fmt --all", "cargo clippy --locked"]);
    Ok(())
}

/// SPEC-506 FST-006, FST-008 (review round 1 of pull request 3): a failed format check ends the
/// gate before the build and the checker's tests start; a failed checker test fails it after the
/// Rust gates, its output in a stage of its own at the end; a failed Rust gate is named.
#[cfg(unix)]
#[test]
fn a_failure_fails_the_gate_and_says_where() -> std::io::Result<()> {
    let stubs = Stubs::new("failures")?;
    let (code, text, calls) = stubs.run("checks", "fmt --all -- --check")?;
    assert_eq!(code, Some(1), "{text}");
    assert!(
        !calls
            .iter()
            .any(|c| c == "python -m unittest" || c == "cargo check --locked"),
        "nothing after quick started: {calls:?}"
    );
    let (code, text, calls) = stubs.run("checks", "unittest")?;
    assert_eq!(code, Some(1), "{text}");
    assert!(calls.iter().any(|c| c == "cargo doc --locked"), "{calls:?}");
    let stage = text.find("::yulius-stage checker-tests").unwrap_or(0);
    let failure = text.find("STUB FAILURE of python -m unittest").unwrap_or(0);
    let docs = text.find("stub cargo doc").unwrap_or(usize::MAX);
    assert!(
        docs < stage && stage < failure,
        "the checker's output comes in its own stage after the Rust gates: {text}"
    );
    let last = last_line(&text);
    assert!(
        last.starts_with("Gate failed:") && last.contains("-m unittest"),
        "{text}"
    );
    let (code, text, _) = stubs.run(
        "checks",
        "clippy --locked --workspace --all-targets --all-features -- -D",
    )?;
    assert_eq!(code, Some(1), "{text}");
    let last = last_line(&text);
    assert!(
        last.starts_with("Gate failed:") && last.contains("clippy --locked"),
        "{text}"
    );
    // Both fail: the Rust gate is named first.
    let (code, text, _) = stubs.run_with(
        "checks",
        &[
            "clippy --locked --workspace --all-targets --all-features -- -D",
            "-m unittest",
        ],
        false,
    )?;
    assert_eq!(code, Some(1), "{text}");
    let last = last_line(&text);
    assert!(
        last.starts_with("Gate failed:") && last.contains("clippy --locked"),
        "{text}"
    );
    Ok(())
}

/// SPEC-506 FST-008 (review round 2 of pull request 3): in `checks`, `ci` and `check` the
/// checker's tests and `cargo check` run at the same time (each stub waits for the other to start;
/// in a row the first times out), and the checker's tests' stage opens when the Rust gates end.
#[cfg(unix)]
#[test]
fn the_checkers_tests_run_beside_the_build() -> std::io::Result<()> {
    let stubs = Stubs::new("beside")?;
    for task in ["checks", "ci", "check"] {
        let (code, text, _) = stubs.run_with(task, &[], true)?;
        assert_eq!(code, Some(0), "{task}: {text}");
        assert!(!text.contains("HANDSHAKE TIMEOUT"), "{task}: {text}");
    }
    let (_, text, _) = stubs.run("checks", "")?;
    let docs = text.find("stub cargo doc").unwrap_or(usize::MAX);
    let stage = text.find("::yulius-stage checker-tests").unwrap_or(0);
    assert!(docs < stage, "{text}");
    Ok(())
}

/// The pull request's `quality` job compiles nothing (`lint` does): the tool check, the checks of
/// seconds and the checker's tests, each in its stage; a failure stops it there.
#[cfg(unix)]
#[test]
fn quality_runs_the_repository_checks_without_a_compile() -> std::io::Result<()> {
    let stubs = Stubs::new("quality")?;
    let (code, text, calls) = stubs.run("quality", "")?;
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(
        calls,
        [
            "python scripts/check_repository.py --doctor",
            "python scripts/check_repository.py",
            "cargo fmt --all",
            "python -m unittest",
        ]
    );
    let stages: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("::yulius-stage"))
        .collect();
    assert_eq!(
        stages,
        [
            "::yulius-stage doctor",
            "::yulius-stage quick",
            "::yulius-stage checker-tests",
        ]
    );
    let (code, text, calls) = stubs.run("quality", "fmt --all -- --check")?;
    assert_eq!(code, Some(1), "{text}");
    assert!(
        !calls.iter().any(|c| c == "python -m unittest"),
        "{calls:?}"
    );
    Ok(())
}

/// A CI job runs one gate (`scripts/dev lint`): the gate names its stage first.
#[cfg(unix)]
#[test]
fn a_single_gate_names_its_stage() -> std::io::Result<()> {
    let stubs = Stubs::new("stage")?;
    for gate in ["lint", "macos", "docs", "deps", "doctor", "test"] {
        let (code, text, _) = stubs.run(gate, "")?;
        assert_eq!(code, Some(0), "{gate}: {text}");
        assert_eq!(
            text.lines().next(),
            Some(format!("::yulius-stage {gate}").as_str()),
            "{text}"
        );
        assert_eq!(text.matches("::yulius-stage").count(), 1, "{text}");
    }
    let gates = [
        "doctor", "quick", "check", "lint", "macos", "test", "deps", "docs",
    ];
    for (task, expected) in [("ci", gates.to_vec()), ("checks", gates.to_vec())] {
        let mut expected: Vec<String> = expected
            .into_iter()
            .filter(|gate| task == "ci" || *gate != "test")
            .chain(["checker-tests"])
            .map(|gate| format!("::yulius-stage {gate}"))
            .collect();
        let (code, text, _) = stubs.run(task, "")?;
        assert_eq!(code, Some(0), "{task}: {text}");
        let stages: Vec<String> = text
            .lines()
            .filter(|line| line.starts_with("::yulius-stage"))
            .map(str::to_owned)
            .collect();
        expected.dedup();
        assert_eq!(stages, expected, "{task}");
    }
    Ok(())
}

/// SPEC-497 OFC-002: the CI test job lists this head's test binaries, runs the tests without the
/// quarantined ones, then the doctests.
#[cfg(unix)]
#[test]
fn the_tests_leave_the_quarantined_ones_out_and_run_the_doctests() -> std::io::Result<()> {
    let stubs = Stubs::new("quarantine")?;
    let record = stubs.dir.join("record");
    std::fs::write(&record, "")?;
    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("test")
        .env("CARGO", stubs.dir.join("cargo"))
        .env("PYTHON", stubs.dir.join("python"))
        .env("XT_RECORD", &record)
        .env("YULIUS_CI_QUARANTINE", "flaky_test")
        .output()?;
    assert!(out.status.success(), "{out:?}");
    let calls: Vec<String> = std::fs::read_to_string(&record)?
        .lines()
        .map(str::to_owned)
        .collect();
    let flags = "--locked --workspace --all-features --profile ci";
    assert_eq!(
        calls,
        [
            format!(
                "cargo nextest list {flags} --list-type binaries-only --message-format oneline"
            ),
            format!("cargo nextest run {flags} -E not (test(=flaky_test))"),
            "python scripts/doctests.py".to_owned(),
        ]
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_independent_docs_and_test_jobs_enter_through_the_pinned_dev_runner() -> std::io::Result<()> {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::Path;

    let home = tempfile::tempdir()?;
    let system = home.path().join("system-bin");
    let pin = home.path().join("pinned-bin");
    let cargo_bin = home.path().join(".cargo/bin");
    for dir in [&system, &pin, &cargo_bin] {
        std::fs::create_dir_all(dir)?;
    }
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()?;
    symlink(
        String::from_utf8_lossy(&python.stdout).trim(),
        system.join("python3"),
    )?;
    let write_tool = |path: &Path, script| {
        std::fs::write(path, script)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    };
    write_tool(
        &cargo_bin.join("rustup"),
        r#"#!/bin/sh
case "$1" in
    which) echo "$PIN/rustc" ;;
    run) shift 2; exec "$@" ;;
    *) exit 2 ;;
esac
"#,
    )?;
    write_tool(
        &pin.join("cargo"),
        r#"#!/bin/sh
if [ "$1" = xtask ]; then shift; exec "$XTASK_BIN" "$@"; fi
echo "$*" >> "$CALLS"
if [ "$1 $2" = "nextest run" ] && [ "$YULIUS_CI_RUN" = fail ]; then exit 1; fi
if [ "$1" = metadata ]; then echo '{"packages": [], "workspace_members": []}'; fi
"#,
    )?;
    let calls = home.path().join("calls");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (gate, run, expected) in [("docs", "ok", 0), ("test", "ok", 0), ("test", "fail", 1)] {
        let out = Command::new(root.join("scripts/dev"))
            .arg(gate)
            .env("HOME", home.path())
            .env("PATH", &system) // CI's initial path contains neither rustup nor Cargo.
            .env("PIN", &pin)
            .env("CALLS", &calls)
            .env("XTASK_BIN", env!("CARGO_BIN_EXE_xtask"))
            .env("YULIUS_CI_RUN", run)
            .env("CARGO_INCREMENTAL", "1")
            // Under Yulius CI scripts/dev would bootstrap this fake home for real.
            .env_remove("YULIUS_CI")
            .env_remove("CARGO")
            .env_remove("PYTHON")
            .env_remove("YULIUS_CI_QUARANTINE")
            .output()?;
        assert_eq!(out.status.code(), Some(expected), "{gate}: {out:?}");
    }
    let calls = std::fs::read_to_string(calls)?;
    for command in [
        "doc --locked --workspace --all-features --no-deps",
        "nextest run --locked --workspace --all-features --profile ci",
    ] {
        assert!(calls.contains(command), "{calls}");
    }
    Ok(())
}
