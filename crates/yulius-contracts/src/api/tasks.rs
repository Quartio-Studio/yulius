//! Tasks, steps, messages, metrics and feedback.

use serde::{Deserialize, Serialize};

use super::common::UserRef;
use super::git::Delivery;

/// Step progress summary.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StepProgress {
    /// Finished steps (done or skipped).
    pub done: u32,
    /// All steps.
    pub total: u32,
    /// Current step title.
    pub current: Option<String>,
    /// Finished share of the plan's effort, 0-100, rounded down (STP-006): 9 of 10 equal steps
    /// is 90.
    #[serde(default)]
    pub percent: u8,
    /// Share of the plan's effort in progress, 0-100: `percent + active_percent` is the finished
    /// and in-progress share rounded down (#150). 0 from older servers.
    #[serde(default)]
    pub active_percent: u8,
    /// How many steps the plan had when its first step was finished, when it has more now: the
    /// plan grew by `total - first_total` steps since (SPEC-484 PLN-010). Absent for a plan
    /// that did not grow, one with no finished step, and from older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_total: Option<u32>,
}

/// The delivery whose checks affect a parked task's group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkedPr {
    /// The pull request number observed on the branch or recorded by delivery.
    pub number: u64,
    /// Open deliveries need another checks read when the overview refreshes.
    pub open: bool,
}

