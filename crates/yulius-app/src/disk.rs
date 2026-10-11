//! Disk upkeep without a person (SPEC-005, SPEC-001 NFR-009): build caches of tasks that do not
//! work are removed, the worktrees of closed and cancelled tasks are cleaned up, and a sandbox
//! near its quota gets the caches of idle tasks pruned too, before a start is refused. The
//! agent of a task whose cache went is told what was freed.
//!
//! Safety: only directories the rules name (recognised tooling or `[disk] caches` of the
//! worktree's `yulius.toml`), never through a link, never `.git`; a working task is never
//! touched; a worktree is removed only when clean.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use yulius_domain::disk::{
    Candidate, Owner, bytes_to_free, cache_paths, human, owner_of, plan_prune,
};
use yulius_domain::ids::{MessageId, TaskId};
use yulius_domain::spec_format::parse_yulius_toml_caches;
use yulius_domain::task::{StopReason, TaskStatus};

use crate::context::App;
use crate::error::AppResult;
use crate::model::{MessageRecord, SandboxRecord, TaskRecord};
use crate::ports::isolation::SandboxRef;

/// How often the upkeep runs.
pub const DISK_EVERY: Duration = Duration::from_secs(5 * 60);
/// First round after start.
const DISK_FIRST: Duration = Duration::from_secs(2 * 60);
/// A task that ended this recently keeps its caches (it is often continued at once) unless the
/// sandbox is under pressure.
pub const ENDED_GRACE_MS: i64 = 10 * 60 * 1000;
/// Entries one size measurement visits at most.
const MAX_ENTRIES: u64 = 4_000_000;
/// Longest `yulius.toml` read for declared caches.
const MAX_TOML_BYTES: u64 = 64 * 1024;

/// What a round did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskReport {
    /// Bytes freed by removing caches (worktrees removed whole are counted by their caches).
    pub freed: u64,
    /// Cache directories removed.
    pub caches: usize,
    /// Worktrees removed (tasks closed or cancelled).
    pub worktrees: usize,
}

