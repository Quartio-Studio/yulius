//! Reproducible local and CI quality gates. This is tooling, not the Yulius product.

use std::io::Write as _;
use std::{env, io, path::Path, process::Command, process::ExitCode};

mod mutants;

const HELP: &str = "Yulius engineering commands (scripts/dev <command>)
  doctor    Verify pinned compiler and repository-local tool versions
  quick     The checks of seconds: repository contracts and Rust formatting (run it before each
            commit you publish; ci and checks run it first)
  check     Validate repository contracts and compile every target
  fmt       Format Rust sources
  lint      Check formatting and Clippy warnings
  macos     Compile every target for macOS (aarch64-apple-darwin) with Clippy warnings denied
  test      Run nextest and documentation tests; in a CI matrix job valued `i/n`
            ($YULIUS_CI_MATRIX) only shard i of n, the doctests in shard 1
  deps      Check advisories, licenses, sources, and unused dependencies
  ci        Run all required gates, failing on the first failed command (the checker's tests,
            beside the Rust gates, fail it at its end)
  checks    Run every gate of ci but test (Yulius CI runs the tests apart, in shards)
  security  Scan the branch's changes with Vidoc (vidoc-1), failing on open critical or high findings
            and on open medium ones in the changed lines;
            --all scans the whole repository (needs a Vidoc key; not in ci)
  mutants   On request, mutation-test changed Rust lines against --base REF (default origin/dev).
            --minutes N, --mutant-seconds N, --jobs N and --tests FILTER configure the campaign.
            The final line reports its baseline, test filter, budget and outcomes";

/// The line that names a stage of a Yulius CI job (SPEC-137): Yulius times the stage from when
/// it reads this line until the next one or the job's end.
fn stage_marker(name: &str) -> String {
    format!("::yulius-stage {name}")
}

/// Names the gate that starts now, when the job runs under Yulius CI (`YULIUS_CI=1`).
fn stage(name: &str) {
    if env::var_os("YULIUS_CI").is_some() {
        println!("{}", stage_marker(name));
    }
}

/// The macOS target the gate compiles for (ADR-0169): Yulius CI runs on Linux only.
const MACOS_TARGET: &str = "aarch64-apple-darwin";

/// What the macOS check sets for Cargo: elsewhere than on a Mac, the C that dependencies vendor
/// (ring, SQLite) is compiled and archived by the pinned zig through `scripts/zig_macos.py`,
/// which bootstrap links as `.tools/bin/macos-cc` and `macos-ar`.
fn macos_env(host_is_macos: bool, tools: &Path) -> Vec<(String, String)> {
    if host_is_macos {
        return Vec::new();
    }
    let suffix = MACOS_TARGET.replace('-', "_");
    vec![
        (
            format!("CC_{suffix}"),
            tools.join("macos-cc").display().to_string(),
        ),
        (
            format!("AR_{suffix}"),
            tools.join("macos-ar").display().to_string(),
        ),
    ]
}

fn run(program: &str, args: &[&str]) -> io::Result<()> {
    run_with(program, args, &[])
}

fn run_with(program: &str, args: &[&str], envs: &[(String, String)]) -> io::Result<()> {
    println!("+ {program} {}", args.join(" "));
    let mut command = Command::new(program);
    command.args(args);
    command.envs(envs.iter().map(|(name, value)| (name, value)));
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{program} {} failed: {status}",
            args.join(" ")
        )))
    }
}

/// The gates of `ci` (with the tests) or `checks` (without), in order: `quick`, the checks of
/// seconds, right after `doctor`, so that a format or contract failure ends a CI run in seconds
/// rather than after the build (SPEC-506 FST-006).
fn gate_sequence(with_tests: bool) -> Vec<&'static str> {
    ["doctor", "quick", "check", "lint", "macos", "test", "deps"]
        .into_iter()
        .filter(|gate| with_tests || *gate != "test")
        .collect()
}

/// The repository checker's own tests' arguments.
const CHECKER_TESTS: [&str; 7] = [
    "-m",
    "unittest",
    "discover",
    "-s",
    "scripts",
    "-p",
    "test_*.py",
];

/// Starts the repository checker's own tests and keeps their output. They wait on local servers
/// and timers (3 CPU-seconds of user time, 7 with the system's, in 68 s), so they run beside the
/// Rust gates, which need the CPUs: those gates then take the longer of the two instead of both,
/// for no more CPU (SPEC-506 FST-008). A line names them when they start.
fn checker_tests(python: &str) -> io::Result<std::process::Output> {
    println!(
        "+ {python} {} (beside the Rust gates; its output follows them)",
        CHECKER_TESTS.join(" ")
    );
    Command::new(python).args(CHECKER_TESTS).output()
}

