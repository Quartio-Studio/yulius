//! `yulius onboard` (alias `yulius import`) orchestration (SPEC-001 J4, §7.5; IMP-001..003,
//! IMP-006..009).
//!
//! 1. **Repository**: the GitHub checkout, its unpublished branches and uncommitted changes.
//! 2. **Yulius**: sign-in (device approval when needed), the repository's sandbox (or whether
//!    the caller may create it) and its onboarding state with what is pending.
//! 3. **Sessions**: the caller's Claude Code sessions of this repository; all, recent or picked;
//!    redacted on this computer (what was redacted is listed); compared with what the server
//!    holds, so only new or changed sessions are sent.
//! 4. **Upload**: one confirmation, a resumable checksummed upload with a progress bar, applied
//!    on the server.
//! 5. **Summary**: what landed, links to the sandbox and its Sessions page, and the onboarding
//!    state; new tasks wait for onboarding and start by themselves once it is ready. The CLI
//!    never starts onboarding: a person clicks Start onboarding on the sandbox page (SPEC-051).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use yulius_contracts::import::{
    FileRef, IMPORT_MANIFEST_VERSION, ImportManifest, ImportResult, MAX_CHUNK_BYTES,
    MAX_IMPORT_TOTAL_BYTES, MAX_SESSION_FILE_BYTES, MAX_SESSIONS_PER_IMPORT, ManifestError,
    OnboardPlan, OnboardPlanRequest, OnboardingStatus, SessionFingerprint, SessionRef,
};
use yulius_domain::redact::RedactionTally;

use crate::device::{DeviceError, DeviceLoginOptions, device_login};
use crate::git::{
    BUNDLE_NAME, RepoError, RepoInfo, UnpublishedRef, create_bundle, discover_repo,
    unpublished_refs,
};
use crate::http::{ApiError, RetryPolicy};
use crate::process::GitError;
use crate::redact::{RedactionReport, stage_redacted};
use crate::secret::Secret;
use crate::sessions::{DiscoveredSession, discover_claude_sessions};
use crate::since::{DEFAULT_SINCE, SinceError, parse_since};
use crate::snapshot::{SNAPSHOT_NAME, SkipReason, Snapshot, SnapshotError, snapshot_uncommitted};
use crate::time::{format_rfc3339, parse_rfc3339_millis};
use crate::token_store::{TokenStore, TokenStoreError};
use crate::ui::{PlainUi, TerminalUi, Tone, Ui, Unit, human_bytes};
use crate::upload::{UploadError, Uploader};

/// Steps shown while onboarding.
const STEPS: usize = 4;
/// Redaction labels listed per rule before eliding the rest.
const MAX_LABELS_PER_RULE: usize = 6;
/// Sessions listed with their redactions (`--show-redactions` lists all).
const MAX_REDACTED_SESSIONS: usize = 8;

/// Which sessions to upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChoice {
    /// Every session of the repository.
    All,
    /// Sessions active within the window (`--since`, default 14 days).
    Recent,
    /// Sessions picked one by one.
    Pick,
    /// No sessions (only the repository's local changes).
    None,
}

impl SessionChoice {
    /// Parses `all`, `recent`, `pick` or `none`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "recent" => Some(Self::Recent),
            "pick" => Some(Self::Pick),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// What to onboard and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOptions {
    /// Any directory inside the checkout.
    pub path: PathBuf,
    /// Yulius server base URL.
    pub server: Url,
    /// Which sessions; `None` asks a person (and means `Recent` without one).
    pub sessions: Option<SessionChoice>,
    /// Window of `Recent`, such as `14d` (asked when choosing recent interactively).
    pub since: Option<String>,
    /// Ask nothing.
    pub assume_yes: bool,
    /// Upload sessions only, not unpublished branches or uncommitted changes.
    pub sessions_only: bool,
    /// Show the plan and upload nothing.
    pub dry_run: bool,
    /// Only show the sandbox and its onboarding state.
    pub status_only: bool,
    /// List every redaction label of every session.
    pub show_redactions: bool,
}

impl ImportOptions {
    /// Options for `path` and `server` with every choice at its default.
    #[must_use]
    pub fn new(path: PathBuf, server: Url) -> Self {
        Self {
            path,
            server,
            sessions: None,
            since: None,
            assume_yes: false,
            sessions_only: false,
            dry_run: false,
            status_only: false,
            show_redactions: false,
        }
    }
}

/// Where Claude Code keeps its sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSources {
    /// Claude configuration directory (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub claude_dir: Option<PathBuf>,
}

