//! Test helpers shared with other crates (feature `test-support`).
//!
//! The fake agent (`tools/fake-agent`) is built as `target/<profile>/fake-agent` whenever the
//! workspace's test targets are built (`cargo nextest run --workspace`, `cargo test
//! --workspace`); `cargo build -p fake-agent` builds it explicitly.

use std::path::PathBuf;

/// Finds the `fake-agent` binary by walking up from the running test executable
/// (`target/<profile>/deps/<test>` → `target/<profile>/fake-agent`), building it once into the
/// same target directory when it is missing: a single test run (`cargo test -p yulius --test
/// …`) used to fail with "fake-agent is not built" (F 2026-10-12, agents hit it in four tasks).
#[must_use]
pub fn locate_fake_agent() -> Option<PathBuf> {
    find_fake_agent().or_else(|| {
        build_fake_agent();
        find_fake_agent()
    })
}

fn find_fake_agent() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|dir| dir.join("fake-agent"))
        .find(|candidate| candidate.is_file())
}

fn build_fake_agent() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        // target/<profile>/deps/<test>
        let Some(profile_dir) = exe.ancestors().nth(2) else {
            return;
        };
        let Some(target_dir) = profile_dir.parent() else {
            return;
        };
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = std::process::Command::new(cargo);
        command
            .args(["build", "--quiet", "-p", "fake-agent", "--target-dir"])
            .arg(target_dir);
        if profile_dir
            .file_name()
            .is_some_and(|name| name == "release")
        {
            command.arg("--release");
        }
        let _ = command.status();
    });
}
