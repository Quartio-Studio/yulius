//! Merges and CI order for a person (SPEC-487 slice 5, MRG-001 to MRG-004; wall 4): where a
//! task's merge stands and why a pull request that passed is not merging yet, the project's
//! merge queue, a task's pull request merged for its owner (or armed until its checks pass), and
//! a queued CI run put first in its project's queue.
//!
//! Contract:
//! - each is a person's function as a page's is: their [`AuthContext`], their access checked at
//!   the call (view to read, write to change), a task's merge by its owner only;
//! - a merge for a person is the page's Merge when the checks at the head passed; while they
//!   run it is armed at the branch's head, and the armed-merge pass merges that exact head once
//!   they pass, under the owner's authority, as it does for an agent's `delivery_merge`;
//! - the CI order changes one run's place: it goes before the other queued runs of its
//!   project; other projects' queues are left as they are (each project has lanes of its own).

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_domain::ci::RunState;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;

impl App {
    /// Where `task`'s merge stands (MRG-001): `armed` and, while it is, the head that is to
    /// merge, since when (`armed_at`, ms), whether Yulius is deciding it now and why it has not
    /// merged yet (`where`: its merge train's sentence, or the one its last decision left); and
    /// `last`, how the last armed merge ended (`head`, `text`).
    pub(crate) fn merge_state(&self, task: &TaskRecord) -> Value {
        let (merge, deciding) = self.armed_merge_view(&task.id);
        let (_, outcome) = self.armed_view(&task.id);
        let armed = merge.is_some() || deciding;
        let mut state = json!({"armed": armed});
        if let Some(merge) = merge {
            state["head"] = json!(merge.head);
            state["armed_at"] = json!(merge.armed_at);
        }
        if deciding {
            state["deciding"] = json!(true);
        }
        if let Some(note) = self.merge_queue_note(task) {
            state["where"] = json!(note);
        }
        if let Some((head, text)) = outcome {
            state["last"] = json!({"head": head, "text": text});
        }
        state
    }

