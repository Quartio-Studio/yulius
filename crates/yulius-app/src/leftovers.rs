//! Leftovers on the server's disk (SPEC-215, ADR-0170): paths under the data directory that no
//! record owns, or whose owner's state says they are disposable. A scan lists them by kind
//! ([`Kind`]); a deletion checks each item's rule again on fresh state right before removing it
//! (a sandbox that is still running, a holder that is alive, a task that started working keeps
//! its files), audits it (`leftovers.delete`, `leftovers.failed`) and treats a missing path as
//! deleted, so an interrupted round resumes on the next one.
//!
//! The automatic round ([`App::start_leftovers_rounds`]) runs at every start, which is when an
//! update lands, and every hour; it deletes only the kinds that need no person. The superadmin
//! sees and deletes them in Settings → Instance, and root or the service user with
//! `yulius leftovers`.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yulius_contracts::api::leftovers::{
    DeleteLeftoversRequest, LeftoverJobView, LeftoverView, LeftoversView,
};
use yulius_domain::disk::human;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::leftovers::{
    AUTO_EVERY_MS, DRAFT_MAX_AGE_MS, ENDED_RUN_AFTER_MS, Kind, STALE_CHECKOUT_AFTER_MS,
    checkout_job_key, item_id, removed_long_enough, smoke_pid,
};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::disk::dir_size;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::playground::purge::delete_tree;

/// First automatic round after a start: holders are adopted and runs recovered by then.
const AUTO_FIRST: Duration = Duration::from_secs(2 * 60);
/// A worktree directory no task owns is listed once it is this old (a task's worktree is made
/// before its record only for a moment).
const ORPHAN_WORKTREE_AFTER_MS: i64 = 60 * 60 * 1000;
/// CI runs read when looking for the jobs a sandbox still runs.
const CI_RUNS_READ: u32 = 200;
/// The host's own Docker data, reported only (ADR-0170).
const HOST_DOCKER: [&str; 2] = ["/var/lib/docker", "/var/lib/containerd"];

/// The file in the data directory that holds the key of `yulius leftovers` (hex, mode 0600,
/// new at every start): root and the service user read it, sandboxed runs cannot.
pub const CLI_KEY_FILE: &str = "leftovers-cli.key";

/// One leftover found by a scan.
#[derive(Debug, Clone)]
pub struct Leftover {
    /// Stable id ([`item_id`] of the path).
    pub id: String,
    /// What it is.
    pub kind: Kind,
    /// The path deleted.
    pub path: PathBuf,
    /// Allocated bytes (with `extra`); `None` when unreadable.
    pub bytes: Option<u64>,
    /// Its sandbox.
    pub sandbox: Option<SandboxId>,
    /// Its sandbox's slug.
    pub slug: Option<String>,
    /// Its task.
    pub task: Option<TaskId>,
    /// Its task's number.
    pub task_number: Option<u32>,
    /// Why it is a leftover.
    pub reason: String,
    /// Further paths deleted with it (everything of a removed sandbox).
    extra: Vec<PathBuf>,
    /// The clone whose worktree records are pruned after it goes.
    repo: Option<PathBuf>,
}

impl Leftover {
    fn new(kind: Kind, path: PathBuf, reason: impl Into<String>) -> Self {
        Self {
            id: item_id(&path.to_string_lossy()),
            kind,
            path,
            bytes: None,
            sandbox: None,
            slug: None,
            task: None,
            task_number: None,
            reason: reason.into(),
            extra: Vec::new(),
            repo: None,
        }
    }

    fn of(mut self, sandbox: &SandboxRecord) -> Self {
        self.sandbox = Some(sandbox.id.clone());
        self.slug = Some(sandbox.slug.clone());
        self
    }

    fn view(&self) -> LeftoverView {
        LeftoverView {
            id: self.id.clone(),
            kind: self.kind.as_str().to_owned(),
            path: self.path.to_string_lossy().into_owned(),
            bytes: self.bytes,
            sandbox: self.slug.clone(),
            task: self.task_number.map(u64::from),
            reason: self.reason.clone(),
            auto: self.kind.auto(),
            deletable: self.kind.deletable(),
        }
    }
}

/// The last scan and the running or last job.
#[derive(Default)]
pub struct LeftoverState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The CLI's key, as written to [`CLI_KEY_FILE`].
    key: Option<String>,
    items: Vec<Leftover>,
    scanned_at: Option<Timestamp>,
    job: Option<LeftoverJobView>,
    next_auto_at: Option<Timestamp>,
}

/// What a deletion did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteReport {
    /// Items deleted.
    pub deleted: usize,
    /// Bytes freed.
    pub freed: u64,
    /// Items kept: "path: why".
    pub kept: Vec<String>,
    /// Failures: "path: error".
    pub failed: Vec<String>,
}

/// Which items a deletion takes.
#[derive(Debug, Clone)]
pub enum Selection {
    /// These ids of the last scan.
    Ids(Vec<String>),
    /// Every item the automatic rounds delete.
    AllSafe,
}

