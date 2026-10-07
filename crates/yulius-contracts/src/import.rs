//! Import manifests and chunk upload DTOs (SPEC-001 §7.5, SPEC-002 §6.5). Owned by work package F.
//!
//! # Upload protocol (version 1)
//!
//! The `yulius import` CLI authenticates with `Authorization: Bearer yul_…` (obtained through the
//! device approval flow, [`DeviceStart`]/[`DevicePoll`]) and then:
//!
//! 1. `POST` [`CREATE_IMPORT_PATH`] with [`CreateImportRequest`] → [`CreateImportResponse`]. The
//!    server verifies the repository and the caller's permission (IMP-002), creates the job and
//!    lists every manifest file it still needs together with the bytes it already holds. Files
//!    are content addressed by `sha256`, so a file the server already stored for this repository
//!    (from an earlier import) is not listed, and a partially received file reports its received
//!    length so the client resumes there (IMP-001, IMP-006).
//! 2. `PUT /api/import/{job}/files/{name}?offset=N` (see [`file_chunk_path`]) with the raw chunk
//!    bytes as the body (at most [`MAX_CHUNK_BYTES`]) and the lowercase hex SHA-256 of the body in
//!    the [`CHUNK_SHA256_HEADER`] header → [`FileProgress`]. `name` is sent as one percent-encoded
//!    path segment (a `/` inside a name is `%2F`). Chunks are idempotent by
//!    (job, name, offset, sha256): a chunk ending at or before the received length is
//!    acknowledged with the current progress; an offset beyond the received length is a
//!    `409 conflict`. A chunk whose digest does not match is `422 invalid`. When the received
//!    length reaches the manifest size, the server verifies the whole file's `sha256`.
//! 3. `POST /api/import/{job}/complete` (see [`complete_path`]) → [`ImportResult`] once every
//!    file is complete. Applying is idempotent: sessions already imported into the sandbox are
//!    skipped and counted in [`ImportResult::sessions_skipped_existing`].
//!
//! # Snapshot archive layout
//!
//! [`ImportManifest::snapshot`] is a gzip-compressed tar archive of the checkout's uncommitted
//! state: regular files and symbolic links under [`SNAPSHOT_FILES_PREFIX`] (repository-relative
//! paths; links are stored as links, never followed) and one [`SNAPSHOT_DELETED_ENTRY`] file whose
//! content is the NUL-terminated list of repository-relative paths deleted relative to `HEAD`.
//!
//! [`ImportManifest::bundle`] is a `git bundle` holding the local branches with commits not on
//! any remote (`--not --remotes`).

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The only manifest version this contract defines.
pub const IMPORT_MANIFEST_VERSION: u32 = 1;
/// Upper bound on the sum of all manifest file sizes (SPEC-002 §9: import total 2 GiB).
pub const MAX_IMPORT_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Upper bound on one uploaded chunk (SPEC-002 §9: chunk 8 MiB).
pub const MAX_CHUNK_BYTES: u64 = 8 * 1024 * 1024;
/// Upper bound on the byte length of a [`FileRef::name`].
pub const MAX_FILE_NAME_BYTES: usize = 255;
/// Upper bound on the byte length of a session identifier.
pub const MAX_SESSION_ID_BYTES: usize = 128;
/// Upper bound on the character length of a session title.
pub const MAX_TITLE_CHARS: usize = 80;
/// Request header carrying the lowercase hex SHA-256 of a chunk body.
pub const CHUNK_SHA256_HEADER: &str = "X-Chunk-Sha256";
/// Path of the job creation endpoint (`POST`).
pub const CREATE_IMPORT_PATH: &str = "/api/import";
/// Path of the device login start endpoint (`POST`, no body).
pub const DEVICE_START_PATH: &str = "/api/device/start";
/// Path of the device login poll endpoint (`POST` [`DevicePollRequest`]).
pub const DEVICE_POLL_PATH: &str = "/api/device/poll";
/// Path of the device approval endpoint (`POST` [`DeviceApproveRequest`], web session).
pub const DEVICE_APPROVE_PATH: &str = "/api/device/approve";
/// Directory prefix of file entries inside the snapshot archive.
pub const SNAPSHOT_FILES_PREFIX: &str = "files/";
/// Name of the snapshot archive entry listing deleted paths (NUL-terminated).
pub const SNAPSHOT_DELETED_ENTRY: &str = "DELETED";

