//! Client side of `yulius onboard` (alias `yulius import`): repository state capture, Claude Code
//! session discovery, redaction and resumable upload (SPEC-001 IMP-001..009, SPEC-002 §6.5, §8a,
//! §9).
//!
//! The API is synchronous; it is meant for the `yulius import` and `yulius login` CLI commands
//! and must not be called from inside an async runtime (the HTTP client is `reqwest::blocking`).
//!
//! * [`discover_repo`], [`unpublished_refs`], [`create_bundle`], [`snapshot_uncommitted`] capture
//!   the local Git state through the `git` CLI (argv only, bounded output and time).
//! * [`discover_claude_sessions`] finds sessions opened in the repository's checkouts within a
//!   time window, with bounded transcript reading.
//! * [`Uploader`], [`device_login`], [`TokenStore`] talk to the Yulius server.
//! * [`stage_redacted`] copies a transcript with secrets redacted and images left out.
//! * [`run_import`] orchestrates one onboarding run behind the [`ImportIo`] boundary; [`Ui`]
//!   is what it shows and asks ([`TerminalUi`] with progress bars, [`PlainUi`] for pipes).

mod device;
mod git;
mod hash;
mod http;
mod jsonl;
mod process;
mod redact;
mod run;
mod secret;
mod sessions;
mod since;
mod snapshot;
mod time;
mod token_store;
mod ui;
mod upload;

pub use device::{DeviceError, DeviceLoginOptions, MAX_DEVICE_WAIT, device_login};
pub use git::{
    GitHubRemote, RepoError, RepoInfo, UnpublishedRef, create_bundle, create_full_bundle,
    discover_playground_checkout, discover_repo, parse_github_remote, unpublished_refs,
};
pub use http::{ApiError, RetryPolicy};
pub use process::GitError;
pub use redact::{IMAGE_PLACEHOLDER, MAX_STAGED_LINE_BYTES, RedactionReport, stage_redacted};
pub use reqwest::Url;
pub use run::{
    ImportError, ImportIo, ImportOptions, OnboardOutcome, SessionChoice, SessionSources,
    TerminalIo, UploadTuning, run_import,
};
pub use secret::Secret;
pub use sessions::{DiscoveredSession, SessionFile, discover_claude_sessions};
pub use since::{DEFAULT_SINCE, MAX_SINCE_DAYS, SinceError, parse_since, prompt_since};
pub use snapshot::{
    MAX_SNAPSHOT_FILE_BYTES, SkipReason, SkippedPath, Snapshot, SnapshotError, snapshot_uncommitted,
};
pub use time::format_rfc3339;
pub use token_store::{TokenStore, TokenStoreError};
pub use ui::{PlainUi, TerminalUi, Tone, Ui, Unit, human_bytes, parse_selection, set_colors};
pub use upload::{UploadError, Uploader};
