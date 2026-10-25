//! Task records, views, archive/restore, feedback (SPEC-001 §5.4, ANL-002,
//! UI-006). Process lifecycle lives in the supervisor.

use std::sync::Arc;

use yulius_contracts::api::common::UserRef;
use yulius_contracts::api::tasks::{
    AttemptView, FeedbackView, Metrics, StepProgress, StepView, SwitchView, TaskDetail, TaskSummary,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{FeedbackRecord, MetricsRecord, StepRecord, TaskOrigin, TaskRecord};
use crate::ports::store::TaskFieldUpdate;

/// Maximum title length derived from a prompt.
const TITLE_MAX: usize = 80;

/// Derives a task title from its prompt: first non-empty line of its text (its attached files
/// left out, SPEC-124), trimmed to 80 characters.
#[must_use]
pub fn title_from_prompt(prompt: &str) -> String {
    let text = yulius_domain::request::Request::parse(prompt).text;
    let line = text
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

/// What an idle task's background work says about its working stretch (SPEC-087 TIM-002): the
/// task reads Working while background work it left will wake it, so the stretch goes on
/// (`Some(true)`); with none left it waits for a person or is done, so the stretch ends
/// (`Some(false)`). Not known yet (its turn just ended, or Yulius restarted) decides nothing
/// (`None`), and neither does a task that is not idle: its status and activity do (the store).
#[must_use]
pub fn idle_stretch(
    status: TaskStatus,
    activity: Option<Activity>,
    background: Option<u32>,
) -> Option<bool> {
    if status != TaskStatus::Running || !activity.is_some_and(Activity::is_idle) {
        return None;
    }
    background.map(|count| count > 0)
}

/// A working stretch that ended because its idle task had no background work left (SPEC-309):
/// its start, and the task's state time when it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndedStretch {
    /// The start of the stretch.
    pub since: Timestamp,
    /// The task's state time (TIM-001) when the stretch ended.
    pub state_at: Timestamp,
}

/// The start of the stretch a task continues when it goes on by itself (its own background work
/// woke it, or its session went on without a prompt) from the state that began at `state_at`:
/// the `ended` stretch's, when the task stayed in the state that stretch ended in (WSW-001).
/// After any change of its status or activity in between it starts a new one (WSW-002).
#[must_use]
pub fn continued_stretch(ended: Option<EndedStretch>, state_at: Timestamp) -> Option<Timestamp> {
    ended.filter(|e| e.state_at == state_at).map(|e| e.since)
}

impl crate::supervisor::TaskRuntime {
    /// The ended stretch this task continues from the state that began at `state_at`
    /// ([`continued_stretch`]). It stays remembered until its start is recorded
    /// ([`Self::forget_ended_stretch`]): a write the task's change refused leaves it for the
    /// turn the task goes on with (SPEC-315).
    pub(crate) fn ended_stretch_from(&self, state_at: Timestamp) -> Option<EndedStretch> {
        let ended = self.ended_stretch.lock().ok()?.take();
        ended.filter(|e| e.state_at == state_at)
    }

    /// Forgets `used` once its start is recorded, unless another stretch ended since.
    pub(crate) fn forget_ended_stretch(&self, _used: EndedStretch) {}
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
        effort: s.effort,
    }
}

