//! `reference_repo` (SPEC-267, ADR-0197): a task reads another repository of its project's
//! GitHub owner through a bundle Yulius fetches as the task's owner. Git runs on the host only in
//! a fresh folder of the server's own; the bundle reaches the sandbox through the attachment
//! path's handles (ADR-0174), and the agent clones it in its containment. The agent never holds
//! the person's token, and the App's access is not widened (ADR-0114).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::github::RepoAccess;
use crate::projects::{blocking, clone_url, valid_owner, valid_repo_name};

/// Largest bundle handed to a task (1 GiB).
pub const MAX_REFERENCE_BYTES: u64 = 1024 * 1024 * 1024;

/// `owner/name` of a `repo` argument, or `None` when it is not one.
#[must_use]
pub fn parse_repo(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.trim().split_once('/')?;
    (valid_owner(owner) && valid_repo_name(name)).then_some((owner, name))
}

/// Removes a reference fetch folder with everything in it (Yulius made it; no agent reaches it).
fn remove_scratch(dir: &Path) {
    if let Err(err) = std::fs::remove_dir_all(dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %dir.display(), error = %err, "a reference fetch folder stays");
    }
}

impl App {
    /// `reference_repo {repo}`: fetches the default branch of another repository of the
    /// sandbox's owner as the task's owner and places a bundle of it among the task's
    /// attachments (SPEC-267 REF-001..004).
    ///
    /// # Errors
    /// `Invalid` for a malformed `repo`; `Conflict` (naming `needs_you`) for another owner, the
    /// sandbox's own repository, a playground, an owner who is gone, disabled, without GitHub or
    /// without read access, and a bundle too large; Git and store failures.
    pub(crate) async fn tool_reference_repo(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let raw = args.get("repo").and_then(Value::as_str).unwrap_or_default();
        let (owner, name) = parse_repo(raw).ok_or_else(|| {
            AppError::Invalid("repo must be a GitHub repository as owner/name".into())
        })?;
        let repo = format!("{owner}/{name}");
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        crate::playground::refuse_in_playground(&sandbox, "Reading another repository")?;
        let refused = |why: String| {
            AppError::Conflict(format!(
                "{why} Only now ask a person with `needs_you`: to give the task's owner read \
                 access to {repo} on GitHub, or to put the files you need into the sandbox."
            ))
        };
        if owner.eq_ignore_ascii_case(&sandbox.repo_owner)
            && name.eq_ignore_ascii_case(&sandbox.repo_name)
        {
            return Err(AppError::Conflict(format!(
                "{repo} is this sandbox's own repository: your worktree already has it, and \
                 `git fetch` gets its other branches."
            )));
        }
        if !owner.eq_ignore_ascii_case(&sandbox.repo_owner) {
            return Err(refused(format!(
                "Yulius fetches only repositories of {}, this project's owner on GitHub. A public \
                 repository you can clone yourself.",
                sandbox.repo_owner
            )));
        }
        let person = self
            .deps
            .store
            .user(&ctx.task.owner)
            .await?
            .ok_or_else(|| refused("This task's owner no longer exists.".into()))?;
        if person.disabled_at.is_some() {
            return Err(refused(
                "This task's owner's account is disabled, so Yulius does not act for them.".into(),
            ));
        }
        let token = self
            .person_github_token(&person, "Reading another repository")
            .await
            .map_err(|err| {
                refused(format!(
                    "Yulius cannot act as this task's owner on GitHub ({err})."
                ))
            })?;
        let login = person.github_login.clone().unwrap_or_default();
        let branch = match self.deps.github.repo_access(&token, owner, name).await {
            Ok(RepoAccess::Access { permissions, repo }) if permissions.pull => repo.default_branch,
            Ok(_) => {
                return Err(refused(format!(
                    "{login}, this task's owner, cannot read {repo} on GitHub (or it does not \
                     exist)."
                )));
            }
            Err(err) => {
                return Err(refused(format!(
                    "GitHub did not say whether {login} can read {repo} ({err})."
                )));
            }
        };
        if !self.deps.git.valid_branch(&branch) {
            return Err(refused(format!(
                "GitHub names no usable default branch of {repo}."
            )));
        }
        let scratch = self.reference_scratch().await?;
        let fetched = async {
            let (work, bundle) = (scratch.join("repo.git"), scratch.join("repo.bundle"));
            let commit = self
                .deps
                .git
                .bundle_branch(&clone_url(owner, name), &branch, &work, &bundle, &token)
                .await
                .map_err(|err| refused(format!("Fetching {repo} as {login} failed ({err}).")))?;
            let size = file_size(&bundle).await?;
            if size > MAX_REFERENCE_BYTES {
                return Err(refused(format!(
                    "{repo}'s `{branch}` bundles to {size} bytes, above the \
                     {MAX_REFERENCE_BYTES} bytes a task gets."
                )));
            }
            let placed = self
                .attach_server_file(
                    (&sandbox.id, &ctx.task.id),
                    &format!("{owner}-{name}.bundle"),
                    &bundle,
                    MAX_REFERENCE_BYTES,
                    login.clone(),
                )
                .await?;
            Ok((commit, size, placed))
        }
        .await;
        let removed = scratch.clone();
        blocking(move || {
            remove_scratch(&removed);
            Ok(())
        })
        .await?;
        let (commit, size, placed) = fetched?;
        self.audit_reference(ctx, &format!("{repo}@{commit} ({branch}) as {login}"))
            .await;
        let clone_into = self.reference_folder(&sandbox, owner, name);
        Ok(json!({
            "repo": repo,
            "branch": branch,
            "commit": commit,
            "bundle": placed.path,
            "bytes": size,
            "clone_into": clone_into,
            "acted_as": login,
            "message": format!(
                "Fetched {repo} `{branch}` at {commit} as {login}. Read-only reference: clone it \
                 outside your worktree with `git clone --quiet --branch {branch} {bundle} \
                 {clone_into}`, or update an earlier copy with `git -C {clone_into} pull --ff-only \
                 {bundle} {branch}`. Never commit its files into this repository beyond what you \
                 port, and never push from it. If you asked a person for access with needs_you, \
                 call needs_you with resolved: true.",
                bundle = placed.path,
            ),
        }))
    }

