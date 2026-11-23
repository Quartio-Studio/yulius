//! The independent code review's rounds as Yulius sees them (SPEC-493 REV-002 to REV-004).
//!
//! A task starts its review with the Agent tool and [`REVIEWER_AGENT`]. Its `PreToolUse` reaches
//! `App::review_gate`, which numbers the round among those of the task's open pull request,
//! decides the reviewer it runs as ([`round_agent`]) and records it; a different reviewer than
//! the one asked for is given back as the tool's new input. `SubagentStart` links the round to
//! Claude Code's subagent, `SubagentStop` ends it with the report's verdict. A merge closes the
//! pull request's rounds, so the next pull request of the task starts at round 1 again.
//!
//! A round is counted once it ended with a report. A reviewer started again for a round that has
//! none (a usage limit cut it off, or it still runs) keeps that round's number and reviewer.
//! When Yulius cannot be asked, the reviewer starts as asked (REV-003).

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use yulius_contracts::hooks::{HookInput, HookOutput, events};
use yulius_domain::harness::{Harness, claude_model_name, valid_session_id};
use yulius_domain::review::{
    LATE_REVIEWER_AGENT, REVIEWER_AGENT, Verdict, reviewer_model, round_agent, verdict_of,
};
use yulius_domain::usage::ModelFamily;

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::AppResult;
use crate::model::ReviewRoundRecord;
use crate::ports::store::ReviewStart;
use yulius_domain::time::Timestamp;

/// Yulius's reviewer agents.
const REVIEWERS: [&str; 2] = [REVIEWER_AGENT, LATE_REVIEWER_AGENT];

/// The switch reason of a move for a Fable reviewer (REV-007): it interrupts the turn first,
/// as a person's switch does, since the agent goes on after the refusal.
pub(crate) const MOVE_FOR_FABLE: &str = "review_fable";

/// Why the reviewer did not start: the session moves.
const MOVING_FOR_FABLE: &str = "Yulius moves this session to a Claude subscription that can run \
     Fable 5.2 for this review round. Do nothing else now: the session goes on there and is \
     asked to start the review again.";

/// Why the reviewer did not start: a move would end background work that runs.
const BUSY_FOR_FABLE: &str = "This subscription cannot run Fable 5.2 for this review round, and \
     Yulius would move the session to one that can, but background work runs (shells, agents, \
     monitors or a `yulius wait`), which a move would end. Finish or stop it, then start the \
     review again.";

/// Why the reviewer did not start: a move would drop a prompt waiting in Claude Code's queue.
const QUEUED_FOR_FABLE: &str = "This subscription cannot run Fable 5.2 for this review round, and \
     Yulius would move the session to one that can, but a prompt waits in Claude Code's queue, \
     which a move would drop. Answer it or end your turn, then start the review again.";

/// The longest the look at whether the head under review can go out may take in the hook.
const READY_LOOK: std::time::Duration = std::time::Duration::from_millis(1500);

/// The longest the move's looks may take: the hook client gives `PreToolUse` 5 s, after which
/// the reviewer starts as asked while a move could still be decided.
const MOVE_DECISION: std::time::Duration = std::time::Duration::from_secs(3);

/// The first prompt of a session moved for a Fable reviewer.
const MOVED_FOR_FABLE_PROMPT: &str = "Yulius moved this session to a Claude subscription that can \
     run Fable 5.2 for the review round you asked for. Start the review again: the Agent tool \
     with the `yulius-reviewer` agent and the same prompt.";

/// What a reviewer that runs on Opus because Fable cannot is told; its report repeats it.
const NO_FABLE_REVIEWER_LINE: &str = "\n\nYulius: this task's session cannot run Fable 5.2 now \
     and cannot move to a subscription that can, so this round runs on Opus 5.5. Say so in the \
     first line of your summary.";

/// The switch reason of a move for a reviewer a usage limit cut off (RLM-002): made at a turn's
/// end or while the session is idle, so it interrupts nothing.
pub(crate) const REVIEW_RESUME: &str = "review_resume";

/// A review round's reviewer that a usage limit cut off and that was not resumed yet
/// (SPEC-493 RLM-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CutOffReviewer {
    /// Claude Code's id of the subagent: `SendMessage` to it resumes it.
    pub(crate) agent_id: String,
    /// The model its round runs.
    pub(crate) model: String,
    /// When the usage limit cut this reviewer off.
    pub(crate) cut_off_at: Timestamp,
}

/// What the new process of a session moved past its reviewer's limit is told (RLM-002), after
/// the prompt it would open with (`after`).
#[must_use]
pub(crate) fn reviewer_resume_prompt(after: Option<&str>, reviewer: &CutOffReviewer) -> String {
    let told = format!(
        "Yulius continued this session in a new process (a move to another Claude subscription \
         or a restart), and a usage limit had cut off your reviewer `{id}` ({model}) in the \
         middle of its review round. Resume that reviewer now: SendMessage to `{id}`, asking it \
         to continue its review. Do not start a new reviewer for this round: the one you resume \
         keeps what it has read. Then continue the task where you left off.",
        model = claude_model_name(&reviewer.model),
        id = reviewer.agent_id,
    );
    match after.map(str::trim).filter(|prompt| !prompt.is_empty()) {
        Some(prompt) => format!("{prompt}\n\n{told}"),
        None => told,
    }
}

/// How often, and how far apart, a session is offered [`reviewer_nowhere`] while it does not take
/// typed text: about two minutes in all.
const NOWHERE_TRIES: u32 = 12;
const NOWHERE_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

/// What a session is told whose cut-off reviewer no subscription can run now (RLM-002, REV-007).
fn reviewer_nowhere(reviewer: &CutOffReviewer) -> String {
    format!(
        "Yulius: a usage limit cut off your reviewer `{id}` in the middle of its review round, and \
         no Claude subscription can run {model} now. Start the review again (the Agent tool with \
         the `yulius-reviewer` agent and the same prompt): this round then runs on Opus 5.5, \
         which says so in its summary.",
        model = claude_model_name(&reviewer.model),
        id = reviewer.agent_id,
    )
}

/// The switch reason of a move before a round whose reviewer's model is about to run out where
/// the session is (RLM-003); it interrupts the turn first, as [`MOVE_FOR_FABLE`] does.
pub(crate) const MOVE_FOR_ROOM: &str = "review_room";

/// Why a session moves before its review round starts.
enum MovePurpose {
    /// Its subscription cannot run Fable 5.2 (REV-007).
    Fable,
    /// Its subscription is about to run out for the reviewer's `model` (RLM-003); `agent` is
    /// the reviewer asked for again there.
    Room { model: String, agent: String },
}

