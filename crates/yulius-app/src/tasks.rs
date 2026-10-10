//! Task records, views, archive/restore, feedback and user messages (SPEC-001 §5.4, ANL-002,
//! UI-006). Process lifecycle lives in the supervisor.

use yulius_contracts::api::common::UserRef;
use yulius_contracts::api::tasks::{
    AttemptView, FeedbackView, MessageFrom, MessageView, Metrics, StepProgress, StepView,
    SwitchView, TaskDetail, TaskSummary,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{FeedbackRecord, InboxEntry, MetricsRecord, StepRecord, TaskRecord};
use crate::ports::store::TaskFieldUpdate;

/// Maximum title length derived from a prompt.
const TITLE_MAX: usize = 80;

/// Derives a task title from its prompt: first non-empty line, trimmed to 80 characters.
#[must_use]
pub fn title_from_prompt(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Untitled task");
    let mut title: String = line.chars().take(TITLE_MAX).collect();
    if line.chars().count() > TITLE_MAX {
        title.push('…');
    }
    title
}

/// Sums known metrics; a field stays unknown only when every run left it unknown.
#[must_use]
pub fn sum_metrics(records: &[MetricsRecord]) -> Metrics {
    fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (None, None) => None,
            (x, y) => Some(x.unwrap_or(0).saturating_add(y.unwrap_or(0))),
        }
    }
    let mut out = Metrics::default();
    for r in records {
        out.input_tokens = add(out.input_tokens, r.input_tokens);
        out.output_tokens = add(out.output_tokens, r.output_tokens);
        out.cache_read_tokens = add(out.cache_read_tokens, r.cache_read);
        out.cache_write_tokens = add(out.cache_write_tokens, r.cache_write);
        out.active_ms = add(out.active_ms, r.active_ms);
        out.turns = match (out.turns, r.turns) {
            (None, None) => None,
            (x, y) => Some(x.unwrap_or(0).saturating_add(y.unwrap_or(0))),
        };
        if let Some(model) = &r.model
            && !out.models.contains(model)
        {
            out.models.push(model.clone());
        }
    }
    out
}

fn step_view(s: &StepRecord) -> StepView {
    StepView {
        id: s.id.to_string(),
        ordinal: s.ordinal,
        title: s.title.clone(),
        status: s.status.clone(),
        note: s.note.clone(),
        updated_by: s.updated_by.clone(),
        updated_at: s.updated_at.map(rfc3339),
    }
}

/// Step progress from a step list.
#[must_use]
pub fn progress(steps: &[StepRecord]) -> StepProgress {
    let total = u32::try_from(steps.len()).unwrap_or(u32::MAX);
    let done = u32::try_from(
        steps
            .iter()
            .filter(|s| s.status == "done" || s.status == "skipped")
            .count(),
    )
    .unwrap_or(0);
    let current = steps
        .iter()
        .find(|s| s.status == "in_progress")
        .or_else(|| steps.iter().find(|s| s.status == "pending"))
        .map(|s| s.title.clone());
    StepProgress {
        done,
        total,
        current,
    }
}

impl App {
    /// Login to show for a user id (`superadmin` or the GitHub login).
    pub(crate) async fn login_of(&self, user: &yulius_domain::ids::UserId) -> String {
        match self.deps.store.user(user).await {
            Ok(Some(u)) => u.github_login.unwrap_or_else(|| "superadmin".into()),
            _ => "unknown".into(),
        }
    }

    /// Summary view of a task.
    ///
    /// # Errors
    /// Store failures.
    pub async fn task_summary(&self, task: &TaskRecord) -> AppResult<TaskSummary> {
        let store = &self.deps.store;
        let steps = store.steps(&task.id).await?;
        let unread = store.unread_count(&task.id).await?;
        let account_label = match store.live_attempt(&task.id).await? {
            Some(attempt) => match attempt.account_id {
                Some(id) => store.account(&id).await?.map(|a| a.label),
                None => None,
            },
            None => None,
        };
        let (after, next) = self.chain_links(task).await;
        Ok(TaskSummary {
            id: task.id.to_string(),
            sandbox_id: task.sandbox_id.to_string(),
            number: task.number,
            title: task.title.clone(),
            harness: task.harness.as_str().to_owned(),
            status: task.status.as_str().to_owned(),
            activity: task.activity.map(|a| a.as_str().to_owned()),
            branch: task.branch.clone(),
            origin: task.origin.as_str().to_owned(),
            created_by: UserRef {
                login: self.login_of(&task.created_by).await,
            },
            created_at: rfc3339(task.created_at),
            updated_at: rfc3339(task.updated_at),
            steps: progress(&steps),
            unread_messages: unread,
            archived: task.archived_at.is_some(),
            account_label,
            mode: task.mode.as_str().to_owned(),
            resumes_at: self.capacity_returns_at(task).await.map(rfc3339),
            after,
            next,
            held_prompt: self.held_prompt_view(&task.id).await,
        })
    }

