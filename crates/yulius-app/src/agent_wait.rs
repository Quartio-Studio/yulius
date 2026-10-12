//! Agents wait without polling (SPEC-011): `delivery_merge` called while the checks of the
//! reviewed head still run arms a merge that Yulius performs as soon as they pass, and
//! `yulius wait` (the sandbox CLI, `POST /internal/agent/wait`) blocks until the checks at the
//! agent's head finish, its delivery moves or an armed merge is decided, and says how it was
//! (SPEC-028 MSR-003).
//!
//! Everything here reads Yulius's own records (CI runs, deliveries, the armed merge and its
//! outcome): waiting never costs a GitHub request.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_domain::ids::SandboxId;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskRecord;

/// How often a waiting request reads the state again.
const WAIT_POLL: Duration = Duration::from_secs(2);
/// Longest single wait request (the CLI asks again until its own limit).
pub const WAIT_MAX: Duration = Duration::from_secs(55);

/// A merge armed by `delivery_merge` while the checks of the reviewed head still ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArmedMerge {
    /// The reviewed head: only this exact commit is merged.
    pub head: String,
    /// The review summary given with it.
    pub summary: String,
}

/// How an armed merge ended (merged, refused or dropped), kept until the next one is armed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeOutcome {
    /// The head that was armed.
    pub head: String,
    /// One line for the agent: what happened and what to do next.
    pub text: String,
}

/// What `yulius wait` watches for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WaitState {
    /// The commit waited on (the agent's HEAD).
    pub head: String,
    /// The Yulius CI run at that head: `none`, `queued`, `running`, `passed`, `failed`,
    /// `error` or `cancelled`.
    pub checks: String,
    /// Why a run ended without passing, when it says.
    #[serde(default)]
    pub checks_detail: Option<String>,
    /// The task's delivery stage (`none`, `pr_open`, `merged_integration`, …).
    pub stage: String,
    /// A merge is armed for when the checks pass.
    pub merge_armed: bool,
    /// How the last armed merge ended (MSR-003), until another one is armed.
    #[serde(default)]
    pub merge_outcome: Option<String>,
    /// The head that armed merge was for.
    #[serde(default)]
    pub merge_outcome_head: Option<String>,
    /// Why Yulius could not push this head, with what to do (DLV-201).
    #[serde(default)]
    pub push_problem: Option<String>,
}

impl WaitState {
    /// Whether the checks at the head have ended (passed or not).
    #[must_use]
    pub fn checks_ended(&self) -> bool {
        matches!(
            self.checks.as_str(),
            "passed" | "failed" | "error" | "cancelled"
        )
    }
}

impl App {
    /// Arms a merge of `head` for when its checks pass (`delivery_merge` while they still run).
    pub(crate) fn arm_merge(&self, task: &TaskRecord, head: &str, summary: &str) {
        self.forget_merge_outcome(task);
        if let Ok(mut armed) = self.runtime(&task.id).armed_merge.lock() {
            *armed = Some(ArmedMerge {
                head: head.to_owned(),
                summary: summary.to_owned(),
            });
        }
    }

    /// Forgets how an earlier armed merge ended (a new `delivery_merge` call answers for itself).
    pub(crate) fn forget_merge_outcome(&self, task: &TaskRecord) {
        if let Ok(mut outcome) = self.runtime(&task.id).merge_outcome.lock() {
            *outcome = None;
        }
    }

    /// Keeps how the armed merge of `head` ended, for `yulius wait`.
    fn merge_decided(&self, task: &TaskRecord, head: &str, text: String) {
        if let Ok(mut outcome) = self.runtime(&task.id).merge_outcome.lock() {
            *outcome = Some(MergeOutcome {
                head: head.to_owned(),
                text,
            });
        }
    }