impl MovePurpose {
    fn reason(&self) -> &'static str {
        match self {
            Self::Fable => MOVE_FOR_FABLE,
            Self::Room { .. } => MOVE_FOR_ROOM,
        }
    }

    /// Why the reviewer did not start: the session moves.
    fn moving(&self) -> String {
        match self {
            Self::Fable => MOVING_FOR_FABLE.to_owned(),
            Self::Room { model, .. } => format!(
                "Yulius moves this session to a Claude subscription with more room for {} for \
                 this review round: the one it runs on is about to run out. Do nothing else now: \
                 the session goes on there and is asked to start the review again.",
                claude_model_name(model)
            ),
        }
    }

    /// The first prompt of the session moved.
    fn prompt(&self) -> String {
        match self {
            Self::Fable => MOVED_FOR_FABLE_PROMPT.to_owned(),
            Self::Room { model, agent } => format!(
                "Yulius moved this session to a Claude subscription with room for {} for the \
                 review round you asked for. Start the review again: the Agent tool with the \
                 `{agent}` agent and the same prompt.",
                claude_model_name(model)
            ),
        }
    }
}

/// Where a session moves for its cut-off reviewer ([`App::reviewer_move_target`]).
pub(crate) enum ReviewerMove {
    /// To this subscription.
    To(yulius_domain::ids::AccountId),
    /// Not now: at a later turn's end.
    Later,
    /// Nowhere: no reviewer waits, or the session was told the round runs on Opus.
    Nothing,
}

/// What a review round that cannot run Fable on its subscription does (REV-007).
enum FableMove {
    /// The Agent call is refused with this reason (the session moves, or work would be lost).
    Refused(String),
    /// No move: the round runs the Opus reviewer.
    Nowhere,
}

