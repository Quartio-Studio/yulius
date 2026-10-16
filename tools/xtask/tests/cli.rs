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
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .env_remove("VIDOC_API_KEY")
        .arg("security")
        .output()?;
    // Exit 2 is "not run", distinct from findings (1): the gate's code passes through.
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("No Vidoc key"));
    // SPEC-195: its options reach the scan (`scripts/dev security --all` is the broad scan).
    let broad = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .env_remove("VIDOC_API_KEY")
        .args(["security", "--all"])
        .output()?;
    assert_eq!(broad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&broad.stdout).contains("scripts/vidoc_scan.py --all"));
    assert!(String::from_utf8_lossy(&broad.stderr).contains("No Vidoc key"));
    Ok(())
}