/// Path of the chunk endpoint before percent-encoding, for documentation and server routing:
/// `/api/import/{job}/files/{name}`. Clients must percent-encode `job` and `name` as single path
/// segments; the server's router decodes them.
#[must_use]
pub fn file_chunk_path(job: &str, name: &str) -> String {
    format!("/api/import/{job}/files/{name}")
}

/// Path of the completion endpoint: `/api/import/{job}/complete`.
#[must_use]
pub fn complete_path(job: &str) -> String {
    format!("/api/import/{job}/complete")
}

/// Everything one `yulius import` run offers the server (version [`IMPORT_MANIFEST_VERSION`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportManifest {
    /// Contract version; always [`IMPORT_MANIFEST_VERSION`].
    pub version: u32,
    /// The GitHub repository and the local checkout's `HEAD`.
    pub repo: RepoDescriptor,
    /// Git bundle of unpublished local commits, when any exist.
    pub bundle: Option<FileRef>,
    /// Snapshot archive of uncommitted changes, when any exist.
    pub snapshot: Option<FileRef>,
    /// Claude and Codex sessions opened in the repository within the window.
    pub sessions: Vec<SessionRef>,
    /// Start of the session window as an RFC 3339 UTC timestamp.
    pub since: String,
}

/// The repository being imported. GitHub is the authority (SPEC-001 IMP-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDescriptor {
    /// Canonical `https://github.com/<owner>/<name>` URL, without credentials or `.git`.
    pub remote_url: String,
    /// GitHub owner (organization or user) login.
    pub owner: String,
    /// GitHub repository name.
    pub name: String,
    /// Checked-out branch; `None` for a detached `HEAD`.
    pub head_branch: Option<String>,
    /// Full object id of the local `HEAD` commit.
    pub head_sha: String,
}

/// One uploaded file, identified by its content hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileRef {
    /// Safe relative name within the job (see [`is_safe_file_name`]); unique per manifest.
    pub name: String,
    /// Exact size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
}

/// The coding-agent CLI that recorded an imported session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportHarness {
    /// Claude Code (`claude`).
    Claude,
    /// Codex CLI (`codex`).
    Codex,
}

impl ImportHarness {
    /// Stable wire name (`claude` or `codex`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

impl fmt::Display for ImportHarness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A native session to import as an organization task (SPEC-001 IMP-003/004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    /// Harness that recorded the session.
    pub harness: ImportHarness,
    /// The harness's own session identifier (used for native resume and deduplication).
    pub session_id: String,
    /// First real user message, whitespace-collapsed, at most [`MAX_TITLE_CHARS`] characters.
    pub title: String,
    /// Working directory relative to the matching checkout root, `/`-separated; `None` when the
    /// session ran at a checkout root or its directory is not inside a known checkout.
    pub cwd_relative: Option<String>,
    /// First recorded timestamp (RFC 3339 UTC).
    pub started_at: String,
    /// Last recorded timestamp (RFC 3339 UTC).
    pub ended_at: String,
    /// Git branch recorded when the session started, if any.
    pub git_branch: Option<String>,
    /// The transcript file first, then auxiliary files (for example Claude subagent transcripts).
    pub files: Vec<FileRef>,
}

/// `POST /api/import` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateImportRequest {
    /// What the client offers.
    pub manifest: ImportManifest,
}

/// `POST /api/import` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateImportResponse {
    /// Opaque job identifier for the chunk and completion endpoints.
    pub job_id: String,
    /// Files the server still needs; absent files are already stored.
    pub missing: Vec<FileProgress>,
}

/// How much of one file the server holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileProgress {
    /// The manifest file name.
    pub name: String,
    /// Contiguous bytes received from offset 0; the next chunk starts here.
    pub received_bytes: u64,
}