impl App {
    /// `PreToolUse` of the Agent tool: a review round's start, or nothing for another subagent.
    /// A failure to decide lets the reviewer start as asked (REV-003).
    pub(crate) async fn review_gate(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> HookOutput {
        match self.start_review_round(ctx, input).await {
            Ok(output) => output,
            Err(err) => {
                tracing::warn!(error = %err, task = %ctx.task.id, "review round not recorded");
                HookOutput::default()
            }
        }
    }

    async fn start_review_round(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookOutput> {
        let Some(tool_input) = input.tool_input.as_ref().and_then(Value::as_object) else {
            return Ok(HookOutput::default());
        };
        let Some(asked) = tool_input
            .get("subagent_type")
            .and_then(Value::as_str)
            .filter(|asked| REVIEWERS.contains(asked))
        else {
            return Ok(HookOutput::default());
        };
        // A reviewer asked for again supersedes one a limit cut off: nothing tells the session
        // to resume that one any more (review round 1 of fix 7).
        self.forget_cut_off_reviewer(&ctx.task.id).await;
        let store = &self.deps.store;
        let rounds = store.review_rounds(&ctx.task.id, true).await?;
        // The round after the last one that reported: a round reported by two of its
        // reviewers (one resumed after a cut-off, or two started at once) counts once.
        let number = rounds
            .iter()
            .filter(|r| r.ended_at.is_some())
            .map(|r| r.round)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let unfinished = rounds
            .iter()
            .rev()
            .find(|r| r.round == number && r.ended_at.is_none());
        let (mut agent, mut reason) = match unfinished {
            Some(round) => (round.agent.clone(), "kept"),
            None => (round_agent(number).to_owned(), "round"),
        };
        // A subagent runs on its session's subscription. One that cannot run Fable now moves
        // the session to one that can run both, when there is one (the reviewer is asked for
        // again there); else the round gets the Opus reviewer, which it can run, rather than a
        // reviewer refused at its first request (REV-007).
        if agent == REVIEWER_AGENT && !self.run_can_take_fable(ctx).await {
            match self.move_for(ctx, &MovePurpose::Fable).await {
                FableMove::Refused(why) => return Ok(HookOutput::deny_tool(why)),
                FableMove::Nowhere => {
                    agent = LATE_REVIEWER_AGENT.to_owned();
                    reason = "no_fable";
                }
            }
        }
        let model = reviewer_model(&agent).unwrap_or_default().to_owned();
        // Nor does a round start where its reviewer's model is about to run out while another
        // subscription has room for it: the session moves first (RLM-003).
        if self.run_is_tight_for(ctx, &model).await {
            let purpose = MovePurpose::Room {
                model: model.clone(),
                agent: agent.clone(),
            };
            if let FableMove::Refused(why) = self.move_for(ctx, &purpose).await {
                return Ok(HookOutput::deny_tool(why));
            }
        }
        let head = self.review_head_at_start(ctx).await;
        store
            .insert_review_round(&ReviewRoundRecord {
                id: self.deps.crypto.new_id(),
                task_id: ctx.task.id.clone(),
                round: number,
                head,
                agent: agent.clone(),
                model: model.clone(),
                reason: reason.to_owned(),
                subagent_id: None,
                model_answered: None,
                started_at: self.now(),
                ended_at: None,
                verdict: None,
                cut_off: false,
                pr_number: None,
                merged_at: None,
            })
            .await?;
        let mut said = format!(
            "Yulius: this is review round {number} of this pull request; the reviewer runs on {} \
             ({agent}).",
            claude_model_name(&model)
        );
        said.push_str(&self.deliver_for_review(&ctx.task).await);
        if reason == "no_fable" {
            said.push_str(
                " This session cannot run Fable 5.2 now, nor move to a subscription that can, so \
                 the round runs on Opus 5.5: say so in the review's summary.",
            );
        }
        // The reviewer's prompt says why it runs on Opus, also when it was asked for by name.
        if agent == asked && reason != "no_fable" {
            return Ok(HookOutput::additional_context(events::PRE_TOOL_USE, said));
        }
        let mut updated = tool_input.clone();
        updated.insert("subagent_type".into(), Value::String(agent));
        if reason == "no_fable"
            && let Some(Value::String(prompt)) = updated.get_mut("prompt")
        {
            prompt.push_str(NO_FABLE_REVIEWER_LINE);
        }
        Ok(HookOutput::run_tool_with(Value::Object(updated), said))
    }

    /// The head under review goes out now, so Yulius CI tests it while the reviewer reads
    /// (SPEC-493 CIR-001, CIR-002): pushed (the pull request opened when there is none) and its
    /// run queued at once, not at the next poll. What the agent is told: that, or why no run
    /// starts (a spec that is not valid, uncommitted changes). A playground, and a task whose
    /// delivery cannot be read, say nothing.
    async fn deliver_for_review(self: &Arc<Self>, task: &crate::model::TaskRecord) -> String {
        if task.worktree_path.is_none() {
            return String::new();
        }
        let Ok(sandbox) = self.dlv_sandbox_of(task).await else {
            return String::new();
        };
        if sandbox.is_playground() {
            return String::new();
        }
        // Only the message waits for this look, within its bound: the delivery checks again.
        if let Ok(Some(reason)) =
            tokio::time::timeout(READY_LOOK, self.review_head_not_ready(task)).await
        {
            return format!(" No Yulius CI run starts for this head: {reason}.");
        }
        let app = Arc::clone(self);
        let task_id = task.id.clone();
        tokio::spawn(async move {
            match app
                .try_auto_open_pr(&task_id, yulius_domain::steps::DeliveryAsk::Review)
                .await
            {
                Ok(crate::delivery::AutoDelivery::Blocked(_)) => {}
                Ok(_) => {
                    if let Err(err) = app.ci_sync_sandbox(&sandbox).await {
                        tracing::warn!(error = %err, task = %task_id, "the run for the head under review was not queued");
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err.error, task = %task_id, "the head under review was not delivered");
                }
            }
        });
        " Yulius delivers this head now when it can, so Yulius CI tests it while the review \
         runs: `ci_status` shows its run."
            .to_owned()
    }

    /// Moves the session of a review round that cannot run Fable to a subscription that can run
    /// both its model and Fable (REV-007): refused with why, the session continues there with a
    /// prompt asking for the review again. While background work runs that a move would end, or
    /// a prompt waits in Claude Code's queue, nothing moves and the refusal says so. A move
    /// refused moments ago, no such subscription, a new session that could not start there, or
    /// looks that take longer than [`MOVE_DECISION`] leave the round to the Opus reviewer: a
    /// move is announced only once it can start.
    async fn move_for(self: &Arc<Self>, ctx: &AgentContext, purpose: &MovePurpose) -> FableMove {
        if let Ok(decided) =
            tokio::time::timeout(MOVE_DECISION, self.decide_move(ctx, purpose)).await
        {
            return decided;
        }
        tracing::info!(task = %ctx.task.id, "no move for a reviewer: the looks took longer than the hook may wait");
        FableMove::Nowhere
    }

    async fn decide_move(self: &Arc<Self>, ctx: &AgentContext, purpose: &MovePurpose) -> FableMove {
        let task = &ctx.task;
        if self.switch_cooling(&task.id) {
            return FableMove::Nowhere;
        }
        let Ok(input) = self
            .rotation_input(task, ctx.attempt.account_id.clone())
            .await
        else {
            return FableMove::Nowhere;
        };
        let target = match purpose {
            MovePurpose::Fable => {
                let fable = ModelFamily::of(Some(yulius_domain::harness::REVIEW_CLAUDE_MODEL));
                yulius_domain::rotation::move_to_also_run(&input, fable)
            }
            MovePurpose::Room { model, .. } => {
                yulius_domain::rotation::move_with_room_for(&input, ModelFamily::of(Some(model)))
            }
        };
        let Some(account) = target else {
            return FableMove::Nowhere;
        };
        // A move ends the session's background shells, agents and monitors and what waits in
        // Claude Code's queue; a look that gives no answer counts as work running (SPEC-483).
        let prompts_before = self.prompts_passed(&task.id);
        self.await_transcript_read(&task.id).await;
        // A move for room gives way to the work: the round starts where it is (RLM-003).
        let room = matches!(purpose, MovePurpose::Room { .. });
        if self.background_or_wait_now(&task.id).await != Some(false) {
            if room {
                return FableMove::Nowhere;
            }
            return FableMove::Refused(BUSY_FOR_FABLE.to_owned());
        }
        if self.prompt_waits_for_turns_end(&task.id, prompts_before) {
            if room {
                return FableMove::Nowhere;
            }
            return FableMove::Refused(QUEUED_FOR_FABLE.to_owned());
        }
        // The new session must be able to start there before the move is announced (ROT-014).
        // The check runs in a task of its own: the decision's deadline ends the wait for it,
        // never the check (its disk upkeep removes caches under a claim).
        let check = {
            let app = Arc::clone(self);
            let (task, account) = (task.clone(), account.clone());
            tokio::spawn(async move { app.preflight_start(&task, Some(&account)).await })
        };
        match check.await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                tracing::info!(task = %task.id, error = %err, "no move for a reviewer: the session could not start there");
                return FableMove::Nowhere;
            }
            Err(_) => return FableMove::Nowhere,
        }
        let app = Arc::clone(self);
        let task_id = task.id.clone();
        let run = ctx.attempt.id.clone();
        let (purpose_reason, prompt) = (purpose.reason(), purpose.prompt());
        tokio::spawn(async move {
            // Only the run this was decided for moves: a later review start that outlasted an
            // earlier move must not restart the session it made.
            crate::race::reached(
                crate::race::Point::ReviewMoveDecided,
                Path::new(task_id.as_str()),
            );
            let live = app.deps.store.live_attempt(&task_id).await.ok().flatten();
            if live.is_none_or(|live| live.id != run) {
                return;
            }
            let (reason, prompt) = (purpose_reason, Some(prompt));
            if let Err(err) = app
                .switch_task_now(&task_id, Some(account), reason, prompt)
                .await
            {
                tracing::warn!(error = %err, task = %task_id, "the move for a reviewer failed");
            }
        });
        FableMove::Refused(purpose.moving())
    }

    /// Whether the subscription the task's run is on is about to run out for `model`: known room
    /// under [`yulius_domain::rotation::MIN_ROOM`] points in a window that binds it
    /// ([`yulius_domain::rotation::Tier::Tight`], RLM-003). Unknown counts as no.
    async fn run_is_tight_for(&self, ctx: &AgentContext, model: &str) -> bool {
        let Some(account) = ctx.attempt.account_id.as_ref() else {
            return false;
        };
        let Ok(views) = self.account_views().await else {
            return false;
        };
        let family = ModelFamily::of(Some(model));
        views
            .iter()
            .find(|view| &view.id == account)
            .is_some_and(|view| {
                yulius_domain::rotation::assess(
                    view,
                    self.now(),
                    yulius_domain::rotation::THRESHOLD,
                    family,
                )
                .is_some_and(|a| a.tier == yulius_domain::rotation::Tier::Tight)
            })
    }

