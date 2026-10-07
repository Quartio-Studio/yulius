//! Client side of `yulius import`: repository state capture, Claude/Codex session discovery and
//! resumable upload (SPEC-001 IMP-001..006, SPEC-002 §6.5, §8a, §9).
//!
//! The API is synchronous; it is meant for the `yulius import` and `yulius login` CLI commands
//! and must not be called from inside an async runtime (the HTTP client is `reqwest::blocking`).
//!
//! * [`discover_repo`], [`unpublished_refs`], [`create_bundle`], [`snapshot_uncommitted`] capture
//!   the local Git state through the `git` CLI (argv only, bounded output and time).
//! * [`discover_claude_sessions`], [`discover_codex_sessions`] find sessions opened in the
//!   repository's checkouts within a time window, with bounded transcript reading.
//! * [`Uploader`], [`device_login`], [`TokenStore`] talk to the Yulius server.
//! * [`run_import`] orchestrates one import run behind the [`ImportIo`] boundary.

mod device;
mod git;
mod hash;
mod http;
mod jsonl;
mod process;
mod run;
mod secret;
mod sessions;
mod since;
mod snapshot;
mod time;
mod token_store;
mod upload;

pub use device::{DeviceError, DeviceLoginOptions, MAX_DEVICE_WAIT, device_login};
pub use git::{
    GitHubRemote, RepoError, RepoInfo, UnpublishedRef, create_bundle, discover_repo,
    parse_github_remote, unpublished_refs,
};
pub use http::{ApiError, RetryPolicy};
pub use process::GitError;
pub use reqwest::Url;
pub use run::{
    ImportError, ImportIo, ImportOptions, SessionSources, TerminalIo, UploadTuning, run_import,
};
pub use secret::Secret;
pub use sessions::{
    DiscoveredSession, SessionFile, discover_claude_sessions, discover_codex_sessions,
};
pub use since::{DEFAULT_SINCE, MAX_SINCE_DAYS, SinceError, parse_since, prompt_since};
pub use snapshot::{
    MAX_SNAPSHOT_FILE_BYTES, SkipReason, SkippedPath, Snapshot, SnapshotError, snapshot_uncommitted,
};
pub use time::format_rfc3339;
pub use token_store::{TokenStore, TokenStoreError};
pub use upload::{UploadError, Uploader};
