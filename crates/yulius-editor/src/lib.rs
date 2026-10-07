//! Per-sandbox openvscode-server lifecycle (SPEC-002 ADR-10; SPEC-001 EDT-001/002).
//!
//! [`EditorManager::ensure`] starts one openvscode-server per sandbox on demand:
//! `--host 127.0.0.1 --port <free port from the range> --connection-token-file <0600 file>
//! --server-base-path <base> --user-data-dir … --extensions-dir … --default-folder <workspace>`.
//! A connection token is always required (never `--without-connection-token`); the reverse
//! proxy injects it as a cookie (see [`EditorInstance`]). Startup is confirmed by an HTTP
//! health check that presents the token. Editors stop when idle
//! ([`EditorManager::reap_idle`]) or on [`EditorManager::stop`], which kills and reaps the
//! process group. openvscode-server is an external prerequisite; its absence is a typed
//! error.
//!
//! Trust model: the editor runs arbitrary code for anyone holding the token (terminals,
//! extensions); only users with write access may reach the proxy (EDT-002). The process is
//! not contained by this crate.
//!
//! The `fake` feature builds the `fake-openvscode-server` test executable.

mod health;
mod manager;
mod process;
mod token;

pub use manager::{
    EditorConfig, EditorError, EditorInstance, EditorManager, IdleReaper, TOKEN_FILE,
    default_base_path,
};
pub use process::StopTimeouts;
pub use token::{ConnectionToken, TOKEN_COOKIE};
