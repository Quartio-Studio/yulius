//! Embeds the commit this binary is built from as `YULIUS_BUILD_COMMIT` (SPEC-002 ADR-18): the
//! environment variable of that name when it holds a full commit id (the update helper sets it
//! to the exact commit it checked out), else `git rev-parse HEAD` of the checkout, else empty
//! (built outside Git: the build is "unknown").

use std::path::{Path, PathBuf};
use std::process::Command;

fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_owned())
}

/// Reruns the script when `path` changes; a missing file (a packed ref) is not watched, since
/// Cargo would treat it as changed on every build.
fn watch(manifest: &Path, path: Option<String>) {
    if let Some(path) = path.map(|p| manifest.join(p)).filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=YULIUS_BUILD_COMMIT");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let from_env = std::env::var("YULIUS_BUILD_COMMIT")
        .ok()
        .map(|c| c.trim().to_owned())
        .filter(|c| is_commit(c));
    let commit = from_env.or_else(|| {
        // Rebuild when HEAD moves: a commit, checkout or reset rewrites these files.
        for args in [
            ["rev-parse", "--git-path", "HEAD"],
            ["rev-parse", "--git-path", "logs/HEAD"],
        ] {
            watch(&manifest, git(&manifest, &args));
        }
        if let Some(head) = git(&manifest, &["symbolic-ref", "-q", "HEAD"]) {
            watch(
                &manifest,
                git(&manifest, &["rev-parse", "--git-path", &head]),
            );
        }
        git(&manifest, &["rev-parse", "HEAD"]).filter(|c| is_commit(c))
    });
    println!(
        "cargo:rustc-env=YULIUS_BUILD_COMMIT={}",
        commit.unwrap_or_default()
    );
}