struct Found {
    task: TaskRecord,
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

/// Allocated bytes under `path` (links not followed), bounded in entries.
fn dir_size(path: &Path) -> u64 {
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

/// `root/relative` when every component up to and including it is a real directory (no link
/// anywhere on the way), else `None`.
fn real_dir_below(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_dir() {
            return None;
        }
    }
    Some(path)
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

impl App {
    pub(super) fn start_disk_upkeep(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(DISK_FIRST).await;
            let mut every = tokio::time::interval(DISK_EVERY);
            loop {
                every.tick().await;
                let Ok(sandboxes) = app.deps.store.sandboxes().await else {
                    continue;
                };
                for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
                    if let Err(err) = app.disk_upkeep(sandbox, false).await {
                        tracing::warn!(sandbox = %sandbox.id, error = %err, "disk upkeep failed");
                    }
                }
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
        let reference = self.sandbox_ref(sandbox);
        let isolation = self.isolation.get();
        let usage = match isolation {
            Some(isolation) => isolation.disk(&reference).await,
            None => None,
        };
        let need = usage.map_or(0, |usage| bytes_to_free(usage.used, usage.limit));
        let scan = self.disk_scan(sandbox, pressure || need > 0).await?;
        let candidates: Vec<Candidate> = scan
            .found
            .iter()
            .map(|f| Candidate {
                owner: f.owner,
                bytes: f.bytes,
            })
            .collect();
        let chosen = plan_prune(&candidates, need, need > 0);
        let report = self.prune(sandbox, &scan, &chosen).await;
        if (report.freed > 0 || report.worktrees > 0)
            && let Some(isolation) = isolation
        {
            isolation.measure_disk(&reference).await;
        }
        Ok(report)
    }

    /// The caches of every task that does not work, with their sizes, and the worktrees of
    /// closed and cancelled tasks. Ended tasks inside the grace period are left out unless
    /// `pressure`.
    async fn disk_scan(&self, sandbox: &SandboxRecord, pressure: bool) -> AppResult<Scan> {
        let root = self.sandbox_dir(&sandbox.id).join("worktrees");
        let now = self.now().0;
        let mut scan = Scan::default();
        for archived in [false, true] {
            for task in self.deps.store.tasks(&sandbox.id, archived).await? {
                let Some(owner) = owner_of(task.status, task.activity) else {
                    continue;
                };
                if owner == Owner::Ended && !pressure && now - task.updated_at.0 < ENDED_GRACE_MS {
                    continue;
                }
                let worktree = root.join(task.id.as_str());
                if !worktree.join(".git").exists() {
                    continue;
                }
                if matches!(
                    task.status,
                    TaskStatus::Closed | TaskStatus::Stopped(StopReason::Cancelled)
                ) {
                    scan.ended.push((task.clone(), worktree.clone()));
                }
                let dir = worktree.clone();
                let present = tokio::task::spawn_blocking(move || {
                    cache_paths(&root_entries(&dir), &declared_caches(&dir))
                        .into_iter()
                        .filter_map(|relative| {
                            let path = real_dir_below(&dir, &relative)?;
                            Some((relative, path))
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                for (relative, path) in present {
                    // A path a commit has files in (or that Git does not ignore) is the
                    // project's work, not a cache, whatever `yulius.toml` says.
                    if !self
                        .deps
                        .git
                        .is_disposable(&worktree, &relative)
                        .await
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let measured = path.clone();
                    let bytes = tokio::task::spawn_blocking(move || dir_size(&measured))
                        .await
                        .unwrap_or(0);
                    scan.found.push(Found {
                        task: task.clone(),
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
        let mut per_task: Vec<(&TaskRecord, Vec<(String, u64)>)> = Vec::new();
        for &index in chosen {
            let item = &scan.found[index];
            // The scan can be minutes old (sizes of huge caches): a task that started working
            // meanwhile keeps its files.
            if !self.not_working(&item.task.id).await {
                continue;
            }
            let path = item.path.clone();
            let removed = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&path))
                .await
                .is_ok_and(|result| result.is_ok());
            if !removed {
                tracing::warn!(path = %item.path.display(), "a build cache could not be removed");
                continue;
            }
            report.freed = report.freed.saturating_add(item.bytes);
            report.caches += 1;
            self.dlv_audit(
                "yulius",
                "disk.prune",
                &sandbox.slug,
                &format!(
                    "{} of task #{}: {}",
                    item.relative,
                    item.task.number,
                    human(item.bytes)
                ),
            )
            .await;
            match per_task.iter_mut().find(|(t, _)| t.id == item.task.id) {
                Some((_, list)) => list.push((item.relative.clone(), item.bytes)),
                None => per_task.push((&item.task, vec![(item.relative.clone(), item.bytes)])),
            }
        }
        // A closed or cancelled task's clean worktree goes whole (its branch keeps the work;
        // Continue recreates it).
        let mut gone: Vec<&TaskId> = Vec::new();
        for (task, worktree) in &scan.ended {
            if !self.still_ended(&task.id).await {
                continue;
            }
            if self.remove_ended_worktree(sandbox, task, worktree).await {
                report.worktrees += 1;
                gone.push(&task.id);
            }
        }
        for (task, list) in per_task {
            if !gone.contains(&&task.id) {
                self.tell_task(task, &list).await;
            }
        }
        report
    }

    async fn not_working(&self, task: &TaskId) -> bool {
        matches!(
            self.deps.store.task(task).await,
            Ok(Some(t)) if owner_of(t.status, t.activity).is_some()
        )
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

    async fn remove_ended_worktree(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        worktree: &Path,
    ) -> bool {
        // Files Git ignores that are not caches (local configuration) are not Git's to lose.
        if self
            .deps
            .git
            .has_ignored_files(worktree)
            .await
            .unwrap_or(true)
        {
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

    /// Tells the agent of `task` what Yulius freed.
    async fn tell_task(&self, task: &TaskRecord, freed: &[(String, u64)]) {
        let list: Vec<String> = freed
            .iter()
            .map(|(path, bytes)| format!("`{path}` ({})", human(*bytes)))
            .collect();
        let body = format!(
            "Yulius freed disk space in this sandbox: {} of this task's worktree {} removed. They are build caches; the next build creates them again.",
            list.join(", "),
            if freed.len() == 1 { "was" } else { "were" }
        );
        let Ok(id) = MessageId::parse(&self.deps.crypto.new_id()) else {
            return;
        };
        let message = MessageRecord {
            id,
            sandbox_id: task.sandbox_id.clone(),
            from_task: None,
            from_user: None,
            to_task: Some(task.id.clone()),
            kind: "info".into(),
            subject: "Yulius freed disk space".into(),
            body,
            created_at: self.now(),
        };
        if self
            .deps
            .store
            .insert_message(&message, std::slice::from_ref(&task.id))
            .await
            .is_ok()
        {
            self.notify_message(&task.id).await;
        }
    }
}
