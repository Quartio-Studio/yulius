//! Reading and changing a project's files from a person's own agent (SPEC-247 RMT-024..026,
//! ADR-0189): reads come from the project's clone at a branch or commit; a change is one commit
//! on a new `yulius/change-…` branch and a pull request that Yulius CI tests, merged by
//! `change_merge` once it passed. Nothing is written into the sandbox's checkout or a task's
//! worktree: the commit is made in a fresh clone of GitHub's repository (agents can write into
//! the sandbox's clone, so its objects and refs are never pushed), removed on every path.

use std::path::{Component, Path};

use serde_json::{Value, json};
use yulius_domain::ci::{GateResult, gate_checks};
use yulius_domain::delivery::ChecksState;

use crate::auth::AuthContext;
use crate::context::App;
use crate::delivery::{delivery_profile, domain_checks};
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::github::NewPull;
use crate::projects::{blocking, clone_url, commit_identity, remove_tree};

/// The branch prefix of change pull requests; `change_merge` merges only these.
pub const CHANGE_BRANCH_PREFIX: &str = "yulius/change-";
/// Most files one change writes or deletes.
pub const MAX_CHANGE_FILES: usize = 50;
/// Most bytes of one file read or written.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// How many of a project's newest change records `change_merge` looks through.
const MAX_RECORDED_CHANGES: u32 = 1000;
/// Most entries `project_files` lists.
const MAX_LISTED: usize = 1000;
/// Most characters of a path.
const MAX_PATH_CHARS: usize = 512;
/// Most characters of a change's title.
const MAX_TITLE_CHARS: usize = 200;
/// Most characters of a change's description.
const MAX_BODY_CHARS: usize = 20_000;
/// Change commits made at once on this instance (each is a full clone).
pub const MAX_CONCURRENT_CHANGES: usize = 2;

/// One file a change writes (`Some` content) or deletes (`None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Repository-relative path.
    pub path: String,
    /// The whole new content; `None` deletes the file.
    pub content: Option<String>,
}

/// A plain repository-relative path: normal components only, none of them `.git`, no
/// backslash or control character.
///
/// # Errors
/// `Invalid`, naming the path.
pub fn valid_path(path: &str) -> AppResult<String> {
    let path = path.trim().trim_start_matches("./");
    let bad = |why: &str| AppError::Invalid(format!("{path:?}: {why}"));
    if path.is_empty() || path.chars().count() > MAX_PATH_CHARS {
        return Err(bad("a path of 1-512 characters is needed"));
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(bad("no backslashes or control characters"));
    }
    let mut parts = 0;
    for component in Path::new(path).components() {
        match component {
            Component::Normal(name) if !name.eq_ignore_ascii_case(".git") => parts += 1,
            Component::Normal(_) => return Err(bad("nothing inside .git")),
            _ => {
                return Err(bad(
                    "a relative path inside the repository, without .. or /",
                ));
            }
        }
    }
    if parts == 0 {
        return Err(bad("a file path is needed"));
    }
    Ok(path.trim_end_matches('/').to_owned())
}

/// The `files` argument of `project_change`.
///
/// # Errors
/// `Invalid` input.
pub fn parse_changes(args: &Value) -> AppResult<Vec<FileChange>> {
    let files = args
        .get("files")
        .and_then(Value::as_array)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| AppError::Invalid("`files` must list at least one file".into()))?;
    if files.len() > MAX_CHANGE_FILES {
        return Err(AppError::Invalid(format!(
            "at most {MAX_CHANGE_FILES} files in one change"
        )));
    }
    let mut out: Vec<FileChange> = Vec::with_capacity(files.len());
    for file in files {
        let path = valid_path(file.get("path").and_then(Value::as_str).unwrap_or(""))?;
        let delete = file.get("delete").and_then(Value::as_bool).unwrap_or(false);
        let content = match (file.get("content"), delete) {
            (Some(Value::String(text)), false) => {
                if text.len() > MAX_FILE_BYTES {
                    return Err(AppError::Invalid(format!(
                        "{path}: at most {} KiB per file",
                        MAX_FILE_BYTES / 1024
                    )));
                }
                Some(text.clone())
            }
            (None | Some(Value::Null), true) => None,
            _ => {
                return Err(AppError::Invalid(format!(
                    "{path}: give its whole `content`, or `delete: true`"
                )));
            }
        };
        if out.iter().any(|c| c.path == path) {
            return Err(AppError::Invalid(format!("{path} is listed twice")));
        }
        out.push(FileChange { path, content });
    }
    Ok(out)
}

