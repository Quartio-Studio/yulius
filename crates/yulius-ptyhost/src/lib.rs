//! Detached PTY host owning one agent run, its wire protocol server and client (SPEC-002 §7.1,
//! ADR-02, ADR-03).
//!
//! A run lives in a private run directory (`runs/<attempt>/`, mode 0700):
//!
//! | File | Writer | Content |
//! | --- | --- | --- |
//! | `launch.json` | server ([`spawn_host`]) | [`LaunchSpec`](yulius_contracts::ptyhost::LaunchSpec), no secrets |
//! | `pty.sock` | host | Unix socket (0600) speaking [`yulius_contracts::ptyhost`] |
//! | `status.json` | host | [`StatusRecord`](yulius_contracts::ptyhost::StatusRecord) once the child runs |
//! | `exit.json` | host | [`ExitFile`](yulius_contracts::ptyhost::ExitFile) once the child is reaped |
//!
//! * [`run_host`] is the blocking body of `yulius ptyhost --run-dir <dir>`: it owns the PTY, the
//!   child (directly, or inside `bwrap` per [`bwrap_argv`]), a vt100 screen model and the socket.
//! * [`spawn_host`] starts a host from the server and waits until it is ready.
//! * [`PtyClient`] is the server-side async client.
//! * [`inspect_run_dir`] and [`terminate_recorded`] reconcile run directories after a server
//!   restart without ever respawning anything.
//!
//! Platform support: Linux and macOS. Bubblewrap containment is Linux-only; elsewhere a contained
//! launch fails with [`ExitReason::SpawnFailed`](yulius_contracts::ptyhost::ExitReason).
//! Host mode provides no containment: a descendant that creates its own session escapes process
//! group cleanup (on Linux the host also sweeps the child's session).

mod client;
pub mod containment;
mod files;
mod host;
mod process;
mod reconcile;
mod spawn;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use client::{CONNECT_TIMEOUT, ClientError, PtyClient, PtyReader, PtyWriter};
pub use containment::{BWRAP_PROGRAM, SYSTEM_READONLY_DIRS, bwrap_argv};
pub use host::{HostError, MAX_SHUTDOWN_GRACE_MS, run_host};
pub use reconcile::{
    RunDirState, TerminateError, current_boot_id, inspect_run_dir, terminate_recorded,
};
pub use spawn::{DEFAULT_SPAWN_TIMEOUT, HostProcessExit, SpawnError, SpawnedHost, spawn_host};
pub use yulius_contracts::ptyhost::BubblewrapSpec;

use sha2::{Digest, Sha256};
use yulius_contracts::ptyhost::TOKEN_BYTES;

/// Lower-case hex SHA-256 of a client token, the value stored in
/// [`LaunchSpec::token_sha256`](yulius_contracts::ptyhost::LaunchSpec::token_sha256).
#[must_use]
pub fn token_sha256_hex(token: &[u8; TOKEN_BYTES]) -> String {
    hex_lower(&Sha256::digest(token))
}

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_digest_is_lower_hex_sha256() {
        // SHA-256 of 32 zero bytes.
        assert_eq!(
            token_sha256_hex(&[0u8; TOKEN_BYTES]),
            "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"
        );
    }
}