/// A task in lists and events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// The installation is reserving its remaining requests for delivery.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub github_display_paused: bool,
    /// A live task whose goal is achieved still has a recorded integration failure that the PR
    /// watch has not cleared: Yulius re-checks it and tells the agent, so it is not Done
    /// (SPEC-621 FIN-004). Other tasks and earlier servers omit it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub candidate_pending: bool,
    /// Whether the server groups a parked task under Needs you. Staging can wake a parked task.
    /// Other tasks and summaries from earlier servers omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked_needs_you: Option<bool>,
    /// The current branch or recorded delivery, including PRs missing from the Git overview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked_pr: Option<ParkedPr>,
    /// Task id.
    pub id: String,
    /// Sandbox id.
    pub sandbox_id: String,
    /// Number within the sandbox.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Harness wire name (`claude`, `codex` or `antigravity`; SPEC-503, SPEC-505).
    pub harness: String,
    /// `random` when the harness was drawn at creation, else `chosen` (SPEC-505 RND-001,
    /// SPEC-520 RHM-003).
    #[serde(default = "chosen")]
    pub harness_choice: String,
    /// Status wire name (domain `TaskStatus::as_str`).
    pub status: String,
    /// `working`, `waiting`, `needs_attention`, `done` (the goal is achieved and the turn ended,
    /// TSK-014), or null.
    pub activity: Option<String>,
    /// Task branch.
    pub branch: Option<String>,
    /// `ui`, `automation`, `import`, `onboarding`, `agent`, `telegram` or `handoff`.
    pub origin: String,
    /// Creator: who asked for the task.
    pub created_by: UserRef,
    /// Owner (SPEC-070): the task acts on GitHub as them, and only they change it.
    pub owner: UserRef,
    /// A person's request for control that waits for the owner's answer (SPEC-070), or null.
    #[serde(default)]
    pub control_request: Option<ControlRequestView>,
    /// Who handed the task over on request, or had it taken (SPEC-510), and may take it back
    /// without asking (SPEC-490 OWN-009), or null.
    #[serde(default)]
    pub control_from: Option<UserRef>,
    /// Whether the owner chose to be asked before anyone takes control (SPEC-510 TKC-005):
    /// then others request control; else they take it.
    #[serde(default)]
    pub owner_asks_first: bool,
    /// Creation time.
    pub created_at: String,
    /// Last change of anything (title, owner, archive, status …).
    pub updated_at: String,
    /// Since when the task is in its current state: the last change of its status or activity
    /// (SPEC-087 TIM-001). Absent from an older server.
    #[serde(default)]
    pub state_since: Option<String>,
    /// The start of its current working stretch, in which it has not waited for a person: kept
    /// through its idle turns while background work wakes it, and through subscription switches
    /// (TIM-002). Null while it waits for a person, is queued or has ended.
    #[serde(default)]
    pub working_since: Option<String>,
    /// Step progress.
    pub steps: StepProgress,
    /// Archived flag.
    pub archived: bool,
    /// Label of the account in use.
    pub account_label: Option<String>,
    /// Id of the account in use, which "Run on…" marks (SPEC-447 MOV-001); absent without a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Email of the Claude account in use: in a task's detail for instance administrators only
    /// (UI-022); absent otherwise and while unknown (SPEC-244).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    /// `goal` or `prompt` (TSK-010).
    #[serde(default = "default_mode")]
    pub mode: String,
    /// While `waiting_capacity` (every account at its usage limit): when the earliest known limit
    /// resets and the task resumes, RFC 3339 (ROT-010). Null otherwise or when unknown.
    #[serde(default)]
    pub resumes_at: Option<String>,
    /// While `waiting_capacity`: no Claude account is usable, so it waits for one to be connected
    /// rather than for capacity (SPEC-246 CLA-004).
    #[serde(default)]
    pub waits_for_account: bool,
    /// While the agent is idle (`waiting` or `done`): background work it left running: shells, as
    /// last probed (TSK-014), and the background agents Claude Code still waited for at its last
    /// turn end (SPEC-032), whichever is more. Null when not known or not idle.
    #[serde(default)]
    pub background_shells: Option<u32>,
    /// Since when those shells have been running, RFC 3339 (complaint 1: a wait that never
    /// ends shows its age). Null when none run or not known.
    #[serde(default)]
    pub background_shells_since: Option<String>,
    /// While the agent is idle: it waits in `yulius wait` for its checks, its delivery or an
    /// armed merge (not counted in `background_shells`, SPEC-050 DSK-102), or Yulius has news of
    /// its pull request for it without such a wait: a merge is armed, or its merge or how an
    /// armed merge ended is not said yet (SPEC-487 CHN-001). It goes on by itself
    /// when Yulius has news, so the task is neither Done nor the person's turn (SPEC-085 STT-002).
    #[serde(default)]
    pub yulius_wait: bool,
    /// The stage of the durable wait for a merge-first staging outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_wait: Option<String>,
    /// Latest settled or inherited staging receipt, independent of task activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_outcome: Option<String>,
    /// While the agent is idle: its merge is armed, so Yulius merges it and tells the agent; the
    /// task is not Done, an achieved goal neither (SPEC-487 CHN-001, SPEC-483 GRP-003).
    #[serde(default)]
    pub merge_armed: bool,
    /// While the agent is idle: Claude Code last recorded its goal as set, not met or failed, so
    /// the agent that stopped left work unfinished and it is the person's turn. Without it, or an
    /// open plan, an idle agent answered (SPEC-429).
    #[serde(default)]
    pub goal_unfinished: bool,
    /// While the agent is idle: the task's spec says its work is not finished (`status` other
    /// than `done` or `abandoned`), so work it declared is left, however complete its plan
    /// reads: the task is not Done, whatever its pull request says (SPEC-491 DON-004).
    #[serde(default)]
    pub spec_unfinished: bool,
    /// The task this one runs after (SPEC-001 TSK-012): while `chained` it waits for it; the
    /// link stays after the task started.
    #[serde(default)]
    pub after: Option<Box<ChainLink>>,
    /// Tasks chained to run after this one.
    #[serde(default)]
    pub next: Vec<ChainLink>,
    /// The task this one was forked from (SPEC-628 FRK-008): its conversation and commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<Box<ForkOrigin>>,
    /// The tasks forked from this one, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forks: Vec<ForkLink>,
    /// A prompt typed into the session that Yulius held for a decision (TSK-013).
    #[serde(default)]
    pub held_prompt: Option<Box<HeldPrompt>>,
    /// What the agent asked a person to do (`needs_you`, SPEC-134); the task needs a person
    /// until someone writes to it or the agent resolves it.
    #[serde(default)]
    pub needs_you: Option<NeedsYouView>,
    /// Actionable explanation of why the task is currently in Needs You (SPEC-578): what
    /// happened, what a person needs to do, where to do it, and when the wait episode began.
    /// Absent whenever the task is not in Needs You.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_you_notice: Option<NeedsYouNoticeView>,
    /// Why the task reads `needs_attention` (SPEC-472): the reason of that flag, while the task
    /// reads it. Null in every other state, and for a flag raised without one.
    #[serde(default)]
    pub attention: Option<AttentionView>,
    /// How "Auto" chose the task's mode and workflow when it was created (TSK-015); null when the
    /// person picked both.
    #[serde(default)]
    pub auto: Option<AutoChoice>,
    /// A supertask (SPEC-071): it runs alone in its project.
    #[serde(default)]
    pub supertask: bool,
    /// While `queued` because of a supertask (SPEC-071): the numbers of the tasks it waits for —
    /// for a supertask the working tasks and earlier supertasks of its project, for another task
    /// the project's pending supertasks. Empty otherwise.
    #[serde(default)]
    pub held_by: Vec<u32>,
    /// While `queued` and not held by a supertask: what it waits for, one line ("Waiting for a
    /// free session: 8 of 8 in use", "Waiting for the Yulius update to finish", "Starting: a
    /// session is free"; SPEC-512 PRK-007). Null otherwise.
    #[serde(default)]
    pub queue_reason: Option<String>,
    /// While `prepared` (SPEC-105): when it starts by itself, RFC 3339; null when it waits for a
    /// person ("when I decide") or is not prepared.
    #[serde(default)]
    pub start_at: Option<String>,
    /// The task rests without ever having run (`closed` with no attempt: created resting, or
    /// ended before its first run), so it is not shown as Closed (SPEC-153 OLC-005).
    #[serde(default)]
    pub not_started: bool,
    /// It never ran and is the onboarding task its `pending` sandbox waits for: it starts with
    /// Start onboarding (SPEC-153 OLC-005).
    #[serde(default)]
    pub awaits_start_onboarding: bool,
    /// Why its live run is paused (SPEC-191 PAU-001): an update holds it or a person paused it.
    /// Null otherwise; a capacity pause is the `waiting_capacity` status.
    #[serde(default)]
    pub pause: Option<TaskPause>,
    /// Its open pull request made no progress for an hour while its CI was idle (SPEC-444
    /// PRN-009): what Yulius told the task, in one sentence. Null otherwise.
    #[serde(default)]
    pub pr_stalled: Option<String>,
    /// The live tasks whose scope claims overlap this live task's (SPEC-493 SCO-002), until a
    /// claim is released or a task is no longer live. Empty otherwise.
    #[serde(default)]
    pub overlaps: Vec<ClaimOverlap>,
}

