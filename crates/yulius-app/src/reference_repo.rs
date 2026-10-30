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
use yulius_domain::sweep::CRITICAL_PERCENT;

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::Secret;
use crate::ports::git::ReferenceBundle;
use crate::ports::github::RepoAccess;
use crate::projects::{blocking, clone_url, valid_owner, valid_repo_name};
use crate::uploads::human_size;

/// Largest bundle handed to a task (1 GiB).
pub const MAX_REFERENCE_BYTES: u64 = 1024 * 1024 * 1024;
/// Reference fetches running at once on this instance.
pub const MAX_CONCURRENT_FETCHES: usize = 2;
/// A fetch folder this old was left by a restart (Git's network timeout is 30 minutes).
const STALE_FETCH: Duration = Duration::from_secs(2 * 3600);
/// Name prefix of the fetch folders in `<data>/tmp`.
const SCRATCH_PREFIX: &str = "reference-";
/// Branches an answer lists by name (the bundle carries all of them).
pub const MAX_LISTED_BRANCHES: usize = 100;

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
    /// GitHub's default branch: the bundle's `HEAD`.
    default_branch: String,
    /// The branch the agent asked for, if any (else the default branch).
    wanted: Option<String>,
    login: String,
    token: Secret,
    sandbox: SandboxId,
    task: TaskId,
    actor: String,
}

/// What a fetch placed among the task's attachments.
struct Fetched {
    /// The branch the clone command checks out, and its commit.
    branch: String,
    commit: String,
    size: u64,
    bundle: String,
    carried: ReferenceBundle,
}

/// A refusal that says only now to ask a person.
fn refused(repo: &str, why: String) -> AppError {
    AppError::Conflict(format!(
        "{why} Only now ask a person with `needs_you`: to give the task's owner read access to \
         {repo} on GitHub, or to put the files you need into the sandbox."
    ))
}

/// A repository larger than a task gets; `measured` says how Yulius knows, ending before "above
/// the cap".
fn too_large(repo: &str, measured: &str) -> AppError {
    refused(
        repo,
        format!(
            "{repo} is too large for a reference copy: {measured} above the {} a task gets.",
            human_size(MAX_REFERENCE_BYTES)
        ),
    )
}

