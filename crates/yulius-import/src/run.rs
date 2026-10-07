//! `yulius import` orchestration (SPEC-001 J4, §7.5; IMP-001..003, IMP-006).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use yulius_contracts::import::{
    FileRef, IMPORT_MANIFEST_VERSION, ImportHarness, ImportManifest, ImportResult, MAX_CHUNK_BYTES,
    MAX_IMPORT_TOTAL_BYTES, ManifestError, SessionRef,
};

use crate::device::{DeviceError, DeviceLoginOptions, device_login};
use crate::git::{
    BUNDLE_NAME, RepoError, RepoInfo, create_bundle, discover_repo, unpublished_refs,
};
use crate::hash::file_ref;
use crate::http::{ApiError, RetryPolicy};
use crate::process::GitError;
use crate::secret::Secret;
use crate::sessions::{DiscoveredSession, discover_claude_sessions, discover_codex_sessions};
use crate::since::{DEFAULT_SINCE, SinceError, parse_since, prompt_since};
use crate::snapshot::{SNAPSHOT_NAME, SkipReason, Snapshot, SnapshotError, snapshot_uncommitted};
use crate::time::format_rfc3339;
use crate::token_store::{TokenStore, TokenStoreError};
use crate::upload::{UploadError, Uploader};

/// Sessions listed individually in the summary before eliding the rest.
const MAX_LISTED_SESSIONS: usize = 50;

/// What to import and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOptions {
    /// Any directory inside the checkout to import.
    pub path: PathBuf,
    /// Yulius server base URL.
    pub server: Url,
    /// Session window such as `14d`; `None` prompts (or uses the default with `assume_yes`).
    pub since: Option<String>,
    /// Skip the window prompt and the confirmation.
    pub assume_yes: bool,
}

/// Where the local harnesses keep their sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSources {
    /// Claude configuration directory (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub claude_dir: Option<PathBuf>,
    /// Codex home (`CODEX_HOME` or `~/.codex`).
    pub codex_home: Option<PathBuf>,
}

impl SessionSources {
    /// Locations from `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `HOME` (absolute paths only).
    #[must_use]
    pub fn from_env() -> Self {
        let absolute = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home = absolute("HOME");
        Self {
            claude_dir: absolute("CLAUDE_CONFIG_DIR")
                .or_else(|| home.as_ref().map(|home| home.join(".claude"))),
            codex_home: absolute("CODEX_HOME")
                .or_else(|| home.as_ref().map(|home| home.join(".codex"))),
        }
    }
}

/// Upload parameters (defaults: 8 MiB chunks, default retry policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadTuning {
    /// Chunk size in bytes (clamped to the 8 MiB contract bound).
    pub chunk_size: u64,
    /// Retry policy per request.
    pub retry: RetryPolicy,
}

impl Default for UploadTuning {
    fn default() -> Self {
        Self {
            chunk_size: MAX_CHUNK_BYTES,
            retry: RetryPolicy::default(),
        }
    }
}

/// The side-effect boundary of [`run_import`]: terminal, credentials, environment and clock.
pub trait ImportIo {
    /// Terminal input and output used for prompts, the summary and progress.
    fn streams(&mut self) -> (&mut dyn BufRead, &mut dyn Write);
    /// A bearer token for `server` (stored, or obtained by logging in).
    ///
    /// # Errors
    /// When no token can be obtained.
    fn token(&mut self, server: &Url) -> Result<Secret, ImportError>;
    /// Session store locations.
    fn session_sources(&self) -> SessionSources;
    /// The current time (start of the session window is `now - since`).
    fn now(&self) -> SystemTime;
    /// Upload parameters.
    fn upload_tuning(&self) -> UploadTuning {
        UploadTuning::default()
    }
}

