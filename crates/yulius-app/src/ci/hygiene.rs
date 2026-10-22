//! Repository hygiene (SPEC-001 DLV-007): after Yulius merges a pull request its branch is
//! deleted; a periodic, bounded round removes each sandbox's safe stale branches and worktrees
//! itself (SPEC-269, ADR-0199) and lists the rest; people and agents clean up on request too.
//!
//! Safety: protected branches (integration, production, default) and branches with an open
//! pull request are never stale; a live task's worktree never is; worktrees are removed only
//! when clean (never forced); local branches only when no worktree uses them and their commits
//! are on GitHub; unmerged remote branches without a pull request (`idle`) only with `force`.
//! GitHub keeps a closed or merged pull request's commits, so deleting its branch loses
//! nothing ("Restore branch" on GitHub).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use yulius_contracts::api::ci::{
    HygieneCleanup, HygieneCleanupResult, HygieneItem, HygieneLastCleanup, HygieneReport,
    HygieneSkip, HygieneTarget,
};
use yulius_domain::ci::{
    BranchPull, LocalBranchFacts, RemoteBranchFacts, StaleReason, WorktreeOwner,
    classify_local_branch, classify_remote_branch, classify_worktree,
};
use yulius_domain::ids::TaskId;
use yulius_domain::permission::Action;
use yulius_domain::task::{StopReason, TaskStatus};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::github::GhPull;

/// Interval of the background scan.
pub const HYGIENE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// First background scan after start.
const HYGIENE_FIRST: Duration = Duration::from_secs(10 * 60);
/// Remote (and local) branches examined per scan.
pub const MAX_BRANCHES_SCANNED: usize = 300;
/// Pull request lookups per scan (one GitHub request each).
pub const MAX_PULL_LOOKUPS: usize = 50;
/// Items a cleanup handles at once.
pub const MAX_CLEANUP_ITEMS: usize = 100;

const REMOTE: &str = "remote_branch";
const LOCAL: &str = "local_branch";
const WORKTREE: &str = "worktree";
/// Who the background round acts as in the audit log.
const AUTO_ACTOR: &str = "yulius";

fn item(kind: &str, name: String, reason: &StaleReason) -> HygieneItem {
    HygieneItem {
        kind: kind.to_owned(),
        name,
        reason_code: reason.code().to_owned(),
        reason: reason.describe(),
        safe: reason.safe(),
    }
}

fn pulls_facts(pulls: &[GhPull], tip: &str) -> Vec<BranchPull> {
    pulls
        .iter()
        .map(|p| BranchPull {
            number: p.number,
            open: p.state == "open" && !p.merged,
            merged: p.merged,
            tip_on_pull: p.head_sha == tip,
        })
        .collect()
}

/// The order a cleanup removes items in: remote branches first, so local copies are judged
/// again against GitHub after them; then worktrees; then local branches.
fn removal_order(kind: &str) -> u8 {
    match kind {
        REMOTE => 0,
        WORKTREE => 1,
        _ => 2,
    }
}

impl App {
    pub(super) fn start_hygiene(self: &Arc<Self>) {
        let app = Arc::clone(self);
        app.schedule_hygiene(HYGIENE_FIRST);
        tokio::spawn(async move {
            tokio::time::sleep(HYGIENE_FIRST).await;
            let mut every = tokio::time::interval(HYGIENE_EVERY);
            loop {
                every.tick().await;
                app.hygiene_round().await;
            }
        });
    }

