//! Agent hook events (SPEC-002 §7.5, §7.6): briefing, the Stop discipline gate,
//! activity tracking, and the turn boundary where the subscription pool switches or pauses a
//! task (SPEC-002 ADR-17). A pause holds the hook ([`HookReply::Hold`]): the agent waits at its
//! boundary and is never told anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_contracts::hooks::{HookInput, HookOutput, events};
use yulius_domain::briefing::{
    BriefingInput, DEFAULT_BUDGET_BYTES, Foundations, MAX_RECENT_CHANGES, ParallelTask,
    RecentChange, TaskBrief, build_briefing,
};
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::spec_format::{DecisionFiles, NamedFile, decision_titles, project_layout};
use yulius_domain::spec_format::{DecisionsLocation, SpecPolicy, validate_spec_for};
use yulius_domain::task::{Activity, TaskStatus};

use crate::capacity::{CONTINUATION_MAX, HOLD_RETRY_SECS};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{PauseKind, TaskMode, TaskRecord};
use crate::ports::store::TaskFieldUpdate;

/// Consecutive Stop-hook blocks per turn before the task needs attention (AC-DSC-3).
pub const MAX_GATE_BLOCKS: u32 = 3;
/// Largest repository document read for the briefing or the gate.
const MAX_DOC_BYTES: u64 = 256 * 1024;
/// Largest number of decision records read.
const MAX_DECISION_FILES: usize = 200;

/// Joins a repository-relative path, refusing absolute paths and `..` components.
fn repo_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(rel))
}

/// Reads `root/relative` as a document of the repository: a regular file (not a link) whose
/// resolved path stays under `root`. Directories linked out of the repository are not followed,
/// and on Linux the opened file's real path is checked again, so a link swapped in after the
/// check cannot lead outside either (#70). These files are agent-written and read by the server
/// outside containment for the briefing, `project_context`, the Stop gate and the repository's
/// `.mcp.json` (SPEC-095). The file is opened without blocking, so a FIFO swapped in after the
/// check cannot hang the server, and read only when it is still a regular file.
pub(crate) fn read_repo_doc(root: &Path, relative: &str) -> Option<String> {
    use std::io::Read;
    let path = repo_path(root, relative)?;
    let canonical_root = std::fs::canonicalize(root).ok()?;
    if !std::fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let resolved = std::fs::canonicalize(&path).ok()?;
    if !resolved.starts_with(&canonical_root) {
        return None;
    }
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(&resolved)
            .ok()?
    };
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?;
        if !real.starts_with(&canonical_root) {
            return None;
        }
    }
    let mut text = String::new();
    file.take(MAX_DOC_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// Whether `root/relative` is a regular file of the repository (see [`read_repo_doc`]).
fn repo_doc_exists(root: &Path, relative: &str) -> bool {
    let Some(path) = repo_path(root, relative) else {
        return false;
    };
    std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
        && std::fs::canonicalize(root)
            .ok()
            .zip(std::fs::canonicalize(&path).ok())
            .is_some_and(|(root, path)| path.starts_with(root))
}

/// What a hook call answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookReply {
    /// Print this output (empty = no decision) and let the CLI go on.
    Output(HookOutput),
    /// Keep the CLI at this boundary: the hook client asks again after `retry_after_secs`
    /// (`202 Accepted` with `Retry-After`). Used while no subscription has capacity.
    Hold {
        /// Seconds until the hook client asks again.
        retry_after_secs: u64,
    },
}

fn hold() -> HookReply {
    HookReply::Hold {
        retry_after_secs: HOLD_RETRY_SECS,
    }
}

/// 256-colour text for the status line (the colours the web mock shows, HRN-005).
fn colour(code: u8, text: &str) -> String {
    format!("\x1b[38;5;{code}m{text}\x1b[39m")
}

impl App {
    /// Handles one hook event for the run owning `token`. Unknown events answer empty.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown or finished runs; store failures.
    pub async fn hook_event(
        self: &Arc<Self>,
        token: &str,
        harness: &str,
        event: &str,
        input: &HookInput,
    ) -> AppResult<HookReply> {
        if harness != "claude" {
            return Err(AppError::NotFound);
        }
        let ctx = self.agent_context(token).await?;
        // This run reports its turn boundaries through hooks: the Stop hook decides switches
        // and pauses, not the transcript.
        self.runtime(&ctx.task.id)
            .hooks_seen
            .store(true, Ordering::SeqCst);
        // The session's first message Yulius types (SPEC-068): its submit reached Yulius, even if
        // held below; any Stop means the session took a prompt, so nothing of it is pending.
        match event {
            events::USER_PROMPT_SUBMIT => crate::supervisor::opening_entered(&ctx.attempt.run_dir),
            events::STOP => crate::supervisor::opening_sent(&ctx.attempt.run_dir),
            _ => {}
        }
        // An instance update pauses every task at its turn boundary, as a capacity pause does:
        // the agent waits here and is never told (SPEC-002 ADR-18).
        if matches!(event, events::STOP | events::USER_PROMPT_SUBMIT)
            && self.hold_for_update(&ctx.task.id)
        {
            return Ok(HookReply::Hold {
                retry_after_secs: crate::updates::HOLD_RETRY_SECS,
            });
        }
        match event {
            events::SESSION_START => Ok(HookReply::Output(
                self.hook_session_start(&ctx, input).await?,
            )),
            events::USER_PROMPT_SUBMIT => self.hook_prompt_submit(&ctx, input).await,
            events::STOP => self.hook_stop(&ctx, input).await,
            events::NOTIFICATION => {
                // Only a notice that asks a person to act needs one: Claude Code's reminder that
                // it waits for input ("idle", about a minute after a turn ended) keeps a finished
                // task waiting or done, so chains and onboarding see it as finished
                // (F 2026-10-11).
                if notification_needs_a_person(input) {
                    self.set_task_activity(&ctx.task, Activity::NeedsAttention)
                        .await?;
                    // The agent's next work shows the person answered (STA-003), unless the
                    // task was flagged already (by the gate, say) or a worker asks, which the
                    // main session's work says nothing about.
                    if ctx.task.activity != Some(Activity::NeedsAttention)
                        && notice_answered_by_work(input)
                    {
                        self.runtime(&ctx.task.id)
                            .notice_attention_ms
                            .store(self.now().0.max(1), Ordering::SeqCst);
                    }
                }
                Ok(HookReply::Output(HookOutput::default()))
            }
            events::SESSION_END => {
                self.session_ending(&ctx, input.reason.as_deref());
                Ok(HookReply::Output(HookOutput::default()))
            }
            _ => Ok(HookReply::Output(HookOutput::default())),
        }
    }