/// `POST /api/import/{job}/complete` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    /// Sandbox that holds the imported project.
    pub sandbox_id: String,
    /// Browser URL of the sandbox.
    pub sandbox_url: String,
    /// Organization tasks created from newly imported sessions.
    pub tasks_created: u32,
    /// Sessions that were already imported into this sandbox.
    pub sessions_skipped_existing: u32,
    /// Sandbox onboarding state wire name (`not_needed|pending|preparing|ready|skipped`, U-08).
    pub onboarding: String,
}

/// `POST /api/device/start` response (docs/api.md, CLI device approval).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStart {
    /// Secret polling code; only the CLI knows it.
    pub device_code: String,
    /// Short code the user confirms in the web UI.
    pub user_code: String,
    /// Page where a signed-in user approves the code.
    pub verification_url: String,
    /// Minimum seconds between polls.
    pub interval_s: u64,
    /// Seconds until the codes expire.
    pub expires_in_s: u64,
}

/// `POST /api/device/poll` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePollRequest {
    /// The code from [`DeviceStart::device_code`].
    pub device_code: String,
}

/// State of a device login request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    /// Not yet approved or denied; keep polling.
    Pending,
    /// Approved; the response carries the token.
    Approved,
    /// A user denied the request.
    Denied,
    /// The codes expired.
    Expired,
}

/// `POST /api/device/poll` response. `Debug` redacts the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePoll {
    /// Current state.
    pub status: DeviceStatus,
    /// CLI bearer token (`yul_…`), present only when approved.
    pub token: Option<String>,
}

impl fmt::Debug for DevicePoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevicePoll")
            .field("status", &self.status)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `POST /api/device/approve` body (web session with CSRF header).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceApproveRequest {
    /// The code shown by the CLI.
    pub user_code: String,
    /// `true` approves, `false` denies.
    pub approve: bool,
}

/// A manifest that violates this contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// `version` is not [`IMPORT_MANIFEST_VERSION`].
    #[error("unsupported import manifest version {0}")]
    UnsupportedVersion(u32),
    /// A file name fails [`is_safe_file_name`].
    #[error("unsafe file name {0:?}")]
    UnsafeFileName(String),
    /// Two files share a name.
    #[error("duplicate file name {0:?}")]
    DuplicateFileName(String),
    /// A digest is not 64 lowercase hex characters.
    #[error("invalid sha256 for {0:?}")]
    InvalidSha256(String),
    /// The files exceed [`MAX_IMPORT_TOTAL_BYTES`].
    #[error("import is {total} bytes; the limit is {limit} bytes")]
    TooLarge {
        /// Sum of all file sizes.
        total: u64,
        /// The enforced limit.
        limit: u64,
    },
    /// A session identifier fails [`is_safe_session_id`].
    #[error("invalid session id {0:?}")]
    InvalidSessionId(String),
    /// A session has no transcript file.
    #[error("session {0:?} has no files")]
    SessionWithoutFiles(String),
    /// A `cwd_relative` value is absolute, empty or contains `..`.
    #[error("invalid relative working directory for session {0:?}")]
    InvalidCwd(String),
    /// A title is longer than [`MAX_TITLE_CHARS`].
    #[error("title too long for session {0:?}")]
    TitleTooLong(String),
}

impl ImportManifest {
    /// Every file in the manifest: bundle, snapshot, then session files in order.
    pub fn files(&self) -> impl Iterator<Item = &FileRef> {
        self.bundle
            .iter()
            .chain(self.snapshot.iter())
            .chain(self.sessions.iter().flat_map(|session| session.files.iter()))
    }