/// Another live task whose claims overlap a task's (SPEC-493 SCO-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimOverlap {
    /// Its number.
    pub task_number: u32,
    /// Its title.
    pub task_title: String,
    /// This task's patterns that overlap its claims.
    pub patterns: Vec<String>,
}

/// Why a task's live run is paused (SPEC-191 PAU-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPause {
    /// `update` (an instance update holds it) or `person` (someone paused it).
    pub by: String,
    /// Who paused it (`person` only).
    pub user: Option<UserRef>,
    /// Since when, RFC 3339 (`person` only).
    pub since: Option<String>,
    /// The pause has not taken hold yet: its run cannot be frozen and its turn goes on.
    pub pending: bool,
}

/// How "Auto" chose a new task's mode and workflow (SPEC-001 TSK-015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoChoice {
    /// How the mode was chosen: `jev` (Jev was confident), `default` (Goal: Jev not configured,
    /// failing or unsure), or null when the person picked it.
    pub mode: Option<String>,
    /// How the workflow was chosen: `jev`, `default` ("Feature" or none), or null when the person
    /// picked it.
    pub workflow: Option<String>,
    /// The task's workflow when it was created; null without one.
    pub workflow_name: Option<String>,
}

/// A request for control of a task (SPEC-070).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRequestView {
    /// Who asks.
    pub by: UserRef,
    /// When, RFC 3339.
    pub at: String,
}

