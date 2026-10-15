//! Moving a playground into a GitHub repository (SPEC-176, ADR-0142). An admin of the playground
//! who could import an empty repository of the instance owner (IMP-002) moves it there: every
//! branch and tag of its bare `origin` is pushed in one atomic push, the clone's `origin` becomes
//! the repository, and the record's compare-and-set turns it into a GitHub sandbox, which is the
//! commit point. A failure before it leaves the playground as it was. The bare `origin` is deleted
//! after it. The move runs detached from its request, so a client that goes away never leaves it
//! half done (ADR-0053).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_contracts::api::sandboxes::Sandbox;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::task::TaskStatus;

use super::no_token;
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret};
use crate::projects::{blocking, clone_url};

impl App {
    /// `POST /api/sandboxes/{sid}/move-to-github` `{name}` (MPG-001..005): moves the playground
    /// `sid` into the empty repository `<instance owner>/<name>` and answers its new view.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` without admin on the playground or the repository,
    /// `Invalid` for a name that is no repository of the instance owner, `Conflict` for a GitHub
    /// sandbox, a missing App, a live task, a repository that is a sandbox or is not empty;
    /// GitHub and Git failures, after which the playground is as it was.
    pub async fn move_playground_to_github(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
        name: &str,
    ) -> AppResult<Sandbox> {
        let (app, ctx) = (Arc::clone(self), ctx.clone());
        let (sid, name) = (sid.to_owned(), name.trim().to_owned());
        Self::run_detached(async move { app.move_to_github(&ctx, &sid, &name).await }).await
    }