    /// A held hook answer for `task`, remembering when the hold was last asked about: a hold
    /// nobody asks about any more was given up by the CLI (ADR-17, [`Self::resume_round`]).
    fn hold_for(&self, task: &yulius_domain::ids::TaskId) -> HookReply {
        self.runtime(task)
            .hold_polled_ms
            .store(self.now().0, Ordering::SeqCst);
        hold()
    }

    /// `SessionEnd` (F 2026-10-10: the task follows its session): Claude Code says why the
    /// session ends before its process exits. Leaving it (`/exit`, Ctrl+C twice, `/logout`)
    /// makes the coming exit a normal close whatever its code; `/clear` keeps the process with
    /// a new conversation, so an achieved goal no longer counts (the new session is adopted into
    /// the lineage by the supervisor's drift check).
    fn session_ending(&self, ctx: &AgentContext, reason: Option<&str>) {
        let runtime = self.runtime(&ctx.task.id);
        let reason = reason
            .filter(|r| !r.is_empty() && r.len() <= 64)
            .unwrap_or("other");
        tracing::info!(task = %ctx.task.id, %reason, "the agent session ends");
        if reason == yulius_contracts::hooks::session_end::CLEAR {
            runtime.goal_achieved.store(false, Ordering::SeqCst);
            return;
        }
        if let Ok(mut end) = runtime.session_end.lock() {
            *end = Some(reason.to_owned());
        }
    }

    /// Claude Code's status line of a run (`yulius hook claude StatusLine`): the repository and
    /// the task's branch, "→ studio git:(feat/12-checkout)" (HRN-005); and the subscription's
    /// plan usage it carries — read by Claude Code from its own API responses, the only usage a
    /// `claude setup-token` sign-in can see (ADR-17).
    ///
    /// # Errors
    /// `NotFound` for another harness, `Unauthenticated` for unknown or finished runs.
    pub async fn status_line(
        self: &Arc<Self>,
        token: &str,
        harness: &str,
        input: &yulius_contracts::hooks::StatusLineInput,
    ) -> AppResult<String> {
        if harness != "claude" {
            return Err(AppError::NotFound);
        }
        let ctx = self.agent_context(token).await?;
        if let (Some(limits), Some(account)) = (&input.rate_limits, &ctx.attempt.account_id) {
            self.session_usage(account, limits).await;
        }
        let sandbox = self.deps.store.sandbox(&ctx.task.sandbox_id).await?;
        let repository = sandbox.map_or_else(|| "repository".to_owned(), |s| s.repo_name);
        let branch = ctx.task.branch.clone().unwrap_or_else(|| "…".into());
        Ok(format!(
            "{} {} {}{}{}",
            colour(114, "→"),
            colour(80, &repository),
            colour(75, "git:("),
            colour(203, &branch),
            colour(75, ")")
        ))
    }

    pub(crate) async fn set_task_activity(
        &self,
        task: &TaskRecord,
        activity: Activity,
    ) -> AppResult<()> {
        if task.status != TaskStatus::Running || task.activity == Some(activity) {
            return Ok(());
        }
        if activity != Activity::NeedsAttention {
            self.runtime(&task.id)
                .notice_attention_ms
                .store(0, Ordering::SeqCst);
        }
        self.deps
            .store
            .set_activity(&task.id, Some(activity), self.now())
            .await?;
        self.publish_task(&task.id).await;
        Ok(())
    }

