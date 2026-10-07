//! Test helpers shared with other crates (feature `test-support`).
//!
//! The fake agent (`tools/fake-agent`) is built as `target/<profile>/fake-agent` whenever the
//! workspace's test targets are built (`cargo nextest run --workspace`, `cargo test
//! --workspace`); `cargo build -p fake-agent` builds it explicitly.

use std::path::PathBuf;

/// Finds the `fake-agent` binary by walking up from the running test executable
/// (`target/<profile>/deps/<test>` → `target/<profile>/fake-agent`).
#[must_use]
pub fn locate_fake_agent() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|dir| dir.join("fake-agent"))
        .find(|candidate| candidate.is_file())
}