    /// Sum of all file sizes (saturating).
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files()
            .fold(0_u64, |total, file| total.saturating_add(file.size))
    }

    /// Checks version, file names, digests, uniqueness, session fields and the total size bound.
    ///
    /// # Errors
    /// Returns the first [`ManifestError`] found.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version != IMPORT_MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedVersion(self.version));
        }
        let mut names = BTreeSet::new();
        for file in self.files() {
            file.validate()?;
            if !names.insert(file.name.as_str()) {
                return Err(ManifestError::DuplicateFileName(file.name.clone()));
            }
        }
        for session in &self.sessions {
            if !is_safe_session_id(&session.session_id) {
                return Err(ManifestError::InvalidSessionId(session.session_id.clone()));
            }
            if session.files.is_empty() {
                return Err(ManifestError::SessionWithoutFiles(session.session_id.clone()));
            }
            if let Some(cwd) = &session.cwd_relative
                && !is_safe_relative_dir(cwd)
            {
                return Err(ManifestError::InvalidCwd(session.session_id.clone()));
            }
            if session.title.chars().count() > MAX_TITLE_CHARS {
                return Err(ManifestError::TitleTooLong(session.session_id.clone()));
            }
        }
        let total = self.total_bytes();
        if total > MAX_IMPORT_TOTAL_BYTES {
            return Err(ManifestError::TooLarge {
                total,
                limit: MAX_IMPORT_TOTAL_BYTES,
            });
        }
        Ok(())
    }
}

impl FileRef {
    /// Checks the name and digest format.
    ///
    /// # Errors
    /// [`ManifestError::UnsafeFileName`] or [`ManifestError::InvalidSha256`].
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !is_safe_file_name(&self.name) {
            return Err(ManifestError::UnsafeFileName(self.name.clone()));
        }
        if !is_sha256_hex(&self.sha256) {
            return Err(ManifestError::InvalidSha256(self.name.clone()));
        }
        Ok(())
    }
}

