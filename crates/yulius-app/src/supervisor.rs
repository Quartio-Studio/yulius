//! Session supervisor: owns the lifecycle of every agent run (SPEC-002 §6.1–§6.3, §7.1).
//!
//! Invariants:
//! - at most one live attempt per task (store index + CAS transitions);
//! - capacity is released only after the run's exit was observed or the run was declared lost;
//! - every attempt records exactly one terminal outcome;
//! - a record that says "running" is never respawned automatically after a restart.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::{Harness, claude_model_name};
use yulius_domain::ids::{AccountId, AttemptId, TaskId};
use yulius_domain::rotation::{self, AccountView, Decision, Trigger};
use yulius_domain::task::{Activity, STOP_INTERRUPT_GRACE_MS, StopReason, TaskEvent, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, FULL_PERCENT, ModelFamily, ReportedLimit};

use crate::attention::AttentionRecord;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::finish::{SHELL_PROBE_MS, TranscriptFacts};
use crate::hub::Hub;
use crate::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, PauseKind, TaskMode, TaskRecord,
};
use crate::ports::harness::{Observation, UsageSnapshot, WatchFrom};
use crate::ports::isolation::LimitCounters;
use crate::ports::session::{RunEvent, RunWriter};
use crate::usage_json;

mod antigravity;
mod codex;
mod end;
mod launch;
mod park;
mod start;
mod switch;
mod typing;

use antigravity::antigravity_run_home;
pub(crate) use antigravity::read_antigravity_token_in;

pub(crate) use codex::{CODEX_PICKER, CODEX_UNREADABLE};
use codex::{CodexInput, CodexPaste};
pub(crate) use end::metrics_of;
pub use launch::branch_for;
pub(crate) use launch::git_credential_env;
pub use start::RunStart;
pub(crate) use start::{CONTINUE_TASK_PROMPT, SwitchCondition};
pub use switch::SWITCH_COOLDOWN_MS;
use typing::CLAUDE_CREDENTIAL_DIGEST;
pub use typing::{GOAL_READY_TIMEOUT, TRUST_ANSWER_TIMEOUT};
pub(crate) use typing::{
    SCREEN_WITHHELD, clear_person_typed, launch_goal_bound_ms, mark_person_typed,
    mark_session_started, native_goal_pending, opening_entered, opening_sent, person_typed_marked,
    read_final_screen, read_saved_screen, save_screen, trust_prompt_shown, write_private_file,
};

/// Boxed `Send` future (breaks recursive async cycles between start, pump and finalize).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Default PTY size before the first viewer reports its size.
pub const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// Transcript poll interval.
pub const WATCH_INTERVAL_MS: u64 = 1_000;
/// How often a live run's steps are synced from its spec's Plan (SPEC-341 LSS-001).
const SPEC_SYNC_INTERVAL: Duration = Duration::from_secs(5);
/// Reattach attempts after an unexpected host disconnect.
pub const REATTACH_ATTEMPTS: u32 = 3;
/// How long a closed run may take to be recorded as ended after the close escalation (SPEC-039).
const CLOSE_SETTLE_MS: u64 = 5_000;
/// How long asking a run to shut down may take: one frame on a local socket; a host that takes
/// longer is stuck, which the escalation handles (SPEC-065 END-005).
const SHUTDOWN_WRITE_BOUND: Duration = Duration::from_secs(2);
/// How many times a stop makes its status change: each lost one means another writer changed the
/// status, and the status machine has no cycle a stop can keep losing to (SPEC-250 STW-001).
const STOP_ROUNDS: usize = 4;
/// How often a close waiting for its run looks again without an update (SPEC-039).
const CLOSE_RECHECK: Duration = Duration::from_secs(1);

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