/// Prints the checker's tests' output in one piece and says how they ended.
fn checker_report(python: &str, out: io::Result<std::process::Output>) -> io::Result<()> {
    let out = out?;
    let mut stdout = io::stdout().lock();
    let mut text = format!("+ {python} {} (its output)\n", CHECKER_TESTS.join(" ")).into_bytes();
    text.extend_from_slice(&out.stdout);
    text.extend_from_slice(&out.stderr);
    stdout.write_all(&text)?;
    stdout.flush()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{python} {} failed: {}",
            CHECKER_TESTS.join(" "),
            out.status
        )))
    }
}

/// Runs the gates of `ci` (with the tests) or `checks` (without), naming each stage: `doctor`
/// and `quick` first, alone, so a failure there ends the run in seconds; then the checker's
/// tests beside all the rest, and their output in a stage of their own at the end (SPEC-506
/// FST-006, FST-008). A Rust gate's failure is named first; a failing checker test no longer
/// stops the Rust gates.
fn gates(with_tests: bool, cargo: &str, python: &str) -> io::Result<()> {
    let gates = gate_sequence(with_tests);
    let (first, rest) = gates.split_at(2);
    for gate in first {
        stage(gate);
        execute_in(gate, true)?;
    }
    let (tests, rest) = beside(
        || checker_tests(python),
        || -> io::Result<()> {
            for gate in rest {
                // `test` names its own stage.
                if *gate != "test" {
                    stage(gate);
                }
                // The wait for the checker's tests is their stage's, not the last Rust gate's.
                if let Err(error) = execute_in(gate, true) {
                    stage("checker-tests");
                    return Err(error);
                }
            }
            stage("docs");
            let docs = docs(cargo);
            stage("checker-tests");
            docs
        },
    );
    let tests = checker_report(python, tests);
    rest.and(tests)
}

/// Runs `first` and `second` at once and returns both results (SPEC-506 FST-008).
fn beside<A: Send, B>(first: impl FnOnce() -> A + Send, second: impl FnOnce() -> B) -> (A, B) {
    std::thread::scope(|scope| {
        let first = scope.spawn(first);
        let second = second();
        let first = first
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (first, second)
    })
}

fn execute(task: &str) -> io::Result<()> {
    execute_in(task, false)
}

