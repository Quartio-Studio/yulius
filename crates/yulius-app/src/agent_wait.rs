//! Agents wait without polling (SPEC-011): `delivery_merge` called while the checks of the
//! reviewed head still run arms a merge that Yulius performs as soon as they pass, and
//! `yulius wait` (the sandbox CLI, `POST /internal/agent/wait`) blocks until the checks at the
//! agent's head finish, its delivery moves or Yulius sends it a notice.
//!
//! Everything here reads Yulius's own records (CI runs, deliveries, the inbox): waiting never
//! costs a GitHub request.

use std::sync::Arc;
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
    /// Yulius notices for the task the agent has not seen yet.
    pub unread: u32,
    /// Subject of the newest unread notice.
    #[serde(default)]
    pub latest_notice: Option<String>,
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
        if let Ok(mut armed) = self.runtime(&task.id).armed_merge.lock() {
            *armed = Some(ArmedMerge {
                head: head.to_owned(),
                summary: summary.to_owned(),
            });
        }
    }

    fn armed_merge(&self, task: &TaskRecord) -> bool {
        self.runtime(&task.id)
            .armed_merge
            .lock()
            .is_ok_and(|armed| armed.is_some())
    }

    /// Performs the armed merges of a sandbox whose checks passed (after every CI run and from
    /// the delivery loop). An armed merge is dropped, and the agent told, when the branch moved
    /// on, the checks failed or the merge was refused.
    pub async fn merge_armed(self: &Arc<Self>, sandbox: &SandboxId) {
        let Ok(tasks) = self.deps.store.tasks(sandbox, false).await else {
            return;
        };
        for task in tasks {
            // Taken while it is decided, so a CI end and the delivery loop never merge twice.
            let taken = self
                .runtime(&task.id)
                .armed_merge
                .lock()
                .ok()
                .and_then(|mut armed| armed.take());
            if let Some(armed) = taken
                && let Some(kept) = self.merge_armed_task(&task, armed).await
            {
                self.arm_merge(&task, &kept.head, &kept.summary);
            }
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
            self.tell_task(
                task,
                "The merge you armed was dropped: the branch moved on",
                format!(
                    "You armed the merge of {} with `delivery_merge`, but the branch is now at \
                     {}. Get the independent review of the new head and call `delivery_merge` \
                     again.",
                    short(&armed.head),
                    head.as_deref().map_or("an unknown commit", short)
                ),
            )
            .await;
            return None;
        }
        let user = match self.deps.store.user(&task.created_by).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => return None,
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
            return None;
        };
        let require_checks = crate::delivery::delivery_profile(&sandbox).require_checks;
        let green = pr.checks == "success" || (pr.checks == "none" && !require_checks);
        if pr.checks == "failure" {
            // The checks notice says why; the agent fixes and arms again.
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
                self.tell_task(
                    task,
                    "Yulius merged your pull request",
                    format!(
                        "Pull request #{number} is merged into `{base}` at {}: its checks passed, \
                         and you armed the merge with the review of that head. Finish what the \
                         delivery profile still asks for this stage (`delivery_status`; deploy \
                         and verify an integration environment with `delivery_deploy`), then end \
                         with your summary. Do not promote unless the task asks for it.",
                        short(&armed.head)
                    ),
                )
                .await;
                None
            }
            Err(err) => {
                self.tell_task(
                    task,
                    "Yulius could not merge your pull request",
                    format!(
                        "The checks of pull request #{number} passed at {}, but the merge you \
                         armed was refused: {}. Fix that (`delivery_status` says what is \
                         missing) and call `delivery_merge` again.",
                        short(&armed.head),
                        err.public_message()
                    ),
                )
                .await;
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
        // New notices: neither read (`messages_inbox`) nor delivered into the session's context
        // by a hook (the agent saw those).
        let unread: Vec<_> = store
            .inbox(&task.id, true, 50)
            .await?
            .into_iter()
            .filter(|entry| !entry.delivered)
            .collect();
        Ok(WaitState {
            head,
            checks,
            checks_detail,
            stage,
            merge_armed: self.armed_merge(task),
            unread: u32::try_from(unread.len()).unwrap_or(u32::MAX),
            latest_notice: unread.last().map(|entry| entry.message.subject.clone()),
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
            if since.is_none_or(|since| *since != state)
                || tokio::time::Instant::now() >= deadline
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
