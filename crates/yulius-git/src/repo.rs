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

    /// Fetches every branch and tag of the remote `url` into `repo` under the same names, forced,
    /// without submodules (a reference copy's fetch, SPEC-330), while `repo` holds at most `limit`
    /// bytes (SPEC-349).
    ///
    /// # Errors
    /// [`GitError::TooLarge`] once `repo` holds more than `limit` bytes (Git is killed); an invalid
    /// URL, a timeout or a failed fetch.
    pub fn fetch_branches_and_tags(
        &self,
        repo: &Path,
        url: &str,
        limit: u64,
    ) -> Result<(), GitError> {
        validate_remote_url(url)?;
        self.run_ok(
            Invocation::new(self, "fetch", Some(repo))
                .args([
                    "fetch",
                    "--quiet",
                    "--no-recurse-submodules",
                    "--no-tags",
                    "--",
                    url,
                    "+refs/heads/*:refs/heads/*",
                    "+refs/tags/*:refs/tags/*",
                ])
                .timeout(self.network_timeout)
                .watch_size(&[repo], limit),
        )?;
        Ok(())
    }

    /// Every branch of `repo` as `(name, commit)` pairs sorted by name, whatever the name (unlike
    /// [`Git::local_branches`], which skips names Yulius would not create): what a bundle of
    /// `--branches` carries (SPEC-330).
    pub fn every_branch(&self, repo: &Path) -> Result<Vec<(String, String)>, GitError> {
        self.refs_under(repo, "refs/heads/")
    }

    /// Every tag of `repo` as `(name, object id)` pairs sorted by name (SPEC-330).
    pub fn every_tag(&self, repo: &Path) -> Result<Vec<(String, String)>, GitError> {
        self.refs_under(repo, "refs/tags/")
    }

    fn refs_under(&self, repo: &Path, prefix: &str) -> Result<Vec<(String, String)>, GitError> {
        let output = self.run_ok(
            Invocation::new(self, "for-each-ref", Some(repo))
                .args(["for-each-ref", "--format=%(objectname)%00%(refname)"])
                .arg(prefix),
        )?;
        let mut refs: Vec<(String, String)> = output
            .stdout_text()
            .lines()
            .filter_map(|line| {
                let (sha, name) = line.split_once('\0')?;
                Some((name.strip_prefix(prefix)?.to_owned(), sha.to_owned()))
            })
            .collect();
        refs.sort();
        Ok(refs)
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

    /// Pushes every branch and tag of `repo` to `url` under the same names in one atomic push
    /// that never forces: the remote takes all of them or none (a playground moved to GitHub,
    /// SPEC-176). With `first`, that branch goes alone before (non-forcing too): GitHub makes the
    /// first branch pushed into an empty repository its default branch. The network timeout
    /// applies to each push.
    pub fn push_all_refs(
        &self,
        repo: &Path,
        url: &str,
        first: Option<&str>,
    ) -> Result<(), GitError> {
        validate_remote_url(url)?;
        if let Some(branch) = first {
            validate_branch_name(branch)?;
            let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
            self.run_ok(
                Invocation::new(self, "push", Some(repo))
                    .args(["push", "--quiet", "--", url, &refspec])
                    .timeout(self.network_timeout),
            )?;
        }
        self.run_ok(
            Invocation::new(self, "push", Some(repo))
                .args(["push", "--quiet", "--atomic", "--", url])
                .args(["refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*"])
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// The branches and tags of the repository at `url`, as it answers now (`git ls-remote`):
    /// `(ref name, commit)` pairs sorted by name, peeled tags left out. The network timeout
    /// applies.
    pub fn remote_refs(&self, repo: &Path, url: &str) -> Result<Vec<(String, String)>, GitError> {
        validate_remote_url(url)?;
        let listed = self.run_ok(
            Invocation::new(self, "ls-remote", Some(repo))
                .args(["ls-remote", "--refs", "--heads", "--tags", "--", url])
                .timeout(self.network_timeout),
        )?;
        let mut refs: Vec<(String, String)> = listed
            .stdout_trimmed()
            .lines()
            .filter_map(|line| {
                let (sha, name) = line.split_once('\t')?;
                Some((name.to_owned(), sha.to_owned()))
            })
            .collect();
        refs.sort();
        Ok(refs)
    }

    /// Removes from `repo`'s own configuration (`.git/config` and `.git/config.worktree`) every
    /// setting that steers transport, runs a command during a fetch or loads more configuration:
    /// `http.*`, `url.*`, `include*`, `credential.*`, SSH, proxy and alternate-refs commands,
    /// submodule recursion (`submodule.recurse`, `fetch.` and `push.recurseSubmodules`) and
    /// `origin`'s proxy and push or pack overrides. Every other setting stays. A `.git` that is not
    /// a directory, or a configuration file that is a link, is refused before any write: Git would
    /// read or write configuration elsewhere (SPEC-176).
    pub fn scrub_transport_config(&self, repo: &Path) -> Result<(), GitError> {
        // Whole keys: a prefix would also take `submodule.<name>.url` of a submodule named so.
        const KEYS: &[&str] = &["submodule.recurse"];
        const PREFIXES: &[&str] = &[
            "http.",
            "https.",
            "url.",
            "include.",
            "includeif.",
            "credential.",
            "core.sshcommand",
            "core.gitproxy",
            "core.askpass",
            "core.alternaterefscommand",
            "fetch.recursesubmodules",
            "push.recursesubmodules",
            "remote.origin.proxy",
            "remote.origin.pushurl",
            "remote.origin.uploadpack",
            "remote.origin.receivepack",
        ];
        // Yulius makes every clone with a real `.git` directory; a pointer file or a link would
        // send the scrub to configuration elsewhere while Git still reads another.
        let git_dir = repo.join(".git");
        if !std::fs::symlink_metadata(&git_dir).is_ok_and(|meta| meta.file_type().is_dir()) {
            return Err(GitError::InvalidInput {
                what: "repository",
                reason: "its .git is not a directory".to_owned(),
            });
        }
        let worktree_config = git_dir.join("config.worktree");
        for file in [git_dir.join("config"), worktree_config.clone()] {
            if std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(GitError::InvalidInput {
                    what: "repository configuration",
                    reason: "a configuration file is a link".to_owned(),
                });
            }
        }
        let mut scopes = vec![vec!["--local".to_owned()]];
        if worktree_config.is_file() {
            scopes.push(vec![
                "--file".to_owned(),
                worktree_config.to_string_lossy().into_owned(),
            ]);
        }
        for scope in scopes {
            let listed = self.run_ok(
                Invocation::new(self, "config", Some(repo))
                    .arg("config")
                    .args(scope.iter().map(String::as_str))
                    .args(["--list", "--name-only"]),
            )?;
            let mut names: Vec<String> = listed
                .stdout_trimmed()
                .lines()
                .filter(|name| {
                    let lower = name.to_ascii_lowercase();
                    KEYS.contains(&lower.as_str())
                        || PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
                })
                .map(str::to_owned)
                .collect();
            names.sort();
            names.dedup();
            for name in names {
                self.run_ok(
                    Invocation::new(self, "config", Some(repo))
                        .arg("config")
                        .args(scope.iter().map(String::as_str))
                        .args(["--unset-all", &name]),
                )?;
            }
        }
        Ok(())
    }

    /// Points the remote `name` of `repo` at `url`.
    pub fn set_remote_url(&self, repo: &Path, name: &str, url: &str) -> Result<(), GitError> {
        validate_remote_name(name)?;
        validate_remote_url(url)?;
        self.run_ok(
            Invocation::new(self, "remote set-url", Some(repo))
                .args(["remote", "set-url", "--", name, url]),
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