/// Runs `task`; `after_quick` leaves out of `check` and `lint` what `quick` already ran.
fn execute_in(task: &str, after_quick: bool) -> io::Result<()> {
    let mut cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // scripts/dev names the pinned Cargo it launched. Preserve explicit overrides by callers.
    if env::var("YULIUS_PROJECT_CARGO").as_deref() == Ok(cargo.as_str()) {
        cargo = "scripts/cargo".to_owned();
    }
    let python = env::var("PYTHON").unwrap_or_else(|_| "python3".into());
    match task {
        "doctor" => run(&python, &["scripts/check_repository.py", "--doctor"]),
        "quick" => {
            run(&python, &["scripts/check_repository.py"])?;
            run(&cargo, &["fmt", "--all", "--", "--check"])
        }
        "check" => {
            if !after_quick {
                run(&python, &["scripts/check_repository.py"])?;
            }
            let compile = || {
                run(
                    &cargo,
                    &[
                        "check",
                        "--locked",
                        "--workspace",
                        "--all-targets",
                        "--all-features",
                    ],
                )
            };
            // In `ci` and `checks` the checker's tests already run beside every Rust gate.
            if after_quick {
                return compile();
            }
            let (tests, checked) = beside(|| checker_tests(&python), compile);
            checked.and(checker_report(&python, tests))
        }
        "fmt" => run(&cargo, &["fmt", "--all"]),
        "lint" => {
            if !after_quick {
                run(&cargo, &["fmt", "--all", "--", "--check"])?;
            }
            run(
                &cargo,
                &[
                    "clippy",
                    "--locked",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            )
        }
        "macos" => {
            let tools = env::current_dir()?.join(".tools/bin");
            run_with(
                &cargo,
                &[
                    "clippy",
                    "--locked",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--target",
                    MACOS_TARGET,
                    "--",
                    "-D",
                    "warnings",
                ],
                &macos_env(cfg!(target_os = "macos"), &tools),
            )
        }
        "test" => {
            // Its own stage, first (the quarantine's listing builds the tests), so the test
            // stage's history goes on in the shards' job (SPEC-137).
            stage("test");
            let mut args = vec![
                "nextest",
                "run",
                "--locked",
                "--workspace",
                "--all-features",
                "--profile",
                "ci",
            ];
            // Tests Yulius CI quarantined as flaky are left out (SPEC-497 OFC-002), each in its
            // own binary where this head builds it.
            let names = std::env::var("YULIUS_CI_QUARANTINE").unwrap_or_default();
            let quarantined = if names.trim().is_empty() {
                String::new()
            } else {
                println!(
                    "+ cargo nextest list --list-type binaries-only (the binaries this head \
                     builds, to leave the quarantined tests out in theirs)"
                );
                let mut list = Command::new(&cargo);
                list.stderr(std::process::Stdio::inherit());
                list.args(["nextest", "list"]).args(&args[2..]).args([
                    "--list-type",
                    "binaries-only",
                    "--message-format",
                    "oneline",
                ]);
                let listed = list.output()?;
                if !listed.status.success() {
                    return Err(io::Error::other("cargo nextest list failed"));
                }
                quarantine_filter(&names, &String::from_utf8_lossy(&listed.stdout))
            };
            let filter = format!("not ({quarantined})");
            if !quarantined.is_empty() {
                println!("quarantined as flaky, left out: {quarantined}");
                args.extend(["-E", filter.as_str()]);
            }
            // A shard of a matrix job (`matrix = "1/2 2/2"`) runs its part of the tests, by a hash
            // of each test's name; the doctests run in the first (SPEC-497 SHR-001).
            let shard = shard_of(&env::var("YULIUS_CI_MATRIX").unwrap_or_default());
            let partition = shard.map(|(index, count)| format!("hash:{index}/{count}"));
            if let Some(partition) = &partition {
                println!("shard {partition} of the tests");
                args.extend(["--partition", partition.as_str()]);
            }
            run(&cargo, &args)?;
            if shard.is_none_or(|(index, _)| index == 1) {
                run(&python, &["scripts/doctests.py"])?;
            }
            Ok(())
        }
        // Everything `ci` runs but the tests, which a CI job of their own runs in shards.
        "checks" => gates(false, &cargo, &python),
        "deps" => {
            run(&cargo, &["deny", "--locked", "check"])?;
            run(&cargo, &["shear", "--locked", "--deny-warnings"])
        }
        "ci" => gates(true, &cargo, &python),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown task")),
    }
}

/// The workspace's documentation, warnings as `RUSTDOCFLAGS` sets them.
fn docs(cargo: &str) -> io::Result<()> {
    run(
        cargo,
        &[
            "doc",
            "--locked",
            "--workspace",
            "--all-features",
            "--no-deps",
        ],
    )
}

/// The shard a matrix value `i/n` names (1-based, `1 <= i <= n`, `n` at most 64), if it is one.
fn shard_of(matrix: &str) -> Option<(u32, u32)> {
    let (index, count) = matrix.trim().split_once('/')?;
    let (index, count): (u32, u32) = (index.parse().ok()?, count.parse().ok()?);
    (1..=64).contains(&count).then_some(())?;
    (1..=count).contains(&index).then_some((index, count))
}

/// The security gate keeps its exit code: 2 (not run here) must stay distinct from 1 (failed).
/// On Unix Python replaces this process, so a signal sent to `scripts/dev` reaches the gate, which
/// then kills the scan it started.
fn security(args: &[String]) -> io::Result<ExitCode> {
    let python = env::var("PYTHON").unwrap_or_else(|_| "python3".into());
    println!("+ {python} scripts/vidoc_scan.py {}", args.join(" "));
    let mut command = Command::new(&python);
    command.arg("scripts/vidoc_scan.py").args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec())
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        Ok(match status.code() {
            Some(0) => ExitCode::SUCCESS,
            Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            None => ExitCode::FAILURE,
        })
    }
}