impl SessionSources {
    /// Locations from `CLAUDE_CONFIG_DIR` and `HOME` (absolute paths only).
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
    /// What is shown and asked.
    fn ui(&mut self) -> &mut dyn Ui;
    /// Whether a person answers questions (otherwise defaults apply).
    fn interactive(&self) -> bool;
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

/// Why an onboarding run stopped.
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
        "the upload would exceed {limit} bytes; choose fewer sessions or commit and push large changes first"
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
    /// Uploading failed; running the command again resumes.
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
    /// The server refuses this person or repository (permissions, foreign owner).
    #[error("{0}")]
    Refused(String),
    /// Terminal I/O failed.
    #[error("terminal I/O failed: {0}")]
    Terminal(#[source] io::Error),
    /// The user declined the confirmation; nothing was uploaded.
    #[error("cancelled; nothing was uploaded")]
    Cancelled,
}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Terminal(error)
    }
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardOutcome {
    /// The plan the server answered (after comparing sessions).
    pub plan: OnboardPlan,
    /// Sessions found for the repository.
    pub sessions_found: usize,
    /// Sessions chosen.
    pub sessions_chosen: usize,
    /// Sessions sent (new or changed).
    pub sessions_sent: usize,
    /// Secrets redacted in the chosen sessions.
    pub redactions: u64,
    /// The applied import; `None` for `--status`, `--dry-run` or nothing to send.
    pub result: Option<ImportResult>,
}

/// A manifest file and the local copy to upload.
struct LocalFile {
    reference: FileRef,
    path: PathBuf,
    label: String,
}

/// One chosen session staged (redacted) for upload.
struct StagedSession {
    reference: SessionRef,
    files: Vec<LocalFile>,
    report: RedactionReport,
}

