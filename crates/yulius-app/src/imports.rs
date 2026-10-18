//! Server half of `yulius import` (SPEC-001 IMP-001..006, J4; SPEC-002 §6.5, §8a). The wire
//! protocol is documented in [`yulius_contracts::import`].
//!
//! A job (`created → receiving → received → applying → completed | failed`) belongs to one
//! user and one sandbox; the sandbox is created through the import path when the repository is
//! not linked yet. Files live under `<data_dir>/sandboxes/<sandbox>/imports/<job>/<name>` and
//! are content addressed across the sandbox's earlier jobs: a file already stored completely is
//! linked into the new job and not requested again, and a partially stored one resumes at its
//! received length. Chunks are idempotent by (job, name, offset, sha256). Completing applies the
//! job: the bundle becomes `import/<login>/…` branches, the snapshot is extracted into a
//! worktree on `import/<login>/snapshot-<date>` created from the client's `HEAD` and committed,
//! and every session becomes (or replaces) a read-only session document of the sandbox,
//! attributed to the uploader ([`crate::session_docs`], SES-001); any member with write access
//! can continue one natively as a task later (IMP-004). Re-applying skips unchanged sessions and
//! an already applied snapshot. Tasks imported as archived tasks by earlier versions keep
//! working (their worktree base is read from the applied job).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use yulius_contracts::import::{
    CreateImportRequest, CreateImportResponse, FileProgress, FileRef, ImportHarness,
    ImportManifest, ImportResult, MAX_IMPORT_TOTAL_BYTES, MAX_SESSIONS_PER_IMPORT, OnboardPlan,
    OnboardPlanRequest, OnboardSandbox, SNAPSHOT_DELETED_ENTRY, SNAPSHOT_FILES_PREFIX, SessionPlan,
    SessionRef, is_safe_session_id, is_sha256_hex, is_valid_chunk_len,
};
use yulius_domain::cron::CivilDate;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, hex_token};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, TaskRecord, UserRecord};
use crate::playground::UploadTarget;
use crate::ports::store::TaskFieldUpdate;
use crate::ports::store_ext::ImportJobRecord;
use crate::projects::{blocking, clone_url, identity};
use crate::session_docs::StoreOutcome;
use crate::supervisor::branch_for;

/// Earlier jobs of a sandbox examined for content reuse, job reuse and applied state.
pub const JOB_LIST_LIMIT: u32 = 200;
/// An `applying` job untouched for this long is considered abandoned and may be re-applied.
pub const STALE_APPLY_MS: i64 = 15 * 60 * 1000;
/// Most entries read from one snapshot archive.
pub const MAX_SNAPSHOT_ENTRIES: u64 = 200_000;
/// Largest `DELETED` list accepted from a snapshot archive.
pub const MAX_DELETED_LIST_BYTES: u64 = 16 * 1024 * 1024;
/// Longest transcript line parsed when rewriting working directories (longer lines are copied
/// verbatim, SPEC-002 §9).
pub const MAX_TRANSCRIPT_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Deepest folder level of auxiliary files copied (each level holds one open handle).
const MAX_AUX_DEPTH: usize = 16;
/// Most auxiliary transcript files copied for one Claude session.
const MAX_AUX_FILES: usize = 1_000;
/// Snapshot branch name candidates tried per day.
const MAX_SNAPSHOT_BRANCHES: u32 = 50;
/// Version of the stored applied-import record.
const APPLIED_VERSION: u32 = 1;

/// Import job state (SPEC-002 §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobState {
    /// Created; files missing.
    Created,
    /// At least one chunk arrived; files missing.
    Receiving,
    /// Every file is complete.
    Received,
    /// Being applied.
    Applying,
    /// Applied; the result is stored.
    Completed,
    /// Applying failed; completing again retries.
    Failed,
    /// A new playground's upload replaced by a newer one of its person (SPEC-171); its files are
    /// gone and it never completes.
    Superseded,
}

impl JobState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Receiving => "receiving",
            Self::Received => "received",
            Self::Applying => "applying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Superseded => "superseded",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "created" => Some(Self::Created),
            "receiving" => Some(Self::Receiving),
            "received" => Some(Self::Received),
            "applying" => Some(Self::Applying),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }

    /// Whether uploads may still change the job.
    #[must_use]
    pub fn accepts_uploads(self) -> bool {
        matches!(
            self,
            Self::Created | Self::Receiving | Self::Received | Self::Failed
        )
    }
}

/// What applying a job did; stored as the job result (versioned JSON, internal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedImport {
    /// Record version.
    pub version: u32,
    /// The response returned to the CLI.
    pub result: ImportResult,
    /// Branches created or updated from the bundle.
    pub bundle_refs: Vec<String>,
    /// The applied snapshot.
    pub snapshot: Option<AppliedSnapshot>,
    /// Tasks created from sessions.
    pub tasks: Vec<AppliedTask>,
}

/// An applied snapshot archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedSnapshot {
    /// Archive digest.
    pub sha256: String,
    /// Client `HEAD` the snapshot applies to.
    pub head_sha: String,
    /// Branch holding the snapshot commit.
    pub branch: String,
    /// Whether the snapshot changed anything (a commit was made).
    pub committed: bool,
    /// Regular files written.
    pub files: u64,
    /// Symbolic links written.
    pub links: u64,
    /// Paths deleted.
    pub deleted: u64,
    /// Entries refused (unsafe paths, links escaping the worktree, other entry types).
    pub skipped: u64,
}

/// A task created from an imported session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedTask {
    /// Task id.
    pub task_id: String,
    /// Harness wire name.
    pub harness: String,
    /// Provider session id.
    pub session_id: String,
    /// Revision the task worktree starts from when it is first continued (the session's
    /// imported branch or snapshot); `None` = the integration branch.
    pub base: Option<String>,
}

