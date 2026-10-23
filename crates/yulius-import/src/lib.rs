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
//! * [`Uploader`], [`device_login`], [`TokenStore`] talk to the Yulius server; [`RemoteClient`]
//!   calls its remote MCP for `yulius remote` and its shortcuts (SPEC-247).
//! * [`stage_redacted`] copies a transcript with secrets redacted and images left out.
//! * [`run_import`] orchestrates one onboarding run behind the [`ImportIo`] boundary; [`Ui`]
//!   is what it shows and asks ([`TerminalUi`] with progress bars, [`PlainUi`] for pipes).

mod device;
mod dotenv;
mod git;
mod hash;
mod http;
mod jsonl;
mod process;
mod redact;
mod remote;
mod repo_values;
mod run;
mod secret;
mod secrets_import;
mod sessions;
mod since;
mod snapshot;
mod states;
mod time;
mod token_store;
mod ui;
mod upload;

pub use device::{DeviceError, DeviceLoginOptions, MAX_DEVICE_WAIT, device_login};
pub use dotenv::{DotenvEntry, DotenvError, MAX_DOTENV_BYTES, MAX_DOTENV_ENTRIES, parse_dotenv};
pub use git::{
    GitHubRemote, RepoError, RepoInfo, UnpublishedRef, create_bundle, create_full_bundle,
    discover_playground_checkout, discover_repo, parse_github_remote, unpublished_refs,
};
pub use http::{ApiError, RetryPolicy};
pub use process::GitError;
pub use redact::{IMAGE_PLACEHOLDER, MAX_STAGED_LINE_BYTES, RedactionReport, stage_redacted};
pub use remote::{PROTOCOL_VERSION, RemoteClient, RemoteError, ToolOutcome};
pub use reqwest::Url;
pub use run::{
    ImportError, ImportIo, ImportOptions, OnboardOutcome, SessionChoice, SessionSources,
    TerminalIo, UploadTuning, run_import,
};
pub use secret::Secret;
pub use secrets_import::{
    MAX_SECRET_FILE_BYTES, PreparedTarget, SecretsImportError, SecretsImportOptions, SecretsIo,
    SecretsSummary, SecretsTarget, VaultClient, import_secrets, prepare_target, prompt_secrets,
    secret_file_entry,
};
pub use sessions::{DiscoveredSession, SessionFile, discover_claude_sessions};
pub use since::{DEFAULT_SINCE, MAX_SINCE_DAYS, SinceError, parse_since, prompt_since};
pub use snapshot::{
    MAX_SNAPSHOT_FILE_BYTES, SkipReason, SkippedPath, Snapshot, SnapshotError, snapshot_uncommitted,
};
pub use states::{
    MAX_STATE_BYTES, StateEntry, StatesSummary, download_state, import_states, pack_state_dir,
    state_entry,
};
pub use time::format_rfc3339;
pub use token_store::{TokenStore, TokenStoreError};
pub use ui::{PlainUi, TerminalUi, Tone, Ui, Unit, human_bytes, parse_selection, set_colors};
pub use upload::{UploadError, Uploader};