/// Longest gap between live metrics saves of a run in progress (#31).
const LIVE_METRICS_MS: i64 = 30_000;

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

    /// The model families waiting tasks have excluded from this account. A window reading
    /// can already have cleared the stored limit by the time a person asks for a refresh.
    pub(crate) async fn excluded_families(&self, account: &AccountId) -> Vec<ModelFamily> {
        let mut families = Vec::new();
        let Ok(pauses) = self.deps.store.open_pauses().await else {
            return families;
        };
        for pause in pauses {
            if let Some(runtime) = self.existing_runtime(&pause.task_id)
                && let Ok(limits) = runtime.blind_limits.lock()
            {
                for (id, family) in limits.left_out() {
                    if id == account && !families.contains(family) {
                        families.push(*family);
                    }
                }
            }
        }
        families
    }

    /// A successful model probe allows this account/model pair in each task's next placement.
    pub(crate) fn model_available(&self, account: &AccountId, family: ModelFamily) {
        if let Ok(runtimes) = self.supervisor.runtimes.lock() {
            for runtime in runtimes.values() {
                if let Ok(mut limits) = runtime.blind_limits.lock() {
                    limits.available(account, family);
                }
            }
        }
        self.capacity_changed();
    }

    /// A delivery records its actual bytes before the provider can acknowledge it.
    /// This disambiguates its callback from an owner editing or recalling a prompt.
    pub(crate) fn note_issued_prompt(&self, task: &TaskId, text: &[u8]) {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(text).into();
        let runtime = self.runtime(task);
        let mut receipts = runtime
            .prompt_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let issued = &mut receipts.issued;
        const LIMIT: usize = 16;
        let pending = issued
            .iter()
            .position(|p| p.digest == digest)
            .and_then(|index| issued.remove(index))
            .map_or(1, |p| p.pending.saturating_add(1));
        if issued.len() == LIMIT {
            issued.pop_front();
        }
        issued.push_back(IssuedPrompt { digest, pending });
    }

    pub(crate) fn record_viewer_key(
        &self,
        task: &TaskId,
        at: i64,
        enter: crate::terminal::ViewerEnter,
    ) -> u64 {
        let seq = self.supervisor.input_seq.fetch_add(1, Ordering::SeqCst) + 1;
        if enter.may_submit() {
            let mut submits = self
                .supervisor
                .viewer_submits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let pending = submits.entry(task.clone()).or_default();
            // Keep the first unacknowledged Enter and the most recent submissions.
            const MAX_PENDING_SUBMITS: usize = 256;
            if pending.len() == MAX_PENDING_SUBMITS {
                pending.remove(1);
            }
            pending.push_back(ViewerSubmission {
                time: at,
                sequence: seq,
                enter,
            });
        }
        seq
    }

    pub(crate) fn viewer_input_sequence(&self) -> u64 {
        self.supervisor.input_seq.load(Ordering::SeqCst)
    }

    pub(crate) fn retract_viewer_submit(&self, task: &TaskId, seq: u64) {
        let mut submits = self
            .supervisor
            .viewer_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pending) = submits.get_mut(task) {
            pending.retain(|key| key.sequence != seq);
            if pending.is_empty() {
                submits.remove(task);
            }
        }
    }

    pub(crate) fn viewer_submit_at(
        &self,
        task: &TaskId,
        sent: i64,
        sequence: u64,
        prompt: &[u8],
        invocation: Option<u128>,
    ) -> Option<(i64, u64)> {
        use crate::terminal::ViewerEnter;
        use sha2::Digest;
        let digest: [u8; 32] = sha2::Sha256::digest(prompt).into();
        let runtime = self.runtime(task);
        let mut submits = self
            .supervisor
            .viewer_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut eligible = submits.get(task).into_iter().flatten().filter(|key| {
            key.sequence <= sequence
                && key.time <= sent
                && sent.saturating_sub(key.time) <= yulius_domain::intake::TYPED_WINDOW_MS
        });
        let mut receipts = runtime
            .prompt_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(id) = invocation {
            // A replay cannot acknowledge another owner's Enter, even if the words match.
            if let Some(previous) = receipts
                .invocations
                .iter_mut()
                .find(|request| request.id == id)
            {
                return previous.held_owner.take();
            }
            const LIMIT: usize = 128;
            if receipts.invocations.len() == LIMIT {
                receipts.invocations.pop_front();
            }
            receipts.invocations.push_back(SubmitInvocation {
                id,
                held_owner: None,
            });
        }
        let direct = eligible
            .clone()
            .find(|key| key.enter == ViewerEnter::Submitted(digest));
        // An invocation with different identity may submit an edited copy of an
        // acknowledged instruction. Older clients need direct input evidence for a copy.
        let owner =
            if direct.is_some() {
                direct
            } else if let Some(issued) = receipts.issued.iter_mut().find(|issued| {
                issued.digest == digest && (invocation.is_none() || issued.pending > 0)
            }) {
                issued.pending = issued.pending.saturating_sub(1);
                None
            } else {
                eligible.next()
            };
        let owner = owner.map(|key| (key.time, key.sequence));
        if let (Some(_), Some((_, seq))) = (invocation, owner)
            && let Some(pending) = submits.get_mut(task)
        {
            // Transfer the Enter to its invocation before another callback can claim it.
            // A held invocation retains the selected key for its own release.
            pending.retain(|key| key.sequence != seq);
            if pending.is_empty() {
                submits.remove(task);
            }
        }
        owner
    }

    /// The release of a held submit acknowledges the original Enter, even if
    /// another invocation was processed while the subscription was unavailable.
    pub(crate) fn keep_held_owner(&self, task: &TaskId, id: u128, owner: (i64, u64)) {
        let runtime = self.runtime(task);
        let mut receipts = runtime
            .prompt_receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(request) = receipts
            .invocations
            .iter_mut()
            .find(|request| request.id == id)
        {
            request.held_owner = Some(owner);
        }
    }

    pub(crate) fn consume_viewer_submits(&self, task: &TaskId, through: u64) {
        let mut keys = self
            .supervisor
            .viewer_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pending) = keys.get_mut(task) {
            pending.retain(|key| key.sequence > through);
            if pending.is_empty() {
                keys.remove(task);
            }
        }
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

    /// Account views for the rotation policy; Done tasks' idle sessions are no load (SPEC-057).
    pub(crate) async fn account_views(&self) -> AppResult<Vec<AccountView>> {
        let accounts = self.deps.store.accounts().await?;
        let live = self.working_attempts().await?;
        Ok(accounts
            .iter()
            .filter(|a| a.credential.is_some())
            .map(|account| {
                crate::capacity::rotation_view(
                    account,
                    crate::accounts::in_use_count(&account.id, &live),
                )
            })
            .collect())
    }

    /// When a paused task resumes: the earliest known time a subscription has capacity for it
    /// again, as the pool policy decides now (ROT-010). `None` while the task is not paused,
    /// once capacity is back, or when no reset time is known.
    pub(crate) async fn capacity_returns_at(&self, task: &TaskRecord) -> Option<Timestamp> {
        if task.status != TaskStatus::WaitingCapacity {
            return None;
        }
        let input = self.rotation_input(task, None).await.ok()?;
        // As the resume loop decides: off what refused the task with no end (SPEC-464 UCL-010).
        match self.decide_for_waiting(&task.id, &input, Trigger::Start).0 {
            Decision::Wait { until } => until,
            Decision::Switch { .. } | Decision::Stay => None,
        }
    }

    /// Where a new attempt of `task` runs (placement, ROT-004): its account, and the fallback
    /// model it is launched with while no subscription can run the task's own (SPEC-482). `None`
    /// when no subscription has capacity and the task must pause (ROT-010). A requested account
    /// is honoured when it is usable and refused otherwise (ROT-007).
    pub(crate) async fn choose_account(
        &self,
        task: &TaskRecord,
        requested: Option<&AccountId>,
    ) -> AppResult<Option<Placed>> {
        let trigger = match requested {
            Some(id) => Trigger::Manual {
                account: id.clone(),
            },
            None => Trigger::Start,
        };
        let input = self.rotation_input(task, None).await?;
        match rotation::decide(&input, trigger) {
            (Decision::Switch { account, .. }, _) => Ok(Some(Placed {
                account: self
                    .deps
                    .store
                    .account(&account)
                    .await?
                    .ok_or(AppError::NotFound)?,
                // The same snapshot decided the account and says the model.
                fallback: crate::capacity::fallback_of(task.model.as_deref())
                    .filter(|_| rotation::placing_family(&input) != input.family),
            })),
            (Decision::Wait { .. }, _) => Ok(None),
            (Decision::Stay, reason) => {
                let label = match requested {
                    Some(id) => self.deps.store.account(id).await?.map(|a| a.label),
                    None => None,
                };
                Err(AppError::Capacity(refused_choice(
                    label.as_deref(),
                    task.harness.provider(),
                    &reason,
                )))
            }
        }
    }

    /// Attaches to a run's host, resets the hub and starts the output pump and transcript watcher.
    async fn attach_run(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        host_token: &[u8; 32],
    ) -> AppResult<()> {
        let runtime = self.runtime(task_id);
        let sandbox = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?
            .sandbox_id;
        // The terminal people see is redacted of agent secrets (SPEC-002 ADR-20).
        let scrub = crate::vault::TerminalScrub::default();
        if let Ok(mut shown) = runtime.scrub.lock() {
            *shown = scrub.clone();
        }
        runtime.screen_flagged.store(false, Ordering::SeqCst);
        let attached = self
            .deps
            .session_host
            .attach(Path::new(&attempt.run_dir), host_token)
            .await?;
        let welcome = attached.welcome.clone();
        let screen = self
            .terminal_snapshot(
                &scrub,
                &sandbox,
                welcome.snapshot.as_deref().unwrap_or_default(),
            )
            .await;
        // The run takes input before anything of it shows: a viewer who sees the agent's screen
        // can type into it (F 2026-10-13: input right after the banner was refused as "not
        // running" on a fast machine).
        let codex = attempt.harness == Harness::Codex;
        let codex_input = codex.then(|| CodexInput::load(&attempt.run_dir));
        let repaint_size = codex_input.as_ref().and_then(|input| input.repaint_size);
        *runtime.run.lock().await = Some(ActiveRun {
            attempt_id: attempt.id.clone(),
            codex_input,
            writer: attached.writer,
        });
        runtime.retain_park_for_attached_run(&attempt.id);
        crate::race::reached(crate::race::Point::RunAttached, Path::new(task_id.as_str()));
        // What the sandbox's limits counted so far: a rise by the run's end says it was at one.
        let counters = self.limit_counters(&sandbox).await;
        runtime.set_start_counters(&attempt.id, counters);
        // A stop or close that came while the run was launching found no run to ask: it asks now,
        // so the run ends as asked instead of at the escalation (SPEC-140 STP-001, #464). One that
        // comes later finds the run itself; the host takes a second request as the first.
        let ending = runtime.intent.lock().is_ok_and(|intent| {
            matches!(
                *intent,
                Some(EndIntent::Cancel | EndIntent::Close | EndIntent::Park)
            )
        });
        if ending {
            // The stop may have come before this run existed, so it armed no escalation for it.
            self.arm_escalation(task_id, Some(attempt.id.clone()));
            self.end_person_pause(task_id).await;
            request_shutdown(&runtime, None).await;
        }
        // A viewer who took the size while the run was starting (the PTY spawned at the size before
        // it): the screen shows that viewer's size from the start, never the spawn size first,
        // and the PTY follows (TSK-006, F 2026-10-10).
        let (cols, rows) = runtime
            .hub
            .controller_size()
            .or(repaint_size)
            .unwrap_or((welcome.cols, welcome.rows));
        // The hub shows this run from now on (SPEC-301 LRS-001).
        runtime.show_screen_of(&attempt.id, |hub| {
            hub.restore(
                welcome.seq,
                &screen,
                cols,
                rows,
                crate::hub::ScreenUpdate::Host {
                    rendered: welcome.rendered_at((cols, rows)),
                },
            );
        });
        if ((cols, rows) != (welcome.cols, welcome.rows)
            || repaint_size.is_some_and(|size| size != (cols, rows)))
            && let Err(err) = self.resize_run(task_id, cols, rows).await
        {
            tracing::debug!(task = %task_id, error = %err, "viewer size after attach failed");
        }
        let app = self.clone();
        let tid = task_id.clone();
        let aid = attempt.id.clone();
        let token = *host_token;
        let mut reader = attached.reader;
        tokio::spawn(async move {
            let mut reattaches = 0;
            loop {
                match reader.next().await {
                    Ok(Some(RunEvent::Output { seq, bytes })) => {
                        app.terminal_output(&scrub, (&tid, &aid), &sandbox, seq, &bytes)
                            .await;
                    }
                    Ok(Some(RunEvent::Snapshot { seq, bytes })) => {
                        let (cols, rows) = app.runtime(&tid).hub.size();
                        let screen = app.terminal_snapshot(&scrub, &sandbox, &bytes).await;
                        app.runtime(&tid)
                            .on_screen_of(&aid, |hub| hub.reset(seq, &screen, cols, rows));
                    }
                    Ok(Some(RunEvent::Exit(exit))) => {
                        crate::race::reached(
                            crate::race::Point::HostExitReceived,
                            Path::new(aid.as_str()),
                        );
                        app.finalize_attempt(&tid, &aid, exit).await;
                        crate::race::reached(
                            crate::race::Point::HostExitFinalized,
                            Path::new(aid.as_str()),
                        );
                        return;
                    }
                    Ok(None) | Err(_) => {
                        // Connection lost without an exit: reattach (the host may have dropped a slow client).
                        if reattaches < REATTACH_ATTEMPTS {
                            reattaches += 1;
                            tokio::time::sleep(Duration::from_millis(200 * u64::from(reattaches)))
                                .await;
                            if let Ok(Some(run_dir)) = app
                                .deps
                                .store
                                .attempt(&aid)
                                .await
                                .map(|a| a.map(|a| a.run_dir))
                                && let Ok(mut again) = app
                                    .deps
                                    .session_host
                                    .attach(Path::new(&run_dir), &token)
                                    .await
                            {
                                {
                                    let screen = app
                                        .terminal_snapshot(
                                            &scrub,
                                            &sandbox,
                                            again.welcome.snapshot.as_deref().unwrap_or_default(),
                                        )
                                        .await;
                                    let rt = app.runtime(&tid);
                                    let mut run = rt.run.lock().await;
                                    // The host may have disconnected at the temporary
                                    // repaint width. The writer lock covers the whole probe,
                                    // so the hub now holds its restored (or latest viewer) size.
                                    let host_size = (again.welcome.cols, again.welcome.rows);
                                    let size = if codex { rt.hub.size() } else { host_size };
                                    if size != host_size
                                        && let Err(err) = again.writer.resize(size.0, size.1).await
                                    {
                                        tracing::warn!(attempt = %aid, error = %err, "could not restore PTY size on reconnect");
                                        continue;
                                    }
                                    let codex_input = run
                                        .as_mut()
                                        .filter(|run| run.attempt_id == aid)
                                        .and_then(|run| run.codex_input.take())
                                        .or_else(|| codex.then(|| CodexInput::load(&run_dir)));
                                    *run = Some(ActiveRun {
                                        attempt_id: aid.clone(),
                                        codex_input,
                                        writer: again.writer,
                                    });
                                    rt.on_screen_of(&aid, |hub| {
                                        hub.restore(
                                            again.welcome.seq,
                                            &screen,
                                            size.0,
                                            size.1,
                                            crate::hub::ScreenUpdate::Host {
                                                rendered: again.welcome.rendered_at(size),
                                            },
                                        );
                                    });
                                    reader = again.reader;
                                    continue;
                                }
                            }
                            continue;
                        }
                        app.resolve_lost_connection(&tid, &aid).await;
                        return;
                    }
                }
            }
        });
        self.start_watcher(task_id, attempt);
        Ok(())
    }

    /// Writes a running task's `activity`, as its session's work or turns decide it, unless the
    /// task reads the flag its live run raised when its host could not be terminated ("Yulius
    /// could not end this session"): no work of the session ends that cause, so the flag stays
    /// until a person stops the task or the run ends (SPEC-635 EFL-001). Once something else
    /// replaced that flag (a pause, another matter's), the session's activity rules again
    /// (EFL-002). Whether it wrote.
    ///
    /// # Errors
    /// The store's.
    pub(crate) async fn write_session_activity(
        &self,
        task_id: &TaskId,
        activity: Activity,
    ) -> AppResult<bool> {
        let runtime = self.runtime(task_id);
        let _gate = runtime.end_gate.lock().await;
        let failed = runtime
            .end_failed
            .lock()
            .ok()
            .and_then(|failed| failed.clone());
        if let Some((run, _)) = failed {
            let store = &self.deps.store;
            if store
                .live_attempt(task_id)
                .await?
                .is_none_or(|live| live.id != run)
            {
                // That run is over: its failure holds nothing any more.
                if let Ok(mut failed) = runtime.end_failed.lock()
                    && failed.as_ref().is_some_and(|(id, _)| *id == run)
                {
                    *failed = None;
                }
            } else {
                let held = runtime.end_flag_pending.load(Ordering::SeqCst) > 0
                    || store
                        .task_attention(task_id)
                        .await?
                        .is_some_and(|(reason, _)| is_could_not_end(&reason));
                crate::race::reached(
                    crate::race::Point::SessionActivityChecked,
                    Path::new(task_id.as_str()),
                );
                if held {
                    tracing::info!(task = %task_id, ?activity, "the run cannot be ended: its flag stays");
                    return Ok(false);
                }
            }
        }
        self.deps
            .store
            .set_activity(task_id, Some(activity), self.now())
            .await?;
        Ok(true)
    }

    /// Closes a finished, running task (TSK-014): its session is shut down gracefully and the
    /// task ends `closed`, not stopped. It stays Running until the session is gone.
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn close_task_now(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        self.close_task_selected(task_id, None).await
    }

    pub(crate) async fn close_task_selected(
        self: &Arc<Self>,
        task_id: &TaskId,
        achieved: Option<(&TaskRecord, Option<&crate::delivery::DeliveryRecord>)>,
    ) -> AppResult<()> {
        let task = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if task.status == TaskStatus::Parked {
            return self.close_parked(&task).await;
        }
        if task.status != TaskStatus::Running {
            return Err(AppError::Conflict(format!(
                "task is {}",
                task.status.as_str()
            )));
        }
        crate::race::reached(
            crate::race::Point::CloseDecided,
            Path::new(task_id.as_str()),
        );
        let closed = self.live_attempt_id(task_id).await;
        let runtime = self.runtime(task_id);
        let delivery = match achieved {
            Some((selected, record)) => Some(
                self.hold_live_delivery(selected, record)
                    .await
                    .ok_or_else(|| {
                        AppError::Conflict(
                            "The task’s delivery is still being verified or has just changed; \
                             try again shortly, or stop the task first."
                                .into(),
                        )
                    })?,
            ),
            None => None,
        };
        if let Ok(mut intent) = runtime.intent.lock() {
            replace_intent(&mut intent, Some(EndIntent::Close), SwitchEnd::Closed);
        }
        drop(delivery);
        self.arm_escalation(task_id, closed);
        self.end_person_pause(task_id).await;
        request_shutdown(&runtime, None).await;
        Ok(())
    }

    /// Closes a parked task (SPEC-512 PRK-001): it has no session to end, so it is `closed` at
    /// once, as a close leaves a running one, and tasks chained after it learn it.
    ///
    /// # Errors
    /// `Conflict` when it is no longer parked.
    async fn close_parked(self: &Arc<Self>, task: &TaskRecord) -> AppResult<()> {
        let closed = TaskStatus::Parked
            .apply(TaskEvent::CloseRequested)
            .map_err(|err| AppError::Conflict(err.to_string()))?;
        if !self
            .deps
            .store
            .transition_task(&task.id, Some(TaskStatus::Parked), closed, None, self.now())
            .await?
        {
            return Err(AppError::Conflict(
                "the task changed meanwhile; try again".into(),
            ));
        }
        self.publish_task(&task.id).await;
        self.advance_chains(&task.id).await;
        Ok(())
    }

    /// Closes a finished, running task like [`Self::close_task_now`] and waits until its run has
    /// ended, the task rests and the run's ending is fully recorded (SPEC-039, SPEC-065 END-003),
    /// so its records and directories can go. A run that already ended by itself is not a
    /// conflict. The close and the wait are bounded together by the close escalation, after which
    /// the run's host is terminated, plus `CLOSE_SETTLE_MS`; the wait follows the task's update
    /// events and looks again every `CLOSE_RECHECK`, on an interval other events do not reset.
    ///
    /// # Errors
    /// `Conflict` when the task was not closed and is neither resting nor stopping, when its
    /// run's host could not be terminated, or when the run is still there at the bound.
    pub async fn close_task_and_wait(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        self.close_selected_and_wait(task_id, None).await
    }

    pub(crate) async fn close_selected_and_wait(
        self: &Arc<Self>,
        task_id: &TaskId,
        achieved: Option<(&TaskRecord, Option<&crate::delivery::DeliveryRecord>)>,
    ) -> AppResult<()> {
        // Subscribed before closing, so the update that says the run ended is never missed.
        let mut events = self.events.subscribe();
        let runtime = self.runtime(task_id);
        let mut finalizing = runtime.finalizing.subscribe();
        let bound = Duration::from_millis(self.config.stop_escalation_ms + CLOSE_SETTLE_MS);
        let ended = tokio::time::timeout(bound, async {
            match self.close_task_selected(task_id, achieved).await {
                Ok(()) => {}
                // Not running any more: a task that rests or is being stopped can go once its
                // run has ended; any other (switching, starting) was not closed.
                Err(AppError::Conflict(why)) => {
                    let status = self.deps.store.task(task_id).await?.map(|t| t.status);
                    // Still running: the owner's delivery could not be held; say so.
                    if achieved.is_some() && status == Some(TaskStatus::Running) {
                        return Err(AppError::Conflict(why));
                    }
                    // Archive and delete both close this way: the message names neither.
                    if status.is_some_and(|s| !s.is_resting() && s != TaskStatus::Stopping) {
                        return Err(AppError::Conflict(
                            "the task is changing state; stop the task first".into(),
                        ));
                    }
                }
                Err(err) => return Err(err),
            }
            let mut recheck = tokio::time::interval_at(
                tokio::time::Instant::now() + CLOSE_RECHECK,
                CLOSE_RECHECK,
            );
            loop {
                let store = &self.deps.store;
                let rests = store
                    .task(task_id)
                    .await?
                    .is_none_or(|t| t.status.is_resting());
                let live = store.live_attempt(task_id).await?;
                if let Some(why) = live.as_ref().and_then(|a| runtime.end_failed_for(&a.id)) {
                    return Err(AppError::Conflict(format!(
                        "the task's session could not be ended: {why}"
                    )));
                }
                if rests && live.is_none() && *finalizing.borrow_and_update() == 0 {
                    return Ok::<bool, AppError>(true);
                }
                loop {
                    tokio::select! {
                        _ = recheck.tick() => break,
                        _ = finalizing.changed() => break,
                        event = events.recv() => match event {
                            Ok(envelope) => {
                                if matches!(&envelope.event, UiEvent::TaskUpdated { task }
                                    if task.id == task_id.as_str())
                                {
                                    break;
                                }
                            }
                            // Updates were missed: look again.
                            Err(broadcast::error::RecvError::Lagged(_)) => break,
                            Err(broadcast::error::RecvError::Closed) => return Ok(false),
                        },
                    }
                }
            }
        })
        .await;
        match ended {
            Ok(Ok(true)) => Ok(()),
            Ok(Err(err)) => Err(err),
            Ok(Ok(false)) | Err(_) => Err(AppError::Conflict(
                "the task's session is still closing; try again".into(),
            )),
        }
    }

    /// The task's live attempt, if any; a store error is logged and means none is known.
    async fn live_attempt_id(&self, task_id: &TaskId) -> Option<AttemptId> {
        match self.deps.store.live_attempt(task_id).await {
            Ok(attempt) => attempt.map(|a| a.id),
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "the ended run is unknown; no escalation");
                None
            }
        }
    }

    /// Arms the escalation of a stop or close: if `attempt` is still the task's live run after
    /// `stop_escalation_ms`, its host is terminated (SPEC-065 END-001). Armed before the run lock
    /// is taken, so neither a blocked writer nor a cancelled caller leaves the run without it
    /// (END-005). Only that run: a task continued meanwhile has a new one.
    fn arm_escalation(self: &Arc<Self>, task_id: &TaskId, attempt: Option<AttemptId>) {
        let Some(attempt) = attempt else {
            return;
        };
        if let Ok(mut failed) = self.runtime(task_id).end_failed.lock() {
            *failed = None;
        }
        let app = self.clone();
        let tid = task_id.clone();
        let wait = Duration::from_millis(self.config.stop_escalation_ms);
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Ok(Some(live)) = app.deps.store.live_attempt(&tid).await
                && live.id == attempt
            {
                crate::race::reached(crate::race::Point::EndEscalating, Path::new(tid.as_str()));
                tracing::warn!(task = %tid, "run did not exit after escalation; terminating host");
                app.terminate_then_mark_lost(&tid, &live, StopReason::Lost)
                    .await;
            }
        });
    }

    /// Stops a task: graceful shutdown with escalation, then (if needed) termination and `lost`.
    /// A status change that loses to a concurrent one (a switch or a start completing) is made
    /// again from the status read anew, so the stop always takes effect (SPEC-250 STW-001).
    ///
    /// # Errors
    /// `Conflict` when the task is not live, or still changes after `STOP_ROUNDS` rounds.
    pub async fn stop_task_now(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        for round in 0..STOP_ROUNDS {
            if self.stop_round(task_id, round > 0).await? {
                return Ok(());
            }
        }
        // The stop failed: its intent goes, so no run ends later as if stopped.
        if let Ok(mut intent) = self.runtime(task_id).intent.lock()
            && matches!(*intent, Some(EndIntent::Cancel))
        {
            *intent = None;
        }
        Err(AppError::Conflict(
            "the task kept changing while it was stopped; try again".into(),
        ))
    }

    /// One round of [`Self::stop_task_now`]: `false` when its status change lost to a concurrent
    /// one. After a lost round (`again`), a task that rests was ended meanwhile, by this stop's
    /// intent or by its run's own end: the stop is done (STW-002).
    async fn stop_round(self: &Arc<Self>, task_id: &TaskId, again: bool) -> AppResult<bool> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        if again && task.status.is_resting() {
            // A run still live is ended all the same: the intent set is this stop's.
            if store.live_attempt(task_id).await?.is_some() {
                self.end_stopped_run(task_id).await;
            }
            return Ok(true);
        }
        // A resting task with a run still on record (SPEC-458 RUN-003, #439): the stop ends that
        // run instead of answering "task is …" and leaving it. One no server holds is settled as
        // after a restart (ADR-0050).
        if task.status.is_resting()
            && let Some(run) = store.live_attempt(task_id).await?
        {
            if !self.holds_run(task_id).await {
                self.settle_unheld_run(task_id, &run.id).await?;
                return Ok(true);
            }
            if let Ok(mut intent) = self.runtime(task_id).intent.lock() {
                replace_intent(&mut intent, Some(EndIntent::Cancel), SwitchEnd::Stopped);
            }
            self.end_stopped_run(task_id).await;
            return Ok(true);
        }
        // A parked task has no run either (SPEC-512 PRK-001).
        if task.status == TaskStatus::Queued
            || task.status == TaskStatus::Chained
            || task.status == TaskStatus::Prepared
            || task.status == TaskStatus::Parked
            || task.status == TaskStatus::WaitingCapacity
                && store.live_attempt(task_id).await?.is_none()
        {
            if !store
                .transition_task(
                    task_id,
                    Some(task.status),
                    TaskStatus::Stopped(StopReason::Cancelled),
                    None,
                    self.now(),
                )
                .await?
            {
                return Ok(false);
            }
            self.end_pause(task_id, "cancelled").await;
            self.publish_task(task_id).await;
            self.advance_chains(task_id).await;
            // A cancelled supertask no longer holds its project (SUP-005): the queue may go on.
            // Any other task that waited frees no session, so it starts nothing (a pass here
            // would start the next waiting task of a bulk stop before the stop reaches it).
            if task.supertask {
                self.drain_queue().await;
            }
            return Ok(true);
        }
        // A task already stopping is stopped again: its escalation is armed anew (SPEC-065). One
        // whose run has ended gets no intent (nothing would consume it): it rests by itself while
        // its ending is recorded, or now when nothing records it (records cut by a crash).
        let next = if task.status == TaskStatus::Stopping {
            if store.live_attempt(task_id).await?.is_none() {
                if *self.runtime(task_id).finalizing.borrow() == 0
                    && store
                        .transition_task(
                            task_id,
                            Some(TaskStatus::Stopping),
                            TaskStatus::Stopped(StopReason::Cancelled),
                            None,
                            self.now(),
                        )
                        .await?
                {
                    // An earlier stop's intent that no run consumed goes with it (STP-002).
                    if let Ok(mut intent) = self.runtime(task_id).intent.lock() {
                        *intent = None;
                    }
                    self.end_pause(task_id, "cancelled").await;
                    self.publish_task(task_id).await;
                    self.advance_chains(task_id).await;
                }
                return Ok(true);
            }
            TaskStatus::Stopping
        } else {
            task.status
                .apply(TaskEvent::CancelRequested)
                .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?
        };
        let runtime = self.runtime(task_id);
        if let Ok(mut intent) = runtime.intent.lock() {
            replace_intent(&mut intent, Some(EndIntent::Cancel), SwitchEnd::Stopped);
        }
        crate::race::reached(crate::race::Point::StopDecided, Path::new(task_id.as_str()));
        // Lost to a concurrent change: the task is read again. The intent stays: it is this
        // stop's, and a run that ends on it meanwhile rests its task (STW-003).
        if task.status != TaskStatus::Stopping
            && !store
                .transition_task(task_id, Some(task.status), next, None, self.now())
                .await?
        {
            return Ok(false);
        }
        self.end_pause(task_id, "cancelled").await;
        self.publish_task(task_id).await;
        self.end_stopped_run(task_id).await;
        Ok(true)
    }

    /// Ends the run of a task a stop decided to end: graceful shutdown, with the escalation armed.
    async fn end_stopped_run(self: &Arc<Self>, task_id: &TaskId) {
        // The run this stop ends; without it (a store error) nothing is escalated.
        let stopped = self.live_attempt_id(task_id).await;
        self.arm_escalation(task_id, stopped);
        self.end_person_pause(task_id).await;
        request_shutdown(&self.runtime(task_id), None).await;
    }

    /// Writes viewer keyboard input to the run.
    ///
    /// # Errors
    /// `Conflict` when no run is attached.
    pub async fn send_input(&self, task_id: &TaskId, bytes: &[u8]) -> AppResult<()> {
        self.send_session_input(task_id, SessionInput::Viewer(bytes))
            .await
    }

    /// Writes input through the attempt's writer, with a screen guard for automation.
    pub(crate) async fn send_session_input(
        &self,
        task_id: &TaskId,
        input: SessionInput<'_>,
    ) -> AppResult<()> {
        let runtime = self
            .existing_runtime(task_id)
            .ok_or_else(|| AppError::Conflict("the task is not running".into()))?;
        let now = self.now();
        if matches!(input, SessionInput::Viewer(_)) {
            runtime.last_input_ms.store(now.0, Ordering::SeqCst);
        }
        let mut run = runtime.run.lock().await;
        let run = run
            .as_mut()
            .ok_or_else(|| AppError::Conflict("the task is not running".into()))?;
        runtime.write_session_input(run, now, input).await
    }

    /// Resizes the run's PTY.
    ///
    /// # Errors
    /// `Conflict` when no run is attached.
    pub async fn resize_run(&self, task_id: &TaskId, cols: u16, rows: u16) -> AppResult<()> {
        let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(5, 1000));
        let runtime = self.runtime(task_id);
        let mut run = runtime.run.lock().await;
        if let Some(input) = run.as_mut().and_then(|run| run.codex_input.as_mut()) {
            input.follow_viewer((cols, rows));
        }
        runtime.hub.resize(cols, rows);
        crate::race::reached(
            crate::race::Point::TerminalResizeOwnsWriter,
            Path::new(task_id.as_str()),
        );
        match run.as_mut() {
            Some(run) => run.writer.resize(cols, rows).await,
            None => Ok(()),
        }
    }

    /// Account usage update from transcripts or providers. The subscription reads `limited`
    /// while the default model cannot run on it: a limit Claude reported is in effect, or a
    /// window that binds that model is used up (SPEC-464).
    pub(crate) async fn record_account_usage(&self, account: &AccountId, usage: &AccountUsage) {
        let now = self.now();
        // A per-model window whose reset passed says nothing about the one that began.
        let usage = &usage.without_reset_model_windows(now);
        let family = crate::capacity::default_family();
        // A reading never makes a subscription that is not healthy usable again: a cancelled
        // one can still report usage (or a late transcript line can), and only a sign-in proves
        // it works (SPEC-483 SGN-002).
        let needs_sign_in = match self.deps.store.account(account).await {
            Ok(Some(record)) if !record.status.healthy() => {
                Some((record.status, record.last_error))
            }
            _ => None,
        };
        crate::race::reached(
            crate::race::Point::UsageStatusRead,
            Path::new(account.as_str()),
        );
        // Nor does it erase why it needs one (SPEC-447 MOV-005).
        let kept_error = needs_sign_in.as_ref().and_then(|(_, error)| error.clone());
        let status = if let Some((status, _)) = needs_sign_in {
            status
        } else if usage.exhausted_for(now, FULL_PERCENT, family) {
            AccountStatus::Limited
        } else {
            AccountStatus::Ok
        };
        let json = usage_json::to_json(usage);
        let _ = self
            .deps
            .store
            .set_account_state(account, status, Some(&json), kept_error.as_deref(), now)
            .await;
        // New usage may let paused tasks resume, or pause the pool (ROT-010).
        self.capacity_changed();
        // At or above the threshold: tasks on this account rotate at their next turn boundary
        // (ROT-003); with hooks the Stop hook decides this itself.
        if usage.exhausted(now, rotation::THRESHOLD)
            && let Ok(live) = self.deps.store.live_attempts().await
        {
            for attempt in live
                .iter()
                .filter(|a| a.account_id.as_ref() == Some(account))
            {
                self.runtime(&attempt.task_id)
                    .proactive_switch
                    .store(true, Ordering::SeqCst);
            }
        }
    }

    /// Marks a refused sign-in while it is still current. The label is for the interrupted
    /// task's move notice, including when a newer sign-in is already available.
    async fn record_access_lost(
        &self,
        account: &AccountId,
        run_dir: &Path,
        message: &str,
    ) -> Option<String> {
        crate::race::reached(
            crate::race::Point::AccessLostRecord,
            Path::new(account.as_str()),
        );
        let launched =
            crate::leftovers::read_small_file(&run_dir.join(CLAUDE_CREDENTIAL_DIGEST), 64);
        // Claude sign-in takes this lock too. Keep it from the credential check through the
        // status write, which must also follow any usage write already under way (#1211).
        let _writing = self.capacity.usage_writes.lock().await;
        let record = match self.deps.store.account(account).await {
            Ok(account) => account?,
            Err(err) => {
                tracing::error!(error = %err, "could not read a refused subscription");
                return None;
            }
        };
        if let (Some(launched), Some(current)) = (launched, &record.credential)
            && launched == self.deps.crypto.sha256_hex(&current.0)
        {
            if let Err(err) = self
                .deps
                .store
                .set_account_state(
                    account,
                    AccountStatus::Relogin,
                    None,
                    Some(message),
                    self.now(),
                )
                .await
            {
                tracing::error!(error = %err, "could not mark a refused subscription");
            }
            self.capacity_changed();
        }
        Some(record.label)
    }

    /// Install the conversation's goal before a resumed process can send its first Stop.
    /// Earlier goal records describe the condition, never a verdict on the new turn.
    async fn initialize_run_goal(
        &self,
        task: &TaskRecord,
        attempt: &AttemptRecord,
        unfinished: bool,
    ) {
        let mut goal = None;
        if task.mode == TaskMode::Prompt
            && attempt.harness == Harness::ClaudeCode
            && attempt.launch_kind == LaunchKind::Resume
            && let (Some(session), Some(worktree)) =
                (&attempt.provider_session_id, &task.worktree_path)
            && yulius_domain::harness::valid_session_id(session)
        {
            let harness = Arc::clone(&self.deps.harness);
            let path = harness.claude_transcript(
                &self.sandbox_home(&task.sandbox_id),
                Path::new(worktree),
                session,
            );
            let anchor = self.sandbox_dir(&task.sandbox_id);
            match tokio::task::spawn_blocking(move || harness.last_claude_goal(&anchor, &path))
                .await
            {
                Ok(Ok(outcome)) => goal = outcome,
                Ok(Err(err)) => {
                    tracing::warn!(task = %task.id, error = %err, "could not read the resumed goal")
                }
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "resumed goal read did not finish")
                }
            }
        }
        let runtime = self.runtime(&task.id);
        // A verdict either commits before the new run installs its state, or sees the new owner.
        let _order = runtime.turn_order.lock().await;
        runtime
            .replace_goal_unfinished(goal.map_or(unfinished, crate::finish::goal_unfinished))
            .await;
        runtime.goal_open.store(
            goal.is_some_and(crate::finish::goal_stays_open),
            Ordering::SeqCst,
        );
        runtime.goal_achieved.store(false, Ordering::SeqCst);
        runtime.goal_submitted.store(false, Ordering::SeqCst);
        runtime.verdict_stop_ms.clear();
    }

    fn start_watcher(self: &Arc<Self>, task_id: &TaskId, attempt: &AttemptRecord) {
        let runtime = self.runtime(task_id);
        runtime.watcher_alive.store(true, Ordering::SeqCst);
        runtime.hooks_seen.store(false, Ordering::SeqCst);
        runtime.at_limit.store(false, Ordering::SeqCst);
        // Unmarked before it is emptied: a run is never taken as read with an empty snapshot.
        if let Ok(mut read) = runtime.usage_read.lock() {
            *read = None;
        }
        if let Ok(mut u) = runtime.usage.lock() {
            *u = UsageSnapshot::default();
        }
        runtime.turn_started_ms.store(0, Ordering::SeqCst);
        runtime.background_shells.store(-1, Ordering::SeqCst);
        runtime.background_waits.store(-1, Ordering::SeqCst);
        // `active_waits` is RAII-balanced by `ActiveWaitGuard` per live `/internal/agent/wait`
        // handler; resetting it here while an old attempt's long-poll is still dropping could
        // desynchronize or underflow the counter (SPEC-580 review F1).
        runtime.background_agents.store(0, Ordering::SeqCst);
        runtime.notice_attention_ms.store(0, Ordering::SeqCst);
        runtime.turn_record_ms.store(0, Ordering::SeqCst);
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        // A retry armed in the old session is that process's: a new one does not make it.
        runtime.goal_retry_armed.store(false, Ordering::SeqCst);
        runtime.stop_answered_ms.store(0, Ordering::SeqCst);
        runtime.turn_began_ms.store(0, Ordering::SeqCst);
        runtime.turn_record_at_ms.store(0, Ordering::SeqCst);
        runtime.turn_closed_ms.store(0, Ordering::SeqCst);
        runtime.turn_moved_ms.store(0, Ordering::SeqCst);
        // A new session runs none of the old one's monitors or agents.
        if let Ok(mut launches) = runtime.launches.lock() {
            launches.clear();
        }
        if let Ok(mut end) = runtime.session_end.lock() {
            *end = None;
        }
        runtime.hold_polled_ms.store(0, Ordering::SeqCst);
        // A new session has its own bridge, heard of afresh (SPEC-432 review round 1).
        runtime.mcp_seen_ms.store(0, Ordering::SeqCst);
        runtime.mcp_pinged_ms.store(0, Ordering::SeqCst);
        if let Ok(mut reconnects) = runtime.mcp_reconnects.lock() {
            *reconnects = crate::mcp_watch::Reconnects::default();
        }
        let app = self.clone();
        let tid = task_id.clone();
        let attempt = attempt.clone();
        let alive = runtime.watcher_alive.clone();
        let generation = runtime.watcher_generation.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::spawn(async move { app.watch_loop(tid, attempt, alive, generation).await });
    }

    async fn watch_loop(
        self: Arc<Self>,
        task_id: TaskId,
        attempt: AttemptRecord,
        alive: Arc<AtomicBool>,
        generation: u64,
    ) {
        let Ok(Some(task)) = self.deps.store.task(&task_id).await else {
            return;
        };
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return;
        };
        let Some(worktree) = task.worktree_path.clone().map(PathBuf::from) else {
            return;
        };
        // A request for a goal pause that is still open (made before this server started) is
        // answered by this run's work, like one made while it is watched (SPEC-469 GPW-004).
        self.watch_goal_pause_request(&task_id).await;
        // So is a flag that the session's work answers (a notice's), raised before this server
        // started: its reason says so (SPEC-472 ATN-010).
        self.watch_answerable_flag(&task_id).await;
        // The launched model stays the same for this attempt. For older run records, the
        // task already read for this watch supplies the model. Polling needs no store read.
        let family = crate::capacity::run_family(
            attempt.model.as_deref(),
            crate::capacity::task_model(&task),
        );
        let mut session_id = attempt.provider_session_id.clone();
        let mut watch = None;
        let mut last_drift_check = Timestamp(0);
        let mut spec_synced = std::time::Instant::now();
        let mut saved: Option<UsageSnapshot> = None;
        let mut saved_at = Timestamp(0);
        // The Claude Code version the run's lines name, as last recorded (SPEC-461 CCM-011).
        let mut cli_version: Option<String> = None;
        // After a restart, the transcript the reattached run already wrote is history until the
        // first poll reaches its end: it is reduced to its final state, not replayed event by
        // event (TSK-014; a replayed prompt used to put a finished task back to Working after
        // every restart). A run started by this server is watched live from its first line.
        let mut replay = self
            .runtime(&task_id)
            .replay_transcript
            .swap(false, Ordering::SeqCst);
        let mut catching_up = false;
        let mut facts = TranscriptFacts::default();
        // The replay's last usage limit, acted on once caught up if no turn came after it.
        let mut replayed_limit = None;
        let mut probed_at = Timestamp(0);
        // `existing_runtime`: a deleted task's runtime is not brought back.
        let current = || {
            alive.load(Ordering::SeqCst)
                && self.existing_runtime(&task_id).is_some_and(|runtime| {
                    runtime.watcher_generation.load(Ordering::SeqCst) == generation
                })
        };
        while current() {
            tokio::time::sleep(Duration::from_millis(WATCH_INTERVAL_MS)).await;
            if !current() {
                break;
            }
            // A round begins: once the next one begins, every line written before this one was
            // read and applied (a Stop hook deciding a switch waits for that, BGW-004).
            self.runtime(&task_id)
                .watch_polls
                .fetch_add(1, Ordering::SeqCst);
            let now = self.now();
            // Claude session-id drift (/clear, /resume inside the TUI): the TUI's SessionStart
            // hook names its new session on the attempt; adopt it into the lineage. Never the
            // newest transcript of the worktree: a `claude` the agent runs there writes one too
            // (SPEC-281 SSN-002).
            // Codex names its session itself: its SessionStart hook names it the same way, and
            // without hooks the rollout started for the worktree since the run began does
            // (SPEC-503 CDX-003).
            if now.millis_since(last_drift_check) > 5_000 {
                last_drift_check = now;
                let mut named = self
                    .deps
                    .store
                    .attempt(&attempt.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|named| named.provider_session_id);
                // The rollouts are read only while no hook has named the session yet.
                if named.is_none()
                    && session_id.is_none()
                    && attempt.harness == Harness::Codex
                    && let Ok(Some((id, _))) = self.deps.harness.discover_codex_session(
                        &self.codex_sessions(&sandbox.id),
                        &worktree,
                        attempt.started_at,
                    )
                {
                    let _ = self.deps.store.set_attempt_session(&attempt.id, &id).await;
                    named = Some(id);
                }
                // An Antigravity run whose SessionStart named nothing: its CLI's last
                // conversation of the worktree, written since the run began (SPEC-505 AGY-005).
                if named.is_none()
                    && session_id.is_none()
                    && attempt.harness == Harness::Antigravity
                    && let Some(id) = self.deps.harness.discover_antigravity_session(
                        &antigravity_run_home(Path::new(&attempt.run_dir)).join(".gemini"),
                        &self.antigravity_brain(&sandbox.id),
                        &worktree,
                        attempt.started_at,
                    )
                {
                    let _ = self.deps.store.set_attempt_session(&attempt.id, &id).await;
                    named = Some(id);
                }
                if let Some(id) = named
                    && session_id.as_deref() != Some(id.as_str())
                {
                    let _ = self
                        .record_lineage(&task, attempt.harness, &id, "native")
                        .await;
                    // `/clear` or `/resume` in the TUI: a person took the session in hand.
                    if session_id.is_some() {
                        self.context_session_changed(&task_id);
                    }
                    session_id = Some(id);
                    watch = None;
                }
            }
            if watch.is_none()
                && let Some(id) = &session_id
                && let path = self.transcript_path(&sandbox, &worktree, attempt.harness, id)
                && path.exists()
            {
                // A resumed session's transcript holds its earlier runs' lines: the run reads its
                // own, from its start, whether it is watched from its launch or reattached after a
                // restart (SPEC-087 MET-002). Reading from the end when the watch opened lost the
                // lines written before it, and after a restart all of the run's figures.
                let from = match attempt.launch_kind {
                    LaunchKind::New => WatchFrom::Start,
                    LaunchKind::Resume => WatchFrom::Since(attempt.started_at),
                };
                watch = self.deps.harness.watch(attempt.harness, &path, from).ok();
                catching_up = replay && watch.is_some();
                replay &= watch.is_none();
                facts = TranscriptFacts::default();
                replayed_limit = None;
            }
            // The steps follow the spec's Plan while a turn runs, not only at its end (SPEC-341
            // LSS-001): a long goal turn that grew its Plan showed the old list's progress.
            if spec_synced.elapsed() >= SPEC_SYNC_INTERVAL {
                spec_synced = std::time::Instant::now();
                if let Ok(Some(fresh)) = self.deps.store.task(&task_id).await
                    && let Err(err) = self.sync_steps_from_spec(&fresh).await
                {
                    tracing::debug!(error = %err, task = %task_id, "step sync from the spec failed");
                }
            }
            // Background shells of an idle task, at most every SHELL_PROBE_MS (TSK-014).
            if now.millis_since(probed_at) >= SHELL_PROBE_MS {
                probed_at = now;
                self.probe_idle_shells(&task_id).await;
            }
            let Some(w) = watch.as_mut() else { continue };
            let observations = match w.poll() {
                Ok(o) => o,
                Err(err) => {
                    tracing::debug!(error = %err, "transcript poll failed");
                    continue;
                }
            };
            // A line's time is stamped on the host's wall clock: read on Yulius's clock, so a
            // test clock that runs shifted orders it with the hooks (SPEC-636).
            let observations: Vec<Observation> = observations
                .into_iter()
                .map(|observation| observation.with_record_times(|at| self.deps.clock.of_wall(at)))
                .collect();
            // A workflow agent's limit of another model than the session runs is not the
            // session's (SPEC-486 WFL-002): it goes into the subscription's reading when it is
            // read live, and never into what is known of the session.
            let (other_models, observations): (Vec<_>, Vec<_>) = observations
                .into_iter()
                .partition(|observation| binds_another_model(observation, family));
            for limit in other_models {
                if let (false, Some(account), Some(limit)) =
                    (catching_up, &attempt.account_id, reported_limit(&limit))
                {
                    tracing::info!(task = %task_id, ?limit, "a workflow agent's model is at its limit");
                    self.record_limit(account, limit, family).await;
                }
            }
            for observation in &observations {
                facts.fold(observation);
            }
            let usage = w.usage();
            let runtime = self.runtime(&task_id);
            if let Ok(mut u) = runtime.usage.lock() {
                *u = usage.clone();
            }
            // Which Claude Code runs this session is kept as soon as a line names it: the task's
            // info shows it, and the turn's end moves a session on an older one (SPEC-461).
            if usage.cli_version.is_some() && usage.cli_version != cli_version {
                cli_version.clone_from(&usage.cli_version);
                if let Some(version) = &cli_version
                    && let Err(err) = self
                        .deps
                        .store
                        .set_run_cli_version(&attempt.id, &task_id, version, self.now())
                        .await
                {
                    tracing::debug!(error = %err, task = %task_id, "the run's Claude Code version was not recorded");
                }
            }
            if w.caught_up()
                && let Ok(mut read) = runtime.usage_read.lock()
            {
                *read = Some(attempt.id.clone());
            }
            let turn_ended = observations
                .iter()
                .any(|o| matches!(o, Observation::TurnComplete { .. }));
            if catching_up {
                // What a review round's reviewer ran on, whether a limit cut it off and its report
                // are facts to note however late (SPEC-493 REV-004): noting one again changes
                // nothing. A report written while Yulius was down is only here (SPEC-633 RRE-002).
                for observation in &observations {
                    if let Observation::SubagentAnswered { .. }
                    | Observation::SubagentCutOff { .. }
                    | Observation::SubagentReported { .. } = observation
                    {
                        self.on_observation(
                            &task_id,
                            &attempt,
                            observation.clone(),
                            ObservationSource::Replay,
                        )
                        .await;
                    }
                }
                // Turns, the goal and a usage limit are applied once, as the replay's final
                // state: a limit that a later turn overtook (Claude Code continued by itself) is
                // history (SPEC-072 LRS-001).
                if let Some(limit) = observations.into_iter().rfind(|o| {
                    matches!(
                        o,
                        Observation::LimitReached { .. }
                            | Observation::WorkflowLimitReached { .. }
                            | Observation::AccessLost { .. }
                    )
                }) {
                    replayed_limit = Some(limit);
                }
                if w.caught_up() {
                    catching_up = false;
                    if let Some(limit) = replayed_limit.take()
                        && facts.limit_pending
                    {
                        self.on_observation(&task_id, &attempt, limit, ObservationSource::Replay)
                            .await;
                        // The goal pause Claude Code wrote at that limit is the limit's too.
                        if let Some(notice) = &facts.limit_goal_pause {
                            self.goal_retry_observed(&task_id, false, notice).await;
                        }
                    } else if !facts.limit_pending {
                        // Claude Code continued past its limit while Yulius was down.
                        self.cut_turn_continued(&task_id, facts.last_turn_at).await;
                    }
                    self.apply_transcript_facts(&task_id, facts.clone()).await;
                    // A session left full while Yulius was down is recovered now (CTX-007).
                    if facts.context_full {
                        self.context_full_replayed(&task_id, &attempt).await;
                    }
                }
            } else {
                // A limit that cut off a subagent of the session's thread is the session's news
                // as a workflow agent's is (SPEC-493 RLM-001), read live only, and after the
                // thread's own lines of this read: a turn they start is under way by then, so a
                // move for a reviewer waits for its end (review round 1 of fix 7).
                let cut_offs: Vec<Observation> = observations
                    .iter()
                    .filter(|o| matches!(o, Observation::SubagentCutOff { .. }))
                    .cloned()
                    .collect();
                for observation in observations {
                    self.on_observation(&task_id, &attempt, observation, ObservationSource::Live)
                        .await;
                }
                for observation in &cut_offs {
                    self.subagent_limit(&task_id, &attempt, family, observation)
                        .await;
                }
            }
            // Live metrics (#31): the dashboard counts a run while it goes, saved after each turn
            // and at most every 30 s while something changes (a turn in progress grows its
            // active time with each line).
            let now = self.now();
            if saved.as_ref() != Some(&usage)
                && (turn_ended || now.millis_since(saved_at) >= LIVE_METRICS_MS)
            {
                self.save_metrics(&task_id, &attempt.id, &runtime).await;
                saved = Some(usage);
                saved_at = now;
            }
        }
    }

    async fn on_observation(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        observation: Observation,
        source: ObservationSource,
    ) {
        let runtime = self.runtime(task_id);
        // The model answered: a later limit nothing says the end of is no repeat of an earlier
        // one, and has its own retry (SPEC-464 UCL-010).
        if model_answered(&observation)
            && let Ok(mut blind) = runtime.blind_limits.lock()
        {
            blind.answered();
        }
        // A line of the model written after a hand-over is of the turn the hand-over started,
        // which a prompt Claude Code scheduled for itself begins with no prompt record read
        // here (SPEC-471 CCQ-003). Said to the queue once the line was applied below, as a
        // prompt record's turn is: such a prompt has had its hook by then, so the task reads
        // Working either way.
        let worked = match &observation {
            Observation::AgentWork { at } | Observation::TurnComplete { at } => *at,
            _ => None,
        };
        // A workflow launched, ended, or met by a limit while unfinished (SPEC-486 WFL-003).
        runtime.with_workflows(|workflows| workflows.observe(&observation));
        match observation {
            Observation::LimitReached {
                resets_at,
                message,
                window_minutes,
                scope,
                at,
            } => {
                tracing::info!(task = %task_id, %message, ?scope, "usage limit reached");
                let limit = ReportedLimit {
                    scope,
                    window_minutes,
                    resets_at,
                };
                self.limit_met(task_id, attempt, limit, at, source).await;
            }
            Observation::WorkflowLimitReached {
                resets_at,
                message,
                window_minutes,
                scope,
                at,
            } => {
                // One that binds the model the session runs (the watcher keeps the others to
                // the subscription's reading): the session cannot go on here either, whatever
                // its own thread is doing (its next request would be refused).
                tracing::info!(task = %task_id, %message, ?scope, "usage limit reached by a workflow's agent");
                let limit = ReportedLimit {
                    scope,
                    window_minutes,
                    resets_at,
                };
                self.limit_met(task_id, attempt, limit, at, source).await;
            }
            Observation::AccessLost { message, at } => {
                // The refused turn moves on like a limit's, including after another sign-in.
                tracing::warn!(task = %task_id, %message, "subscription access refused");
                runtime.end_turn();
                runtime.limit_cut_the_turn(&attempt.id, at, None);
                let refused = match &attempt.account_id {
                    Some(account) => {
                        self.record_access_lost(account, Path::new(&attempt.run_dir), &message)
                            .await
                    }
                    None => None,
                };
                let app = self.clone();
                let tid = task_id.clone();
                let account = attempt.account_id.clone();
                let provider = attempt.harness.provider();
                let attempt_id = attempt.id.clone();
                tokio::spawn(async move {
                    app.rotate_limited_attempt(&tid, &attempt_id, account, false, None, source)
                        .await;
                    // Said in one line, with a Needs you when nothing is left (SPEC-447 MOV-006).
                    if let Some(label) = refused {
                        let left = crate::moves::Left::Refused {
                            provider,
                            label,
                            message,
                        };
                        app.tell_left(&tid, &left).await;
                    }
                });
            }
            Observation::UserTurn { at, wake } => {
                crate::race::reached(
                    crate::race::Point::UserTurnRead,
                    Path::new(task_id.as_str()),
                );
                if let Some(at) = at {
                    runtime.turn_moved_ms.fetch_max(at.0, Ordering::SeqCst);
                }
                // The retry's prompt (or a person's) starts the turn an armed retry waited for.
                runtime.goal_retry_armed.store(false, Ordering::SeqCst);
                // A new prompt: an achieved goal no longer makes the task done (TSK-014). Claude
                // Code's own wake-ups (background work ended) keep it, as the prompt hook does.
                if !wake {
                    runtime.goal_achieved.store(false, Ordering::SeqCst);
                }
                // A prompt of a turn whose clean Stop already came is not a new turn. Held until
                // Working is set, so a clean Stop arriving meanwhile opens its verdict wait, or
                // settles, after this prompt started its turn (SPEC-453 TPO-001).
                let _order = runtime.turn_order.lock().await;
                if at.is_some_and(|at| at.0 <= runtime.last_stop_ms.load(Ordering::SeqCst)) {
                    // Its turn came and went: what Claude Code handed on had it (SPEC-471).
                    runtime.with_queue(yulius_domain::agent_cli::Queue::turn_seen);
                    return;
                }
                crate::race::reached(
                    crate::race::Point::UserTurnChecked,
                    Path::new(task_id.as_str()),
                );
                // A prompt reached the agent while its turn or prompt was held for capacity:
                // the CLI gave the hold up (an interrupt, its own hook timeout), so the task
                // runs again (F 2026-10-10: the task follows its session).
                self.hold_overtaken(task_id, at, Activity::Working).await;
                runtime.at_limit.store(false, Ordering::SeqCst);
                runtime.background_agents.store(0, Ordering::SeqCst);
                runtime.notice_attention_ms.store(0, Ordering::SeqCst);
                runtime.resumed_turn.store(false, Ordering::SeqCst);
                // A new turn: the one that waited for its verdict goes on (BGW-005).
                runtime.verdict_stop_ms.clear();
                let now = self.now();
                runtime
                    .turn_began_ms
                    .store(at.map_or(now.0, |at| at.0), Ordering::SeqCst);
                runtime.start_turn(now);
                // Claude Code's own wake-up goes on with the stretch (SPEC-309 WSW-001), also when
                // its prompt hook never reached Yulius.
                let stretch = if wake {
                    self.continue_working_stretch(task_id).await
                } else {
                    None
                };
                let _ = self
                    .write_session_activity(task_id, Activity::Working)
                    .await;
                // When this turn began after the pull request watch typed, it is that news'
                // turn (SPEC-487 CHN-002); a prompt read late is another turn's.
                self.pr_news_turn_began(task_id, at.map(|at| at.0));
                self.publish_task(task_id).await;
                self.keep_working_stretch(task_id, stretch).await;
                // Only now, with the task reading Working: the turn of what Claude Code handed
                // on from its queue is known, so the hand-over is over (SPEC-471 CCQ-003).
                runtime.with_queue(yulius_domain::agent_cli::Queue::turn_seen);
            }
            Observation::TurnComplete { at } => {
                if let Some(at) = at {
                    runtime.turn_closed_ms.fetch_max(at.0, Ordering::SeqCst);
                    runtime.turn_moved_ms.fetch_max(at.0, Ordering::SeqCst);
                }
                self.context_replied(task_id, &attempt.id);
                // A continuation that is only the answer ending the turn has no agent work.
                self.cut_turn_continued(task_id, at).await;
                runtime.end_turn();
                // A sub-agent's turn end (no time) leaves the session at its limit.
                if at.is_some() {
                    runtime.at_limit.store(false, Ordering::SeqCst);
                    // The session answered: a goal pause before it asks nobody any more.
                    self.goal_pause_overtaken(task_id).await;
                }
                // With hooks the Stop hook is the boundary that switches or pauses; the
                // transcript decides only for a run without them.
                if !runtime.hooks_seen.load(Ordering::SeqCst) {
                    self.turn_ended(task_id, attempt.account_id.clone());
                }
            }
            Observation::TurnInterrupted { at } => {
                // Esc or Ctrl+C stopped the turn: no Stop hook runs, the agent waits for input.
                runtime.goal_retry_armed.store(false, Ordering::SeqCst);
                runtime.end_turn();
                self.turn_interrupted(task_id, at).await;
            }
            Observation::Goal(outcome, at) => {
                self.goal_observed(task_id, &attempt.id, outcome, at).await;
                crate::race::reached(
                    crate::race::Point::NativeVerdictChecked,
                    Path::new(attempt.id.as_str()),
                );
            }
            Observation::TurnEnded {
                background_agents,
                at,
            } => {
                if let Some(at) = at {
                    runtime.turn_closed_ms.fetch_max(at.0, Ordering::SeqCst);
                }
                // What still waits in Claude Code's queue was left by this turn's end: it is
                // not handed on at a later one (SPEC-479 SWQ-002).
                runtime.with_queue(yulius_domain::agent_cli::Queue::turn_closed);
                self.background_agents_observed(task_id, background_agents.unwrap_or(0))
                    .await;
                self.turn_record_seen(task_id, &attempt.id, at).await;
            }
            Observation::AgentWork { at } => {
                runtime.goal_retry_armed.store(false, Ordering::SeqCst);
                self.context_replied(task_id, &attempt.id);
                // A usage limit ends the turn, so a `limit` pause is open only between turns (or
                // after a limit replayed at a restart, which may find a turn open).
                if runtime.at_limit.load(Ordering::SeqCst) || !runtime.turn_in_progress() {
                    self.cut_turn_continued(task_id, at).await;
                }
                runtime.at_limit.store(false, Ordering::SeqCst);
                // The agent works: a goal pause before it asks nobody any more (SPEC-469).
                self.goal_pause_overtaken(task_id).await;
                self.agent_worked(task_id, &attempt.id, at).await;
            }
            Observation::GoalRetry { armed, notice } => {
                self.goal_retry_observed(task_id, armed, &notice).await;
            }
            Observation::ContextFull => self.context_full_observed(task_id, attempt).await,
            Observation::Compacted => self.compacted_observed(task_id).await,
            Observation::CompactFailed => self.compact_failed_observed(task_id, attempt).await,
            Observation::BackgroundStarted {
                id,
                agent,
                expires_in_ms,
                at,
            } => {
                self.background_started(task_id, id, agent, expires_in_ms, at)
                    .await;
            }
            Observation::BackgroundEnded { id } => self.background_ended(task_id, &id).await,
            Observation::BackgroundResumable { id } => {
                self.background_resumable(task_id, id).await;
            }
            // Kept above; its background work is the `BackgroundStarted` beside it.
            Observation::WorkflowLaunched { .. } => {}
            // What a review round's reviewer really ran on, and whether a limit cut it off
            // (SPEC-493 REV-004): nothing else depends on it.
            Observation::SubagentAnswered {
                agent_id,
                model,
                at,
            } => {
                self.review_subagent_noted(
                    task_id,
                    &agent_id,
                    Some(&model),
                    false,
                    source.review_time(at, self.now()),
                )
                .await;
            }
            // A reviewer's report in the transcript ends its round when its `SubagentStop`
            // never reached Yulius (SPEC-633 RRE-002).
            Observation::SubagentReported {
                agent_id,
                report,
                at,
            } => {
                self.review_agent_reported(
                    task_id,
                    &agent_id,
                    &report,
                    source.review_time(at, self.now()),
                )
                .await;
            }
            Observation::SubagentCutOff { agent_id, at, .. } => {
                self.review_subagent_noted(
                    task_id,
                    &agent_id,
                    None,
                    true,
                    source.review_time(at, self.now()),
                )
                .await;
            }
            // In the order of the lines: the count is ahead of nothing the watcher applied.
            Observation::Queue(change, at) => runtime.with_queue(|queue| queue.record(change, at)),
            Observation::Session { .. } => {}
            // The account's windows as Codex saw them on its last response (SPEC-503 CDX-004).
            Observation::RateLimits(usage) => {
                if let Some(account) = &attempt.account_id {
                    self.rollout_usage(account, &usage).await;
                }
            }
        }
        runtime.with_queue(|queue| queue.work_seen(worked));
    }

    /// A usage limit that cut off a subagent of `task_id`'s thread (SPEC-493 RLM-001, RLM-002):
    /// the subscription's reading keeps it, for the model family it binds. One that binds the
    /// session's own model (`family`, or the subscription) moves or pauses the session as any
    /// limit does; one of another model leaves the session where it is, unless the subagent is
    /// a review round's reviewer, whose model the session then moves for. Either way a relaunch
    /// tells the session to resume that reviewer.
    pub(crate) async fn subagent_limit(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        family: ModelFamily,
        observation: &Observation,
    ) {
        let Observation::SubagentCutOff {
            agent_id,
            at,
            resets_at,
            message,
            window_minutes,
            scope,
        } = observation
        else {
            return;
        };
        use yulius_domain::usage::LimitScope;
        // A limit of "the model that ran" of a review round's reviewer is of its round's model,
        // also when no answer of it was read yet (refused at its first request, or resumed in a
        // new process); one of a subagent whose model cannot be told is no one's (review round
        // 1 of fix 7): it is never taken for the session's.
        // Pair the eligibility read and reservation with the hooks that clear recovery.
        // Release this lock before writing account usage so a reviewer can report meanwhile.
        let runtime = self.runtime(task_id);
        let mut pending = runtime.cut_off_reviewer.lock().await;
        let reviewer = self.open_round_reviewer(task_id, agent_id, *at).await;
        let scope = match (*scope, &reviewer) {
            (LimitScope::Model(None), Some(reviewer)) => {
                match ModelFamily::of(Some(&reviewer.model)) {
                    ModelFamily::Any => LimitScope::Model(None),
                    named => LimitScope::Model(Some(named)),
                }
            }
            (scope, _) => scope,
        };
        if scope == LimitScope::Model(None) {
            tracing::info!(task = %task_id, %message, "a usage limit of a subagent whose model is not known: not the session's");
            return;
        }
        if let Some(reviewer) = &reviewer {
            *pending = Some(reviewer.clone());
        }
        drop(pending);
        let limit = ReportedLimit {
            scope,
            window_minutes: *window_minutes,
            resets_at: *resets_at,
        };
        let binds_session = !matches!(
            scope,
            LimitScope::Model(Some(other)) if other != family
        );
        tracing::info!(task = %task_id, %message, ?scope, binds_session, "usage limit reached by a subagent");
        if binds_session {
            self.limit_met(task_id, attempt, limit, *at, ObservationSource::Live)
                .await;
            return;
        }
        // Recorded before the move is decided: where the session is reads as unable to run the
        // reviewer's model only then (review round 1 of fix 7).
        if let Some(account) = &attempt.account_id {
            self.record_limit(account, limit, family).await;
        }
        if let Some(reviewer) = reviewer {
            let pending = runtime.cut_off_reviewer.lock().await;
            if pending.as_ref() == Some(&reviewer) {
                runtime.reviewer_move.store(true, Ordering::SeqCst);
            }
        }
    }

    /// The session of `task_id` is at the usage limit `limit`, recorded at `at`: its turn is
    /// over, the subscription's reading keeps the limit, and the pool decides where the task
    /// goes on (SPEC-034, SPEC-464).
    async fn limit_met(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        limit: ReportedLimit,
        at: Option<Timestamp>,
        source: ObservationSource,
    ) {
        let runtime = self.runtime(task_id);
        runtime.end_turn();
        let blind = limit.without_an_end(self.now());
        // The limit of the model this run was launched with (SPEC-482 FBK-006), unless it names
        // another one: the model the session was switched to inside it (SPEC-493 MDL-002).
        let family = self.run_family_of(attempt).await;
        let ran = switched_model(&limit, family);
        runtime.limit_cut_the_turn(&attempt.id, at, ran);
        if let Some(account) = &attempt.account_id {
            // A hand-over may place the replacement itself. The reporting account and
            // model still count if the pool later exhausts its retry without an answer.
            if blind && let Ok(mut met) = runtime.blind_limits.lock() {
                met.met(account, ran.unwrap_or(family));
            }
            self.record_limit(account, limit, family).await;
        }
        let app = self.clone();
        let tid = task_id.clone();
        let account = attempt.account_id.clone();
        let attempt_id = attempt.id.clone();
        tokio::spawn(async move {
            crate::race::reached(crate::race::Point::LimitMoveReady, Path::new(tid.as_str()));
            app.rotate_limited_attempt(&tid, &attempt_id, account, blind, ran, source)
                .await;
            crate::race::reached(
                crate::race::Point::LimitMoveFinished,
                Path::new(tid.as_str()),
            );
        });
    }

    /// Reconciliation can replace a run holding the previous owner's servers after reading
    /// this limit. Apply the pool decision only if the reporting attempt still owns the run.
    async fn rotate_limited_attempt(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        account: Option<AccountId>,
        blind: bool,
        ran: Option<ModelFamily>,
        source: ObservationSource,
    ) {
        let Some(runtime) = self.existing_runtime(task_id) else {
            return;
        };
        let recovery = runtime.reattach_move.lock().await;
        // All reports wait for a pending hand-over. Only a replayed move holds the lock
        // through its decision. A new live pause can resume while its notices finish.
        let _recovery = matches!(source, ObservationSource::Replay).then_some(recovery);
        let current = runtime
            .run
            .lock()
            .await
            .as_ref()
            .is_some_and(|run| &run.attempt_id == attempt_id);
        if current {
            self.rotate_past(task_id, account, Trigger::LimitReached, blind, ran)
                .await;
        }
    }

    /// Applies the pool policy for a running task and acts on the decision (SPEC-001 §7.4):
    /// stay, move to another subscription by native resume, or pause (ROT-010).
    pub async fn rotate(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
    ) {
        self.rotate_at(task_id, current, trigger, false).await;
    }

    /// [`Self::rotate`]; `blind` says the limit that asks for it is a model's that nothing says
    /// the end of (SPEC-464 UCL-010).
    async fn rotate_at(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
        blind: bool,
    ) {
        self.rotate_past(task_id, current, trigger, blind, None)
            .await;
    }

    /// [`Self::rotate_at`]; `ran` is the model family the limit binds when that is not the one
    /// the run was launched with ([`switched_model`]). The run placed past the limit is the
    /// task's own model again, so the subscription it sits on still counts, and the session's
    /// terminal says why it goes on with another model than its person chose inside it.
    async fn rotate_past(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
        blind: bool,
        ran: Option<ModelFamily>,
    ) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let limit = matches!(trigger, Trigger::LimitReached);
        // A limit can also end a turn that is already held at its boundary.
        if task.status != TaskStatus::Running
            && !(limit && task.status == TaskStatus::WaitingCapacity)
        {
            return;
        }
        let Ok(mut input) = self.rotation_input(&task, current.clone()).await else {
            return;
        };
        if ran.is_some() {
            input.running = ran;
        }
        let (decision, reason) = rotation::decide(&input, trigger);
        self.runtime(task_id)
            .proactive_switch
            .store(false, Ordering::SeqCst);
        match decision {
            Decision::Stay => {}
            Decision::Switch { account } => {
                // A turn the limit cut goes on past it, with nothing typed (SPEC-034).
                let reason = format!("{reason:?}");
                let switched = if limit {
                    self.switch_after_limit(task_id, Some(account), &reason)
                        .await
                } else {
                    self.switch_task_now(task_id, Some(account), &reason, None)
                        .await
                };
                if let Err(err) = switched {
                    tracing::warn!(error = %err, task = %task_id, "automatic switch failed");
                }
            }
            Decision::Wait { until } => {
                // A model's limit nothing says the end of, with nowhere to go: the task is
                // tried again by itself only so often (SPEC-464 UCL-010). After that it waits
                // for a reset that is known, off this subscription, or asks a person.
                if blind && limit {
                    let next = self
                        .runtime(task_id)
                        .blind_limits
                        .lock()
                        .map_or(crate::moves::BlindWait::Pause, |mut blind| {
                            blind.nowhere_to_go(until, task.status)
                        });
                    if next == crate::moves::BlindWait::AskAPerson {
                        self.limit_without_end(&task).await;
                        return;
                    }
                }
                let paused = if limit {
                    let paused = self
                        .pause_task(task_id, PauseKind::Limit, current, None)
                        .await;
                    // The pool resumes a task that waits at its limit by itself: a request
                    // that says it does not go on by itself is answered, as at a move past
                    // the limit (SPEC-464 UCL-007).
                    if paused.is_ok() {
                        crate::race::reached(
                            crate::race::Point::LimitPauseStored,
                            Path::new(task_id.as_str()),
                        );
                        self.end_limit_requests(task_id, crate::goal_retry::TakenOn::Paused)
                            .await;
                    }
                    paused
                } else {
                    self.pause_task(task_id, PauseKind::Turn, current, None)
                        .await
                };
                if let Err(err) = paused {
                    tracing::warn!(error = %err, task = %task_id, "could not pause the task");
                }
            }
        }
    }
}

