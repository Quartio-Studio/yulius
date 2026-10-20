//! An empty GitHub repository (no commits, no branch yet) gets its first commit when its
//! onboarding starts (SPEC-241 EMP-002, ADR-0187): `README.md` on the default branch GitHub
//! reports (`main` when it reports none), pushed with the installation token as a created
//! project's scaffold is. A task's worktree needs a branch to start from; an empty repository
//! has none.

use std::path::Path;

use crate::context::App;
use crate::error::AppResult;
use crate::model::SandboxRecord;
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

    /// Gives the sandbox's repository its first commit when GitHub holds none, authored by
    /// `actor` (a login; else the sandbox's creator), and records the branch it went to.
    /// Returns whether it pushed. A repository with any branch is left as it is (EMP-003).
    ///
    /// # Errors
    /// Fetch, commit or push failures (GitHub refused the push, someone pushed first); store
    /// failures.
    pub(crate) async fn seed_empty_repository(
        &self,
        record: &SandboxRecord,
        actor: &str,
    ) -> AppResult<bool> {
        if record.is_playground() {
            return Ok(false);
        }
        let (owner, name) = (&record.repo_owner, &record.repo_name);
        let url = clone_url(owner, name);
        let repo = self.sandbox_repo(&record.id);
        // A clone with branches had them from GitHub: nothing to ask GitHub then.
        if !self.remote_is_empty(&repo).await {
            return Ok(false);
        }
        let token = self.installation_token().await?;
        // What GitHub holds now: someone may have pushed since the repository was added.
        self.deps.git.sync_clone(&url, &repo, &token).await?;
        if !self.remote_is_empty(&repo).await {
            return Ok(false);
        }
        let branch = self.reported_default_branch(owner, name).await;
        let store = &self.deps.store;
        let author = match store.user_by_login(actor).await? {
            Some(user) => Some(user),
            None => store.user(&record.created_by).await?,
        };
        let (author, email) = commit_identity(author.as_ref());
        let scratch = self.sandbox_dir(&record.id).join("seed");
        remove_tree(scratch.clone()).await;
        let pushed = async {
            self.deps.git.sync_clone(&url, &scratch, &token).await?;
            let files = vec![("README.md".to_owned(), format!("# {name}\n"))];
            let (root, first) = (scratch.clone(), branch.clone());
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
            self.deps.git.push(&scratch, &branch, &branch, &token).await
        }
        .await;
        remove_tree(scratch).await;
        pushed?;
        self.deps.git.sync_clone(&url, &repo, &token).await?;
        if record.default_branch != branch {
            store.set_default_branch(&record.id, &branch).await?;
        }
        store
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

    /// The default branch GitHub reports for `owner/name` through the App's installation, else
    /// [`FALLBACK_BRANCH`]; never what an empty clone's `HEAD` names (EMP-001).
    async fn reported_default_branch(&self, owner: &str, name: &str) -> String {
        let reported = match self.installation_repo(owner, name).await {
            Ok(repo) => repo.map(|repo| repo.default_branch),
            Err(err) => {
                tracing::warn!(repo = %format!("{owner}/{name}"), error = %err, "reading GitHub's default branch failed");
                None
            }
        };
        reported
            .filter(|branch| !branch.is_empty() && self.deps.git.valid_branch(branch))
            .unwrap_or_else(|| FALLBACK_BRANCH.to_owned())
    }
}