    /// Whether a merge is armed, counting one being decided right now (taken out of the slot
    /// while GitHub is asked): a wait must not read that gap as a decision.
    fn armed_merge(&self, task: &TaskRecord) -> bool {
        let runtime = self.runtime(&task.id);
        runtime.merge_deciding.load(Ordering::SeqCst) > 0
            || runtime
                .armed_merge
                .lock()
                .is_ok_and(|armed| armed.is_some())
    }

    /// Performs the armed merges of a sandbox whose checks passed (after every CI run and from
    /// the delivery loop). An armed merge is dropped when the branch moved on, the checks failed
    /// or the merge was refused; `yulius wait` says which.
    pub async fn merge_armed(self: &Arc<Self>, sandbox: &SandboxId) {
        let Ok(tasks) = self.deps.store.tasks(sandbox, false).await else {
            return;
        };
        for task in tasks {
            // Taken while it is decided, so a CI end and the delivery loop never merge twice; the
            // count keeps it armed for `yulius wait` until it is decided or put back.
            let runtime = self.runtime(&task.id);
            runtime.merge_deciding.fetch_add(1, Ordering::SeqCst);
            let taken = runtime
                .armed_merge
                .lock()
                .ok()
                .and_then(|mut armed| armed.take());
            if let Some(armed) = taken
                && let Some(kept) = self.merge_armed_task(&task, armed).await
                && let Ok(mut slot) = runtime.armed_merge.lock()
            {
                *slot = Some(kept);
            }
            runtime.merge_deciding.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Merges one armed task when its checks passed; returns the armed merge to keep waiting.
    async fn merge_armed_task(
        self: &Arc<Self>,
        task: &TaskRecord,
        armed: ArmedMerge,
    ) -> Option<ArmedMerge> {
        let head = match task.worktree_path.as_deref() {
            Some(worktree) => self
                .deps
                .git
                .log(std::path::Path::new(worktree), "HEAD", 1)
                .await
                .ok()
                .and_then(|log| log.into_iter().next())
                .map(|commit| commit.sha),
            None => None,
        };
        if head.as_deref() != Some(armed.head.as_str()) {
            self.merge_decided(
                task,
                &armed.head,
                format!(
                    "The merge you armed was dropped: you armed {} with `delivery_merge`, but the \
                     branch is now at {}. Get the independent review of the new head and call \
                     `delivery_merge` again.",
                    short(&armed.head),
                    head.as_deref().map_or("an unknown commit", short)
                ),
            );
            return None;
        }
        let user = match self.deps.store.user(&task.created_by).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => {
                self.merge_decided(
                    task,
                    &armed.head,
                    "The merge you armed was dropped: the task's creator can no longer act. \
                     Open an issue (`issue_create`) and end with your summary."
                        .to_owned(),
                );
                return None;
            }
        };
        let auth = AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
        };
        let id = task.id.to_string();
        let Ok(view) = self.task_delivery(&auth, &id).await else {
            return Some(armed);
        };
        let Some(pr) = view.pr.as_ref() else {
            return Some(armed);
        };
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            self.merge_decided(
                task,
                &armed.head,
                "The merge you armed was dropped: the project's sandbox could not be read. Check \
                 `delivery_status` and call `delivery_merge` again."
                    .to_owned(),
            );
            return None;
        };
        let require_checks = crate::delivery::delivery_profile(&sandbox).require_checks;
        let green = pr.checks == "success" || (pr.checks == "none" && !require_checks);
        if pr.checks == "failure" {
            self.merge_decided(
                task,
                &armed.head,
                format!(
                    "The merge you armed was dropped: the checks of pull request #{} failed at \
                     {}. Read why with `ci_status`, fix it test-first, get the review of the new \
                     head and call `delivery_merge` again.",
                    pr.number,
                    short(&armed.head)
                ),
            );
            return None;
        }
        if !green {
            return Some(armed);
        }
        let number = pr.number;
        let base = pr.base.clone();
        match self.merge_task_pr_for_task(&auth, &id).await {
            Ok(_) => {
                let detail = format!(
                    "merged at head {} reviewed independently ({}); armed while the checks ran",
                    armed.head,
                    if armed.summary.is_empty() {
                        "no summary"
                    } else {
                        armed.summary.as_str()
                    }
                );
                if let Err(err) = self
                    .deps
                    .store
                    .audit(
                        self.now(),
                        &format!("task:{}", task.number),
                        "delivery.agent.merge",
                        Some(&format!("{}#{}", sandbox.slug, task.number)),
                        Some(&detail),
                    )
                    .await
                {
                    tracing::warn!(task = %task.id, error = %err, "audit of an armed merge failed");
                }
                self.merge_decided(
                    task,
                    &armed.head,
                    format!(
                        "Yulius merged pull request #{number} into `{base}` at {}: its checks \
                         passed. Finish what the delivery profile still asks for this stage \
                         (`delivery_status`; deploy and verify an integration environment with \
                         `delivery_deploy`), then end with your summary. Do not promote unless \
                         the task asks for it.",
                        short(&armed.head)
                    ),
                );
                None
            }
            Err(err) => {
                self.merge_decided(
                    task,
                    &armed.head,
                    format!(
                        "The merge you armed was refused: the checks of pull request #{number} \
                         passed at {}, but {}. Fix that (`delivery_status` says what is missing) \
                         and call `delivery_merge` again.",
                        short(&armed.head),
                        err.public_message()
                    ),
                );
                None
            }
        }
    }

    /// The state `yulius wait` compares (local records only).
    ///
    /// # Errors
    /// `Unauthenticated` for an unknown task token; store failures.
    pub async fn agent_wait_state(&self, token: &str, head: Option<&str>) -> AppResult<WaitState> {
        let ctx = self.agent_context(token).await?;
        let task = &ctx.task;
        let store = &self.deps.store;
        let head = head
            .map(str::trim)
            .filter(|h| h.len() >= 7 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let (checks, checks_detail) = if head.is_empty() {
            ("none".to_owned(), None)
        } else {
            let runs = store.ci_runs(&task.sandbox_id, 50).await?;
            match runs.iter().find(|run| run.sha.starts_with(&head)) {
                Some(run) => (run.state.as_str().to_owned(), run.reason.clone()),
                None => ("none".to_owned(), None),
            }
        };
        let stage = store
            .delivery(&task.id)
            .await?
            .map_or_else(|| "none".to_owned(), |record| record.stage.encode());
        let outcome = self
            .runtime(&task.id)
            .merge_outcome
            .lock()
            .ok()
            .and_then(|outcome| outcome.clone());
        let push_problem = self
            .runtime(&task.id)
            .push_problem
            .lock()
            .ok()
            .and_then(|problem| problem.clone())
            .filter(|(at, _)| !head.is_empty() && (at.starts_with(&head) || head.starts_with(at)))
            .map(|(_, text)| text);
        Ok(WaitState {
            head,
            checks,
            checks_detail,
            stage,
            merge_armed: self.armed_merge(task),
            merge_outcome_head: outcome.as_ref().map(|o| o.head.clone()),
            merge_outcome: outcome.map(|o| o.text),
            push_problem,
        })
    }

    /// `POST /internal/agent/wait`: the state now when `since` is absent or already differs,
    /// else the first state that differs from it within `timeout` (at most [`WAIT_MAX`]), else
    /// the unchanged state.
    ///
    /// # Errors
    /// As [`App::agent_wait_state`].
    pub async fn agent_wait(
        &self,
        token: &str,
        head: Option<&str>,
        since: Option<&WaitState>,
        timeout: Duration,
    ) -> AppResult<WaitState> {
        let deadline = tokio::time::Instant::now() + timeout.min(WAIT_MAX);
        loop {
            let state = self.agent_wait_state(token, head).await?;
            if since.is_none_or(|since| *since != state) || tokio::time::Instant::now() >= deadline
            {
                return Ok(state);
            }
            tokio::time::sleep(WAIT_POLL).await;
        }
    }
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}