    async fn hook_session_start(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookOutput> {
        if ctx.attempt.provider_session_id.is_none()
            && let Some(session) = input.session_id.as_deref()
            && !session.is_empty()
            && session.len() <= 128
        {
            self.deps
                .store
                .set_attempt_session(&ctx.attempt.id, session)
                .await?;
        }
        if let Err(err) = self.sync_steps_from_spec(&ctx.task).await {
            tracing::warn!(error = %err, task = %ctx.task.id, "step sync from the spec failed");
        }
        // The prompt box reads input from now on: the goal can be typed (TSK-010).
        self.runtime(&ctx.task.id)
            .session_started
            .store(true, Ordering::SeqCst);
        let briefing = self.briefing(&ctx.task, ctx.attempt.harness).await?;
        Ok(HookOutput::additional_context(
            events::SESSION_START,
            briefing,
        ))
    }

    /// The session briefing for a task (SPEC-002 §7.6, COM-004, DSC-009).
    ///
    /// # Errors
    /// Store failures.
    pub async fn briefing(
        &self,
        task: &TaskRecord,
        harness: yulius_domain::harness::Harness,
    ) -> AppResult<String> {
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let root = task
            .worktree_path
            .as_ref()
            .map_or_else(|| self.sandbox_repo(&task.sandbox_id), PathBuf::from);
        let profile = crate::delivery::delivery_profile(&sandbox);
        let root_for_read = root.clone();
        let repo = tokio::task::spawn_blocking(move || RepoDocs::read(&root_for_read))
            .await
            .map_err(AppError::internal)?;

        let (_, steps) = self.step_states_of(&task.id).await?;
        let workflow = self.task_workflow(task).await.ok().flatten();
        let spec_policy = SpecPolicy::from_workflow(workflow.as_ref().map(|w| w.require_spec));
        let all = store.tasks(&task.sandbox_id, false).await?;
        let claims = store.active_claims(&task.sandbox_id).await?;
        let mut recent: Vec<&TaskRecord> = all
            .iter()
            .filter(|t| t.status == TaskStatus::Closed && t.id != task.id && t.closed_at.is_some())
            .collect();
        recent.sort_by_key(|t| std::cmp::Reverse(t.closed_at));
        let recent_changes = recent
            .into_iter()
            .take(MAX_RECENT_CHANGES)
            .map(|t| RecentChange {
                title: t.title.clone(),
                summary: t
                    .prompt
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect(),
                at: t.closed_at.unwrap_or(t.updated_at),
            })
            .collect();
        let parallel_tasks = all
            .iter()
            .filter(|t| t.id != task.id && t.status.is_live())
            .map(|t| ParallelTask {
                number: t.number,
                title: t.title.clone(),
                harness: t.harness,
                status: t.status,
                branch: t.branch.clone().unwrap_or_default(),
                scopes: claims
                    .iter()
                    .filter(|c| c.task_id == t.id)
                    .map(|c| c.pattern.clone())
                    .collect(),
            })
            .collect();
        let decisions = repo.decision_files();
        let input = BriefingInput {
            project_name: if sandbox.is_playground() {
                format!("{} (playground)", sandbox.repo_name)
            } else {
                format!("{}/{}", sandbox.repo_owner, sandbox.repo_name)
            },
            harness,
            task: TaskBrief {
                id: task.id.clone(),
                number: task.number,
                title: task.title.clone(),
                goal: task.prompt.clone(),
                spec_path: task.spec_path.clone().unwrap_or_default(),
                steps,
                branch: task.branch.clone().unwrap_or_default(),
                workflow: workflow.map(|w| w.name),
                spec_policy,
            },
            profile: &profile,
            layout: &repo.layout,
            product_head: repo.product.as_deref(),
            architecture_present: repo.architecture_present,
            decision_titles: decision_titles(&decisions),
            changes_head: repo.changes.as_deref(),
            recent_changes,
            parallel_tasks,
            port_base: sandbox.port_base,
            port_count: sandbox.port_count,
            repo_instructions: repo.instructions.clone(),
            foundations: repo.foundations.clone(),
            now: self.now(),
            playground: sandbox.is_playground(),
        };
        let workflow = self.workflow_instructions(task).await.map(|text| {
            let clipped: String = text.chars().take(3_000).collect();
            // The workflows describe GitHub delivery; a playground has only the local merge.
            let note = if sandbox.is_playground() {
                "\nIn this playground, delivery is `delivery_merge` alone: no pull request, CI, \
                 deploy or promotion.\n"
            } else {
                ""
            };
            format!("\n\n## Workflow instructions\n\n{clipped}\n{note}")
        });
        let chain = self.chain_briefing(task).await;
        let budget = DEFAULT_BUDGET_BYTES
            .saturating_sub(workflow.as_ref().map_or(0, String::len))
            .saturating_sub(chain.as_ref().map_or(0, String::len));
        let mut briefing = build_briefing(&input, budget);
        if let Some(workflow) = workflow {
            briefing.push_str(&workflow);
        }
        if let Some(chain) = chain {
            briefing.push_str(&chain);
        }
        Ok(briefing)
    }

    async fn hook_prompt_submit(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookReply> {
        let store = &self.deps.store;
        // A prompt held for capacity stays held while its pause lasts (also across a restart).
        let refreshed;
        let ctx = match store.open_pause(&ctx.task.id).await? {
            Some(pause) if pause.kind == PauseKind::Prompt => {
                return Ok(self.hold_for(&ctx.task.id));
            }
            // A held turn the user interrupted to type something: the new prompt replaces it,
            // and the task runs again until this prompt is held or goes on.
            Some(_) => {
                self.hold_ended(&ctx.task.id, "superseded", Activity::Waiting)
                    .await;
                refreshed = AgentContext {
                    attempt: ctx.attempt.clone(),
                    task: store.task(&ctx.task.id).await?.ok_or(AppError::NotFound)?,
                };
                &refreshed
            }
            None => ctx,
        };
        // Scope discipline (TSK-013): a held prompt never reaches the agent, so nothing below
        // (capacity, activity) happens for it. A prompt typed into a task held
        // at its turn end (paused for capacity) is checked like any other.
        if let Some(held) = self.screen_prompt(ctx, input).await? {
            return Ok(HookReply::Output(held));
        }
        // The start of a turn: with no subscription below the threshold, the prompt waits here
        // until capacity returns (ROT-010). A prompt too long to carry to another account runs;
        // a limit the CLI reports pauses it then.
        let carried = input
            .prompt
            .as_deref()
            .filter(|p| !p.trim().is_empty() && p.len() <= CONTINUATION_MAX);
        if carried.is_some() || input.prompt.is_none() {
            let current = ctx.attempt.account_id.clone();
            let pool = self.rotation_input(&ctx.task, current.clone()).await?;
            if let (Decision::Wait { .. }, _) = rotation::decide(&pool, Trigger::Proactive) {
                self.pause_task(
                    &ctx.task.id,
                    PauseKind::Prompt,
                    current,
                    carried.map(str::to_owned),
                )
                .await?;
                return Ok(self.hold_for(&ctx.task.id));
            }
        }
        // The session took its first prompt: a first message Yulius typed is no longer pending
        // (SPEC-068).
        crate::supervisor::opening_sent(&ctx.attempt.run_dir);
        // A typed prompt starts new work: an achieved goal no longer makes the task done
        // (TSK-014). Claude Code's own wake-ups (a background shell ended) keep it.
        if crate::finish::is_typed_prompt(input.prompt.as_deref()) {
            self.prompt_submitted(&ctx.task.id);
        }
        // `/compact` runs no turn and no Stop hook ends it: the task stays as it was (SPEC-085
        // STT-005).
        if !crate::finish::prompt_starts_work(input.prompt.as_deref()) {
            return Ok(HookReply::Output(HookOutput::default()));
        }
        if ctx.task.gate_blocks != 0 {
            let update = TaskFieldUpdate {
                gate_blocks: Some(0),
                ..TaskFieldUpdate::default()
            };
            store.update_task(&ctx.task.id, &update, self.now()).await?;
        }
        self.runtime(&ctx.task.id)
            .resumed_turn
            .store(false, Ordering::SeqCst);
        self.set_task_activity(&ctx.task, Activity::Working).await?;
        Ok(HookReply::Output(HookOutput::default()))
    }

    /// The discipline gate's reasons at a turn end (the spec gate).
    async fn stop_reasons(&self, task: &TaskRecord) -> AppResult<Vec<String>> {
        Ok(self.spec_gate(task).await?.into_iter().collect())
    }

    /// The prompt for a session resumed on another subscription after a held turn: the gate's
    /// reasons when the gate would have kept the agent working, else none (a goal-mode session
    /// gets its goal again when it launches).
    pub(crate) async fn stop_continuation(&self, task: &TaskRecord) -> Option<String> {
        if task.gate_blocks >= MAX_GATE_BLOCKS {
            return None;
        }
        let reasons = self.stop_reasons(task).await.ok()?;
        if reasons.is_empty() {
            return None;
        }
        Some(reasons.join("\n\n"))
    }

    async fn hook_stop(
        self: &Arc<Self>,
        ctx: &AgentContext,
        _input: &HookInput,
    ) -> AppResult<HookReply> {
        let store = &self.deps.store;
        let mut task = &ctx.task;
        // A turn held for capacity stays held while its pause lasts (also across a restart).
        let refreshed;
        if let Some(pause) = store.open_pause(&task.id).await? {
            if matches!(pause.kind, PauseKind::Turn | PauseKind::Limit) {
                return Ok(self.hold_for(&task.id));
            }
            // A turn ended while its prompt was held: the CLI gave that hold up and the turn ran.
            self.hold_ended(&task.id, "superseded", Activity::Working)
                .await;
            refreshed = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
            task = &refreshed;
        }
        // The turn on a goal session's first message (its attached files) ended, not held: it
        // stops as it is, without gates or rotation, so the goal is typed next (SPEC-025).
        let opening = self
            .runtime(&task.id)
            .opening_turn
            .swap(false, Ordering::SeqCst);
        if opening && task.mode == TaskMode::Goal {
            return Ok(HookReply::Output(HookOutput::default()));
        }
        if let Err(err) = self.sync_steps_from_spec(task).await {
            tracing::warn!(error = %err, task = %task.id, "step sync from the spec failed");
        }
        let reasons = self.stop_reasons(task).await?;
        let gate_continues = !reasons.is_empty() && task.gate_blocks < MAX_GATE_BLOCKS;
        // Would the agent work on after this boundary (a gate block, or its goal)?
        let continues = gate_continues || task.mode == TaskMode::Goal;

        // The turn boundary: stay, move to another subscription, or pause (ROT-003, ROT-010).
        self.runtime(&task.id)
            .proactive_switch
            .store(false, Ordering::SeqCst);
        let current = ctx.attempt.account_id.clone();
        let pool = self.rotation_input(task, current.clone()).await?;
        let decision = rotation::decide(&pool, Trigger::Proactive);
        // A switch restarts the session and ends its background shells, agents and monitors: a
        // build, gate or review the agent left running would be lost (tasks #3 and #5, F
        // 2026-10-11/12, #271). An agent launched in this turn is known only from the lines
        // written before this Stop: they are read first (SPEC-079 BGW-004).
        let busy =
            if matches!(decision.0, Decision::Switch { .. }) && !self.switch_cooling(&task.id) {
                self.await_transcript_read(&task.id).await;
                self.shells_running_now(&task.id).await
            } else {
                false
            };
        let stays = match decision {
            (Decision::Stay, _) => true,
            (Decision::Switch { .. }, _) if busy => true,
            // A switch was refused moments ago (the new session could not start): the agent
            // keeps working on its current subscription instead of being held.
            (Decision::Switch { .. }, _) if self.switch_cooling(&task.id) => true,
            (Decision::Switch { account }, reason) => {
                // The agent waits here while it moves; the resumed session gets the gate's
                // reasons as its prompt when the gate would have kept it working.
                let continuation = gate_continues.then(|| reasons.join("\n\n"));
                let app = Arc::clone(self);
                let task_id = task.id.clone();
                let reason = format!("{reason:?}");
                tokio::spawn(async move {
                    if let Err(err) = app
                        .switch_task_now(&task_id, Some(account), &reason, continuation)
                        .await
                    {
                        tracing::warn!(error = %err, task = %task_id, "switch at the turn boundary failed");
                    }
                });
                return Ok(self.hold_for(&task.id));
            }
            (Decision::Wait { .. }, _) if continues => {
                self.pause_task(&task.id, PauseKind::Turn, current, None)
                    .await?;
                return Ok(self.hold_for(&task.id));
            }
            // The agent goes idle anyway; its next prompt is held if capacity is still gone.
            (Decision::Wait { .. }, _) => false,
        };

        // A clean turn end on the same subscription may carry one gentle step nudge (STP-008).
        // It is not a gate block: it is given once and never escalates to "needs attention".
        if reasons.is_empty() && stays {
            match self.take_turn_end_nudge(task).await {
                Ok(Some(text)) => return Ok(HookReply::Output(HookOutput::block(text))),
                Ok(None) => {}
                Err(err) => tracing::warn!(error = %err, task = %task.id, "step nudge unavailable"),
            }
        }
        if reasons.is_empty() {
            if task.gate_blocks != 0 {
                let update = TaskFieldUpdate {
                    gate_blocks: Some(0),
                    ..TaskFieldUpdate::default()
                };
                store.update_task(&task.id, &update, self.now()).await?;
            }
            // Done when the goal is achieved, waiting otherwise (TSK-014), once the goal verdict
            // came while a goal is open (SPEC-079 BGW-005).
            self.clean_stop(task).await?;
            return Ok(HookReply::Output(HookOutput::default()));
        }

        if task.gate_blocks >= MAX_GATE_BLOCKS {
            self.set_task_activity(task, Activity::NeedsAttention)
                .await?;
            self.runtime(&task.id).hub.notice(format!(
                "Yulius: the agent stopped {MAX_GATE_BLOCKS} times without fixing: {}",
                reasons.join(" ").chars().take(300).collect::<String>()
            ));
            return Ok(HookReply::Output(HookOutput::default()));
        }
        let update = TaskFieldUpdate {
            gate_blocks: Some(task.gate_blocks + 1),
            ..TaskFieldUpdate::default()
        };
        store.update_task(&task.id, &update, self.now()).await?;
        self.publish_task(&task.id).await;
        Ok(HookReply::Output(HookOutput::block(reasons.join("\n\n"))))
    }

    /// A turn of a run without hooks ended (seen in its transcript): run a pending proactive
    /// rotation at this boundary (SPEC-002 §6.3).
    pub(crate) fn turn_ended(
        self: &Arc<Self>,
        task_id: &yulius_domain::ids::TaskId,
        account: Option<yulius_domain::ids::AccountId>,
    ) {
        let runtime = self.runtime(task_id);
        if runtime.proactive_switch.swap(false, Ordering::SeqCst) {
            let app = Arc::clone(self);
            let task_id = task_id.clone();
            tokio::spawn(async move {
                app.rotate(&task_id, account, Trigger::Proactive).await;
            });
        }
    }

    /// The discipline gate on the task spec: `Some(reason)` when the worktree has changes, the
    /// task needs a spec (its workflow, or a change beyond documentation, DSC-013) and the spec
    /// is missing or invalid.
    async fn spec_gate(&self, task: &TaskRecord) -> AppResult<Option<String>> {
        let Some(worktree) = task.worktree_path.as_ref() else {
            return Ok(None);
        };
        let worktree = PathBuf::from(worktree);
        if !self.branch_changed(task).await? || !self.spec_required(task).await? {
            return Ok(None);
        }
        // A required spec that does not exist yet blocks the turn here, not only the pull
        // request later (F 2026-10-11: features finished without one, and nothing opened); a
        // renamed spec is followed by its frontmatter.
        let spec_path = match self.task_spec_path(task, &worktree).await {
            Ok(path) => path,
            Err(dir) => {
                let place = task
                    .spec_path
                    .as_deref()
                    .map_or_else(|| format!("under `{dir}`"), |path| format!("at `{path}`"));
                return Ok(Some(format!(
                    "This task needs a spec (a feature, a fix, or a change beyond documentation) \
                     and none exists yet. Write it {place} in the Yulius format with \
                     `yulius-task: {}` in its frontmatter (spec_check validates it), keep its \
                     Plan in your steps, commit it and carry on. Decide its content yourself from \
                     the request and the code; do not ask anyone.",
                    task.id
                )));
            }
        };
        let spec_path = &spec_path;
        if repo_path(&worktree, spec_path).is_none() {
            return Ok(Some(format!(
                "The task spec path `{spec_path}` is not a repository path."
            )));
        }
        let expected = task.id.clone();
        let spec_path = spec_path.clone();
        tokio::task::spawn_blocking(move || match read_repo_doc(&worktree, &spec_path) {
            None => Some(format!(
                "This task needs a spec (a feature or a fix, or a change beyond documentation) but `{spec_path}` does not exist. Create it in the Yulius format (frontmatter with `yulius-task: {expected}`, the required sections and a Plan checklist of similar-effort steps) before finishing."
            )),
            Some(text) => {
                let report = validate_spec_for(&text, &expected);
                (!report.is_valid()).then(|| report.agent_message(&spec_path))
            }
        })
        .await
        .map_err(AppError::internal)
    }
}

impl App {
    /// Keeps the task's steps in sync with the Plan checklist of its spec (STP-004): the
    /// top-level plan items (at most [`crate::coordination::MAX_STEPS`]) become the steps in
    /// order; checked items are `done`; unchecked items keep the status and effort of the
    /// existing step with the same title (agents may mark them `in_progress` before ticking the
    /// box), so unticking never reopens a finished step; new items start `pending`. Writes only
    /// on change. Returns whether the steps changed.
    ///
    /// # Errors
    /// Store failures.
    pub async fn sync_steps_from_spec(&self, task: &TaskRecord) -> AppResult<bool> {
        let (Some(worktree), Some(spec_path)) =
            (task.worktree_path.as_ref(), task.spec_path.as_ref())
        else {
            return Ok(false);
        };
        let (root, rel) = (PathBuf::from(worktree), spec_path.clone());
        let expected = task.id.clone();
        let plan = tokio::task::spawn_blocking(move || {
            read_repo_doc(&root, &rel).map(|text| validate_spec_for(&text, &expected).plan)
        })
        .await
        .map_err(AppError::internal)?;
        let Some(plan) = plan else { return Ok(false) };
        let items: Vec<_> = plan
            .into_iter()
            .filter(|item| item.depth == 0 && !item.text.trim().is_empty())
            .take(crate::coordination::MAX_STEPS)
            .collect();
        if items.is_empty() {
            return Ok(false);
        }
        let existing = self.deps.store.steps(&task.id).await?;
        let now = self.now();
        let mut steps: Vec<crate::model::StepRecord> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let title: String = yulius_domain::steps::normalize_title(&item.text)
                .chars()
                .take(yulius_domain::steps::MAX_TITLE_CHARS)
                .collect();
            // A repeated item (the spec check reports it) becomes its own step.
            let previous = crate::discipline::step_by_title(&existing, &title)
                .filter(|p| !steps.iter().any(|s| s.id == p.id));
            let status = if item.checked {
                "done".to_owned()
            } else {
                previous.map_or_else(|| "pending".to_owned(), |p| p.status.clone())
            };
            let unchanged = previous.is_some_and(|p| p.status == status);
            steps.push(crate::model::StepRecord {
                id: match previous {
                    Some(p) => p.id.clone(),
                    None => yulius_domain::ids::StepId::parse(&self.deps.crypto.new_id())
                        .map_err(AppError::internal)?,
                },
                task_id: task.id.clone(),
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                title,
                status,
                note: previous.and_then(|p| p.note.clone()),
                updated_by: if unchanged {
                    previous.and_then(|p| p.updated_by.clone())
                } else {
                    Some("spec".into())
                },
                updated_at: if unchanged {
                    previous.and_then(|p| p.updated_at)
                } else {
                    Some(now)
                },
                effort: previous.map_or(1, |p| p.effort),
            });
        }
        let same = steps.len() == existing.len()
            && steps
                .iter()
                .zip(&existing)
                .all(|(a, b)| a.title == b.title && a.status == b.status);
        if same {
            return Ok(false);
        }
        self.deps
            .store
            .replace_steps(&task.id, &steps, "spec", now)
            .await?;
        self.publish_steps(task).await;
        Ok(true)
    }
}

/// A Git credential issued to a task or a user process (`POST /internal/git-credential`).
#[derive(Debug)]
pub struct GitCredential {
    /// A user's GitHub token, or an installation access token.
    pub token: crate::model::Secret,
}

impl App {
    /// Issues the run's GitHub token (SPEC-002 §8, R-06): the task owner's own token, so the
    /// agent's pushes are theirs on GitHub (F 2026-10-11); without a usable one, the App's
    /// installation token restricted to the run's repository, said once in the task's terminal.
    /// `gh` in the run asks the same way (`yulius gh-token`, SPEC-004). Requests for any other
    /// host or repository are refused.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown runs, `Forbidden` for other repositories, GitHub failures.
    pub async fn agent_git_credential(
        &self,
        token: &str,
        host: &str,
        path: Option<&str>,
    ) -> AppResult<GitCredential> {
        let ctx = self.agent_context(token).await?;
        if host != "github.com" {
            return Err(AppError::Forbidden(
                "credentials are only issued for github.com".into(),
            ));
        }
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if let Some(path) = path {
            let wanted = path
                .trim_matches('/')
                .trim_end_matches(".git")
                .to_ascii_lowercase();
            let own = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name).to_ascii_lowercase();
            if wanted != own {
                return Err(AppError::Forbidden(
                    "credentials are only issued for this project's repository".into(),
                ));
            }
        }
        let token = match self.owner_github_token(&ctx.task).await {
            Some(token) => token,
            None => self.repository_installation_token(&sandbox).await?,
        };
        Ok(GitCredential { token })
    }

