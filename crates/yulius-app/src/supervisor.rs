//! Session supervisor: owns the lifecycle of every agent run (SPEC-002 §6.1–§6.3, §7.1).
//!
//! Invariants:
//! - at most one live attempt per task (store index + CAS transitions);
//! - capacity is released only after the run's exit was observed or the run was declared lost;
//! - every attempt records exactly one terminal outcome;
//! - a record that says "running" is never respawned automatically after a restart.
//!
//! This module holds the per-task state (`TaskRuntime`, `SupervisorState`); the lifecycle lives
//! in its children: `start` (admission and the queue), `launch`, `typing` (the opening message
//! and the goal), `watch`, `input`, `switch`, `placement` (subscriptions, limits, rotation),
//! `stop`, `park`, `end` (a run's end and the restart's reconcile), and the `codex` and
//! `antigravity` specifics.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, AttemptId, TaskId};
use yulius_domain::task::Activity;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::ModelFamily;

use crate::attention::AttentionRecord;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::Hub;
use crate::ports::harness::UsageSnapshot;
use crate::ports::isolation::LimitCounters;
use crate::ports::session::RunWriter;

mod antigravity;
mod codex;
mod end;
mod input;
mod launch;
mod park;
mod placement;
mod start;
mod stop;
mod switch;
mod typing;
mod watch;

pub(crate) use antigravity::read_antigravity_token_in;

pub(crate) use codex::{CODEX_PICKER, CODEX_UNREADABLE};
use codex::{CodexInput, CodexPaste};
pub(crate) use end::metrics_of;
pub use launch::branch_for;
pub(crate) use launch::git_credential_env;
pub(crate) use placement::{
    Placed, model_change_notice, refused_choice, switched_back_notice, switched_model,
};
pub use start::RunStart;
pub(crate) use start::{CONTINUE_TASK_PROMPT, SwitchCondition};
pub use switch::SWITCH_COOLDOWN_MS;
pub use typing::{GOAL_READY_TIMEOUT, TRUST_ANSWER_TIMEOUT};
pub(crate) use typing::{
    SCREEN_WITHHELD, clear_person_typed, launch_goal_bound_ms, mark_person_typed,
    mark_session_started, native_goal_pending, opening_entered, opening_sent, person_typed_marked,
    read_final_screen, read_saved_screen, save_screen, trust_prompt_shown, write_private_file,
};
pub use watch::{REATTACH_ATTEMPTS, WATCH_INTERVAL_MS};

/// Boxed `Send` future (breaks recursive async cycles between start, pump and finalize).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Default PTY size before the first viewer reports its size.
pub const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// How many times a stop makes its status change: each lost one means another writer changed the
/// status, and the status machine has no cycle a stop can keep losing to (SPEC-250 STW-001).
const STOP_ROUNDS: usize = 4;
/// Why Yulius is ending a run.
enum EndIntent {
    /// User or system stop.
    Cancel,
    /// A finished task is closed (archived when Done): it ends as a clean exit, `closed`.
    Close,
    /// An idle task's session is parked (SPEC-512 PRK-003): it ends as a close does and the task
    /// becomes `parked`, keeping its activity.
    Park,
    /// Account/provider switch; the sender is told when the old run has exited, or why the switch
    /// was overtaken.
    Switch(oneshot::Sender<SwitchEnd>),
    /// A switch gave the run up when it did not end in time, and terminates its host (SPEC-483
    /// SWT-002): whoever records the run's end, the task rests as after that timeout.
    SwitchTimeout,
}

/// A task end the reader can hand to the competing loss recorder (SPEC-563).
enum TakenRequest {
    Close,
    Cancel,
    Park,
}

/// How the wait of a switch for its old run ended (SPEC-250 STW-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwitchEnd {
    /// The old run exited: the switch goes on.
    Ended,
    /// A stop took over the old run's end.
    Stopped,
    /// A close took over the old run's end.
    Closed,
    /// A park took over the old run's end (SPEC-512 PRK-003).
    Parked,
    /// The old run was lost.
    Lost,
    /// The old run did not end in time and the switch ended it (SPEC-483 SWT-002).
    TimedOut,
}

/// Puts `next` in a task's end intent slot. A switch whose intent it replaces is told `why` it was
/// overtaken, so it says so whatever the run's end has recorded yet (SPEC-250 STW-006).
fn replace_intent(slot: &mut Option<EndIntent>, next: Option<EndIntent>, why: SwitchEnd) {
    if let Some(EndIntent::Switch(waiting)) = std::mem::replace(slot, next) {
        let _ = waiting.send(why);
    }
}

#[derive(Clone, Copy)]
enum ObservationSource {
    Replay,
    Live,
}