    /// [`App::move_playground_to_github`], run to its end whatever happens to the request.
    async fn move_to_github(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
        name: &str,
    ) -> AppResult<Sandbox> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &id, Action::Administer).await?;
        refuse_github_sandbox(&record)?;
        let (owner, _) = self.instance_owner().await.map_err(|err| match err {
            AppError::Conflict(_) => AppError::Conflict(
                "connect this instance to GitHub first (Administration → GitHub): a playground \
                 moves into a repository of the instance's GitHub owner"
                    .into(),
            ),
            other => other,
        })?;
        self.refuse_linked(&owner, name).await?;
        let repo = self.import_permission(&ctx.user, &owner, name).await?;
        let owner = if repo.owner.is_empty() {
            owner
        } else {
            repo.owner.clone()
        };
        let name = if repo.name.is_empty() {
            name.to_owned()
        } else {
            repo.name.clone()
        };
        let url = clone_url(&owner, &name);
        let token = self.installation_token().await?;
        // Merges of this playground hold the same pass: none lands on `origin` during the move.
        // Taken only now, so GitHub's answers above never hold up the sandbox's merges.
        let pass = self.armed_pass(&id);
        let _pass = pass.lock().await;
        // Read under the pass: a move or merge that held it before may have changed it.
        let record = self
            .deps
            .store
            .sandbox(&id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        refuse_github_sandbox(&record)?;
        self.refuse_busy(&id).await?;
        // GitHub's name for the repository, which may differ from the one typed (a renamed
        // repository): never push into another sandbox's repository.
        self.refuse_linked(&owner, &name).await?;
        let origin = self.playground_origin(&id);
        let target = Target {
            url: &url,
            owner: &owner,
            name: &name,
            default_branch: &record.default_branch,
        };
        self.push_playground(&origin, &target, &token).await?;
        self.repoint_clone(&record, &url, &token).await?;
        // Again before the commit point: a task may have started during the push.
        if let Err(err) = self.refuse_busy(&id).await {
            self.restore_origin(&record).await;
            return Err(err);
        }
        let repo_id = (repo.id > 0).then_some(repo.id);
        let recorded = self
            .deps
            .store
            .move_to_github(&id, &owner, &name, repo_id)
            .await;
        let failure = match recorded {
            Ok(true) => None,
            Ok(false) => Some(AppError::Conflict(
                "the playground changed during the move (removed or moved already); nothing was \
                 changed here"
                    .into(),
            )),
            Err(err) => Some(err),
        };
        if let Some(err) = failure {
            self.restore_origin(&record).await;
            return Err(err);
        }
        // The move is done. Its code is on GitHub now: the server's copy goes, as a purge would
        // delete it. What follows only reports it, and never turns it into an error.
        delete_origin(origin).await;
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "sandbox.moved_to_github",
                Some(id.as_str()),
                Some(&format!("{} → {owner}/{name}", record.slug)),
            )
            .await
        {
            tracing::warn!(sandbox = %id, error = %err, "could not audit a playground's move to GitHub");
        }
        // Access mirrors GitHub now: every viewer reads its sandboxes again.
        self.publish_resync(None);
        let moved = self
            .deps
            .store
            .sandbox(&id)
            .await?
            .ok_or(AppError::NotFound)?;
        let level = self
            .access_level(&ctx.user, &moved)
            .await
            .unwrap_or(AccessLevel::None);
        self.sandbox_view(&moved, level).await
    }

    /// Refuses the move while a task of the sandbox is live or queued, or a CI run of it is
    /// (MPG-005): the agent was started, or is about to start, with a playground's briefing, tools
    /// and credentials, and the run tests a commit that may exist only on this server. A chained
    /// or prepared task starts later, as a GitHub task.
    async fn refuse_busy(&self, id: &SandboxId) -> AppResult<()> {
        let testing = self
            .deps
            .store
            .active_ci_runs()
            .await?
            .iter()
            .any(|run| run.sandbox_id == *id);
        if testing {
            return Err(AppError::Conflict(
                "Yulius CI is testing this playground: move it once its run ended".into(),
            ));
        }
        let mut live: Vec<u32> = self
            .deps
            .store
            .tasks(id, false)
            .await?
            .into_iter()
            .chain(self.deps.store.tasks(id, true).await?)
            .filter(|task| {
                task.status.is_live()
                    || matches!(
                        task.status,
                        TaskStatus::Queued | TaskStatus::WaitingCapacity
                    )
            })
            .map(|task| task.number)
            .collect();
        if live.is_empty() {
            return Ok(());
        }
        live.sort_unstable();
        Err(AppError::Conflict(format!(
            "stop the playground's live and queued tasks first ({}): they were started for a \
             playground; they keep their history and continue on GitHub after the move",
            live.iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// Pushes every branch and tag of the playground's `origin` to `url` in one atomic push, when
    /// the repository is empty. A repository that holds only them, at their commits or behind
    /// them (a move that failed after its push), is brought up to date (MPG-002).
    async fn push_playground(
        &self,
        origin: &Path,
        target: &Target<'_>,
        token: &Secret,
    ) -> AppResult<()> {
        let &Target {
            url,
            owner,
            name,
            default_branch,
        } = target;
        let git = &self.deps.git;
        let ours = git
            .remote_refs(origin, &origin.to_string_lossy(), &no_token())
            .await?;
        let theirs = git.remote_refs(origin, url, token).await?;
        if theirs == ours {
            return Ok(());
        }
        if theirs.is_empty() {
            // The playground's default branch first: GitHub makes the first branch pushed into
            // an empty repository its default.
            let wanted = format!("refs/heads/{default_branch}");
            let first = ours
                .iter()
                .any(|(name, _)| *name == wanted)
                .then_some(default_branch);
            return git.push_all_refs(origin, url, first, token).await;
        }
        // An earlier move that failed after its push left GitHub with the playground's refs as
        // they were; merges since only moved them forward, and the push fast-forwards them.
        let ours: HashMap<&str, &str> = ours
            .iter()
            .map(|(name, sha)| (name.as_str(), sha.as_str()))
            .collect();
        let mut behind = true;
        for (name, sha) in &theirs {
            behind = match ours.get(name.as_str()) {
                Some(our) if *our == sha => true,
                Some(our) => git.is_ancestor(origin, sha, our).await.unwrap_or(false),
                None => false,
            };
            if !behind {
                break;
            }
        }
        if behind {
            return git.push_all_refs(origin, url, None, token).await;
        }
        Err(AppError::Conflict(format!(
            "{owner}/{name} is not empty ({} on GitHub); a playground moves only into an empty \
             repository: create a new one without a README, license or .gitignore",
            plural_refs(theirs.len())
        )))
    }

    /// Refuses a repository that a sandbox already holds.
    async fn refuse_linked(&self, owner: &str, name: &str) -> AppResult<()> {
        match self.deps.store.sandbox_by_repo(owner, name).await? {
            Some(existing) => Err(AppError::Conflict(format!(
                "{owner}/{name} is already the sandbox /s/{}",
                existing.slug
            ))),
            None => Ok(()),
        }
    }

    /// Points the clone's `origin` at `url` and fetches it; on failure points it back.
    async fn repoint_clone(
        &self,
        record: &SandboxRecord,
        url: &str,
        token: &Secret,
    ) -> AppResult<()> {
        let clone = self.sandbox_repo(&record.id);
        let git = &self.deps.git;
        git.set_origin_url(&clone, url).await?;
        if let Err(err) = git.sync_clone(url, &clone, token).await {
            self.restore_origin(record).await;
            return Err(err);
        }
        Ok(())
    }

    /// Points a playground's clone back at its own `origin` after a failed move.
    async fn restore_origin(&self, record: &SandboxRecord) {
        let clone = self.sandbox_repo(&record.id);
        let origin = self.playground_origin(&record.id);
        if let Err(err) = self
            .deps
            .git
            .set_origin_url(&clone, &origin.to_string_lossy())
            .await
        {
            tracing::warn!(sandbox = %record.id, error = %err, "could not point the playground's clone back at its origin");
        }
    }
}

/// The GitHub repository a playground moves into.
struct Target<'a> {
    url: &'a str,
    owner: &'a str,
    name: &'a str,
    /// The playground's default branch, pushed first so GitHub takes it as its default.
    default_branch: &'a str,
}

/// Refuses a sandbox that is no playground (any more).
fn refuse_github_sandbox(record: &SandboxRecord) -> AppResult<()> {
    if record.is_playground() {
        return Ok(());
    }
    Err(AppError::Conflict(format!(
        "{}/{} is already a GitHub repository's sandbox; only a playground moves to GitHub",
        record.repo_owner, record.repo_name
    )))
}

fn plural_refs(count: usize) -> String {
    if count == 1 {
        "1 branch or tag".to_owned()
    } else {
        format!("{count} branches and tags")
    }
}

/// Deletes a moved playground's bare `origin`; a failure is logged, the move stays done.
async fn delete_origin(origin: PathBuf) {
    let path = origin.clone();
    let removed = blocking(move || match std::fs::remove_dir_all(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(AppError::internal(err)),
        _ => Ok(()),
    })
    .await;
    if let Err(err) = removed {
        tracing::warn!(origin = %origin.display(), error = %err, "could not delete a moved playground's origin");
    }
}
