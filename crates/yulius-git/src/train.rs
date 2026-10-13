//! Merge trains (SPEC-084): the integration branch as it will be after several pull requests
//! merge one after another, built in a scratch worktree exactly as Yulius brings each one up to
//! date (SPEC-050 DLV-203) and as the host then squashes or merges it.

use std::path::Path;

use crate::Git;
use crate::commit::{Author, MergeInto};
use crate::error::GitError;
use crate::runner::Invocation;
use crate::validate::validate_revision;

/// How a merge method lands a pull request on the integration branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainLanding {
    /// One new commit with the head's tree and the integration branch as its only parent.
    Squash,
    /// A merge commit of the integration branch and the head.
    MergeCommit,
}

/// What one pull request of a train did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainStep {
    /// It merged cleanly: the integration branch's tree after it lands.
    Clean {
        /// Tree of the head brought up to date (and of the integration branch after it lands).
        tree: String,
    },
    /// Bringing it up to date conflicts in these paths: it is left out and the next one is
    /// merged onto the train as it was.
    Conflict(Vec<String>),
}

/// A built train.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainBuild {
    /// The integration branch's commit it starts from.
    pub base: String,
    /// That commit's tree.
    pub base_tree: String,
    /// One step per head, in order.
    pub steps: Vec<TrainStep>,
    /// The integration branch after every clean step landed (a local commit; `base` when none
    /// was clean).
    pub head: String,
}

/// What a train is built from.
#[derive(Debug, Clone, Copy)]
pub struct TrainInput<'a> {
    /// The integration branch (a revision).
    pub base: &'a str,
    /// The pull request heads, in merge order.
    pub heads: &'a [String],
    /// How each lands on the integration branch.
    pub landing: TrainLanding,
    /// Who makes the train's commits.
    pub author: &'a Author,
}

impl Git {
    /// Builds a merge train in the scratch worktree `scratch` (inside `owned_root`; created on
    /// first use as a detached worktree of `repo`, reused afterwards like a CI checkout).
    ///
    /// Starting from `base` (the integration branch), for each of `heads` in order: the head is
    /// checked out and the integration branch so far is merged into it with `git merge`, as
    /// Yulius brings an armed pull request up to date (the head's `.gitattributes` and the
    /// repository's merge drivers apply, `merge=union` included); a conflict is aborted and that
    /// head is left out. A clean result then lands as `landing` makes it land on the host: a new
    /// commit with its tree on top of the integration branch so far. Commits are made as
    /// `author`; nothing is pushed. The worktree is left at the train's last commit, which keeps
    /// it reachable while Yulius CI tests it.
    ///
    /// # Errors
    /// Invalid input, unknown revisions, paths outside `owned_root`, or Git failures.
    pub fn build_train(
        &self,
        repo: &Path,
        scratch: &Path,
        owned_root: &Path,
        input: &TrainInput<'_>,
    ) -> Result<TrainBuild, GitError> {
        let TrainInput {
            base,
            heads,
            landing,
            author,
        } = *input;
        author.validate()?;
        validate_revision(base)?;
        let base = self.rev_parse(repo, base)?;
        let base_tree = self
            .tree_of(repo, &base)?
            .ok_or_else(|| GitError::RevisionNotFound(base.clone()))?;
        let mut current = base.clone();
        let mut steps = Vec::with_capacity(heads.len());
        for head in heads {
            validate_revision(head)?;
            let head = self.rev_parse(repo, head)?;
            self.ci_checkout(repo, scratch, &head, owned_root)?;
            let merged = match self.merge_as(scratch, &current, Some(author))? {
                MergeInto::UpToDate => head.clone(),
                MergeInto::Merged(commit) => commit,
                MergeInto::Conflict(files) => {
                    steps.push(TrainStep::Conflict(files));
                    continue;
                }
            };
            let tree = self
                .tree_of(scratch, &merged)?
                .ok_or_else(|| GitError::RevisionNotFound(merged.clone()))?;
            current = self.land(scratch, &tree, &current, &merged, landing, author)?;
            steps.push(TrainStep::Clean { tree });
        }
        if current != base {
            self.ci_checkout(repo, scratch, &current, owned_root)?;
        }
        Ok(TrainBuild {
            base,
            base_tree,
            steps,
            head: current,
        })
    }

    /// The commit a host makes when it lands `merged` (whose tree is `tree`) on `onto`.
    fn land(
        &self,
        dir: &Path,
        tree: &str,
        onto: &str,
        merged: &str,
        landing: TrainLanding,
        author: &Author,
    ) -> Result<String, GitError> {
        let mut invocation = Invocation::new(self, "commit-tree", Some(dir))
            .args(["commit-tree", "-m", "Yulius merge train"])
            .arg("-p")
            .arg(onto);
        if landing == TrainLanding::MergeCommit {
            invocation = invocation.arg("-p").arg(merged);
        }
        let output = self.run_ok(
            invocation
                .arg(tree)
                .env("GIT_AUTHOR_NAME", author.name.as_str())
                .env("GIT_AUTHOR_EMAIL", author.email.as_str())
                .env("GIT_COMMITTER_NAME", author.name.as_str())
                .env("GIT_COMMITTER_EMAIL", author.email.as_str()),
        )?;
        let commit = output.stdout_trimmed();
        if crate::repo::is_object_id(&commit) {
            Ok(commit)
        } else {
            Err(GitError::Parse {
                command: "commit-tree",
                detail: crate::error::truncate_for_error(&commit),
            })
        }
    }
}