/// The model family `limit` names when that is not `family`, the one the run was launched with:
/// the session's person switched the model inside it (`/model`), which no launch follows, and
/// Claude Code's record of the refused request named it (SPEC-493 MDL-002). The limit is that
/// model's alone.
pub(crate) fn switched_model(limit: &ReportedLimit, family: ModelFamily) -> Option<ModelFamily> {
    match limit.scope {
        yulius_domain::usage::LimitScope::Model(Some(named)) if named != family => Some(named),
        _ => None,
    }
}

/// What a session's terminal says when the model its person switched it to (`ran`) is at its
/// limit and Yulius continues the session with `next`, the model the relaunch runs (the
/// session's own, or its fallback).
pub(crate) fn switched_back_notice(ran: ModelFamily, next: &str) -> String {
    let ran = match ran {
        ModelFamily::Fable => "Fable",
        ModelFamily::Opus => "Opus",
        ModelFamily::Sonnet => "Sonnet",
        ModelFamily::Any => "That model",
    };
    format!(
        "{ran} is used up on this subscription: this session goes on with {}.",
        claude_model_name(next)
    )
}

/// Whether `observation` is a workflow agent's limit of another model family than `family`, the
/// one the session's run was launched with (SPEC-486 WFL-002): that agent's alone.
fn binds_another_model(observation: &Observation, family: ModelFamily) -> bool {
    matches!(
        observation,
        Observation::WorkflowLimitReached {
            scope: yulius_domain::usage::LimitScope::Model(Some(other)),
            ..
        } if *other != family
    )
}