/// Runs one onboarding (see the module documentation). Re-running with unchanged content
/// uploads nothing: transcripts redact deterministically, bundle and snapshot are content
/// addressed, the plan leaves out sessions the server holds, and the server lists only files it
/// lacks (IMP-006). Nothing is pushed to GitHub.
///
/// # Errors
/// Any [`ImportError`]; [`ImportError::Cancelled`] when the user declines.
pub fn run_import(
    options: &ImportOptions,
    io: &mut dyn ImportIo,
) -> Result<OnboardOutcome, ImportError> {
    let interactive = io.interactive() && !options.assume_yes;
    let steps = if options.status_only { 2 } else { STEPS };

    // 1. Repository.
    let repo = discover_repo(&options.path)?;
    io.ui().step(1, steps, "Repository")?;
    show_repository(io.ui(), &repo)?;
    let staging = tempfile::Builder::new()
        .prefix("yulius-onboard-")
        .tempdir()
        .map_err(|source| ImportError::Staging {
            name: "staging directory".to_owned(),
            source,
        })?;
    let mut budget = MAX_IMPORT_TOTAL_BYTES;
    let mut local_files: Vec<LocalFile> = Vec::new();
    let mut bundle = None;
    let mut snapshot_file = None;
    if !options.sessions_only && !options.status_only {
        let refs = unpublished_refs(&repo.root)?;
        let bundle_path = staging.path().join(BUNDLE_NAME);
        bundle = create_bundle(&repo.root, &refs, &bundle_path)?;
        if let Some(reference) = &bundle {
            budget = take_budget(budget, reference.size)?;
            local_files.push(LocalFile {
                reference: reference.clone(),
                path: bundle_path,
                label: "unpublished branches".to_owned(),
            });
        }
        let snapshot_path = staging.path().join(SNAPSHOT_NAME);
        let snapshot = snapshot_uncommitted(&repo.root, &snapshot_path, budget)?;
        if let Some(reference) = &snapshot.file {
            budget = take_budget(budget, reference.size)?;
            local_files.push(LocalFile {
                reference: reference.clone(),
                path: snapshot_path,
                label: "uncommitted changes".to_owned(),
            });
        }
        show_local_changes(io.ui(), &refs, bundle.as_ref(), &snapshot)?;
        snapshot_file = snapshot.file;
    } else if options.sessions_only {
        io.ui()
            .fact("Local work", "not uploaded (--sessions-only)", Tone::Dim)?;
    }

    // 2. Yulius: sign-in, sandbox, onboarding.
    io.ui().step(2, steps, "Yulius")?;
    let token = io.token(&options.server)?;
    let tuning = io.upload_tuning();
    let uploader = Uploader::new(options.server.clone(), token)?
        .with_chunk_size(tuning.chunk_size)
        .with_retry_policy(tuning.retry);
    let mut plan = uploader.plan(&OnboardPlanRequest {
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        sessions: Vec::new(),
    })?;
    io.ui().fact(
        "Signed in",
        &format!("{} as @{}", host_of(&options.server), plan.login),
        Tone::Good,
    )?;
    match &plan.sandbox {
        Some(sandbox) => {
            io.ui().fact("Sandbox", &sandbox.url, Tone::Plain)?;
            show_onboarding(io.ui(), &sandbox.onboarding)?;
        }
        None if plan.can_create && !options.status_only => {
            io.ui().fact(
                "Sandbox",
                &format!("new: {}/{} is added when you upload", repo.owner, repo.name),
                Tone::Warn,
            )?;
        }
        None => {}
    }
    if let Some(refusal) = &plan.refusal {
        return Err(ImportError::Refused(refusal.clone()));
    }
    if options.status_only {
        let ready = plan.sandbox.as_ref().is_some_and(|s| s.onboarding.ready);
        let title = match &plan.sandbox {
            Some(_) if ready => "Ready: tasks can start.".to_owned(),
            Some(_) => "Not ready yet: tasks start once onboarding is done.".to_owned(),
            None => format!(
                "{}/{} is not on Yulius yet; run `yulius onboard` to add it.",
                repo.owner, repo.name
            ),
        };
        io.ui().done(&title, ready)?;
        return Ok(OnboardOutcome {
            plan,
            sessions_found: 0,
            sessions_chosen: 0,
            sessions_sent: 0,
            redactions: 0,
            result: None,
        });
    }

    // 3. Sessions: find, choose, redact, compare.
    io.ui().step(3, steps, "Claude Code sessions")?;
    let sources = io.session_sources();
    let found = discover_sessions(&repo, &sources, UNIX_EPOCH)?;
    let chosen = choose_sessions(options, io, &found, interactive)?;
    let mut staged = Vec::new();
    let mut redactions = RedactionReport::default();
    if !chosen.is_empty() {
        let total: u64 = chosen.iter().map(|s| found[*s].total_size()).sum();
        io.ui()
            .progress_start("Redacting secrets", total, Unit::Bytes);
        let mut done = 0_u64;
        for index in &chosen {
            let session = &found[*index];
            io.ui().progress_set(done, &session.title);
            if let Some(session) = stage_session(session, staging.path(), &mut budget, io.ui())? {
                redactions.merge(&session.report);
                staged.push(session);
            }
            done += found[*index].total_size();
        }
        io.ui().progress_set(total, "");
        io.ui().progress_finish(&format!(
            "Scanned {} for secrets",
            plural(staged.len(), "session")
        ))?;
        show_redactions(io.ui(), &staged, &redactions, options.show_redactions)?;
    }
    let mut to_send: Vec<&StagedSession> = staged.iter().collect();
    if !staged.is_empty() && plan.sandbox.is_some() {
        plan = uploader.plan(&OnboardPlanRequest {
            owner: repo.owner.clone(),
            name: repo.name.clone(),
            sessions: staged
                .iter()
                .map(|s| SessionFingerprint {
                    harness: s.reference.harness,
                    session_id: s.reference.session_id.clone(),
                    sha256: s.reference.files[0].sha256.clone(),
                    size: s.reference.files[0].size,
                })
                .collect(),
        })?;
        let states: BTreeMap<&str, &str> = plan
            .sessions
            .iter()
            .map(|s| (s.session_id.as_str(), s.state.as_str()))
            .collect();
        let count = |state: &str| states.values().filter(|s| **s == state).count();
        let mut line = format!(
            "{} new · {} changed · {} already there",
            count("new"),
            count("changed"),
            count("unchanged")
        );
        if count("taken") > 0 {
            let _ = write!(
                line,
                " · {} uploaded by someone else (kept)",
                count("taken")
            );
        }
        io.ui().fact("On Yulius", &line, Tone::Plain)?;
        to_send.retain(|s| {
            matches!(
                states.get(s.reference.session_id.as_str()),
                Some(&"new" | &"changed") | None
            )
        });
    }

    // 4. Upload.
    io.ui().step(4, steps, "Upload")?;
    let mut files: Vec<&LocalFile> = local_files.iter().collect();
    files.extend(to_send.iter().flat_map(|s| s.files.iter()));
    let manifest = ImportManifest {
        version: IMPORT_MANIFEST_VERSION,
        repo: repo.descriptor(),
        bundle: bundle.clone(),
        snapshot: snapshot_file.clone(),
        sessions: to_send.iter().map(|s| s.reference.clone()).collect(),
        since: format_rfc3339(UNIX_EPOCH),
    };
    manifest.validate()?;
    let outcome = |plan: OnboardPlan, result| OnboardOutcome {
        plan,
        sessions_found: found.len(),
        sessions_chosen: chosen.len(),
        sessions_sent: to_send.len(),
        redactions: redactions.tally.total(),
        result,
    };
    if files.is_empty() && plan.sandbox.is_some() {
        io.ui().fact(
            "Upload",
            "nothing new: everything is already on Yulius",
            Tone::Good,
        )?;
        let title = format!("{}/{} is up to date on Yulius", repo.owner, repo.name);
        io.ui().done(&title, true)?;
        if let Some(sandbox) = &plan.sandbox {
            show_links(io.ui(), &sandbox.url, &sandbox.sessions_url)?;
            show_onboarding(io.ui(), &sandbox.onboarding)?;
        }
        return Ok(outcome(plan, None));
    }
    let total_bytes: u64 = files.iter().map(|f| f.reference.size).sum();
    let what = describe_upload(to_send.len(), bundle.is_some(), snapshot_file.is_some());
    io.ui().fact(
        "Will send",
        &format!("{what} ({})", human_bytes(total_bytes)),
        Tone::Plain,
    )?;
    if options.dry_run {
        io.ui().done("Dry run: nothing was uploaded.", true)?;
        return Ok(outcome(plan, None));
    }
    if interactive
        && !io.ui().confirm(
            &format!("Upload {what} to {}?", host_of(&options.server)),
            true,
        )?
    {
        return Err(ImportError::Cancelled);
    }
    io.ui().progress_start(
        if plan.sandbox.is_some() {
            "Preparing the upload"
        } else {
            "Adding the repository to Yulius"
        },
        0,
        Unit::Items,
    );
    let job = uploader.create_job(&manifest);
    let job = match job {
        Ok(job) => {
            io.ui().progress_finish(if plan.sandbox.is_some() {
                "Upload prepared"
            } else {
                "Repository added"
            })?;
            job
        }
        Err(err) => {
            io.ui().progress_cancel();
            return Err(err.into());
        }
    };
    let by_name: BTreeMap<&str, &LocalFile> = files
        .iter()
        .map(|file| (file.reference.name.as_str(), *file))
        .collect();
    let missing_bytes: u64 = job
        .missing
        .iter()
        .filter_map(|m| {
            by_name
                .get(m.name.as_str())
                .map(|f| f.reference.size.saturating_sub(m.received_bytes))
        })
        .sum();
    let held = files.len().saturating_sub(job.missing.len());
    let resumed: u64 = job.missing.iter().map(|m| m.received_bytes).sum();
    if held > 0 || resumed > 0 {
        let mut note = String::new();
        if held > 0 {
            let _ = write!(note, "{} already on the server", plural(held, "file"));
        }
        if resumed > 0 {
            if !note.is_empty() {
                note.push_str("; ");
            }
            let _ = write!(note, "resuming after {}", human_bytes(resumed));
        }
        io.ui().detail(&note)?;
    }
    if !job.missing.is_empty() {
        io.ui()
            .progress_start("Uploading", missing_bytes, Unit::Bytes);
        let mut base = 0_u64;
        for missing in &job.missing {
            let file = by_name
                .get(missing.name.as_str())
                .ok_or_else(|| ImportError::UnknownFile(missing.name.clone()))?;
            let start = missing.received_bytes;
            let label = file.label.clone();
            {
                let ui = io.ui();
                let sent = uploader.upload_file(
                    &job.job_id,
                    &file.reference,
                    &file.path,
                    start,
                    &mut |done, _size| {
                        ui.progress_set(base + done.saturating_sub(start), &label);
                    },
                );
                if sent.is_err() {
                    ui.progress_cancel();
                }
                sent?;
            }
            base += file.reference.size.saturating_sub(start);
        }
        io.ui().progress_finish(&format!(
            "Uploaded {} ({})",
            plural(job.missing.len(), "file"),
            human_bytes(missing_bytes)
        ))?;
    }
    io.ui()
        .progress_start("Applying on the server", 0, Unit::Items);
    let result = match uploader.complete(&job.job_id) {
        Ok(result) => result,
        Err(err) => {
            io.ui().progress_cancel();
            return Err(err.into());
        }
    };
    io.ui().progress_finish("Applied")?;
    show_summary(io.ui(), &repo, &result, &plan.login, &redactions.tally)?;
    Ok(outcome(plan, Some(result)))
}