    /// An installation token restricted to `sandbox`'s repository (agents' Git, and the
    /// superadmin's shells, ACC-012).
    ///
    /// # Errors
    /// `Conflict` when the App is not installed; GitHub failures.
    pub(crate) async fn repository_installation_token(
        &self,
        sandbox: &crate::model::SandboxRecord,
    ) -> AppResult<crate::model::Secret> {
        let installation = self
            .deps
            .store
            .instance()
            .await?
            .and_then(|i| i.installation_id)
            .ok_or_else(|| AppError::Conflict("the GitHub App is not installed".into()))?;
        let app = self.github_app().await?;
        self.deps
            .github
            .repository_token(&app, installation, &sandbox.repo_name)
            .await
    }
}

/// Repository documents read for the briefing.
struct RepoDocs {
    layout: yulius_domain::spec_format::DocsLayout,
    product: Option<String>,
    architecture_present: bool,
    changes: Option<String>,
    decision_file: Option<String>,
    decision_dir: Vec<(String, String)>,
    instructions: Vec<String>,
    foundations: Foundations,
}

/// Most entries of `.github/workflows` examined.
const MAX_WORKFLOW_ENTRIES: usize = 200;

/// GitHub Actions workflow files of the repository (a real directory inside it, not a link).
fn actions_workflows(root: &Path) -> usize {
    let Some(dir) = repo_path(root, ".github/workflows") else {
        return 0;
    };
    let inside = std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir())
        && std::fs::canonicalize(root)
            .ok()
            .zip(std::fs::canonicalize(&dir).ok())
            .is_some_and(|(root, dir)| dir.starts_with(root));
    if !inside {
        return 0;
    }
    std::fs::read_dir(&dir).map_or(0, |entries| {
        entries
            .take(MAX_WORKFLOW_ENTRIES)
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.ends_with(".yml") || name.ends_with(".yaml"))
            .count()
    })
}

