//! [`GitPort`] over `yulius-git` (git CLI, argv only, bounded, run on blocking threads).
//!
//! HTTPS credentials for server-side clone/fetch/push are passed as an `http.extraHeader` through
//! `GIT_CONFIG_*` environment variables for that one command — never in argv or on disk.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::Secret;
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort, ReviewChanges};
use yulius_contracts::api::git::{ChangedFile, DiffHunk, DiffLine, TaskCommit};
use yulius_git::{
    Author, FileDiff, FileStatus, Git, GitError, ImportRef, LineKind, WorktreeRemoval,
};

/// Git adapter.
#[derive(Clone)]
pub struct GitAdapter {
    exe: PathBuf,
}

impl GitAdapter {
    /// Adapter using the `git` executable on PATH (or `exe`).
    #[must_use]
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self { exe: exe.into() }
    }

    fn git(&self) -> Git {
        Git::new(self.exe.clone())
    }

    fn git_with_token(&self, token: &Secret) -> Git {
        let basic = base64_encode(format!("x-access-token:{}", token.expose()).as_bytes());
        self.git()
            .with_env("GIT_CONFIG_COUNT", "1")
            .with_env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
            .with_env(
                "GIT_CONFIG_VALUE_0",
                format!("AUTHORIZATION: basic {basic}"),
            )
    }
}

