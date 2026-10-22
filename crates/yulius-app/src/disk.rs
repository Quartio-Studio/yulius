//! Disk upkeep without a person (SPEC-008, SPEC-001 NFR-009): build caches of tasks that do not
//! work are removed, the worktrees of closed and cancelled tasks are cleaned up, and a sandbox
//! near its quota gets its Docker engine's unused images and build cache (SPEC-187) and the
//! caches of idle tasks pruned too, before a start is refused. A removed sandbox's files and
//! other leftovers are the leftovers module's (SPEC-215). Each
//! removal is audited; the agents are not told (SPEC-028 MSR-002).
//!
//! Safety: only directories the rules name (recognised tooling or `[disk] caches` of the
//! worktree's `yulius.toml`), never through a link, never `.git`; a working task is never
//! touched; a worktree is removed only when clean. A sandbox can swap any folder below its
//! directory for a link, so every worktree, checkout and cache is reached from the sandbox
//! directory with no link on the way, checked at the scan and again right before its removal,
//! which goes through `O_NOFOLLOW` handles (SPEC-263).

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use yulius_domain::disk::{
    Candidate, Owner, bytes_to_free, cache_paths, human, job_cache_cap, only_rebuildable, owner_of,
    plan_prune, prune_after,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::spec_format::parse_yulius_toml_caches;
use yulius_domain::sweep::Level;
use yulius_domain::task::{Activity, StopReason, TaskStatus};

use crate::context::App;
use crate::error::AppResult;
use crate::leftovers::no_link_between;
use crate::model::{SandboxRecord, TaskRecord};
use crate::ports::isolation::SandboxRef;

/// How often the upkeep runs.
pub const DISK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How often the upkeep reads the data disk's level (SPEC-276 RDA-006).
pub const DISK_TICK: Duration = Duration::from_secs(60);
/// How often Cargo build directories are swept while the disk is not low (SPEC-276 RDA-001).
pub const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// First round after start.
const DISK_FIRST: Duration = Duration::from_secs(2 * 60);
/// A task that ended this recently keeps its caches (it is often continued at once) unless the
/// sandbox is under pressure.
pub const ENDED_GRACE_MS: i64 = 10 * 60 * 1000;
/// Entries one size measurement visits at most.
const MAX_ENTRIES: u64 = 4_000_000;
/// Longest `yulius.toml` read for declared caches.
const MAX_TOML_BYTES: u64 = 64 * 1024;
/// A directory under `worktrees/` that belongs to no task (an agent's own `git worktree add`)
/// untouched this long is treated as ended (F 2026-10-12: `wt-206` kept 2.7 GB).
const ORPHAN_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

/// What a round did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskReport {
    /// Bytes freed by removing caches (worktrees removed whole are counted by their caches) and
    /// by pruning the sandbox's Docker engine.
    pub freed: u64,
    /// Cache directories removed.
    pub caches: usize,
    /// Worktrees removed (tasks closed or cancelled).
    pub worktrees: usize,
}

/// A cache [`App::ended_caches`] found.
pub(crate) struct EndedCache {
    /// Its task; `None` for a worktree no task owns.
    pub task: Option<TaskRecord>,
    /// "task #12" or "worktree wt-206".
    pub label: String,
    pub path: PathBuf,
    /// Relative to the worktree.
    pub relative: String,
    pub bytes: u64,
}

struct Found {
    /// The task whose worktree holds it; `None` for a CI job checkout or an orphan worktree.
    task: Option<TaskRecord>,
    /// A CI job checkout (claimed while its cache goes).
    ci: bool,
    /// What to call it in the audit log.
    label: String,
    owner: Owner,
    path: PathBuf,
    relative: String,
    bytes: u64,
}

#[derive(Default)]
struct Scan {
    found: Vec<Found>,
    /// Worktrees of closed and cancelled tasks.
    ended: Vec<(TaskRecord, PathBuf)>,
}

