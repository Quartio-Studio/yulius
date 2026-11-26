//! GitHub's side of a repository for the fake GitHub: a bare repository where merges land as
//! GitHub lands them.

use std::path::PathBuf;

use super::{FakeRemote, git};

/// A bare repository behind [`super::FakeGitHub`]: `merge` lands the tree of merging the head into
/// the base, as a merge commit (`merge`) or one commit on the base (`squash`, `rebase`).
pub struct BareRemote(pub PathBuf);

impl BareRemote {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        git(&self.0, args).map_err(|err| err.to_string())
    }
}

impl FakeRemote for BareRemote {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        let reference = format!("refs/heads/{branch}");
        self.run(&["rev-parse", "--verify", "--quiet", &reference])
            .ok()
    }

    fn merge(&self, base: &str, head: &str, method: &str, message: &str) -> Result<String, String> {
        let base_sha = self.branch_sha(base).ok_or("base branch not found")?;
        let tree = self.run(&["merge-tree", "--write-tree", &base_sha, head])?;
        let mut args = vec!["commit-tree", tree.as_str(), "-p", base_sha.as_str()];
        if method == "merge" {
            args.extend(["-p", head]);
        }
        args.extend(["-m", message]);
        let commit = self.run(&args)?;
        self.run(&[
            "update-ref",
            &format!("refs/heads/{base}"),
            &commit,
            &base_sha,
        ])?;
        Ok(commit)
    }

    fn create_branch(&self, branch: &str, sha: &str) -> Result<(), String> {
        self.run(&["update-ref", &format!("refs/heads/{branch}"), sha, ""])
            .map(drop)
    }

    fn fast_forward(&self, branch: &str, sha: &str) -> Result<(), String> {
        let reference = format!("refs/heads/{branch}");
        let mut args = vec!["update-ref", reference.as_str(), sha];
        let current = self.branch_sha(branch);
        if let Some(current) = &current {
            if self.contains(current, sha) != Some(true) {
                return Err("not a fast-forward".into());
            }
            args.push(current);
        }
        self.run(&args).map(drop)
    }

    fn delete_branch(&self, branch: &str) -> Result<bool, String> {
        if self.branch_sha(branch).is_none() {
            return Ok(false);
        }
        self.run(&["update-ref", "-d", &format!("refs/heads/{branch}")])?;
        Ok(true)
    }

    fn contains(&self, ancestor: &str, descendant: &str) -> Option<bool> {
        let status = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .ok()?;
        status.code().map(|code| code == 0)
    }
}
