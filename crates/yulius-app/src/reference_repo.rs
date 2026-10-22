//! `reference_repo` (SPEC-267, ADR-0197): a task reads another repository of its project's
//! GitHub owner through a bundle Yulius fetches as the task's owner. Git runs on the host only in
//! a fresh folder of the server's own; the bundle reaches the sandbox through the attachment
//! path's handles (ADR-0174), and the agent clones it in its containment. The agent never holds
//! the person's token, and the App's access is not widened (ADR-0114).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_domain::ids::{SandboxId, TaskId};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::Secret;
use crate::ports::github::RepoAccess;
use crate::projects::{blocking, clone_url, valid_owner, valid_repo_name};

/// Largest bundle handed to a task (1 GiB).
pub const MAX_REFERENCE_BYTES: u64 = 1024 * 1024 * 1024;
/// Reference fetches running at once on this instance.
pub const MAX_CONCURRENT_FETCHES: usize = 2;
/// A fetch folder this old was left by a restart (Git's network timeout is 30 minutes).
const STALE_FETCH: Duration = Duration::from_secs(2 * 3600);
/// Name prefix of the fetch folders in `<data>/tmp`.
const SCRATCH_PREFIX: &str = "reference-";

/// `owner/name` of a `repo` argument, or `None` when it is not one.
#[must_use]
pub fn parse_repo(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.trim().split_once('/')?;
    (valid_owner(owner) && valid_repo_name(name)).then_some((owner, name))
}

/// What a detached fetch needs, owned.
struct Fetch {
    repo: String,
    url: String,
    file: String,
    branch: String,
    login: String,
    token: Secret,
    sandbox: SandboxId,
    task: TaskId,
    actor: String,
}

/// A refusal that says only now to ask a person.
fn refused(repo: &str, why: String) -> AppError {
    AppError::Conflict(format!(
        "{why} Only now ask a person with `needs_you`: to give the task's owner read access to \
         {repo} on GitHub, or to put the files you need into the sandbox."
    ))
}