impl From<DeleteLeftoversRequest> for Selection {
    fn from(request: DeleteLeftoversRequest) -> Self {
        if request.all_safe {
            Self::AllSafe
        } else {
            Self::Ids(request.ids)
        }
    }
}

/// A CI claim held while a sandbox's checkouts go, so no run starts in them half deleted.
struct CiClaim<'a> {
    app: &'a App,
    sandbox: SandboxId,
}

impl Drop for CiClaim<'_> {
    fn drop(&mut self) {
        self.app.ci_release(&self.sandbox);
    }
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The newest modification time (ms) of `path` and its direct entries.
fn touched_ms(path: &Path) -> Option<i64> {
    let own = std::fs::symlink_metadata(path).ok()?.mtime() * 1000;
    let newest = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok().map(|m| m.mtime() * 1000))
        .max()
        .unwrap_or(own);
    Some(own.max(newest))
}

fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    found.sort();
    found
}

fn pid_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Allocated bytes of `path` (a file or a tree); `None` when it cannot be read at all.
fn size_of(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_dir() {
        std::fs::read_dir(path).ok()?;
        Some(dir_size(path))
    } else {
        Some(meta.blocks().saturating_mul(512))
    }
}

/// The directory a worktree's Git record (`.git/worktrees/<name>/gitdir`) points at, when that
/// directory is gone.
fn gone_worktree(record: &Path) -> Option<PathBuf> {
    let gitdir = std::fs::read_to_string(record.join("gitdir")).ok()?;
    let dot_git = PathBuf::from(gitdir.trim());
    let worktree = dot_git.parent()?.to_path_buf();
    (!exists(&worktree)).then_some(worktree)
}

impl App {
    fn data_path(&self, parts: &[&str]) -> PathBuf {
        let mut path = self.config.data_dir.clone();
        for part in parts {
            path.push(part);
        }
        path
    }

    /// Whether `path` lies inside the data directory, with no `..` on the way (every deletion
    /// is checked against it).
    fn inside_data(&self, path: &Path) -> bool {
        path.starts_with(&self.config.data_dir)
            && path != self.config.data_dir
            && !path.components().any(|c| matches!(c, Component::ParentDir))
    }

    /// Scans the data directory for leftovers, with their sizes.
    ///
    /// # Errors
    /// Store failures.
    pub async fn leftovers_scan(&self) -> AppResult<Vec<Leftover>> {
        let store = &self.deps.store;
        let live = store.sandboxes().await?;
        let removed = store.removed_sandboxes().await?;
        let mut found = Vec::new();
        for sandbox in &removed {
            self.scan_removed(sandbox, &mut found).await?;
        }
        for sandbox in &live {
            self.scan_live(sandbox, &mut found).await?;
        }
        for sandbox in live.iter().chain(&removed) {
            self.scan_git_records(sandbox, &mut found);
        }
        let known: HashSet<String> = live
            .iter()
            .chain(&removed)
            .map(|s| s.id.to_string())
            .collect();
        self.scan_instance(&known, &mut found).await?;
        // One item per path: a removed sandbox's run directory is also an old one.
        let mut seen = HashSet::new();
        found.retain(|item: &Leftover| seen.insert(item.id.clone()));
        let measured = tokio::task::spawn_blocking(move || {
            for item in &mut found {
                if item.bytes.is_none() {
                    item.bytes = std::iter::once(&item.path)
                        .chain(&item.extra)
                        .filter(|p| exists(p))
                        .map(|p| size_of(p))
                        .try_fold(0u64, |sum, bytes| bytes.map(|b| sum.saturating_add(b)));
                }
            }
            found
        })
        .await
        .map_err(AppError::internal)?;
        Ok(measured)
    }