fn take_budget(budget: u64, size: u64) -> Result<u64, ImportError> {
    budget.checked_sub(size).ok_or(ImportError::TooLarge {
        limit: MAX_IMPORT_TOTAL_BYTES,
    })
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn host_of(url: &Url) -> String {
    url.host_str()
        .map_or_else(|| url.to_string(), str::to_owned)
}

fn describe_upload(sessions: usize, bundle: bool, snapshot: bool) -> String {
    let mut parts = Vec::new();
    if sessions > 0 {
        parts.push(plural(sessions, "session"));
    }
    if bundle {
        parts.push("unpublished branches".to_owned());
    }
    if snapshot {
        parts.push("uncommitted changes".to_owned());
    }
    if parts.is_empty() {
        "the repository link".to_owned()
    } else {
        parts.join(", ")
    }
}

fn show_repository(ui: &mut dyn Ui, repo: &RepoInfo) -> io::Result<()> {
    let branch = repo.head_branch.as_deref().unwrap_or("detached HEAD");
    let short_sha: String = repo.head_sha.chars().take(10).collect();
    ui.fact(
        "GitHub",
        &format!("{}/{}", repo.owner, repo.name),
        Tone::Plain,
    )?;
    ui.fact("Checkout", &format!("{branch} @ {short_sha}"), Tone::Dim)
}

fn show_local_changes(
    ui: &mut dyn Ui,
    refs: &[UnpublishedRef],
    bundle: Option<&FileRef>,
    snapshot: &Snapshot,
) -> io::Result<()> {
    match bundle {
        Some(bundle) => {
            let names: Vec<&str> = refs.iter().map(|r| r.branch.as_str()).collect();
            ui.fact(
                "Unpublished",
                &format!(
                    "{}: {} ({})",
                    plural(refs.len(), "branch"),
                    names.join(", "),
                    human_bytes(bundle.size)
                ),
                Tone::Plain,
            )?;
        }
        None => ui.fact("Unpublished", "none: every commit is on GitHub", Tone::Dim)?,
    }
    match &snapshot.file {
        Some(file) => ui.fact(
            "Uncommitted",
            &format!(
                "{}, {} deleted ({})",
                plural(snapshot.files + snapshot.links, "file"),
                snapshot.deleted,
                human_bytes(file.size)
            ),
            Tone::Plain,
        )?,
        None => ui.fact("Uncommitted", "none", Tone::Dim)?,
    }
    for skipped in &snapshot.skipped {
        let reason = match skipped.reason {
            SkipReason::TooLarge { size } => format!("larger than 50 MiB ({})", human_bytes(size)),
            SkipReason::OutsideRepository => "resolves outside the repository".to_owned(),
            SkipReason::NotRegularFile => "not a regular file".to_owned(),
        };
        ui.warn(&format!("left out {}: {reason}", skipped.path.display()))?;
    }
    Ok(())
}

/// The onboarding state with its summary and what is pending.
fn show_onboarding(ui: &mut dyn Ui, status: &OnboardingStatus) -> io::Result<()> {
    let tone = if status.ready { Tone::Good } else { Tone::Warn };
    let state = match status.state.as_str() {
        "not_needed" | "ready" => "● ready",
        "skipped" => "● ready (preparation skipped)",
        "preparing" => "● preparing",
        // The CLI never starts onboarding; a person does on the sandbox page (SPEC-051).
        "pending" => "● waiting for Start onboarding in the browser",
        other => other,
    };
    ui.fact("Onboarding", state, tone)?;
    ui.detail(&status.summary)?;
    for item in &status.pending {
        ui.detail(&format!("· {item}"))?;
    }
    if let Some(task) = &status.task
        && !status.ready
    {
        ui.detail(&format!("Follow it: {}", task.url))?;
    }
    if !status.ready {
        ui.detail("New tasks wait for it and start by themselves once onboarding is ready.")?;
    }
    Ok(())
}

fn show_links(ui: &mut dyn Ui, sandbox: &str, sessions: &str) -> io::Result<()> {
    ui.fact("Sandbox", sandbox, Tone::Plain)?;
    ui.fact("Sessions", sessions, Tone::Plain)
}

fn show_summary(
    ui: &mut dyn Ui,
    repo: &RepoInfo,
    result: &ImportResult,
    login: &str,
    redactions: &RedactionTally,
) -> io::Result<()> {
    ui.done(&format!("{}/{} is on Yulius", repo.owner, repo.name), true)?;
    ui.fact(
        "Sessions",
        &format!(
            "{} new · {} updated · {} unchanged, attributed to @{login}",
            result.sessions_imported, result.sessions_updated, result.sessions_unchanged
        ),
        Tone::Good,
    )?;
    if !redactions.is_empty() {
        ui.fact(
            "Secrets",
            &format!("{} redacted before upload", redactions.total()),
            Tone::Plain,
        )?;
    }
    let sessions_url = result
        .sessions_url
        .clone()
        .unwrap_or_else(|| format!("{}/sessions", result.sandbox_url));
    show_links(ui, &result.sandbox_url, &sessions_url)?;
    match &result.onboarding_status {
        Some(status) => show_onboarding(ui, status),
        None => ui.fact("Onboarding", &result.onboarding, Tone::Plain),
    }
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
    // Newest first: the order people choose from.
    sessions.sort_by(|a, b| {
        (
            b.ended_at.as_str(),
            b.started_at.as_str(),
            b.session_id.as_str(),
        )
            .cmp(&(
                a.ended_at.as_str(),
                a.started_at.as_str(),
                a.session_id.as_str(),
            ))
    });
    Ok(sessions)
}

fn ended_millis(session: &DiscoveredSession) -> i64 {
    parse_rfc3339_millis(&session.ended_at).unwrap_or(0)
}

fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn session_label(session: &DiscoveredSession) -> String {
    let day: String = session.ended_at.chars().take(10).collect();
    let branch = session
        .git_branch
        .as_deref()
        .map(|b| format!(", {b}"))
        .unwrap_or_default();
    format!(
        "{day}  {}  ({}{branch})",
        session.title,
        human_bytes(session.total_size())
    )
}

/// Indices of the chosen sessions (newest first).
fn choose_sessions(
    options: &ImportOptions,
    io: &mut dyn ImportIo,
    found: &[DiscoveredSession],
    interactive: bool,
) -> Result<Vec<usize>, ImportError> {
    let claude_dir = io
        .session_sources()
        .claude_dir
        .map_or_else(|| "~/.claude".to_owned(), |d| d.display().to_string());
    if found.is_empty() {
        io.ui().fact(
            "Found",
            &format!("no sessions of this repository in {claude_dir}/projects"),
            Tone::Dim,
        )?;
        return Ok(Vec::new());
    }
    let total: u64 = found.iter().map(DiscoveredSession::total_size).sum();
    let oldest: String = found
        .iter()
        .map(|s| s.started_at.as_str())
        .min()
        .unwrap_or("")
        .chars()
        .take(10)
        .collect();
    io.ui().fact(
        "Found",
        &format!(
            "{} ({}) since {oldest}, yours, in {claude_dir}",
            plural(found.len(), "session"),
            human_bytes(total)
        ),
        Tone::Plain,
    )?;
    let default_window = options
        .since
        .clone()
        .unwrap_or_else(|| DEFAULT_SINCE.to_owned());
    let recent_of = |window: &str, now: SystemTime| -> Result<Vec<usize>, ImportError> {
        let start = millis(now.checked_sub(parse_since(window)?).unwrap_or(UNIX_EPOCH));
        Ok(found
            .iter()
            .enumerate()
            .filter(|(_, s)| ended_millis(s) >= start)
            .map(|(i, _)| i)
            .collect())
    };
    let now = io.now();
    let default_recent = recent_of(&default_window, now)?;
    let choice = match options.sessions {
        Some(choice) => choice,
        None if interactive => {
            let options_text = vec![
                format!(
                    "Recent: the last {default_window} ({})",
                    plural(default_recent.len(), "session")
                ),
                format!(
                    "All {} ({})",
                    plural(found.len(), "session"),
                    human_bytes(total)
                ),
                "Pick sessions…".to_owned(),
                "None: only the repository".to_owned(),
            ];
            match io.ui().choose(
                "Which Claude Code sessions should go to Yulius?",
                &options_text,
                0,
            )? {
                1 => SessionChoice::All,
                2 => SessionChoice::Pick,
                3 => SessionChoice::None,
                _ => SessionChoice::Recent,
            }
        }
        None => SessionChoice::Recent,
    };
    let mut chosen = match choice {
        SessionChoice::All => (0..found.len()).collect(),
        SessionChoice::None => Vec::new(),
        SessionChoice::Recent => {
            if interactive && options.since.is_none() && options.sessions.is_none() {
                let window = io
                    .ui()
                    .ask("Window (for example 14d, 36h, 2w)", &default_window)?;
                recent_of(&window, now)?
            } else {
                default_recent
            }
        }
        SessionChoice::Pick => {
            let labels: Vec<String> = found.iter().map(session_label).collect();
            let marks: Vec<bool> = (0..found.len())
                .map(|i| default_recent.contains(&i))
                .collect();
            if interactive || io.interactive() {
                io.ui().pick("Sessions to upload", &labels, &marks)?
            } else {
                default_recent
            }
        }
    };
    if chosen.len() > MAX_SESSIONS_PER_IMPORT {
        io.ui().warn(&format!(
            "{} chosen; the newest {MAX_SESSIONS_PER_IMPORT} go now, run the command again for the rest",
            plural(chosen.len(), "session")
        ))?;
        chosen.truncate(MAX_SESSIONS_PER_IMPORT);
    }
    let bytes: u64 = chosen.iter().map(|i| found[*i].total_size()).sum();
    io.ui().fact(
        "Chosen",
        &format!(
            "{} ({})",
            plural(chosen.len(), "session"),
            human_bytes(bytes)
        ),
        Tone::Plain,
    )?;
    Ok(chosen)
}

/// Stages a session's transcripts with secrets redacted (complete lines only, so a live session
/// yields a consistent snapshot) and builds its manifest entry. Sessions whose main transcript
/// stages empty or exceeds the per-session bound are left out (with a warning for the latter).
fn stage_session(
    session: &DiscoveredSession,
    staging: &Path,
    budget: &mut u64,
    ui: &mut dyn Ui,
) -> Result<Option<StagedSession>, ImportError> {
    let mut references = Vec::new();
    let mut files = Vec::new();
    let mut report = RedactionReport::default();
    for (index, file) in session.files.iter().enumerate() {
        let destination = staging.join(&file.name);
        let limit = (*budget).min(MAX_SESSION_FILE_BYTES);
        let staged = stage_redacted(&file.path, &destination, &file.name, limit);
        let (reference, file_report) = match staged {
            Ok(staged) => staged,
            Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
                if *budget < MAX_SESSION_FILE_BYTES {
                    return Err(ImportError::TooLarge {
                        limit: MAX_IMPORT_TOTAL_BYTES,
                    });
                }
                if index == 0 {
                    ui.warn(&format!(
                        "left out \"{}\": larger than {} after redaction",
                        session.title,
                        human_bytes(MAX_SESSION_FILE_BYTES)
                    ))?;
                    return Ok(None);
                }
                continue;
            }
            Err(source) => {
                return Err(ImportError::Staging {
                    name: file.name.clone(),
                    source,
                });
            }
        };
        if reference.size == 0 {
            if index == 0 {
                return Ok(None);
            }
            continue;
        }
        *budget -= reference.size;
        report.merge(&file_report);
        references.push(reference.clone());
        files.push(LocalFile {
            reference,
            path: destination,
            label: format!("session: {}", session.title),
        });
    }
    let redactions = u32::try_from(report.tally.total()).unwrap_or(u32::MAX);
    Ok(Some(StagedSession {
        reference: SessionRef {
            harness: session.harness,
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            cwd_relative: session.cwd_relative.clone(),
            started_at: session.started_at.clone(),
            ended_at: session.ended_at.clone(),
            git_branch: session.git_branch.clone(),
            files: references,
            redactions,
        },
        files,
        report,
    }))
}

