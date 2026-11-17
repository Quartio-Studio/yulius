//! Reproducible local and CI quality gates. This is tooling, not the Yulius product.

use std::{env, io, path::Path, process::Command, process::ExitCode};

mod mutants;

const HELP: &str = "Yulius engineering commands (scripts/dev <command>)
  doctor    Verify pinned compiler and repository-local tool versions
  check     Validate repository contracts and compile every target
  fmt       Format Rust sources
  lint      Check formatting and Clippy warnings
  macos     Compile every target for macOS (aarch64-apple-darwin) with Clippy warnings denied
  test      Run nextest and documentation tests
  deps      Check advisories, licenses, sources, and unused dependencies
  ci        Run all required gates, failing on the first failed command
  security  Scan the branch's changes with Vidoc (vidoc-1), failing on open critical or high findings
            and on open medium ones in the changed lines;
            --all scans the whole repository (needs a Vidoc key; not in ci)
  mutants   Mutation-test the Rust lines the branch changed, once per pull request (SPEC-493):
            --base REF (origin/<integration branch>), --minutes N (30), --mutant-seconds N
            (300), --jobs N (2), --tests FILTER (the product's tests to run in place of those
            the branch changed); the last line is the summary for the spec's Evidence";

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

fn execute(task: &str) -> io::Result<()> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let python = env::var("PYTHON").unwrap_or_else(|_| "python3".into());
    match task {
        "doctor" => run(&python, &["scripts/check_repository.py", "--doctor"]),
        "check" => {
            run(&python, &["scripts/check_repository.py"])?;
            run(
                &python,
                &[
                    "-m",
                    "unittest",
                    "discover",
                    "-s",
                    "scripts",
                    "-p",
                    "test_*.py",
                ],
            )?;
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
        }
        "fmt" => run(&cargo, &["fmt", "--all"]),
        "lint" => {
            run(&cargo, &["fmt", "--all", "--", "--check"])?;
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
                let mut list = Command::new(&cargo);
                list.args(["nextest", "list"]).args(&args[2..]).args([
                    "--list-type",
                    "binaries-only",
                    "--message-format",
                    "oneline",
                ]);
                let listed = list.output()?;
                if !listed.status.success() {
                    eprintln!("{}", String::from_utf8_lossy(&listed.stderr));
                    return Err(io::Error::other("cargo nextest list failed"));
                }
                quarantine_filter(&names, &String::from_utf8_lossy(&listed.stdout))
            };
            let filter = format!("not ({quarantined})");
            if !quarantined.is_empty() {
                println!("quarantined as flaky, left out: {quarantined}");
                args.extend(["-E", filter.as_str()]);
            }
            run(&cargo, &args)?;
            run(&python, &["scripts/doctests.py"])
        }
        "deps" => {
            run(&cargo, &["deny", "--locked", "check"])?;
            run(&cargo, &["shear", "--locked", "--deny-warnings"])
        }
        "ci" => {
            for gate in ["doctor", "check", "lint", "macos", "test", "deps"] {
                stage(gate);
                execute(gate)?;
            }
            stage("docs");
            run(
                &cargo,
                &[
                    "doc",
                    "--locked",
                    "--workspace",
                    "--all-features",
                    "--no-deps",
                ],
            )
        }
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown task")),
    }
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
            "doctor", "check", "fmt", "lint", "macos", "test", "deps", "ci", "security", "mutants",
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
    use super::{macos_env, quarantine_filter, stage_marker};
    use std::path::Path;

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
