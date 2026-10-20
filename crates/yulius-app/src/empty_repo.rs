//! An empty GitHub repository (no commits, no branch yet) gets its first commit when its
//! onboarding starts (SPEC-241 EMP-002, ADR-0187): `README.md` on the default branch GitHub
//! reports (`main` when it reports none), pushed with the installation token as a created
//! project's scaffold is. A task's worktree needs a branch to start from; an empty repository
//! has none.

use std::path::Path;

use crate::context::App;
use crate::error::AppResult;
use crate::model::{SandboxRecord, Secret};
use crate::ports::github::RepoAccess;
use crate::projects::{blocking, clone_url, commit_identity, remove_tree, write_scaffold};

/// The branch of a repository whose host reports none.
const FALLBACK_BRANCH: &str = "main";

impl App {
    /// Whether the clone `repo` knows no branch of `origin` (as last fetched). `false` when that
    /// cannot be told: nothing is then written to the repository.
    pub(crate) async fn remote_is_empty(&self, repo: &Path) -> bool {
        match self.deps.git.remote_branches(repo).await {
            Ok(branches) => branches.is_empty(),
            Err(err) => {
                tracing::warn!(error = %err, "listing the clone's remote branches failed");
                false
            }
        }
    }

    /// Makes the sandbox's default branch one its onboarding task can start from (SPEC-241):
    /// when GitHub holds no commit, a first one authored by `actor` (a login; else the sandbox's
    /// creator) on the branch GitHub reports; when the recorded branch is not among the
    /// repository's branches and GitHub's is, the record is corrected. Returns whether it pushed.
    /// A repository with any branch is never written to (EMP-003).
    ///
    /// # Errors
    /// GitHub lookups, fetch, commit or push failures (GitHub refused the push, someone pushed
    /// first); store failures.
    pub(crate) async fn prepare_default_branch(
        &self,
        record: &SandboxRecord,
        actor: &str,
    ) -> AppResult<bool> {
        if record.is_playground() {
            return Ok(false);
        }
        let repo = self.sandbox_repo(&record.id);
        // The recorded branch came from GitHub: nothing to ask GitHub then.
        if self.has_remote_branch(&repo, &record.default_branch).await {
            return Ok(false);
        }
        let _serial = self.repository_seed_lock.lock().await;
        let (owner, name) = (&record.repo_owner, &record.repo_name);
        let url = clone_url(owner, name);
        let token = self.installation_token().await?;
        // What GitHub holds now: someone may have pushed since, or a start just before this one.
        self.deps.git.sync_clone(&url, &repo, &token).await?;
        let branch = self.reported_default_branch(&token, owner, name).await?;
        if !self.remote_is_empty(&repo).await {
            if record.default_branch != branch && self.has_remote_branch(&repo, &branch).await {
                self.deps
                    .store
                    .set_default_branch(&record.id, &branch)
                    .await?;
            }
            return Ok(false);
        }
        self.push_first_commit(record, actor, &token, &branch)
            .await?;
        if record.default_branch != branch {
            self.deps
                .store
                .set_default_branch(&record.id, &branch)
                .await?;
        }
        self.deps.git.sync_clone(&url, &repo, &token).await?;
        self.deps
            .store
            .audit(
                self.now(),
                actor,
                "sandbox.repository.seeded",
                Some(record.id.as_str()),
                Some(&format!(
                    "{owner}/{name} had no commits: README.md pushed to {branch}"
                )),
            )
            .await?;
        Ok(true)
    }

    /// Whether the clone `repo` knows `branch` of `origin` (as last fetched).
    async fn has_remote_branch(&self, repo: &Path, branch: &str) -> bool {
        self.deps
            .git
            .remote_branches(repo)
            .await
            .is_ok_and(|branches| branches.iter().any(|b| b.name == branch))
    }

    /// Commits `README.md` on `branch` in a scratch clone of the sandbox's empty repository and
    /// pushes it without forcing; the scratch clone is removed on every path.
    async fn push_first_commit(
        &self,
        record: &SandboxRecord,
        actor: &str,
        token: &Secret,
        branch: &str,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let author = match store.user_by_login(actor).await? {
            Some(user) => Some(user),
            None => store.user(&record.created_by).await?,
        };
        let (author, email) = commit_identity(author.as_ref());
        let name = &record.repo_name;
        let scratch = self.sandbox_dir(&record.id).join("seed");
        remove_tree(scratch.clone()).await;
        let pushed = async {
            self.deps
                .git
                .sync_clone(&clone_url(&record.repo_owner, name), &scratch, token)
                .await?;
            let files = vec![("README.md".to_owned(), format!("# {name}\n"))];
            let (root, first) = (scratch.clone(), branch.to_owned());
            blocking(move || write_scaffold(&root, &first, &files)).await?;
            self.deps
                .git
                .commit_all(
                    &scratch,
                    "chore: first commit\n\nCreated by Yulius: the repository had no commits, and \
                     onboarding starts from its default branch.",
                    &author,
                    &email,
                )
                .await?;
            self.deps.git.push(&scratch, branch, branch, token).await
        }
        .await;
        remove_tree(scratch).await;
        pushed
    }

    /// The default branch GitHub reports for `owner/name` to the App's installation, else
    /// [`FALLBACK_BRANCH`]; never what an empty clone's `HEAD` names (EMP-001).
    ///
    /// # Errors
    /// GitHub failures other than the repository being out of the installation's reach.
    async fn reported_default_branch(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
    ) -> AppResult<String> {
        let reported = match self.deps.github.repo_access(token, owner, name).await? {
            RepoAccess::Access { repo, .. } => Some(repo.default_branch),
            RepoAccess::NoAccess => None,
        };
        Ok(reported
            .filter(|branch| self.deps.git.valid_branch(branch))
            .unwrap_or_else(|| FALLBACK_BRANCH.to_owned()))
    }
}