    /// A removed sandbox: its disposable parts once its rule holds, and everything else of it
    /// as one manual item.
    async fn scan_removed(
        &self,
        sandbox: &SandboxRecord,
        out: &mut Vec<Leftover>,
    ) -> AppResult<()> {
        let id = sandbox.id.as_str();
        let dir = self.sandbox_dir(&sandbox.id);
        let mut parts = vec![
            (Kind::RemovedWorktrees, dir.join("worktrees")),
            (Kind::RemovedCi, dir.join("ci")),
            (Kind::RemovedBrowser, dir.join("browser")),
            (Kind::RemovedCd, self.data_path(&["cd", id])),
            (Kind::RemovedDisplay, self.data_path(&["displays", id])),
            (Kind::RemovedSystem, self.data_path(&["sysroots", id])),
            (
                Kind::RemovedRuntime,
                self.data_path(&["runtime", "sandboxes", id]),
            ),
        ];
        let mut tasks = self.deps.store.tasks(&sandbox.id, false).await?;
        tasks.extend(self.deps.store.tasks(&sandbox.id, true).await?);
        let runs = self.data_path(&["runs"]);
        for task in &tasks {
            for attempt in self.deps.store.attempts(&task.id).await? {
                let run = PathBuf::from(&attempt.run_dir);
                if run.starts_with(&runs) && run != runs {
                    parts.push((Kind::EndedRun, run));
                }
            }
        }
        // Listed once its rule holds (a removed sandbox still stopping is not a leftover yet);
        // the system layer and state wait for the holder too.
        let blocked = self.removed_blocker(sandbox, None).await?.is_some();
        let holder = match self.isolation.get() {
            Some(isolation) => isolation.holder_live(id).await,
            None => false,
        };
        for (kind, path) in &parts {
            let waits_for_holder = matches!(kind, Kind::RemovedSystem | Kind::RemovedRuntime);
            if !exists(path) || blocked || (holder && waits_for_holder) {
                continue;
            }
            let reason = match kind {
                Kind::EndedRun => "an agent run of a removed sandbox".to_owned(),
                _ => format!("the sandbox {} was removed", sandbox.slug),
            };
            let mut item = Leftover::new(*kind, path.clone(), reason).of(sandbox);
            if *kind == Kind::RemovedWorktrees {
                item.repo = Some(self.sandbox_repo(&sandbox.id));
            }
            out.push(item);
        }
        // Everything else PRJ-009 keeps: a person may delete it, files only.
        let mut extra: Vec<PathBuf> = parts.into_iter().map(|(_, path)| path).collect();
        if sandbox.is_playground() {
            extra.push(self.playground_origin(&sandbox.id));
        }
        for task in &tasks {
            extra.push(self.data_path(&["transcripts", task.id.as_str()]));
        }
        let prefix = format!("sandbox-{}-", sandbox.slug);
        for (name, path) in entries(&self.data_path(&["exports"])) {
            if name.starts_with(&prefix) && name.ends_with(".json") {
                extra.push(path);
            }
        }
        extra.retain(|path| exists(path) && !path.starts_with(&dir));
        if exists(&dir) || !extra.is_empty() {
            let mut item = Leftover::new(
                Kind::RemovedSandbox,
                dir,
                format!(
                    "everything of the removed sandbox {}: its clone, sessions, transcripts and \
                     export, kept unless a person deletes them",
                    sandbox.slug
                ),
            )
            .of(sandbox);
            item.extra = extra;
            out.push(item);
        }
        Ok(())
    }

    /// Why a removed sandbox's files (of `kind`, or any of them) must stay now; `None` when its
    /// rule holds: removed long enough, nothing of it runs, no CD run, and for its system layer
    /// and state, no live holder.
    async fn removed_blocker(
        &self,
        sandbox: &SandboxRecord,
        kind: Option<Kind>,
    ) -> AppResult<Option<String>> {
        let Some(record) = self.deps.store.sandbox(&sandbox.id).await? else {
            return Ok(Some("its record is gone".into()));
        };
        let Some(removed) = record.removed_at else {
            return Ok(Some("the sandbox is live again".into()));
        };
        if !removed_long_enough(removed.0, self.now().0) {
            return Ok(Some("it was removed less than 10 minutes ago".into()));
        }
        let running = self.still_running(&sandbox.id).await?;
        if !running.is_empty() {
            return Ok(Some(format!("still running: {}", running.join(", "))));
        }
        if self
            .deps
            .store
            .running_cd_runs()
            .await?
            .iter()
            .any(|run| run.sandbox_id == sandbox.id)
        {
            return Ok(Some("a deploy of it runs".into()));
        }
        let holder = matches!(
            kind,
            Some(Kind::RemovedSystem | Kind::RemovedRuntime | Kind::RemovedSandbox)
        );
        if holder
            && let Some(isolation) = self.isolation.get()
            && isolation.holder_live(sandbox.id.as_str()).await
        {
            return Ok(Some("its holder still runs".into()));
        }
        Ok(None)
    }

    /// A live sandbox: caches the upkeep would remove, worktrees no task owns, checkouts of jobs
    /// no run uses and old draft attachments.
    async fn scan_live(&self, sandbox: &SandboxRecord, out: &mut Vec<Leftover>) -> AppResult<()> {
        let now = self.now().0;
        for cache in self.ended_caches(sandbox).await? {
            let reason = match &cache.task {
                Some(task) => format!(
                    "{} of task #{}: {}",
                    cache.relative,
                    task.number,
                    task_state(task)
                ),
                None => format!("{} of {}: no task owns it", cache.relative, cache.label),
            };
            let mut item = Leftover::new(Kind::TaskCache, cache.path, reason).of(sandbox);
            item.bytes = Some(cache.bytes);
            if let Some(task) = cache.task {
                item.task = Some(task.id);
                item.task_number = Some(task.number);
            }
            out.push(item);
        }
        let mut owned: HashSet<String> = HashSet::new();
        for archived in [false, true] {
            for task in self.deps.store.tasks(&sandbox.id, archived).await? {
                owned.insert(task.id.to_string());
            }
        }
        let dir = self.sandbox_dir(&sandbox.id);
        for (name, path) in entries(&dir.join("worktrees")) {
            let old = touched_ms(&path).is_some_and(|t| now - t >= ORPHAN_WORKTREE_AFTER_MS);
            if owned.contains(&name) || !old {
                continue;
            }
            let mut item = Leftover::new(
                Kind::OrphanWorktree,
                path,
                "a worktree no task owns (it may hold uncommitted work)",
            )
            .of(sandbox);
            item.repo = Some(self.sandbox_repo(&sandbox.id));
            out.push(item);
        }
        self.scan_checkouts(sandbox, out).await?;
        let drafts = dir.join("attachments").join("drafts");
        for (_, user_dir) in entries(&drafts) {
            for (name, file) in entries(&user_dir) {
                let old = std::fs::symlink_metadata(&file)
                    .is_ok_and(|m| now - m.mtime() * 1000 >= DRAFT_MAX_AGE_MS);
                if old {
                    out.push(
                        Leftover::new(
                            Kind::OldDraft,
                            file,
                            format!("a draft attachment ({name}) older than 7 days"),
                        )
                        .of(sandbox),
                    );
                }
            }
        }
        Ok(())
    }