/// A size above the 1 GiB cap in GB, rounded up to a tenth: never the cap itself.
fn above_cap(bytes: u64) -> String {
    const GIB: u128 = 1024 * 1024 * 1024;
    let tenths = (u128::from(bytes) * 10).div_ceil(GIB);
    if tenths % 10 == 0 {
        format!("{} GB", tenths / 10)
    } else {
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
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
    /// `reference_repo {repo, branch?}`: fetches every branch and tag of another repository of
    /// the sandbox's owner as the task's owner and places a bundle of them among the task's
    /// attachments; `branch` (default: the default branch) is the one the clone checks out
    /// (SPEC-267 REF-001..004, SPEC-330).
    ///
    /// # Errors
    /// `Invalid` for a malformed `repo` or `branch`, and for a `branch` the repository lacks;
    /// `Conflict` (naming `needs_you`) for another owner, the
    /// sandbox's own repository, a playground, an owner who is gone, disabled, without GitHub or
    /// without read access, a repository too large (as GitHub reports it, while it is fetched or
    /// as bundled: SPEC-349), and a data disk without room for it; Git and store failures.
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
        let wanted = match args.get("branch") {
            None | Some(Value::Null) => None,
            Some(Value::String(branch)) if self.deps.git.valid_branch(branch) => {
                Some(branch.clone())
            }
            Some(_) => {
                return Err(AppError::Invalid(
                    "branch must be a branch name of the repository, e.g. dev".into(),
                ));
            }
        };
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
        // GitHub's size lags behind pushes, so it only refuses up front; the fetch is bounded
        // while it runs (SPEC-349 RFB-001, RFB-003).
        let reported = found.size_kib.map(|kib| kib.saturating_mul(1024));
        if let Some(bytes) = reported.filter(|bytes| *bytes > MAX_REFERENCE_BYTES) {
            return Err(too_large(
                &repo,
                &format!("GitHub reports it at {},", above_cap(bytes)),
            ));
        }
        // At its peak the fetched copy, its bundle and the attached bundle are on the disk; with
        // no size from GitHub (or a new repository's 0), each may reach the cap.
        let peak = reported
            .filter(|bytes| *bytes > 0)
            .unwrap_or(MAX_REFERENCE_BYTES)
            .saturating_mul(3);
        if let Some(room) = self.disk_room_if_short(peak).await {
            return Err(AppError::Conflict(format!(
                "The server's disk has no room for a copy of {repo}: only {} can be added before \
                 it is {CRITICAL_PERCENT} % full. Only now ask a person with `needs_you` to free \
                 space on the server's disk, or to put the files you need into the sandbox.",
                human_size(room)
            )));
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
            default_branch: found.default_branch,
            wanted,
            login,
            token,
            sandbox: sandbox.id.clone(),
            task: ctx.task.id.clone(),
            actor: ctx.actor(),
        };
        // To its end even when the agent's request goes away (ADR-0053's detached start): the
        // fetch keeps its slot, the folder is always removed, and a placed bundle is audited.
        let (default_branch, login) = (fetch.default_branch.clone(), fetch.login.clone());
        let app = Arc::clone(self);
        let fetched = Self::run_detached(async move {
            let _slot = slot;
            let fetched = app.fetch_reference(&fetch).await?;
            app.audit_reference(
                &fetch,
                &format!(
                    "{}@{} ({}; {} branches, {} tags) as {}",
                    fetch.repo,
                    fetched.commit,
                    fetched.branch,
                    fetched.carried.branches.len(),
                    fetched.carried.tags,
                    fetch.login
                ),
            )
            .await;
            Ok(fetched)
        })
        .await?;
        let Fetched {
            branch,
            commit,
            size,
            bundle,
            carried,
        } = fetched;
        let clone_into = self
            .sandbox_dir_absolute(&sandbox.id)
            .join("reference")
            .join(owner)
            .join(name)
            .to_string_lossy()
            .into_owned();
        let (branch_count, tag_count) = (carried.branches.len(), carried.tags);
        let branches: Vec<Value> = carried
            .branches
            .iter()
            .take(MAX_LISTED_BRANCHES)
            .map(|(name, commit)| json!({"name": name, "commit": commit}))
            .collect();
        let (copy, from) = (sh_quote(&clone_into), sh_quote(&bundle));
        let clone_command = format!(
            "git clone --quiet --branch {} {from} {copy}",
            sh_quote(&branch)
        );
        // Forced refspecs: a re-pointed tag updates instead of failing the fetch (`--tags` never
        // moves an existing tag).
        let update_command = format!(
            "git -C {copy} fetch --quiet --prune {from} '+refs/heads/*:refs/remotes/origin/*' \
             '+refs/tags/*:refs/tags/*' && git -C {copy} checkout --quiet -B {} {}",
            sh_quote(&branch),
            sh_quote(&format!("origin/{branch}"))
        );
        Ok(json!({
            "repo": repo,
            "branch": branch,
            "commit": commit,
            "default_branch": default_branch,
            "branches": branches,
            "branch_count": branch_count,
            "tag_count": tag_count,
            "bundle": bundle,
            "bytes": size,
            "clone_into": clone_into,
            "clone_command": clone_command,
            "update_command": update_command,
            "acted_as": login,
            "message": format!(
                "Fetched {repo} as {login}: all {branch_count} branches and {tag_count} tags; \
                 `{branch}` is at {commit} (default branch `{default_branch}`). Read-only \
                 reference: clone it outside your worktree with `{clone_command}`; every other \
                 branch is in the copy as origin/<name> (`git -C {copy} checkout <name>`). Update \
                 an earlier copy instead with `{update_command}`. Then remove the bundle with \
                 `yulius remove {from}` (calling this tool again replaces it). Never commit its \
                 files into this repository beyond what you port, and never push from it. If you \
                 asked a person for access with needs_you, call needs_you with resolved: true.",
            ),
        }))
    }

    /// Sweeps fetch folders older than [`STALE_FETCH`] (a restart in the middle of a fetch),
    /// then fetches every branch and tag into a fresh folder, bundles them and attaches the
    /// bundle when it has the wanted branch; the folder is removed whatever happens.
    async fn fetch_reference(&self, fetch: &Fetch) -> AppResult<Fetched> {
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
            let carried = self
                .deps
                .git
                .bundle_repository(
                    &fetch.url,
                    &fetch.default_branch,
                    (&work, &bundle),
                    MAX_REFERENCE_BYTES,
                    &fetch.token,
                )
                .await
                .map_err(|err| match err {
                    // Git was stopped at the cap (SPEC-349 RFB-003): the port's only capacity error.
                    AppError::Capacity(_) => {
                        too_large(&fetch.repo, "Yulius stopped Git when its copy grew")
                    }
                    err => refused(
                        &fetch.repo,
                        format!("Fetching {} as {} failed ({err}).", fetch.repo, fetch.login),
                    ),
                })?;
            let branch = fetch
                .wanted
                .clone()
                .unwrap_or_else(|| fetch.default_branch.clone());
            let Some(commit) = carried
                .branches
                .iter()
                .find(|(name, _)| *name == branch)
                .map(|(_, commit)| commit.clone())
            else {
                return Err(missing_branch(&fetch.repo, &branch, &carried));
            };
            let size = file_size(&bundle).await?;
            if size > MAX_REFERENCE_BYTES {
                return Err(too_large(
                    &fetch.repo,
                    &format!("it bundles to {},", above_cap(size)),
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
            Ok(Fetched {
                branch,
                commit,
                size,
                bundle: placed.path,
                carried,
            })
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

/// `text` as one POSIX shell word: branch names may hold `$`, `(` or `'`, and an agent pastes the
/// answered commands into its shell.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A branch the repository lacks: the agent's own mistake, so it is told the branches and no
/// person is asked.
fn missing_branch(repo: &str, branch: &str, carried: &ReferenceBundle) -> AppError {
    let names: Vec<&str> = carried
        .branches
        .iter()
        .take(MAX_LISTED_BRANCHES)
        .map(|(name, _)| name.as_str())
        .collect();
    let more = carried.branches.len().saturating_sub(names.len());
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    AppError::Invalid(format!(
        "{repo} has no branch `{branch}`. Its branches: {}{more}. Call reference_repo again \
         with one of them, or without branch for the default branch.",
        names.join(", ")
    ))
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

    /// Review round 1: a size just above the cap never reads as the cap.
    #[test]
    fn a_size_above_the_cap_is_rounded_up() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(above_cap(GIB + 1), "1.1 GB");
        assert_eq!(above_cap(GIB + GIB / 10), "1.1 GB");
        assert_eq!(above_cap(GIB + GIB / 10 + 1), "1.2 GB");
        assert_eq!(above_cap(2 * GIB), "2 GB");
        assert_eq!(above_cap(2 * GIB - 1), "2 GB");
        assert_eq!(above_cap(u64::MAX), "17179869184 GB");
    }

    #[test]
    fn a_shell_word_keeps_quotes_and_expansions_literal() {
        assert_eq!(sh_quote("dev"), "'dev'");
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
        assert_eq!(sh_quote("$(x)"), "'$(x)'");
    }

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
