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