    /// CI checkouts whose job no run of the sandbox used for [`STALE_CHECKOUT_AFTER_MS`], or of a
    /// lane past the configured ones. A sandbox with no recent run keeps its checkouts (they are
    /// idle, not stale).
    async fn scan_checkouts(
        &self,
        sandbox: &SandboxRecord,
        out: &mut Vec<Leftover>,
    ) -> AppResult<()> {
        let jobs_dir = self.ci_dir(&sandbox.id).join("jobs");
        let checkouts = entries(&jobs_dir);
        if checkouts.is_empty() {
            return Ok(());
        }
        let since = self.now().0 - STALE_CHECKOUT_AFTER_MS;
        let mut used: HashSet<String> = HashSet::new();
        let mut recent = 0;
        for run in self.deps.store.ci_runs(&sandbox.id, CI_RUNS_READ).await? {
            if run.queued_at.0 < since {
                continue;
            }
            recent += 1;
            for job in self.deps.store.ci_jobs(&run.id, false).await? {
                used.insert(job.key);
            }
        }
        if recent == 0 {
            return Ok(());
        }
        let lanes = self.config.ci.lanes.max(1);
        for (name, path) in checkouts {
            let (unlaned, lane) = match name.split_once('@') {
                Some((key, lane)) => (key, lane.parse::<u32>().ok()),
                None => (name.as_str(), Some(0)),
            };
            let job_used = used.contains(unlaned) || used.contains(checkout_job_key(&name));
            let lane_used = lane.is_some_and(|lane| lane < lanes);
            if job_used && lane_used {
                continue;
            }
            let reason = if job_used {
                format!("CI checkout {name}: a lane past the {lanes} configured")
            } else {
                format!("CI checkout {name}: no run used its job for 14 days")
            };
            out.push(Leftover::new(Kind::StaleCiCheckout, path, reason).of(sandbox));
        }
        Ok(())
    }

    /// Git's records of worktrees whose directories are gone (a partly cleaned sandbox).
    fn scan_git_records(&self, sandbox: &SandboxRecord, out: &mut Vec<Leftover>) {
        let repo = self.sandbox_repo(&sandbox.id);
        for (name, record) in entries(&repo.join(".git").join("worktrees")) {
            if let Some(worktree) = gone_worktree(&record) {
                let mut item = Leftover::new(
                    Kind::GitMetadata,
                    record,
                    format!(
                        "Git's record of the worktree {name}, whose directory {} is gone",
                        worktree.display()
                    ),
                )
                .of(sandbox);
                item.repo = Some(repo.clone());
                out.push(item);
            }
        }
    }