/// `true` when `name` is a safe relative file name: 1–[`MAX_FILE_NAME_BYTES`] bytes of
/// `/`-separated components, each made of ASCII letters, digits, `.`, `_` and `-`, not starting
/// with `.` (which rules out `.`, `..` and hidden files), with no empty component, leading or
/// trailing `/`.
#[must_use]
pub fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_FILE_NAME_BYTES
        && name.split('/').all(|component| {
            !component.is_empty()
                && !component.starts_with('.')
                && component
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// `true` when `id` is a usable session identifier: 1–[`MAX_SESSION_ID_BYTES`] bytes of ASCII
/// letters, digits, `.`, `_` and `-`, not starting with `.`.
#[must_use]
pub fn is_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_BYTES
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `true` when `text` is 64 lowercase hex characters.
#[must_use]
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `true` when a chunk of `len` bytes is within [`MAX_CHUNK_BYTES`].
#[must_use]
pub fn is_valid_chunk_len(len: u64) -> bool {
    len <= MAX_CHUNK_BYTES
}

fn is_safe_relative_dir(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> FileRef {
        FileRef {
            name: name.to_owned(),
            size,
            sha256: "a".repeat(64),
        }
    }

    fn manifest() -> ImportManifest {
        ImportManifest {
            version: IMPORT_MANIFEST_VERSION,
            repo: RepoDescriptor {
                remote_url: "https://github.com/Quartio-Studio/demo".to_owned(),
                owner: "Quartio-Studio".to_owned(),
                name: "demo".to_owned(),
                head_branch: Some("main".to_owned()),
                head_sha: "0".repeat(40),
            },
            bundle: Some(file("repo.bundle", 10)),
            snapshot: Some(file("snapshot.tar.gz", 20)),
            sessions: vec![SessionRef {
                harness: ImportHarness::Claude,
                session_id: "0b6c-4f".to_owned(),
                title: "Fix login".to_owned(),
                cwd_relative: Some("crates/app".to_owned()),
                started_at: "2026-10-01T10:00:00.000Z".to_owned(),
                ended_at: "2026-10-01T11:00:00.000Z".to_owned(),
                git_branch: Some("main".to_owned()),
                files: vec![
                    file("sessions/claude/0b6c-4f.jsonl", 30),
                    file("sessions/claude/0b6c-4f/subagents/agent-1.jsonl", 5),
                ],
            }],
            since: "2026-09-23T10:00:00.000Z".to_owned(),
        }
    }

    #[test]
    fn manifest_wire_shape_round_trips() -> Result<(), serde_json::Error> {
        let manifest = manifest();
        let json = serde_json::to_value(CreateImportRequest {
            manifest: manifest.clone(),
        })?;
        assert_eq!(json["manifest"]["version"], 1);
        assert_eq!(json["manifest"]["sessions"][0]["harness"], "claude");
        assert_eq!(json["manifest"]["repo"]["head_branch"], "main");
        let back: CreateImportRequest = serde_json::from_value(json)?;
        assert_eq!(back.manifest, manifest);
        assert_eq!(manifest.total_bytes(), 65);
        assert_eq!(manifest.validate(), Ok(()));
        Ok(())
    }

    #[test]
    fn validation_rejects_contract_violations() {
        let mut bad = manifest();
        bad.version = 2;
        assert_eq!(bad.validate(), Err(ManifestError::UnsupportedVersion(2)));

        let mut bad = manifest();
        bad.sessions[0].files[1].name = "repo.bundle".to_owned();
        assert_eq!(
            bad.validate(),
            Err(ManifestError::DuplicateFileName("repo.bundle".to_owned()))
        );

        let mut bad = manifest();
        bad.sessions[0].files[0].name = "../escape.jsonl".to_owned();
        assert!(matches!(
            bad.validate(),
            Err(ManifestError::UnsafeFileName(_))
        ));

        let mut bad = manifest();
        bad.sessions[0].files[0].sha256 = "A".repeat(64);
        assert!(matches!(bad.validate(), Err(ManifestError::InvalidSha256(_))));

        let mut bad = manifest();
        bad.sessions[0].session_id = "a/b".to_owned();
        assert!(matches!(
            bad.validate(),
            Err(ManifestError::InvalidSessionId(_))
        ));

        let mut bad = manifest();
        bad.sessions[0].cwd_relative = Some("../other".to_owned());
        assert!(matches!(bad.validate(), Err(ManifestError::InvalidCwd(_))));

        let mut bad = manifest();
        bad.sessions[0].title = "x".repeat(81);
        assert!(matches!(bad.validate(), Err(ManifestError::TitleTooLong(_))));

        let mut bad = manifest();
        bad.sessions[0].files.clear();
        assert!(matches!(
            bad.validate(),
            Err(ManifestError::SessionWithoutFiles(_))
        ));

        let mut bad = manifest();
        if let Some(bundle) = bad.bundle.as_mut() {
            bundle.size = MAX_IMPORT_TOTAL_BYTES;
        }
        assert!(matches!(bad.validate(), Err(ManifestError::TooLarge { .. })));
    }

    #[test]
    fn safe_file_names() {
        for good in [
            "repo.bundle",
            "snapshot.tar.gz",
            "sessions/codex/rollout-2026-10-01T10-00-00-abc.jsonl",
            "a_b-c.d",
        ] {
            assert!(is_safe_file_name(good), "{good}");
        }
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/",
            "..",
            "a/../b",
            ".hidden",
            "a\\b",
            "sp ace",
            "nul\0",
            "ü",
        ] {
            assert!(!is_safe_file_name(bad), "{bad:?}");
        }
        assert!(!is_safe_file_name(&"a".repeat(MAX_FILE_NAME_BYTES + 1)));
        assert!(is_valid_chunk_len(MAX_CHUNK_BYTES));
        assert!(!is_valid_chunk_len(MAX_CHUNK_BYTES + 1));
    }

    #[test]
    fn device_poll_wire_and_redaction() -> Result<(), serde_json::Error> {
        let poll: DevicePoll =
            serde_json::from_str(r#"{"status":"approved","token":"yul_secret"}"#)?;
        assert_eq!(poll.status, DeviceStatus::Approved);
        assert_eq!(poll.token.as_deref(), Some("yul_secret"));
        assert!(!format!("{poll:?}").contains("yul_secret"));
        let pending: DevicePoll = serde_json::from_str(r#"{"status":"pending","token":null}"#)?;
        assert_eq!(pending.status, DeviceStatus::Pending);
        let start: DeviceStart = serde_json::from_str(
            r#"{"device_code":"d","user_code":"ABCD-EFGH","verification_url":"https://y/device","interval_s":5,"expires_in_s":600}"#,
        )?;
        assert_eq!(start.interval_s, 5);
        assert_eq!(complete_path("j1"), "/api/import/j1/complete");
        assert_eq!(
            file_chunk_path("j1", "repo.bundle"),
            "/api/import/j1/files/repo.bundle"
        );
        Ok(())
    }
}