/// The limit a workflow agent's limit observation reports.
fn reported_limit(observation: &Observation) -> Option<ReportedLimit> {
    match observation {
        Observation::WorkflowLimitReached {
            resets_at,
            window_minutes,
            scope,
            ..
        } => Some(ReportedLimit {
            scope: *scope,
            window_minutes: *window_minutes,
            resets_at: *resets_at,
        }),
        _ => None,
    }
}

/// Whether `observation` shows the model answered in the session: a record of its own work or of
/// its turn's end, not one Claude Code writes itself for a refusal, and not a sub-agent's turn
/// end (which carries no time).
fn model_answered(observation: &Observation) -> bool {
    matches!(
        observation,
        Observation::AgentWork { .. } | Observation::TurnComplete { at: Some(_) }
    )
}

/// Asks the task's run, if one is attached, to shut down gracefully (interrupt, then TERM, then
/// kill on the host's side). Taking the run lock and writing are bounded together (SPEC-065
/// END-005): a stuck host must not hold the lock that ending the run needs.
async fn request_shutdown(runtime: &TaskRuntime, selected: Option<&AttemptId>) {
    let asked = tokio::time::timeout(SHUTDOWN_WRITE_BOUND, async {
        if let Some(run) = runtime.run.lock().await.as_mut()
            && selected.is_none_or(|id| id == &run.attempt_id)
        {
            let _ = run
                .writer
                .shutdown(u32::try_from(STOP_INTERRUPT_GRACE_MS).unwrap_or(5000))
                .await;
        }
    })
    .await;
    if asked.is_err() {
        tracing::warn!("the run did not take the shutdown request in time; the escalation ends it");
    }
}