/// Step progress from a step list (effort-weighted, STP-006).
#[must_use]
pub fn progress(steps: &[StepRecord]) -> StepProgress {
    crate::discipline::progress_view(steps)
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
        let account_label = match store.live_attempt(&task.id).await? {
            Some(attempt) => match attempt.account_id {
                Some(id) => store.account(&id).await?.map(|a| a.label),
                None => None,
            },
            None => None,
        };
        let (after, next) = self.chain_links(task).await;
        let times = store.task_times(&task.id).await?;
        let not_started =
            task.status == TaskStatus::Closed && !store.has_attempts(&task.id).await?;
        let awaits_start_onboarding = not_started
            && task.origin == TaskOrigin::Onboarding
            && store
                .sandbox(&task.sandbox_id)
                .await?
                .is_some_and(|sandbox| {
                    sandbox.onboarding_task_id.as_ref() == Some(&task.id)
                        && crate::onboarding::OnboardingState::parse(&sandbox.onboarding_state)
                            == Some(crate::onboarding::OnboardingState::Pending)
                });
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
            owner: UserRef {
                login: self.login_of(&task.owner).await,
            },
            control_request: match store.control_request(&task.id).await? {
                Some(request) => Some(yulius_contracts::api::tasks::ControlRequestView {
                    by: UserRef {
                        login: self.login_of(&request.by).await,
                    },
                    at: rfc3339(request.at),
                }),
                None => None,
            },
            created_at: rfc3339(task.created_at),
            updated_at: rfc3339(task.updated_at),
            state_since: times.map(|t| rfc3339(t.state_at)),
            working_since: times.and_then(|t| t.working_since).map(rfc3339),
            steps: progress(&steps),
            archived: task.archived_at.is_some(),
            account_label,
            // The sign-in email is for administrators (UI-022), and summaries go to everyone who
            // sees the task: only `task_detail` adds it, for an administrator (SPEC-244).
            account_email: None,
            mode: task.mode.as_str().to_owned(),
            resumes_at: self.capacity_returns_at(task).await.map(rfc3339),
            waits_for_account: task.status == TaskStatus::WaitingCapacity
                && self
                    .agent_accounts(task.harness.provider())
                    .await
                    .is_ok_and(|a| a != crate::account_gate::AgentAccounts::Usable),
            after,
            next,
            held_prompt: self.held_prompt_view(&task.id).await,
            // An ended task waits for nobody (ADR-0111).
            needs_you: if task.status.is_resting() {
                None
            } else {
                self.needs_you_view(&task.id).await
            },
            auto: self.auto_choice_view(&task.id).await,
            supertask: task.supertask,
            start_at: self.prepared_start_at(task).await.map(rfc3339),
            held_by: if task.status == TaskStatus::Queued {
                self.supertask_holders(task).await.unwrap_or_default()
            } else {
                Vec::new()
            },
            background_shells: (task.status == TaskStatus::Running
                && task.activity.is_some_and(Activity::is_idle))
            .then(|| self.background_shells_of(&task.id))
            .flatten(),
            background_shells_since: self
                .background_shells_since(&task.id)
                .map(|ms| rfc3339(yulius_domain::time::Timestamp(ms))),
            yulius_wait: task.status == TaskStatus::Running
                && task.activity.is_some_and(Activity::is_idle)
                && self.agent_waits_on_yulius(&task.id),
            not_started,
            awaits_start_onboarding,
            pause: self.pause_view(task).await,
        })
    }

    /// Publishes a task update to the UI.
    pub async fn publish_task(&self, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        self.follow_working_stretch(&task).await;
        if let Ok(summary) = self.task_summary(&task).await {
            self.events.publish(
                Some(task.sandbox_id.clone()),
                false,
                yulius_contracts::events::UiEvent::TaskUpdated {
                    task: Box::new(summary),
                },
            );
        }
    }

    /// Ends an idle task's working stretch once no background work is left, or keeps it while
    /// some is (SPEC-087 TIM-002). Every change of what the task shows is published, so this
    /// follows what people see. The write applies only while the task is still in the state it
    /// read (SPEC-315 FWS-001).
    async fn follow_working_stretch(&self, task: &TaskRecord) {
        let background = self.background_shells_of(&task.id);
        let Some(working) = idle_stretch(task.status, task.activity, background) else {
            return;
        };
        let Ok(Some(times)) = self.deps.store.task_times(&task.id).await else {
            return;
        };
        let next = match (working, times.working_since) {
            (false, Some(since)) => {
                // The shell's end wakes the agent a moment later: that turn continues it.
                if let Ok(mut ended) = self.runtime(&task.id).ended_stretch.lock() {
                    *ended = Some(EndedStretch {
                        since,
                        state_at: times.state_at,
                    });
                }
                None
            }
            (true, None) => Some(
                self.take_ended_stretch(&task.id, times.state_at)
                    .unwrap_or_else(|| self.now()),
            ),
            _ => return,
        };
        // Only the state this was decided on: a turn that started since keeps its stretch.
        if let Err(err) = self
            .deps
            .store
            .set_working_since_from(&task.id, (task.activity, times), next)
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "could not record the working stretch");
        }
    }

    /// The start of the stretch the task continues from the state that began at `state_at`
    /// ([`continued_stretch`]); the remembered stretch is used up either way.
    fn take_ended_stretch(&self, task_id: &TaskId, state_at: Timestamp) -> Option<Timestamp> {
        let ended = self
            .existing_runtime(task_id)?
            .ended_stretch
            .lock()
            .ok()?
            .take();
        continued_stretch(ended, state_at)
    }

    /// The task goes on by itself, not at a person's prompt: Claude Code's own wake-up (its
    /// background work ended), or its session went on after a stop. Called before it is made
    /// Working, it continues the stretch its lack of background work ended, when nothing changed
    /// since (SPEC-309 WSW-001), and returns that start for [`App::keep_working_stretch`]; the
    /// store keeps it when the task turns Working.
    pub(crate) async fn continue_working_stretch(&self, task_id: &TaskId) -> Option<Timestamp> {
        let times = self.deps.store.task_times(task_id).await.ok().flatten()?;
        if times.working_since.is_some() {
            return None;
        }
        let since = self.take_ended_stretch(task_id, times.state_at)?;
        if let Err(err) = self
            .deps
            .store
            .set_working_since(task_id, Some(since))
            .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not continue the working stretch");
        }
        Some(since)
    }

    /// After the task was made Working: the stretch [`App::continue_working_stretch`] continued
    /// stays, also when a publish between the two writes ended it again (its idle task had no
    /// background work), and the task is published again then.
    pub(crate) async fn keep_working_stretch(&self, task_id: &TaskId, since: Option<Timestamp>) {
        let Some(since) = since else { return };
        let working = matches!(
            self.deps.store.task(task_id).await,
            Ok(Some(task)) if task.status == TaskStatus::Running
                && task.activity == Some(Activity::Working)
        );
        let Ok(Some(times)) = self.deps.store.task_times(task_id).await else {
            return;
        };
        if !working || times.working_since == Some(since) {
            return;
        }
        if let Err(err) = self
            .deps
            .store
            .set_working_since(task_id, Some(since))
            .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not continue the working stretch");
            return;
        }
        self.publish_task(task_id).await;
    }

    /// The task `id` for `action` by `ctx`'s person: viewing needs access to its sandbox;
    /// anything more changes the task, which only its owner may (SPEC-070 R3): it acts on GitHub
    /// with their token.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` with insufficient access or for a non-owner.
    pub(crate) async fn load_task(
        &self,
        ctx: &AuthContext,
        id: &str,
        action: Action,
    ) -> AppResult<TaskRecord> {
        let id = TaskId::parse(id).map_err(|_| AppError::NotFound)?;
        let task = self.deps.store.task(&id).await?.ok_or(AppError::NotFound)?;
        self.require(&ctx.user, &task.sandbox_id, action).await?;
        if action != Action::View {
            self.require_owner(ctx, &task).await?;
        }
        Ok(task)
    }

    /// Refuses `ctx`'s person unless they own `task` (SPEC-070 R3).
    ///
    /// # Errors
    /// `Forbidden`, naming the owner and the way to control.
    pub(crate) async fn require_owner(
        &self,
        ctx: &AuthContext,
        task: &TaskRecord,
    ) -> AppResult<()> {
        if task.owner == ctx.user.id {
            return Ok(());
        }
        Err(AppError::Forbidden(format!(
            "Task #{} is @{}'s: it acts on GitHub as them. Ask them for control.",
            task.number,
            self.login_of(&task.owner).await
        )))
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
        let mut summary = self.task_summary(&task).await?;
        // Subscription emails are for administrators (UI-022, SPEC-244 SUB-005).
        let admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
        if admin
            && let Some(attempt) = store.live_attempt(&task.id).await?
            && let Some(id) = attempt.account_id
        {
            summary.account_email = store.account(&id).await?.and_then(|a| a.email);
        }
        let mut attempts = Vec::new();
        let mut wall_ms: i64 = 0;
        let now = self.now();
        for a in store.attempts(&task.id).await? {
            wall_ms =
                wall_ms.saturating_add(a.ended_at.unwrap_or(now).millis_since(a.started_at).max(0));
            let account = match &a.account_id {
                Some(id) => store.account(id).await?,
                None => None,
            };
            let (account_label, account_email) = account.map_or((None, None), |acc| {
                (Some(acc.label), acc.email.filter(|_| admin))
            });
            attempts.push(AttemptView {
                seq: a.seq,
                harness: a.harness.as_str().to_owned(),
                account_label,
                account_email,
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
        let started = !attempts.is_empty() || !store.provider_sessions(&task.id).await?.is_empty();
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
            request: task.prompt.clone(),
            started,
        })
    }

    /// Archives a resting task (UI-006), or a finished one (TSK-014): Done by the sidebar's rule
    /// (`task_is_done`: the goal achieved, or the plan complete with its pull request merged or
    /// tested, and no background shell), so nothing is left to wait for and it is closed as it
    /// is archived.
    ///
    /// # Errors
    /// `Conflict` for other live tasks; while a finished task's session is closing (it changed
    /// state meanwhile: "stop the task first"; still closing at the bound: "try again"; or its
    /// session could not be ended); and for the onboarding task its sandbox still waits for
    /// (SPEC-106 OTR-004).
    pub async fn archive_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        archive: bool,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let done = task.status == TaskStatus::Running && self.task_is_done(&task).await;
        if archive && done && task.origin == TaskOrigin::Onboarding {
            // Its run's end decides whether onboarding is over, which the check below reads.
            self.close_task_and_wait(&task.id).await?;
        } else if archive && done {
            self.close_task_now(&task.id).await?;
        } else if archive && !task.status.is_resting() {
            return Err(AppError::Conflict(
                "stop the task before archiving it".into(),
            ));
        }
        if archive {
            self.ensure_archive_allowed(&task).await?;
        }
        let now = self.now();
        let update = TaskFieldUpdate {
            archived_at: Some(archive.then_some(now)),
            ..TaskFieldUpdate::default()
        };
        self.deps.store.update_task(&task.id, &update, now).await?;
        self.publish_task(&task.id).await;
        if archive {
            // Its build output goes now, not in the next round (SPEC-276 RDA-008).
            let app = Arc::clone(self);
            let sandbox = task.sandbox_id.clone();
            tokio::spawn(async move {
                if let Ok(Some(record)) = app.deps.store.sandbox(&sandbox).await
                    && let Err(err) = app.disk_upkeep(&record, false).await
                {
                    tracing::warn!(sandbox = %sandbox, error = %err, "disk upkeep after archiving failed");
                }
            });
        }
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
    /// A finished task (`task_is_done`, as for archiving) is closed first, and deleted once its
    /// run has ended (SPEC-039).
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without access; `Conflict` while the task runs or starts, or
    /// while a finished task's session is still closing.
    pub async fn delete_task(self: &Arc<Self>, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let actor = ctx
            .user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into());
        self.delete_task_as(ctx, id, &actor, None).await
    }

    /// [`App::delete_task`] audited as `actor`; a management tool passes its caller and the
    /// caller's audit detail (SPEC-066 MGT-010).
    ///
    /// # Errors
    /// As [`App::delete_task`].
    pub(crate) async fn delete_task_as(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        actor: &str,
        caller_detail: Option<&serde_json::Value>,
    ) -> AppResult<()> {
        let mut task = self.load_task(ctx, id, Action::Work).await?;
        if task.status == TaskStatus::Running && self.task_is_done(&task).await {
            self.close_task_and_wait(&task.id).await?;
            task = self
                .deps
                .store
                .task(&task.id)
                .await?
                .ok_or(AppError::NotFound)?;
        }
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
        let deleted_in = task.sandbox_id.clone();
        if !self.deps.store.delete_task(&task.id).await? {
            return Err(AppError::NotFound);
        }
        // A start rule naming the deleted task now sees it as ended (SPEC-017), and a deleted
        // supertask no longer holds its project (SPEC-071).
        self.advance_rule_chains(&deleted_in).await;
        // A deleted onboarding task leaves its sandbox to start onboarding again (SPEC-106).
        if task.origin == TaskOrigin::Onboarding
            && let Err(err) = self.refresh_onboarding(&deleted_in).await
        {
            tracing::warn!(sandbox = %deleted_in, error = %err, "could not refresh onboarding");
        }
        if task.supertask {
            self.drain_queue().await;
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
        self.remove_task_attachments(&task.sandbox_id, &task.id);
        self.forget_runtime(&task.id);
        let named = format!("#{} {}", task.number, task.title);
        let detail = match caller_detail {
            Some(caller) => {
                let mut detail = caller.clone();
                detail["task"] = serde_json::json!(named);
                detail.to_string()
            }
            None => named,
        };
        let _ = self
            .deps
            .store
            .audit(
                self.now(),
                actor,
                "task.deleted",
                Some(task.id.as_str()),
                Some(&detail),
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
        // SPEC-124 INL-008: a file in the text, or on a line of its own, is not part of a title.
        assert_eq!(
            title_from_prompt("@/srv/a.png\nThis part @'/srv/b.png' looks wrong"),
            "This part looks wrong"
        );
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

    /// SPEC-087 TIM-002: background work left by an idle task keeps its working stretch (the task
    /// reads Working); none left ends it; not known yet changes nothing; a task that is not idle
    /// is decided by its status and activity alone.
    #[test]
    fn background_work_decides_an_idle_task_s_working_stretch() {
        let running = TaskStatus::Running;
        assert_eq!(
            idle_stretch(running, Some(Activity::Waiting), Some(1)),
            Some(true)
        );
        assert_eq!(
            idle_stretch(running, Some(Activity::Done), Some(0)),
            Some(false)
        );
        assert_eq!(idle_stretch(running, Some(Activity::Waiting), None), None);
        assert_eq!(
            idle_stretch(running, Some(Activity::Working), Some(0)),
            None
        );
        assert_eq!(
            idle_stretch(running, Some(Activity::NeedsAttention), Some(0)),
            None
        );
        assert_eq!(
            idle_stretch(TaskStatus::Queued, Some(Activity::Waiting), Some(0)),
            None
        );
    }

    /// SPEC-315: the stretch an idle task with background work continues stays remembered until
    /// its start is recorded; a write the task's change refused leaves it for the task's own
    /// wake-up, and only the stretch that was used is forgotten.
    #[test]
    fn an_ended_stretch_is_forgotten_only_once_recorded() {
        let runtime = crate::supervisor::TaskRuntime::new();
        let ended = EndedStretch {
            since: Timestamp(1_000),
            state_at: Timestamp(5_000),
        };
        let remembered = |runtime: &crate::supervisor::TaskRuntime| {
            runtime.ended_stretch.lock().ok().and_then(|slot| *slot)
        };
        *runtime.ended_stretch.lock().expect("slot") = Some(ended);
        assert_eq!(runtime.ended_stretch_from(Timestamp(6_000)), None);
        assert_eq!(runtime.ended_stretch_from(Timestamp(5_000)), Some(ended));
        // The write was refused: nothing is forgotten.
        assert_eq!(remembered(&runtime), Some(ended), "kept for the wake-up");
        // Another stretch ended since: the one used is not it.
        let later = EndedStretch {
            since: Timestamp(7_000),
            state_at: Timestamp(8_000),
        };
        *runtime.ended_stretch.lock().expect("slot") = Some(later);
        runtime.forget_ended_stretch(ended);
        assert_eq!(remembered(&runtime), Some(later));
        runtime.forget_ended_stretch(later);
        assert_eq!(remembered(&runtime), None, "recorded, so used up");
    }

    /// SPEC-309 WSW-001/WSW-002: a task that goes on by itself from the idle state its stretch
    /// ended in continues that stretch; after any change of state in between, or with none
    /// remembered, it starts a new one.
    #[test]
    fn an_ended_stretch_continues_only_from_the_same_idle_state() {
        let ended = EndedStretch {
            since: Timestamp(1_000),
            state_at: Timestamp(5_000),
        };
        assert_eq!(
            continued_stretch(Some(ended), Timestamp(5_000)),
            Some(Timestamp(1_000))
        );
        assert_eq!(continued_stretch(Some(ended), Timestamp(6_000)), None);
        assert_eq!(continued_stretch(None, Timestamp(5_000)), None);
    }
}
