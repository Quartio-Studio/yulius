//! Deleting a removed sandbox's files for good (SPEC-171 PGL-005..007 for playgrounds, ADR-0139;
//! SPEC-276 RDA-010/RDA-011 for every sandbox, ADR-0205). The purge is a recorded request: it
//! deletes every file of the sandbox on this server (its clone, home and sessions, worktrees, CI
//! directories, browser profile, system layer, runtime state, runs, transcripts, export and a
//! playground's `origin`) through the leftovers' guards and mapped removal, only once nothing of the
//! sandbox runs (no agent run or task still stopping, no shell, no browser, no CI run, no deploy,
//! nothing left in its namespaces, its holder stopped). It is tried at once, every
//! [`PURGE_RETRY`] for [`PURGE_WATCH`], and on every disk round until done. The sandbox row, its
//! tasks and the audit trail stay; a GitHub repository is never touched; a playground's quota no
//! longer counts it.

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use yulius_contracts::api::sandboxes::PurgeState;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};

/// Most directories made writable while deleting one tree (a tool's read-only cache).
const MAX_DIRS_MADE_WRITABLE: usize = 1_000_000;
/// The purge request answers within this; a longer deletion goes on in the background.
pub const PURGE_ANSWER: Duration = Duration::from_secs(20);
/// A waiting purge is tried again this often after its request.
pub const PURGE_RETRY: Duration = Duration::from_secs(15);
/// ... for this long; the disk rounds take it from there.
pub const PURGE_WATCH: Duration = Duration::from_secs(10 * 60);

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Deletes the tree at `path` without following links. A tree some tool made read-only (a Go
/// module cache) is made writable and deleted again. `Ok` once nothing is left at `path`.
pub(crate) fn delete_tree(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {}
    }
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        seen += 1;
        if seen > MAX_DIRS_MADE_WRITABLE {
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let mut permissions = meta.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        let _ = std::fs::set_permissions(&dir, permissions);
        if let Ok(children) = std::fs::read_dir(&dir) {
            stack.extend(children.flatten().map(|entry| entry.path()));
        }
    }
    match std::fs::remove_dir_all(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// What a purge answers while another one of the same sandbox deletes its files.
fn deleting() -> PurgeState {
    PurgeState {
        state: "waiting".into(),
        waiting_for: vec!["its files being deleted".into()],
    }
}

/// Marks a sandbox's purge running until dropped.
struct PurgeMark<'a> {
    app: &'a App,
    sandbox: SandboxId,
}

impl<'a> PurgeMark<'a> {
    fn take(app: &'a App, sandbox: &SandboxId) -> Option<Self> {
        let mut running = app
            .purging
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        running.insert(sandbox.clone()).then(|| Self {
            app,
            sandbox: sandbox.clone(),
        })
    }
}

impl Drop for PurgeMark<'_> {
    fn drop(&mut self) {
        self.app
            .purging
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.sandbox);
    }
}