/// One end of a chain (SPEC-001 TSK-012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainLink {
    /// Task id; null when the predecessor was deleted.
    pub task_id: Option<String>,
    /// Task number.
    pub number: u32,
    /// Title; null when deleted.
    pub title: Option<String>,
    /// Status wire name; null when deleted.
    pub status: Option<String>,
    /// On a waiting chained task: why it needs a person instead of starting by itself (its
    /// predecessor stopped, failed, was cancelled, closed before it was done or was deleted).
    #[serde(default)]
    pub blocked: Option<String>,
    /// On a task waiting for a start rule (SPEC-017): the rule in canonical text, for example
    /// `#12 done and #13 done`. This link then shows the first task the rule names. Null for a
    /// plain "run after" chain.
    #[serde(default)]
    pub rule: Option<String>,
}

/// Where a fork came from (SPEC-628 FRK-008): the source task when it was forked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkOrigin {
    /// The source task's id; null once it was deleted.
    pub task_id: Option<String>,
    /// Its number.
    pub number: u32,
    /// Its title when it was forked.
    pub title: String,
    /// Its branch.
    pub branch: String,
    /// The commit of its branch the fork started from.
    pub commit: String,
    /// The session whose copy the fork continued.
    pub session_id: String,
    /// When it was forked (RFC 3339).
    pub forked_at: String,
}

/// A task forked from this one (SPEC-628 FRK-008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkLink {
    /// The fork's id.
    pub task_id: String,
    /// Its number.
    pub number: u32,
    /// Its title.
    pub title: String,
    /// Its status wire name.
    pub status: String,
    /// The commit it started from.
    pub commit: String,
}

/// `POST /api/tasks/{tid}/fork` (SPEC-628 FRK-001): a new task continues a copy of this task's
/// conversation from its branch's head, on the same harness.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ForkTask {
    /// What the fork should do differently: its first message (at most 64 KiB).
    #[serde(default)]
    pub message: Option<String>,
    /// Title; `Fork of #N: <title>` when absent.
    #[serde(default)]
    pub title: Option<String>,
    /// Model of the source's harness; the harness default when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// Effort; the default when absent.
    #[serde(default)]
    pub effort: Option<String>,
    /// Subscription of the harness's provider; the pool's choice when absent.
    #[serde(default)]
    pub account_id: Option<String>,
}

/// What a task's agent asked a person to do (SPEC-134 CDT-004, `needs_you`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsYouView {
    /// What to do, or what to decide when `decision`, one line.
    pub action: String,
    /// What happened (SPEC-327).
    #[serde(default)]
    pub happened: Option<String>,
    /// What is wrong or blocked because of it.
    #[serde(default)]
    pub problem: Option<String>,
    /// The person chooses rather than does: read "Decide" instead of "To do".
    #[serde(default)]
    pub decision: bool,
    /// Where to do it (a Yulius page).
    #[serde(default)]
    pub link: Option<String>,
    /// When it was asked (RFC 3339).
    pub at: String,
    /// When it was asked, in milliseconds: what an answer names as the request it answers
    /// (SPEC-512 ANS-004).
    #[serde(default)]
    pub at_ms: i64,
    /// The sandbox-level problem it is about (`docker`), shared with the sandbox's other tasks
    /// that name it: one card shows them all (SPEC-494 DFJ-011).
    #[serde(default)]
    pub cause: Option<String>,
    /// With a `cause`: when the oldest open request of the sandbox's working tasks naming it was
    /// asked (RFC 3339), the same for all of them: one notification per such episode.
    #[serde(default)]
    pub episode: Option<String>,
    /// Whom besides the owner it waits for, GitHub logins (SPEC-512 ANS-001): they and the owner
    /// answer it; empty when it names nobody.
    #[serde(default)]
    pub who: Vec<String>,
    /// Whether it named people, still or before the owner withdrew them: then only its owner
    /// and `who` answer it (ANS-002).
    #[serde(default)]
    pub named: bool,
    /// Browser tab id when this request came from `browser_request_user_action` (SPEC-578).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_tab: Option<String>,
}