fn harness_of(harness: ImportHarness) -> Harness {
    match harness {
        ImportHarness::Claude => Harness::ClaudeCode,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_token(Sha256::digest(bytes).as_slice())
}

fn is_object_id(text: &str) -> bool {
    (text.len() == 40 || text.len() == 64)
        && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fff…](Z|±HH:MM)` to epoch milliseconds.
#[must_use]
pub fn parse_rfc3339(text: &str) -> Option<Timestamp> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't')
    {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let date = CivilDate::new(
        num(0..4)?,
        u8::try_from(num(5..7)?).ok()?,
        u8::try_from(num(8..10)?).ok()?,
    )?;
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0_i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let padded: String = fraction[..digits]
            .chars()
            .chain("000".chars())
            .take(3)
            .collect();
        millis = padded.parse().ok()?;
        rest = &fraction[digits..];
    }
    let offset_minutes = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours: i64 = rest.get(1..3)?.parse().ok()?;
            let minutes: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (hours * 60 + minutes)
        }
    };
    let seconds =
        date.days_since_epoch() * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    Some(Timestamp(seconds.checked_mul(1000)?.checked_add(millis)?))
}

/// The branch prefix component for a user (`import/<login>/…`).
fn import_login(user: &UserRecord) -> String {
    user.github_login
        .as_deref()
        .filter(|login| {
            !login.is_empty()
                && login.len() <= 39
                && !login.starts_with('-')
                && login
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or("user")
        .to_owned()
}

fn progress_response(
    job_id: &str,
    manifest: &ImportManifest,
    received: &BTreeMap<String, u64>,
) -> CreateImportResponse {
    let mut missing: Vec<FileProgress> = manifest
        .files()
        .filter_map(|file| {
            let got = received
                .get(&file.name)
                .copied()
                .unwrap_or(0)
                .min(file.size);
            (got < file.size).then(|| FileProgress {
                name: file.name.clone(),
                received_bytes: got,
            })
        })
        .collect();
    missing.sort_by(|a, b| a.name.cmp(&b.name));
    CreateImportResponse {
        job_id: job_id.to_owned(),
        missing,
    }
}

fn all_complete(manifest: &ImportManifest, received: &BTreeMap<String, u64>) -> bool {
    manifest
        .files()
        .all(|file| received.get(&file.name).copied().unwrap_or(0) >= file.size)
}

fn parse_manifest(job: &ImportJobRecord) -> AppResult<ImportManifest> {
    serde_json::from_str(&job.manifest).map_err(AppError::internal)
}

fn applied_of(job: &ImportJobRecord) -> Option<AppliedImport> {
    if job.state != JobState::Completed.as_str() {
        return None;
    }
    serde_json::from_str(job.result.as_deref()?).ok()
}

impl App {
    fn import_dir(&self, sandbox: &SandboxId, job: &str) -> PathBuf {
        self.sandbox_dir(sandbox).join("imports").join(job)
    }

    async fn owned_job(&self, ctx: &AuthContext, job_id: &str) -> AppResult<ImportJobRecord> {
        self.deps
            .store
            .import_job(job_id)
            .await?
            .filter(|job| job.user_id == ctx.user.id)
            .ok_or(AppError::NotFound)
    }

    /// The sandbox an import goes to: the linked one (write access needed) or a new one
    /// created through the import path (admin or maintain needed, IMP-002).
    async fn import_target(
        self: &Arc<Self>,
        ctx: &AuthContext,
        owner: &str,
        name: &str,
    ) -> AppResult<SandboxRecord> {
        refuse_non_github(owner)?;
        match self.deps.store.sandbox_by_repo(owner, name).await? {
            Some(existing) => {
                let (record, _) = self.require(&ctx.user, &existing.id, Action::Work).await?;
                Ok(record)
            }
            None => self.import_repository(&ctx.user, owner, name, false).await,
        }
    }

    /// `POST /api/onboard/plan` (`yulius onboard`): the repository's sandbox with its onboarding
    /// state and what is pending, whether the caller may create it, and which offered sessions
    /// the server still needs (IMP-006). Nothing is changed.
    ///
    /// # Errors
    /// `Invalid` requests; GitHub failures while checking a new repository are reported as the
    /// refusal, not as errors.
    pub async fn onboard_plan(
        self: &Arc<Self>,
        ctx: &AuthContext,
        req: &OnboardPlanRequest,
    ) -> AppResult<OnboardPlan> {
        if req.sessions.len() > MAX_SESSIONS_PER_IMPORT {
            return Err(AppError::Invalid(format!(
                "at most {MAX_SESSIONS_PER_IMPORT} sessions per request"
            )));
        }
        if let Some(bad) = req
            .sessions
            .iter()
            .find(|s| !is_safe_session_id(&s.session_id) || !is_sha256_hex(&s.sha256))
        {
            return Err(AppError::Invalid(format!(
                "invalid session fingerprint {:?}",
                bad.session_id
            )));
        }
        let login = self.login_of(&ctx.user.id).await;
        let new_states = || {
            req.sessions
                .iter()
                .map(|s| SessionPlan {
                    session_id: s.session_id.clone(),
                    state: "new".to_owned(),
                })
                .collect::<Vec<_>>()
        };
        if req.playground {
            if !req.owner.trim().is_empty() {
                return Err(AppError::Invalid(
                    "a playground upload names no GitHub owner".into(),
                ));
            }
            match self.own_playground(&ctx.user, &req.name).await? {
                Some(record) => return self.plan_for(ctx, req, record, login).await,
                None => {
                    // A new playground is made by the upload, within the person's limit (PGL-002).
                    let refusal = match self.playground_quota(&ctx.user).await {
                        Ok(()) => None,
                        Err(AppError::Conflict(reason)) => Some(reason),
                        Err(err) => return Err(err),
                    };
                    return Ok(OnboardPlan {
                        login,
                        sandbox: None,
                        can_create: refusal.is_none(),
                        refusal,
                        sessions: new_states(),
                    });
                }
            }
        }
        refuse_non_github(req.owner.trim())?;
        let Some(record) = self
            .deps
            .store
            .sandbox_by_repo(req.owner.trim(), req.name.trim())
            .await?
        else {
            let (can_create, refusal) = match self
                .import_permission(&ctx.user, req.owner.trim(), req.name.trim())
                .await
            {
                Ok(_) => (true, None),
                Err(err) => (false, Some(err.public_message())),
            };
            return Ok(OnboardPlan {
                login,
                sandbox: None,
                can_create,
                refusal,
                sessions: new_states(),
            });
        };
        self.plan_for(ctx, req, record, login).await
    }

    /// The plan of an upload into `record`, an existing sandbox: the caller's access, which
    /// offered sessions it needs and its onboarding state.
    async fn plan_for(
        self: &Arc<Self>,
        ctx: &AuthContext,
        req: &OnboardPlanRequest,
        record: SandboxRecord,
        login: String,
    ) -> AppResult<OnboardPlan> {
        let new_states = || {
            req.sessions
                .iter()
                .map(|s| SessionPlan {
                    session_id: s.session_id.clone(),
                    state: "new".to_owned(),
                })
                .collect::<Vec<_>>()
        };
        let level = self
            .access_level(&ctx.user, &record)
            .await
            .unwrap_or(AccessLevel::None);
        if level == AccessLevel::None {
            return Ok(OnboardPlan {
                login,
                sandbox: None,
                can_create: false,
                refusal: Some(format!(
                    "{}/{} is a sandbox you cannot see; ask for access on GitHub",
                    record.repo_owner, record.repo_name
                )),
                sessions: new_states(),
            });
        }
        let refusal = (!level.allows(Action::Work)).then(|| {
            format!(
                "uploading needs write access to {}/{} on GitHub; you have {}",
                record.repo_owner,
                record.repo_name,
                level.as_str()
            )
        });
        let mut sessions = Vec::with_capacity(req.sessions.len());
        for offered in &req.sessions {
            let state = self
                .session_plan_state(
                    &record.id,
                    &ctx.user.id,
                    harness_of(offered.harness),
                    &offered.session_id,
                    &offered.sha256,
                )
                .await?;
            sessions.push(SessionPlan {
                session_id: offered.session_id.clone(),
                state: state.to_owned(),
            });
        }
        self.refresh_onboarding(&record.id).await?;
        let record = self
            .deps
            .store
            .sandbox(&record.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let url = format!("{}/s/{}", self.config.public_url, record.slug);
        Ok(OnboardPlan {
            login,
            sandbox: Some(OnboardSandbox {
                id: record.id.to_string(),
                slug: record.slug.clone(),
                sessions_url: format!("{url}/sessions"),
                url,
                onboarding: self.onboarding_status(&record).await?,
            }),
            can_create: false,
            refusal,
            sessions,
        })
    }

    /// `POST /api/import`: validates the manifest and the repository, creates (or reuses) the
    /// job and lists the files the server still needs.
    ///
    /// # Errors
    /// `Invalid` manifests and non-GitHub or foreign repositories, `Forbidden` without the
    /// needed permission, `Upstream` GitHub/Git failures.
    pub async fn create_import(
        self: &Arc<Self>,
        ctx: &AuthContext,
        req: &CreateImportRequest,
    ) -> AppResult<CreateImportResponse> {
        let manifest = &req.manifest;
        manifest
            .validate()
            .map_err(|err| AppError::Invalid(err.to_string()))?;
        let repo = &manifest.repo;
        if !is_object_id(&repo.head_sha) {
            return Err(AppError::Invalid("head_sha is not a Git object id".into()));
        }
        let store = &self.deps.store;
        let canonical = serde_json::to_string(manifest).map_err(AppError::internal)?;
        let sandbox_id = if manifest.playground {
            match self
                .playground_upload_target(ctx, manifest, &canonical)
                .await?
            {
                UploadTarget::Into(id) => id,
                UploadTarget::Resume(job) => {
                    return Ok(progress_response(&job.id, manifest, &job.received));
                }
            }
        } else {
            let expected = format!("https://github.com/{}/{}", repo.owner, repo.name);
            let remote = repo.remote_url.trim_end_matches('/');
            let remote = remote.strip_suffix(".git").unwrap_or(remote);
            if !remote.eq_ignore_ascii_case(&expected) {
                return Err(AppError::Invalid(format!(
                    "every Yulius project needs a GitHub repository: the remote must be {expected}"
                )));
            }
            self.import_target(ctx, &repo.owner, &repo.name).await?.id
        };
        let jobs = store.import_jobs(&sandbox_id, JOB_LIST_LIMIT).await?;
        if let Some(job) = jobs.iter().find(|job| {
            job.user_id == ctx.user.id
                && job.manifest == canonical
                && JobState::parse(&job.state).is_some_and(JobState::accepts_uploads)
        }) {
            return Ok(progress_response(&job.id, manifest, &job.received));
        }
        // Content reuse: the largest stored prefix of a file with the same digest and size.
        let mut plan = Vec::new();
        for file in manifest.files() {
            let mut best: Option<(PathBuf, u64)> = None;
            for job in &jobs {
                let Ok(earlier) = parse_manifest(job) else {
                    continue;
                };
                for candidate in earlier.files() {
                    if candidate.sha256 != file.sha256 || candidate.size != file.size {
                        continue;
                    }
                    let got = job
                        .received
                        .get(&candidate.name)
                        .copied()
                        .unwrap_or(0)
                        .min(candidate.size);
                    if got > best.as_ref().map_or(0, |(_, n)| *n) {
                        best = Some((
                            self.import_dir(&sandbox_id, &job.id).join(&candidate.name),
                            got,
                        ));
                    }
                }
            }
            plan.push(StagedFile {
                name: file.name.clone(),
                size: file.size,
                reuse: best,
            });
        }
        let job_id = self.deps.crypto.new_id();
        let job_dir = self.import_dir(&sandbox_id, &job_id);
        let received = blocking(move || stage_job_files(&job_dir, plan)).await?;
        let now = self.now();
        let state = if all_complete(manifest, &received) {
            JobState::Received
        } else {
            JobState::Created
        };
        store
            .insert_import_job(&ImportJobRecord {
                id: job_id.clone(),
                sandbox_id: Some(sandbox_id.clone()),
                user_id: ctx.user.id.clone(),
                state: state.as_str().to_owned(),
                manifest: canonical,
                received: received.clone(),
                result: None,
                created_at: now,
                updated_at: now,
            })
            .await?;
        store
            .audit(
                now,
                &self.login_of(&ctx.user.id).await,
                "import.created",
                Some(&job_id),
                Some(&format!(
                    "{}/{}{} files={} sessions={}",
                    repo.owner,
                    repo.name,
                    if manifest.playground {
                        " (playground)"
                    } else {
                        ""
                    },
                    manifest.files().count(),
                    manifest.sessions.len()
                )),
            )
            .await?;
        Ok(progress_response(&job_id, manifest, &received))
    }

    /// `PUT /api/import/{job}/files/{name}?offset=N`: stores one chunk. Idempotent by
    /// (job, name, offset, sha256): chunks ending at or before the received length are
    /// acknowledged; an offset beyond it is a `Conflict`; a body whose digest differs from
    /// `sha256` is `Invalid`. The last chunk triggers whole-file verification (a mismatch resets
    /// the file to offset 0).
    ///
    /// # Errors
    /// See above; `NotFound` for unknown jobs, files or other users' jobs.
    pub async fn put_import_chunk(
        &self,
        ctx: &AuthContext,
        job_id: &str,
        name: &str,
        offset: u64,
        sha256: &str,
        body: &[u8],
    ) -> AppResult<FileProgress> {
        let job = self.owned_job(ctx, job_id).await?;
        let manifest = parse_manifest(&job)?;
        let file = manifest
            .files()
            .find(|file| file.name == name)
            .cloned()
            .ok_or(AppError::NotFound)?;
        let len = u64::try_from(body.len()).unwrap_or(u64::MAX);
        if !is_valid_chunk_len(len) {
            return Err(AppError::Invalid("the chunk exceeds 8 MiB".into()));
        }
        if !is_sha256_hex(sha256) || sha256_hex(body) != sha256 {
            return Err(AppError::Invalid("chunk digest mismatch".into()));
        }
        let progress = |received_bytes| FileProgress {
            name: file.name.clone(),
            received_bytes,
        };
        let received = job
            .received
            .get(&file.name)
            .copied()
            .unwrap_or(0)
            .min(file.size);
        if received >= file.size {
            return Ok(progress(file.size));
        }
        if !JobState::parse(&job.state).is_some_and(JobState::accepts_uploads) {
            return Err(AppError::Conflict(format!(
                "the import is {}; no more uploads are accepted",
                job.state
            )));
        }
        if offset > received {
            return Err(AppError::Conflict(format!(
                "offset {offset} is beyond the {received} bytes received for {name}"
            )));
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| AppError::Invalid("offset overflow".into()))?;
        if end > file.size {
            return Err(AppError::Invalid(format!(
                "the chunk extends beyond the declared size of {name}"
            )));
        }
        if end <= received {
            return Ok(progress(received));
        }
        let skip = usize::try_from(received - offset).map_err(AppError::internal)?;
        let fresh = body[skip..].to_vec();
        let fresh_len = u64::try_from(fresh.len()).unwrap_or(u64::MAX);
        let new_len = received + fresh_len;
        let sandbox = job
            .sandbox_id
            .clone()
            .ok_or_else(|| AppError::Internal("import job without sandbox".into()))?;
        let path = self.import_dir(&sandbox, &job.id).join(&file.name);
        let expected = file.sha256.clone();
        let complete = new_len == file.size;
        let verified = blocking(move || {
            write_at(&path, received, &fresh)?;
            if complete {
                Ok(Some(sha256_file(&path)? == expected))
            } else {
                Ok(None)
            }
        })
        .await?;
        let store = &self.deps.store;
        let now = self.now();
        if verified == Some(false) {
            let path = self.import_dir(&sandbox, &job.id).join(&file.name);
            blocking(move || truncate_file(&path, 0)).await?;
            let _ = store
                .advance_import_file(&job.id, &file.name, received, 0, now)
                .await?;
            return Err(AppError::Invalid(format!(
                "{name} does not match its sha256; upload it again from offset 0"
            )));
        }
        if !store
            .advance_import_file(&job.id, &file.name, received, new_len, now)
            .await?
        {
            return Err(AppError::Conflict(
                "another upload of this file advanced concurrently; resume from the reported offset"
                    .into(),
            ));
        }
        let current = store.import_job(&job.id).await?.ok_or(AppError::NotFound)?;
        if all_complete(&manifest, &current.received) {
            store
                .transition_import_job(
                    &job.id,
                    &[JobState::Created.as_str(), JobState::Receiving.as_str()],
                    JobState::Received.as_str(),
                    None,
                    None,
                    now,
                )
                .await?;
        } else {
            store
                .transition_import_job(
                    &job.id,
                    &[JobState::Created.as_str()],
                    JobState::Receiving.as_str(),
                    None,
                    None,
                    now,
                )
                .await?;
        }
        Ok(progress(new_len))
    }

    /// `POST /api/import/{job}/complete`: applies a fully uploaded job (idempotent: a completed
    /// job returns its stored result; re-applying skips known sessions).
    ///
    /// # Errors
    /// `Conflict` while files are incomplete or another completion is applying.
    pub async fn complete_import(
        self: &Arc<Self>,
        ctx: &AuthContext,
        job_id: &str,
    ) -> AppResult<ImportResult> {
        let job = self.owned_job(ctx, job_id).await?;
        if let Some(applied) = applied_of(&job) {
            return Ok(applied.result);
        }
        if JobState::parse(&job.state) == Some(JobState::Superseded) {
            return Err(AppError::Conflict(
                "this upload was superseded by a newer playground upload of yours; run the \
                 command again"
                    .into(),
            ));
        }
        let manifest = parse_manifest(&job)?;
        let incomplete = manifest
            .files()
            .filter(|file| job.received.get(&file.name).copied().unwrap_or(0) < file.size)
            .count();
        if incomplete > 0 {
            return Err(AppError::Conflict(format!(
                "{incomplete} file(s) are incomplete; upload them before completing"
            )));
        }
        let store = &self.deps.store;
        let now = self.now();
        let claimed = match JobState::parse(&job.state) {
            Some(JobState::Applying) if now.millis_since(job.updated_at) > STALE_APPLY_MS => {
                store
                    .transition_import_job(
                        &job.id,
                        &[JobState::Applying.as_str()],
                        JobState::Applying.as_str(),
                        Some(job.updated_at),
                        None,
                        now,
                    )
                    .await?
            }
            Some(JobState::Applying) => false,
            _ => {
                store
                    .transition_import_job(
                        &job.id,
                        &[
                            JobState::Created.as_str(),
                            JobState::Receiving.as_str(),
                            JobState::Received.as_str(),
                            JobState::Failed.as_str(),
                        ],
                        JobState::Applying.as_str(),
                        None,
                        None,
                        now,
                    )
                    .await?
            }
        };
        if !claimed {
            return Err(AppError::Conflict(
                "this import is already being applied".into(),
            ));
        }
        match self.apply_import(ctx, &job, &manifest).await {
            Ok(applied) => {
                let json = serde_json::to_string(&applied).map_err(AppError::internal)?;
                store
                    .transition_import_job(
                        &job.id,
                        &[JobState::Applying.as_str()],
                        JobState::Completed.as_str(),
                        None,
                        Some(&json),
                        self.now(),
                    )
                    .await?;
                store
                    .audit(
                        self.now(),
                        &self.login_of(&ctx.user.id).await,
                        "import.completed",
                        Some(&job.id),
                        Some(&format!(
                            "tasks={} skipped={} snapshot={}",
                            applied.result.tasks_created,
                            applied.result.sessions_skipped_existing,
                            applied
                                .snapshot
                                .as_ref()
                                .map_or("none", |s| s.branch.as_str())
                        )),
                    )
                    .await?;
                Ok(applied.result)
            }
            Err(err) => {
                let _ = store
                    .transition_import_job(
                        &job.id,
                        &[JobState::Applying.as_str()],
                        JobState::Failed.as_str(),
                        None,
                        None,
                        self.now(),
                    )
                    .await;
                Err(err)
            }
        }
    }

    async fn apply_import(
        self: &Arc<Self>,
        ctx: &AuthContext,
        job: &ImportJobRecord,
        manifest: &ImportManifest,
    ) -> AppResult<AppliedImport> {
        let sid = job
            .sandbox_id
            .clone()
            .ok_or_else(|| AppError::Internal("import job without sandbox".into()))?;
        let store = &self.deps.store;
        let job_dir = self.import_dir(&sid, &job.id);
        // A new playground from `yulius onboard --playground` is made now, from the upload
        // (SPEC-171 PGL-003): its branches are its own, its uncommitted work already on its branch.
        let (sandbox, made) = match store.sandbox(&sid).await? {
            None if manifest.playground => {
                let (record, snapshot) = self
                    .playground_from_cli(&ctx.user, &sid, &job_dir, manifest)
                    .await?;
                (record, Some(snapshot))
            }
            found => (
                found.filter(|s| s.removed_at.is_none()).ok_or_else(|| {
                    AppError::Conflict("the sandbox of this import was removed".into())
                })?,
                None,
            ),
        };
        self.require(&ctx.user, &sid, Action::Work).await?;
        let login = import_login(&ctx.user);
        let prefix = format!("import/{login}");
        let repo_dir = self.sandbox_repo(&sid);
        let (bundle_refs, snapshot) = match made {
            Some(snapshot) => (Vec::new(), snapshot),
            None => {
                if sandbox.is_playground() {
                    // Its `origin` is on this server; GitHub is never called (PLY-002).
                    self.dlv_refresh_clone(&sandbox).await?;
                } else {
                    let token = self.installation_token().await?;
                    self.deps
                        .git
                        .sync_clone(
                            &clone_url(&sandbox.repo_owner, &sandbox.repo_name),
                            &repo_dir,
                            &token,
                        )
                        .await?;
                }
                let bundle_refs = match &manifest.bundle {
                    Some(bundle) => match self.applied_bundle(&sandbox, &ctx.user, bundle).await? {
                        // The same person's same bundle is already here (SPEC-171: a re-run).
                        Some(refs) => refs,
                        None => {
                            self.deps
                                .git
                                .import_bundle(&repo_dir, &job_dir.join(&bundle.name), &prefix)
                                .await?
                        }
                    },
                    None => Vec::new(),
                };
                let snapshot = match &manifest.snapshot {
                    Some(_) => Some(
                        self.apply_snapshot(&sandbox, job, manifest, &login, &ctx.user)
                            .await?,
                    ),
                    None => None,
                };
                (bundle_refs, snapshot)
            }
        };
        let (mut imported, mut updated, mut unchanged) = (0_u32, 0_u32, 0_u32);
        for session in &manifest.sessions {
            let base = self
                .session_base(
                    &repo_dir,
                    session,
                    manifest,
                    &bundle_refs,
                    snapshot.as_ref(),
                    &prefix,
                )
                .await;
            match self
                .store_imported_session(&sandbox, session, &job_dir, &ctx.user, base)
                .await?
            {
                StoreOutcome::New => imported += 1,
                StoreOutcome::Updated => updated += 1,
                StoreOutcome::Unchanged | StoreOutcome::Taken => unchanged += 1,
            }
        }
        // The upload never starts onboarding: a person starts it on the sandbox page in the
        // browser (SPEC-051 OBD-001, ADR-0038).
        let onboarding = self.refresh_onboarding(&sid).await?;
        let record = store.sandbox(&sid).await?.ok_or(AppError::NotFound)?;
        let status = self.onboarding_status(&record).await?;
        let base_url = format!("{}/s/{}", self.config.public_url, sandbox.slug);
        Ok(AppliedImport {
            version: APPLIED_VERSION,
            result: ImportResult {
                sandbox_id: sid.to_string(),
                sessions_url: Some(format!("{base_url}/sessions")),
                sandbox_url: base_url,
                tasks_created: 0,
                sessions_skipped_existing: unchanged,
                onboarding,
                sessions_imported: imported,
                sessions_updated: updated,
                sessions_unchanged: unchanged,
                onboarding_status: Some(status),
            },
            bundle_refs,
            snapshot,
            tasks: Vec::new(),
        })
    }

    /// In a playground, the branches an earlier completed job of `user` made from the same
    /// bundle, while they all still exist: importing it again would change nothing (a playground
    /// made from it holds its branches as its own, and none are listed). A GitHub sandbox imports
    /// every bundle, as before (SPEC-171).
    async fn applied_bundle(
        &self,
        sandbox: &SandboxRecord,
        user: &UserRecord,
        bundle: &FileRef,
    ) -> AppResult<Option<Vec<String>>> {
        if !sandbox.is_playground() {
            return Ok(None);
        }
        let repo = self.sandbox_repo(&sandbox.id);
        for job in self
            .deps
            .store
            .import_jobs(&sandbox.id, JOB_LIST_LIMIT)
            .await?
        {
            if job.user_id != user.id {
                continue;
            }
            let Some(applied) = applied_of(&job) else {
                continue;
            };
            let same = parse_manifest(&job).is_ok_and(|earlier| {
                earlier
                    .bundle
                    .is_some_and(|b| b.sha256 == bundle.sha256 && b.size == bundle.size)
            });
            if same {
                for branch in &applied.bundle_refs {
                    let short = branch.strip_prefix("refs/heads/").unwrap_or(branch);
                    if !self.deps.git.branch_exists(&repo, short).await? {
                        return Ok(None);
                    }
                }
                return Ok(Some(applied.bundle_refs));
            }
        }
        Ok(None)
    }

    /// Where an imported session's worktree should start: the snapshot when the session ran on
    /// the snapshot's branch, the session branch imported from the bundle, or the session
    /// branch on `origin`.
    async fn session_base(
        &self,
        repo_dir: &Path,
        session: &SessionRef,
        manifest: &ImportManifest,
        bundle_refs: &[String],
        snapshot: Option<&AppliedSnapshot>,
        prefix: &str,
    ) -> Option<String> {
        let branch = session.git_branch.as_deref()?;
        if let Some(snapshot) = snapshot
            && manifest.repo.head_branch.as_deref() == Some(branch)
        {
            return Some(snapshot.branch.clone());
        }
        let imported = format!("{prefix}/{branch}");
        let full = format!("refs/heads/{imported}");
        if bundle_refs
            .iter()
            .any(|name| name == &imported || name == &full)
        {
            return Some(imported);
        }
        if self.deps.git.valid_branch(branch)
            && self
                .deps
                .git
                .branch_exists(repo_dir, branch)
                .await
                .unwrap_or(false)
        {
            return Some(format!("origin/{branch}"));
        }
        None
    }

    /// Extracts the snapshot archive into a worktree on a new
    /// `import/<login>/snapshot-<date>[-n]` branch created from the client's `HEAD`, commits it
    /// and removes the (clean) worktree. A snapshot applied earlier is not applied again.
    async fn apply_snapshot(
        &self,
        sandbox: &SandboxRecord,
        job: &ImportJobRecord,
        manifest: &ImportManifest,
        login: &str,
        user: &UserRecord,
    ) -> AppResult<AppliedSnapshot> {
        let file: &FileRef = manifest
            .snapshot
            .as_ref()
            .ok_or_else(|| AppError::Internal("no snapshot in the manifest".into()))?;
        let previous = self
            .deps
            .store
            .import_jobs(&sandbox.id, JOB_LIST_LIMIT)
            .await?;
        if let Some(done) = previous
            .iter()
            .filter_map(applied_of)
            .filter_map(|applied| applied.snapshot)
            .find(|s| s.sha256 == file.sha256 && s.head_sha == manifest.repo.head_sha)
        {
            return Ok(done);
        }
        let git = &self.deps.git;
        let repo = self.sandbox_repo(&sandbox.id);
        let date = CivilDate::from_timestamp(self.now(), 0)
            .map_or_else(|| "undated".to_owned(), |d| d.to_string());
        let base_name = format!("import/{login}/snapshot-{date}");
        let mut branch = base_name.clone();
        let mut n = 1;
        while git.branch_exists(&repo, &branch).await? {
            n += 1;
            if n > MAX_SNAPSHOT_BRANCHES {
                return Err(AppError::Conflict(
                    "too many snapshot branches today; delete old import branches first".into(),
                ));
            }
            branch = format!("{base_name}-{n}");
        }
        if !git.valid_branch(&branch) {
            return Err(AppError::Invalid(format!("invalid branch name {branch}")));
        }
        let worktrees = self.sandbox_dir(&sandbox.id).join("worktrees");
        let mut path = worktrees.join(format!("import-{}", job.id));
        let mut attempt = 1;
        while path.exists() {
            attempt += 1;
            if attempt > MAX_SNAPSHOT_BRANCHES {
                return Err(AppError::Conflict(
                    "snapshot worktree paths exhausted".into(),
                ));
            }
            path = worktrees.join(format!("import-{}-{attempt}", job.id));
        }
        let parent = worktrees.clone();
        blocking(move || fs::create_dir_all(&parent).map_err(AppError::internal)).await?;
        git.add_worktree(&repo, &path, &branch, &manifest.repo.head_sha)
            .await?;
        let archive = self.import_dir(&sandbox.id, &job.id).join(&file.name);
        let root = path.clone();
        let stats = blocking(move || extract_snapshot(&archive, &root)).await?;
        let (name, email) = identity(user);
        let committed = git
            .commit_all(
                &path,
                &format!(
                    "Import uncommitted changes of {login} ({date})\n\nImported by yulius import from a local checkout at {}.",
                    manifest.repo.head_sha
                ),
                &name,
                &email,
            )
            .await?;
        match git.remove_worktree(&repo, &path, &worktrees).await {
            Ok(true) => {}
            Ok(false) => tracing::warn!(path = %path.display(), "snapshot worktree kept"),
            Err(err) => tracing::warn!(error = %err, "snapshot worktree removal failed"),
        }
        Ok(AppliedSnapshot {
            sha256: file.sha256.clone(),
            head_sha: manifest.repo.head_sha.clone(),
            branch,
            committed,
            files: stats.files,
            links: stats.links,
            deleted: stats.deleted,
            skipped: stats.skipped,
        })
    }

    async fn imported_task_base(
        &self,
        sandbox: &SandboxId,
        task: &TaskId,
    ) -> AppResult<Option<String>> {
        for job in self.deps.store.import_jobs(sandbox, JOB_LIST_LIMIT).await? {
            if let Some(applied) = applied_of(&job)
                && let Some(entry) = applied.tasks.iter().find(|t| t.task_id == task.as_str())
            {
                return Ok(entry.base.clone());
            }
        }
        self.continued_session_base(sandbox, task).await
    }

    /// Creates the worktree of an imported task from its imported base (snapshot or session
    /// branch) when it does not exist yet; otherwise leaves worktree creation to the supervisor.
    async fn ensure_imported_worktree(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        worktree: &Path,
    ) -> AppResult<()> {
        if worktree.join(".git").exists() {
            return Ok(());
        }
        let Some(base) = self.imported_task_base(&sandbox.id, &task.id).await? else {
            return Ok(());
        };
        let git = &self.deps.git;
        let repo = self.sandbox_repo(&sandbox.id);
        if !repo.join(".git").exists() {
            return Ok(());
        }
        let branch = task
            .branch
            .clone()
            .unwrap_or_else(|| branch_for(task.number, &task.title));
        if !git.valid_branch(&branch) || git.branch_exists(&repo, &branch).await? {
            return Ok(());
        }
        if let Some(parent) = worktree.parent() {
            let parent = parent.to_path_buf();
            blocking(move || fs::create_dir_all(&parent).map_err(AppError::internal)).await?;
        }
        git.add_worktree(&repo, worktree, &branch, &base).await?;
        let profile = self.profile_of(sandbox);
        git.install_push_guard(
            worktree,
            &[
                profile.integration_branch.clone(),
                profile.production_branch.clone(),
            ],
        )
        .await?;
        let owner = self.deps.store.user(&task.owner).await?;
        let (name, email) = crate::projects::commit_identity(owner.as_ref());
        git.set_identity(worktree, &name, &email).await?;
        self.deps
            .store
            .update_task(
                &task.id,
                &TaskFieldUpdate {
                    branch: Some(branch),
                    worktree_path: Some(worktree.to_string_lossy().into_owned()),
                    ..TaskFieldUpdate::default()
                },
                self.now(),
            )
            .await?;
        Ok(())
    }

    /// Prepares the native resume of an imported session (IMP-004): when the task's latest
    /// lineage entry is `imported`, creates the task worktree from the imported base if it is
    /// missing, copies the stored transcript to where the harness expects it for that worktree
    /// (`HarnessPort::claude_transcript` under the sandbox home, every record's `cwd`
    /// rewritten) unless a native copy already exists, and
    /// returns the session id to resume. Returns `None` for tasks whose latest session is not
    /// imported. Call it before `ensure_worktree`/launch of a `Resume`.
    ///
    /// # Errors
    /// `NotFound` for unknown tasks, Git or I/O failures.
    pub async fn prepare_imported_resume(&self, task_id: &TaskId) -> AppResult<Option<String>> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let Some(entry) = store.provider_sessions(task_id).await?.pop() else {
            return Ok(None);
        };
        if entry.origin != "imported" {
            return Ok(None);
        }
        let Some(stored) = entry.transcript_ref.clone().map(PathBuf::from) else {
            return Ok(None);
        };
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let worktree = task.worktree_path.as_ref().map_or_else(
            || {
                self.sandbox_dir(&sandbox.id)
                    .join("worktrees")
                    .join(task.id.as_str())
            },
            PathBuf::from,
        );
        self.ensure_imported_worktree(&sandbox, &task, &worktree)
            .await?;
        let cwd = worktree.to_string_lossy().into_owned();
        match entry.harness {
            Harness::ClaudeCode => {
                let sandbox_dir = self.sandbox_dir(&sandbox.id);
                let dest = self.deps.harness.claude_transcript(
                    &sandbox_dir.join("home"),
                    &worktree,
                    &entry.session_id,
                );
                let random = self.deps.crypto.random_bytes();
                let suffix = hex_token(random.get(..8).unwrap_or(&random));
                blocking(move || {
                    place_claude_transcript(&sandbox_dir, &dest, &stored, &cwd, &suffix)
                })
                .await?;
            }
        }
        Ok(Some(entry.session_id))
    }
}

/// Relative path of an auxiliary session file below the session's own directory.
pub(crate) fn aux_relative(name: &str, session: &SessionRef) -> String {
    let prefix = format!("sessions/{}/{}/", session.harness, session.session_id);
    name.strip_prefix(&prefix).unwrap_or(name).to_owned()
}

/// A manifest file of a new job and the stored prefix it can reuse (path, length).
struct StagedFile {
    name: String,
    size: u64,
    reuse: Option<(PathBuf, u64)>,
}

/// Creates the job directory and pre-fills files from earlier jobs; returns received lengths.
fn stage_job_files(job_dir: &Path, plan: Vec<StagedFile>) -> AppResult<BTreeMap<String, u64>> {
    fs::create_dir_all(job_dir).map_err(AppError::internal)?;
    let mut received = BTreeMap::new();
    for StagedFile {
        name,
        size,
        reuse: source,
    } in plan
    {
        let target = job_dir.join(&name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(AppError::internal)?;
        }
        let mut got = 0;
        if size == 0 {
            File::create(&target).map_err(AppError::internal)?;
        } else if let Some((src, available)) = source {
            match copy_prefix(&src, &target, available, available == size) {
                Ok(()) => got = available,
                Err(err) => tracing::warn!(error = %err, "could not reuse stored import file"),
            }
        }
        received.insert(name, got);
    }
    Ok(received)
}

/// Copies the first `len` bytes of `src` to `dest` (hard link for complete files).
fn copy_prefix(src: &Path, dest: &Path, len: u64, complete: bool) -> io::Result<()> {
    if fs::metadata(src)?.len() < len {
        return Err(io::Error::other("stored file shorter than recorded"));
    }
    if dest.exists() {
        fs::remove_file(dest)?;
    }
    if complete && fs::hard_link(src, dest).is_ok() {
        return Ok(());
    }
    let mut input = File::open(src)?.take(len);
    let mut output = File::create(dest)?;
    let copied = io::copy(&mut input, &mut output)?;
    output.sync_data()?;
    if copied == len {
        Ok(())
    } else {
        Err(io::Error::other("short copy"))
    }
}

/// Truncates `path` to `offset` and appends `bytes` there (creating the file).
fn write_at(path: &Path, offset: u64, bytes: &[u8]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AppError::internal)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(AppError::internal)?;
    if file.metadata().map_err(AppError::internal)?.len() < offset {
        return Err(AppError::Internal(
            "stored import file is shorter than its recorded progress".into(),
        ));
    }
    file.set_len(offset).map_err(AppError::internal)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(AppError::internal)?;
    file.write_all(bytes).map_err(AppError::internal)?;
    file.sync_data().map_err(AppError::internal)
}

fn truncate_file(path: &Path, len: u64) -> AppResult<()> {
    match OpenOptions::new().write(true).open(path) {
        Ok(file) => file.set_len(len).map_err(AppError::internal),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::internal(err)),
    }
}

fn sha256_file(path: &Path) -> AppResult<String> {
    let mut file = File::open(path).map_err(AppError::internal)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(AppError::internal)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex_token(hasher.finalize().as_slice()))
}

/// Writes a read-only copy of `src` at `dest` (temporary file, fsync, rename).
pub(crate) fn copy_immutable(src: &Path, dest: &Path) -> AppResult<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| AppError::Internal("transcript path without parent".into()))?;
    fs::create_dir_all(parent).map_err(AppError::internal)?;
    let mut tmp_name = dest.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    {
        let mut input = File::open(src).map_err(AppError::internal)?;
        let mut output = File::create(&tmp).map_err(AppError::internal)?;
        io::copy(&mut input, &mut output).map_err(AppError::internal)?;
        output.sync_all().map_err(AppError::internal)?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o444)).map_err(AppError::internal)?;
    fs::rename(&tmp, dest).map_err(AppError::internal)?;
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(AppError::internal)
}

/// Moves a staged directory into place (rename; merges into an existing directory).
pub(crate) fn move_tree(from: &Path, to: &Path) -> AppResult<()> {
    if !to.exists() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(AppError::internal)?;
        }
        return fs::rename(from, to).map_err(AppError::internal);
    }
    for entry in fs::read_dir(from).map_err(AppError::internal)? {
        let entry = entry.map_err(AppError::internal)?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(AppError::internal)?.is_dir() {
            move_tree(&entry.path(), &target)?;
        } else {
            fs::rename(entry.path(), target).map_err(AppError::internal)?;
        }
    }
    fs::remove_dir_all(from).map_err(AppError::internal)
}

/// Counters of one snapshot extraction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotStats {
    /// Regular files written.
    pub files: u64,
    /// Links written.
    pub links: u64,
    /// Paths deleted.
    pub deleted: u64,
    /// Entries refused.
    pub skipped: u64,
}

/// A repository-relative path made only of normal components, none of them `.git`.
pub(crate) fn safe_relative(raw: &[u8]) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains(&0) {
        return None;
    }
    let path = Path::new(OsStr::from_bytes(raw));
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) if !part.as_bytes().eq_ignore_ascii_case(b".git") => {
                out.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Whether a link at `rel` with target `target` stays inside the root (relative, never above
/// the root, never into `.git`).
fn link_stays_inside(rel: &Path, target: &[u8]) -> bool {
    if target.is_empty() || target.contains(&0) {
        return false;
    }
    let mut depth = rel.parent().map_or(0, |parent| parent.components().count());
    for component in Path::new(OsStr::from_bytes(target)).components() {
        match component {
            Component::Normal(part) => {
                if part.as_bytes().eq_ignore_ascii_case(b".git") {
                    return false;
                }
                depth += 1;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Makes sure every parent of `rel` below `root` is a real directory (creating missing ones)
/// and clears a file or link at the target; `None` when a link, file or directory is in the way.
pub(crate) fn prepare_target(root: &Path, rel: &Path) -> AppResult<Option<PathBuf>> {
    let parts: Vec<&OsStr> = rel.iter().collect();
    let Some((last, parents)) = parts.split_last() else {
        return Ok(None);
    };
    let mut current = root.to_path_buf();
    for part in parents {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(AppError::internal)?;
            }
            Err(err) => return Err(AppError::internal(err)),
        }
    }
    current.push(last);
    match fs::symlink_metadata(&current) {
        Ok(meta) if meta.is_dir() => return Ok(None),
        Ok(_) => fs::remove_file(&current).map_err(AppError::internal)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(AppError::internal(err)),
    }
    Ok(Some(current))
}

/// Parents of `rel` are real directories inside `root` (for deletions; nothing is created).
fn parents_are_directories(root: &Path, rel: &Path) -> bool {
    let mut current = root.to_path_buf();
    let parts: Vec<&OsStr> = rel.iter().collect();
    let Some((_, parents)) = parts.split_last() else {
        return false;
    };
    for part in parents {
        current.push(part);
        if !fs::symlink_metadata(&current).is_ok_and(|meta| meta.is_dir()) {
            return false;
        }
    }
    true
}

/// `yulius onboard` imports GitHub repositories: an owner that is none (a playground's empty one,
/// SPEC-099) is refused before any lookup reaches a playground or GitHub.
fn refuse_non_github(owner: &str) -> AppResult<()> {
    if crate::projects::valid_owner(owner) {
        Ok(())
    } else {
        Err(AppError::Invalid(format!(
            "{owner:?} is not a GitHub owner; `yulius onboard` imports GitHub repositories (a \
             playground is made in the browser)"
        )))
    }
}

/// Whether a PAX extension record (`<len> <key>=<value>\n`, repeated) sets `size`.
fn pax_sets_size(record: &[u8]) -> bool {
    let mut rest = record;
    while !rest.is_empty() {
        let Some(space) = rest.iter().position(|b| *b == b' ') else {
            return false;
        };
        let Some(len) = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|digits| digits.parse::<usize>().ok())
            .filter(|len| *len > space && *len <= rest.len())
        else {
            return false;
        };
        let body = &rest[space + 1..len];
        if body.split(|b| *b == b'=').next() == Some(b"size".as_slice()) {
            return true;
        }
        rest = &rest[len..];
    }
    false
}

/// Longest metadata entry (a GNU long name or link, a PAX record) an uploaded archive may hold:
/// the tar reader reads one whole, in memory, before the entry it describes.
const MAX_ARCHIVE_METADATA_BYTES: u64 = 64 * 1024;

/// Checks an uploaded gzip tar before anything of it is used, reading only its headers and its
/// small metadata entries: at most [`MAX_SNAPSHOT_ENTRIES`] entries, metadata entries of at
/// most [`MAX_ARCHIVE_METADATA_BYTES`], no PAX `size` override (an entry is as large as its
/// header says), and declared sizes adding up to at most `max_bytes`, so nothing is decompressed
/// past it.
///
/// # Errors
/// `Invalid` for an archive over a bound or unreadable.
pub(crate) fn check_archive(archive: &Path, max_bytes: u64) -> AppResult<()> {
    let file = File::open(archive).map_err(AppError::internal)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    let invalid = |err: io::Error| AppError::Invalid(format!("unreadable archive: {err}"));
    let (mut entries, mut declared) = (0_u64, 0_u64);
    for entry in tar.entries().map_err(invalid)?.raw(true) {
        let mut entry = entry.map_err(invalid)?;
        entries += 1;
        if entries > MAX_SNAPSHOT_ENTRIES.saturating_mul(2) {
            return Err(AppError::Invalid("the archive has too many entries".into()));
        }
        let size = entry.header().size().map_err(invalid)?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname()
            || kind.is_gnu_longlink()
            || kind.is_pax_local_extensions()
            || kind.is_pax_global_extensions()
        {
            if size > MAX_ARCHIVE_METADATA_BYTES {
                return Err(AppError::Invalid(
                    "the archive has an oversized metadata entry (a long name or a PAX record)"
                        .into(),
                ));
            }
            let mut record = Vec::new();
            entry.read_to_end(&mut record).map_err(invalid)?;
            let pax = kind.is_pax_local_extensions() || kind.is_pax_global_extensions();
            if pax && pax_sets_size(&record) {
                return Err(AppError::Invalid(
                    "the archive has a PAX size override in a metadata entry".into(),
                ));
            }
        }
        declared = declared.saturating_add(size);
        if declared > max_bytes {
            return Err(AppError::Invalid(
                "the archive expands beyond the import size limit".into(),
            ));
        }
    }
    Ok(())
}

/// Applies a snapshot archive (contracts layout: `files/…` entries and a `DELETED` list) to
/// `root`, refusing paths that escape it, `.git` paths, writes through links and links
/// pointing outside the root.
///
/// # Errors
/// Unreadable archives, archives over the import bounds, I/O failures.
pub fn extract_snapshot(archive: &Path, root: &Path) -> AppResult<SnapshotStats> {
    check_archive(archive, MAX_IMPORT_TOTAL_BYTES.saturating_mul(2))?;
    let root = fs::canonicalize(root).map_err(AppError::internal)?;
    let file = File::open(archive).map_err(AppError::internal)?;
    let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    let mut stats = SnapshotStats::default();
    let mut deleted: Option<Vec<u8>> = None;
    let mut entries_seen = 0_u64;
    let mut bytes_written = 0_u64;
    let invalid = |err: io::Error| AppError::Invalid(format!("unreadable snapshot archive: {err}"));
    for entry in tar.entries().map_err(invalid)? {
        let mut entry = entry.map_err(invalid)?;
        entries_seen += 1;
        if entries_seen > MAX_SNAPSHOT_ENTRIES {
            return Err(AppError::Invalid(
                "the snapshot archive has too many entries".into(),
            ));
        }
        let raw_path = entry.path_bytes().into_owned();
        if raw_path == SNAPSHOT_DELETED_ENTRY.as_bytes() {
            let mut list = Vec::new();
            (&mut entry)
                .take(MAX_DELETED_LIST_BYTES + 1)
                .read_to_end(&mut list)
                .map_err(invalid)?;
            if u64::try_from(list.len()).unwrap_or(u64::MAX) > MAX_DELETED_LIST_BYTES {
                return Err(AppError::Invalid(
                    "the snapshot's deleted-path list is too large".into(),
                ));
            }
            deleted = Some(list);
            continue;
        }
        let Some(rel) = raw_path
            .strip_prefix(SNAPSHOT_FILES_PREFIX.as_bytes())
            .and_then(safe_relative)
        else {
            stats.skipped += 1;
            continue;
        };
        let kind = entry.header().entry_type();
        if kind.is_file() {
            let size = entry.header().size().map_err(invalid)?;
            bytes_written = bytes_written.saturating_add(size);
            if bytes_written > MAX_IMPORT_TOTAL_BYTES {
                return Err(AppError::Invalid(
                    "the snapshot expands beyond the import size limit".into(),
                ));
            }
            let Some(target) = prepare_target(&root, &rel)? else {
                stats.skipped += 1;
                continue;
            };
            let mode = entry.header().mode().unwrap_or(0o644);
            let mut output = File::create(&target).map_err(AppError::internal)?;
            io::copy(&mut (&mut entry).take(size), &mut output).map_err(AppError::internal)?;
            let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
            fs::set_permissions(&target, fs::Permissions::from_mode(mode))
                .map_err(AppError::internal)?;
            stats.files += 1;
        } else if kind.is_symlink() {
            let Some(link) = entry.link_name_bytes().map(|link| link.into_owned()) else {
                stats.skipped += 1;
                continue;
            };
            if !link_stays_inside(&rel, &link) {
                stats.skipped += 1;
                continue;
            }
            let Some(target) = prepare_target(&root, &rel)? else {
                stats.skipped += 1;
                continue;
            };
            std::os::unix::fs::symlink(OsStr::from_bytes(&link), &target)
                .map_err(AppError::internal)?;
            stats.links += 1;
        } else {
            stats.skipped += 1;
        }
    }
    if let Some(list) = deleted {
        for raw in list.split(|b| *b == 0).filter(|raw| !raw.is_empty()) {
            let Some(rel) = safe_relative(raw) else {
                stats.skipped += 1;
                continue;
            };
            if !parents_are_directories(&root, &rel) {
                continue;
            }
            let target = root.join(&rel);
            if fs::symlink_metadata(&target).is_ok_and(|meta| !meta.is_dir()) {
                fs::remove_file(&target).map_err(AppError::internal)?;
                stats.deleted += 1;
            }
        }
    }
    Ok(stats)
}

/// Sets a Claude record's top-level `cwd`.
fn rewrite_claude(record: &mut serde_json::Value, cwd: &str) -> bool {
    match record.as_object_mut() {
        Some(object) if object.get("cwd").is_some_and(serde_json::Value::is_string) => {
            object.insert("cwd".into(), serde_json::Value::String(cwd.to_owned()));
            true
        }
        _ => false,
    }
}

/// Reads one line of at most `max` bytes into `buf` (newline included). Returns `false` at end
/// of input; an over-long line fills `buf` with its first `max + 1` bytes.
fn read_line_bounded(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> io::Result<bool> {
    buf.clear();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.by_ref().take(limit).read_until(b'\n', buf)?;
    Ok(n > 0)
}

/// Copies the imported Claude transcript `stored` (and its auxiliary files) to `dest`, below the
/// sandbox directory, unless an entry is there already (a native continuation is never
/// overwritten). The agent owns its home: every folder is opened from the sandbox directory
/// without following links and every file is written through its folder's handle, so a link it
/// planted never steers the copy (#657, ADR-0174).
fn place_claude_transcript(
    sandbox_dir: &Path,
    dest: &Path,
    stored: &Path,
    cwd: &str,
    suffix: &str,
) -> AppResult<()> {
    let outside = || AppError::Internal("the transcript path is outside the sandbox".into());
    let parts = dest
        .strip_prefix(sandbox_dir)
        .map_err(|_| outside())?
        .components()
        .map(|c| match c {
            Component::Normal(part) => Ok(part),
            _ => Err(outside()),
        })
        .collect::<AppResult<Vec<&OsStr>>>()?;
    let (name, folders) = parts.split_last().ok_or_else(outside)?;
    let dir = crate::files::open_dirs_below(sandbox_dir, folders, true).map_err(link_refused)?;
    match nix::sys::stat::fstatat(&dir, *name, nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => return Ok(()),
        Err(nix::errno::Errno::ENOENT) => {}
        Err(err) => return Err(crate::files::errno_error(err)),
    }
    rewrite_transcript(stored, &dir, name, suffix, |record| {
        rewrite_claude(record, cwd)
    })?;
    let aux = Path::new(name).with_extension("");
    copy_claude_aux(
        &stored.with_extension(""),
        &dir,
        aux.as_os_str(),
        cwd,
        suffix,
    )
}

/// A link or a file on the way to a folder Yulius writes into.
fn link_refused(err: nix::errno::Errno) -> AppError {
    match err {
        nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => AppError::Forbidden(
            "a folder on the way to the transcript is a link or not a folder: Yulius follows no \
             links"
                .into(),
        ),
        other => crate::files::errno_error(other),
    }
}

/// Copies a JSONL transcript to `name` in the folder `dir`, applying `rewrite` to every
/// parseable record; unparseable and over-long lines are copied verbatim. Writes through a
/// temporary file and renames, within the folder's handle ([`crate::files::replace_in`]).
fn rewrite_transcript(
    src: &Path,
    dir: &std::os::fd::OwnedFd,
    name: &OsStr,
    suffix: &str,
    rewrite: impl Fn(&mut serde_json::Value) -> bool,
) -> AppResult<()> {
    let mut reader = BufReader::new(File::open(src).map_err(AppError::internal)?);
    let temporary = format!(".{}.yulius-import-{suffix}", name.to_string_lossy());
    crate::files::replace_in(dir, name, &temporary, |file| {
        let mut writer = BufWriter::new(file);
        copy_rewritten(&mut reader, &mut writer, &rewrite)?;
        writer.flush()
    })
}

/// The lines of `reader` to `writer`, each parseable record through `rewrite`.
fn copy_rewritten(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    rewrite: &impl Fn(&mut serde_json::Value) -> bool,
) -> io::Result<()> {
    let mut line = Vec::new();
    while read_line_bounded(reader, &mut line, MAX_TRANSCRIPT_LINE_BYTES)? {
        let complete = line.ends_with(b"\n");
        if !complete && line.len() > MAX_TRANSCRIPT_LINE_BYTES {
            // Over-long line: copy it verbatim up to and including its newline.
            writer.write_all(&line)?;
            loop {
                line.clear();
                let n = reader.by_ref().take(1 << 20).read_until(b'\n', &mut line)?;
                writer.write_all(&line)?;
                if n == 0 || line.ends_with(b"\n") {
                    break;
                }
            }
            continue;
        }
        let body = line.strip_suffix(b"\n").unwrap_or(&line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        let rewritten = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|mut record| rewrite(&mut record).then(|| serde_json::to_vec(&record)))
            .and_then(Result::ok);
        match rewritten {
            Some(bytes) => {
                writer.write_all(&bytes)?;
                writer.write_all(b"\n")?;
            }
            None => {
                writer.write_all(&line)?;
                if !complete {
                    writer.write_all(b"\n")?;
                }
            }
        }
    }
    Ok(())
}

/// Copies Claude auxiliary files (subagent transcripts) from `src` to the folder `name` in
/// `parent`, with `cwd` rewritten, bounded ([`MAX_AUX_FILES`], [`MAX_AUX_DEPTH`]); every folder
/// is opened through its parent's handle, and only the folders on the way down stay open.
fn copy_claude_aux(
    src: &Path,
    parent: &std::os::fd::OwnedFd,
    name: &OsStr,
    cwd: &str,
    suffix: &str,
) -> AppResult<()> {
    if !fs::symlink_metadata(src).is_ok_and(|meta| meta.is_dir()) {
        return Ok(());
    }
    let top = crate::files::open_dir_at(parent, name, true).map_err(link_refused)?;
    copy_aux_level(src, &top, cwd, suffix, 0, &mut 0).map(|_| ())
}

/// One folder level of [`copy_claude_aux`]; `copied` counts the files copied so far. Returns
/// `false` once [`MAX_AUX_FILES`] is passed, which stops the whole copy.
fn copy_aux_level(
    from: &Path,
    dir: &std::os::fd::OwnedFd,
    cwd: &str,
    suffix: &str,
    depth: usize,
    copied: &mut usize,
) -> AppResult<bool> {
    for entry in fs::read_dir(from).map_err(AppError::internal)? {
        let entry = entry.map_err(AppError::internal)?;
        let kind = entry.file_type().map_err(AppError::internal)?;
        let child = entry.file_name();
        if kind.is_dir() && depth < MAX_AUX_DEPTH {
            let opened = crate::files::open_dir_at(dir, &child, true).map_err(link_refused)?;
            if !copy_aux_level(&entry.path(), &opened, cwd, suffix, depth + 1, copied)? {
                return Ok(false);
            }
        } else if kind.is_file() {
            *copied += 1;
            if *copied > MAX_AUX_FILES {
                return Ok(false);
            }
            rewrite_transcript(&entry.path(), dir, &child, suffix, |record| {
                rewrite_claude(record, cwd)
            })?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn rfc3339_parsing() -> TestResult {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(Timestamp(0)));
        assert_eq!(
            parse_rfc3339("2026-10-01T10:00:00.5Z"),
            Some(Timestamp(1_790_848_800_500))
        );
        assert_eq!(
            parse_rfc3339("2026-10-01T12:00:00+02:00"),
            parse_rfc3339("2026-10-01T10:00:00Z")
        );
        for bad in [
            "",
            "2026-13-01T00:00:00Z",
            "2026-10-01 10:00:00",
            "2026-10-01T10:00:00",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
        Ok(())
    }

    #[test]
    fn paths_and_links_are_confined() {
        assert_eq!(safe_relative(b"a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_relative(b"./a"), Some(PathBuf::from("a")));
        for bad in [
            &b"../x"[..],
            b"/etc/passwd",
            b"a/../../x",
            b".git/config",
            b"a/.GIT/x",
            b"",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
        assert!(link_stays_inside(Path::new("a/b/link"), b"../c"));
        assert!(link_stays_inside(Path::new("a/link"), b"../c"));
        assert!(!link_stays_inside(Path::new("link"), b"../c"));
        assert!(!link_stays_inside(Path::new("a/link"), b"/etc/passwd"));
        assert!(!link_stays_inside(Path::new("a/link"), b"../.git/hooks"));
    }

    enum Entry<'a> {
        File(&'a str, &'a [u8]),
        Link(&'a str, &'a str),
    }

    fn archive(
        entries: &[Entry<'_>],
        deleted: &[&str],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            match entry {
                Entry::File(name, bytes) => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_size(u64::try_from(bytes.len())?);
                    header.set_mode(0o755);
                    builder.append_data(&mut header, name, *bytes)?;
                }
                Entry::Link(name, target) => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    builder.append_link(&mut header, name, target)?;
                }
            }
        }
        let mut list = Vec::new();
        for path in deleted {
            list.extend_from_slice(path.as_bytes());
            list.push(0);
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(u64::try_from(list.len())?);
        header.set_mode(0o644);
        builder.append_data(&mut header, SNAPSHOT_DELETED_ENTRY, list.as_slice())?;
        Ok(builder.into_inner()?.finish()?)
    }

    #[test]
    fn snapshot_extraction_applies_changes_and_refuses_escapes() -> TestResult {
        let work = tempfile::tempdir()?;
        let root = work.path().join("wt");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/old.rs"), "old")?;
        fs::write(root.join("keep.txt"), "keep")?;
        let outside = work.path().join("outside");
        fs::create_dir_all(&outside)?;
        std::os::unix::fs::symlink(&outside, root.join("escape"))?;
        let bytes = archive(
            &[
                Entry::File("files/src/new.rs", b"fn main() {}"),
                Entry::File("files/escape/evil.txt", b"evil"),
                Entry::Link("files/link-ok", "src/new.rs"),
                Entry::Link("files/link-bad", "../../outside"),
                Entry::File("other/ignored", b"x"),
            ],
            &["src/old.rs", "escape/x", "missing.txt"],
        )?;
        let path = work.path().join("snapshot.tar.gz");
        fs::write(&path, bytes)?;
        let stats = extract_snapshot(&path, &root)?;
        assert_eq!(fs::read_to_string(root.join("src/new.rs"))?, "fn main() {}");
        assert_eq!(
            fs::metadata(root.join("src/new.rs"))?.permissions().mode() & 0o777,
            0o755
        );
        assert!(!root.join("src/old.rs").exists(), "deleted path removed");
        assert_eq!(fs::read_to_string(root.join("keep.txt"))?, "keep");
        assert!(
            !outside.join("evil.txt").exists(),
            "never writes through a link"
        );
        assert_eq!(
            fs::read_link(root.join("link-ok"))?,
            PathBuf::from("src/new.rs")
        );
        assert!(fs::symlink_metadata(root.join("link-bad")).is_err());
        assert_eq!(stats.files, 1);
        assert_eq!(stats.links, 1);
        assert_eq!(stats.deleted, 1);
        assert_eq!(stats.skipped, 3);

        fs::write(&path, b"not a gzip archive")?;
        assert!(matches!(
            extract_snapshot(&path, &root),
            Err(AppError::Invalid(_))
        ));
        Ok(())
    }

    #[test]
    fn transcript_rewrite_changes_only_cwd() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("in.jsonl");
        let long = format!(
            "{{\"cwd\":\"/x\",\"pad\":\"{}\"}}",
            "a".repeat(MAX_TRANSCRIPT_LINE_BYTES)
        );
        fs::write(
            &src,
            format!(
                "{{\"type\":\"user\",\"cwd\":\"/home/me/repo\",\"sessionId\":\"s\"}}\nnot json\n{long}\n{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s\",\"cwd\":\"/old\"}}}}"
            ),
        )?;
        let claude = dir.path().join("out/claude.jsonl");
        let out = crate::files::open_dirs_below(dir.path(), &[OsStr::new("out")], true)?;
        rewrite_transcript(&src, &out, OsStr::new("claude.jsonl"), "t", |r| {
            rewrite_claude(r, "/wt")
        })?;
        let text = fs::read_to_string(&claude)?;
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        let first: serde_json::Value = serde_json::from_str(lines[0])?;
        assert_eq!(first["cwd"], "/wt");
        assert_eq!(first["sessionId"], "s");
        assert_eq!(lines[1], "not json");
        assert_eq!(lines[2], long, "over-long lines are copied verbatim");
        let other: serde_json::Value = serde_json::from_str(lines[3])?;
        assert_eq!(
            other["payload"]["cwd"], "/old",
            "records without a top-level cwd are untouched"
        );
        Ok(())
    }

    #[test]
    fn a_linked_project_folder_is_never_written_through() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sandbox = dir.path().join("sandbox");
        let outside = dir.path().join("outside");
        fs::create_dir_all(sandbox.join("home/.claude"))?;
        fs::create_dir_all(&outside)?;
        std::os::unix::fs::symlink(&outside, sandbox.join("home/.claude/projects"))?;
        let stored = dir.path().join("stored.jsonl");
        fs::write(&stored, "{\"cwd\":\"/old\"}\n")?;
        let dest = sandbox.join("home/.claude/projects/-wt/s.jsonl");
        assert!(matches!(
            place_claude_transcript(&sandbox, &dest, &stored, "/wt", "t"),
            Err(AppError::Forbidden(_))
        ));
        assert_eq!(
            fs::read_dir(&outside)?.count(),
            0,
            "nothing written outside"
        );
        Ok(())
    }

    #[test]
    fn auxiliary_trees_are_copied_to_a_bounded_depth() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sandbox = dir.path().join("sandbox");
        fs::create_dir_all(&sandbox)?;
        let stored = dir.path().join("s.jsonl");
        fs::write(&stored, "{}\n")?;
        let aux = dir.path().join("s/subagents");
        fs::create_dir_all(&aux)?;
        fs::write(aux.join("agent-1.jsonl"), "{\"cwd\":\"/old\"}\n")?;
        // Each folder level holds one open handle while it is copied: deeper ones are left out.
        let deep = (0..MAX_AUX_DEPTH + 2).fold(aux.clone(), |path, n| path.join(format!("d{n}")));
        fs::create_dir_all(&deep)?;
        fs::write(deep.join("deep.jsonl"), "{}\n")?;
        let dest = sandbox.join("home/.claude/projects/-wt/s.jsonl");
        place_claude_transcript(&sandbox, &dest, &stored, "/wt", "t")?;
        let copied = sandbox.join("home/.claude/projects/-wt/s/subagents");
        assert!(fs::read_to_string(copied.join("agent-1.jsonl"))?.contains("/wt"));
        let deep_copy = (0..MAX_AUX_DEPTH + 2).fold(copied, |path, n| path.join(format!("d{n}")));
        assert!(!deep_copy.join("deep.jsonl").exists());
        Ok(())
    }

    #[test]
    fn chunk_files_resume_and_verify() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("a/b.bin");
        write_at(&path, 0, b"hello ")?;
        write_at(&path, 6, b"world")?;
        assert_eq!(sha256_file(&path)?, sha256_hex(b"hello world"));
        // A crash may leave bytes beyond the recorded length; the next write truncates them.
        write_at(&path, 5, b"!")?;
        assert_eq!(fs::read(&path)?, b"hello!");
        assert!(write_at(&path, 100, b"x").is_err());
        let copy = dir.path().join("copy.bin");
        copy_prefix(&path, &copy, 3, false)?;
        assert_eq!(fs::read(&copy)?, b"hel");
        assert!(copy_prefix(&path, &copy, 99, false).is_err());
        Ok(())
    }
}