    /// The instance: old run directories, transcripts of deleted tasks, directories no sandbox
    /// owns (a smoke test's whose process is gone), unused bases, the host's Docker data.
    async fn scan_instance(
        &self,
        known: &HashSet<String>,
        out: &mut Vec<Leftover>,
    ) -> AppResult<()> {
        let now = self.now().0;
        let live: HashSet<String> = self
            .deps
            .store
            .live_attempts()
            .await?
            .into_iter()
            .flat_map(|a| [a.id.to_string(), a.run_dir])
            .collect();
        for (name, path) in entries(&self.data_path(&["runs"])) {
            let old = touched_ms(&path).is_some_and(|t| now - t >= ENDED_RUN_AFTER_MS);
            if !old || live.contains(&name) || live.contains(&path.to_string_lossy().into_owned()) {
                continue;
            }
            out.push(Leftover::new(
                Kind::EndedRun,
                path,
                "a run directory of no live run, untouched for a day",
            ));
        }
        for (name, path) in entries(&self.data_path(&["transcripts"])) {
            let Ok(task) = TaskId::parse(&name) else {
                continue;
            };
            if name.starts_with('.') || self.deps.store.task(&task).await?.is_some() {
                continue;
            }
            out.push(Leftover::new(
                Kind::OrphanTranscripts,
                path,
                "transcripts of a task that no longer exists",
            ));
        }
        for parent in [
            &["sandboxes"][..],
            &["sysroots"],
            &["runtime", "sandboxes"],
            &["cd"],
            &["displays"],
        ] {
            for (name, path) in entries(&self.data_path(parent)) {
                if known.contains(&name) || name.starts_with('.') {
                    continue;
                }
                let item = match smoke_pid(&name) {
                    Some(pid) if !pid_alive(pid) => Leftover::new(
                        Kind::StaleSmoke,
                        path,
                        format!("a Docker smoke test's sandbox; its process {pid} is gone"),
                    ),
                    Some(_) => continue,
                    None => Leftover::new(Kind::Unowned, path, "no sandbox record owns it"),
                };
                out.push(item);
            }
        }
        if let Some(isolation) = self.isolation.get() {
            for base in isolation.unused_bases().await {
                out.push(Leftover::new(
                    Kind::UnusedBase,
                    base,
                    "a base of sandboxes' systems that is not current and no live holder mounts",
                ));
            }
        }
        for host in HOST_DOCKER {
            let path = PathBuf::from(host);
            if exists(&path) {
                out.push(Leftover::new(
                    Kind::HostDocker,
                    path,
                    "the host's own Docker data (docker-ce): Yulius does not use it; remove it \
                     on the host if nothing else does",
                ));
            }
        }
        Ok(())
    }

