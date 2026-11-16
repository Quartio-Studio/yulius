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
use yulius_domain::harness::claude_model_name;
use yulius_domain::review::{
    LATE_REVIEWER_AGENT, REVIEWER_AGENT, reviewer_model, round_agent, verdict_of,
};
use yulius_domain::usage::ModelFamily;

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::AppResult;
use crate::model::ReviewRoundRecord;
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
}

/// What the new process of a session moved past its reviewer's limit is told (RLM-002), after
/// the prompt it would open with (`after`).
#[must_use]
pub(crate) fn reviewer_resume_prompt(after: Option<&str>, reviewer: &CutOffReviewer) -> String {
    let told = format!(
        "Yulius continued this session in a new process on a Claude subscription that can run          {model}, because a usage limit cut off your reviewer `{id}` in the middle of its review          round. Resume that reviewer now: SendMessage to `{id}`, asking it to continue its          review. Do not start a new reviewer for this round: the one you resume keeps what it          has read. Then continue the task where you left off.",
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
        "Yulius: a usage limit cut off your reviewer `{id}` in the middle of its review round, and          no Claude subscription can run {model} now. Start the review again (the Agent tool with          the `yulius-reviewer` agent and the same prompt): this round then runs on Opus 5.5,          which says so in its summary.",
        model = claude_model_name(&reviewer.model),
        id = reviewer.agent_id,
    )
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
            match self.move_for_fable(ctx).await {
                FableMove::Refused(why) => return Ok(HookOutput::deny_tool(why)),
                FableMove::Nowhere => {
                    agent = LATE_REVIEWER_AGENT.to_owned();
                    reason = "no_fable";
                }
            }
        }
        let model = reviewer_model(&agent).unwrap_or_default().to_owned();
        let head = match ctx.task.worktree_path.as_deref() {
            Some(worktree) => self
                .deps
                .git
                .log(Path::new(worktree), "HEAD", 1)
                .await
                .ok()
                .and_then(|commits| commits.into_iter().next())
                .map(|commit| commit.sha),
            None => None,
        };
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
    async fn move_for_fable(self: &Arc<Self>, ctx: &AgentContext) -> FableMove {
        if let Ok(decided) =
            tokio::time::timeout(MOVE_DECISION, self.decide_move_for_fable(ctx)).await
        {
            return decided;
        }
        tracing::info!(task = %ctx.task.id, "no move for a Fable reviewer: the looks took longer than the hook may wait");
        FableMove::Nowhere
    }

    async fn decide_move_for_fable(self: &Arc<Self>, ctx: &AgentContext) -> FableMove {
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
        let fable = ModelFamily::of(Some(yulius_domain::harness::REVIEW_CLAUDE_MODEL));
        let Some(account) = yulius_domain::rotation::move_to_also_run(&input, fable) else {
            return FableMove::Nowhere;
        };
        // A move ends the session's background shells, agents and monitors and what waits in
        // Claude Code's queue; a look that gives no answer counts as work running (SPEC-483).
        let prompts_before = self.prompts_passed(&task.id);
        self.await_transcript_read(&task.id).await;
        if self.background_or_wait_now(&task.id).await != Some(false) {
            return FableMove::Refused(BUSY_FOR_FABLE.to_owned());
        }
        if self.prompt_waits_for_turns_end(&task.id, prompts_before) {
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
                tracing::info!(task = %task.id, error = %err, "no move for a Fable reviewer: the session could not start there");
                return FableMove::Nowhere;
            }
            Err(_) => return FableMove::Nowhere,
        }
        let app = Arc::clone(self);
        let task_id = task.id.clone();
        let run = ctx.attempt.id.clone();
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
            let prompt = Some(MOVED_FOR_FABLE_PROMPT.to_owned());
            if let Err(err) = app
                .switch_task_now(&task_id, Some(account), MOVE_FOR_FABLE, prompt)
                .await
            {
                tracing::warn!(error = %err, task = %task_id, "the move for a Fable reviewer failed");
            }
        });
        FableMove::Refused(MOVING_FOR_FABLE.to_owned())
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

    /// The cut-off reviewer `agent_id` of `task` started again or reported: nothing is left to
    /// tell a relaunch, and no move is due for it (RLM-002).
    fn reviewer_went_on(&self, task: &yulius_domain::ids::TaskId, agent_id: &str) {
        let Some(runtime) = self.existing_runtime(task) else {
            return;
        };
        if let Ok(mut cut) = runtime.cut_off_reviewer.lock()
            && cut.as_ref().is_some_and(|c| c.agent_id == agent_id)
        {
            *cut = None;
            runtime
                .reviewer_move
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// A usage limit cut off subagent `agent_id` of `task` (RLM-002). When it is an unfinished
    /// review round's reviewer, a relaunch tells the session to resume it; and when the limit
    /// binds the reviewer's model and not the session's (`moves`), the session moves to a
    /// subscription that can run both: at once when its thread is idle, else at its turn's
    /// end. A limit that binds the session's own model moves it as any limit does.
    pub(crate) async fn reviewer_cut_off(
        self: &Arc<Self>,
        task: &yulius_domain::ids::TaskId,
        agent_id: &str,
        moves: bool,
    ) {
        let Ok(rounds) = self.deps.store.review_rounds(task, true).await else {
            return;
        };
        let Some(round) = rounds
            .iter()
            .find(|r| r.ended_at.is_none() && r.subagent_id.as_deref() == Some(agent_id))
        else {
            return;
        };
        let runtime = self.runtime(task);
        if let Ok(mut cut) = runtime.cut_off_reviewer.lock() {
            *cut = Some(CutOffReviewer {
                agent_id: agent_id.to_owned(),
                model: round.model.clone(),
            });
        }
        if !moves {
            return;
        }
        if runtime.turn_in_progress() {
            runtime
                .reviewer_move
                .store(true, std::sync::atomic::Ordering::SeqCst);
        } else {
            let app = Arc::clone(self);
            let task = task.clone();
            tokio::spawn(async move { app.move_for_reviewer(&task).await });
        }
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
        let Some(reviewer) = runtime.cut_off_reviewer.lock().ok().and_then(|c| c.clone()) else {
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
        if let Ok(mut cut) = runtime.cut_off_reviewer.lock() {
            *cut = None;
        }
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

    /// Moves `task_id`'s idle session for its cut-off reviewer (RLM-002): while background work
    /// runs or a prompt waits (which a move would end), or a move cannot be decided now, at a
    /// later turn's end instead.
    pub(crate) async fn move_for_reviewer(self: &Arc<Self>, task_id: &yulius_domain::ids::TaskId) {
        let runtime = self.runtime(task_id);
        let later = || {
            runtime
                .reviewer_move
                .store(true, std::sync::atomic::Ordering::SeqCst);
        };
        let (Ok(Some(task)), Ok(Some(attempt))) = (
            self.deps.store.task(task_id).await,
            self.deps.store.live_attempt(task_id).await,
        ) else {
            return;
        };
        let account = match self
            .reviewer_move_target(&task, attempt.account_id.clone())
            .await
        {
            ReviewerMove::To(account) => account,
            ReviewerMove::Later => return later(),
            ReviewerMove::Nothing => return,
        };
        let prompts_before = self.prompts_passed(task_id);
        self.await_transcript_read(task_id).await;
        if runtime.turn_in_progress()
            || self.background_or_wait_now(task_id).await != Some(false)
            || self.prompt_waits_for_turns_end(task_id, prompts_before)
        {
            return later();
        }
        tracing::info!(task = %task_id, "the session moves for its cut-off reviewer");
        if let Err(err) = self
            .switch_task_now(task_id, Some(account), REVIEW_RESUME, None)
            .await
        {
            tracing::warn!(error = %err, task = %task_id, "the move for a cut-off reviewer failed");
        }
    }

    /// What a subagent's transcript says of a review round's reviewer: the model that answered
    /// it, or that a usage limit cut it off. A subagent that is no round's changes nothing.
    pub(crate) async fn review_subagent_noted(
        &self,
        task: &yulius_domain::ids::TaskId,
        subagent: &str,
        model: Option<&str>,
        cut_off: bool,
    ) {
        if let Err(err) = self
            .deps
            .store
            .note_review_subagent(task, subagent, model, cut_off)
            .await
        {
            tracing::warn!(error = %err, task = %task, "review round not noted");
        }
    }

    /// `SubagentStart`: links a reviewer's subagent to the round that started it. A resumed
    /// subagent (the same id) changes nothing, but that it was resumed (RLM-002).
    pub(crate) async fn review_subagent_started(&self, ctx: &AgentContext, input: &HookInput) {
        let (Some(agent), Some(id)) = (input.agent_type.as_deref(), input.agent_id.as_deref())
        else {
            return;
        };
        self.reviewer_went_on(&ctx.task.id, id);
        if !REVIEWERS.contains(&agent) {
            return;
        }
        if let Err(err) = self
            .deps
            .store
            .link_review_subagent(&ctx.task.id, &[agent], id)
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "review subagent not linked");
        }
    }

    /// `SubagentStop`: a reviewer's report ends its round, with the verdict its last line says.
    pub(crate) async fn review_subagent_stopped(&self, ctx: &AgentContext, input: &HookInput) {
        let Some(id) = input.agent_id.as_deref() else {
            return;
        };
        self.reviewer_went_on(&ctx.task.id, id);
        if !input
            .agent_type
            .as_deref()
            .is_some_and(|agent| REVIEWERS.contains(&agent))
        {
            return;
        }
        let verdict = input
            .last_assistant_message
            .as_deref()
            .and_then(verdict_of)
            .map(|verdict| verdict.as_str());
        if let Err(err) = self
            .deps
            .store
            .end_review_round(&ctx.task.id, id, self.now(), verdict, None)
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "review round not ended");
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
        let rounds = self
            .deps
            .store
            .merged_review_rounds(&sandboxes, from_at, to_at)
            .await?;
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
        use yulius_contracts::api::analytics::{MergeQueueAnalytics, MergeQueueDay};
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
        let numbers = mq::queue_numbers(&events, from_at);
        let mut totals = MergeQueueAnalytics {
            range,
            merges: numbers.merges,
            median_arm_to_merge_minutes: numbers.median_arm_to_merge_minutes,
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

    /// The rounds of `task`'s open pull request, as the delivery view shows them. A failed read
    /// shows none.
    pub(crate) async fn review_round_views(
        &self,
        task: &yulius_domain::ids::TaskId,
    ) -> Vec<yulius_contracts::api::git::ReviewRoundView> {
        self.deps
            .store
            .review_rounds(task, true)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(round_view)
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
}

impl MergedHead {
    /// What a merged pull request merged: its head, and the parent its merge commit landed on
    /// when GitHub names that commit (a merge found after it was made, SPEC-493 fix 1d).
    #[must_use]
    pub fn of_pull(pull: &crate::ports::github::GhPull) -> Self {
        Self {
            head: pull.head_sha.clone(),
            base: pull.merge_commit_sha.as_ref().map(|sha| format!("{sha}^1")),
        }
    }
}

/// A round as the delivery view shows it.
fn round_view(r: ReviewRoundRecord) -> yulius_contracts::api::git::ReviewRoundView {
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
    }
}