/// Makes `dir` (and its parents) and leaves it readable by the server's user only.
fn private_dir(dir: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dir).map_err(AppError::internal)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(AppError::internal)?;
    }
    Ok(())
}

/// The branch name part made of a change's title.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// Writes the changes under `root`, refusing any path through a symbolic link: the worktree
/// holds the repository's own links, which may point anywhere.
fn apply(root: &Path, changes: &[FileChange]) -> AppResult<()> {
    for change in changes {
        let mut at = root.to_path_buf();
        for component in Path::new(&change.path).components() {
            at.push(component);
            match std::fs::symlink_metadata(&at) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(AppError::Invalid(format!(
                        "{}: the path goes through a symbolic link",
                        change.path
                    )));
                }
                Ok(_) | Err(_) => {}
            }
        }
        let target = root.join(&change.path);
        match &change.content {
            Some(text) => {
                if target.is_dir() {
                    return Err(AppError::Invalid(format!("{} is a directory", change.path)));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(AppError::internal)?;
                }
                std::fs::write(&target, text).map_err(AppError::internal)?;
            }
            None => match std::fs::remove_file(&target) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Err(AppError::Invalid(format!(
                        "{} does not exist, so it cannot be deleted",
                        change.path
                    )));
                }
                Err(err) => return Err(AppError::internal(err)),
            },
        }
    }
    crate::race::reached(crate::race::Point::ChangeWritten, root);
    Ok(())
}

