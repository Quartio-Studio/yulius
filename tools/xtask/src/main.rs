//! Reproducible local and CI quality gates. This is tooling, not the Yulius product.

use std::{env, io, path::Path, process::Command, process::ExitCode};

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
            --all scans the whole repository (needs a Vidoc key; not in ci)";

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
            run(
                &cargo,
                &[
                    "nextest",
                    "run",
                    "--locked",
                    "--workspace",
                    "--all-features",
                    "--profile",
                    "ci",
                ],
            )?;
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

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] || args == ["help"] {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if (args.len() != 1 && args[0] != "security")
        || ![
            "doctor", "check", "fmt", "lint", "macos", "test", "deps", "ci", "security",
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
    if let Err(error) = env::set_current_dir(root).and_then(|()| execute(&args[0])) {
        eprintln!("Gate failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{macos_env, stage_marker};
    use std::path::Path;

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
