//! Commits, pushes and worktree-scoped identity.

use std::path::Path;

use crate::Git;
use crate::error::GitError;
use crate::runner::{Invocation, failure};
use crate::validate::{
    validate_branch_name, validate_identity_part, validate_push_refspec, validate_remote,
    validate_revision,
};

/// Longest accepted commit message in bytes.
pub const MAX_COMMIT_MESSAGE: usize = 64 * 1024;

/// Author and committer identity for [`Git::commit_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// Display name.
    pub name: String,
    /// Email address.
    pub email: String,
}

impl Author {
    /// Creates an identity.
    #[must_use]
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
        }
    }

    fn validate(&self) -> Result<(), GitError> {
        validate_identity_part("author name", &self.name)?;
        validate_identity_part("author email", &self.email)?;
        if !self.email.contains('@') {
            return Err(GitError::InvalidInput {
                what: "author email",
                reason: "missing '@'".to_owned(),
            });
        }
        Ok(())
    }
}

impl Git {
    /// Stages everything (`git add --all`) and commits it as `author` (passed through
    /// `GIT_AUTHOR_*`/`GIT_COMMITTER_*`, never `-c`). Returns `None` when there was nothing
    /// to commit, otherwise the new commit id. Repository hooks run normally.
    pub fn commit_all(
        &self,
        worktree: &Path,
        message: &str,
        author: &Author,
    ) -> Result<Option<String>, GitError> {
        author.validate()?;
        if message.trim().is_empty() || message.len() > MAX_COMMIT_MESSAGE || message.contains('\0')
        {
            return Err(GitError::InvalidInput {
                what: "commit message",
                reason: "empty, NUL-containing or longer than 64 KiB".to_owned(),
            });
        }
        self.run_ok(Invocation::new(self, "add", Some(worktree)).args(["add", "--all"]))?;
        let staged = self.run(Invocation::new(self, "diff", Some(worktree)).args([
            "diff",
            "--cached",
            "--quiet",
            "--no-ext-diff",
        ]))?;
        match staged.status.code() {
            Some(0) => return Ok(None),
            Some(1) => {}
            _ => return Err(failure("diff", &staged)),
        }
        self.run_ok(
            Invocation::new(self, "commit", Some(worktree))
                .args(["commit", "--quiet", "--no-edit", "-m", message])
                .env("GIT_AUTHOR_NAME", author.name.as_str())
                .env("GIT_AUTHOR_EMAIL", author.email.as_str())
                .env("GIT_COMMITTER_NAME", author.name.as_str())
                .env("GIT_COMMITTER_EMAIL", author.email.as_str()),
        )?;
        self.rev_parse(worktree, "HEAD").map(Some)
    }

    /// Keeps a task worktree's work and reverts the worktree (SPEC-002 ADR-16, "redirect"):
    /// commits any uncommitted work as `author`, creates the branch `keep` at the resulting
    /// commit (refusing when it exists, so nothing is overwritten), then resets the worktree's
    /// branch to its merge base with the first of `bases` that resolves. Returns the kept
    /// commit. Nothing is deleted: the work stays reachable from `keep`.
    pub fn keep_and_revert(
        &self,
        worktree: &Path,
        keep: &str,
        bases: &[String],
        message: &str,
        author: &Author,
    ) -> Result<String, GitError> {
        validate_branch_name(keep)?;
        let base = bases
            .iter()
            .find(|base| validate_revision(base).is_ok() && self.rev_parse(worktree, base).is_ok())
            .ok_or_else(|| GitError::InvalidInput {
                what: "revert base",
                reason: "none of the base revisions exists".to_owned(),
            })?;
        self.commit_all(worktree, message, author)?;
        let head = self.rev_parse(worktree, "HEAD")?;
        let fork = self.merge_base(worktree, base, &head)?;
        self.create_branch(worktree, keep, &head)?;
        self.run_ok(
            Invocation::new(self, "reset", Some(worktree))
                .args(["reset", "--hard", "--quiet"])
                .arg(&fork),
        )?;
        Ok(head)
    }

    /// Pushes one explicit, non-forcing refspec (`refs/heads/A:refs/heads/B` or
    /// `refs/heads/A`) to `remote`. Hooks (including the Yulius pre-push guard) run.
    pub fn push(&self, repo: &Path, remote: &str, refspec: &str) -> Result<(), GitError> {
        validate_remote(remote)?;
        validate_push_refspec(refspec)?;
        self.run_ok(
            Invocation::new(self, "push", Some(repo))
                .args(["push", "--quiet", "--", remote, refspec])
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// Sets `user.name`/`user.email` for this worktree only.
    ///
    /// Choice: worktree-scoped configuration (`extensions.worktreeConfig=true` on the
    /// repository, then `git config --worktree`), not `-c` per command, so an agent's own
    /// plain `git commit` inside the worktree uses the identity while other worktrees and
    /// the primary checkout keep theirs. Values are passed as data arguments.
    pub fn set_local_identity(
        &self,
        worktree: &Path,
        name: &str,
        email: &str,
    ) -> Result<(), GitError> {
        Author::new(name, email).validate()?;
        self.enable_worktree_config(worktree)?;
        for (key, value) in [("user.name", name), ("user.email", email)] {
            self.run_ok(Invocation::new(self, "config", Some(worktree)).args([
                "config",
                "--worktree",
                key,
                value,
            ]))?;
        }
        Ok(())
    }

    /// Enables `extensions.worktreeConfig` on the repository when it is not already set.
    pub(crate) fn enable_worktree_config(&self, worktree: &Path) -> Result<(), GitError> {
        let current = self.run(Invocation::new(self, "config", Some(worktree)).args([
            "config",
            "--bool",
            "--get",
            "extensions.worktreeConfig",
        ]))?;
        if current.status.success() && current.stdout_trimmed() == "true" {
            return Ok(());
        }
        self.run_ok(Invocation::new(self, "config", Some(worktree)).args([
            "config",
            "--local",
            "extensions.worktreeConfig",
            "true",
        ]))?;
        Ok(())
    }
}