/// Removes the fetch folders below `tmp` older than [`STALE_FETCH`]: a fetch takes at most Git's
/// network timeout, so an older one was cut short by a restart.
fn sweep_stale(tmp: &Path) {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > STALE_FETCH);
        let ours = entry
            .file_name()
            .to_string_lossy()
            .starts_with(SCRATCH_PREFIX);
        if ours && old && entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_scratch(&entry.path());
        }
    }
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
        self: &Arc<Self>,
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
        if owner.eq_ignore_ascii_case(&sandbox.repo_owner)
            && name.eq_ignore_ascii_case(&sandbox.repo_name)
        {
            return Err(AppError::Conflict(format!(
                "{repo} is this sandbox's own repository: your worktree already has it, and \
                 `git fetch` gets its other branches."
            )));
        }
        let of_owner = |found: &str| found.eq_ignore_ascii_case(&sandbox.repo_owner);
        let other_owner = |found: &str| {
            refused(
                &repo,
                format!(
                    "{repo} belongs to {found}, and Yulius fetches only repositories of {}, this \
                     project's owner on GitHub. A public repository you can clone yourself.",
                    sandbox.repo_owner
                ),
            )
        };
        if !of_owner(owner) {
            return Err(other_owner(owner));
        }
        let person = self
            .deps
            .store
            .user(&ctx.task.owner)
            .await?
            .ok_or_else(|| refused(&repo, "This task's owner no longer exists.".into()))?;
        if person.disabled_at.is_some() {
            return Err(refused(
                &repo,
                "This task's owner's account is disabled, so Yulius does not act for them.".into(),
            ));
        }
        let token = self
            .person_github_token(&person, "Reading another repository")
            .await
            .map_err(|err| {
                refused(
                    &repo,
                    format!("Yulius cannot act as this task's owner on GitHub ({err})."),
                )
            })?;
        let login = person.github_login.clone().unwrap_or_default();
        // GitHub follows a moved repository's redirect: the owner it answers with is the one
        // that counts, not the name asked for.
        let found = match self.deps.github.repo_access(&token, owner, name).await {
            Ok(RepoAccess::Access { permissions, repo }) if permissions.pull => repo,
            Ok(_) => {
                return Err(refused(
                    &repo,
                    format!(
                        "{login}, this task's owner, cannot read {repo} on GitHub (or it does \
                         not exist)."
                    ),
                ));
            }
            Err(err) => {
                return Err(refused(
                    &repo,
                    format!("GitHub did not say whether {login} can read {repo} ({err})."),
                ));
            }
        };
        if !of_owner(&found.owner)
            || parse_repo(&format!("{}/{}", found.owner, found.name)).is_none()
        {
            return Err(other_owner(&found.owner));
        }
        if !self.deps.git.valid_branch(&found.default_branch) {
            return Err(refused(
                &repo,
                format!("GitHub names no usable default branch of {repo}."),
            ));
        }
        let slot = Arc::clone(&self.uploads.references)
            .try_acquire_owned()
            .map_err(|_| {
                AppError::Capacity(
                    "other reference fetches are running; try again in a few minutes".into(),
                )
            })?;
        let fetch = Fetch {
            repo: repo.clone(),
            url: clone_url(&found.owner, &found.name),
            file: format!("{owner}-{name}.bundle"),
            branch: found.default_branch,
            login,
            token,
            sandbox: sandbox.id.clone(),
            task: ctx.task.id.clone(),
            actor: ctx.actor(),
        };
        // To its end even when the agent's request goes away (ADR-0053's detached start): the
        // fetch keeps its slot, the folder is always removed, and a placed bundle is audited.
        let (branch, login) = (fetch.branch.clone(), fetch.login.clone());
        let app = Arc::clone(self);
        let (commit, size, bundle) = Self::run_detached(async move {
            let _slot = slot;
            let fetched = app.fetch_reference(&fetch).await?;
            app.audit_reference(
                &fetch,
                &format!(
                    "{}@{} ({}) as {}",
                    fetch.repo, fetched.0, fetch.branch, fetch.login
                ),
            )
            .await;
            Ok(fetched)
        })
        .await?;
        let clone_into = self
            .sandbox_dir_absolute(&sandbox.id)
            .join("reference")
            .join(owner)
            .join(name)
            .to_string_lossy()
            .into_owned();
        Ok(json!({
            "repo": repo,
            "branch": branch,
            "commit": commit,
            "bundle": bundle,
            "bytes": size,
            "clone_into": clone_into,
            "acted_as": login,
            "message": format!(
                "Fetched {repo} `{branch}` at {commit} as {login}. Read-only reference: clone it \
                 outside your worktree with `git clone --quiet --branch {branch} {bundle} \
                 {clone_into}`, or update an earlier copy with `git -C {clone_into} pull --ff-only \
                 {bundle} {branch}`, then remove the bundle with `yulius remove {bundle}` (calling \
                 this tool again replaces it). Never commit its files into this repository beyond \
                 what you port, and never push from it. If you asked a person for access with \
                 needs_you, call needs_you with resolved: true.",
            ),
        }))
    }

    /// Sweeps fetch folders older than [`STALE_FETCH`] (a restart in the middle of a fetch),
    /// then fetches into a fresh folder, bundles and attaches; the folder is removed whatever
    /// happens.
    async fn fetch_reference(&self, fetch: &Fetch) -> AppResult<(String, u64, String)> {
        let tmp = self.config.data_dir.join("tmp");
        let swept = tmp.clone();
        blocking(move || {
            sweep_stale(&swept);
            Ok(())
        })
        .await?;
        let scratch = self.reference_scratch(&tmp).await?;
        let fetched = async {
            let (work, bundle) = (scratch.join("repo.git"), scratch.join("repo.bundle"));
            let commit = self
                .deps
                .git
                .bundle_branch(&fetch.url, &fetch.branch, &work, &bundle, &fetch.token)
                .await
                .map_err(|err| {
                    refused(
                        &fetch.repo,
                        format!("Fetching {} as {} failed ({err}).", fetch.repo, fetch.login),
                    )
                })?;
            let size = file_size(&bundle).await?;
            if size > MAX_REFERENCE_BYTES {
                return Err(refused(
                    &fetch.repo,
                    format!(
                        "{}'s `{}` bundles to {size} bytes, above the {MAX_REFERENCE_BYTES} bytes \
                         a task gets.",
                        fetch.repo, fetch.branch
                    ),
                ));
            }
            let placed = self
                .attach_server_file(
                    (&fetch.sandbox, &fetch.task),
                    &fetch.file,
                    &bundle,
                    MAX_REFERENCE_BYTES,
                    fetch.actor.clone(),
                )
                .await?;
            Ok((commit, size, placed.path))
        }
        .await;
        let removed = scratch.clone();
        blocking(move || {
            remove_scratch(&removed);
            Ok(())
        })
        .await?;
        fetched
    }

    /// A fresh, empty folder below `tmp` (`<data>/tmp`), out of every sandbox.
    async fn reference_scratch(&self, tmp: &Path) -> AppResult<PathBuf> {
        let random = self.deps.crypto.random_bytes();
        let dir = tmp.join(format!(
            "{SCRATCH_PREFIX}{}",
            crate::auth::hex_token(random.get(..8).unwrap_or(&random))
        ));
        let (made, parent) = (dir.clone(), tmp.to_path_buf());
        blocking(move || {
            std::fs::create_dir_all(&parent).map_err(AppError::internal)?;
            std::fs::create_dir(&made).map_err(AppError::internal)
        })
        .await?;
        Ok(dir)
    }

    async fn audit_reference(&self, fetch: &Fetch, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &fetch.actor,
                "task.reference_repo",
                Some(fetch.task.as_str()),
                Some(detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %fetch.task, "reference_repo audit failed");
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