    /// One background round (SPEC-269 HSC-001): each live sandbox is scanned and its safe items
    /// are removed as Yulius, through the same checks as a person's Clean up. Unmerged work
    /// (`idle`) stays listed for a person.
    pub async fn hygiene_round(&self) {
        // The interval ticks from the start of a round.
        self.schedule_hygiene(HYGIENE_EVERY);
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            let request = HygieneCleanup::default();
            if let Err(err) = self
                .hygiene_cleanup_by(sandbox, &request, AUTO_ACTOR, true)
                .await
            {
                tracing::warn!(sandbox = %sandbox.slug, error = %err, "automatic hygiene cleanup failed");
            }
        }
    }

    fn schedule_hygiene(&self, after: Duration) {
        let after_ms = i64::try_from(after.as_millis()).unwrap_or(i64::MAX);
        if let Ok(mut next) = self.ci.hygiene_next.lock() {
            *next = Some(self.now().0.saturating_add(after_ms));
        }
    }

    /// A report with the sandbox's last cleanup and the next automatic one.
    fn with_cleanups(&self, sandbox: &SandboxRecord, mut report: HygieneReport) -> HygieneReport {
        report.last_cleanup = self
            .ci
            .hygiene_cleanups
            .lock()
            .ok()
            .and_then(|cleanups| cleanups.get(sandbox.id.as_str()).cloned());
        report.next_cleanup_at = self
            .ci
            .hygiene_next
            .lock()
            .ok()
            .and_then(|next| *next)
            .map(|ms| rfc3339(yulius_domain::time::Timestamp(ms)));
        report
    }

    fn protected_branches(sandbox: &SandboxRecord) -> HashSet<String> {
        let profile = delivery_profile(sandbox);
        [
            profile.integration_branch,
            profile.production_branch,
            sandbox.default_branch.clone(),
        ]
        .into_iter()
        .collect()
    }

    /// Scans a sandbox for stale branches and worktrees and keeps the report.
    pub(crate) async fn hygiene_scan(&self, sandbox: &SandboxRecord) -> HygieneReport {
        let mut report = HygieneReport {
            scanned_at: Some(rfc3339(self.now())),
            ..HygieneReport::default()
        };
        let repo = self.sandbox_repo(&sandbox.id);
        if !repo.join(".git").exists() {
            report
                .notes
                .push("The sandbox repository is not cloned yet.".into());
            return self.keep_report(sandbox, report);
        }
        // A playground's branches are all local: no GitHub to ask (SPEC-099).
        let token = match self.dlv_installation_token(sandbox).await {
            _ if sandbox.is_playground() => {
                report
                    .notes
                    .push(crate::playground::PLAYGROUND_NOTE.to_owned());
                None
            }
            Ok(token) => Some(token),
            Err(err) => {
                report.notes.push(format!(
                    "GitHub is not available ({}): remote branches are not checked.",
                    err.public_message()
                ));
                None
            }
        };
        if token.is_some()
            && let Err(err) = self.dlv_refresh_clone(sandbox).await
        {
            report.notes.push(format!(
                "Fetching failed ({}): branches are as of the last fetch.",
                err.public_message()
            ));
        }
        let protected = Self::protected_branches(sandbox);
        let profile = delivery_profile(sandbox);
        if let Some(token) = &token {
            self.hygiene_remote(sandbox, &repo, token, &protected, &profile, &mut report)
                .await;
        }
        self.hygiene_local(sandbox, &repo, &protected, &mut report)
            .await;
        self.keep_report(sandbox, report)
    }

    fn keep_report(&self, sandbox: &SandboxRecord, report: HygieneReport) -> HygieneReport {
        if let Ok(mut reports) = self.ci.hygiene.lock() {
            reports.insert(sandbox.id.as_str().to_owned(), report.clone());
        }
        self.with_cleanups(sandbox, report)
    }

    async fn hygiene_remote(
        &self,
        sandbox: &SandboxRecord,
        repo: &Path,
        token: &crate::model::Secret,
        protected: &HashSet<String>,
        profile: &yulius_domain::delivery::DeliveryProfile,
        report: &mut HygieneReport,
    ) {
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let open: HashSet<String> =
            match self.deps.github.list_pulls(token, owner, name, None).await {
                Ok(pulls) => pulls.into_iter().map(|p| p.head).collect(),
                Err(err) => {
                    report.notes.push(format!(
                        "Open pull requests can't be listed ({}): remote branches are not checked.",
                        err.public_message()
                    ));
                    return;
                }
            };
        let mut branches = match self.deps.git.remote_branches(repo).await {
            Ok(branches) => branches,
            Err(err) => {
                report.notes.push(format!(
                    "Remote branches can't be listed: {}",
                    err.public_message()
                ));
                return;
            }
        };
        // Oldest first: the likeliest leftovers are examined within the bound.
        branches.sort_by_key(|b| b.committed_ms);
        if branches.len() > MAX_BRANCHES_SCANNED {
            report.notes.push(format!(
                "Only the {MAX_BRANCHES_SCANNED} oldest of {} branches were examined.",
                branches.len()
            ));
            branches.truncate(MAX_BRANCHES_SCANNED);
        }
        let targets: Vec<String> = [&profile.integration_branch, &profile.production_branch]
            .iter()
            .map(|b| format!("origin/{b}"))
            .collect();
        let now = self.now().0;
        let mut lookups = 0usize;
        let mut bound_hit = false;
        for branch in branches {
            if protected.contains(&branch.name) {
                continue;
            }
            let mut merged = false;
            for target in &targets {
                if matches!(
                    self.deps.git.is_ancestor(repo, &branch.sha, target).await,
                    Ok(true)
                ) {
                    merged = true;
                    break;
                }
            }
            let pulls = if open.contains(&branch.name) {
                Some(vec![BranchPull {
                    number: 0,
                    open: true,
                    merged: false,
                    tip_on_pull: true,
                }])
            } else if merged {
                Some(Vec::new())
            } else if lookups < MAX_PULL_LOOKUPS {
                lookups += 1;
                self.deps
                    .github
                    .branch_pulls(token, &sandbox.repo_owner, &sandbox.repo_name, &branch.name)
                    .await
                    .ok()
                    .map(|pulls| pulls_facts(&pulls, &branch.sha))
            } else {
                bound_hit = true;
                None
            };
            let facts = RemoteBranchFacts {
                protected: false,
                merged,
                pulls,
                last_commit_ms: branch.committed_ms,
            };
            if let Some(reason) = classify_remote_branch(&facts, now) {
                report.items.push(item(REMOTE, branch.name, &reason));
            }
        }
        if bound_hit {
            report.notes.push(format!(
                "Pull requests were looked up for {MAX_PULL_LOOKUPS} branches; the rest is checked by the next scan."
            ));
        }
    }

    async fn hygiene_local(
        &self,
        sandbox: &SandboxRecord,
        repo: &Path,
        protected: &HashSet<String>,
        report: &mut HygieneReport,
    ) {
        let worktrees = match self.deps.git.worktrees(repo).await {
            Ok(worktrees) => worktrees,
            Err(err) => {
                report.notes.push(format!(
                    "Worktrees can't be listed: {}",
                    err.public_message()
                ));
                return;
            }
        };
        let checked_out: HashSet<String> =
            worktrees.iter().filter_map(|w| w.branch.clone()).collect();
        if let Ok(mut locals) = self.deps.git.local_branches(repo).await {
            if locals.len() > MAX_BRANCHES_SCANNED {
                locals.sort_by_key(|b| b.committed_ms);
                locals.truncate(MAX_BRANCHES_SCANNED);
                report.notes.push(format!(
                    "Only the {MAX_BRANCHES_SCANNED} oldest local branches were examined."
                ));
            }
            for branch in locals {
                let facts = LocalBranchFacts {
                    protected: protected.contains(&branch.name),
                    checked_out: checked_out.contains(&branch.name),
                    on_remote: matches!(self.deps.git.on_remote(repo, &branch.sha).await, Ok(true)),
                };
                if let Some(reason) = classify_local_branch(facts) {
                    report.items.push(item(LOCAL, branch.name, &reason));
                }
            }
        }
        let sandbox_dir = self.sandbox_dir(&sandbox.id);
        let task_root = sandbox_dir.join("worktrees");
        let ci_root = self.ci_dir(&sandbox.id);
        let mut tasks = HashMap::new();
        for archived in [false, true] {
            if let Ok(list) = self.deps.store.tasks(&sandbox.id, archived).await {
                for task in list {
                    tasks.insert(task.id.as_str().to_owned(), task);
                }
            }
        }
        for worktree in worktrees.into_iter().filter(|w| !w.main) {
            if worktree.path.starts_with(&ci_root) {
                // CI checkouts are the CI's caches, kept on purpose.
                continue;
            }
            let owner = match worktree.path.parent() {
                Some(parent) if parent == task_root => {
                    let id = worktree
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    match tasks.get(&id) {
                        // A snapshot import's checkout is the import's, not a task's.
                        None if id.starts_with(crate::imports::IMPORT_WORKTREE_PREFIX) => {
                            WorktreeOwner::Other
                        }
                        None => WorktreeOwner::Deleted,
                        Some(task) => WorktreeOwner::Task {
                            number: task.number,
                            archived: task.archived_at.is_some(),
                            merged: match TaskId::parse(&id) {
                                Ok(task_id) => self.task_merged(&task_id).await,
                                Err(_) => false,
                            },
                            // In use unless resting: a queued or paused task (its process alive
                            // at a turn boundary) resumes in this worktree (SPEC-269).
                            live: !task.status.is_resting(),
                            ended: matches!(
                                task.status,
                                TaskStatus::Closed | TaskStatus::Stopped(StopReason::Cancelled)
                            ),
                        },
                    }
                }
                _ => WorktreeOwner::Other,
            };
            if let Some(reason) = classify_worktree(owner, worktree.prunable) {
                let name = worktree.path.strip_prefix(&sandbox_dir).map_or_else(
                    |_| worktree.path.to_string_lossy().into_owned(),
                    |p| p.to_string_lossy().into_owned(),
                );
                report.items.push(item(WORKTREE, name, &reason));
            }
        }
    }

    /// The last report of a sandbox, scanning when there is none yet.
    pub async fn hygiene_report(&self, sandbox: &SandboxRecord) -> HygieneReport {
        let kept = self
            .ci
            .hygiene
            .lock()
            .ok()
            .and_then(|reports| reports.get(sandbox.id.as_str()).cloned());
        match kept {
            Some(report) => self.with_cleanups(sandbox, report),
            None => self.hygiene_scan(sandbox).await,
        }
    }

    /// Removes stale items (all safe ones when `request.items` is empty), after a fresh scan.
    ///
    /// # Errors
    /// Too many items requested.
    pub async fn hygiene_cleanup_as(
        &self,
        sandbox: &SandboxRecord,
        request: &HygieneCleanup,
        actor: &str,
    ) -> AppResult<HygieneCleanupResult> {
        self.hygiene_cleanup_by(sandbox, request, actor, false)
            .await
    }

    /// [`Self::hygiene_cleanup_as`], `automatic` for the background round. One cleanup of a
    /// sandbox runs at a time: a person's Clean up during the round waits for it, then scans.
    async fn hygiene_cleanup_by(
        &self,
        sandbox: &SandboxRecord,
        request: &HygieneCleanup,
        actor: &str,
        automatic: bool,
    ) -> AppResult<HygieneCleanupResult> {
        if request.items.len() > MAX_CLEANUP_ITEMS {
            return Err(AppError::Invalid(format!(
                "clean up at most {MAX_CLEANUP_ITEMS} items at once"
            )));
        }
        let lock = self.hygiene_lock(sandbox);
        let _one = lock.lock().await;
        let report = self.hygiene_scan(sandbox).await;
        let mut skipped = Vec::new();
        let mut selected: Vec<HygieneItem> = if request.items.is_empty() {
            report
                .items
                .iter()
                .filter(|i| i.safe || request.force)
                .cloned()
                .collect()
        } else {
            let mut chosen = Vec::new();
            for target in &request.items {
                match report
                    .items
                    .iter()
                    .find(|i| i.kind == target.kind && i.name == target.name)
                {
                    Some(found) => chosen.push(found.clone()),
                    None => skipped.push(HygieneSkip {
                        target: target.clone(),
                        reason: "not stale (it is in use, protected or already gone)".into(),
                    }),
                }
            }
            chosen
        };
        selected.sort_by_key(|i| removal_order(&i.kind));
        let repo = self.sandbox_repo(&sandbox.id);
        let token = if sandbox.is_playground() {
            None
        } else {
            self.dlv_installation_token(sandbox).await.ok()
        };
        let mut removed = Vec::new();
        // Whether the clone was fetched after the remote deletions (`Err`: it could not be).
        let mut fetched: Option<Result<(), String>> = None;
        for found in selected.into_iter().take(MAX_CLEANUP_ITEMS) {
            let target = HygieneTarget {
                kind: found.kind.clone(),
                name: found.name.clone(),
            };
            if !found.safe && !request.force {
                skipped.push(HygieneSkip {
                    target,
                    reason: format!("{}; pass force to delete unmerged work", found.reason),
                });
                continue;
            }
            let outcome: Result<(), String> = match found.kind.as_str() {
                REMOTE => match &token {
                    Some(token) => {
                        self.delete_remote_branch(sandbox, &repo, token, &found.name)
                            .await
                    }
                    None => Err("GitHub is not available".into()),
                },
                LOCAL => {
                    // Judged again against GitHub as it is now, after the remote deletions
                    // above: a local copy whose branch just went may hold the only commits.
                    if fetched.is_none() {
                        fetched = Some(
                            self.dlv_refresh_clone(sandbox)
                                .await
                                .map_err(|e| e.public_message()),
                        );
                    }
                    match &fetched {
                        Some(Err(err)) => Err(format!(
                            "GitHub could not be fetched ({err}), so its commits there are not confirmed"
                        )),
                        _ => match self
                            .deps
                            .git
                            .on_remote(&repo, &format!("refs/heads/{}", found.name))
                            .await
                        {
                            Ok(true) => self
                                .deps
                                .git
                                .delete_local_branch(&repo, &found.name)
                                .await
                                .map_err(|e| e.public_message()),
                            Ok(false) => Err("its commits are no longer on GitHub".into()),
                            Err(err) => Err(err.public_message()),
                        },
                    }
                }
                WORKTREE if found.reason_code == "missing" => self
                    .deps
                    .git
                    .prune_worktrees(&repo)
                    .await
                    .map_err(|e| e.public_message()),
                WORKTREE => self.remove_task_worktree(sandbox, &repo, &found.name).await,
                _ => Err("unknown kind".into()),
            };
            match outcome {
                Ok(()) => {
                    self.dlv_audit(
                        actor,
                        "hygiene.cleanup",
                        &sandbox.slug,
                        &format!("{} {}: {}", found.kind, found.name, found.reason),
                    )
                    .await;
                    removed.push(target);
                }
                Err(reason) => skipped.push(HygieneSkip { target, reason }),
            }
        }
        if fetched.is_none() && removed.iter().any(|t| t.kind == REMOTE) {
            let _ = self.dlv_refresh_clone(sandbox).await;
        }
        // An automatic round with nothing to do keeps the last cleanup on the report.
        if !(automatic && removed.is_empty() && skipped.is_empty())
            && let Ok(mut cleanups) = self.ci.hygiene_cleanups.lock()
        {
            cleanups.insert(
                sandbox.id.as_str().to_owned(),
                HygieneLastCleanup {
                    at: rfc3339(self.now()),
                    by: actor.to_owned(),
                    automatic,
                    removed: removed.clone(),
                    skipped: skipped.clone(),
                },
            );
        }
        // Nothing removed: nothing changed since the scan above (a round with nothing to do
        // costs one scan).
        let report = if removed.is_empty() {
            self.with_cleanups(sandbox, report)
        } else {
            self.hygiene_scan(sandbox).await
        };
        Ok(HygieneCleanupResult {
            removed,
            skipped,
            report,
        })
    }

    /// Deletes a branch on GitHub only while its tip there is the commit the scan judged (the
    /// clone's `origin/<branch>`): a fetch that failed, or a push since, leaves new commits the
    /// verdict never saw (SPEC-269 review round 2).
    async fn delete_remote_branch(
        &self,
        sandbox: &SandboxRecord,
        repo: &Path,
        token: &crate::model::Secret,
        branch: &str,
    ) -> Result<(), String> {
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        let judged = self
            .deps
            .git
            .resolve_commit(repo, &format!("refs/remotes/origin/{branch}"))
            .await
            .map_err(|e| e.public_message())?;
        let now = self
            .deps
            .github
            .branch_sha(token, owner, name, branch)
            .await
            .map_err(|e| e.public_message())?;
        match (judged, now) {
            (_, None) => Err("it is already gone from GitHub".into()),
            (Some(judged), Some(now)) if judged == now => self
                .deps
                .github
                .delete_branch(token, owner, name, branch)
                .await
                .map(|_| ())
                .map_err(|e| e.public_message()),
            _ => Err(
                "it moved on GitHub since Yulius last fetched it; its new commits are judged at the next check"
                    .into(),
            ),
        }
    }

    /// Removes a task's worktree (`worktrees/<task id>`), checking right before it that Git
    /// ignores nothing in it but rebuildable caches (SPEC-019, SPEC-269: local configuration or
    /// data is not Git's to lose) and that its task is still resting or gone.
    async fn remove_task_worktree(
        &self,
        sandbox: &SandboxRecord,
        repo: &Path,
        name: &str,
    ) -> Result<(), String> {
        let path = self.sandbox_dir(&sandbox.id).join(name);
        let owned = self.sandbox_dir(&sandbox.id).join("worktrees");
        if !self.worktree_only_caches(&path).await {
            return Err(
                "it still holds files Git ignores that are not rebuildable caches (local configuration or data)"
                    .into(),
            );
        }
        // Read last, right before Git removes it; a task that cannot be read keeps it.
        let task_id = path
            .file_name()
            .and_then(|n| TaskId::parse(&n.to_string_lossy()).ok());
        if let Some(task_id) = task_id {
            match self.deps.store.task(&task_id).await {
                Ok(None) => {}
                Ok(Some(task)) if task.status.is_resting() => {}
                Ok(Some(_)) => return Err("its task is running or waiting again".into()),
                Err(err) => {
                    return Err(format!("its task can't be read: {}", err.public_message()));
                }
            }
        }
        match self.deps.git.remove_worktree(repo, &path, &owned).await {
            Ok(true) => Ok(()),
            Ok(false) => Err("it has uncommitted changes or is locked".into()),
            Err(err) => Err(err.public_message()),
        }
    }

    /// After a merge through Yulius: deletes the pull request's head branch unless it is
    /// protected or another open pull request uses it. Returns a note when it could not.
    pub(crate) async fn delete_merged_branch(
        &self,
        sandbox: &SandboxRecord,
        pull: &GhPull,
    ) -> Option<String> {
        if Self::protected_branches(sandbox).contains(&pull.head) {
            return None;
        }
        let token = match self.dlv_installation_token(sandbox).await {
            Ok(token) => token,
            Err(err) => {
                return Some(format!(
                    "Branch {} was kept: {}",
                    pull.head,
                    err.public_message()
                ));
            }
        };
        let (owner, name) = (&sandbox.repo_owner, &sandbox.repo_name);
        match self.deps.github.list_pulls(&token, owner, name, None).await {
            Ok(open)
                if open
                    .iter()
                    .any(|p| p.head == pull.head && p.number != pull.number) =>
            {
                return None;
            }
            Ok(_) => {}
            Err(err) => {
                return Some(format!(
                    "Branch {} was kept: open pull requests can't be listed ({})",
                    pull.head,
                    err.public_message()
                ));
            }
        }
        match self
            .deps
            .github
            .delete_branch(&token, owner, name, &pull.head)
            .await
        {
            Ok(_) => {
                self.dlv_audit(
                    "yulius",
                    "hygiene.branch_deleted",
                    &sandbox.slug,
                    &format!("{} after pull request #{} merged", pull.head, pull.number),
                )
                .await;
                None
            }
            Err(err) => Some(format!(
                "Branch {} could not be deleted: {}",
                pull.head,
                err.public_message()
            )),
        }
    }

    // ---------------------------------------------------------------- API

    /// `GET /api/sandboxes/{sid}/hygiene`: the last report (scans when there is none).
    ///
    /// # Errors
    /// Access errors.
    pub async fn hygiene_view(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<HygieneReport> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::View).await?;
        Ok(self.hygiene_report(&sandbox).await)
    }

    /// A scan on request, after any cleanup of the sandbox running now: a scan that ended after
    /// it would keep listing what it removed.
    pub(crate) async fn hygiene_scan_locked(&self, sandbox: &SandboxRecord) -> HygieneReport {
        let lock = self.hygiene_lock(sandbox);
        let _one = lock.lock().await;
        self.hygiene_scan(sandbox).await
    }

    fn hygiene_lock(&self, sandbox: &SandboxRecord) -> Arc<tokio::sync::Mutex<()>> {
        self.ci
            .hygiene_locks
            .lock()
            .map(|mut locks| {
                locks
                    .entry(sandbox.id.as_str().to_owned())
                    .or_default()
                    .clone()
            })
            .unwrap_or_default()
    }

    /// `POST /api/sandboxes/{sid}/hygiene/scan`.
    ///
    /// # Errors
    /// Access errors.
    pub async fn hygiene_scan_now(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
    ) -> AppResult<HygieneReport> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        Ok(self.hygiene_scan_locked(&sandbox).await)
    }

    /// `POST /api/sandboxes/{sid}/hygiene/cleanup`.
    ///
    /// # Errors
    /// Access errors, invalid requests.
    pub async fn hygiene_cleanup(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        request: &HygieneCleanup,
    ) -> AppResult<HygieneCleanupResult> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        self.hygiene_cleanup_as(&sandbox, request, &actor).await
    }
}