/// Actionable explanation of why a task is currently in Needs You (SPEC-578).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsYouNoticeView {
    /// Who/what raised this wait: `agent`, `browser`, `terminal`, or `system`.
    pub source: String,
    /// Stable wait-episode key for deduplication across repeated polls.
    pub episode: String,
    /// What happened, one bounded redacted line.
    pub happened: String,
    /// Optional problem or failure context line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Concrete next action for a person, one bounded redacted line.
    pub action: String,
    /// Relevant Yulius page URL where the person can act.
    pub link: String,
    /// Whether the person chooses rather than does.
    #[serde(default)]
    pub decision: bool,
    /// Browser tab id when this wait is a browser handoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_tab: Option<String>,
    /// When this wait episode began (RFC 3339).
    pub at: String,
    /// When this wait episode began (milliseconds).
    #[serde(default)]
    pub at_ms: i64,
}

/// One timestamped system or agent message recorded in a task's Needs You history (SPEC-578).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitHistoryEntry {
    /// Stable wait-episode key.
    pub episode: String,
    /// `agent`, `browser`, `terminal`, or `system`.
    pub source: String,
    /// What happened.
    pub happened: String,
    /// Optional problem or failure context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Concrete next action.
    pub action: String,
    /// Relevant Yulius link.
    pub link: String,
    /// Whether the person was asked to decide.
    #[serde(default)]
    pub decision: bool,
    /// When recorded (RFC 3339).
    pub at: String,
    /// When recorded (milliseconds).
    #[serde(default)]
    pub at_ms: i64,
    /// When resolved (RFC 3339), or absent while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

/// Why a task was flagged `needs_attention` (SPEC-472, ADR-0306).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionView {
    /// `terminal`: Claude Code asks in the task's terminal (a permission menu, a question);
    /// `yulius`: Yulius flagged the task itself.
    pub kind: String,
    /// What is wrong, one line: Yulius's reason, or Claude Code's own words for its question.
    #[serde(default)]
    pub problem: Option<String>,
    /// What the person does.
    #[serde(default)]
    pub action: Option<String>,
    /// Where they do it (a page of this Yulius).
    #[serde(default)]
    pub link: Option<String>,
}

/// A prompt held by the scope check (SPEC-001 TSK-013).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldPrompt {
    /// Hold id.
    pub id: String,
    /// `new_scope`, `drops_scope` or `redirect`.
    pub class: String,
    /// The prompt (at most 2,000 characters; the full text stays on the server).
    pub excerpt: String,
    /// Jev's probability of the class, in percent.
    pub percent: u32,
    /// Login of the person who typed it, when known.
    pub typed_by: Option<String>,
    /// When it was held.
    pub created_at: String,
}

fn default_mode() -> String {
    "prompt".into()
}

fn default_harness() -> String {
    "auto".into()
}

fn chosen() -> String {
    "chosen".into()
}