/// The nextest filter of the tests Yulius CI quarantined (`names`, one per line, as
/// `$YULIUS_CI_QUARANTINE` gives them) among this head's test binaries (`binaries`, one id per
/// line): `crate tests::name` as `(binary_id(=crate) & test(=tests::name))` when the head builds
/// that binary, left out when it does not (nextest refuses a filter naming a binary it lacks), a
/// bare `tests::name` as `test(=tests::name)`. A name a filter cannot hold is left out.
fn quarantine_filter(names: &str, binaries: &str) -> String {
    let safe = |part: &str| {
        !part.is_empty()
            && !part.starts_with('/')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.' | '/'))
    };
    let built: Vec<&str> = binaries.lines().map(str::trim).collect();
    names
        .lines()
        .map(str::trim)
        .filter_map(|name| match name.split_once(' ') {
            Some((binary, test)) if safe(binary) && safe(test) => built
                .contains(&binary)
                .then(|| format!("(binary_id(={binary}) & test(={test}))")),
            None if safe(name) => Some(format!("test(={name})")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] || args == ["help"] {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if (args.len() != 1 && args[0] != "security" && args[0] != "mutants")
        || ![
            "doctor", "quick", "check", "fmt", "lint", "macos", "test", "deps", "ci", "checks",
            "security", "mutants",
        ]
        .contains(&args[0].as_str())
    {
        eprintln!("Invalid command.\n{HELP}");
        return ExitCode::from(2);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if args[0] == "security" {
        return env::set_current_dir(root)
            .and_then(|()| security(&args[1..]))
            .unwrap_or_else(|error| {
                eprintln!("Gate failed: {error}");
                ExitCode::FAILURE
            });
    }
    if args[0] == "mutants" {
        let integration = env::var("YULIUS_INTEGRATION_BRANCH").unwrap_or_else(|_| "dev".into());
        let settings = match mutants::Settings::parse(&args[1..], &integration) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("{error}\n{HELP}");
                return ExitCode::from(2);
            }
        };
        let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        return match mutants::run(&root, &cargo, &settings) {
            Ok(outcomes) => mutants::exit_code(outcomes.as_ref()),
            Err(error) => {
                eprintln!("Mutants failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if let Err(error) = env::set_current_dir(root).and_then(|()| execute(&args[0])) {
        eprintln!("Gate failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{beside, gate_sequence, macos_env, quarantine_filter, shard_of, stage_marker};
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    /// SPEC-506 FST-008: the two steps of `beside` run at once: each hands the other a token and
    /// waits for the other's, which only works when both run (in a row, the first times out).
    #[test]
    fn beside_runs_both_steps_at_once() {
        let (to_second, from_first) = mpsc::channel();
        let (to_first, from_second) = mpsc::channel();
        let wait = Duration::from_secs(30);
        let (first, second) = beside(
            move || {
                to_second.send(1).ok();
                from_second.recv_timeout(wait)
            },
            move || {
                to_first.send(2).ok();
                from_first.recv_timeout(wait)
            },
        );
        assert_eq!((first, second), (Ok(2), Ok(1)));
    }

    /// SPEC-506 FST-006 (AC-4): the checks of seconds run first, right after the tool check, in
    /// `ci` and in `checks` (the CI job `quality`), so a format or contract failure ends a run
    /// before the build; `checks` leaves the tests to their own jobs.
    #[test]
    fn the_quick_checks_run_first() {
        assert_eq!(
            gate_sequence(true),
            ["doctor", "quick", "check", "lint", "macos", "test", "deps"]
        );
        assert_eq!(
            gate_sequence(false),
            ["doctor", "quick", "check", "lint", "macos", "deps"]
        );
    }

    /// SPEC-497 OFC-002 (review round 1): a quarantined test is left out in its own binary, and
    /// one whose binary this head does not build is not named (nextest would refuse the filter).
    #[test]
    fn quarantined_tests_are_left_out_in_the_binaries_this_head_builds() {
        let names = "yulius::main local_ci::slow\nyulius-egress::browser images::a\ntests::bare\n\
                     app (evil)\n/a tests::x\n";
        assert_eq!(
            quarantine_filter(names, "yulius-domain\nyulius::main\n"),
            "(binary_id(=yulius::main) & test(=local_ci::slow)) | test(=tests::bare)"
        );
        assert_eq!(quarantine_filter(names, ""), "test(=tests::bare)");
        assert_eq!(quarantine_filter("", "app\n"), "");
    }

    /// SPEC-497 SHR-001: a matrix value `i/n` is a shard of the tests; anything else is none.
    #[test]
    fn a_matrix_value_names_a_shard_of_the_tests() {
        assert_eq!(shard_of("1/2"), Some((1, 2)));
        assert_eq!(shard_of(" 2/2 "), Some((2, 2)));
        for none in ["", "1", "0/2", "3/2", "a/2", "1/0", "1/65", "1/2/3"] {
            assert_eq!(shard_of(none), None, "{none}");
        }
    }

    #[test]
    fn a_stage_line_is_the_marker_yulius_reads() {
        assert_eq!(stage_marker("lint"), "::yulius-stage lint");
    }

    #[test]
    fn the_macos_check_compiles_c_with_zig_off_a_mac() {
        let tools = Path::new("/w/.tools/bin");
        assert_eq!(
            macos_env(false, tools),
            [
                (
                    "CC_aarch64_apple_darwin".to_owned(),
                    "/w/.tools/bin/macos-cc".to_owned()
                ),
                (
                    "AR_aarch64_apple_darwin".to_owned(),
                    "/w/.tools/bin/macos-ar".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn the_macos_check_uses_the_macs_own_compiler_on_a_mac() {
        assert!(macos_env(true, Path::new("/w/.tools/bin")).is_empty());
    }
}