impl ObservationSource {
    /// A live undated record uses arrival time. A replay cannot use the server restart time.
    fn review_time(self, at: Option<Timestamp>, now: Timestamp) -> Option<Timestamp> {
        match self {
            Self::Replay => at,
            Self::Live => Some(at.unwrap_or(now)),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SessionInput<'a> {
    Viewer(&'a [u8]),
    Paste(&'a [u8]),
    ContinuePaste(&'a [u8]),
    Submit,
}

impl<'a> SessionInput<'a> {
    fn bytes(self) -> &'a [u8] {
        match self {
            Self::Viewer(bytes) | Self::Paste(bytes) | Self::ContinuePaste(bytes) => bytes,
            Self::Submit => b"\r",
        }
    }
}

struct ActiveRun {
    attempt_id: AttemptId,
    codex_input: Option<CodexInput>,
    writer: Box<dyn RunWriter>,
}

/// The durable draft mark and the prompt submission whose hook may be retried.
#[derive(Default)]
pub(crate) struct DraftMark {
    /// Recovery observed the durable first-key time but cannot date the latest key.
    pub(crate) recovered_at: Option<i64>,
    pub(crate) written: bool,
    /// Wall time, viewer key sequence and owner attribution at the first submit request.
    pub(crate) submit: (i64, u64, bool),
}

/// Bounded run-local evidence for directly issued instructions and submit retries.
#[derive(Default)]
struct PromptReceipts {
    issued: VecDeque<IssuedPrompt>,
    invocations: VecDeque<SubmitInvocation>,
}

struct SubmitInvocation {
    id: u128,
    held_owner: Option<(i64, u64)>,
}

struct IssuedPrompt {
    digest: [u8; 32],
    pending: u16,
}

/// Runtime state of one task with a hub that outlives individual runs.
pub struct TaskRuntime {
    /// Terminal fan-out.
    pub hub: Hub,
    run: tokio::sync::Mutex<Option<ActiveRun>>,
    /// The run whose screen `hub` shows: set when its attach resets the hub, kept after it ends
    /// until the next run's attach (SPEC-301 LRS-001).
    screen_of: Mutex<Option<AttemptId>>,
    intent: Mutex<Option<EndIntent>>,
    usage: Arc<Mutex<UsageSnapshot>>,
    /// The run whose transcript `usage` has read to its end at least once: only then is the
    /// snapshot that run's figures (SPEC-143 RDM-001). A run that ends before (it ended while
    /// Yulius was down, or right after a reattach) is counted from its transcript instead.
    usage_read: Mutex<Option<AttemptId>>,
    watcher_alive: Arc<AtomicBool>,
    /// The latest transcript watcher started: an older one ends even when the next run starts
    /// before it saw its own run end (it would read the new run's lines as its own).
    watcher_generation: AtomicU64,
    /// Last keyboard input from a viewer (ms).
    pub last_input_ms: AtomicI64,
    /// Proactive switch requested at the next turn boundary.
    pub proactive_switch: AtomicBool,
    /// The subscription past its rebalance threshold this idle task stays on because no other
    /// has room, and when that episode was told (SPEC-629 RBL-007): told once per episode.
    pub(crate) rebalance_no_room: Mutex<Option<(AccountId, Timestamp)>>,
    /// A review round's reviewer a usage limit cut off, not yet resumed: a relaunch tells the
    /// session to resume it (SPEC-493 RLM-002).
    pub(crate) cut_off_reviewer: tokio::sync::Mutex<Option<crate::review::CutOffReviewer>>,
    /// The session moves for its cut-off reviewer at its next turn's end (RLM-002).
    pub(crate) reviewer_move: AtomicBool,
    /// The current run called a Yulius hook: its Stop hook is the turn boundary that decides
    /// switches and pauses, not the transcript (SPEC-002 §7.5).
    pub(crate) hooks_seen: AtomicBool,
    /// The current run's last turn ended at a usage limit: no prompt or agent work since
    /// (SPEC-072 LRS-002). A person's switch then resumes past the limit even without a `limit`
    /// pause (an automatic switch after the limit was refused).
    pub(crate) at_limit: AtomicBool,
    /// The dynamic workflows the session launched whose end was not read (SPEC-486 WFL-003).
    pub(crate) workflows: Mutex<crate::dynamic_workflow::Unfinished>,
    /// The goal pause Claude Code wrote at that limit, held for the limit path, or that path's
    /// word that the session stays there (SPEC-464 UCL-007).
    pub(crate) limit_goal_pause: Mutex<crate::goal_retry::LimitGoalPause>,
    /// When Claude Code recorded that limit (ms, 0 = unknown): what was audited at or after it
    /// is this limit's (SPEC-464 UCL-007).
    pub(crate) limit_at_ms: AtomicI64,
    /// The limits nothing says the end of that left the task with nowhere to go: how often in a
    /// row, and the subscription the pool no longer starts it on by itself (SPEC-464 UCL-010).
    pub(crate) blind_limits: Mutex<crate::moves::BlindLimits>,
    /// The model family a limit refused after the session's person switched to it (`/model`),
    /// recorded with the reporting attempt before either a pool move or an ownership restart
    /// can launch its replacement (SPEC-493 MDL-002).
    switched_limit: Mutex<Option<(AttemptId, ModelFamily)>>,
    /// When the current turn started (ms, 0 = no turn in progress). Active agent time comes
    /// from the transcript (SPEC-087 MET-001), not from this.
    turn_started_ms: AtomicI64,
    pub(crate) candidate_observation: Mutex<crate::pr_watch::CandidateObservation>,
    /// The head Yulius last tried to open a pull request for by itself (#35), so a failed try is
    /// not repeated every turn.
    pub(crate) auto_pr_head: Mutex<Option<String>>,
    /// The head and merged pull request Yulius last found to add nothing to that pull request
    /// (SPEC-425 FUP-002), so a merged task's later Stops ask GitHub only for a new head.
    pub(crate) merged_covers: Mutex<Option<(String, u64)>>,
    /// When a person last typed into the task's terminal (ms, 0 = never): a prompt submitted
    /// soon after is theirs (TSK-013).
    pub(crate) typed_at_ms: AtomicI64,
    /// Who typed last.
    pub(crate) typed_by: Mutex<Option<yulius_domain::ids::UserId>>,
    /// When Yulius last typed a message into the session for it to submit (ms, 0 = never): a
    /// person's message, a pull request's news (`type_into_session`). It counts as a draft
    /// until its submit, which Claude Code makes at Yulius's own Enter. The wait in Claude
    /// Code's queue while a turn runs comes after that: [`Self::queue`] knows it (SPEC-468
    /// CCF-011, SPEC-471).
    pub(crate) message_typed_ms: AtomicI64,
    /// When the session's prompt box was last known empty (ms, 0 = not known): its run's
    /// launch, or the last prompt submitted from it. What was typed after that may still sit
    /// there (SPEC-468 CCF-010, CCF-011).
    pub(crate) box_emptied_ms: AtomicI64,
    /// The keystroke ([`Self::typed_at_ms`]) whose possible draft the session's terminal said a
    /// message of Yulius's own waits for (SPEC-480 DRF-006): one line per keystroke.
    pub(crate) draft_told_ms: AtomicI64,
    /// The run's folder holds the mark of a person's possible draft (`person-typed`, SPEC-480
    /// DRF-009). The guard orders a key's timestamp and file write with the submit hook's
    /// check and file removal (SPEC-537).
    pub(crate) draft_mark: tokio::sync::Mutex<DraftMark>,
    pub(crate) viewer_prompt: Mutex<crate::terminal::ViewerPrompt>,
    /// Fingerprints of recent instructions Yulius emitted to this session.
    prompt_receipts: Mutex<PromptReceipts>,
    /// Sequence of the last viewer input in this run. Before the first, only the durable
    /// mark identifies a possible draft, and a prompt may predate it (DRF-009).
    pub(crate) typed_seq: AtomicU64,
    /// The task's owner had a message typed behind their possible draft and Yulius is about to
    /// submit both (SPEC-487 TCT-006): the next prompt that goes on from the box carried that
    /// draft to the agent, also one this server knows from its mark alone. The mark is taken
    /// by whichever prompt from the box goes on next; it is not checked to be that message.
    /// It matters only for a draft known from its mark alone (no key seen by this server),
    /// and there only if another prompt, held since before the restart, goes on while Claude
    /// Code did not take Yulius's Enter as a submit: the draft would then read as ended
    /// while its text is still in the box.
    pub(crate) sent_past_draft: AtomicBool,
    /// The viewer input sequence when the submit that emptied the box arrived. Keys after that submit
    /// win ties in the millisecond clock, including while a held prompt is answered.
    pub(crate) emptied_seq: AtomicU64,
    /// The run's last prompt hook was answered with a hold: the next one is that prompt sent
    /// again by the hook client, not another submit. A hold the CLI gave up (its client holds
    /// for days) leaves this set until the run's next prompt is answered, which then counts as
    /// the held one going on: the session reads as holding a draft until the prompt after
    /// that is submitted, never as holding none. A new run holds none
    /// ([`Self::run_starts`]).
    pub(crate) prompt_held: AtomicBool,
    /// A reattach owns the old attempt's recovery until any required hand-over finishes.
    /// A limit move from its transcript waits here before deciding which attempt to move.
    pub(crate) reattach_move: Arc<tokio::sync::Mutex<()>>,
    /// Whose personal MCP servers the current run was started with (SPEC-332 MSC-009): a live
    /// run keeps its MCP configuration, so a hand-over restarts it, and only that person messages
    /// it meanwhile.
    pub(crate) personal_mcp_of: Mutex<Option<yulius_domain::ids::UserId>>,
    /// Claude Code recorded the session goal as achieved and no prompt came since (TSK-014).
    pub(crate) goal_achieved: AtomicBool,
    /// Claude Code recorded something about the goal since the `/goal` was typed: it was
    /// submitted, not left in the prompt box (a prompt alone proves nothing: it may be another).
    pub(crate) goal_submitted: AtomicBool,
    /// The latest turn end Yulius knows of (ms): when a Stop that reached Yulius was sent, or
    /// when Claude Code recorded a goal verdict Yulius applied (clamped to now). A prompt sent
    /// before it belongs to a turn that is over (SPEC-346 LPH-002).
    pub(crate) turn_end_ms: AtomicI64,
    /// Orders a prompt's turn start with a later turn end: a prompt holds it from its
    /// [`Self::turn_end_ms`] check until it set Working, and a Stop or verdict takes it to move
    /// `turn_end_ms`, so their settling never lands before a prompt that passed the check. The
    /// same for a prompt read from the transcript and its [`Self::last_stop_ms`] check, which a
    /// clean Stop moves under it (SPEC-453 TPO-001).
    pub(crate) turn_order: tokio::sync::Mutex<()>,
    /// Latest native Antigravity turn hook's send time. Serialize the full Stop decision
    /// with invocations, independently of transcript reads that need `turn_order`.
    pub(crate) antigravity_hook_ms: tokio::sync::Mutex<i64>,
    /// Claude Code's SessionStart hook arrived for the current attempt: its prompt box reads
    /// input (typing before it is lost or left unsubmitted, F 2026-10-12).
    pub(crate) session_started: AtomicBool,
    /// The redaction state of the terminal `hub` shows (the attached run's): while it withholds
    /// output the screen is not what the program drew (ADR-0317).
    scrub: Mutex<crate::vault::TerminalScrub>,
    /// The task's person was told that nothing is typed into this screen (once per attach).
    screen_flagged: AtomicBool,
    /// The goal-mode session opened with a first message (a resumed session's continuation)
    /// whose turn has not ended yet: its goal is typed after that turn's Stop hook.
    pub opening_turn: AtomicBool,
    /// Yulius types the run's first message (SPEC-030): from the spawn until its Enter, a
    /// person's keys are not written into it (SPEC-117 ONE-006).
    pub(crate) typing_opening: AtomicBool,
    /// A person was told their keys wait for the first message (once per first message).
    pub(crate) typing_opening_said: AtomicBool,
    /// Yulius types a Superagent's message into the run (SPEC-230): one at a time.
    pub(crate) typing_message: AtomicBool,
    /// When the current run was spawned (ms; a reattached run's attempt start): without a
    /// SessionStart, its prompt box is taken to read input half
    /// [`crate::context::AppConfig::goal_ready_ms`] later, as the goal typist does (SPEC-230).
    pub(crate) spawned_ms: AtomicI64,
    /// A merge `delivery_merge` armed while the checks still ran (SPEC-011), Yulius's decisions
    /// of it in progress and how the last one ended (SPEC-028 MSR-003), under one lock so
    /// `yulius wait` reads them together (SPEC-064 WAI-003).
    pub(crate) armed: Mutex<crate::agent_wait::ArmedSlot>,
    /// Orders durable singleton decisions with an operator or agent changing its arm.
    pub(crate) arm_writer: Arc<tokio::sync::RwLock<()>>,
    /// Why Yulius could not push or deliver the agent's head, for `yulius wait`: the agent learns
    /// it instead of waiting for a run that never comes (SPEC-050 DLV-201, DLV-204).
    pub(crate) push_problem: Mutex<Option<crate::delivery::PushNote>>,
    /// The last local head already checked for open-PR CI repair publication by `yulius wait` or
    /// `pr_watch`, so periodic passes and long-poll reconnects do not repeat GitHub calls for the
    /// same head while the server stays up (SPEC-553 REP-004).
    pub(crate) repair_checked_head: Mutex<Option<String>>,
    /// When the last clean Stop ended a turn (ms), so a prompt of that turn read late from the
    /// transcript does not reopen it.
    pub(crate) last_stop_ms: AtomicI64,
    /// Background shells the agent left running, as last probed (-1 = not known).
    pub(crate) background_shells: AtomicI64,
    /// The sandbox's limit counters when this server attached to the run (SPEC-268 PID-005).
    limits_at_start: Mutex<Option<(AttemptId, LimitCounters)>>,
    /// Background shells of the idle agent that only run `yulius wait`, as last probed (-1 = not
    /// known): it waits for Yulius and goes on by itself (SPEC-085 STT-002).
    pub(crate) background_waits: AtomicI64,
    /// Open `POST /internal/agent/wait` (`yulius wait`) requests currently served for this task
    /// (SPEC-580 AWM-001): complements the process probe before the first probe runs and in tests.
    pub(crate) active_waits: AtomicU32,
    /// Since when (ms) the idle agent's background shells have been running (0 = none).
    pub(crate) shells_since_ms: AtomicI64,
    /// The run's strays as last probed, by their first process `(pid, started)`, to when (ms) each
    /// was first seen (SPEC-353 LAP-004).
    pub(crate) strays_seen: Mutex<HashMap<(u32, u64), i64>>,
    /// Background agents Claude Code still waited for when the last turn ended (its
    /// `turn_duration` record, STA-001): they wake the session by themselves, so the idle task
    /// is not done. Counted with the shells.
    pub(crate) background_agents: AtomicU32,
    /// When (ms) a notice that asks a person to act, or a request Yulius opened because the
    /// session did not go on by itself (SPEC-469, SPEC-464 UCL-010), flagged the task
    /// `needs_attention` (0 = no such flag): the agent's work after it shows the person
    /// answered, or the session went on (STA-003).
    pub(crate) notice_attention_ms: AtomicI64,
    /// The state time (ms) of the `needs_attention` flag an agent's secret request raised (0 =
    /// none): a person's answer ends only that flag, while the task still reads it (SPEC-213).
    pub(crate) secret_wait_at: AtomicI64,
    /// Held while a `needs_attention` flag is raised, or a secret request's flag is marked or
    /// ended, so that none of them reads a state another is changing (SPEC-213).
    pub(crate) attention_lock: tokio::sync::Mutex<()>,
    /// Held by each writer of the task's steps (the spec sync, `steps_set`, `step_update`, a
    /// person skipping steps) from its read to its write, so none writes back a status another
    /// just changed (SPEC-341 LSS-002). Spec and native goal updates share this lock with
    /// the final parking check. Public for the integration tests that hold it.
    pub steps_lock: tokio::sync::Mutex<()>,
    /// When (ms) the record that closes a turn was last seen: the background agents it counts
    /// are known from then on (STA-001), which the chain and Done decisions wait for.
    pub(crate) turn_record_ms: AtomicI64,
    /// The task was made Working again by the goal verdict or by work after a stop, not by a
    /// prompt: when the turn closes without any Stop hook (an error ended it) it is idle again.
    pub(crate) resumed_turn: AtomicBool,
    /// Claude Code armed a goal retry after the turn an error ended (SPEC-333 AER-001): the turn
    /// that closes stays Working until the retry's prompt, or a person's, starts the next one.
    pub(crate) goal_retry_armed: AtomicBool,
    /// Yulius asked a person because the session does not go on by itself (a goal Claude Code
    /// stopped retrying, or a usage limit nothing says the end of, SPEC-464 UCL-010), and the
    /// session's agent has not worked since: its next work ends that request (SPEC-469 GPW-001).
    /// It outlives the run: a session Yulius restarts answers it too.
    pub(crate) goal_pause_asked: AtomicBool,
    /// When Yulius last answered a Stop hook of this run, whatever it decided (ms, 0 = never).
    /// A turn that began later and closes had no Stop Yulius answered: its hook failed open
    /// (SPEC-290 GDW-001).
    pub(crate) stop_answered_ms: AtomicI64,
    /// When the last turn began (ms): its prompt's transcript time, or when work resumed it.
    pub(crate) turn_began_ms: AtomicI64,
    /// Stop hooks of this run Yulius is answering now: the record that closes their turn leaves
    /// the turn to them (SPEC-290 GDW-001; a client that gave up still has it answered, ADR-0115).
    pub(crate) stops_in_flight: AtomicU32,
    /// When the last record that closes a turn was written (its own time, ms; 0 = none or no
    /// time): a failed Stop settles its turn only after that turn's record (SPEC-290 GDW-001).
    pub(crate) turn_record_at_ms: AtomicI64,
    /// When the last record that closes the session's turn was written (ms; 0 = none), from the
    /// replayed transcript and live: Yulius wakes a task whose turn closed while it was down
    /// (SPEC-358 MSB-007).
    pub(crate) turn_closed_ms: AtomicI64,
    /// When the session's last prompt or answer was recorded (ms; 0 = none), replayed and live:
    /// a Stop answered after it was the last turn's (MSB-007).
    pub(crate) turn_moved_ms: AtomicI64,
    /// The server start (ms) after whose outage this task was woken, once (SPEC-358 MSB-007).
    pub(crate) woken_after_outage_ms: AtomicI64,
    /// When the session's MCP client last reached `POST /mcp` on this server (ms, 0 = never)
    /// (SPEC-432 MCR-002).
    pub(crate) mcp_seen_ms: AtomicI64,
    /// When its bridge last pinged (ms, 0 = never): it pings every minute.
    pub(crate) mcp_pinged_ms: AtomicI64,
    /// The `/mcp reconnect yulius` typed since its Yulius MCP was last heard (MCR-003).
    pub(crate) mcp_reconnects: Mutex<crate::mcp_watch::Reconnects>,
    /// Why its pull request is stalled, while it is (SPEC-444 PRN-009): the task list shows it.
    pub(crate) pr_stalled: Mutex<Option<String>>,
    /// News the pull request watch typed into the session whose turn has not begun yet: until
    /// then the task goes on by itself (SPEC-487 CHN-002).
    pub(crate) pr_news: Mutex<Option<crate::pr_watch::TypedNews>>,
    /// The last head and integration commit pair whose conflict was checked, and whether they
    /// conflict (PRN-004).
    pub(crate) pr_conflict: Mutex<Option<(String, bool)>>,
    /// The merge notice of its pull request, read from GitHub once until typed (PRN-005).
    pub(crate) pr_merged_note: Mutex<Option<(u64, String)>>,
    /// Background work the transcript says the session started and no notification ended yet:
    /// Monitor streams and background agents by Claude Code's task id (SPEC-079 BGW-002).
    pub(crate) launches: Mutex<std::collections::BTreeMap<String, crate::finish::Launch>>,
    /// The working stretch this idle task's lack of background work last ended: a turn the task
    /// goes on with by itself from that same state continues it (SPEC-309 WSW-001).
    pub(crate) ended_stretch: Mutex<Option<crate::tasks::EndedStretch>>,
    /// Claude Code recorded a goal that is active and not achieved (set, or "not met"): its
    /// evaluator gives a verdict after each Stop (SPEC-079 BGW-005).
    pub(crate) goal_open: AtomicBool,
    /// Claude Code last recorded the goal as set, not met or failed: an idle agent that stopped
    /// has work left, so it is the person's turn (SPEC-429 ANS-002).
    pub(crate) goal_unfinished: AtomicBool,
    /// The clean Stop (ms) whose turn waits for the goal verdict before it reads idle (0 = none).
    /// Whoever takes it settles that turn (SPEC-079 BGW-005).
    pub(crate) verdict_stop_ms: crate::finish::VerdictSlot,
    /// What waits in Claude Code's queue, as the run's `queue-operation` records say: a prompt
    /// entered while a turn runs, or a notification of its own, which the process holds in
    /// memory alone, so a restart loses it (SPEC-471 CCQ-001). A run starts with none.
    pub(crate) queue: Mutex<yulius_domain::agent_cli::Queue>,
    /// When a prompt from the session's prompt box last went on to Claude Code (ms, 0 = none in
    /// this run): where it went (a turn of its own, or the queue) is written a moment later
    /// (SPEC-471 CCQ-004).
    pub(crate) prompt_passed_ms: AtomicI64,
    /// How many prompts from the prompt box went on so far (it only grows): a turn's end that
    /// looks at the session reads it twice and so sees that one went on meanwhile (SPEC-471
    /// CCQ-004).
    pub(crate) prompts_passed: AtomicU64,
    /// The run was reattached after a restart of the server and its lines were not read again
    /// yet: what waits in Claude Code's queue is not known (SPEC-471 CCQ-001).
    pub(crate) queue_unread: AtomicBool,
    /// Polls of the transcript watcher so far: the Stop hook waits for one more before it decides
    /// a switch, so the lines written before the Stop are read (SPEC-079 BGW-004).
    pub(crate) watch_polls: AtomicU64,
    /// The run was reattached after a restart: the transcript it already wrote is history.
    pub(crate) replay_transcript: AtomicBool,
    /// Why Claude Code said the current session ends (`SessionEnd` hook), until the run exits.
    pub(crate) session_end: Mutex<Option<String>>,
    /// When a held hook of the current run last asked again (ms, 0 = not since this server
    /// started): a hold nobody polls any more was given up by the CLI (ADR-17).
    pub(crate) hold_polled_ms: AtomicI64,
    /// How a person's pause of the current run stands (SPEC-191 PER-001, `pause::PersonFreeze`).
    pub(crate) person_freeze: std::sync::atomic::AtomicU8,
    /// When a hook of the current run was last held for a person's pause (ms, 0 = not held).
    pub(crate) person_held_ms: AtomicI64,
    /// Turn-end step nudges already given to the agent (`Nudge::key`), so each is given once.
    pub(crate) nudged: Mutex<std::collections::BTreeSet<String>>,
    /// The terminal already said that this task acts on GitHub as the App, not its owner
    /// (F 2026-10-11), so it is said once.
    pub(crate) acting_as_app_said: AtomicBool,
    /// When a switch of this task was last refused before it started (ms, 0 = never): an
    /// automatic switch waits [`SWITCH_COOLDOWN_MS`] before it tries again.
    pub(crate) switch_refused_ms: AtomicI64,
    /// When a restart of this task's session on a newer Claude Code was last refused before it
    /// started (ms, 0 = never): the next one waits [`SWITCH_COOLDOWN_MS`] too. Apart from
    /// `switch_refused_ms`: a refused restart keeps no subscription switch waiting, and is
    /// neither shown nor recorded (SPEC-461 CCM-013).
    pub(crate) claude_move_refused_ms: AtomicI64,
    /// The recovery of a session whose context is full (SPEC-060); outlives a run, so a fresh
    /// session counts as its second stage.
    pub(crate) context: Mutex<crate::context_full::ContextEpisode>,
    /// Run endings being recorded (`finalize_attempt`, `mark_lost`): raised before the attempt is
    /// marked ended, lowered once its chains and queue work is done (SPEC-065 END-003).
    finalizing: tokio::sync::watch::Sender<u32>,
    /// Serializes final-screen publication and the corresponding attempt transition.
    recording_exit: tokio::sync::Mutex<()>,
    /// Run-ending observers using the old terminal or run, before onboarding and chain work.
    terminal_cleanup: tokio::sync::watch::Sender<u32>,
    /// The run whose host could not be terminated after its stop or close escalation, and why
    /// (SPEC-065 END-001): it stays live until its processes are gone.
    end_failed: Mutex<Option<(AttemptId, String)>>,
    /// Taken by each session activity write with its look at `end_failed`, and by the record of
    /// that failure: a write lands before the failure is recorded, or sees it and leaves the
    /// flag (SPEC-635 EFL-003).
    end_gate: tokio::sync::Mutex<()>,
    /// How many raises of a could-not-end flag are under way (from the failure's record until
    /// the raise is over, [`FlagPending`]): a session write meanwhile is held as if the flag
    /// stood, since it would land on top of it (EFL-003).
    end_flag_pending: AtomicU32,
    /// The attempt committed to park and its idle activity. Kept until the stored
    /// task state settles, including when a switch cleared its activity meanwhile.
    parked_activity: Mutex<Option<(AttemptId, Activity)>>,
    /// The request a reader took before it recorded this attempt's end (SPEC-563).
    taken_request: Mutex<Option<(AttemptId, TakenRequest)>>,
}

/// A write that found the run's writer taken by another one: nothing was written.
pub(crate) const SESSION_WRITER_BUSY: &str = "the session writer is busy";
impl TaskRuntime {
    /// The paste needs immediate writer access. Its Enter can wait, but both belong to the
    /// attempt that the watcher observed. A switch attaches a new writer to this runtime.
    pub(crate) async fn write_observed(
        &self,
        attempt: &AttemptId,
        now: Timestamp,
        input: SessionInput<'_>,
        immediate_paste: bool,
    ) -> AppResult<()> {
        let mut run = match input {
            SessionInput::Paste(_) | SessionInput::ContinuePaste(_) if immediate_paste => self
                .run
                .try_lock()
                .map_err(|_| AppError::Conflict(SESSION_WRITER_BUSY.into()))?,
            _ => self.run.lock().await,
        };
        let run = run
            .as_mut()
            .filter(|run| &run.attempt_id == attempt)
            .ok_or_else(|| {
                AppError::Conflict("the observed session is no longer attached".into())
            })?;
        self.write_session_input(run, now, input).await
    }

    /// Check with the writer acquired, both before a paste and before its separate Enter.
    async fn write_session_input(
        &self,
        run: &mut ActiveRun,
        now: Timestamp,
        input: SessionInput<'_>,
    ) -> AppResult<()> {
        if let SessionInput::Viewer(bytes) = input {
            return run.writer.input(bytes).await;
        }
        if let Some(hold) = &mut run.codex_input {
            if let Err(err) = self.check_codex_input_screen(&run.attempt_id) {
                if matches!(input, SessionInput::Submit) && hold.state == CodexPaste::InFlight {
                    hold.state = CodexPaste::Recovering;
                }
                return Err(err);
            }
            // A new paste can follow a cancelled message whose Enter never ran.
            let recovering = hold.state == CodexPaste::Recovering
                || (hold.state == CodexPaste::InFlight && matches!(input, SessionInput::Paste(_)));
            if recovering {
                if !self.hub.prompt_box_shown_empty(Harness::Codex) {
                    return Err(AppError::Conflict(
                        "Codex may still have an unsent automated paste. \
                         Inspect the composer before clearing any unsent paste and retrying."
                            .into(),
                    ));
                }
                hold.release()?;
            }
            let starting = hold.state == CodexPaste::Idle && !matches!(input, SessionInput::Submit);
            if starting {
                hold.record()?;
            }
            if recovering || starting {
                // The screen can redraw while the run's paste record is written.
                self.check_codex_input_screen(&run.attempt_id)?;
            }
        }
        self.last_input_ms.store(now.0, Ordering::SeqCst);
        let written = run.writer.input(input.bytes()).await;
        if let Some(hold) = &mut run.codex_input
            && hold.state == CodexPaste::InFlight
        {
            if written.is_err() {
                hold.state = CodexPaste::Recovering;
            } else if matches!(input, SessionInput::Submit) {
                // A submit already written to the PTY is sent even if removing its mark fails.
                let _ = hold.release();
            }
        }
        written
    }

    /// Runs `write` on the hub only while it shows the screen of `run`, under the lock an
    /// attach takes to switch the screen to its run: output, snapshots and reattachments of a
    /// replaced run never reach its replacement's screen (SPEC-614, #1624).
    pub(crate) fn on_screen_of(&self, run: &AttemptId, write: impl FnOnce(&Hub)) {
        let shown = self
            .screen_of
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if shown.as_ref() == Some(run) {
            write(&self.hub);
        }
    }

    /// Makes the hub show `run`'s screen (SPEC-301 LRS-001): `restore` resets it under the lock
    /// [`Self::on_screen_of`] checks, so nothing of the previous run is drawn after it.
    fn show_screen_of(&self, run: &AttemptId, restore: impl FnOnce(&Hub)) {
        let mut shown = self
            .screen_of
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        restore(&self.hub);
        *shown = Some(run.clone());
    }

    pub(crate) fn screen_attempt(&self) -> Option<AttemptId> {
        self.screen_of.lock().ok()?.clone()
    }

    /// Consumes the reader's request only for the attempt whose end was recorded.
    fn take_reader_request(&self, attempt_id: &AttemptId) -> Option<TakenRequest> {
        self.taken_request
            .lock()
            .ok()?
            .take_if(|(run, _)| run == attempt_id)
            .map(|(_, request)| request)
    }

    /// Reattaching the same host preserves its park. A new attempt retires it.
    fn retain_park_for_attached_run(&self, attempt_id: &AttemptId) {
        if let Ok(mut kept) = self.parked_activity.lock() {
            kept.take_if(|(parked, _)| parked != attempt_id);
        }
    }

    /// A reader can take the end intent before it writes the parked task state.
    pub(crate) fn park_committed(&self) -> bool {
        self.parked_activity
            .lock()
            .map_or(true, |kept| kept.is_some())
    }

    /// A committed park's run end is being recorded (SPEC-634 PSP-003): the reader took the
    /// park's intent, so `App::parking` no longer says so, and the task may still read
    /// Running until it settles. Nothing typed now reaches a session.
    pub(crate) fn park_end_recording(&self) -> bool {
        *self.terminal_cleanup.borrow() > 0 && self.park_committed()
    }

    pub(crate) fn closing_or_settling(&self) -> bool {
        self.intent.lock().map_or(true, |intent| {
            matches!(*intent, Some(EndIntent::Close)) || *self.terminal_cleanup.borrow() > 0
        })
    }

    /// Whether the screen `hub` shows is not what the program drew: its output is withheld
    /// (agent secrets cannot be read) or some was dropped since the last whole screen. A state
    /// that cannot be read counts.
    pub(crate) fn screen_withheld(&self) -> bool {
        self.scrub
            .lock()
            .map_or(true, |scrub| scrub.screen_unknown())
    }

    /// Whether a run is attached (a process can redraw the terminal).
    pub(crate) async fn has_run(&self) -> bool {
        self.run.lock().await.is_some()
    }

    /// Records `attempt`'s start counters (none where nothing counts), replacing an older run's.
    fn set_start_counters(&self, attempt: &AttemptId, counters: Option<LimitCounters>) {
        if let Ok(mut start) = self.limits_at_start.lock() {
            *start = counters.map(|counters| (attempt.clone(), counters));
        }
    }

    /// `attempt`'s start counters, left in place: an observer that loses the end's transition
    /// must not take them from the one that wins.
    fn start_counters(&self, attempt: &AttemptId) -> Option<LimitCounters> {
        self.limits_at_start
            .lock()
            .ok()?
            .as_ref()
            .filter(|(of, _)| of == attempt)
            .map(|(_, counters)| *counters)
    }

    /// Forgets `attempt`'s start counters once its end is recorded; another run's stay.
    fn clear_start_counters(&self, attempt: &AttemptId) {
        if let Ok(mut start) = self.limits_at_start.lock()
            && start.as_ref().is_some_and(|(of, _)| of == attempt)
        {
            *start = None;
        }
    }

    /// When something was last typed into the session's prompt box (ms, 0 = never): a person's
    /// keystroke, or a message Yulius typed for Claude Code to submit (SPEC-468 CCF-010,
    /// CCF-011).
    pub(crate) fn last_typed_ms(&self) -> i64 {
        self.typed_at_ms
            .load(Ordering::SeqCst)
            .max(self.message_typed_ms.load(Ordering::SeqCst))
    }

    /// The empty time for a strict timestamp comparison. When the two wall times
    /// coincide, use the order of the submit and key observed by this server.
    pub(crate) fn empty_time_for_typing(&self) -> i64 {
        let emptied = self.box_emptied_ms.load(Ordering::SeqCst);
        if self.typed_at_ms.load(Ordering::SeqCst) == emptied
            && self.typed_seq.load(Ordering::SeqCst) > self.emptied_seq.load(Ordering::SeqCst)
        {
            emptied.saturating_sub(1)
        } else {
            emptied
        }
    }

    /// A run starts at `now_ms`, in a new process: its prompt box is empty, and no prompt of
    /// it is held (one held in the run before went with that run's hook client; SPEC-468
    /// CCF-010). Nothing waits in its queue and no prompt of it went on: what the run before
    /// queued went with its process (SPEC-471 CCQ-001).
    pub(crate) async fn run_starts(&self, now_ms: i64) {
        let mut mark = self.draft_mark.lock().await;
        self.box_emptied_ms.store(now_ms, Ordering::SeqCst);
        // Its folder is new: no draft is marked there (SPEC-480 DRF-009).
        *mark = DraftMark::default();
        *self
            .viewer_prompt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Default::default();
        self.typed_seq.store(0, Ordering::SeqCst);
        self.emptied_seq.store(0, Ordering::SeqCst);
        self.sent_past_draft.store(false, Ordering::SeqCst);
        self.prompt_held.store(false, Ordering::SeqCst);
        self.with_queue(|queue| *queue = yulius_domain::agent_cli::Queue::default());
        self.queue_unread.store(false, Ordering::SeqCst);
        self.prompt_passed_ms.store(0, Ordering::SeqCst);
    }

    /// What the run's lines said so far about Claude Code's queue.
    pub(crate) fn queue(&self) -> yulius_domain::agent_cli::Queue {
        self.queue.lock().map(|queue| *queue).unwrap_or_default()
    }

    /// Whether Claude Code's queue may hold something a restart would lose: its records say
    /// so, or they were not read yet ([`Self::queue_unread`]), or what they said cannot be
    /// read (a poisoned lock): not known counts as holding.
    pub(crate) fn queue_may_hold(&self) -> bool {
        self.queue_unread.load(Ordering::SeqCst)
            || self.queue.lock().map_or(true, |queue| queue.holds())
    }

    /// Whether Claude Code's queue may hold something a turn's end hands on, which a move of
    /// the session at that end would lose (SPEC-479 SWQ-001): its records say so, or they were
    /// not read yet, or what they said cannot be read. An entry a turn's end left behind does
    /// not count ([`yulius_domain::agent_cli::Queue::hands_on`]).
    pub(crate) fn queue_may_hand_on(&self) -> bool {
        self.queue_unread.load(Ordering::SeqCst)
            || self.queue.lock().map_or(true, |queue| queue.hands_on())
    }

    /// Reads or changes what is known of the session's dynamic workflows (SPEC-486).
    pub(crate) fn with_workflows<T: Default>(
        &self,
        change: impl FnOnce(&mut crate::dynamic_workflow::Unfinished) -> T,
    ) -> T {
        self.workflows
            .lock()
            .map(|mut workflows| change(&mut workflows))
            .unwrap_or_default()
    }

    /// Changes what is known of the queue.
    pub(crate) fn with_queue(&self, change: impl FnOnce(&mut yulius_domain::agent_cli::Queue)) {
        if let Ok(mut queue) = self.queue.lock() {
            change(&mut queue);
            // News the watch typed while a turn ran shows in the queue, which decides from
            // here on how long it waits (SPEC-487 CHN-002). Whatever the queue hands on after
            // the typing counts: what leaves it leaves in the order it came.
            if queue.hands_on()
                && let Ok(mut news) = self.pr_news.lock()
                && let Some(news) = news.as_mut()
            {
                news.in_queue = true;
            }
        }
    }

    pub(crate) fn new() -> Self {
        Self {
            hub: Hub::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            run: tokio::sync::Mutex::new(None),
            screen_of: Mutex::new(None),
            intent: Mutex::new(None),
            usage: Arc::new(Mutex::new(UsageSnapshot::default())),
            usage_read: Mutex::new(None),
            watcher_alive: Arc::new(AtomicBool::new(false)),
            watcher_generation: AtomicU64::new(0),
            last_input_ms: AtomicI64::new(0),
            proactive_switch: AtomicBool::new(false),
            rebalance_no_room: Mutex::new(None),
            cut_off_reviewer: tokio::sync::Mutex::new(None),
            reviewer_move: AtomicBool::new(false),
            hooks_seen: AtomicBool::new(false),
            at_limit: AtomicBool::new(false),
            workflows: Mutex::new(crate::dynamic_workflow::Unfinished::default()),
            limit_goal_pause: Mutex::new(crate::goal_retry::LimitGoalPause::None),
            limit_at_ms: AtomicI64::new(0),
            blind_limits: Mutex::new(crate::moves::BlindLimits::default()),
            switched_limit: Mutex::new(None),
            turn_started_ms: AtomicI64::new(0),
            auto_pr_head: Mutex::new(None),
            merged_covers: Mutex::new(None),
            candidate_observation: Mutex::default(),
            typed_at_ms: AtomicI64::new(0),
            message_typed_ms: AtomicI64::new(0),
            box_emptied_ms: AtomicI64::new(0),
            draft_told_ms: AtomicI64::new(0),
            draft_mark: tokio::sync::Mutex::default(),
            viewer_prompt: Mutex::default(),
            prompt_receipts: Mutex::default(),
            typed_seq: AtomicU64::new(0),
            sent_past_draft: AtomicBool::new(false),
            emptied_seq: AtomicU64::new(0),
            prompt_held: AtomicBool::new(false),
            typed_by: Mutex::new(None),
            reattach_move: Arc::default(),
            personal_mcp_of: Mutex::new(None),
            goal_achieved: AtomicBool::new(false),
            goal_submitted: AtomicBool::new(false),
            turn_end_ms: AtomicI64::new(0),
            turn_order: tokio::sync::Mutex::default(),
            antigravity_hook_ms: tokio::sync::Mutex::default(),
            session_started: AtomicBool::new(false),
            scrub: Mutex::new(crate::vault::TerminalScrub::default()),
            screen_flagged: AtomicBool::new(false),
            opening_turn: AtomicBool::new(false),
            typing_opening: AtomicBool::new(false),
            typing_opening_said: AtomicBool::new(false),
            typing_message: AtomicBool::new(false),
            spawned_ms: AtomicI64::new(0),
            armed: Mutex::new(crate::agent_wait::ArmedSlot::default()),
            arm_writer: Arc::default(),
            push_problem: Mutex::new(None),
            repair_checked_head: Mutex::new(None),
            last_stop_ms: AtomicI64::new(0),
            stop_answered_ms: AtomicI64::new(0),
            turn_began_ms: AtomicI64::new(0),
            stops_in_flight: AtomicU32::new(0),
            turn_record_at_ms: AtomicI64::new(0),
            turn_closed_ms: AtomicI64::new(0),
            turn_moved_ms: AtomicI64::new(0),
            woken_after_outage_ms: AtomicI64::new(0),
            mcp_seen_ms: AtomicI64::new(0),
            mcp_pinged_ms: AtomicI64::new(0),
            mcp_reconnects: Mutex::new(crate::mcp_watch::Reconnects::default()),
            pr_stalled: Mutex::new(None),
            pr_news: Mutex::new(None),
            pr_conflict: Mutex::new(None),
            pr_merged_note: Mutex::new(None),
            background_shells: AtomicI64::new(-1),
            limits_at_start: Mutex::new(None),
            background_waits: AtomicI64::new(-1),
            active_waits: AtomicU32::new(0),
            shells_since_ms: AtomicI64::new(0),
            strays_seen: Mutex::new(HashMap::new()),
            background_agents: AtomicU32::new(0),
            notice_attention_ms: AtomicI64::new(0),
            secret_wait_at: AtomicI64::new(0),
            attention_lock: tokio::sync::Mutex::new(()),
            steps_lock: tokio::sync::Mutex::new(()),
            turn_record_ms: AtomicI64::new(0),
            resumed_turn: AtomicBool::new(false),
            goal_retry_armed: AtomicBool::new(false),
            goal_pause_asked: AtomicBool::new(false),
            launches: Mutex::new(std::collections::BTreeMap::new()),
            ended_stretch: Mutex::new(None),
            goal_open: AtomicBool::new(false),
            goal_unfinished: AtomicBool::new(false),
            verdict_stop_ms: crate::finish::VerdictSlot::default(),
            queue: Mutex::default(),
            prompt_passed_ms: AtomicI64::new(0),
            prompts_passed: AtomicU64::new(0),
            queue_unread: AtomicBool::new(false),
            watch_polls: AtomicU64::new(0),
            replay_transcript: AtomicBool::new(false),
            session_end: Mutex::new(None),
            hold_polled_ms: AtomicI64::new(0),
            person_freeze: std::sync::atomic::AtomicU8::new(0),
            person_held_ms: AtomicI64::new(0),
            nudged: Mutex::new(std::collections::BTreeSet::new()),
            acting_as_app_said: AtomicBool::new(false),
            switch_refused_ms: AtomicI64::new(0),
            claude_move_refused_ms: AtomicI64::new(0),
            context: Mutex::new(crate::context_full::ContextEpisode::default()),
            finalizing: tokio::sync::watch::channel(0).0,
            recording_exit: tokio::sync::Mutex::new(()),
            terminal_cleanup: tokio::sync::watch::channel(0).0,
            end_failed: Mutex::new(None),
            end_gate: tokio::sync::Mutex::new(()),
            end_flag_pending: AtomicU32::new(0),
            parked_activity: Mutex::new(None),
            taken_request: Mutex::new(None),
        }
    }

    /// Why the host of `attempt` could not be terminated, if it could not (SPEC-065 END-001).
    fn end_failed_for(&self, attempt: &AttemptId) -> Option<String> {
        self.end_failed
            .lock()
            .ok()?
            .as_ref()
            .filter(|(id, _)| id == attempt)
            .map(|(_, why)| why.clone())
    }

    /// Whether the run's transcript is being watched (a turn record can still arrive).
    pub(crate) fn is_watching(&self) -> bool {
        self.watcher_alive.load(Ordering::SeqCst)
    }

    /// How many rounds the transcript watcher has begun: once two more began, the lines written
    /// before were read and applied (what tests and the Stop hook wait on, SPEC-079 BGW-004).
    pub fn transcript_rounds(&self) -> u64 {
        self.watch_polls.load(Ordering::SeqCst)
    }

    /// Whether a turn is known to be in progress (one started and none ended since).
    pub(crate) fn turn_in_progress(&self) -> bool {
        self.turn_started_ms.load(Ordering::SeqCst) > 0
    }

    /// Whether a live PTY writer is currently attached to this runtime.
    pub(crate) async fn has_live_run(&self) -> bool {
        self.run.lock().await.is_some()
    }

    /// Records the native goal under the final parking check's lock, returning the previous value.
    pub(crate) async fn replace_goal_unfinished(&self, unfinished: bool) -> bool {
        let _work = self.steps_lock.lock().await;
        self.goal_unfinished.swap(unfinished, Ordering::SeqCst)
    }

    /// A turn started now unless one is already in progress.
    pub(crate) fn start_turn(&self, now: Timestamp) {
        let _ = self
            .turn_started_ms
            .compare_exchange(0, now.0, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Ends the turn in progress, if any.
    pub(crate) fn end_turn(&self) {
        self.turn_started_ms.store(0, Ordering::SeqCst);
    }

    /// A usage limit or a refused subscription, recorded `at`, cut the run's turn: the run is at
    /// its limit, and nothing is known yet of a goal pause written there (SPEC-464 UCL-007).
    fn limit_cut_the_turn(
        &self,
        attempt: &AttemptId,
        at: Option<Timestamp>,
        ran: Option<ModelFamily>,
    ) {
        if let Ok(mut kept) = self.switched_limit.lock() {
            *kept = ran.map(|family| (attempt.clone(), family));
        }
        if let Ok(mut pause) = self.limit_goal_pause.lock() {
            *pause = crate::goal_retry::LimitGoalPause::None;
        }
        self.limit_at_ms
            .store(at.map_or(0, |at| at.0), Ordering::SeqCst);
        self.at_limit.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
struct ViewerSubmission {
    time: i64,
    sequence: u64,
    enter: crate::terminal::ViewerEnter,
}

/// Supervisor-owned per-process state.
#[derive(Default)]
pub struct SupervisorState {
    /// Viewer Enter events awaiting the provider's submit callback. Contains only timing, order and provenance evidence.
    viewer_submits: Mutex<HashMap<TaskId, VecDeque<ViewerSubmission>>>,
    input_seq: AtomicU64,
    runtimes: Mutex<HashMap<TaskId, Arc<TaskRuntime>>>,
    /// Shared, so an install on the blocking pool can own its guard (SPEC-507 SKR-005).
    launch: Arc<tokio::sync::Mutex<()>>,
    /// The task the one launch under `launch` works on: `Starting` with no run on record yet,
    /// which the start-up reconciliation must not take for a dead server's (SPEC-458 RUN-001).
    launching: Mutex<Option<TaskId>>,
}

impl App {
    /// Runtime for a task, created on demand.
    pub fn runtime(&self, task: &TaskId) -> Arc<TaskRuntime> {
        let Ok(mut map) = self.supervisor.runtimes.lock() else {
            return Arc::new(TaskRuntime::new());
        };
        map.entry(task.clone())
            .or_insert_with(|| Arc::new(TaskRuntime::new()))
            .clone()
    }

    /// Drops a deleted task's runtime state.
    pub(crate) fn forget_runtime(&self, task: &TaskId) {
        self.consume_viewer_submits(task, u64::MAX);
        if let Ok(mut map) = self.supervisor.runtimes.lock() {
            map.remove(task);
        }
    }

    pub(crate) fn existing_runtime(&self, task: &TaskId) -> Option<Arc<TaskRuntime>> {
        self.supervisor
            .runtimes
            .lock()
            .ok()
            .and_then(|m| m.get(task).cloned())
    }
}

/// A could-not-end flag being raised (`TaskRuntime::end_flag_pending`): counted from its
/// record until dropped, whether the raise ended, was cancelled or panicked.
struct FlagPending<'a>(&'a AtomicU32);

impl<'a> FlagPending<'a> {
    fn raise(pending: &'a AtomicU32) -> Self {
        pending.fetch_add(1, Ordering::SeqCst);
        Self(pending)
    }
}

impl Drop for FlagPending<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How the flag of a run whose host could not be terminated begins.
const COULD_NOT_END: &str = "Yulius could not end this session (";

/// What the flag of a run whose host could not be terminated (`why`) says is wrong.
fn could_not_end(why: &str) -> String {
    format!("{COULD_NOT_END}{why}). It still counts as running, so the task cannot be deleted.")
}

/// Whether `reason` is a flag [`could_not_end`] raised, for this failure or an earlier one of
/// the same run (whose words differ: the flag keeps its first reason).
fn is_could_not_end(reason: &AttentionRecord) -> bool {
    reason.kind == crate::attention::AttentionKind::Yulius
        && reason
            .problem
            .as_deref()
            .is_some_and(|problem| problem.starts_with(COULD_NOT_END))
}

#[cfg(test)]
mod tests {
    /// SPEC-621 FIN-003: from its Close intent until its run's end has settled, a session
    /// refuses a new merge arm.
    #[test]
    fn a_close_intent_or_a_settling_run_counts_as_closing() {
        let runtime = super::TaskRuntime::new();
        assert!(!runtime.closing_or_settling());
        if let Ok(mut intent) = runtime.intent.lock() {
            *intent = Some(super::EndIntent::Close);
        }
        assert!(runtime.closing_or_settling());
        if let Ok(mut intent) = runtime.intent.lock() {
            *intent = None;
        }
        runtime.terminal_cleanup.send_replace(1);
        assert!(runtime.closing_or_settling());
    }

    /// SPEC-634 PSP-003: only a committed park whose run end is being recorded refuses typed
    /// input as parking; another run's end, or a park not yet ending, does not.
    #[test]
    fn only_a_committed_parks_recorded_end_counts_as_park_end_recording()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = super::TaskRuntime::new();
        assert!(!runtime.park_end_recording());
        runtime.terminal_cleanup.send_replace(1);
        assert!(!runtime.park_end_recording(), "a run end without a park");
        runtime.terminal_cleanup.send_replace(0);
        *runtime.parked_activity.lock().map_err(|_| "park lock")? = Some((
            super::AttemptId::parse("parking-run")?,
            super::Activity::Waiting,
        ));
        assert!(
            !runtime.park_end_recording(),
            "a park whose run end has not begun"
        );
        runtime.terminal_cleanup.send_replace(1);
        assert!(runtime.park_end_recording());
        Ok(())
    }

    /// SPEC-468 CCF-010: a run starts with an empty prompt box and holds no prompt: one held
    /// in the run before went with that run's hook client, so the new run's first prompt is a
    /// submit, not that prompt sent again.
    #[tokio::test]
    async fn a_new_run_starts_with_an_empty_prompt_box_and_no_held_prompt() {
        use std::sync::atomic::Ordering;
        let runtime = super::TaskRuntime::new();
        runtime.typed_at_ms.store(5_000, Ordering::SeqCst);
        runtime.prompt_held.store(true, Ordering::SeqCst);
        runtime.run_starts(7_000).await;
        assert_eq!(runtime.box_emptied_ms.load(Ordering::SeqCst), 7_000);
        assert!(!runtime.prompt_held.load(Ordering::SeqCst));
    }

    /// SPEC-471 CCQ-001: what the run before left in Claude Code's queue went with its process,
    /// and a resumed session does not get it back: a run starts with an empty queue and with
    /// no prompt that just went on.
    #[tokio::test]
    async fn a_new_run_starts_with_an_empty_queue() {
        use std::sync::atomic::Ordering;
        use yulius_domain::agent_cli::QueueChange;
        let runtime = super::TaskRuntime::new();
        runtime.with_queue(|queue| queue.record(QueueChange::Added, None));
        runtime.with_queue(|queue| queue.record(QueueChange::HandedOn, None));
        runtime.with_queue(|queue| queue.record(QueueChange::Added, None));
        runtime.prompt_passed_ms.store(6_500, Ordering::SeqCst);
        assert!(runtime.queue().holds());
        runtime.run_starts(7_000).await;
        assert_eq!(runtime.queue(), yulius_domain::agent_cli::Queue::default());
        assert_eq!(runtime.prompt_passed_ms.load(Ordering::SeqCst), 0);
        assert!(!runtime.queue_may_hold());
        // A run reattached after a restart of the server: not known until its lines were read,
        // which counts as holding; the next launch starts a process whose queue is empty.
        runtime.queue_unread.store(true, Ordering::SeqCst);
        assert!(runtime.queue_may_hold(), "not known counts as holding");
        runtime.run_starts(8_000).await;
        assert!(!runtime.queue_may_hold());
    }

    /// SPEC-471 CCQ-001: what the queue's records said cannot be read any more once a thread
    /// died holding them: that is "not known", which counts as holding something.
    #[test]
    fn a_queue_that_cannot_be_read_counts_as_holding() {
        let runtime = std::sync::Arc::new(super::TaskRuntime::new());
        assert!(!runtime.queue_may_hold());
        let dying = std::sync::Arc::clone(&runtime);
        let died = std::thread::spawn(move || {
            let _held = dying.queue.lock();
            std::panic::resume_unwind(Box::new("a thread dies with the queue's lock"));
        })
        .join();
        assert!(died.is_err());
        assert!(runtime.queue.is_poisoned());
        assert!(runtime.queue_may_hold());
    }

    /// SPEC-479 SWQ-001, SWQ-002: what a move at a turn's end asks of the queue. An entry that
    /// waits counts until a turn's closing record found it waiting; a queue that is not known
    /// (a reattached run not read yet, a lock a thread died with) counts; a new run has none.
    #[tokio::test]
    async fn a_move_asks_the_queue_for_what_a_turns_end_hands_on() {
        use std::sync::atomic::Ordering;
        use yulius_domain::agent_cli::{Queue, QueueChange};
        let runtime = std::sync::Arc::new(super::TaskRuntime::new());
        assert!(!runtime.queue_may_hand_on());
        runtime.with_queue(|queue| queue.record(QueueChange::Added, None));
        assert!(runtime.queue_may_hand_on());
        runtime.with_queue(Queue::turn_closed);
        assert!(!runtime.queue_may_hand_on(), "left by a turn's end");
        assert!(runtime.queue_may_hold(), "the restart's rule counts it");
        runtime.queue_unread.store(true, Ordering::SeqCst);
        assert!(runtime.queue_may_hand_on(), "not known counts");
        runtime.run_starts(8_000).await;
        assert!(!runtime.queue_may_hand_on());
        assert_eq!(runtime.queue().left_over(), 0);
        let dying = std::sync::Arc::clone(&runtime);
        let died = std::thread::spawn(move || {
            let _held = dying.queue.lock();
            std::panic::resume_unwind(Box::new("a thread dies with the queue's lock"));
        })
        .join();
        assert!(died.is_err());
        assert!(runtime.queue_may_hand_on());
    }

    /// SPEC-635 (review round 3): a could-not-end flag counts as being raised until its raise is
    /// over, however it ends (a cancelled future drops it too), and two raises of one task are
    /// counted apart: the first one's end does not end the second's.
    #[test]
    fn a_flag_being_raised_counts_until_its_raise_is_dropped() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let pending = AtomicU32::new(0);
        let first = super::FlagPending::raise(&pending);
        let second = super::FlagPending::raise(&pending);
        drop(first);
        assert_eq!(
            pending.load(Ordering::SeqCst),
            1,
            "the second raise still counts"
        );
        let cancelled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _raising = second;
            panic!("the raise was cut short");
        }));
        assert!(cancelled.is_err());
        assert_eq!(pending.load(Ordering::SeqCst), 0, "nothing is left pending");
    }

    /// SPEC-635 (review round 2): a run's could-not-end flag is known by its wording, whatever
    /// the failure's words: a second failure of the same run says something else (a pid), and
    /// the flag keeps its first reason. Another flag of Yulius's, or Claude Code's, is not it.
    #[test]
    fn a_could_not_end_flag_is_known_whatever_the_failures_words() {
        use crate::attention::AttentionRecord;
        let first = AttentionRecord::yulius(&super::could_not_end("pid 41 survived"), "Stop.");
        assert!(super::is_could_not_end(&first));
        let second = AttentionRecord::yulius(&super::could_not_end("pid 42 survived"), "Stop.");
        assert!(super::is_could_not_end(&second));
        assert!(!super::is_could_not_end(&AttentionRecord::yulius(
            "Another matter.",
            "Look."
        )));
        assert!(!super::is_could_not_end(&AttentionRecord::terminal(Some(
            "Yulius could not end this session (said in the terminal)"
        ))));
    }

    #[test]
    fn reattachment_preserves_only_the_attached_attempts_unsettled_park()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = super::TaskRuntime::new();
        let parked = super::AttemptId::parse("previous-owner")?;
        let replacement = super::AttemptId::parse("continued-owner")?;
        *runtime.parked_activity.lock().map_err(|_| "park lock")? =
            Some((parked.clone(), super::Activity::Waiting));
        runtime.retain_park_for_attached_run(&parked);
        assert!(runtime.park_committed());
        runtime.retain_park_for_attached_run(&replacement);
        assert!(!runtime.park_committed());
        Ok(())
    }

    /// SPEC-614 TRO-001, TRO-003: the hub takes a run's writes only while it shows that run, and
    /// an attach resets the hub while it holds the lock those writes check.
    #[test]
    fn the_hub_takes_writes_only_of_the_run_it_shows() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = super::TaskRuntime::new();
        let old = super::AttemptId::parse("old-run")?;
        let new = super::AttemptId::parse("new-run")?;
        let wrote = |run: &super::AttemptId| {
            let mut wrote = false;
            runtime.on_screen_of(run, |_| wrote = true);
            wrote
        };
        assert!(!wrote(&old), "no run's screen is shown yet");
        runtime.show_screen_of(&old, |hub| hub.output(1, b"old"));
        assert!(wrote(&old));
        assert!(!wrote(&new));
        let mut locked = false;
        runtime.show_screen_of(&new, |_| {
            locked = runtime.screen_of.try_lock().is_err();
        });
        assert!(locked, "the reset runs under the lock the writes check");
        assert!(!wrote(&old), "the replaced run writes nothing");
        assert!(wrote(&new));
        Ok(())
    }

