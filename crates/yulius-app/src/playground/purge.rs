//! Purging a removed playground's code (SPEC-171 PGL-005..007, ADR-0139). A playground's code
//! exists only on this server: its bare `origin` and its sandbox directory. The purge is a
//! recorded request: it deletes both only once no process of the sandbox runs (no agent run or
//! task still stopping, no shell, no browser, no CI run, nothing left in its namespaces), and it is
//! tried at once and on every disk upkeep round until then. The sandbox row, its tasks, the
//! removal's export and the audit trail stay; the quota, which counts the `origin`s on disk, no
//! longer counts it.

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_contracts::api::sandboxes::PurgeState;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::projects::blocking;

/// Most directories made writable while deleting one tree (a tool's read-only cache).
const MAX_DIRS_MADE_WRITABLE: usize = 1_000_000;

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Deletes the tree at `path` without following links. A tree some tool made read-only (a Go
/// module cache) is made writable and deleted again. `Ok` once nothing is left at `path`.
fn delete_tree(path: &Path) -> io::Result<()> {
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

impl App {
    /// `POST /api/sandboxes/{sid}/purge` (PGL-005): an admin of a live playground removes it (as
    /// `DELETE` does) and asks for its code to be deleted for good; the superadmin may ask it for
    /// an already removed playground. Answers where the purge stands.
    ///
    /// # Errors
    /// `NotFound` without access (a removed playground for anyone but the superadmin);
    /// `Forbidden` without admin; `Conflict` for a GitHub sandbox.
    pub async fn purge_playground(
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
        if !record.is_playground() {
            return Err(AppError::Conflict(
                "only a playground's code can be purged: this sandbox's code is on GitHub, and \
                 removing the sandbox keeps the repository"
                    .into(),
            ));
        }
        if record.removed_at.is_none() {
            self.delete_sandbox(ctx, sid).await?;
        }
        if store.request_purge(&id, &ctx.user.id, self.now()).await? {
            store
                .audit(
                    self.now(),
                    &self.login_of(&ctx.user.id).await,
                    "sandbox.purge_requested",
                    Some(id.as_str()),
                    Some(&format!(
                        "{}: its origin and sandbox directory are deleted once nothing of it runs",
                        record.slug
                    )),
                )
                .await?;
        }
        self.try_purge(&id).await
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
    async fn still_running(&self, id: &SandboxId) -> AppResult<Vec<String>> {
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

    /// Deletes the requested purge's `origin` and sandbox directory when nothing of the sandbox
    /// runs; otherwise deletes nothing and says what still runs.
    async fn try_purge(&self, id: &SandboxId) -> AppResult<PurgeState> {
        let store = &self.deps.store;
        let Some(sandbox) = store
            .sandbox(id)
            .await?
            .filter(|s| s.is_playground() && s.removed_at.is_some())
        else {
            return Err(AppError::Conflict(format!(
                "{id} is not a removed playground; nothing is purged"
            )));
        };
        let purged = || PurgeState {
            state: "purged".into(),
            waiting_for: Vec::new(),
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
        // No CI run may start on its checkouts while they go.
        if !self.ci_claim(id) {
            return Ok(PurgeState {
                state: "waiting".into(),
                waiting_for: vec!["a CI run".into()],
            });
        }
        // Checked again under the claim: a purge beside this one may have finished meanwhile.
        let pending = store.pending_purges().await;
        if !pending.as_ref().is_ok_and(|p| p.contains(id)) {
            self.ci_release(id);
            return pending.map(|_| purged());
        }
        let deleted = self.delete_playground_code(&sandbox).await;
        self.ci_release(id);
        deleted?;
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
                    "{}: its origin and sandbox directory are deleted; tasks and audit kept",
                    sandbox.slug
                )),
            )
            .await?;
        Ok(purged())
    }

    /// Deletes the sandbox directory, then `origin`: a purge that fails half-way keeps the code
    /// (and its place in the quota) and is tried again on the next round.
    async fn delete_playground_code(&self, sandbox: &SandboxRecord) -> AppResult<()> {
        let origin = self.playground_origin(&sandbox.id);
        let dir = self.sandbox_dir(&sandbox.id);
        blocking(move || {
            for path in [&dir, &origin] {
                delete_tree(path).map_err(|err| {
                    AppError::internal(format!("cannot delete {}: {err}", path.display()))
                })?;
            }
            Ok(())
        })
        .await
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
