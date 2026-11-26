//! Shared fixtures moved from `secrets_env`.

use std::path::Path;
use std::process::Command;

/// A synthetic key file: `-----BEGIN <label>-----`, `body`, `-----END <label>-----`, each on
/// its own line, with a final line break (the armour is built at run time so no scanner takes
/// the source for a key).
pub(crate) fn key_file(label: &str, body: &[&str]) -> String {
    let mut lines = vec![format!("-----BEGIN {label}-----")];
    lines.extend(body.iter().map(|l| (*l).to_owned()));
    lines.push(format!("-----END {label}-----"));
    format!("{}\n", lines.join("\n"))
}

/// A synthetic 3-line PEM key.
pub(crate) fn pem_key() -> String {
    key_file(
        &format!("{} KEY", "SYNTHETIC ED25519"),
        &["c3ludGhldGljLWVkMjU1MTktcGVtLWtleS1ib2R5"],
    )
}

/// A synthetic 7-line OpenSSH key (`n` makes each a different one).
pub(crate) fn openssh_key(n: u8) -> String {
    let body: Vec<String> = (1..=5)
        .map(|line| format!("c3ludGhldGljLW9wZW5zc2gta2V5LXZ{n}-bGluZS0w{line}"))
        .collect();
    let body: Vec<&str> = body.iter().map(String::as_str).collect();
    key_file(&format!("{} PRIVATE KEY", "OPENSSH"), &body)
}

/// What `sh` reads for `name` from the `.env` in `dir`, byte for byte. The name is `sh`'s `$1`,
/// never script text (AGENTS.md: no shell interpolation): `set -a` exports what `.env` sets and
/// `printenv` prints it with one newline of its own, which is cut.
pub(crate) fn sourced(dir: &Path, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("sh")
        .args(["-c", "set -a && . ./.env && printenv \"$1\"", "sh", name])
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "sourcing .env for {name} failed (or it is unset): {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    let mut value = String::from_utf8(out.stdout)?;
    if value.pop() != Some('\n') {
        return Err(format!("printenv printed no final newline for {name}").into());
    }
    Ok(value)
}