    /// `App::merge_state` of task `id` for a person who can see its project.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn merge_state_for_person(&self, ctx: &AuthContext, id: &str) -> AppResult<Value> {
        let task = self.load_task(ctx, id, Action::View).await?;
        Ok(self.merge_state(&task))
    }

    /// The merge queue of `sandbox` (MRG-002): its armed merges in the order a merge train takes
    /// them (when they were armed), each with its task, owner, head and where it stands, and
    /// the merge train under test, if any.
    ///
    /// # Errors
    /// `NotFound` without view access; store failures.
    pub async fn merge_queue_for_person(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxId,
    ) -> AppResult<Value> {
        self.require_for(ctx, sandbox, Action::View).await?;
        let tasks = self.deps.store.tasks(sandbox, false).await?;
        let mut armed = Vec::new();
        for task in &tasks {
            let state = self.merge_state(task);
            if state["armed"] != true {
                continue;
            }
            let pull = self
                .deps
                .store
                .delivery(&task.id)
                .await?
                .and_then(|record| record.pr_number);
            let mut entry = json!({
                "number": task.number,
                "title": task.title,
                "owner": self.login_of(&task.owner).await,
                "pull_request": pull,
            });
            for key in ["head", "armed_at", "deciding", "where"] {
                if let Some(value) = state.get(key) {
                    entry[key] = value.clone();
                }
            }
            armed.push(entry);
        }
        armed.sort_by_key(|entry| entry["armed_at"].as_i64().unwrap_or(i64::MAX));
        let train = self.train_summary(sandbox).map(|mut train| {
            // Members and those left out by task number, as every tool names tasks.
            for key in ["members", "left_out"] {
                if let Some(list) = train.get_mut(key).and_then(Value::as_array_mut) {
                    for member in list {
                        let number = member["task"]
                            .as_str()
                            .and_then(|id| tasks.iter().find(|task| task.id.as_str() == id))
                            .map(|task| task.number);
                        member["task"] = json!(number);
                    }
                }
            }
            train
        });
        Ok(json!({"armed": armed, "train": train}))
    }

    /// Merges task `id`'s pull request for its owner (MRG-003), as the page's Merge does once
    /// the checks at its head passed; while they still run (or the branch is behind its base),
    /// arms the merge of the branch's head for them: Yulius merges that exact head once its
    /// checks pass, brings it up to date first when it can, and a new commit on the branch
    /// drops it. Armed already at that head, it keeps its place. Audited under the person's name.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Conflict` without an open pull request, when the
    /// checks at the head failed, or when the pull request goes elsewhere than the integration
    /// branch; the page's refusals.
    pub async fn merge_for_person(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<Value> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let record = self.deps.store.delivery(&task.id).await?;
        match record
            .as_ref()
            .map(|record| (record.stage.clone(), record.pr_number))
        {
            Some((DeliveryStage::PrOpen, Some(_))) => {}
            Some((stage, Some(number))) if stage != DeliveryStage::PrOpen => {
                return Err(AppError::Conflict(format!(
                    "task #{}'s pull request #{number} is not open ({}): nothing is left to merge",
                    task.number,
                    stage.wire_name()
                )));
            }
            _ => {
                return Err(AppError::Conflict(format!(
                    "task #{} has no pull request open: its agent opens one with its delivery, \
                     or the task's page does (Open pull request)",
                    task.number
                )));
            }
        }
        let worktree = task
            .worktree_path
            .as_deref()
            .map(std::path::Path::new)
            .ok_or_else(|| AppError::Conflict(format!("task #{} has no worktree", task.number)))?;
        let head = self
            .deps
            .git
            .log(worktree, "HEAD", 1)
            .await?
            .into_iter()
            .next()
            .map(|commit| commit.sha)
            .ok_or_else(|| {
                AppError::Conflict(format!("task #{}'s branch has no commit", task.number))
            })?;
        let before = self.task_delivery(ctx, id).await?;
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let profile = crate::delivery::delivery_profile(&sandbox);
        if let Some(pull) = before.pr.as_ref()
            && pull.base != profile.integration_branch
        {
            return Err(AppError::Conflict(format!(
                "pull request #{} goes into `{}`, not `{}`: a task's change merges into `{}` only",
                pull.number, pull.base, profile.integration_branch, profile.integration_branch
            )));
        }
        let checks = before
            .pr
            .as_ref()
            .map_or("none", |pull| pull.checks.as_str());
        let behind = !self
            .deps
            .git
            .is_ancestor(
                worktree,
                &format!("origin/{}", profile.integration_branch),
                "HEAD",
            )
            .await
            .unwrap_or(true);
        let merge_now =
            !behind && (checks == "success" || (checks == "none" && !profile.require_checks));
        if checks == "failure" {
            return Err(AppError::Conflict(format!(
                "the checks at task #{}'s head failed: nothing merges on red. Read why with \
                 ci_runs and ci_run; its agent fixes it, or run it again with ci_rerun when the \
                 code did not cause it",
                task.number
            )));
        }
        let login = self.login_of(&ctx.user.id).await;
        if merge_now {
            let view = self.merge_task_pr(ctx, id).await?;
            self.clear_armed(&task);
            self.audit_merge(&login, "delivery.person.merge", &task, &head)
                .await;
            return Ok(json!({
                "merged": true,
                "armed": false,
                "head": head,
                "delivery": crate::person_tools::record(&view)?,
            }));
        }
        let summary = format!("armed by @{login}, the task's owner");
        if !self.arm_merge(&task, &head, &head, &summary) {
            return Ok(json!({
                "merged": false,
                "armed": true,
                "head": head,
                "checks": checks,
                "note": "This head is armed already and keeps its place in the merge queue.",
            }));
        }
        self.audit_merge(&login, "delivery.person.merge_armed", &task, &head)
            .await;
        Ok(json!({
            "merged": false,
            "armed": true,
            "head": head,
            "checks": if behind { "behind" } else { checks },
            "note": "Yulius merges this exact head as soon as its checks pass (bringing the \
                     branch up to date first when it is behind); a new commit on the branch drops \
                     it. task_delivery and merge_queue say where it stands.",
        }))
    }

    async fn audit_merge(&self, login: &str, action: &str, task: &TaskRecord, head: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                login,
                action,
                Some(task.id.as_str()),
                Some(&json!({"head": head}).to_string()),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, action, "a merge was not audited");
        }
    }

    /// Puts queued CI run `run_id` of `sandbox` before its project's other queued runs
    /// (MRG-004): the next free lane of the project starts it. Audited under the person's name.
    ///
    /// # Errors
    /// `NotFound` for a run of another project or none; `Conflict` when it is not queued or is
    /// first already; `Forbidden` without write access.
    pub async fn ci_run_first(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxId,
        run_id: &str,
    ) -> AppResult<Value> {
        self.require_for(ctx, sandbox, Action::Work).await?;
        let store = &self.deps.store;
        let run = store
            .ci_run(run_id)
            .await?
            .filter(|run| run.sandbox_id == *sandbox)
            .ok_or(AppError::NotFound)?;
        if run.state != RunState::Queued {
            return Err(AppError::Conflict(format!(
                "run {run_id} is not queued ({}): only a run waiting for a lane has a place",
                run.state.as_str()
            )));
        }
        if !store.ci_run_to_front(run_id).await? {
            let still = store.ci_run(run_id).await?;
            return Err(AppError::Conflict(match still {
                Some(run) if run.state == RunState::Queued => {
                    format!("run {run_id} is first in this project's queue already")
                }
                _ => format!("run {run_id} left the queue meanwhile"),
            }));
        }
        if let Err(err) = store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "ci.run_prioritized",
                Some(sandbox.as_str()),
                Some(&json!({"run": run_id}).to_string()),
            )
            .await
        {
            tracing::warn!(run = run_id, error = %err, "a CI order was not audited");
        }
        let mut queued: Vec<(i64, String)> = store
            .ci_runs(sandbox, 200)
            .await?
            .into_iter()
            .filter(|run| run.state == RunState::Queued)
            .map(|run| (run.queued_at.0, run.id))
            .collect();
        queued.sort();
        Ok(json!({
            "run": run_id,
            "queue": queued.into_iter().map(|(_, id)| id).collect::<Vec<_>>(),
        }))
    }
}