impl App {
    /// `POST /api/sandboxes/{sid}/purge` (PGL-005, RDA-011): an admin of a live sandbox removes it
    /// (as `DELETE` does) and asks for its files to be deleted for good; the superadmin may ask it
    /// for an already removed sandbox. Answers where the purge stands; one that waits is tried
    /// again every 15 s for 10 minutes, then on every disk round.
    ///
    /// # Errors
    /// `NotFound` without access (a removed sandbox for anyone but the superadmin); `Forbidden`
    /// without admin.
    pub async fn purge_sandbox(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
    ) -> AppResult<PurgeState> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let store = &self.deps.store;
        let record = store.sandbox(&id).await?.ok_or(AppError::NotFound)?;
        if record.removed_at.is_none() {
            self.require(&ctx.user, &id, Action::Administer).await?;
        } else if !ctx.user.is_superadmin {
            return Err(AppError::NotFound);
        }
        if record.removed_at.is_none() {
            self.delete_sandbox(ctx, sid).await?;
        }
        if store.request_purge(&id, &ctx.user.id, self.now()).await? {
            let what = if record.is_playground() {
                "its code and every file of it on this server are"
            } else {
                "every file of it on this server is (its GitHub repository stays)"
            };
            store
                .audit(
                    self.now(),
                    &self.login_of(&ctx.user.id).await,
                    "sandbox.purge_requested",
                    Some(id.as_str()),
                    Some(&format!(
                        "{}: {what} deleted once nothing of it runs",
                        record.slug
                    )),
                )
                .await?;
        }
        // Deleting tens of gigabytes takes a while: the request answers after [`PURGE_ANSWER`]
        // at most while the deletion goes on (it also outlives a client that went away).
        let app = Arc::clone(self);
        let purge = id.clone();
        let running = tokio::spawn(async move { app.try_purge(&purge).await });
        let state = match tokio::time::timeout(PURGE_ANSWER, running).await {
            Ok(joined) => joined.map_err(AppError::internal)??,
            Err(_) => deleting(),
        };
        if state.state != "purged" {
            self.watch_purge_with(id, PURGE_RETRY, PURGE_WATCH);
        }
        Ok(state)
    }

    /// Tries the purge of `id` again every `retry` until it is done or `watch` passed (its
    /// processes stop within about a minute of the removal); the disk rounds take it from there.
    /// The request uses `PURGE_RETRY` and `PURGE_WATCH` (tests shorten them).
    pub fn watch_purge_with(self: &Arc<Self>, id: SandboxId, retry: Duration, watch: Duration) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let until = tokio::time::Instant::now() + watch;
            while tokio::time::Instant::now() < until {
                tokio::time::sleep(retry).await;
                match app.try_purge(&id).await {
                    Ok(state) if state.state == "purged" => return,
                    Ok(_) => {}
                    Err(err) => tracing::warn!(sandbox = %id, error = %err, "purge failed"),
                }
            }
        });
    }

    /// One round over the pending purges (PGL-006); returns how many were done. Runs with the
    /// disk upkeep.
    pub async fn run_purges(&self) -> usize {
        let pending = match self.deps.store.pending_purges().await {
            Ok(pending) => pending,
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the pending purges");
                return 0;
            }
        };
        let mut done = 0;
        for id in pending {
            match self.try_purge(&id).await {
                Ok(state) if state.state == "purged" => done += 1,
                Ok(_) => {}
                Err(err) => tracing::warn!(sandbox = %id, error = %err, "purge failed"),
            }
        }
        done
    }

    /// What of the sandbox still runs, as people read it: agent runs (and tasks still stopping),
    /// shells, the browser, a CI run and other processes in its namespaces.
    pub(crate) async fn still_running(&self, id: &SandboxId) -> AppResult<Vec<String>> {
        let store = &self.deps.store;
        let mut tasks = store.tasks(id, false).await?;
        tasks.extend(store.tasks(id, true).await?);
        let runs = store
            .live_attempts()
            .await?
            .iter()
            .filter(|attempt| tasks.iter().any(|task| task.id == attempt.task_id))
            .count();
        let stopping = tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Stopping)
            .count();
        let mut waiting = Vec::new();
        if runs.max(stopping) > 0 {
            waiting.push(plural(runs.max(stopping), "agent run"));
        }
        let shells = self.sandbox_shells(id).len();
        if shells > 0 {
            waiting.push(plural(shells, "shell"));
        }
        if self.deps.browser.running(id).await.is_some() {
            waiting.push("the browser".to_owned());
        }
        if self.ci_busy(id) {
            waiting.push("a CI run".to_owned());
        }
        if let Some(isolation) = self.isolation.get() {
            let processes = isolation.processes(id).await;
            if processes > 0 && waiting.is_empty() {
                waiting.push(plural(processes, "other process"));
            }
        }
        Ok(waiting)
    }

    /// Deletes the requested purge's files when nothing of the sandbox runs; otherwise deletes
    /// nothing and says what still runs.
    async fn try_purge(&self, id: &SandboxId) -> AppResult<PurgeState> {
        let store = &self.deps.store;
        let Some(sandbox) = store.sandbox(id).await?.filter(|s| s.removed_at.is_some()) else {
            return Err(AppError::Conflict(format!(
                "{id} is not a removed sandbox; nothing is purged"
            )));
        };
        let purged = || PurgeState {
            state: "purged".into(),
            waiting_for: Vec::new(),
        };
        // One purge of a sandbox at a time: a request, its watch and a round may meet.
        let Some(_one) = PurgeMark::take(self, id) else {
            return Ok(deleting());
        };
        // Done already (a request again, or a round that ran beside this one).
        if !store.pending_purges().await?.contains(id) {
            return Ok(purged());
        }
        let waiting = self.still_running(id).await?;
        if !waiting.is_empty() {
            return Ok(PurgeState {
                state: "waiting".into(),
                waiting_for: waiting,
            });
        }
        if let Some(why) = self.purge_removed_files(&sandbox).await? {
            return Ok(PurgeState {
                state: "waiting".into(),
                waiting_for: vec![why],
            });
        }
        self.forget_leftovers_of(id);
        if !store.finish_purge(id, self.now()).await? {
            return Ok(purged());
        }
        store
            .audit(
                self.now(),
                "yulius",
                "sandbox.purged",
                Some(id.as_str()),
                Some(&format!(
                    "{}: every file of it on this server is deleted; tasks and audit kept",
                    sandbox.slug
                )),
            )
            .await?;
        Ok(purged())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_tree_is_deleted_and_a_missing_one_is_fine()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("sandbox");
        let cache = root.join("home/go/pkg/mod/example.com/lib@v1");
        std::fs::create_dir_all(&cache)?;
        std::fs::write(cache.join("lib.go"), "package lib\n")?;
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("keep.txt"), "kept\n")?;
        std::os::unix::fs::symlink(&outside, root.join("link"))?;
        for dir in [cache.as_path(), cache.parent().ok_or("parent")?] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555))?;
        }
        delete_tree(&root)?;
        assert!(!root.exists());
        assert!(
            outside.join("keep.txt").exists(),
            "a link is never followed"
        );
        delete_tree(&root)?;
        Ok(())
    }
}