    /// Checks `item`'s rule again on fresh state right before it is deleted (LFT-007). `Ok`
    /// with a CI claim when its sandbox's checkouts are touched; `Err` with why it stays.
    async fn leftover_guard(&self, item: &Leftover) -> Result<Option<CiClaim<'_>>, String> {
        if !item.kind.deletable() {
            return Err("reported only: the host's own data".into());
        }
        if !self.inside_data(&item.path) || item.extra.iter().any(|p| !self.inside_data(p)) {
            return Err("outside the data directory".into());
        }
        let store = &self.deps.store;
        let sandbox = match &item.sandbox {
            Some(id) => Some(
                store
                    .sandbox(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("its sandbox record is gone")?,
            ),
            None => None,
        };
        let claim = |sandbox: &SandboxRecord| {
            if self.ci_claim(&sandbox.id) {
                Ok(Some(CiClaim {
                    app: self,
                    sandbox: sandbox.id.clone(),
                }))
            } else {
                Err("a CI run of its sandbox runs".to_owned())
            }
        };
        match item.kind {
            Kind::RemovedWorktrees
            | Kind::RemovedCi
            | Kind::RemovedBrowser
            | Kind::RemovedCd
            | Kind::RemovedDisplay
            | Kind::RemovedSystem
            | Kind::RemovedRuntime
            | Kind::RemovedSandbox => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                if let Some(why) = self
                    .removed_blocker(&sandbox, Some(item.kind))
                    .await
                    .map_err(|e| e.to_string())?
                {
                    return Err(why);
                }
                if matches!(item.kind, Kind::RemovedCi | Kind::RemovedSandbox) {
                    return claim(&sandbox);
                }
                Ok(None)
            }
            Kind::EndedRun => {
                let live = store.live_attempts().await.map_err(|e| e.to_string())?;
                let path = item.path.to_string_lossy();
                if live.iter().any(|a| a.run_dir == path) {
                    return Err("its run is live".into());
                }
                if let Some(sandbox) = &sandbox
                    && let Some(why) = self
                        .removed_blocker(sandbox, None)
                        .await
                        .map_err(|e| e.to_string())?
                {
                    return Err(why);
                }
                Ok(None)
            }
            Kind::TaskCache => {
                if let Some(task) = &item.task
                    && !self.cache_still_ended(task).await
                {
                    return Err("its task works again".into());
                }
                Ok(None)
            }
            Kind::StaleCiCheckout => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                claim(&sandbox)
            }
            Kind::OrphanWorktree => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                for archived in [false, true] {
                    let tasks = store
                        .tasks(&sandbox.id, archived)
                        .await
                        .map_err(|e| e.to_string())?;
                    if tasks.iter().any(|t| Some(t.id.to_string()) == name) {
                        return Err("a task owns it now".into());
                    }
                }
                Ok(None)
            }
            Kind::GitMetadata => {
                if gone_worktree(&item.path).is_none() {
                    return Err("its worktree is back".into());
                }
                Ok(None)
            }
            Kind::OrphanTranscripts => {
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                let task = name.and_then(|n| TaskId::parse(&n).ok());
                match task {
                    Some(task)
                        if store
                            .task(&task)
                            .await
                            .map_err(|e| e.to_string())?
                            .is_none() =>
                    {
                        Ok(None)
                    }
                    _ => Err("its task exists".into()),
                }
            }
            Kind::StaleSmoke | Kind::Unowned => {
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Ok(id) = SandboxId::parse(&name)
                    && store
                        .sandbox(&id)
                        .await
                        .map_err(|e| e.to_string())?
                        .is_some()
                {
                    return Err("a sandbox owns it now".into());
                }
                if smoke_pid(&name).is_some_and(pid_alive) {
                    return Err("its smoke test runs".into());
                }
                if let Some(isolation) = self.isolation.get()
                    && isolation.holder_live(&name).await
                {
                    return Err("a holder of it runs".into());
                }
                Ok(None)
            }
            Kind::UnusedBase => match self.isolation.get() {
                Some(isolation) if isolation.unused_bases().await.contains(&item.path) => Ok(None),
                _ => Err("the base is current or mounted".into()),
            },
            Kind::OldDraft => Ok(None),
            Kind::HostDocker => Err("reported only".into()),
        }
    }

    /// Removes one path of an item: a missing one is fine; a tree whose files may belong to a
    /// sandbox's mapped ids goes through the isolation.
    async fn remove_leftover_path(&self, path: &Path) -> Result<(), String> {
        if !exists(path) {
            return Ok(());
        }
        let mapped = ["sysroots", "system"]
            .iter()
            .map(|d| self.data_path(&[d]))
            .chain([self.data_path(&["runtime", "sandboxes"])])
            .any(|root| path.starts_with(root));
        if mapped && let Some(isolation) = self.isolation.get() {
            return isolation.remove_owned_tree(path).await;
        }
        let target = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let meta = std::fs::symlink_metadata(&target)?;
            if meta.is_dir() {
                delete_tree(&target)
            } else {
                match std::fs::remove_file(&target) {
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                }
            }
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }

    /// Deletes one item after its guard: measured, removed, audited.
    async fn delete_leftover(&self, item: &Leftover, actor: &str, report: &mut DeleteReport) {
        let label = item.path.to_string_lossy().into_owned();
        let guard = match self.leftover_guard(item).await {
            Ok(guard) => guard,
            Err(why) => {
                report.kept.push(format!("{label}: {why}"));
                return;
            }
        };
        let paths: Vec<PathBuf> = std::iter::once(item.path.clone())
            .chain(item.extra.iter().cloned())
            .collect();
        let measured = paths.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            measured
                .iter()
                .filter_map(|p| size_of(p))
                .fold(0u64, u64::saturating_add)
        })
        .await
        .unwrap_or(0);
        let mut result = Ok(());
        if item.kind == Kind::GitMetadata {
            if let Some(repo) = &item.repo {
                result = self
                    .deps
                    .git
                    .prune_worktrees(repo)
                    .await
                    .map_err(|e| e.to_string());
            }
            if result.is_ok() && exists(&item.path) {
                result = Err("git worktree prune kept it".into());
            }
        } else {
            for path in &paths {
                if let Err(err) = self.remove_leftover_path(path).await {
                    result = Err(err);
                    break;
                }
            }
            if let Some(repo) = item.repo.as_ref().filter(|r| exists(r))
                && let Err(err) = self.deps.git.prune_worktrees(repo).await
            {
                tracing::warn!(repo = %repo.display(), error = %err, "git worktree prune failed");
            }
        }
        drop(guard);
        let target = item.slug.clone().unwrap_or_else(|| "instance".into());
        match result {
            Ok(()) => {
                report.deleted += 1;
                report.freed = report.freed.saturating_add(bytes);
                self.dlv_audit(
                    actor,
                    "leftovers.delete",
                    &target,
                    &format!(
                        "{label} ({}, {}): {}",
                        item.kind.as_str(),
                        human(bytes),
                        item.reason
                    ),
                )
                .await;
            }
            Err(err) => {
                tracing::warn!(path = %label, error = %err, "a leftover could not be deleted");
                report.failed.push(format!("{label}: {err}"));
                self.dlv_audit(
                    actor,
                    "leftovers.failed",
                    &target,
                    &format!("{label} ({}): {err}", item.kind.as_str()),
                )
                .await;
            }
        }
    }

    /// Deletes `items` one after the other, each after its guard, updating the job's progress.
    pub async fn delete_leftovers(&self, items: &[Leftover], actor: &str) -> DeleteReport {
        let mut report = DeleteReport::default();
        for item in items {
            self.delete_leftover(item, actor, &mut report).await;
            let freed = report.freed;
            let deleted = report.deleted;
            let kept = report.kept.clone();
            let failed = report.failed.clone();
            self.with_leftovers(|inner| {
                if let Some(job) = inner.job.as_mut() {
                    job.done += 1;
                    job.deleted = deleted;
                    job.freed = freed;
                    job.kept = kept;
                    job.failed = failed;
                }
            });
        }
        // The list no longer shows what went.
        let gone: HashSet<String> = items
            .iter()
            .filter(|item| !exists(&item.path))
            .map(|item| item.id.clone())
            .collect();
        self.with_leftovers(|inner| inner.items.retain(|item| !gone.contains(&item.id)));
        report
    }

    fn with_leftovers<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let mut inner = self
            .leftovers
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut inner)
    }

    /// Takes the job slot for `action`; `false` when a scan or a deletion runs.
    fn begin_leftover_job(&self, action: &str, actor: &str, total: usize) -> bool {
        let started_at = rfc3339(self.now());
        self.with_leftovers(|inner| {
            if inner.job.as_ref().is_some_and(|job| job.running) {
                return false;
            }
            inner.job = Some(LeftoverJobView {
                action: action.to_owned(),
                running: true,
                actor: actor.to_owned(),
                total,
                started_at,
                ..LeftoverJobView::default()
            });
            true
        })
    }

    fn end_leftover_job(&self, error: Option<String>) {
        let ended_at = rfc3339(self.now());
        self.with_leftovers(|inner| {
            if let Some(job) = inner.job.as_mut() {
                job.running = false;
                job.ended_at = Some(ended_at);
                if let Some(error) = error {
                    job.failed.push(error);
                }
            }
        });
    }

    async fn scan_into_state(&self) -> AppResult<()> {
        let items = self.leftovers_scan().await?;
        let now = self.now();
        self.with_leftovers(|inner| {
            inner.items = items;
            inner.scanned_at = Some(now);
        });
        Ok(())
    }

    /// One automatic round (LFT-009): scan, delete every item that needs no person. Skipped
    /// while a person's scan or deletion runs.
    pub async fn run_leftovers_round(&self) -> Option<DeleteReport> {
        if !self.begin_leftover_job("scan", "yulius", 0) {
            return None;
        }
        if let Err(err) = self.scan_into_state().await {
            tracing::warn!(error = %err, "the leftovers scan failed");
            self.end_leftover_job(Some(err.to_string()));
            return None;
        }
        let safe: Vec<Leftover> = self.with_leftovers(|inner| {
            inner
                .items
                .iter()
                .filter(|item| item.kind.auto())
                .cloned()
                .collect()
        });
        self.with_leftovers(|inner| {
            if let Some(job) = inner.job.as_mut() {
                job.action = "delete".into();
                job.total = safe.len();
            }
        });
        let report = self.delete_leftovers(&safe, "yulius").await;
        self.end_leftover_job(None);
        if report.deleted > 0 || !report.failed.is_empty() {
            tracing::info!(
                deleted = report.deleted,
                freed = report.freed,
                failed = report.failed.len(),
                "leftovers round"
            );
        }
        Some(report)
    }

    /// Writes a new key for `yulius leftovers` to [`CLI_KEY_FILE`] (0600, replaced atomically)
    /// and keeps it for [`App::leftovers_key_matches`].
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn write_leftovers_key(&self) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let key: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self.config.data_dir.join(CLI_KEY_FILE);
        let staged = self.config.data_dir.join(format!(".{CLI_KEY_FILE}.new"));
        let _ = std::fs::remove_file(&staged);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)?;
        file.write_all(key.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&staged, &path)?;
        self.with_leftovers(|inner| inner.key = Some(key));
        Ok(())
    }

    /// Whether `presented` is the CLI's key (compared in constant time; none before it is
    /// written).
    #[must_use]
    pub fn leftovers_key_matches(&self, presented: &str) -> bool {
        self.with_leftovers(|inner| {
            inner.key.as_ref().is_some_and(|key| {
                key.len() == presented.len()
                    && key
                        .bytes()
                        .zip(presented.bytes())
                        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                        == 0
            })
        })
    }

    /// The automatic rounds: at start (after [`AUTO_FIRST`]) and every hour. The CLI's key is
    /// written first.
    pub fn start_leftovers_rounds(self: &Arc<Self>) {
        let every = Duration::from_millis(u64::try_from(AUTO_EVERY_MS).unwrap_or(u64::MAX));
        self.start_leftovers_rounds_with(AUTO_FIRST, every);
    }

    /// [`App::start_leftovers_rounds`] with its periods (tests shorten them).
    pub fn start_leftovers_rounds_with(self: &Arc<Self>, first: Duration, every: Duration) {
        if let Err(err) = self.write_leftovers_key() {
            tracing::warn!(error = %err, "cannot write the key of yulius leftovers");
        }
        let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        let next = Timestamp(self.now().0.saturating_add(ms(first)));
        self.with_leftovers(|inner| inner.next_auto_at = Some(next));
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(first).await;
            let mut ticks = tokio::time::interval(every);
            loop {
                ticks.tick().await;
                app.run_leftovers_round().await;
                let next = Timestamp(app.now().0.saturating_add(ms(every)));
                app.with_leftovers(|inner| inner.next_auto_at = Some(next));
            }
        });
    }

    /// The view without a person (the CLI's loopback route checks its key first).
    pub async fn leftovers_overview(&self) -> LeftoversView {
        let data_dir = self.config.data_dir.clone();
        // The disk probe reads OS counters: off the executor.
        let probe = Arc::clone(&self.deps.system);
        let dir = data_dir.clone();
        let disk = tokio::task::spawn_blocking(move || probe.snapshot(&dir))
            .await
            .unwrap_or_default();
        self.with_leftovers(|inner| {
            let deletable = inner.items.iter().filter(|i| i.kind.deletable());
            let total_bytes = deletable.clone().filter_map(|i| i.bytes).sum();
            let safe_bytes = deletable
                .filter(|i| i.kind.auto())
                .filter_map(|i| i.bytes)
                .sum();
            LeftoversView {
                items: inner.items.iter().map(Leftover::view).collect(),
                scanned_at: inner.scanned_at.map(rfc3339),
                total_bytes,
                safe_bytes,
                disk_used: disk.disk_used,
                disk_total: disk.disk_total,
                data_dir: data_dir.to_string_lossy().into_owned(),
                job: inner.job.clone(),
                next_auto_at: inner.next_auto_at.map(rfc3339),
            }
        })
    }

    /// `GET /api/admin/leftovers` (superadmin; LFT-010).
    ///
    /// # Errors
    /// `Forbidden` for anyone but the superadmin.
    pub async fn leftovers_view(&self, ctx: &AuthContext) -> AppResult<LeftoversView> {
        self.require_superadmin(ctx)?;
        Ok(self.leftovers_overview().await)
    }

    /// `POST /api/admin/leftovers/scan` (superadmin): rescans in the background.
    ///
    /// # Errors
    /// `Forbidden`; `Conflict` while a scan or deletion runs.
    pub async fn scan_leftovers(self: &Arc<Self>, ctx: &AuthContext) -> AppResult<LeftoversView> {
        self.require_superadmin(ctx)?;
        let actor = self.login_of(&ctx.user.id).await;
        self.start_leftovers_scan(&actor).await
    }

    /// Starts a background scan for `actor`.
    ///
    /// # Errors
    /// `Conflict` while a scan or deletion runs.
    pub async fn start_leftovers_scan(self: &Arc<Self>, actor: &str) -> AppResult<LeftoversView> {
        if !self.begin_leftover_job("scan", actor, 0) {
            return Err(AppError::Conflict(
                "a leftovers scan or deletion runs".into(),
            ));
        }
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let result = app.scan_into_state().await;
            app.end_leftover_job(result.err().map(|e| e.to_string()));
        });
        Ok(self.leftovers_overview().await)
    }

    /// `POST /api/admin/leftovers/delete` (superadmin): deletes the selection in the background.
    ///
    /// # Errors
    /// `Forbidden`; `Conflict` while a scan or deletion runs; `Invalid` for unknown ids or
    /// an empty selection.
    pub async fn delete_leftovers_request(
        self: &Arc<Self>,
        ctx: &AuthContext,
        request: DeleteLeftoversRequest,
    ) -> AppResult<LeftoversView> {
        self.require_superadmin(ctx)?;
        let actor = self.login_of(&ctx.user.id).await;
        self.start_leftovers_delete(&actor, request.into()).await
    }

    /// Starts a background deletion of `selection` from the last scan for `actor`.
    ///
    /// # Errors
    /// `Conflict` while a scan or deletion runs; `Invalid` for unknown ids or nothing to
    /// delete.
    pub async fn start_leftovers_delete(
        self: &Arc<Self>,
        actor: &str,
        selection: Selection,
    ) -> AppResult<LeftoversView> {
        let chosen: Vec<Leftover> = self.with_leftovers(|inner| match &selection {
            Selection::AllSafe => inner
                .items
                .iter()
                .filter(|i| i.kind.auto())
                .cloned()
                .collect(),
            Selection::Ids(ids) => inner
                .items
                .iter()
                .filter(|i| ids.contains(&i.id))
                .cloned()
                .collect(),
        });
        if let Selection::Ids(ids) = &selection {
            let unknown: Vec<&String> = ids
                .iter()
                .filter(|id| !chosen.iter().any(|i| &i.id == *id))
                .collect();
            if !unknown.is_empty() {
                return Err(AppError::Invalid(format!(
                    "not in the last scan (scan again): {}",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        if chosen.is_empty() {
            return Err(AppError::Invalid("nothing to delete".into()));
        }
        if !self.begin_leftover_job("delete", actor, chosen.len()) {
            return Err(AppError::Conflict(
                "a leftovers scan or deletion runs".into(),
            ));
        }
        let app = Arc::clone(self);
        let actor = actor.to_owned();
        tokio::spawn(async move {
            app.delete_leftovers(&chosen, &actor).await;
            app.end_leftover_job(None);
        });
        Ok(self.leftovers_overview().await)
    }
}

/// How a task's state reads in a reason.
fn task_state(task: &crate::model::TaskRecord) -> String {
    use yulius_domain::task::{Activity, TaskStatus};
    match (task.status, task.activity) {
        (TaskStatus::Running, Some(Activity::Done)) => "done, quiet for 30 minutes".into(),
        (TaskStatus::Running, _) => "idle for 4 hours".into(),
        (TaskStatus::Closed, _) => "closed".into(),
        (TaskStatus::Stopped(_), _) => "stopped".into(),
        (TaskStatus::Failed(_), _) => "failed".into(),
        _ => "idle for 4 hours".into(),
    }
}
