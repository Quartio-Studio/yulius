//! Reproducible local and CI quality gates. This is tooling, not the Yulius product.

use std::{env, io, path::Path, process::Command, process::ExitCode};

const HELP: &str = "Yulius engineering commands (scripts/dev <command>)
  doctor  Verify pinned compiler and repository-local tool versions
  check   Validate repository contracts and compile every target
  fmt     Format Rust sources
  lint    Check formatting and Clippy warnings
  test    Run nextest and documentation tests
  deps    Check advisories, licenses, sources, and unused dependencies
  ci      Run all required gates, failing on the first failed command";

fn run(program: &str, args: &[&str]) -> io::Result<()> {
    println!("+ {program} {}", args.join(" "));
    let mut command = Command::new(program);
    command.args(args);
    if args.first() == Some(&"machete") {
        // cargo-machete distinguishes Cargo subcommands from `cargo run` by this
        // variable. Do not leak xtask's package context into the nested command.
        command.env_remove("CARGO_PKG_NAME");
    }
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
            run(&cargo, &["machete"])
        }
        "ci" => {
            for gate in ["doctor", "check", "lint", "test", "deps"] {
                execute(gate)?;
            }
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

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] || args == ["help"] {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.len() != 1
        || !["doctor", "check", "fmt", "lint", "test", "deps", "ci"].contains(&args[0].as_str())
    {
        eprintln!("Invalid command.\n{HELP}");
        return ExitCode::from(2);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if let Err(error) = env::set_current_dir(root).and_then(|()| execute(&args[0])) {
        eprintln!("Gate failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