    /// Whether the subscription the task's run is on can run Fable now: below the pool's
    /// threshold in every window that binds Fable. Unknown (no subscription, a failed read)
    /// counts as yes: the reviewer then starts as asked.
    async fn run_can_take_fable(&self, ctx: &AgentContext) -> bool {
        let Some(account) = ctx.attempt.account_id.as_ref() else {
            return true;
        };
        let Ok(views) = self.account_views().await else {
            return true;
        };
        let fable = ModelFamily::of(Some(yulius_domain::harness::REVIEW_CLAUDE_MODEL));
        views
            .iter()
            .find(|view| &view.id == account)
            .is_none_or(|view| {
                yulius_domain::rotation::assess(
                    view,
                    self.now(),
                    yulius_domain::rotation::THRESHOLD,
                    fable,
                )
                .is_some()
            })
    }

    /// Forgets `task`'s cut-off reviewer and any move due for it: a new reviewer was asked
    /// for, the session starts afresh, or its round is over (review round 1 of fix 7).
    pub(crate) async fn forget_cut_off_reviewer(&self, task: &yulius_domain::ids::TaskId) {
        let Some(runtime) = self.existing_runtime(task) else {
            return;
        };
        let mut cut = runtime.cut_off_reviewer.lock().await;
        *cut = None;
        runtime
            .reviewer_move
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// The reviewer `agent_id` of `task`'s open review round, with the model its round runs:
    /// `None` when that subagent is no open round's.
    pub(crate) async fn open_round_reviewer(
        &self,
        task: &yulius_domain::ids::TaskId,
        agent_id: &str,
        cut_off_at: Option<Timestamp>,
    ) -> Option<CutOffReviewer> {
        let cut_off_at = cut_off_at.unwrap_or_else(|| self.now());
        let model = self
            .deps
            .store
            .unfinished_review_model_at(task, agent_id, cut_off_at)
            .await
            .ok()??;
        Some(CutOffReviewer {
            agent_id: agent_id.to_owned(),
            model,
            cut_off_at,
        })
    }

    /// Whether `task` has a review round that has not reported (a reviewer runs, or was cut
    /// off).
    pub(crate) async fn review_round_open(&self, task: &yulius_domain::ids::TaskId) -> bool {
        self.deps
            .store
            .review_rounds(task, true)
            .await
            .is_ok_and(|rounds| rounds.iter().any(|r| r.ended_at.is_none()))
    }

    /// The cut-off reviewer `agent_id` of `task` started again or reported: nothing is left to
    /// tell a relaunch, and no move is due for it (RLM-002).
    async fn reviewer_went_on(
        &self,
        task: &yulius_domain::ids::TaskId,
        agent_id: &str,
        sent: Timestamp,
    ) {
        let Some(runtime) = self.existing_runtime(task) else {
            return;
        };
        let mut cut = runtime.cut_off_reviewer.lock().await;
        if cut
            .as_ref()
            .is_some_and(|c| c.agent_id == agent_id && sent >= c.cut_off_at)
        {
            *cut = None;
            runtime
                .reviewer_move
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Injects the transcript watcher's usage-limit observation for an active reviewer in tests.
    #[cfg(feature = "test-support")]
    pub async fn cut_off_review_for_test(
        self: &Arc<Self>,
        task: &yulius_domain::ids::TaskId,
        id: &str,
        at: Option<Timestamp>,
    ) {
        if let Ok(Some(attempt)) = self.deps.store.live_attempt(task).await {
            self.subagent_limit(
                task,
                &attempt,
                ModelFamily::Opus,
                &crate::ports::harness::Observation::SubagentCutOff {
                    agent_id: id.to_owned(),
                    at,
                    scope: yulius_domain::usage::LimitScope::Model(Some(ModelFamily::Fable)),
                    resets_at: Some(Timestamp(self.now().0 + 86_400_000)),
                    window_minutes: Some(10_080),
                    message: "reviewer reached its weekly limit".to_owned(),
                },
            )
            .await;
        }
    }

    /// Injects the transcript watcher's report of the background agent `id` in tests.
    #[cfg(feature = "test-support")]
    pub async fn review_reported_for_test(
        &self,
        task: &yulius_domain::ids::TaskId,
        id: &str,
        report: &str,
        at: Option<Timestamp>,
    ) {
        self.review_agent_reported(task, id, report, at).await;
    }

    /// Reads the resume instruction's recipient and the scheduled subscription move in tests.
    #[cfg(feature = "test-support")]
    pub async fn review_recovery_for_test(
        &self,
        task: &yulius_domain::ids::TaskId,
    ) -> (Option<String>, bool) {
        let runtime = self.runtime(task);
        let cut = runtime.cut_off_reviewer.lock().await;
        let reviewer = cut.as_ref().map(|r| r.agent_id.clone());
        (
            reviewer,
            runtime
                .reviewer_move
                .load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    /// Where `task`'s session moves for its cut-off reviewer (RLM-002): a subscription that can
    /// run the reviewer's model beside the session's, or none. With none the reviewer is
    /// forgotten and the session told the round runs on Opus 5.5 (REV-007). `Later` while a
    /// move cannot be decided now (one was refused moments ago, the pool cannot be read).
    pub(crate) async fn reviewer_move_target(
        self: &Arc<Self>,
        task: &crate::model::TaskRecord,
        current: Option<yulius_domain::ids::AccountId>,
    ) -> ReviewerMove {
        let runtime = self.runtime(&task.id);
        let Some(reviewer) = runtime.cut_off_reviewer.lock().await.clone() else {
            return ReviewerMove::Nothing;
        };
        if self.switch_cooling(&task.id) {
            return ReviewerMove::Later;
        }
        let Ok(input) = self.rotation_input(task, current).await else {
            return ReviewerMove::Later;
        };
        let family = ModelFamily::of(Some(&reviewer.model));
        if let Some(account) = yulius_domain::rotation::move_to_also_run(&input, family) {
            return ReviewerMove::To(account);
        }
        *runtime.cut_off_reviewer.lock().await = None;
        let paste = format!("\x1b[200~{}\x1b[201~", reviewer_nowhere(&reviewer));
        let app = Arc::clone(self);
        let task = task.clone();
        tokio::spawn(async move {
            // The session may not take typed text yet (a person's draft, text typed moments
            // ago): it is told once it does, within a bound.
            for attempt in 1..=NOWHERE_TRIES {
                match app
                    .type_into_session(
                        &task,
                        paste.as_bytes(),
                        crate::task_message::Typed::Prompt,
                        crate::task_message::Sender::Yulius,
                    )
                    .await
                {
                    Ok(()) => return,
                    Err(err) if attempt == NOWHERE_TRIES => {
                        tracing::warn!(error = %err, task = %task.id, "the session was not told its reviewer cannot go on");
                    }
                    Err(_) => tokio::time::sleep(NOWHERE_RETRY).await,
                }
            }
        });
        ReviewerMove::Nothing
    }

    /// What a subagent's transcript says of a review round's reviewer: the model that answered
    /// it, or that a usage limit cut it off. A subagent that is no round's changes nothing.
    pub(crate) async fn review_subagent_noted(
        &self,
        task: &yulius_domain::ids::TaskId,
        subagent: &str,
        model: Option<&str>,
        cut_off: bool,
        at: Option<Timestamp>,
    ) {
        if let Err(err) = self
            .deps
            .store
            .note_review_subagent(task, subagent, model, cut_off, at)
            .await
        {
            tracing::warn!(error = %err, task = %task, "review round not noted");
        }
    }

    /// The session's transcript says the background agent `agent_id` completed with `report`
    /// at `at` (SPEC-633 RRE-002): the open round that agent runs ends with the report's
    /// verdict, unless its `SubagentStop` ended it already. Nothing started after `at` ends, so
    /// a replayed first report leaves a resumed reviewer's next round open. An undated replay
    /// ends nothing.
    pub(crate) async fn review_agent_reported(
        &self,
        task: &yulius_domain::ids::TaskId,
        agent_id: &str,
        report: &str,
        at: Option<Timestamp>,
    ) {
        let Some(at) = at else {
            return;
        };
        let verdict = verdict_of(report).map(Verdict::as_str);
        match self
            .deps
            .store
            .end_review_round(task, agent_id, at, verdict, None)
            .await
        {
            Ok(true) => self.reviewer_went_on(task, agent_id, at).await,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(error = %err, task = %task, "review round not ended from its report")
            }
        }
    }

    /// The worktree head observed at the start of this review.
    async fn review_head_at_start(&self, ctx: &AgentContext) -> Option<String> {
        let worktree = Path::new(ctx.task.worktree_path.as_deref()?);
        crate::race::reached(crate::race::Point::ReviewHead, worktree);
        self.deps
            .git
            .log(worktree, "HEAD", 1)
            .await
            .ok()?
            .into_iter()
            .next()
            .map(|c| c.sha)
    }

    /// A native Codex review child belongs to the authenticated parent session.
    fn codex_review_child<'a>(
        ctx: &AgentContext,
        input: &'a HookInput,
    ) -> Option<(&'a str, &'a str)> {
        let parent = input.session_id.as_deref()?;
        let child = input.agent_id.as_deref()?;
        (ctx.attempt.provider_session_id.as_deref() == Some(parent)
            && child != parent
            && valid_session_id(child))
        .then_some((parent, child))
    }

    async fn codex_review_event(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
        at: Timestamp,
        verdict: Option<Verdict>,
    ) -> AppResult<()> {
        let Some((parent, child)) = Self::codex_review_child(ctx, input) else {
            return Ok(());
        };
        let store = &self.deps.store;
        let model = input.model.as_deref().filter(|model| !model.is_empty());
        if let Some(verdict) = verdict
            && store
                .end_review_round(&ctx.task.id, child, at, Some(verdict.as_str()), model)
                .await?
        {
            return Ok(());
        }
        // The start hook can still be in flight after the CLI's timeout. Either event can
        // establish the round from the same rollout metadata in the store.
        let Some(model) = model else {
            return Ok(());
        };
        let anchor = self.sandbox_dir(&ctx.task.sandbox_id);
        let sessions = self.codex_sessions(&ctx.task.sandbox_id);
        let harness = Arc::clone(&self.deps.harness);
        let (parent, id) = (parent.to_owned(), child.to_owned());
        let Ok(Some(start)) = tokio::task::spawn_blocking(move || {
            let path = harness.find_codex_rollout(&sessions, &id).ok()??;
            let file = crate::transcripts::open(&anchor, &path).ok()?;
            crate::transcripts::codex_review_start(file, &parent, &id)
        })
        .await
        else {
            return Ok(());
        };
        if start.started_at > at {
            return Ok(());
        }
        store
            .record_codex_review_round(
                &ctx.task.id,
                child,
                model,
                start.head.as_deref(),
                start.started_at,
                verdict.map(|verdict| (at, verdict)),
            )
            .await
    }

    /// Links a newly launched reviewer, or opens its next round when it continues after a report.
    pub(crate) async fn review_subagent_started(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
        at: Timestamp,
    ) {
        if ctx.attempt.harness == Harness::Codex {
            if let Err(err) = self.codex_review_event(ctx, input, at, None).await {
                tracing::warn!(error = %err, task = %ctx.task.id, "Codex review start not recorded");
            }
            return;
        }
        let (Some(agent), Some(id)) = (input.agent_type.as_deref(), input.agent_id.as_deref())
        else {
            return;
        };
        if !REVIEWERS.contains(&agent) {
            return;
        }
        let store = &self.deps.store;
        let continuation_id = self.deps.crypto.new_id();
        let result = match store
            .start_review_subagent(&ctx.task.id, agent, id, &continuation_id, at)
            .await
        {
            Ok(ReviewStart::Ignored) => return,
            Ok(ReviewStart::Active) => {
                self.reviewer_went_on(&ctx.task.id, id, at).await;
                return;
            }
            Ok(ReviewStart::Continued) => {
                let Some(head) = self.review_head_at_start(ctx).await else {
                    return;
                };
                store.set_review_round_head(&continuation_id, &head).await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            tracing::warn!(error = %err, task = %ctx.task.id, "review subagent start not recorded");
        }
    }

    /// `SubagentStop`: a reviewer's report ends its round, with the verdict its last line says.
    pub(crate) async fn review_subagent_stopped(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
        at: Timestamp,
    ) {
        let verdict = input.last_assistant_message.as_deref().and_then(verdict_of);
        if ctx.attempt.harness == Harness::Codex {
            // A Codex child can finish turns while it is still preparing its report.
            if let Some(verdict) = verdict
                && let Err(err) = self.codex_review_event(ctx, input, at, Some(verdict)).await
            {
                tracing::warn!(error = %err, task = %ctx.task.id, "Codex review report not recorded");
            }
            return;
        }
        let (Some(id), Some(agent)) = (
            input.agent_id.as_deref(),
            input
                .agent_type
                .as_deref()
                .filter(|agent| REVIEWERS.contains(agent)),
        ) else {
            return;
        };
        let store = &self.deps.store;
        let verdict = verdict.map(Verdict::as_str);
        // A start that came before its round (a `PreToolUse` that outlasted the hook client)
        // linked nothing: the report ends the round it left unlinked (SPEC-633 RRE-001).
        let ended = match store
            .end_review_round(&ctx.task.id, id, at, verdict, None)
            .await
        {
            Ok(false) => {
                store
                    .end_unlinked_review_round(&ctx.task.id, agent, id, at, verdict)
                    .await
            }
            other => other,
        };
        match ended {
            Ok(true) => self.reviewer_went_on(&ctx.task.id, id, at).await,
            Ok(false) => {}
            Err(err) => tracing::warn!(error = %err, task = %ctx.task.id, "review round not ended"),
        }
    }

    /// `GET /api/analytics/reviews`: the review's numbers over the pull requests of the
    /// sandboxes the caller sees (or `sandbox`) merged in `[from, to]` (days, `YYYY-MM-DD`),
    /// SPEC-493 REV-006.
    ///
    /// # Errors
    /// `Invalid` dates, `NotFound` for a sandbox the caller cannot see.
    pub async fn analytics_reviews(
        &self,
        ctx: &crate::auth::AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<yulius_contracts::api::analytics::ReviewAnalytics> {
        use yulius_contracts::api::analytics::{ModelCount, ReviewAnalytics};
        let (sandboxes, from_at, to_at, range) =
            self.analytics_window(ctx, sandbox, from, to).await?;
        let mut rounds = self
            .deps
            .store
            .merged_review_rounds(&sandboxes, from_at, to_at)
            .await?;
        // In the order the reports ended (stable: start order otherwise), so a round's last
        // report is the one that ended last, also when a reviewer cut off was resumed.
        rounds.sort_by_key(|r| r.ended_at);
        let facts: Vec<yulius_domain::review::RoundFact> = rounds
            .into_iter()
            .map(|r| yulius_domain::review::RoundFact {
                // A pull request is its merge: a playground's merges have no number.
                pull: (
                    r.task_id.to_string(),
                    r.merged_at.map_or(0, |at| u64::try_from(at.0).unwrap_or(0)),
                ),
                round: r.round,
                model: r.model_answered.unwrap_or(r.model),
                verdict: r
                    .verdict
                    .as_deref()
                    .and_then(yulius_domain::review::Verdict::parse),
                reported: r.ended_at.is_some(),
                no_fable: r.reason == "no_fable",
            })
            .collect();
        let numbers = yulius_domain::review::review_numbers(&facts);
        Ok(ReviewAnalytics {
            range,
            pull_requests: numbers.pull_requests,
            first_rounds_requesting_changes: numbers.first_rounds_requesting_changes,
            first_rounds_without_report: numbers.first_rounds_without_report,
            median_rounds: numbers.median_rounds,
            rounds_by_model: numbers
                .rounds_by_model
                .into_iter()
                .map(|(model, rounds)| ModelCount { model, rounds })
                .collect(),
            no_fable_rounds: numbers.no_fable_rounds,
            re_reviews_after_changes: numbers.re_reviews_after_changes,
            re_reviews_asking_again: numbers.re_reviews_asking_again,
            rounds_after_approval: numbers.rounds_after_approval,
            rounds_after_approval_asking: numbers.rounds_after_approval_asking,
        })
    }

    /// `GET /api/analytics/merge-queue` (SPEC-493 MQC-005): the merge queue's candidates formed,
    /// merged and dissolved per day in the visible sandboxes (or `sandbox`), and the minutes
    /// from arming a merge to it, read from the audit log (at most 20,000 entries a sandbox).
    ///
    /// # Errors
    /// `Invalid` for a range whose start is after its end; `NotFound` for a sandbox the caller
    /// cannot see; store failures.
    pub async fn analytics_merge_queue(
        &self,
        ctx: &crate::auth::AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<yulius_contracts::api::analytics::MergeQueueAnalytics> {
        use yulius_contracts::api::analytics::{
            CiMinutesDay, MergeQueueAnalytics, MergeQueueDay, MergeQueueDissolved,
        };
        use yulius_domain::merge_queue as mq;
        let (sandboxes, from_at, to_at, range) =
            self.analytics_window(ctx, sandbox, from, to).await?;
        let mut actions = vec![mq::FORMED, mq::MERGED, mq::DISSOLVED];
        actions.extend(mq::ARMED);
        actions.extend(mq::MERGE);
        // A merge in the range may have been armed before it: entries are read from 30 days
        // earlier, and only those in the range are counted (SPEC-493, fix 4's reviews).
        const ARMED_BEFORE_MS: i64 = 30 * 24 * 60 * 60 * 1000;
        let read_from = from_at.map(|at| at.plus_millis(-ARMED_BEFORE_MS));
        let mut events = Vec::new();
        for id in &sandboxes {
            let Some(record) = self.deps.store.sandbox(id).await? else {
                continue;
            };
            // A person's armed merge names the task by its id; the rest by `<slug>#<n>`.
            let mut tasks = self.deps.store.tasks(id, false).await?;
            tasks.extend(self.deps.store.tasks(id, true).await?);
            let numbered: std::collections::HashMap<String, String> = tasks
                .iter()
                .map(|t| (t.id.to_string(), format!("{}#{}", record.slug, t.number)))
                .collect();
            let ids: Vec<String> = numbered.keys().cloned().collect();
            let entries = self
                .deps
                .store
                .audit_of_sandbox(&record.slug, &ids, &actions, read_from, to_at, 20_000)
                .await?;
            events.extend(entries.into_iter().map(|(target, entry)| mq::QueueEvent {
                at: entry.at,
                target: numbered.get(&target).cloned().unwrap_or(target),
                action: entry.action,
                detail: entry.detail.unwrap_or_default(),
            }));
        }
        // CI minutes by how the runs ended, from each sandbox's daily tally (SPEC-497 OFC-003).
        let mut minutes = mq::Minutes::default();
        let mut minute_days: std::collections::BTreeMap<String, mq::Minutes> =
            std::collections::BTreeMap::new();
        for id in &sandboxes {
            for row in self.deps.store.ci_minutes(id, from_at, to_at).await? {
                minutes.add(&row.outcome, row.run_ms, row.rerun_ms);
                minute_days
                    .entry(row.day)
                    .or_default()
                    .add(&row.outcome, row.run_ms, row.rerun_ms);
            }
        }
        // Minutes CI ran nothing because of its configuration (SPEC-504 CFG-004), from the
        // episodes recorded, one started before the range included.
        let mut config_blocked_minutes = 0;
        let until = to_at.map_or(self.now(), |to| to.min(self.now()));
        for id in &sandboxes {
            let Some(record) = self.deps.store.sandbox(id).await? else {
                continue;
            };
            let entries = self
                .deps
                .store
                .audit_of_sandbox(
                    &record.slug,
                    &[],
                    &[mq::CONFIG_BLOCKED, mq::CONFIG_UNBLOCKED],
                    read_from,
                    to_at,
                    20_000,
                )
                .await?;
            let episodes: Vec<(yulius_domain::time::Timestamp, bool)> = entries
                .into_iter()
                .map(|(_, entry)| (entry.at, entry.action == mq::CONFIG_BLOCKED))
                .collect();
            config_blocked_minutes += mq::blocked_minutes(&episodes, from_at, until);
        }
        let numbers = mq::queue_numbers(&events, from_at);
        let mut totals = MergeQueueAnalytics {
            range,
            dissolved_recent: numbers
                .dissolved_recent
                .iter()
                .map(|(why, event)| MergeQueueDissolved {
                    at: yulius_domain::time::format_rfc3339(event.at),
                    sandbox: event.target.clone(),
                    why: why.as_str().to_owned(),
                    reason: event.detail.clone(),
                })
                .collect(),
            merges: numbers.merges,
            median_arm_to_merge_minutes: numbers.median_arm_to_merge_minutes,
            ci_minutes: ci_minutes(minutes),
            ci_minutes_by_day: minute_days
                .into_iter()
                .map(|(day, minutes)| CiMinutesDay {
                    day,
                    minutes: ci_minutes(minutes),
                })
                .collect(),
            config_blocked_minutes,
            ..MergeQueueAnalytics::default()
        };
        for day in numbers.days {
            totals.formed += day.formed;
            totals.merged += day.merged;
            let dissolved: std::collections::BTreeMap<String, u32> = day
                .dissolved
                .into_iter()
                .map(|(why, n)| (why.to_owned(), n))
                .collect();
            for (why, n) in &dissolved {
                totals.dissolved += n;
                *totals.dissolved_by.entry(why.clone()).or_default() += n;
            }
            totals.days.push(MergeQueueDay {
                day: day.day,
                formed: day.formed,
                merged: day.merged,
                dissolved,
            });
        }
        Ok(totals)
    }

    /// `GET /api/analytics/ci-load` (SPEC-497): the host's load from CI per hour over `[from,
    /// to]`, for any signed-in person (the host's figures, no project's).
    ///
    /// # Errors
    /// A malformed or reversed range.
    pub async fn analytics_ci_load(
        &self,
        ctx: &crate::auth::AuthContext,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<yulius_contracts::api::analytics::CiLoadAnalytics> {
        use yulius_contracts::api::analytics::{CiLoadAnalytics, CiLoadHour};
        let (_, from_at, to_at, range) = self.analytics_window(ctx, None, from, to).await?;
        let minutes = self.deps.store.ci_load(from_at, to_at).await?;
        let hours = yulius_domain::resources::load_by_hour(&minutes);
        let tenth = |v: f64| (v * 10.0).round() / 10.0;
        let busy: Vec<f64> = hours.iter().filter_map(|h| h.busy_percent).collect();
        #[expect(clippy::cast_precision_loss, reason = "hours in a range")]
        let busy_percent =
            (!busy.is_empty()).then(|| tenth(busy.iter().sum::<f64>() / busy.len() as f64));
        Ok(CiLoadAnalytics {
            range,
            wait_job_minutes: hours.iter().map(|h| h.wait_job_minutes).sum(),
            peak_wait_job_minutes: hours.iter().map(|h| h.wait_job_minutes).max().unwrap_or(0),
            busy_percent,
            hours: hours
                .into_iter()
                .map(|h| CiLoadHour {
                    hour: h.hour,
                    minutes: h.minutes,
                    wait_job_minutes: h.wait_job_minutes,
                    busy_percent: h.busy_percent.map(tenth),
                    held_cpus: tenth(h.held_cpus),
                    measured_cpus: h.measured_cpus.map(tenth),
                })
                .collect(),
        })
    }

    /// Prefer rounds in progress. A finished delivery can also show its merged review.
    pub(crate) async fn review_round_views(
        &self,
        delivery: &crate::delivery::DeliveryRecord,
    ) -> Vec<yulius_contracts::api::git::ReviewRoundView> {
        let store = &self.deps.store;
        let rounds = if crate::delivery::past_merge(&delivery.stage) {
            store
                .delivery_review_rounds(&delivery.task_id, delivery.pr_number)
                .await
        } else {
            store.review_rounds(&delivery.task_id, true).await
        }
        .unwrap_or_default();
        let open_pr = (delivery.stage == yulius_domain::delivery::DeliveryStage::PrOpen)
            .then_some(delivery.pr_number)
            .flatten();
        rounds
            .into_iter()
            .map(|round| {
                let pr = round.pr_number.or(open_pr);
                round_view(round, pr)
            })
            .collect()
    }

    /// Writes the merge record of `task`'s pull request `pr`, merged at `at` (the time the
    /// store closed its rounds) with `merged` (its head and the integration branch before it)
    /// when known: the models of the runs that worked on it (those still going when the
    /// previous pull request merged, and every later one) and each of its review rounds with its
    /// model, verdict and head (SPEC-493 REV-005). A merge whose head no recorded round approved
    /// (that head, or one followed only by clean merges of the integration branch at
    /// [`MergedHead::base`], the reviewed-head rule) says so; without a known head, any approving
    /// round counts.
    pub async fn record_review_models(
        &self,
        task: &yulius_domain::ids::TaskId,
        pr: Option<u64>,
        at: Timestamp,
        merged: Option<&MergedHead>,
    ) {
        let store = &self.deps.store;
        let Ok(all) = store.review_rounds(task, false).await else {
            return;
        };
        let Ok(Some(record)) = store.task(task).await else {
            return;
        };
        let target = match store.sandbox(&record.sandbox_id).await {
            Ok(Some(sandbox)) => format!("{}#{}", sandbox.slug, record.number),
            _ => task.to_string(),
        };
        let (this, before): (Vec<_>, Vec<_>) =
            all.into_iter().partition(|r| r.merged_at == Some(at));
        // The task's previous pull request: the latest merge before this one, by its rounds or
        // by its own merge record (a merge with no recorded round writes one too).
        let recorded = store
            .last_audit(&target, "delivery.review.models", Timestamp(0))
            .await
            .ok()
            .flatten()
            .map(|entry| entry.at)
            .filter(|earlier| *earlier <= at);
        let since = before
            .iter()
            .filter_map(|r| r.merged_at)
            .filter(|merged| *merged < at)
            .chain(recorded)
            .max();
        let own = record
            .model
            .clone()
            .unwrap_or_else(|| yulius_domain::harness::DEFAULT_CLAUDE_MODEL.to_owned());
        let mut written: Vec<String> = Vec::new();
        for attempt in store.attempts(task).await.unwrap_or_default() {
            // A run that ended before the previous merge wrote that pull request, not this one.
            if since.is_some_and(|since| attempt.ended_at.is_some_and(|ended| ended < since)) {
                continue;
            }
            let model = attempt.model.unwrap_or_else(|| own.clone());
            if !written.contains(&model) {
                written.push(model);
            }
        }
        let rounds: Vec<String> = this
            .iter()
            .map(|r| {
                format!(
                    "{} {} {}{}{}",
                    r.round,
                    r.model_answered.as_deref().unwrap_or(&r.model),
                    r.verdict.as_deref().unwrap_or(if r.ended_at.is_some() {
                        "no verdict"
                    } else {
                        "no report"
                    }),
                    r.head
                        .as_deref()
                        .map(|h| format!(" at {}", h.chars().take(9).collect::<String>()))
                        .unwrap_or_default(),
                    if r.reason == "no_fable" {
                        " (no Fable)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        let mut approved = false;
        for round in this
            .iter()
            .filter(|r| r.verdict.as_deref() == Some("approve"))
        {
            approved = match (merged, round.head.as_deref()) {
                (None, _) => true,
                (Some(merged), Some(head)) if same_commit(head, &merged.head) => true,
                // Followed only by clean merges of the integration branch at the merge's base, or
                // by a follow-up within the bound (SPEC-493 APR-001; review round 1 of fix 6);
                // with that base unknown, only the approved head itself counts.
                (Some(merged), Some(head)) => {
                    match (merged.base.as_deref(), record.worktree_path.as_deref()) {
                        (Some(base), Some(worktree)) => {
                            let worktree = Path::new(worktree);
                            self.review_covers_against(
                                &record,
                                worktree,
                                head,
                                &merged.head,
                                Some(base),
                            )
                            .await
                                || self
                                    .follow_up_since_review(
                                        &record,
                                        worktree,
                                        head,
                                        &merged.head,
                                        Some(base),
                                    )
                                    .await
                                    .is_ok()
                        }
                        _ => false,
                    }
                }
                (Some(_), None) => false,
            };
            if approved {
                break;
            }
        }
        let detail = format!(
            "{}: written by {}; review rounds: {}{}",
            pr.map_or_else(|| "the merge".to_owned(), |n| format!("pull request #{n}")),
            if written.is_empty() {
                "unknown".to_owned()
            } else {
                written.join(", ")
            },
            if rounds.is_empty() {
                "none recorded".to_owned()
            } else {
                rounds.join("; ")
            },
            if approved {
                ""
            } else {
                "; no recorded review round approved its head"
            }
        );
        if let Err(err) = store
            .audit(
                self.now(),
                "yulius",
                "delivery.review.models",
                Some(&target),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %task, "merge record of the review not written");
        }
    }
}

/// Whether `a` and `b` name the same commit: the shorter (at least 7 hex digits) is a prefix of
/// the longer, as `delivery_merge` reads a reviewed head.
fn same_commit(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    crate::agent_delivery::same_commit(short, long)
}

/// The head a merge merged, and when known a commit of the integration branch that holds none
/// of the pull request's own commits: what a review is judged against (SPEC-493 REV-005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedHead {
    /// The merged head.
    pub head: String,
    /// `<merge>^1` for a GitHub merge: the branch before a squash or merge commit, the previous
    /// (rewritten) copy for a rebase merge. A playground's base after its own squash.
    pub base: Option<String>,
    /// Actual landed commit, when known. The review base may differ from it.
    pub landing: Option<crate::delivery::evidence::Landing>,
}

impl MergedHead {
    /// What a merged pull request merged: its head, and the parent its merge commit landed on
    /// when GitHub names that commit (a merge found after it was made, SPEC-493 fix 1d).
    #[must_use]
    pub fn of_pull(pull: &crate::ports::github::GhPull) -> Self {
        Self {
            head: pull.head_sha.clone(),
            landing: crate::delivery::evidence::Landing::closed_pull(pull),
            base: pull.merge_commit_sha.as_ref().map(|sha| format!("{sha}^1")),
        }
    }
}

/// A round as the delivery view shows it.
fn round_view(
    r: ReviewRoundRecord,
    pr_number: Option<u64>,
) -> yulius_contracts::api::git::ReviewRoundView {
    use yulius_domain::time::format_rfc3339;
    yulius_contracts::api::git::ReviewRoundView {
        round: r.round,
        agent: r.agent,
        model: r.model,
        model_answered: r.model_answered,
        reason: r.reason,
        head: r.head,
        verdict: r.verdict,
        reported: r.ended_at.is_some(),
        cut_off: r.cut_off,
        started_at: format_rfc3339(r.started_at),
        ended_at: r.ended_at.map(format_rfc3339),
        pr_number,
        merged_at: r.merged_at.map(format_rfc3339),
    }
}

/// The contract's form of [`yulius_domain::merge_queue::Minutes`], rounded to tenths.
fn ci_minutes(
    minutes: yulius_domain::merge_queue::Minutes,
) -> yulius_contracts::api::analytics::CiMinutes {
    let tenth = |m: f64| (m * 10.0).round() / 10.0;
    yulius_contracts::api::analytics::CiMinutes {
        passed: tenth(minutes.passed),
        failed: tenth(minutes.failed),
        failed_outside: tenth(minutes.failed_outside),
        cancelled: tenth(minutes.cancelled),
        rerun: tenth(minutes.rerun),
        outside_share: minutes
            .outside_share()
            .map(|share| (share * 1000.0).round() / 1000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-493 RLM-002 (review round 1 of fix 7): what a session is told of its cut-off
    /// reviewer reads as whole sentences, and claims nothing of the subscription it is on.
    #[test]
    fn a_cut_off_reviewer_is_told_in_whole_sentences() {
        let reviewer = CutOffReviewer {
            agent_id: "arev1".to_owned(),
            cut_off_at: Timestamp(0),
            model: "claude-fable-5-2".to_owned(),
        };
        let resume = reviewer_resume_prompt(Some("Gate says: fix it."), &reviewer);
        assert!(resume.starts_with("Gate says: fix it.\n\nYulius continued this session"));
        assert!(resume.contains(
            "a usage limit had cut off your reviewer `arev1` (Fable 5.2) in the middle of its \
             review round. Resume that reviewer now: SendMessage to `arev1`, asking it to \
             continue its review. Do not start a new reviewer for this round: the one you resume \
             keeps what it has read."
        ));
        let nowhere = reviewer_nowhere(&reviewer);
        assert!(nowhere.contains(
            "no Claude subscription can run Fable 5.2 now. Start the review again (the Agent \
             tool with the `yulius-reviewer` agent and the same prompt): this round then runs on \
             Opus 5.5, which says so in its summary."
        ));
        for text in [resume, nowhere] {
            assert!(!text.contains("  "), "{text}");
        }
    }
}
