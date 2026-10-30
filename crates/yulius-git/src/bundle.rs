//! Git bundles for `yulius import`: create, verify and fetch into `refs/heads/import/...`.

use std::path::Path;

use crate::Git;
use crate::error::GitError;
use crate::repo::is_object_id;
use crate::runner::Invocation;
use crate::validate::{require_absolute, validate_branch_name, validate_full_ref};

/// Branch prefix that every imported ref lands under.
pub const IMPORT_PREFIX: &str = "import/";

/// One rev-list argument of [`Git::create_bundle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleSpec {
    /// Include a full ref name (`refs/heads/...` or `refs/tags/...`).
    Include(String),
    /// Exclude history reachable from a revision (resolved to a commit, passed as `^<id>`);
    /// the receiver must already have it (a bundle prerequisite).
    Exclude(String),
}

/// A ref advertised by a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleHead {
    /// Full ref name inside the bundle.
    pub refname: String,
    /// Commit id.
    pub sha: String,
}

/// Mapping of a bundle ref to a local import branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRef {
    /// Full ref name inside the bundle (`refs/heads/...` or `refs/tags/...`).
    pub source: String,
    /// Local branch name; must start with `import/`.
    pub target_branch: String,
}

impl Git {
    /// Writes a bundle at the absolute `out_path` containing the included refs minus the
    /// excluded history.
    pub fn create_bundle(
        &self,
        repo: &Path,
        out_path: &Path,
        refs_and_exclusions: &[BundleSpec],
    ) -> Result<(), GitError> {
        require_absolute("bundle path", out_path)?;
        let mut revisions = Vec::with_capacity(refs_and_exclusions.len());
        for spec in refs_and_exclusions {
            match spec {
                BundleSpec::Include(name) => {
                    validate_full_ref(name)?;
                    revisions.push(name.clone());
                }
                BundleSpec::Exclude(rev) => {
                    revisions.push(format!("^{}", self.rev_parse(repo, rev)?));
                }
            }
        }
        if !refs_and_exclusions
            .iter()
            .any(|spec| matches!(spec, BundleSpec::Include(_)))
        {
            return Err(GitError::InvalidInput {
                what: "bundle refs",
                reason: "at least one ref must be included".to_owned(),
            });
        }
        self.run_ok(
            Invocation::new(self, "bundle create", Some(repo))
                .args(["bundle", "create", "--quiet"])
                .arg(out_path)
                .args(&revisions),
        )?;
        Ok(())
    }

    /// Writes a bundle at the absolute `out_path` of `HEAD`, every branch and every tag of
    /// `repo`: a whole repository's copy (a task's reference copy, SPEC-330), of at most `limit`
    /// bytes (SPEC-349).
    ///
    /// # Errors
    /// [`GitError::TooLarge`] once the bundle holds more than `limit` bytes (Git is killed); a
    /// relative path or a failed bundle.
    pub fn create_full_bundle(
        &self,
        repo: &Path,
        out_path: &Path,
        limit: u64,
    ) -> Result<(), GitError> {
        require_absolute("bundle path", out_path)?;
        // Git writes the bundle into `<path>.lock` and renames it once it is complete.
        let mut lock = out_path.as_os_str().to_owned();
        lock.push(".lock");
        let lock = std::path::PathBuf::from(lock);
        self.run_ok(
            Invocation::new(self, "bundle create", Some(repo))
                .args(["bundle", "create", "--quiet"])
                .arg(out_path)
                .args(["HEAD", "--branches", "--tags"])
                .watch_size(&[out_path, &lock], limit),
        )?;
        Ok(())
    }

    /// Verifies the bundle against `repo` (its prerequisites must exist there) and returns
    /// the refs it carries.
    pub fn verify_bundle(&self, repo: &Path, bundle: &Path) -> Result<Vec<BundleHead>, GitError> {
        require_absolute("bundle path", bundle)?;
        self.run_ok(
            Invocation::new(self, "bundle verify", Some(repo))
                .args(["bundle", "verify", "--quiet"])
                .arg(bundle),
        )?;
        let output = self.run_ok(
            Invocation::new(self, "bundle list-heads", Some(repo))
                .args(["bundle", "list-heads"])
                .arg(bundle),
        )?;
        let mut heads = Vec::new();
        for line in output.stdout_text().lines() {
            let Some((sha, refname)) = line.split_once(' ') else {
                continue;
            };
            if !is_object_id(sha) {
                return Err(GitError::Parse {
                    command: "bundle list-heads",
                    detail: format!("bad object id {sha:?}"),
                });
            }
            heads.push(BundleHead {
                refname: refname.to_owned(),
                sha: sha.to_owned(),
            });
        }
        Ok(heads)
    }

    /// Fetches bundle refs into local `refs/heads/import/...` branches without forcing (an
    /// existing import branch only moves forward) and returns `(branch, commit id)` pairs.
    pub fn fetch_bundle_into(
        &self,
        repo: &Path,
        bundle: &Path,
        ref_mapping: &[ImportRef],
    ) -> Result<Vec<(String, String)>, GitError> {
        require_absolute("bundle path", bundle)?;
        if ref_mapping.is_empty() {
            return Err(GitError::InvalidInput {
                what: "bundle ref mapping",
                reason: "empty".to_owned(),
            });
        }
        let mut refspecs = Vec::with_capacity(ref_mapping.len());
        for mapping in ref_mapping {
            validate_full_ref(&mapping.source)?;
            validate_branch_name(&mapping.target_branch)?;
            if !mapping.target_branch.starts_with(IMPORT_PREFIX) {
                return Err(GitError::InvalidInput {
                    what: "import branch",
                    reason: format!("{} must start with {IMPORT_PREFIX}", mapping.target_branch),
                });
            }
            refspecs.push(format!(
                "{}:refs/heads/{}",
                mapping.source, mapping.target_branch
            ));
        }
        self.run_ok(
            Invocation::new(self, "fetch", Some(repo))
                .args([
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--no-recurse-submodules",
                    "--",
                ])
                .arg(bundle)
                .args(&refspecs)
                .timeout(self.network_timeout),
        )?;
        ref_mapping
            .iter()
            .map(|mapping| {
                let sha = self.rev_parse(repo, &format!("refs/heads/{}", mapping.target_branch))?;
                Ok((mapping.target_branch.clone(), sha))
            })
            .collect()
    }
}