/// `POST /api/sandboxes/{sid}/tasks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTask {
    /// Initial prompt.
    pub prompt: String,
    /// Harness: `claude`, `codex` (SPEC-503) or `antigravity` (SPEC-505); `auto` (the default)
    /// draws uniformly across harnesses with non-exhausted subscriptions, unless `model` implies
    /// one (SPEC-520 RHM-001).
    #[serde(default = "default_harness")]
    pub harness: String,
    /// Workflow to seed steps/instructions; `auto` lets Jev choose one or none (TSK-015).
    #[serde(default)]
    pub workflow_id: Option<String>,
    /// Model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Effort override.
    #[serde(default)]
    pub effort: Option<String>,
    /// Title (derived from the prompt when absent).
    #[serde(default)]
    pub title: Option<String>,
    /// `goal` (default), `prompt` (TSK-010) or `auto`: Jev chooses (TSK-015).
    #[serde(default)]
    pub mode: Option<String>,
    /// Run after this task of the same sandbox (SPEC-001 TSK-012): the new task waits as
    /// `chained` until that task is Done.
    #[serde(default)]
    pub after_task_id: Option<String>,
    /// Start when this logical sentence over the sandbox's tasks is true (SPEC-017), for example
    /// `#12 done and (#13 done or #14 ended)`. Not together with `after_task_id`.
    #[serde(default)]
    pub start_rule: Option<String>,
    /// A supertask (SPEC-071): it waits for the project's running tasks, then runs alone while
    /// the project's other tasks wait for it. Not together with `after_task_id` or `start_rule`.
    #[serde(default)]
    pub supertask: bool,
    /// Prepared (SPEC-105): the task waits as `prepared` until a person starts it ("when I
    /// decide"). Not together with `after_task_id`, `start_rule` or `supertask`.
    #[serde(default)]
    pub prepared: bool,
    /// Prepared, and it starts by itself at this time (RFC 3339; at most 60 s past, at most 366
    /// days ahead). Implies `prepared`.
    #[serde(default)]
    pub start_at: Option<String>,
}

/// `PUT /api/tasks/{tid}/schedule` (SPEC-105): when a prepared task starts by itself.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScheduleTask {
    /// The new time (RFC 3339), or null: it waits for a person.
    #[serde(default)]
    pub start_at: Option<String>,
}

/// `PUT /api/tasks/{tid}/start` (SPEC-126): how a task that waits to start (`chained` or
/// `prepared`) starts instead, as New task's start choice; none of the fields: Start now.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChangeStart {
    /// Run after this task of the same sandbox (TSK-012).
    #[serde(default)]
    pub after_task_id: Option<String>,
    /// Start when this rule over the sandbox's tasks is true (SPEC-017). Not together with
    /// `after_task_id`.
    #[serde(default)]
    pub start_rule: Option<String>,
    /// Start now as a supertask (SPEC-071). Not together with `after_task_id` or `start_rule`.
    #[serde(default)]
    pub supertask: bool,
    /// Wait until a person starts it (SPEC-105). Not together with the other choices.
    #[serde(default)]
    pub prepared: bool,
    /// Prepared, and it starts by itself at this time (RFC 3339, PRP-001's window).
    #[serde(default)]
    pub start_at: Option<String>,
}

/// `POST /api/tasks/{tid}/continue` and `/switch`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContinueTask {
    /// Target account (another Claude account); the best one when absent.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Optional prompt delivered when waking a parked task via `/continue` (SPEC-553 PRK-102);
    /// bounded to 64 KiB and ignored for non-parked tasks.
    #[serde(default)]
    pub prompt: Option<String>,
}

/// One process run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptView {
    /// Sequence within the task.
    pub seq: u32,
    /// Harness.
    pub harness: String,
    /// Account label.
    pub account_label: Option<String>,
    /// Email of the account's Claude account (instance administrators only, UI-022; absent while
    /// unknown, SPEC-244).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    /// `new` or `resume`.
    pub launch_kind: String,
    /// Attempt state wire name.
    pub state: String,
    /// Start time.
    pub started_at: String,
    /// End time.
    pub ended_at: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Exit reason.
    pub exit_reason: Option<String>,
    /// The model that answered in this run, as its transcript names it (absent while unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The Claude Code version this run's transcript names (SPEC-461 CCM-011; absent while
    /// unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
}