/// Whether the upkeep runs a round now: the first one, one [`DISK_EVERY`] after the last
/// (`since_last`), or at once when the data disk's level rose (SPEC-276 RDA-006).
pub(crate) fn round_due(since_last: Option<Duration>, level: Level, before: Level) -> bool {
    since_last.is_none_or(|since| since >= DISK_EVERY) || level > before
}

/// Allocated bytes under `path` (links not followed), bounded in entries.
pub(crate) fn dir_size(path: &Path) -> u64 {
    let mut bytes = 0u64;
    let mut entries = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(next) = stack.pop() {
        entries += 1;
        if entries > MAX_ENTRIES {
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        bytes = bytes.saturating_add(meta.blocks().saturating_mul(512));
        if meta.is_dir()
            && let Ok(children) = std::fs::read_dir(&next)
        {
            stack.extend(children.flatten().map(|e| e.path()));
        }
    }
    bytes
}

/// Whether `path` is a real directory below `root` (a sandbox's directory) with no link on the
/// way: every folder from `root` (exclusive) down to it, and it itself (SPEC-263 DUL-001).
fn real_dir_from(root: &Path, path: &Path) -> bool {
    no_link_between(root, path) && std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Removes the cache `path` below the sandbox directory `root` after checking it again: a folder
/// swapped for a link since the scan is refused, and one swapped after this check fails the
/// removal's `O_NOFOLLOW` walk (SPEC-263 DUL-002). `remove_below` reaches entries through
/// `/proc/self/fd`, so other systems (macOS development hosts, whose runs are not contained)
/// refuse instead of removing by path.
fn remove_cache(root: &Path, path: &Path) -> std::io::Result<()> {
    crate::race::reached(crate::race::Point::CachePrune, path);
    if !real_dir_from(root, path) {
        return Err(std::io::Error::other(
            "not a real directory, or a link on the way",
        ));
    }
    if cfg!(target_os = "linux") {
        crate::leftovers::remove_below(root, path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "caches are removed through directory handles on Linux only",
        ))
    }
}

fn root_entries(worktree: &Path) -> Vec<String> {
    std::fs::read_dir(worktree)
        .map(|dir| {
            dir.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn declared_caches(worktree: &Path) -> Vec<String> {
    let path = worktree.join("yulius.toml");
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return Vec::new();
    };
    if !meta.is_file() || meta.len() > MAX_TOML_BYTES {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| parse_yulius_toml_caches(&text).ok())
        .unwrap_or_default()
}

/// Marks a sandbox's upkeep running until dropped.
struct UpkeepMark<'a> {
    app: &'a App,
    sandbox: SandboxId,
}

impl<'a> UpkeepMark<'a> {
    fn take(app: &'a App, sandbox: &SandboxId) -> Option<Self> {
        let mut running = app
            .upkeeps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        running.insert(sandbox.clone()).then(|| Self {
            app,
            sandbox: sandbox.clone(),
        })
    }
}

impl Drop for UpkeepMark<'_> {
    fn drop(&mut self) {
        self.app
            .upkeeps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.sandbox);
    }
}