/// The project's foundations as the briefing reports them (DSC-014).
fn foundations(root: &Path, toml: Option<&str>) -> Foundations {
    let commands = toml.and_then(|text| {
        yulius_domain::spec_format::parse_yulius_toml(text)
            .ok()
            .map(|config| config.commands)
    });
    let set = |value: Option<&String>| value.is_some_and(|v| !v.trim().is_empty());
    Foundations {
        config: toml.is_some(),
        test_command: commands.as_ref().is_some_and(|c| set(c.test.as_ref())),
        lint_command: commands.as_ref().is_some_and(|c| set(c.lint.as_ref())),
        actions_workflows: actions_workflows(root),
    }
}

impl RepoDocs {
    fn read(root: &Path) -> Self {
        let toml = read_repo_doc(root, "yulius.toml");
        let layout = project_layout(toml.as_deref());
        let doc = |rel: &str| read_repo_doc(root, rel);
        let product = doc(&layout.product).map(|t| t.chars().take(4_000).collect());
        let architecture_present = repo_doc_exists(root, &layout.architecture);
        let changes = doc(&layout.changes).map(|t| t.chars().take(4_000).collect());
        let (mut decision_file, mut decision_dir) = (None, Vec::new());
        match &layout.decisions {
            DecisionsLocation::File(rel) => decision_file = doc(rel.as_str()),
            DecisionsLocation::Directory(rel) => {
                // The directory itself must be a real one inside the repository (#70).
                if let Some(dir) = repo_path(root, rel.as_str())
                    && std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir())
                    && std::fs::canonicalize(root)
                        .ok()
                        .zip(std::fs::canonicalize(&dir).ok())
                        .is_some_and(|(root, dir)| dir.starts_with(root))
                    && let Ok(entries) = std::fs::read_dir(&dir)
                {
                    let mut names: Vec<String> = entries
                        .filter_map(Result::ok)
                        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                        .filter(|name| name.ends_with(".md"))
                        .collect();
                    names.sort();
                    for name in names.into_iter().take(MAX_DECISION_FILES) {
                        if let Some(text) = read_repo_doc(root, &format!("{rel}/{name}")) {
                            decision_dir.push((name, text));
                        }
                    }
                }
            }
        }
        let instructions = [layout.agents.as_str(), "CLAUDE.md"]
            .iter()
            .filter(|rel| repo_doc_exists(root, rel))
            .map(|rel| (*rel).to_owned())
            .collect();
        let foundations = foundations(root, toml.as_deref());
        Self {
            layout,
            product,
            architecture_present,
            changes,
            decision_file,
            decision_dir,
            instructions,
            foundations,
        }
    }

    fn decision_files(&self) -> DecisionFiles<'_> {
        if let Some(text) = &self.decision_file {
            return DecisionFiles::File(text);
        }
        if self.decision_dir.is_empty() {
            return DecisionFiles::Missing;
        }
        DecisionFiles::Directory(
            self.decision_dir
                .iter()
                .map(|(name, contents)| NamedFile { name, contents })
                .collect(),
        )
    }
}