/// Why an import run stopped.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The directory is not an importable GitHub checkout (IMP-002).
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// A Git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// Capturing uncommitted changes failed.
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    /// The session window is invalid.
    #[error(transparent)]
    Since(#[from] SinceError),
    /// Listing a session store failed.
    #[error("cannot list sessions in {}: {source}", path.display())]
    Sessions {
        /// Session store directory.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// Copying a file into the staging directory failed.
    #[error("cannot stage {name}: {source}")]
    Staging {
        /// Manifest file name.
        name: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// Everything together exceeds the 2 GiB import bound.
    #[error(
        "the import would exceed {limit} bytes; choose a shorter session window or commit and push large changes first"
    )]
    TooLarge {
        /// The enforced bound.
        limit: u64,
    },
    /// The manifest violates the contract.
    #[error("invalid import manifest: {0}")]
    Manifest(#[from] ManifestError),
    /// Talking to the server failed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// Uploading failed; running the import again resumes.
    #[error(transparent)]
    Upload(#[from] UploadError),
    /// Device login failed.
    #[error(transparent)]
    Device(#[from] DeviceError),
    /// Reading or writing saved credentials failed.
    #[error(transparent)]
    TokenStore(#[from] TokenStoreError),
    /// The server asked for a file that is not in the manifest.
    #[error("the server requested unknown file {0:?}")]
    UnknownFile(String),
    /// Terminal I/O failed.
    #[error("terminal I/O failed: {0}")]
    Terminal(#[source] io::Error),
    /// The user declined the confirmation; nothing was uploaded.
    #[error("import cancelled; nothing was uploaded")]
    Cancelled,
}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Terminal(error)
    }
}

/// A manifest file and the local copy to upload.
struct LocalFile {
    reference: FileRef,
    path: PathBuf,
}

/// Runs one import: discover the repository → authenticate → choose the window → bundle and
/// snapshot in a temporary directory → discover and stage sessions → summary and confirmation
/// (unless `assume_yes`) → create the job → upload the files the server reports missing,
/// resuming partial ones → complete → print the sandbox URL.
///
/// Re-running with unchanged content uploads nothing: bundle, snapshot and staged transcripts are
/// deterministic and content addressed, and the server lists only what it lacks (IMP-006).
/// Nothing is pushed to GitHub.
///
/// # Errors
/// Any [`ImportError`]; [`ImportError::Cancelled`] when the user declines.
pub fn run_import(
    options: &ImportOptions,
    io: &mut dyn ImportIo,
) -> Result<ImportResult, ImportError> {
    let repo = discover_repo(&options.path)?;
    let token = io.token(&options.server)?;
    let window = match &options.since {
        Some(text) => parse_since(text)?,
        None if options.assume_yes => parse_since(DEFAULT_SINCE)?,
        None => {
            let (input, output) = io.streams();
            prompt_since(input, output, DEFAULT_SINCE)?
        }
    };
    let since = io.now().checked_sub(window).unwrap_or(UNIX_EPOCH);

    let staging = tempfile::Builder::new()
        .prefix("yulius-import-")
        .tempdir()
        .map_err(|source| ImportError::Staging {
            name: "staging directory".to_owned(),
            source,
        })?;
    let mut files: Vec<LocalFile> = Vec::new();
    let mut budget = MAX_IMPORT_TOTAL_BYTES;

    let refs = unpublished_refs(&repo.root)?;
    let bundle_path = staging.path().join(BUNDLE_NAME);
    let bundle = create_bundle(&repo.root, &refs, &bundle_path)?;
    if let Some(reference) = &bundle {
        budget = budget
            .checked_sub(reference.size)
            .ok_or(ImportError::TooLarge {
                limit: MAX_IMPORT_TOTAL_BYTES,
            })?;
        files.push(LocalFile {
            reference: reference.clone(),
            path: bundle_path,
        });
    }

    let snapshot_path = staging.path().join(SNAPSHOT_NAME);
    let snapshot = snapshot_uncommitted(&repo.root, &snapshot_path, budget)?;
    if let Some(reference) = &snapshot.file {
        budget = budget
            .checked_sub(reference.size)
            .ok_or(ImportError::TooLarge {
                limit: MAX_IMPORT_TOTAL_BYTES,
            })?;
        files.push(LocalFile {
            reference: reference.clone(),
            path: snapshot_path,
        });
    }

    let discovered = discover_sessions(&repo, &io.session_sources(), since)?;
    let mut sessions = Vec::new();
    for session in &discovered {
        if let Some(staged) = stage_session(session, staging.path(), &mut budget, &mut files)? {
            sessions.push(staged);
        }
    }

    let manifest = ImportManifest {
        version: IMPORT_MANIFEST_VERSION,
        repo: repo.descriptor(),
        bundle,
        snapshot: snapshot.file.clone(),
        sessions,
        since: format_rfc3339(since),
    };
    manifest.validate()?;

    let summary = summary_text(&repo, &refs, &snapshot, &manifest, options);
    let (_, output) = io.streams();
    output.write_all(summary.as_bytes())?;
    if !options.assume_yes && !confirm(io, &options.server)? {
        return Err(ImportError::Cancelled);
    }

    let tuning = io.upload_tuning();
    let uploader = Uploader::new(options.server.clone(), token)?
        .with_chunk_size(tuning.chunk_size)
        .with_retry_policy(tuning.retry);
    let job = uploader.create_job(&manifest)?;
    let by_name: BTreeMap<&str, &LocalFile> = files
        .iter()
        .map(|file| (file.reference.name.as_str(), file))
        .collect();
    {
        let (_, output) = io.streams();
        writeln!(
            output,
            "Uploading {} of {} files ({} already on the server).",
            job.missing.len(),
            by_name.len(),
            by_name.len().saturating_sub(job.missing.len())
        )?;
    }
    for missing in &job.missing {
        let file = by_name
            .get(missing.name.as_str())
            .ok_or_else(|| ImportError::UnknownFile(missing.name.clone()))?;
        upload_one(&uploader, &job.job_id, file, missing.received_bytes, io)?;
    }
    let result = uploader.complete(&job.job_id)?;
    let (_, output) = io.streams();
    writeln!(
        output,
        "Imported {} new session(s) as organization tasks ({} already imported). Onboarding: {}.",
        result.tasks_created, result.sessions_skipped_existing, result.onboarding
    )?;
    writeln!(output, "Sandbox: {}", result.sandbox_url)?;
    output.flush()?;
    Ok(result)
}

fn discover_sessions(
    repo: &RepoInfo,
    sources: &SessionSources,
    since: SystemTime,
) -> Result<Vec<DiscoveredSession>, ImportError> {
    let paths = repo.checkout_paths();
    let mut sessions = Vec::new();
    if let Some(dir) = &sources.claude_dir {
        sessions.extend(
            discover_claude_sessions(dir, &paths, since).map_err(|source| {
                ImportError::Sessions {
                    path: dir.clone(),
                    source,
                }
            })?,
        );
    }
    if let Some(home) = &sources.codex_home {
        sessions.extend(
            discover_codex_sessions(home, &paths, &repo.remote_url, since).map_err(|source| {
                ImportError::Sessions {
                    path: home.clone(),
                    source,
                }
            })?,
        );
    }
    Ok(sessions)
}

/// Copies a session's transcripts into staging (cut after the last complete line, so a live
/// session yields a consistent snapshot) and builds its manifest entry. Sessions whose main
/// transcript stages empty are dropped.
fn stage_session(
    session: &DiscoveredSession,
    staging: &Path,
    budget: &mut u64,
    files: &mut Vec<LocalFile>,
) -> Result<Option<SessionRef>, ImportError> {
    let mut references = Vec::new();
    for (index, file) in session.files.iter().enumerate() {
        let destination = staging.join(&file.name);
        let reference = stage_file(&file.path, &destination, &file.name, *budget)?;
        if reference.size == 0 {
            if index == 0 {
                return Ok(None);
            }
            continue;
        }
        *budget -= reference.size;
        references.push(reference.clone());
        files.push(LocalFile {
            reference,
            path: destination,
        });
    }
    Ok(Some(SessionRef {
        harness: session.harness,
        session_id: session.session_id.clone(),
        title: session.title.clone(),
        cwd_relative: session.cwd_relative.clone(),
        started_at: session.started_at.clone(),
        ended_at: session.ended_at.clone(),
        git_branch: session.git_branch.clone(),
        files: references,
    }))
}

fn stage_file(
    source: &Path,
    destination: &Path,
    name: &str,
    budget: u64,
) -> Result<FileRef, ImportError> {
    let staging_error = |source| ImportError::Staging {
        name: name.to_owned(),
        source,
    };
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(staging_error)?;
    }
    let input = File::open(source).map_err(staging_error)?;
    let mut output = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(staging_error)?;
    let copied =
        io::copy(&mut input.take(budget.saturating_add(1)), &mut output).map_err(staging_error)?;
    if copied > budget {
        return Err(ImportError::TooLarge {
            limit: MAX_IMPORT_TOTAL_BYTES,
        });
    }
    let complete = complete_lines_len(&mut output, copied).map_err(staging_error)?;
    if complete < copied {
        output.set_len(complete).map_err(staging_error)?;
    }
    drop(output);
    file_ref(destination, name).map_err(staging_error)
}

/// Length of the prefix ending with the last newline (0 when there is none).
fn complete_lines_len(file: &mut File, len: u64) -> io::Result<u64> {
    const BLOCK: u64 = 64 * 1024;
    let mut end = len;
    let mut buffer = vec![0_u8; BLOCK as usize];
    while end > 0 {
        let start = end.saturating_sub(BLOCK);
        let size = usize::try_from(end - start).unwrap_or(0);
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer[..size])?;
        if let Some(index) = buffer[..size].iter().rposition(|b| *b == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

fn upload_one(
    uploader: &Uploader,
    job: &str,
    file: &LocalFile,
    resume_from: u64,
    io: &mut dyn ImportIo,
) -> Result<(), ImportError> {
    let name = &file.reference.name;
    let total = file.reference.size;
    {
        let (_, output) = io.streams();
        if resume_from > 0 {
            writeln!(
                output,
                "  {name} ({}, resuming at {})",
                human_bytes(total),
                human_bytes(resume_from)
            )?;
        } else {
            writeln!(output, "  {name} ({})", human_bytes(total))?;
        }
    }
    let mut next_report = resume_from.saturating_add(total / 4);
    let mut write_failure = None;
    let result = uploader.upload_file(
        job,
        &file.reference,
        &file.path,
        resume_from,
        &mut |done, size| {
            if size > 0 && done < size && done >= next_report {
                next_report = done.saturating_add(size / 4);
                let (_, output) = io.streams();
                if let Err(error) = writeln!(output, "    {}%", done.saturating_mul(100) / size) {
                    write_failure.get_or_insert(error);
                }
            }
        },
    );
    result?;
    if let Some(error) = write_failure {
        return Err(error.into());
    }
    Ok(())
}

fn confirm(io: &mut dyn ImportIo, server: &Url) -> Result<bool, ImportError> {
    let (input, output) = io.streams();
    write!(output, "Upload to {server}? [y/N]: ")?;
    output.flush()?;
    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        writeln!(output)?;
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn summary_text(
    repo: &RepoInfo,
    refs: &[crate::git::UnpublishedRef],
    snapshot: &Snapshot,
    manifest: &ImportManifest,
    options: &ImportOptions,
) -> String {
    let mut text = String::new();
    let branch = repo.head_branch.as_deref().unwrap_or("detached HEAD");
    let short_sha: String = repo.head_sha.chars().take(10).collect();
    let _ = writeln!(
        text,
        "Repository   {}/{} ({branch} @ {short_sha})",
        repo.owner, repo.name
    );
    let _ = writeln!(text, "GitHub       {}", repo.remote_url);
    match &manifest.bundle {
        Some(bundle) => {
            let names: Vec<&str> = refs
                .iter()
                .map(|reference| reference.branch.as_str())
                .collect();
            let _ = writeln!(
                text,
                "Unpublished  {} branch(es): {} -> bundle {}",
                refs.len(),
                names.join(", "),
                human_bytes(bundle.size)
            );
        }
        None => {
            let _ = writeln!(
                text,
                "Unpublished  none (all local commits are on a remote)"
            );
        }
    }
    match &snapshot.file {
        Some(file) => {
            let _ = writeln!(
                text,
                "Uncommitted  {} file(s), {} link(s), {} deleted -> snapshot {}",
                snapshot.files,
                snapshot.links,
                snapshot.deleted,
                human_bytes(file.size)
            );
        }
        None => {
            let _ = writeln!(text, "Uncommitted  none");
        }
    }
    for skipped in &snapshot.skipped {
        let reason = match skipped.reason {
            SkipReason::TooLarge { size } => format!("larger than 50 MiB ({})", human_bytes(size)),
            SkipReason::OutsideRepository => "resolves outside the repository".to_owned(),
            SkipReason::NotRegularFile => "not a regular file".to_owned(),
        };
        let _ = writeln!(text, "  skipped {}: {reason}", skipped.path.display());
    }
    let window = options.since.as_deref().unwrap_or(DEFAULT_SINCE);
    let _ = writeln!(
        text,
        "Sessions     since {} ({window} window, opened in this repository or its worktrees):",
        manifest.since
    );
    for harness in [ImportHarness::Claude, ImportHarness::Codex] {
        let matching: Vec<&SessionRef> = manifest
            .sessions
            .iter()
            .filter(|session| session.harness == harness)
            .collect();
        let bytes: u64 = matching
            .iter()
            .flat_map(|session| session.files.iter())
            .map(|file| file.size)
            .sum();
        let _ = writeln!(
            text,
            "  {:<7}{} session(s), {}",
            harness.as_str(),
            matching.len(),
            human_bytes(bytes)
        );
    }
    for session in manifest.sessions.iter().take(MAX_LISTED_SESSIONS) {
        let day: String = session.ended_at.chars().take(10).collect();
        let size: u64 = session.files.iter().map(|file| file.size).sum();
        let _ = writeln!(
            text,
            "  - [{}] {day} {} ({})",
            session.harness,
            session.title,
            human_bytes(size)
        );
    }
    if manifest.sessions.len() > MAX_LISTED_SESSIONS {
        let _ = writeln!(
            text,
            "  … and {} more",
            manifest.sessions.len() - MAX_LISTED_SESSIONS
        );
    }
    let _ = writeln!(
        text,
        "Total        {} in {} file(s) for {}",
        human_bytes(manifest.total_bytes()),
        manifest.files().count(),
        options.server
    );
    text
}

/// [`ImportIo`] for an interactive terminal: stdin/stdout, tokens from a [`TokenStore`] (device
/// login when none is saved), session locations from the environment and the system clock.
pub struct TerminalIo {
    input: io::StdinLock<'static>,
    output: io::Stdout,
    store: TokenStore,
    login: DeviceLoginOptions,
}

impl TerminalIo {
    /// Uses `store` for credentials and default device login options.
    #[must_use]
    pub fn new(store: TokenStore) -> Self {
        Self {
            input: io::stdin().lock(),
            output: io::stdout(),
            store,
            login: DeviceLoginOptions::default(),
        }
    }
}

impl ImportIo for TerminalIo {
    fn streams(&mut self) -> (&mut dyn BufRead, &mut dyn Write) {
        (&mut self.input, &mut self.output)
    }

    fn token(&mut self, server: &Url) -> Result<Secret, ImportError> {
        if let Some(token) = self.store.load(server)? {
            return Ok(token);
        }
        writeln!(
            self.output,
            "Not signed in to {server}; starting device login."
        )?;
        let output = &mut self.output;
        let mut write_failure = None;
        let token = device_login(
            server,
            &mut |start| {
                if let Err(error) = writeln!(
                    output,
                    "Open {} and confirm code {} (expires in {} min).",
                    start.verification_url,
                    start.user_code,
                    start.expires_in_s.div_ceil(60)
                ) {
                    write_failure.get_or_insert(error);
                }
            },
            &self.login,
        )?;
        if let Some(error) = write_failure {
            return Err(error.into());
        }
        self.store.save(server, &token)?;
        writeln!(self.output, "Signed in.")?;
        Ok(token)
    }

    fn session_sources(&self) -> SessionSources {
        SessionSources::from_env()
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_lines_cut_partial_tail() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("f");
        for (content, expected) in [
            (&b"a\nb\npartial"[..], 4_u64),
            (b"a\nb\n", 4),
            (b"no newline", 0),
            (b"", 0),
        ] {
            fs::write(&path, content)?;
            let mut file = File::open(&path)?;
            assert_eq!(
                complete_lines_len(&mut file, content.len() as u64)?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_bytes(10), "10 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
    }
}
