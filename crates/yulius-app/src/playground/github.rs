//! Moving a playground into a GitHub repository (SPEC-176, ADR-0142). An admin of the playground
//! who could import an empty repository of the instance owner (IMP-002) moves it there: every
//! branch and tag of its bare `origin` is pushed in one atomic push, the clone's `origin` becomes
//! the repository, and the record's compare-and-set turns it into a GitHub sandbox, which is the
//! commit point. A failure before it leaves the playground as it was. The bare `origin` is deleted
//! after it.

use std::path::Path;
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
use crate::projects::{clone_url, remove_tree};

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
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &id, Action::Administer).await?;
        if !record.is_playground() {
            return Err(AppError::Conflict(format!(
                "{}/{} is already a GitHub repository's sandbox; only a playground moves to GitHub",
                record.repo_owner, record.repo_name
            )));
        }
        let (owner, _) = self.instance_owner().await.map_err(|_| {
            AppError::Conflict(
                "connect this instance to GitHub first (Administration → GitHub): a playground \
                 moves into a repository of the instance's GitHub owner"
                    .into(),
            )
        })?;
        let name = name.trim();
        // Merges of this playground hold the same pass: none lands on `origin` during the move.
        let pass = self.armed_pass(&id);
        let _pass = pass.lock().await;
        self.refuse_live_tasks(&id).await?;
        if let Some(existing) = self.deps.store.sandbox_by_repo(&owner, name).await? {
            return Err(AppError::Conflict(format!(
                "{owner}/{name} is already the sandbox /s/{}",
                existing.slug
            )));
        }
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
        let origin = self.playground_origin(&id);
        self.push_playground(&origin, &url, &token, &owner, &name)
            .await?;
        self.repoint_clone(&record, &url, &token).await?;
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
        // Its code is on GitHub now: the server's copy goes, as a purge would delete it.
        remove_tree(origin).await;
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "sandbox.moved_to_github",
                Some(id.as_str()),
                Some(&format!("{} → {owner}/{name}", record.slug)),
            )
            .await?;
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

    /// Refuses the move while a task of the sandbox is live or queued (MPG-005): its agent was
    /// started, or is about to start, with a playground's briefing, tools and credentials. A
    /// chained or prepared task starts later, as a GitHub task.
    async fn refuse_live_tasks(&self, id: &SandboxId) -> AppResult<()> {
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
    /// the repository is empty. A repository that already holds exactly them (a move that failed
    /// after its push) takes nothing more (MPG-002).
    async fn push_playground(
        &self,
        origin: &Path,
        url: &str,
        token: &Secret,
        owner: &str,
        name: &str,
    ) -> AppResult<()> {
        let git = &self.deps.git;
        let ours = git
            .remote_refs(origin, &origin.to_string_lossy(), &no_token())
            .await?;
        let theirs = git.remote_refs(origin, url, token).await?;
        if theirs.is_empty() {
            return git.push_all_refs(origin, url, token).await;
        }
        if theirs == ours {
            return Ok(());
        }
        Err(AppError::Conflict(format!(
            "{owner}/{name} is not empty ({} on GitHub); a playground moves only into an empty \
             repository: create a new one without a README, license or .gitignore",
            plural_refs(theirs.len())
        )))
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

fn plural_refs(count: usize) -> String {
    if count == 1 {
        "1 branch or tag".to_owned()
    } else {
        format!("{count} branches and tags")
    }
}