fn map_err(err: GitError) -> AppError {
    match err {
        GitError::InvalidInput { .. } => AppError::Invalid(err.to_string()),
        other => AppError::Upstream(format!("git: {other}")),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, GitError> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(AppError::internal)?
        .map_err(map_err)
}

fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(
                    T[usize::try_from((n >> (18 - 6 * i)) & 63).unwrap_or(0)],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[async_trait]
impl GitPort for GitAdapter {
    fn valid_branch(&self, name: &str) -> bool {
        yulius_git::validate_branch_name(name).is_ok()
    }

    async fn sync_clone(&self, url: &str, dest: &Path, token: &Secret) -> AppResult<()> {
        let git = self.git_with_token(token);
        let (url, dest) = (url.to_owned(), dest.to_path_buf());
        blocking(move || {
            if dest.join(".git").exists() {
                git.fetch(&dest, "origin")
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|source| GitError::Io {
                        context: "creating the clone parent",
                        source,
                    })?;
                }
                git.clone(&url, &dest)
            }
        })
        .await
    }

    async fn default_branch(&self, repo: &Path) -> AppResult<String> {
        let (git, repo) = (self.git(), repo.to_path_buf());
        blocking(move || git.default_branch(&repo)).await
    }

    async fn branch_exists(&self, repo: &Path, branch: &str) -> AppResult<bool> {
        let (git, repo, branch) = (self.git(), repo.to_path_buf(), branch.to_owned());
        blocking(move || {
            Ok(git.branch_exists(&repo, &branch)?
                || git
                    .rev_parse(&repo, &format!("refs/remotes/origin/{branch}"))
                    .is_ok())
        })
        .await
    }

    async fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> AppResult<()> {
        let (git, repo, path, branch, base) = (
            self.git(),
            repo.to_path_buf(),
            path.to_path_buf(),
            branch.to_owned(),
            base.to_owned(),
        );
        blocking(move || {
            if git.branch_exists(&repo, &branch)? {
                git.worktree_add_existing(&repo, &path, &branch).map(|_| ())
            } else {
                git.worktree_add(&repo, &path, &branch, &base).map(|_| ())
            }
        })
        .await
    }

    async fn remove_worktree(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
    ) -> AppResult<bool> {
        let (git, repo, path, root) = (
            self.git(),
            repo.to_path_buf(),
            path.to_path_buf(),
            owned_root.to_path_buf(),
        );
        blocking(move || {
            Ok(matches!(
                git.worktree_remove_if_safe(&repo, &path, &root)?,
                WorktreeRemoval::Removed
            ))
        })
        .await
    }

    async fn discard_worktree(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        branch: Option<&str>,
    ) -> AppResult<()> {
        let (git, repo, path, root, branch) = (
            self.git(),
            repo.to_path_buf(),
            path.to_path_buf(),
            owned_root.to_path_buf(),
            branch.map(str::to_owned),
        );
        blocking(move || git.worktree_discard(&repo, &path, &root, branch.as_deref())).await
    }

    async fn has_changes(&self, worktree: &Path) -> AppResult<bool> {
        let (git, wt) = (self.git(), worktree.to_path_buf());
        blocking(move || git.has_changes(&wt)).await
    }

    async fn install_push_guard(&self, worktree: &Path, protected: &[String]) -> AppResult<()> {
        let (git, wt, protected) = (self.git(), worktree.to_path_buf(), protected.to_vec());
        blocking(move || git.install_pre_push_guard(&wt, &protected).map(|_| ())).await
    }

    async fn set_identity(&self, worktree: &Path, name: &str, email: &str) -> AppResult<()> {
        let (git, wt, name, email) = (
            self.git(),
            worktree.to_path_buf(),
            name.to_owned(),
            email.to_owned(),
        );
        blocking(move || git.set_local_identity(&wt, &name, &email)).await
    }

    async fn review_changes(&self, worktree: &Path, base: &str) -> AppResult<ReviewChanges> {
        let (git, worktree, base) = (self.git(), worktree.to_path_buf(), base.to_owned());
        let diff = blocking(move || git.worktree_diff(&worktree, &base)).await?;
        Ok(ReviewChanges {
            base_sha: diff.base_sha,
            head_sha: diff.head_sha,
            files: diff.files.into_iter().map(changed_file).collect(),
            uncommitted: u32::try_from(diff.uncommitted).unwrap_or(u32::MAX),
            truncated: diff.truncated,
        })
    }

    async fn review_commits(
        &self,
        repo: &Path,
        base: &str,
        head: &str,
        limit: u32,
    ) -> AppResult<Vec<TaskCommit>> {
        let (git, repo, base, head) = (
            self.git(),
            repo.to_path_buf(),
            base.to_owned(),
            head.to_owned(),
        );
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let commits = blocking(move || git.branch_commits(&repo, &base, &head, limit)).await?;
        Ok(commits
            .into_iter()
            .map(|c| TaskCommit {
                sha: c.sha,
                subject: c.subject,
                author: c.author,
                at: yulius_app::auth::rfc3339(c.at),
                files: c.files,
                insertions: c.insertions,
                deletions: c.deletions,
            })
            .collect())
    }

    async fn diff_stat(&self, repo: &Path, base: &str, head: &str) -> AppResult<DiffStat> {
        let (git, repo, base, head) = (
            self.git(),
            repo.to_path_buf(),
            base.to_owned(),
            head.to_owned(),
        );
        let stat = blocking(move || git.diff_stat(&repo, &base, &head)).await?;
        Ok(DiffStat {
            files: u32::try_from(stat.files).unwrap_or(u32::MAX),
            insertions: u32::try_from(stat.insertions).unwrap_or(u32::MAX),
            deletions: u32::try_from(stat.deletions).unwrap_or(u32::MAX),
        })
    }

    async fn ahead_behind(&self, repo: &Path, base: &str, head: &str) -> AppResult<(u32, u32)> {
        let (git, repo, base, head) = (
            self.git(),
            repo.to_path_buf(),
            base.to_owned(),
            head.to_owned(),
        );
        let ab = blocking(move || git.ahead_behind(&repo, &head, &base)).await?;
        Ok((
            u32::try_from(ab.ahead).unwrap_or(u32::MAX),
            u32::try_from(ab.behind).unwrap_or(u32::MAX),
        ))
    }

    async fn log(&self, repo: &Path, rev: &str, limit: u32) -> AppResult<Vec<CommitInfo>> {
        let (git, repo, rev) = (self.git(), repo.to_path_buf(), rev.to_owned());
        let commits =
            blocking(move || git.log(&repo, &rev, usize::try_from(limit).unwrap_or(50))).await?;
        Ok(commits
            .into_iter()
            .map(|c| CommitInfo {
                sha: c.sha,
                subject: c.subject,
                author: c.author,
                at_ms: c.at.0,
            })
            .collect())
    }

    async fn commit_all(
        &self,
        worktree: &Path,
        message: &str,
        name: &str,
        email: &str,
    ) -> AppResult<bool> {
        let (git, wt, message, author) = (
            self.git(),
            worktree.to_path_buf(),
            message.to_owned(),
            Author::new(name, email),
        );
        blocking(move || {
            git.commit_all(&wt, &message, &author)
                .map(|sha| sha.is_some())
        })
        .await
    }

    async fn keep_and_revert(
        &self,
        worktree: &Path,
        keep: &str,
        base: &str,
        name: &str,
        email: &str,
    ) -> AppResult<String> {
        let (git, wt, keep, author) = (
            self.git(),
            worktree.to_path_buf(),
            keep.to_owned(),
            Author::new(name, email),
        );
        let bases = vec![
            format!("refs/remotes/origin/{base}"),
            format!("refs/heads/{base}"),
        ];
        blocking(move || {
            git.keep_and_revert(
                &wt,
                &keep,
                &bases,
                &format!("yulius: keep this work before reverting the task (kept on {keep})"),
                &author,
            )
        })
        .await
    }

    async fn push(
        &self,
        repo: &Path,
        local_branch: &str,
        remote_branch: &str,
        token: &Secret,
    ) -> AppResult<()> {
        let (git, repo) = (self.git_with_token(token), repo.to_path_buf());
        let refspec = format!("refs/heads/{local_branch}:refs/heads/{remote_branch}");
        blocking(move || git.push(&repo, "origin", &refspec)).await
    }

    async fn import_bundle(
        &self,
        repo: &Path,
        bundle: &Path,
        prefix: &str,
    ) -> AppResult<Vec<String>> {
        let (git, repo, bundle, prefix) = (
            self.git(),
            repo.to_path_buf(),
            bundle.to_path_buf(),
            prefix.trim_matches('/').to_owned(),
        );
        blocking(move || {
            let heads = git.verify_bundle(&repo, &bundle)?;
            let mapping: Vec<ImportRef> = heads
                .iter()
                .filter_map(|h| {
                    let short = h.refname.strip_prefix("refs/heads/")?;
                    Some(ImportRef {
                        source: h.refname.clone(),
                        target_branch: format!("{prefix}/{short}"),
                    })
                })
                .collect();
            if mapping.is_empty() {
                return Ok(Vec::new());
            }
            Ok(git
                .fetch_bundle_into(&repo, &bundle, &mapping)?
                .into_iter()
                .map(|(branch, _)| branch)
                .collect())
        })
        .await
    }
}

/// A reviewed file in the API's form (`added|modified|deleted|renamed`, lines `ctx|add|del`).
fn changed_file(file: FileDiff) -> ChangedFile {
    let status = match file.status {
        FileStatus::Added => "added",
        FileStatus::Modified => "modified",
        FileStatus::Deleted => "deleted",
        FileStatus::Renamed => "renamed",
    };
    ChangedFile {
        path: file.path,
        old_path: file.old_path,
        status: status.to_owned(),
        insertions: file.insertions,
        deletions: file.deletions,
        binary: file.binary,
        hunks: file
            .hunks
            .into_iter()
            .map(|hunk| DiffHunk {
                header: hunk.header,
                lines: hunk
                    .lines
                    .into_iter()
                    .map(|line| DiffLine {
                        kind: match line.kind {
                            LineKind::Context => "ctx",
                            LineKind::Added => "add",
                            LineKind::Deleted => "del",
                        }
                        .to_owned(),
                        old: line.old,
                        new: line.new,
                        text: line.text,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