    #[test]
    fn a_taken_request_belongs_to_one_attempt_and_is_consumed_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = super::TaskRuntime::new();
        let owner = super::AttemptId::parse("a-owner")?;
        let stale = super::AttemptId::parse("a-stale")?;
        *runtime.taken_request.lock().map_err(|_| "request lock")? =
            Some((owner.clone(), super::TakenRequest::Close));
        assert!(runtime.take_reader_request(&stale).is_none());
        assert!(matches!(
            runtime.take_reader_request(&owner),
            Some(super::TakenRequest::Close)
        ));
        assert!(runtime.take_reader_request(&owner).is_none());
        Ok(())
    }

    /// SPEC-268 PID-005, review round 1: an attempt reads only its own start counters, and a late
    /// end of an older attempt (or an observer that lost the transition) leaves the newer one's.
    #[test]
    fn a_runs_start_counters_are_its_own() -> Result<(), Box<dyn std::error::Error>> {
        use crate::ports::isolation::LimitCounters;
        let runtime = super::TaskRuntime::new();
        let old = yulius_domain::ids::AttemptId::parse("a-old")?;
        let new = yulius_domain::ids::AttemptId::parse("a-new")?;
        let at = LimitCounters {
            task_limit_hits: 2,
            oom_kills: 0,
        };
        runtime.set_start_counters(&new, Some(at));
        assert_eq!(runtime.start_counters(&old), None);
        runtime.clear_start_counters(&old);
        assert_eq!(runtime.start_counters(&new), Some(at), "read twice");
        assert_eq!(runtime.start_counters(&new), Some(at));
        runtime.clear_start_counters(&new);
        assert_eq!(runtime.start_counters(&new), None);
        Ok(())
    }
}
