//! Repository-level operations: init, clone, fetch, remotes, revisions and branches.

use std::path::Path;

use crate::Git;
use crate::error::GitError;
use crate::runner::{Invocation, failure};
use crate::validate::{
    require_absolute, validate_branch_name, validate_remote, validate_remote_name,
    validate_remote_url, validate_revision,
};

impl Git {
    /// Creates a repository at the absolute `path` whose first branch is `initial_branch`.
    pub fn init(&self, path: &Path, initial_branch: &str, bare: bool) -> Result<(), GitError> {
        require_absolute("repository path", path)?;
        validate_branch_name(initial_branch)?;
        let mut invocation = Invocation::new(self, "init", None)
            .arg("init")
            .arg("--quiet")
            .arg(format!("--initial-branch={initial_branch}"));
        if bare {
            invocation = invocation.arg("--bare");
        }
        self.run_ok(invocation.arg("--").arg(path))?;
        Ok(())
    }

    /// Adds a remote `name` pointing at `url` (https/http/ssh/git/file URL or absolute path).
    pub fn remote_add(&self, repo: &Path, name: &str, url: &str) -> Result<(), GitError> {
        validate_remote_name(name)?;
        validate_remote_url(url)?;
        self.run_ok(
            Invocation::new(self, "remote add", Some(repo))
                .args(["remote", "add", "--", name, url]),
        )?;
        Ok(())
    }

    /// Clones `url` into the absolute `dest` (network timeout applies).
    pub fn clone(&self, url: &str, dest: &Path) -> Result<(), GitError> {
        validate_remote_url(url)?;
        require_absolute("clone destination", dest)?;
        self.run_ok(
            Invocation::new(self, "clone", None)
                .args(["clone", "--quiet", "--no-recurse-submodules", "--", url])
                .arg(dest)
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// Fetches `remote` (a configured name or URL) with pruning (network timeout applies).
    pub fn fetch(&self, repo: &Path, remote: &str) -> Result<(), GitError> {
        validate_remote(remote)?;
        self.run_ok(
            Invocation::new(self, "fetch", Some(repo))
                .args([
                    "fetch",
                    "--quiet",
                    "--prune",
                    "--no-recurse-submodules",
                    "--",
                    remote,
                ])
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// Fetches every branch of the repository at the absolute `source` into `repo`'s
    /// `refs/remotes/origin/*`, pruning the ones gone: `origin` by its known path, never by the
    /// URL in `repo`'s configuration, which a sandbox may rewrite (a playground, SPEC-099).
    pub fn fetch_branches_from(&self, repo: &Path, source: &Path) -> Result<(), GitError> {
        require_absolute("source repository", source)?;
        self.run_ok(
            Invocation::new(self, "fetch", Some(repo))
                .args([
                    "fetch",
                    "--quiet",
                    "--prune",
                    "--no-recurse-submodules",
                    "--no-tags",
                    "--",
                ])
                .arg(source)
                .arg("+refs/heads/*:refs/remotes/origin/*")
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// Copies every branch and tag of the repository at the absolute `source` into `repo` under
    /// the same names (a playground's new `origin` from an uploaded `.git`, SPEC-099). Only refs
    /// and the objects they reach travel; `source`'s configuration and hooks are never read for
    /// anything but the transfer, which runs with the fixed hardening.
    pub fn copy_refs(&self, repo: &Path, source: &Path) -> Result<(), GitError> {
        require_absolute("source repository", source)?;
        self.run_ok(
            Invocation::new(self, "fetch", Some(repo))
                .args([
                    "fetch",
                    "--quiet",
                    "--no-recurse-submodules",
                    "--no-tags",
                    // A shallow history stays shallow instead of being refused.
                    "--update-shallow",
                    "--",
                ])
                .arg(source)
                .args(["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"])
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// The remote default branch (`refs/remotes/origin/HEAD`), falling back to the branch
    /// `HEAD` points at (correct for a fresh clone).
    pub fn default_branch(&self, repo: &Path) -> Result<String, GitError> {
        let remote_head = self.run(Invocation::new(self, "symbolic-ref", Some(repo)).args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ]))?;
        if remote_head.status.success() {
            let name = remote_head.stdout_trimmed();
            if let Some(branch) = name.strip_prefix("origin/") {
                return Ok(branch.to_owned());
            }
        }
        let head = self.run_ok(Invocation::new(self, "symbolic-ref", Some(repo)).args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ]))?;
        let branch = head.stdout_trimmed();
        if branch.is_empty() {
            return Err(GitError::Parse {
                command: "symbolic-ref",
                detail: "empty branch name".to_owned(),
            });
        }
        Ok(branch)
    }

    /// Resolves `rev` to a full commit id (`rev-parse --verify --end-of-options <rev>^{commit}`).
    pub fn rev_parse(&self, repo: &Path, rev: &str) -> Result<String, GitError> {
        validate_revision(rev)?;
        let output = self.run(
            Invocation::new(self, "rev-parse", Some(repo))
                .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
                .arg(format!("{rev}^{{commit}}")),
        )?;
        if !output.status.success() {
            if output.status.code() == Some(1) {
                return Err(GitError::RevisionNotFound(rev.to_owned()));
            }
            return Err(failure("rev-parse", &output));
        }
        let sha = output.stdout_trimmed();
        if !is_object_id(&sha) {
            return Err(GitError::Parse {
                command: "rev-parse",
                detail: format!("not an object id: {sha:?}"),
            });
        }
        Ok(sha)
    }

    /// Whether `refs/heads/<name>` exists.
    pub fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, GitError> {
        validate_branch_name(name)?;
        let output = self.run(
            Invocation::new(self, "show-ref", Some(repo))
                .args(["show-ref", "--verify", "--quiet"])
                .arg(format!("refs/heads/{name}")),
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failure("show-ref", &output)),
        }
    }

    /// Atomically creates `refs/heads/<name>` at `start` (resolved to a commit first);
    /// fails with [`GitError::BranchExists`] when it already exists. Returns the commit id.
    pub fn create_branch(&self, repo: &Path, name: &str, start: &str) -> Result<String, GitError> {
        validate_branch_name(name)?;
        let sha = self.rev_parse(repo, start)?;
        let output = self.run(
            Invocation::new(self, "update-ref", Some(repo))
                .args([
                    "update-ref",
                    "--create-reflog",
                    "-m",
                    "yulius: create branch",
                ])
                .arg(format!("refs/heads/{name}"))
                .arg(&sha)
                .arg(""),
        )?;
        if output.status.success() {
            return Ok(sha);
        }
        if self.branch_exists(repo, name)? {
            return Err(GitError::BranchExists(name.to_owned()));
        }
        Err(failure("update-ref", &output))
    }
}

/// A full SHA-1 or SHA-256 object id in lowercase hex.
pub(crate) fn is_object_id(text: &str) -> bool {
    (text.len() == 40 || text.len() == 64)
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