impl App {
    /// The commit a read names: `ref` as a branch of origin or a commit, else the integration
    /// branch (the default branch while the integration branch does not exist yet, before
    /// onboarding made it); after a fetch.
    async fn read_revision(&self, sandbox: &SandboxRecord, args: &Value) -> AppResult<String> {
        self.dlv_refresh_clone(sandbox).await?;
        let repo = self.sandbox_repo(&sandbox.id);
        let asked = args
            .get("ref")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty());
        let candidates: Vec<String> = match asked {
            None => {
                let integration = delivery_profile(sandbox).integration_branch;
                vec![
                    format!("origin/{integration}"),
                    format!("origin/{}", sandbox.default_branch),
                ]
            }
            Some(sha)
                if (7..=40).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()) =>
            {
                vec![sha.to_owned()]
            }
            Some(branch) if self.deps.git.valid_branch(branch) => vec![format!("origin/{branch}")],
            Some(other) => {
                return Err(AppError::Invalid(format!(
                    "{other:?} is not a branch or commit"
                )));
            }
        };
        for rev in &candidates {
            if let Ok(Some(sha)) = self.deps.git.resolve_commit(&repo, rev).await {
                return Ok(sha);
            }
        }
        Err(AppError::Invalid(format!(
            "the repository has no {}",
            asked.unwrap_or("integration or default branch")
        )))
    }

    /// `project_files`: the entries of a directory at a revision.
    ///
    /// # Errors
    /// `Invalid` paths, `NotFound` revisions, Git failures.
    pub(crate) async fn remote_files(
        &self,
        sandbox: &SandboxRecord,
        args: &Value,
    ) -> AppResult<Value> {
        let dir = match args.get("path").and_then(Value::as_str).map(str::trim) {
            None | Some("" | "." | "/") => String::new(),
            Some(path) => valid_path(path)?,
        };
        let sha = self.read_revision(sandbox, args).await?;
        let repo = self.sandbox_repo(&sandbox.id);
        let mut entries = self.deps.git.list_dir(&repo, &sha, &dir).await?;
        let total = entries.len();
        entries.truncate(MAX_LISTED);
        Ok(json!({"commit": sha, "path": dir, "entries": entries, "total": total}))
    }

    /// `project_file_read`: one file at a revision, at most [`MAX_FILE_BYTES`].
    ///
    /// # Errors
    /// `Invalid` paths, `NotFound` files or revisions, Git failures.
    pub(crate) async fn remote_file_read(
        &self,
        sandbox: &SandboxRecord,
        args: &Value,
    ) -> AppResult<Value> {
        let path = valid_path(args.get("path").and_then(Value::as_str).unwrap_or(""))?;
        let sha = self.read_revision(sandbox, args).await?;
        let repo = self.sandbox_repo(&sandbox.id);
        let mut content = self
            .deps
            .git
            .show_file(&repo, &sha, &path)
            .await?
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "{path} does not exist at {}",
                    &sha[..7.min(sha.len())]
                ))
            })?;
        let bytes = content.len();
        let truncated = bytes > MAX_FILE_BYTES;
        if truncated {
            let mut cut = MAX_FILE_BYTES;
            while !content.is_char_boundary(cut) {
                cut -= 1;
            }
            content.truncate(cut);
        }
        Ok(
            json!({"path": path, "commit": sha, "bytes": bytes, "truncated": truncated, "content": content}),
        )
    }

    /// `project_change` (ADR-0189): the files as one commit authored by the person on a new
    /// change branch from the integration branch head, pushed, with a pull request into the
    /// integration branch.
    ///
    /// # Errors
    /// `Invalid` input, `Conflict` when nothing changes, Git and GitHub failures.
    pub(crate) async fn remote_change(
        &self,
        ctx: &AuthContext,
        actor: &str,
        sandbox: &SandboxRecord,
        args: &Value,
    ) -> AppResult<Value> {
        crate::playground::refuse_in_playground(sandbox, "A change pull request")?;
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| {
                !t.is_empty()
                    && t.chars().count() <= MAX_TITLE_CHARS
                    && !t.chars().any(char::is_control)
            })
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "a `title` of 1-{MAX_TITLE_CHARS} characters is needed"
                ))
            })?
            .to_owned();
        let body = args
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        if body.chars().count() > MAX_BODY_CHARS {
            return Err(AppError::Invalid(format!(
                "the `body` holds at most {MAX_BODY_CHARS} characters"
            )));
        }
        let changes = parse_changes(args)?;
        let profile = delivery_profile(sandbox);
        let base = format!("origin/{}", profile.integration_branch);
        let id: String = self
            .deps
            .crypto
            .new_id()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        let branch = match slug(&title) {
            s if s.is_empty() => format!("{CHANGE_BRANCH_PREFIX}{id}"),
            s => format!("{CHANGE_BRANCH_PREFIX}{id}-{s}"),
        };
        // A private directory of the server, outside every sandbox: agents can write anywhere
        // under their sandbox, which would let them change the clone's configuration (where the
        // push sends the person's token) or the files between writing and committing.
        let root = self.config.data_dir.join("changes");
        let scratch = root.join(&id);
        let (clone, work) = (scratch.join("repo"), scratch.join("work"));
        let (name, email) = commit_identity(Some(&ctx.user));
        let token = self.dlv_author_token(&ctx.user, sandbox).await?;
        // A few at once: each is a full clone. Nothing queues for a slot.
        let _slot = self.change_slots.try_acquire().map_err(|_| {
            AppError::Capacity(format!(
                "{MAX_CONCURRENT_CHANGES} changes are being made right now; try again in a moment"
            ))
        })?;
        blocking(move || private_dir(&root)).await?;
        let made = async {
            self.deps
                .git
                .sync_clone(
                    &clone_url(&sandbox.repo_owner, &sandbox.repo_name),
                    &clone,
                    &token,
                )
                .await?;
            if self.deps.git.resolve_commit(&clone, &base).await?.is_none() {
                return Err(AppError::Conflict(format!(
                    "the repository has no {} branch yet (onboarding makes it)",
                    profile.integration_branch
                )));
            }
            self.deps
                .git
                .add_worktree(&clone, &work, &branch, &base)
                .await?;
            let (root, list) = (work.clone(), changes.clone());
            blocking(move || apply(&root, &list)).await?;
            let message = if body.is_empty() {
                title.clone()
            } else {
                format!("{title}\n\n{body}")
            };
            if !self
                .deps
                .git
                .commit_all(&work, &message, &name, &email)
                .await?
            {
                return Err(AppError::Conflict(
                    "the files already hold this content: nothing to change".into(),
                ));
            }
            let sha = self
                .deps
                .git
                .resolve_commit(&work, "HEAD")
                .await?
                .unwrap_or_default();
            if let Err(err) = self.deps.git.scrub_transport_config(&clone).await {
                tracing::debug!(error = %err, "scrubbing a change clone's configuration");
            }
            self.deps.git.push(&work, &branch, &branch, &token).await?;
            Ok(sha)
        }
        .await;
        remove_tree(scratch).await;
        let sha = made?;
        let opened = self
            .deps
            .github
            .create_pull(
                &token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                &NewPull {
                    head: branch.clone(),
                    base: profile.integration_branch.clone(),
                    title: title.clone(),
                    body: format!(
                        "{body}{}Opened by {actor} through Yulius remote control (`project_change`).",
                        if body.is_empty() { "" } else { "\n\n" }
                    ),
                },
            )
            .await;
        let pull = match opened {
            Ok(pull) => pull,
            Err(err) => {
                // No change branch without its pull request: a retry would push another.
                if let Err(gone) = self
                    .deps
                    .github
                    .delete_branch(&token, &sandbox.repo_owner, &sandbox.repo_name, &branch)
                    .await
                {
                    tracing::warn!(error = %gone, branch, "deleting a change branch without a pull request failed");
                }
                return Err(err);
            }
        };
        let detail =
            json!({"branch": branch, "pull": pull.number, "commit": sha, "files": changes.len()})
                .to_string();
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                actor,
                "project.change",
                Some(sandbox.id.as_str()),
                Some(&detail),
            )
            .await
        {
            // The record is what change_merge trusts: without it the pull request is merged on
            // GitHub. The pull request exists, so this is no failed call (which invites a retry).
            tracing::warn!(error = %err, "change audit failed");
            return Ok(json!({
                "pull_request": {"number": pull.number, "url": pull.html_url},
                "branch": branch,
                "commit": sha,
                "warning": "Yulius could not record this change, so change_merge will not merge it: review and merge it on GitHub",
            }));
        }
        Ok(json!({
            "pull_request": {"number": pull.number, "url": pull.html_url},
            "branch": branch,
            "commit": sha,
            "files": changes.iter().map(|c| json!({"path": c.path, "deleted": c.content.is_none()})).collect::<Vec<_>>(),
            "next": "Yulius CI tests the pull request (ci_runs); change_merge merges it once it passed",
        }))
    }

    /// `change_merge`: squash-merges (the profile's method) an open change pull request once
    /// its checks passed at its exact head, then deletes its branch.
    ///
    /// # Errors
    /// `Invalid` input, `Forbidden` for other pull requests, `Conflict` while the checks did
    /// not pass, GitHub failures.
    pub(crate) async fn remote_change_merge(
        &self,
        ctx: &AuthContext,
        actor: &str,
        sandbox: &SandboxRecord,
        args: &Value,
    ) -> AppResult<Value> {
        crate::playground::refuse_in_playground(sandbox, "A change pull request")?;
        let number = args.get("number").and_then(Value::as_u64).ok_or_else(|| {
            AppError::Invalid("`number` must be the pull request's number".into())
        })?;
        let profile = delivery_profile(sandbox);
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let token = self.dlv_author_token(&ctx.user, sandbox).await?;
        let github = &self.deps.github;
        let pull = github.get_pull(&token, owner, name, number).await?;
        // Only what project_change opened, at the commit it made (ADR-0189): a branch someone
        // else named like one, or one pushed to since, is not the person's change.
        let made = self
            .deps
            .store
            .audit_details(sandbox.id.as_str(), "project.change", MAX_RECORDED_CHANGES)
            .await?
            .into_iter()
            .filter_map(|detail| serde_json::from_str::<Value>(&detail).ok())
            .find(|d| {
                d["pull"].as_u64() == Some(number)
                    && d["branch"].as_str() == Some(pull.head.as_str())
            });
        let Some(made) =
            made.filter(|_| pull.head.starts_with(CHANGE_BRANCH_PREFIX) && !pull.from_fork)
        else {
            return Err(AppError::Forbidden(format!(
                "change_merge merges only pull requests project_change opened (branches {CHANGE_BRANCH_PREFIX}…); a task delivers its own"
            )));
        };
        if made["commit"].as_str() != Some(pull.head_sha.as_str()) {
            return Err(AppError::Conflict(format!(
                "pull request #{number} was pushed to since project_change made it; review it on GitHub, or make the change again"
            )));
        }
        if pull.state != "open" || pull.merged {
            return Err(AppError::Conflict(format!(
                "pull request #{number} is not open"
            )));
        }
        if pull.base != profile.integration_branch {
            return Err(AppError::Conflict(format!(
                "pull request #{number} goes into {}, not {}",
                pull.base, profile.integration_branch
            )));
        }
        // As delivery decides (DLV-006): Yulius CI's run at the head alone, else GitHub's checks
        // for a project without Yulius CI; a project with no checks at all merges unchecked.
        // What the integration branch and the pull request say now, not a cached view.
        self.dlv_refresh_clone(sandbox).await?;
        self.ci_forget_gate(&sandbox.id);
        let mut gate = self.ci_gate_result(sandbox, &pull.head_sha).await;
        // As the scheduler decides (`ci_sync_sandbox`): a pull request that brings Yulius CI is
        // tested with it before it merges; one whose configuration Yulius CI cannot read, or any
        // while the integration branch's is unusable (then nothing is tested), never merges here.
        if gate == GateResult::NotConfigured && self.config.ci.enabled {
            if let Some(problem) = self.ci_gate(sandbox).await.problem {
                return Err(AppError::Conflict(format!(
                    "Yulius CI tests nothing while the integration branch's yulius.toml is unusable ({problem}); merge this fix on GitHub, or have a task fix it"
                )));
            }
            match self
                .ci_jobs_at(&self.sandbox_repo(&sandbox.id), &pull.head_sha)
                .await
            {
                Ok(jobs) if !jobs.is_empty() => gate = GateResult::Run(None),
                Ok(_) => {}
                Err(why) => {
                    return Err(AppError::Conflict(format!(
                        "Yulius CI cannot read the pull request's yulius.toml ({why}); fix it with another project_change"
                    )));
                }
            }
        }
        let state = match gate {
            GateResult::NotConfigured => domain_checks(
                github
                    .checks(&token, owner, name, &pull.head_sha)
                    .await?
                    .state,
            ),
            gate => gate_checks(ChecksState::None, gate),
        };
        if !matches!(state, ChecksState::Success | ChecksState::None) {
            return Err(AppError::Conflict(format!(
                "the checks at {} are {}: merge once they passed (ci_runs, ci_run)",
                &pull.head_sha[..7.min(pull.head_sha.len())],
                state.as_str()
            )));
        }
        let merged = github
            .merge_pull(
                &token,
                owner,
                name,
                number,
                profile.merge.as_str(),
                &pull.head_sha,
            )
            .await?;
        if let Err(err) = github.delete_branch(&token, owner, name, &pull.head).await {
            tracing::warn!(error = %err, branch = %pull.head, "deleting a merged change branch failed");
        }
        let detail = json!({"pull": number, "head": pull.head_sha, "merged": merged}).to_string();
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                actor,
                "project.change_merged",
                Some(sandbox.id.as_str()),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, "change merge audit failed");
        }
        Ok(json!({"merged": {
            "number": number,
            "commit": merged,
            "into": profile.integration_branch,
            "checks": state.as_str(),
        }}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_repository_and_out_of_git() {
        assert_eq!(
            valid_path("yulius.toml").ok().as_deref(),
            Some("yulius.toml")
        );
        assert_eq!(valid_path("./docs/a.md").ok().as_deref(), Some("docs/a.md"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            ".git/config",
            "a/.git/hooks/x",
            ".GIT/config",
            "a\\b",
            "a\nb",
            ".",
        ] {
            assert!(valid_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn changes_are_whole_files_or_deletions_listed_once() {
        let ok = parse_changes(
            &json!({"files": [{"path": "a.txt", "content": "x"}, {"path": "b.txt", "delete": true}]}),
        );
        assert_eq!(
            ok.ok(),
            Some(vec![
                FileChange {
                    path: "a.txt".into(),
                    content: Some("x".into())
                },
                FileChange {
                    path: "b.txt".into(),
                    content: None
                },
            ])
        );
        for bad in [
            json!({}),
            json!({"files": []}),
            json!({"files": [{"path": "a"}]}),
            json!({"files": [{"path": "a", "content": "x", "delete": true}]}),
            json!({"files": [{"path": "a", "content": "x"}, {"path": "./a", "content": "y"}]}),
            json!({"files": [{"path": "a", "content": "x".repeat(MAX_FILE_BYTES + 1)}]}),
        ] {
            assert!(parse_changes(&bad).is_err(), "{bad}");
        }
        let many: Vec<Value> = (0..=MAX_CHANGE_FILES)
            .map(|n| json!({"path": format!("f{n}"), "content": ""}))
            .collect();
        assert!(parse_changes(&json!({"files": many})).is_err());
    }

    #[test]
    fn a_link_in_the_path_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::os::unix::fs::symlink(outside.path(), root.path().join("docs"))?;
        let change = FileChange {
            path: "docs/x.md".into(),
            content: Some("x".into()),
        };
        assert!(apply(root.path(), &[change]).is_err());
        assert!(!outside.path().join("x.md").exists());
        let plain = FileChange {
            path: "ci/jobs.toml".into(),
            content: Some("y".into()),
        };
        apply(root.path(), &[plain])?;
        assert_eq!(
            std::fs::read_to_string(root.path().join("ci/jobs.toml"))?,
            "y"
        );
        Ok(())
    }

    #[test]
    fn branch_slugs_are_short_and_plain() {
        assert_eq!(
            slug("Set up CI: [[ci.jobs]] for Rust!"),
            "set-up-ci-ci-jobs-for-rust"
        );
        assert!(slug(&"long ".repeat(40)).len() <= 41);
        assert_eq!(slug("!!!"), "");
    }
}