impl App {
    pub(super) fn start_disk_upkeep(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(DISK_FIRST).await;
            // The disk's level is read every minute; a round runs every DISK_EVERY, and at once
            // when the level rises (SPEC-276 RDA-006).
            let mut tick = tokio::time::interval(DISK_TICK);
            let mut rounds: Option<tokio::time::Instant> = None;
            let mut swept: Option<tokio::time::Instant> = None;
            loop {
                tick.tick().await;
                let (level, before) = app.check_disk().await;
                if !round_due(rounds.map(|at| at.elapsed()), level, before) {
                    continue;
                }
                rounds = Some(tokio::time::Instant::now());
                // Cargo build directories lose what nobody used (RDA-001): hourly, and every
                // round while the disk is low.
                let sweep_due = swept.is_none_or(|at| at.elapsed() >= SWEEP_EVERY);
                if sweep_due || level >= Level::Low {
                    swept = Some(tokio::time::Instant::now());
                }
                app.disk_round(sweep_due).await;
            }
        });
    }

    pub(crate) fn sandbox_ref(&self, sandbox: &SandboxRecord) -> SandboxRef {
        let first = sandbox.port_base;
        SandboxRef {
            id: sandbox.id.clone(),
            hostname: crate::identity::sandbox_hostname(&sandbox.slug),
            dir: self.sandbox_dir(&sandbox.id),
            port_first: first,
            port_last: first.saturating_add(sandbox.port_count.saturating_sub(1)),
        }
    }

    /// One round for `sandbox`: caches and worktrees of ended tasks (after a grace period), and
    /// at 90 % of the quota — or with `pressure`, when a start was refused — caches of idle
    /// tasks until it is below 80 %.
    ///
    /// # Errors
    /// Store failures.
    pub async fn disk_upkeep(
        &self,
        sandbox: &SandboxRecord,
        pressure: bool,
    ) -> AppResult<DiskReport> {
        self.disk_upkeep_needing(sandbox, pressure, 0).await
    }

    /// [`App::disk_upkeep`] that also frees `host_need` bytes for a critical data disk (SPEC-276
    /// RDA-006), as quota pressure does: the engine's unused images, then the largest caches of
    /// idle tasks and CI checkouts.
    ///
    /// # Errors
    /// Store failures.
    pub async fn disk_upkeep_needing(
        &self,
        sandbox: &SandboxRecord,
        pressure: bool,
        host_need: u64,
    ) -> AppResult<DiskReport> {
        // One upkeep of a sandbox at a time (the archive's round beside the loop's): a second
        // would remove the same caches and report spurious failures.
        let Some(_one) = UpkeepMark::take(self, &sandbox.id) else {
            return Ok(DiskReport::default());
        };
        let reference = self.sandbox_ref(sandbox);
        let isolation = self.isolation.get();
        let usage = match isolation {
            Some(isolation) => isolation.disk(&reference).await,
            None => None,
        };
        let need = usage
            .map_or(0, |usage| bytes_to_free(usage.used, usage.limit))
            .max(host_need);
        // The engine's unused images and build cache go first: rebuilt or pulled again, never
        // a task's (SPEC-187 DKR-006).
        let engine_freed = match isolation {
            Some(isolation) if need > 0 => isolation.prune_engine(&reference).await.unwrap_or(0),
            _ => 0,
        };
        if engine_freed > 0 {
            self.dlv_audit(
                "yulius",
                "disk.prune",
                &sandbox.slug,
                &format!(
                    "unused container images and build cache: {}",
                    human(engine_freed)
                ),
            )
            .await;
        }
        let need = need.saturating_sub(engine_freed);
        let cap = usage.map(|usage| job_cache_cap(usage.limit));
        let scan = self.disk_scan(sandbox, pressure || need > 0, cap).await?;
        let candidates: Vec<Candidate> = scan
            .found
            .iter()
            .map(|f| Candidate {
                owner: f.owner,
                bytes: f.bytes,
            })
            .collect();
        let chosen = plan_prune(&candidates, need, need > 0);
        let mut report = self.prune(sandbox, &scan, &chosen).await;
        report.freed = report.freed.saturating_add(engine_freed);
        if (report.freed > 0 || report.worktrees > 0)
            && let Some(isolation) = isolation
        {
            isolation.measure_disk(&reference).await;
        }
        Ok(report)
    }

    /// The largest consumers of the sandbox's last measurement, for people: "task #26 35.0 GB, CI
    /// job checkouts 14.0 GB"; empty when nothing was measured (SPEC-059 DSK-202).
    pub(crate) async fn disk_consumers(&self, sandbox: &SandboxRecord) -> String {
        let Some(isolation) = self.isolation.get() else {
            return String::new();
        };
        let mut parts = Vec::new();
        for (path, bytes) in isolation.largest(&self.sandbox_ref(sandbox)).await {
            let label = match path.strip_prefix("worktrees/") {
                Some(name) => match TaskId::parse(name) {
                    Ok(id) => match self.deps.store.task(&id).await {
                        Ok(Some(task)) => format!("task #{}", task.number),
                        _ => format!("worktree {name}"),
                    },
                    Err(_) => format!("worktree {name}"),
                },
                None if path == "ci/jobs" => "CI job checkouts".to_owned(),
                None => path,
            };
            parts.push(format!("{label} {}", human(bytes)));
        }
        parts.join(", ")
    }

    /// The cache directories of one worktree or checkout of the sandbox directory `root`, with
    /// their sizes: recognised or declared paths that are real directories reached from `root`
    /// with no link on the way (none when the worktree is not one), ignored by Git and holding
    /// no tracked file (a path a commit has files in is the project's work, not a cache, whatever
    /// `yulius.toml` says). None off Linux, where no cache is removed ([`remove_cache`]), so
    /// they are not measured every round for nothing.
    async fn find_caches(&self, root: &Path, worktree: &Path) -> Vec<(String, PathBuf, u64)> {
        if !cfg!(target_os = "linux") {
            return Vec::new();
        }
        let (top, dir) = (root.to_path_buf(), worktree.to_path_buf());
        let present = tokio::task::spawn_blocking(move || {
            if !real_dir_from(&top, &dir) {
                return Vec::new();
            }
            cache_paths(&root_entries(&dir), &declared_caches(&dir))
                .into_iter()
                .filter_map(|relative| {
                    let path = dir.join(&relative);
                    real_dir_from(&top, &path).then_some((relative, path))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let mut found = Vec::new();
        for (relative, path) in present {
            if !self
                .deps
                .git
                .is_disposable(worktree, &relative)
                .await
                .unwrap_or(false)
            {
                continue;
            }
            // Checked again right before it is measured: Git's answer took a while.
            let (root, measured) = (root.to_path_buf(), path.clone());
            let bytes = tokio::task::spawn_blocking(move || {
                real_dir_from(&root, &measured).then(|| dir_size(&measured))
            })
            .await
            .unwrap_or_default();
            let Some(bytes) = bytes else {
                continue;
            };
            found.push((relative, path, bytes));
        }
        found
    }

    /// The caches of every task that does not work and of idle CI checkouts, with their sizes,
    /// and the worktrees of closed and cancelled tasks. Ended tasks inside the grace period
    /// are left out unless `pressure`; `cap` is the most one CI job may keep.
    async fn disk_scan(
        &self,
        sandbox: &SandboxRecord,
        pressure: bool,
        cap: Option<u64>,
    ) -> AppResult<Scan> {
        let sandbox_dir = self.sandbox_dir(&sandbox.id);
        let root = sandbox_dir.join("worktrees");
        let now = self.now().0;
        let mut scan = Scan::default();
        for archived in [false, true] {
            for task in self.deps.store.tasks(&sandbox.id, archived).await? {
                let Some(owner) = self.cache_owner(&task, pressure).await else {
                    continue;
                };
                // An archived task was put away: no grace (SPEC-276 RDA-008).
                if owner == Owner::Ended
                    && !pressure
                    && task.archived_at.is_none()
                    && now - task.updated_at.0 < ENDED_GRACE_MS
                {
                    continue;
                }
                let worktree = root.join(task.id.as_str());
                if !real_dir_from(&sandbox_dir, &worktree) || !worktree.join(".git").exists() {
                    continue;
                }
                if matches!(
                    task.status,
                    TaskStatus::Closed | TaskStatus::Stopped(StopReason::Cancelled)
                ) {
                    scan.ended.push((task.clone(), worktree.clone()));
                }
                for (relative, path, bytes) in self.find_caches(&sandbox_dir, &worktree).await {
                    scan.found.push(Found {
                        label: format!("task #{}", task.number),
                        task: Some(task.clone()),
                        ci: false,
                        owner,
                        path,
                        relative,
                        bytes,
                    });
                }
            }
        }
        // Worktrees no task owns (an agent's own `git worktree add` next to its task's): their
        // caches go once nobody touched them for a day.
        let mut tasks: Vec<String> = Vec::new();
        for archived in [false, true] {
            for task in self.deps.store.tasks(&sandbox.id, archived).await? {
                tasks.push(task.id.to_string());
            }
        }
        let entries: Vec<(String, PathBuf, i64)> = std::fs::read_dir(&root)
            .map(|dir| {
                dir.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let modified = e.metadata().ok()?.mtime() * 1000;
                        Some((name, e.path(), modified))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (name, worktree, modified) in entries {
            if tasks.contains(&name)
                || now - modified < ORPHAN_AFTER_MS
                || !real_dir_from(&sandbox_dir, &worktree)
                || !worktree.join(".git").exists()
            {
                continue;
            }
            for (relative, path, bytes) in self.find_caches(&sandbox_dir, &worktree).await {
                scan.found.push(Found {
                    label: format!("worktree {name}"),
                    task: None,
                    ci: false,
                    owner: Owner::Ended,
                    path,
                    relative,
                    bytes,
                });
            }
        }
        // Persistent CI job checkouts (SPEC-008): their caches go while no run executes — only
        // under pressure, or at once when one job's cache outgrows `cap`.
        if !self.ci_busy(&sandbox.id) {
            let jobs = self.ci_dir(&sandbox.id).join("jobs");
            let names: Vec<String> = std::fs::read_dir(&jobs)
                .map(|dir| {
                    dir.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            for name in names {
                let checkout = jobs.join(&name);
                if !real_dir_from(&sandbox_dir, &checkout) || !checkout.join(".git").exists() {
                    continue;
                }
                for (relative, path, bytes) in self.find_caches(&sandbox_dir, &checkout).await {
                    let owner = if cap.is_some_and(|cap| bytes > cap) {
                        Owner::Ended
                    } else {
                        Owner::Idle
                    };
                    scan.found.push(Found {
                        label: format!("CI job {name}"),
                        task: None,
                        ci: true,
                        owner,
                        path,
                        relative,
                        bytes,
                    });
                }
            }
        }
        Ok(scan)
    }

    async fn prune(&self, sandbox: &SandboxRecord, scan: &Scan, chosen: &[usize]) -> DiskReport {
        let mut report = DiskReport::default();
        for &index in chosen {
            let item = &scan.found[index];
            // The scan can be minutes old (sizes of huge caches): a task that started working
            // meanwhile keeps its files.
            let unchanged = match &item.task {
                Some(task) => self.owner_is(&task.id, item.owner).await,
                None => true,
            };
            if !unchanged {
                continue;
            }
            // A CI checkout is claimed while its cache goes, so no run starts in it half
            // deleted; a run that executes (or a claim) keeps it.
            let claimed = item.ci;
            if claimed && !self.ci_claim(&sandbox.id) {
                continue;
            }
            let (root, path) = (self.sandbox_dir(&sandbox.id), item.path.clone());
            let removed = tokio::task::spawn_blocking(move || remove_cache(&root, &path))
                .await
                .unwrap_or_else(|err| Err(std::io::Error::other(err)));
            if claimed {
                self.ci_release(&sandbox.id);
            }
            if let Err(err) = removed {
                tracing::warn!(
                    path = %item.path.display(),
                    error = %err,
                    "a build cache could not be removed"
                );
                continue;
            }
            report.freed = report.freed.saturating_add(item.bytes);
            report.caches += 1;
            self.dlv_audit(
                "yulius",
                "disk.prune",
                &sandbox.slug,
                &format!("{} of {}: {}", item.relative, item.label, human(item.bytes)),
            )
            .await;
        }
        // A closed or cancelled task's clean worktree goes whole (its branch keeps the work;
        // Continue recreates it).
        for (task, worktree) in &scan.ended {
            if !self.still_ended(&task.id).await {
                continue;
            }
            if self.remove_ended_worktree(sandbox, task, worktree).await {
                report.worktrees += 1;
            }
        }
        report
    }

    /// Whether `task` is still what the scan found it to be (ended, or idle).
    async fn owner_is(&self, task: &TaskId, owner: Owner) -> bool {
        matches!(
            self.deps.store.task(task).await,
            Ok(Some(t)) if self.cache_owner(&t, true).await == Some(owner)
        )
    }

    /// Whose caches `task`'s are now: by its state, its background work and how long it has not
    /// moved. An idle running task is probed afresh (SPEC-059 DSK-201): a background shell or
    /// subagent keeps its caches, and so does a probe that cannot answer (#245: a cached
    /// "unknown" read as "none" under a running gate). Without `pressure` an idle task that moved
    /// within [`prune_after`] its activity is idle whatever it runs (its caches stay anyway), so
    /// it is not probed: each probe can take up to the probe timeout.
    pub(crate) async fn cache_owner(&self, task: &TaskRecord, pressure: bool) -> Option<Owner> {
        let quiet_ms = self.now().0 - task.updated_at.0;
        let idle =
            task.status == TaskStatus::Running && task.activity.is_some_and(Activity::is_idle);
        let probe = idle && (pressure || quiet_ms >= prune_after(task.activity));
        let busy = probe && self.background_work_now(&task.id).await != Some(false);
        owner_of(task.status, task.activity, busy, quiet_ms)
    }

    /// The caches the upkeep removes without pressure (SPEC-215 `task-cache`): of tasks that
    /// ended or are done and quiet, past their grace, and of worktrees no task owns.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn ended_caches(&self, sandbox: &SandboxRecord) -> AppResult<Vec<EndedCache>> {
        let scan = self.disk_scan(sandbox, false, None).await?;
        Ok(scan
            .found
            .into_iter()
            .filter(|f| !f.ci && f.owner == Owner::Ended)
            .map(|f| EndedCache {
                task: f.task,
                label: f.label,
                path: f.path,
                relative: f.relative,
                bytes: f.bytes,
            })
            .collect())
    }

    /// Whether `task`'s caches still count as ended (checked again right before a deletion).
    pub(crate) async fn cache_still_ended(&self, task: &TaskId) -> bool {
        self.owner_is(task, Owner::Ended).await
    }

    async fn still_ended(&self, task: &TaskId) -> bool {
        matches!(
            self.deps.store.task(task).await,
            Ok(Some(t)) if matches!(
                t.status,
                TaskStatus::Closed | TaskStatus::Stopped(StopReason::Cancelled)
            )
        )
    }

    /// Whether Git ignores nothing in `worktree` but rebuildable caches (`target`, `.tools`,
    /// `node_modules`, declared ones): files Git ignores that are not (local configuration,
    /// data) are not Git's to lose, so only then may Yulius remove the worktree by itself.
    pub(crate) async fn worktree_only_caches(&self, worktree: &Path) -> bool {
        let declared = declared_caches(worktree);
        matches!(
            self.deps.git.ignored_entries(worktree).await,
            Ok(Some(ignored)) if only_rebuildable(&ignored, &declared)
        )
    }

    async fn remove_ended_worktree(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        worktree: &Path,
    ) -> bool {
        if !self.worktree_only_caches(worktree).await {
            return false;
        }
        // Checked again right before Git removes it: a link on the way would hand Git another
        // sandbox's folder (SPEC-263 DUL-003). Git resolves the path itself once more.
        if !real_dir_from(&self.sandbox_dir(&sandbox.id), worktree) {
            return false;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        let owned = self.sandbox_dir(&sandbox.id).join("worktrees");
        match self.deps.git.remove_worktree(&repo, worktree, &owned).await {
            Ok(true) => {
                self.dlv_audit(
                    "yulius",
                    "hygiene.cleanup",
                    &sandbox.slug,
                    &format!("worktree of task #{}: it ended", task.number),
                )
                .await;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_runs_first_every_five_minutes_and_at_once_when_the_disk_fills() {
        let minute = Duration::from_secs(60);
        assert!(round_due(None, Level::Ok, Level::Ok), "the first one");
        assert!(!round_due(Some(minute), Level::Ok, Level::Ok));
        assert!(round_due(Some(DISK_EVERY), Level::Ok, Level::Ok));
        assert!(
            round_due(Some(minute), Level::Low, Level::Ok),
            "the level rose"
        );
        assert!(round_due(Some(minute), Level::Critical, Level::Low));
        assert!(
            !round_due(Some(minute), Level::Low, Level::Low),
            "low already: every 5 minutes"
        );
        assert!(!round_due(Some(minute), Level::Ok, Level::Low), "it fell");
    }
}