/// The `notification_type`s of Claude Code 2.1.292 that ask a person to act: a permission prompt
/// (also of a worker), a question for the user (also a URL to open) and an agent that needs
/// input. The others report something (the idle reminder, a sign-in, a background agent that
/// finished, a push notification, a quota resume, a refusal fallback) and ask nobody: they are
/// frequent and arrive while the agent works, so a type this list does not know is not one
/// either (STA-002, SPEC-032).
const NOTICES_ASKING_A_PERSON: [&str; 5] = [
    "permission_prompt",
    "worker_permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// Whether the main session's next work shows the person answered the notice: a permission
/// prompt or a question of the session itself. A worker's prompt or an agent that needs input
/// is not answered by the session going on with something else.
fn notice_answered_by_work(input: &HookInput) -> bool {
    !matches!(
        input.notification_type.as_deref(),
        Some("worker_permission_prompt" | "agent_needs_input")
    )
}

/// Whether a `Notification` hook asks a person to act ([`NOTICES_ASKING_A_PERSON`]).
/// Without `notification_type` (older Claude Code) the idle reminder is known by its text.
fn notification_needs_a_person(input: &HookInput) -> bool {
    match input.notification_type.as_deref() {
        Some(kind) => NOTICES_ASKING_A_PERSON.contains(&kind),
        None => !input
            .message
            .as_deref()
            .is_some_and(|m| m.to_ascii_lowercase().contains("waiting for your input")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RepoDocs, foundations, notice_answered_by_work, notification_needs_a_person, repo_path,
    };
    use std::path::Path;
    use yulius_contracts::hooks::HookInput;

    fn notice(kind: Option<&str>, message: Option<&str>) -> HookInput {
        HookInput {
            notification_type: kind.map(str::to_owned),
            message: message.map(str::to_owned),
            ..HookInput::default()
        }
    }

    /// STA-002, STA-003 (SPEC-032): the allow-list, and which notices the session's own work
    /// answers.
    #[test]
    fn only_notices_that_ask_a_person_flag_the_task() {
        for kind in [
            "permission_prompt",
            "worker_permission_prompt",
            "elicitation_dialog",
            "elicitation_url_dialog",
            "agent_needs_input",
        ] {
            assert!(
                notification_needs_a_person(&notice(Some(kind), None)),
                "{kind}"
            );
        }
        for kind in [
            "idle_prompt",
            "auth_success",
            "agent_completed",
            "push_notification",
            "x",
        ] {
            assert!(
                !notification_needs_a_person(&notice(Some(kind), None)),
                "{kind}"
            );
        }
        // Older CLIs: the idle text is the reminder, anything else asks.
        assert!(!notification_needs_a_person(&notice(
            None,
            Some("Claude is waiting for your input")
        )));
        assert!(notification_needs_a_person(&notice(
            None,
            Some("Claude needs your permission")
        )));
        // The session's own next work answers its prompts, not a worker's or an agent's.
        assert!(notice_answered_by_work(&notice(
            Some("permission_prompt"),
            None
        )));
        assert!(notice_answered_by_work(&notice(
            None,
            Some("Claude needs your permission")
        )));
        assert!(!notice_answered_by_work(&notice(
            Some("worker_permission_prompt"),
            None
        )));
        assert!(!notice_answered_by_work(&notice(
            Some("agent_needs_input"),
            None
        )));
    }

    /// #70: a document path through a linked directory never leaves the repository (the
    /// briefing and `project_context` would hand the agent a host file otherwise).
    #[test]
    fn repo_docs_never_follow_linked_directories_out_of_the_repository()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("repo");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(outside.join("decisions"))?;
        std::fs::write(outside.join("secret.md"), "TOP SECRET")?;
        std::fs::write(
            outside.join("decisions/0001-secret.md"),
            "# Secret decision",
        )?;
        std::os::unix::fs::symlink(&outside, root.join("x"))?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nproduct = \"x/secret.md\"\nchanges = \"x/secret.md\"\ndecisions = \"x/decisions\"\n",
        )?;
        let docs = RepoDocs::read(&root);
        assert_eq!(
            docs.product, None,
            "product read through a linked directory"
        );
        assert_eq!(
            docs.changes, None,
            "changes read through a linked directory"
        );
        assert!(
            docs.decision_dir.is_empty(),
            "decisions listed through a link"
        );
        // A real document inside the repository is still read.
        std::fs::create_dir_all(root.join("docs"))?;
        std::fs::write(root.join("docs/product.md"), "# Product")?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nproduct = \"docs/product.md\"\n",
        )?;
        assert_eq!(RepoDocs::read(&root).product.as_deref(), Some("# Product"));
        Ok(())
    }

    /// DSC-014: the briefing knows which foundations a project lacks, without following links
    /// out of the repository.
    #[test]
    fn foundations_are_read_from_the_repository() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join(".github/workflows"))?;
        std::fs::write(root.join(".github/workflows/ci.yml"), "on: push\n")?;
        std::fs::write(root.join(".github/workflows/README.md"), "notes\n")?;
        let bare = foundations(&root, None);
        assert!(!bare.config && !bare.test_command && !bare.lint_command);
        assert_eq!(bare.actions_workflows, 1);
        let configured = foundations(
            &root,
            Some("[commands]\ntest = \"cargo test\"\nlint = \"cargo clippy\"\n"),
        );
        assert!(configured.config && configured.test_command && configured.lint_command);
        std::fs::remove_dir_all(root.join(".github"))?;
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("deploy.yml"), "on: push\n")?;
        std::fs::create_dir_all(root.join(".github"))?;
        std::os::unix::fs::symlink(&outside, root.join(".github/workflows"))?;
        assert_eq!(
            foundations(&root, None).actions_workflows,
            0,
            "a linked directory is not read"
        );
        Ok(())
    }

    #[test]
    fn repo_paths_stay_inside_the_root() {
        let root = Path::new("/r");
        assert_eq!(repo_path(root, "docs/a.md"), Some(root.join("docs/a.md")));
        assert_eq!(repo_path(root, "../x"), None);
        assert_eq!(repo_path(root, "/etc/passwd"), None);
        assert_eq!(repo_path(root, "a/../../x"), None);
    }
}