    /// A fresh, empty folder below `<data>/tmp`, out of every sandbox.
    async fn reference_scratch(&self) -> AppResult<PathBuf> {
        let random = self.deps.crypto.random_bytes();
        let dir = self.config.data_dir.join("tmp").join(format!(
            "reference-{}",
            crate::auth::hex_token(random.get(..8).unwrap_or(&random))
        ));
        let made = dir.clone();
        blocking(move || {
            if let Some(parent) = made.parent() {
                std::fs::create_dir_all(parent).map_err(AppError::internal)?;
            }
            std::fs::create_dir(&made).map_err(AppError::internal)
        })
        .await?;
        Ok(dir)
    }

    /// Where the agent keeps its copy: `<sandbox>/reference/<owner>/<name>`, outside every
    /// worktree, as runs see it.
    fn reference_folder(&self, sandbox: &SandboxRecord, owner: &str, name: &str) -> String {
        let dir = self.sandbox_dir(&sandbox.id);
        std::path::absolute(&dir)
            .unwrap_or(dir)
            .join("reference")
            .join(owner)
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    async fn audit_reference(&self, ctx: &AgentContext, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &ctx.actor(),
                "task.reference_repo",
                Some(ctx.task.id.as_str()),
                Some(detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "reference_repo audit failed");
        }
    }
}

async fn file_size(path: &Path) -> AppResult<u64> {
    Ok(tokio::fs::metadata(path)
        .await
        .map_err(AppError::internal)?
        .len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_owner_and_a_name_are_a_repository() {
        assert_eq!(parse_repo("acme/lib"), Some(("acme", "lib")));
        assert_eq!(
            parse_repo(" Quartio-Studio/product "),
            Some(("Quartio-Studio", "product"))
        );
        for bad in [
            "",
            "lib",
            "acme/",
            "/lib",
            "acme/lib/x",
            "../lib",
            "acme/..",
            "acme/.git",
            "acme/lib.git",
            "-x/lib",
            "ac me/lib",
            "acme/li b",
        ] {
            assert_eq!(parse_repo(bad), None, "{bad}");
        }
    }
}