/// Where the pool places a run (SPEC-482).
pub(crate) struct Placed {
    /// The subscription.
    pub account: AccountRecord,
    /// The model the run is launched with instead of the task's own, while no subscription can
    /// run that; `None`: the task's own.
    pub fallback: Option<&'static str>,
}

/// What a session's terminal says when a launch changes the model it runs (SPEC-482 FBK-008):
/// `model` is not the task's `own` (the launch falls back), or it is again after a run on
/// another model (`before`, the model of the task's last run). `None` when nothing changes.
pub(crate) fn model_change_notice(own: &str, before: Option<&str>, model: &str) -> Option<String> {
    let name = claude_model_name;
    if model != own {
        return (before != Some(model)).then(|| {
            format!(
                "{} is used up on every subscription: this session runs on {}.",
                name(own),
                name(model)
            )
        });
    }
    // Only a fallback is undone: a task set to another model since its last run says nothing.
    let fell_back = before.is_some_and(|before| {
        before != own && yulius_domain::harness::fallback_model(own) == Some(before)
    });
    fell_back.then(|| format!("{} can run again: this session runs on it.", name(own)))
}

/// Why a person's choice of subscription cannot take the task, in one plain sentence naming it
/// (SPEC-447 MOV-002).
pub(crate) fn refused_choice(
    label: Option<&str>,
    provider: yulius_domain::harness::Provider,
    reason: &rotation::DecisionReason,
) -> String {
    use rotation::{DecisionReason, Refusal};
    let name = label.unwrap_or("That subscription");
    match reason {
        DecisionReason::ManualRefused(Refusal::UnknownAccount) => {
            "That subscription no longer exists.".into()
        }
        DecisionReason::ManualRefused(Refusal::Unavailable) => {
            format!("{name} needs a new sign-in, so it cannot take the task.")
        }
        DecisionReason::ManualRefused(Refusal::Exhausted) => {
            format!("{name} is at its usage limit, so it cannot take the task until it resets.")
        }
        DecisionReason::ManualRefused(Refusal::ProviderMismatch) => {
            format!(
                "{name} is not {} subscription, so it cannot take the task.",
                crate::account_gate::a_kind(provider)
            )
        }
        _ => "No other subscription can take the task right now.".into(),
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

    /// SPEC-482 FBK-008: the terminal says a change of the model once, at the launch that
    /// makes it: onto the fallback, and back to the task's own model after a run on it.
    #[test]
    fn a_launch_says_when_the_model_changes() {
        use super::model_change_notice as said;
        let (fable, opus) = ("claude-fable-5-2", "claude-opus-5-5");
        let fell_back =
            "Fable 5.2 is used up on every subscription: this session runs on Opus 5.5.";
        let back = "Fable 5.2 can run again: this session runs on it.";
        // The first run, a run after one on Fable, and one after a run that names no model.
        for before in [None, Some(fable)] {
            assert_eq!(said(fable, before, opus).as_deref(), Some(fell_back));
            assert_eq!(said(fable, before, fable), None);
        }
        assert_eq!(said(fable, Some(opus), opus), None, "said at the first");
        assert_eq!(said(fable, Some(opus), fable).as_deref(), Some(back));
        // An older Fable model names itself.
        assert_eq!(
            said("claude-fable-5-1", Some(opus), "claude-fable-5-1").as_deref(),
            Some("Fable 5.1 can run again: this session runs on it.")
        );
        // A task a person set to another model since its last run did not fall back.
        assert_eq!(said(opus, Some(fable), opus), None);
        assert_eq!(
            said("claude-sonnet-5-5", Some(opus), "claude-sonnet-5-5"),
            None
        );
    }

    /// SPEC-464 UCL-010: only the model's own records say it answered. A turn's work (a tool
    /// call) and its end both do; a refusal Claude Code writes itself, a prompt and a
    /// sub-agent's turn end (no time) do not, so a retry refused the same way stays a retry.
    #[test]
    fn the_models_own_records_say_it_answered() {
        use super::{Observation, Timestamp, model_answered};
        use yulius_domain::usage::LimitScope;
        let at = Some(Timestamp(1_000));
        assert!(model_answered(&Observation::AgentWork { at }));
        assert!(model_answered(&Observation::AgentWork { at: None }));
        assert!(model_answered(&Observation::TurnComplete { at }));
        assert!(!model_answered(&Observation::TurnComplete { at: None }));
        assert!(!model_answered(&Observation::UserTurn { at, wake: false }));
        assert!(!model_answered(&Observation::LimitReached {
            resets_at: None,
            message: "You're out of usage credits.".into(),
            window_minutes: None,
            scope: LimitScope::Model(None),
            at,
        }));
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