/// What was redacted: totals by rule (with key names, never values), images left out, and the
/// sessions concerned.
fn show_redactions(
    ui: &mut dyn Ui,
    staged: &[StagedSession],
    total: &RedactionReport,
    all: bool,
) -> io::Result<()> {
    if total.tally.is_empty() {
        ui.fact("Secrets", "none found", Tone::Good)?;
    } else {
        let sessions = staged.iter().filter(|s| !s.report.tally.is_empty()).count();
        ui.fact(
            "Redacted",
            &format!(
                "{} in {} (replaced by [REDACTED:<rule>])",
                plural(
                    usize::try_from(total.tally.total()).unwrap_or(usize::MAX),
                    "secret"
                ),
                plural(sessions, "session")
            ),
            Tone::Warn,
        )?;
        for (rule, count) in &total.tally.by_rule {
            let labels: Vec<&str> = total
                .tally
                .by_label
                .keys()
                .filter(|(r, _)| r == rule)
                .map(|(_, label)| label.as_str())
                .collect();
            let shown = if all {
                labels.len()
            } else {
                MAX_LABELS_PER_RULE
            };
            let mut line = format!(
                "{rule} ×{count}: {}",
                labels
                    .iter()
                    .take(shown)
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if labels.len() > shown {
                let _ = write!(line, ", … {} more", labels.len() - shown);
            }
            ui.detail(&line)?;
        }
        let listed = if all {
            staged.len()
        } else {
            MAX_REDACTED_SESSIONS
        };
        let concerned: Vec<&StagedSession> = staged
            .iter()
            .filter(|s| !s.report.tally.is_empty())
            .collect();
        for session in concerned.iter().take(listed) {
            let day: String = session.reference.ended_at.chars().take(10).collect();
            ui.detail(&format!(
                "{day} \"{}\": {}",
                session.reference.title,
                plural(
                    usize::try_from(session.report.tally.total()).unwrap_or(usize::MAX),
                    "secret"
                )
            ))?;
        }
        if concerned.len() > listed {
            ui.detail(&format!(
                "… and {} more (--show-redactions lists all)",
                plural(concerned.len() - listed, "session")
            ))?;
        }
    }
    if total.images > 0 {
        ui.detail(&format!(
            "{} left out (screenshots can show secrets)",
            plural(usize::try_from(total.images).unwrap_or(usize::MAX), "image")
        ))?;
    }
    if total.long_lines > 0 {
        ui.detail(&format!(
            "{} over 16 MiB left out",
            plural(
                usize::try_from(total.long_lines).unwrap_or(usize::MAX),
                "line"
            )
        ))?;
    }
    Ok(())
}

/// [`ImportIo`] for the process's terminal: [`TerminalUi`] when a person is there, plain lines
/// otherwise; tokens from a [`TokenStore`] (device login when none is saved), session locations
/// from the environment and the system clock.
pub struct TerminalIo {
    ui: Box<dyn Ui>,
    interactive: bool,
    store: TokenStore,
    login: DeviceLoginOptions,
}

impl TerminalIo {
    /// Uses `store` for credentials and default device login options.
    #[must_use]
    pub fn new(store: TokenStore) -> Self {
        let interactive = TerminalUi::attended();
        let ui: Box<dyn Ui> = if interactive {
            Box::new(TerminalUi::new())
        } else {
            Box::new(PlainUi::new(io::stdin().lock(), io::stdout()))
        };
        Self {
            ui,
            interactive,
            store,
            login: DeviceLoginOptions::default(),
        }
    }
}

impl ImportIo for TerminalIo {
    fn ui(&mut self) -> &mut dyn Ui {
        self.ui.as_mut()
    }

    fn interactive(&self) -> bool {
        self.interactive
    }

    fn token(&mut self, server: &Url) -> Result<Secret, ImportError> {
        if let Some(token) = self.store.load(server)? {
            return Ok(token);
        }
        self.ui.fact(
            "Sign in",
            &format!(
                "not signed in to {}; approve this computer in the browser",
                host_of(server)
            ),
            Tone::Warn,
        )?;
        let ui = self.ui.as_mut();
        let mut shown = None;
        let token = device_login(
            server,
            &mut |start| {
                let result = ui
                    .detail(&format!(
                        "Open {} and confirm the code {} (expires in {} min).",
                        start.verification_url,
                        start.user_code,
                        start.expires_in_s.div_ceil(60)
                    ))
                    .map(|()| {
                        ui.progress_start("Waiting for approval", 0, Unit::Items);
                    });
                shown.get_or_insert(result);
            },
            &self.login,
        );
        if shown.is_some() {
            if token.is_ok() {
                let _ = self.ui.progress_finish("Approved");
            } else {
                self.ui.progress_cancel();
            }
        }
        let token = token?;
        if let Some(Err(error)) = shown {
            return Err(error.into());
        }
        self.store.save(server, &token)?;
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
    fn choices_and_descriptions() {
        assert_eq!(SessionChoice::parse("ALL"), Some(SessionChoice::All));
        assert_eq!(SessionChoice::parse("pick"), Some(SessionChoice::Pick));
        assert_eq!(SessionChoice::parse("x"), None);
        assert_eq!(
            describe_upload(2, true, false),
            "2 sessions, unpublished branches"
        );
        assert_eq!(describe_upload(0, false, false), "the repository link");
        assert_eq!(plural(1, "file"), "1 file");
    }
}