/// A task step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepView {
    /// Step id.
    pub id: String,
    /// Position.
    pub ordinal: u32,
    /// Title.
    pub title: String,
    /// `pending`, `in_progress`, `done`, `skipped` or `blocked`.
    pub status: String,
    /// Note.
    pub note: Option<String>,
    /// Last actor.
    pub updated_by: Option<String>,
    /// Last change time.
    pub updated_at: Option<String>,
    /// Declared effort, 1-3 (STP-006).
    #[serde(default = "one")]
    pub effort: u8,
}

fn one() -> u8 {
    1
}

/// What the MCP tools `steps_get`, `steps_set` and `step_update` answer: the plan, its progress
/// and the current nudges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    /// The plan's progress.
    pub progress: PlanProgress,
    /// Its steps, in order.
    pub steps: Vec<PlanStep>,
    /// What the agent is reminded of.
    #[serde(default)]
    pub nudges: Vec<String>,
    /// The step `step_update` changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<StepChange>,
}

/// A plan's progress as the MCP tools say it, in a plan record and a task brief's `steps`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanProgress {
    /// Finished steps (done or skipped).
    pub done: u32,
    /// All steps.
    pub total: u32,
    /// Finished share of the plan's effort, 0-100.
    #[serde(default)]
    pub percent: u8,
    /// Current step title.
    pub current: Option<String>,
    /// Steps the plan had when its first step was finished, when it has grown since.
    pub first_total: Option<u32>,
}

impl From<&StepProgress> for PlanProgress {
    fn from(p: &StepProgress) -> Self {
        Self {
            done: p.done,
            total: p.total,
            percent: p.percent,
            current: p.current.clone(),
            first_total: p.first_total,
        }
    }
}

/// One step of a [`PlanRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step id.
    pub id: String,
    /// Position.
    pub ordinal: u32,
    /// Title.
    pub title: String,
    /// `pending`, `in_progress`, `done`, `skipped` or `blocked`.
    pub status: String,
    /// Declared effort, 1-3.
    #[serde(default)]
    pub effort: u8,
    /// Note.
    pub note: Option<String>,
    /// When it was first started (RFC 3339).
    pub started_at: Option<String>,
    /// When it was finished (RFC 3339).
    pub finished_at: Option<String>,
}

/// The step a `step_update` changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepChange {
    /// Its position.
    pub ordinal: u32,
    /// Its title.
    pub title: String,
    /// Its new status.
    pub status: String,
}

/// Token and duration metrics; `None` means unknown, never zero.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Wall-clock milliseconds across runs.
    pub wall_ms: u64,
    /// Active (working) milliseconds.
    pub active_ms: Option<u64>,
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Cache-read tokens.
    pub cache_read_tokens: Option<u64>,
    /// Cache-write tokens.
    pub cache_write_tokens: Option<u64>,
    /// Models used.
    pub models: Vec<String>,
    /// Account/provider switches.
    pub switches: u32,
    /// Completed turns.
    pub turns: Option<u32>,
}

/// Feedback entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackView {
    /// Author login.
    pub login: String,
    /// `correct` or `incorrect`.
    pub verdict: String,
    /// Comment.
    pub comment: String,
    /// Time.
    pub at: String,
}

/// A recorded switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchView {
    /// Time.
    pub at: String,
    /// Reason code.
    pub reason: String,
    /// From (harness/account label).
    pub from: String,
    /// To (harness/account label).
    pub to: String,
}

