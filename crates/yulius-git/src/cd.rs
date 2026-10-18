//! Git operations for CD (deployment, SPEC-134): a mirror of the project that only Yulius writes,
//! filled from the sandbox clone with the exact commit GitHub reports and checked object by
//! object, and a plain export of one commit's tree for a run.
//!
//! The sandbox clone is writable by the project's agents: they could plant `refs/replace`
//! entries, rewrite objects or move tracking refs. A fetch into the mirror goes through
//! `upload-pack`/`index-pack` with `transfer.fsckObjects`, so every object is re-hashed and a
//! rewritten object leaves the commit incomplete (refused); replacement refs are never fetched and
//! never honoured (`GIT_NO_REPLACE_OBJECTS`).

use std::path::Path;

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;
use crate::validate::{
    require_absolute, validate_branch_name, validate_remote_url, validate_revision,
};

/// Whether `sha` is a full hexadecimal object id.
fn full_sha(sha: &str) -> bool {
    (sha.len() == 40 || sha.len() == 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Git {
    /// Fetches `refs/remotes/origin/<branch>` of `from` (the sandbox clone) into the bare
    /// `mirror` (created when missing) as `refs/heads/<branch>`, checking every object, and
    /// requires it to be `sha` (the head GitHub reports).
    ///
    /// # Errors
    /// Invalid input, a ref that is not `sha`, objects that do not check out, or Git failures.
    pub fn cd_mirror_fetch(
        &self,
        mirror: &Path,
        from: &Path,
        branch: &str,
        sha: &str,
    ) -> Result<(), GitError> {
        require_absolute("CD mirror", mirror)?;
        require_absolute("sandbox clone", from)?;
        validate_branch_name(branch)?;
        if !full_sha(sha) {
            return Err(GitError::InvalidInput {
                what: "commit",
                reason: "a full object id is required".into(),
            });
        }
        self.init_mirror(mirror)?;
        let refspec = format!("+refs/remotes/origin/{branch}:refs/heads/{branch}");
        self.run_ok(
            Invocation::new(self, "fetch", Some(mirror))
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .args([
                    "-c",
                    "transfer.fsckObjects=true",
                    "-c",
                    "fetch.fsckObjects=true",
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--",
                ])
                .arg(from)
                .arg(&refspec),
        )?;
        let fetched = self.rev_parse(mirror, &format!("refs/heads/{branch}"))?;
        if !fetched.eq_ignore_ascii_case(sha) {
            return Err(GitError::InvalidInput {
                what: "CD source",
                reason: format!(
                    "the sandbox clone's {branch} is {fetched}, not {sha} as GitHub reports"
                ),
            });
        }
        Ok(())
    }

    /// Fetches `branch`'s head from `source` (a remote URL, or the absolute path of a repository)
    /// into the bare `mirror` (created when missing) as `refs/heads/<branch>`, one commit deep,
    /// checking every object and ignoring replacement refs, and returns it. Nothing is read from
    /// a sandbox clone: the only configuration in play is the mirror's own, which only Yulius
    /// writes (delivery environment runs, SPEC-219).
    ///
    /// # Errors
    /// Invalid input, a branch `source` lacks, or Git failures.
    pub fn mirror_fetch_head(
        &self,
        mirror: &Path,
        source: &str,
        branch: &str,
    ) -> Result<String, GitError> {
        require_absolute("environment mirror", mirror)?;
        validate_branch_name(branch)?;
        if source.starts_with('/') {
            require_absolute("source repository", Path::new(source))?;
        } else if source.contains("://") {
            validate_remote_url(source)?;
        } else {
            return Err(GitError::InvalidInput {
                what: "source",
                reason: "a URL or an absolute path is required".into(),
            });
        }
        self.init_mirror(mirror)?;
        let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
        self.run_ok(
            Invocation::new(self, "fetch", Some(mirror))
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .args([
                    "-c",
                    "transfer.fsckObjects=true",
                    "-c",
                    "fetch.fsckObjects=true",
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--depth=1",
                    "--",
                    source,
                    &refspec,
                ])
                .timeout(self.network_timeout),
        )?;
        self.rev_parse(mirror, &format!("refs/heads/{branch}"))
    }

    /// Creates the bare `mirror` (and its parent) when it does not exist yet.
    fn init_mirror(&self, mirror: &Path) -> Result<(), GitError> {
        if mirror.join("HEAD").exists() {
            return Ok(());
        }
        if let Some(parent) = mirror.parent() {
            std::fs::create_dir_all(parent).map_err(|source| GitError::Io {
                context: "creating the mirror's directory",
                source,
            })?;
        }
        self.init(mirror, "main", true)
    }

    /// Writes the tree of commit `sha` of `mirror` as plain files into the new directory `dest`
    /// (no `.git`), ignoring replacement refs.
    ///
    /// # Errors
    /// `dest` exists, invalid input, or Git failures.
    pub fn export_tree(&self, mirror: &Path, sha: &str, dest: &Path) -> Result<(), GitError> {
        require_absolute("export directory", dest)?;
        validate_revision(sha)?;
        if dest.exists() {
            return Err(GitError::InvalidInput {
                what: "export directory",
                reason: "already exists".into(),
            });
        }
        std::fs::create_dir_all(dest).map_err(|source| GitError::Io {
            context: "creating the export directory",
            source,
        })?;
        let index = dest.with_extension("index");
        let index_text = index.to_string_lossy().into_owned();
        let tree = format!("{sha}^{{tree}}");
        let read = self.run_ok(
            Invocation::new(self, "read-tree", Some(mirror))
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .env("GIT_INDEX_FILE", index_text.clone())
                .args(["read-tree", "--end-of-options"])
                .arg(&tree),
        );
        let checkout = read.and_then(|_| {
            self.run_ok(
                Invocation::new(self, "checkout-index", Some(mirror))
                    .env("GIT_NO_REPLACE_OBJECTS", "1")
                    .env("GIT_INDEX_FILE", index_text)
                    .arg("--work-tree")
                    .arg(dest)
                    .args(["checkout-index", "--all", "--force"]),
            )
        });
        let _ = std::fs::remove_file(&index);
        checkout.map(|_| ())
    }
}