    /// Publishes a task update to the UI.
    pub async fn publish_task(&self, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if let Ok(summary) = self.task_summary(&task).await {
            self.events.publish(
                Some(task.sandbox_id.clone()),
                false,
                yulius_contracts::events::UiEvent::TaskUpdated { task: summary },
            );
        }
    }

    pub(crate) async fn load_task(
        &self,
        ctx: &AuthContext,
        id: &str,
        action: Action,
    ) -> AppResult<TaskRecord> {
        let id = TaskId::parse(id).map_err(|_| AppError::NotFound)?;
        let task = self.deps.store.task(&id).await?.ok_or(AppError::NotFound)?;
        self.require(&ctx.user, &task.sandbox_id, action).await?;
        Ok(task)
    }

    /// Tasks of a sandbox (archived or active).
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn list_tasks(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        archived: bool,
    ) -> AppResult<Vec<TaskSummary>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let mut out = Vec::new();
        for task in self.deps.store.tasks(&sid, archived).await? {
            out.push(self.task_summary(&task).await?);
        }
        Ok(out)
    }

    /// Archived tasks across all visible sandboxes.
    ///
    /// # Errors
    /// Store failures.
    pub async fn archived_tasks(&self, ctx: &AuthContext) -> AppResult<Vec<TaskSummary>> {
        let mut out = Vec::new();
        for (sandbox, _) in self.visible_sandboxes(&ctx.user).await? {
            for task in self.deps.store.tasks(&sandbox.id, true).await? {
                out.push(self.task_summary(&task).await?);
            }
        }
        Ok(out)
    }

    /// Full task detail.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn task_detail(&self, ctx: &AuthContext, id: &str) -> AppResult<TaskDetail> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let store = &self.deps.store;
        let summary = self.task_summary(&task).await?;
        let mut attempts = Vec::new();
        let mut wall_ms: i64 = 0;
        let now = self.now();
        for a in store.attempts(&task.id).await? {
            wall_ms =
                wall_ms.saturating_add(a.ended_at.unwrap_or(now).millis_since(a.started_at).max(0));
            let label = match &a.account_id {
                Some(id) => store.account(id).await?.map(|acc| acc.label),
                None => None,
            };
            attempts.push(AttemptView {
                seq: a.seq,
                harness: a.harness.as_str().to_owned(),
                account_label: label,
                launch_kind: a.launch_kind.as_str().to_owned(),
                state: a.state.as_str().to_owned(),
                started_at: rfc3339(a.started_at),
                ended_at: a.ended_at.map(rfc3339),
                exit_code: a.exit_code,
                exit_reason: a.exit_reason.clone(),
            });
        }
        let mut metrics = sum_metrics(&store.task_metrics(&task.id).await?);
        metrics.wall_ms = u64::try_from(wall_ms).unwrap_or(0);
        let switches: Vec<SwitchView> = store
            .switches(&task.id)
            .await?
            .into_iter()
            .map(|s| SwitchView {
                at: rfc3339(s.at),
                reason: s.reason,
                from: s.from_desc,
                to: s.to_desc,
            })
            .collect();
        metrics.switches = u32::try_from(switches.len()).unwrap_or(u32::MAX);
        let mut feedback = Vec::new();
        for f in store.feedback(&task.id).await? {
            feedback.push(FeedbackView {
                login: self.login_of(&f.user_id).await,
                verdict: f.verdict,
                comment: f.comment,
                at: rfc3339(f.at),
            });
        }
        let steps = store.steps(&task.id).await?;
        Ok(TaskDetail {
            summary,
            spec_path: task.spec_path.clone(),
            worktree: task.worktree_path.clone(),
            attempts,
            steps_list: steps.iter().map(step_view).collect(),
            metrics,
            feedback,
            delivery: self.task_detail_delivery(&task, &ctx.user).await,
            switches,
        })
    }

    /// Archives a resting task (UI-006).
    ///
    /// # Errors
    /// `Conflict` for live tasks.
    pub async fn archive_task(
        &self,
        ctx: &AuthContext,
        id: &str,
        archive: bool,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        if archive && !task.status.is_resting() {
            return Err(AppError::Conflict(
                "stop the task before archiving it".into(),
            ));
        }
        let now = self.now();
        let update = TaskFieldUpdate {
            archived_at: Some(archive.then_some(now)),
            ..TaskFieldUpdate::default()
        };
        self.deps.store.update_task(&task.id, &update, now).await?;
        self.publish_task(&task.id).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// Deletes a task permanently (SPEC-001 TSK-009): write access, not live. Removes the
    /// local worktree and branch (uncommitted work included), run directories, archived
    /// transcripts and every record; remote branches and pull requests are untouched. Audited.
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without access; `Conflict` while the task runs or starts.
    pub async fn delete_task(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let starting = matches!(
            task.status,
            TaskStatus::Starting
                | TaskStatus::Running
                | TaskStatus::Switching
                | TaskStatus::Stopping
        );
        if starting || self.deps.store.live_attempt(&task.id).await?.is_some() {
            return Err(AppError::Conflict(
                "stop the task before deleting it".into(),
            ));
        }
        let attempts = self.deps.store.attempts(&task.id).await?;
        if let Some(path) = &task.worktree_path {
            let repo = self.sandbox_repo(&task.sandbox_id);
            let root = self.sandbox_dir(&task.sandbox_id).join("worktrees");
            if let Err(err) = self
                .deps
                .git
                .discard_worktree(
                    &repo,
                    std::path::Path::new(path),
                    &root,
                    task.branch.as_deref(),
                )
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "could not discard the task worktree");
            }
        }
        // Tasks waiting for this one need a person now (TSK-012).
        self.block_successors_deleted(&task).await;
        if !self.deps.store.delete_task(&task.id).await? {
            return Err(AppError::NotFound);
        }
        let runs = self.config.data_dir.join("runs");
        for attempt in attempts {
            let dir = std::path::PathBuf::from(&attempt.run_dir);
            if dir.starts_with(&runs) {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        let _ = std::fs::remove_dir_all(
            self.config
                .data_dir
                .join("transcripts")
                .join(task.id.as_str()),
        );
        self.forget_runtime(&task.id);
        let actor = ctx
            .user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into());
        let _ = self
            .deps
            .store
            .audit(
                self.now(),
                &actor,
                "task.deleted",
                Some(task.id.as_str()),
                Some(&format!("#{} {}", task.number, task.title)),
            )
            .await;
        self.events.publish(
            Some(task.sandbox_id.clone()),
            false,
            yulius_contracts::events::UiEvent::TaskRemoved {
                task_id: task.id.as_str().to_owned(),
            },
        );
        Ok(())
    }

    /// Records the caller's feedback (ANL-002). Read access suffices.
    ///
    /// # Errors
    /// `Invalid` verdict.
    pub async fn task_feedback(
        &self,
        ctx: &AuthContext,
        id: &str,
        verdict: &str,
        comment: &str,
    ) -> AppResult<()> {
        if verdict != "correct" && verdict != "incorrect" {
            return Err(AppError::Invalid(
                "verdict must be correct or incorrect".into(),
            ));
        }
        let task = self.load_task(ctx, id, Action::View).await?;
        let comment: String = comment.chars().take(2000).collect();
        self.deps
            .store
            .put_feedback(&FeedbackRecord {
                task_id: task.id,
                user_id: ctx.user.id.clone(),
                verdict: verdict.to_owned(),
                comment,
                at: self.now(),
            })
            .await
    }

    /// Converts a stored message to its API view.
    pub(crate) async fn message_view(&self, entry: &InboxEntry) -> AppResult<MessageView> {
        let m = &entry.message;
        let title = match (&m.from_task, &m.from_user) {
            (Some(t), _) => match self.deps.store.task(t).await? {
                Some(task) => format!("#{} {}", task.number, task.title),
                None => "task".into(),
            },
            (None, Some(u)) => self.login_of(u).await,
            (None, None) => "Yulius".into(),
        };
        let login = match &m.from_user {
            Some(u) => Some(self.login_of(u).await),
            None => None,
        };
        Ok(MessageView {
            id: m.id.to_string(),
            from: MessageFrom {
                task_id: m.from_task.as_ref().map(ToString::to_string),
                login,
                title,
            },
            to_task_id: m.to_task.as_ref().map(ToString::to_string),
            kind: m.kind.clone(),
            subject: m.subject.clone(),
            body: m.body.clone(),
            created_at: rfc3339(m.created_at),
            read: entry.read,
        })
    }

    /// Messages involving a task.
    ///
    /// # Errors
    /// Access errors.
    pub async fn task_messages(&self, ctx: &AuthContext, id: &str) -> AppResult<Vec<MessageView>> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let mut out = Vec::new();
        for entry in self.deps.store.task_messages(&task.id, 200).await? {
            out.push(self.message_view(&entry).await?);
        }
        Ok(out)
    }

    /// Whether a status counts toward "active" lists.
    #[must_use]
    pub fn is_active_status(status: TaskStatus) -> bool {
        !matches!(
            status,
            TaskStatus::Closed | TaskStatus::Stopped(_) | TaskStatus::Failed(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_from_prompts() {
        assert_eq!(
            title_from_prompt("\n  Make me an online store \nwith 3D"),
            "Make me an online store"
        );
        assert_eq!(title_from_prompt("   "), "Untitled task");
        let long = "x".repeat(100);
        assert_eq!(title_from_prompt(&long).chars().count(), TITLE_MAX + 1);
    }

    #[test]
    fn unknown_metrics_stay_unknown() {
        let unknown = MetricsRecord::default();
        let known = MetricsRecord {
            input_tokens: Some(10),
            model: Some("m".into()),
            ..MetricsRecord::default()
        };
        let sum = sum_metrics(std::slice::from_ref(&unknown));
        assert_eq!(sum.input_tokens, None);
        let sum = sum_metrics(&[unknown, known.clone(), known]);
        assert_eq!(sum.input_tokens, Some(20));
        assert_eq!(sum.output_tokens, None);
        assert_eq!(sum.models, vec!["m".to_owned()]);
    }
}