/// `GET /api/tasks/{tid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDetail {
    /// Summary fields.
    #[serde(flatten)]
    pub summary: TaskSummary,
    /// Registered spec path.
    pub spec_path: Option<String>,
    /// Worktree path on the instance.
    pub worktree: Option<String>,
    /// Runs.
    pub attempts: Vec<AttemptView>,
    /// Steps.
    pub steps_list: Vec<StepView>,
    /// Metrics.
    pub metrics: Metrics,
    /// Feedback.
    pub feedback: Vec<FeedbackView>,
    /// Delivery state.
    pub delivery: Delivery,
    /// Switch history.
    pub switches: Vec<SwitchView>,
    /// The whole request the agent gets when the task starts, queued messages included
    /// (SPEC-052 QMS-005).
    pub request: String,
    /// Whether the task ever had a run or a provider session; until then it takes queued
    /// messages while it waits (SPEC-052 QMS-001).
    pub started: bool,
    /// The model the task asks for (`--model`): its own, or Yulius's default when it names none
    /// (SPEC-461 CCM-012). What answered is `metrics.models`.
    #[serde(default)]
    pub model: String,
    /// Whether that is Yulius's default, the task naming none.
    #[serde(default)]
    pub model_default: bool,
    /// The model its live run was launched with when that is not `model`: the fallback, while
    /// no subscription could run `model` at that launch (SPEC-482). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    /// The effort it asks for (`--effort`): its own, else the default; absent for a model that
    /// takes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Whether the task inherits its effort when the next process starts.
    #[serde(default)]
    pub effort_default: bool,
    /// The Claude Code version of its latest session that names one (absent while unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    /// The newer Claude Code version its running session moves to at the end of its turn
    /// (SPEC-461 CCM-013); absent when it runs the version new runs get, or does not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_moves_to: Option<String>,
    /// Running tasks are no longer moved to that version (a session could not continue on it):
    /// this one gets it at its next start, not when its turn ends (SPEC-468 CCF-003).
    #[serde(default)]
    pub cli_moves_stopped: bool,
    /// Timestamped system and agent Needs You messages recorded for this task (SPEC-578),
    /// newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wait_history: Vec<WaitHistoryEntry>,
}

/// `POST /api/tasks/{tid}/queued-messages` (SPEC-052 QMS-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueMessage {
    /// The message, added to the request as it is (trimmed).
    pub text: String,
}

/// `POST /api/tasks/{tid}/needs-you/answer` (SPEC-516 NYA-001): the answer to what the task's
/// agent asked a person, from anyone who may work in its project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerNeedsYou {
    /// The answer, typed into the task's session as one labelled message.
    pub text: String,
    /// The request it answers, as its `at_ms` reads (SPEC-512 ANS-004): one replaced since is
    /// not answered by it.
    #[serde(default)]
    pub asked_at_ms: Option<i64>,
}

/// `POST /api/tasks/{tid}/needs-you/withdraw` (SPEC-512 ANS-005): the owner withdraws a person
/// the open request names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawNeedsYou {
    /// The GitHub login to withdraw.
    pub login: String,
}

/// Answer to a queued message: the request as the agent will get it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// The whole request, the new message last.
    pub request: String,
}

/// `GET /api/tasks/{tid}/prompt`: the prompt a task started with (SPEC-043).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPrompt {
    /// The text, without its attachment lines (as the session got it).
    pub text: String,
    /// The attached files, in order; `GET /api/tasks/{tid}/prompt/attachments/{n}` serves one.
    pub attachments: Vec<PromptAttachment>,
    /// The prompt in order: its text and each attached file where it sits (SPEC-124).
    pub parts: Vec<PromptPart>,
}

/// One piece of a task's prompt, in order (SPEC-124): `{"text"}` or `{"attachment": n}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PromptPart {
    /// Text as written.
    Text {
        /// The text.
        text: String,
    },
    /// Attached file `n` of [`TaskPrompt::attachments`], at this place.
    Attachment {
        /// Its index in the attachments.
        attachment: usize,
    },
}

/// A file attached to a task's prompt (SPEC-043).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAttachment {
    /// File name.
    pub name: String,
    /// Whether Claude Code takes it as an image (it was pasted as one).
    pub image: bool,
    /// Whether it can be served: a regular file in the sandbox's attachments folder (drafts no
    /// request names are removed after a week; a path mentioned by hand is never served).
    pub available: bool,
}

/// `POST /api/tasks/{tid}/feedback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackRequest {
    /// `correct` or `incorrect`.
    pub verdict: String,
    /// Optional comment.
    #[serde(default)]
    pub comment: String,
}
