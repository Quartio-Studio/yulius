//! Task and attempt state machines (SPEC-002 §6.1, §6.2).
//!
//! Every nonterminal state has an owner, an exit trigger and a timeout; the timeouts are
//! constants here and enforced by the application supervisor.

/// Why a task's live run ended without a normal close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopReason {
    /// The user or system cancelled the run.
    Cancelled,
    /// The agent process exited abnormally.
    Crashed,
    /// The agent process was ended by a signal from outside Yulius (`kill`, the OOM killer);
    /// Claude Code itself exits with 128 + the signal (SIGTERM → 143, SIGHUP → 129).
    Killed,
    /// The process could not be confirmed (server restart, reboot, unreachable host).
    Lost,
    /// A switch could not complete.
    SwitchFailed,
    /// A switch exceeded its total time budget.
    SwitchTimeout,
    /// Yulius restarted and the run was no longer attached.
    ServerLost,
}

/// Why a task failed to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailReason {
    /// The process could not be spawned.
    SpawnError,
    /// The process did not report running in time.
    StartTimeout,
    /// Preconditions failed (worktree, credentials, capacity rejection).
    Precondition,
}

/// Durable task status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    /// Waiting for a free session slot. Owner: scheduler.
    Queued,
    /// Process spawn in progress. Owner: supervisor. Timeout: [`START_TIMEOUT_MS`].
    Starting,
    /// Agent process alive. Owner: supervisor.
    Running,
    /// Account or provider switch in progress. Owner: rotation controller.
    /// Timeout: [`SWITCH_TIMEOUT_MS`].
    Switching,
    /// Paused because no subscription has capacity (ROT-010): held at a turn boundary or a
    /// limit with the process alive, or waiting to start without one. Owner: the capacity
    /// controller; re-evaluated at the earliest reset, on new usage or accounts, and every
    /// 5 minutes.
    WaitingCapacity,
    /// Cancellation in progress until the process tree is reaped. Owner: supervisor.
    Stopping,
    /// The user left the agent session normally; continuable.
    Closed,
    /// The run ended for a [`StopReason`]; continuable.
    Stopped(StopReason),
    /// The run never started; continuable after fixing the cause.
    Failed(FailReason),
    /// Waiting for another task to be Done before it starts (SPEC-001 TSK-012). Owner: the
    /// chain controller. Exit: the predecessor is Done or a person starts it now → `queued`;
    /// cancel → `stopped(cancelled)`. A predecessor that ends any other way leaves it here with
    /// a reason ("needs you"); it never starts by itself after that.
    Chained,
}

/// Fine-grained activity while [`TaskStatus::Running`] (derived from hooks and transcripts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Activity {
    /// The agent is processing a turn.
    Working,
    /// The agent finished its turn and waits for input.
    Waiting,
    /// A person must act (permission prompt, browser hand-off, discipline gate, capacity).
    NeedsAttention,
    /// The agent finished the task: its turn ended cleanly after Claude Code recorded the task's
    /// goal as achieved (SPEC-001 TSK-014). The session stays attached and idle; any new prompt
    /// makes the task [`Activity::Working`] again.
    Done,
}

/// What Claude Code recorded about the session's goal (`/goal`, SPEC-001 TSK-010/TSK-014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoalOutcome {
    /// A goal is set and active (Claude Code's sentinel record); not achieved yet.
    Set,
    /// An active goal was cleared without being achieved (a sentinel marking it inactive).
    Cleared,
    /// The goal evaluator found it not met yet; the agent keeps working.
    NotMet,
    /// The goal was achieved.
    Met,
    /// The goal could not be achieved (the evaluator judged it impossible).
    Failed,
}

/// Inputs that move a task between states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskEvent {
    /// A session slot was granted to a queued task.
    CapacityGranted,
    /// The process host reported the agent running.
    Spawned,
    /// Spawning failed or timed out.
    StartFailed(FailReason),
    /// The agent process exited; `normal` when it exited cleanly on the user's request.
    Exited {
        /// Clean exit (status 0, not signalled).
        normal: bool,
    },
    /// The agent process was ended by a signal Yulius did not send.
    Killed,
    /// Stop requested by a user or the system.
    CancelRequested,
    /// The process tree was confirmed reaped after cancellation.
    Reaped,
    /// The rotation controller decided to switch.
    SwitchStarted,
    /// The replacement run is live.
    SwitchCompleted,
    /// The switch failed or timed out.
    SwitchAborted(StopReason),
    /// No account has capacity right now.
    CapacityExhausted,
    /// Capacity returned on the account a paused live run already uses.
    CapacityReturned,
    /// The run can no longer be confirmed.
    Lost(StopReason),
    /// A user asked to continue a non-live task.
    ContinueRequested,
    /// A continue request had to wait for capacity.
    ContinueQueued,
    /// A chained task's predecessor is Done, or a person started it now.
    ChainReleased,
}

/// An event that is not valid in the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("event {event:?} is not valid in state {from:?}")]
pub struct TransitionError {
    /// Current state.
    pub from: TaskStatus,
    /// Rejected event.
    pub event: TaskEvent,
}

/// Spawn must report running within this time.
pub const START_TIMEOUT_MS: i64 = 30_000;
/// Maximum total duration of an account/provider switch.
pub const SWITCH_TIMEOUT_MS: i64 = 120_000;
/// Graceful interrupt period before SIGTERM.
pub const STOP_INTERRUPT_GRACE_MS: i64 = 5_000;
/// SIGTERM period before SIGKILL.
pub const STOP_TERM_GRACE_MS: i64 = 5_000;
/// Wait for reaping after SIGKILL before declaring the run lost.
pub const STOP_KILL_WAIT_MS: i64 = 10_000;

impl TaskStatus {
    /// Whether a process may be alive in this state (capacity is held).
    #[must_use]
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Switching | Self::Stopping
        )
    }

    /// Whether the task may be archived or continued (no live process, not queued).
    #[must_use]
    pub fn is_resting(self) -> bool {
        matches!(self, Self::Closed | Self::Stopped(_) | Self::Failed(_))
    }

    /// Applies an event.
    ///
    /// # Errors
    /// Returns [`TransitionError`] when the event is invalid in this state; callers use the
    /// error to reject racing requests (one durable outcome per run).
    pub fn apply(self, event: TaskEvent) -> Result<Self, TransitionError> {
        use TaskEvent as E;
        use TaskStatus as S;
        let next = match (self, event) {
            (S::Queued, E::CapacityGranted) => S::Starting,
            (S::Queued | S::Chained, E::CancelRequested) => S::Stopped(StopReason::Cancelled),
            (S::Chained, E::ChainReleased) => S::Queued,
            (S::Starting, E::Spawned) => S::Running,
            (S::Starting, E::StartFailed(reason)) => S::Failed(reason),
            (S::Starting | S::Running | S::Switching, E::CancelRequested) => S::Stopping,
            (S::Running, E::Exited { normal: true }) => S::Closed,
            (S::Running | S::Starting, E::Exited { normal: false }) => {
                S::Stopped(StopReason::Crashed)
            }
            (S::Running | S::Starting | S::WaitingCapacity, E::Killed) => {
                S::Stopped(StopReason::Killed)
            }
            (S::Running, E::SwitchStarted) | (S::WaitingCapacity, E::SwitchStarted) => S::Switching,
            (S::Running | S::Switching | S::Starting | S::Queued, E::CapacityExhausted) => {
                S::WaitingCapacity
            }
            (S::WaitingCapacity, E::CapacityReturned) => S::Running,
            (S::WaitingCapacity, E::CapacityGranted) => S::Starting,
            (S::WaitingCapacity, E::ContinueQueued) => S::Queued,
            (S::WaitingCapacity, E::Exited { normal: true }) => S::Closed,
            (S::WaitingCapacity, E::Exited { normal: false }) => S::Stopped(StopReason::Crashed),
            (S::WaitingCapacity, E::Lost(reason)) => S::Stopped(reason),
            (S::Switching, E::SwitchCompleted) => S::Running,
            (S::Switching, E::SwitchAborted(reason)) => S::Stopped(reason),
            (S::WaitingCapacity, E::CancelRequested) => S::Stopped(StopReason::Cancelled),
            (S::Stopping, E::Reaped | E::Exited { .. } | E::Killed) => {
                S::Stopped(StopReason::Cancelled)
            }
            (S::Starting | S::Running | S::Switching | S::Stopping, E::Lost(reason)) => {
                S::Stopped(reason)
            }
            (S::Closed | S::Stopped(_) | S::Failed(_), E::ContinueRequested) => S::Starting,
            (S::Closed | S::Stopped(_) | S::Failed(_), E::ContinueQueued) => S::Queued,
            (from, event) => return Err(TransitionError { from, event }),
        };
        Ok(next)
    }

    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Switching => "switching",
            Self::WaitingCapacity => "waiting_capacity",
            Self::Stopping => "stopping",
            Self::Closed => "closed",
            Self::Stopped(StopReason::Cancelled) => "stopped_cancelled",
            Self::Stopped(StopReason::Crashed) => "stopped_crashed",
            Self::Stopped(StopReason::Killed) => "stopped_killed",
            Self::Stopped(StopReason::Lost) => "stopped_lost",
            Self::Stopped(StopReason::SwitchFailed) => "stopped_switch_failed",
            Self::Stopped(StopReason::SwitchTimeout) => "stopped_switch_timeout",
            Self::Stopped(StopReason::ServerLost) => "stopped_server_lost",
            Self::Failed(FailReason::SpawnError) => "failed_spawn_error",
            Self::Failed(FailReason::StartTimeout) => "failed_start_timeout",
            Self::Failed(FailReason::Precondition) => "failed_precondition",
            Self::Chained => "chained",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        ALL_STATUSES
            .iter()
            .copied()
            .find(|status| status.as_str() == text)
    }
}

/// Every task status, for exhaustive mapping.
pub const ALL_STATUSES: [TaskStatus; 18] = [
    TaskStatus::Queued,
    TaskStatus::Starting,
    TaskStatus::Running,
    TaskStatus::Switching,
    TaskStatus::WaitingCapacity,
    TaskStatus::Stopping,
    TaskStatus::Closed,
    TaskStatus::Stopped(StopReason::Cancelled),
    TaskStatus::Stopped(StopReason::Crashed),
    TaskStatus::Stopped(StopReason::Killed),
    TaskStatus::Stopped(StopReason::Lost),
    TaskStatus::Stopped(StopReason::SwitchFailed),
    TaskStatus::Stopped(StopReason::SwitchTimeout),
    TaskStatus::Stopped(StopReason::ServerLost),
    TaskStatus::Failed(FailReason::SpawnError),
    TaskStatus::Failed(FailReason::StartTimeout),
    TaskStatus::Failed(FailReason::Precondition),
    TaskStatus::Chained,
];

impl Activity {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::NeedsAttention => "needs_attention",
            Self::Done => "done",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "working" => Some(Self::Working),
            "waiting" => Some(Self::Waiting),
            "needs_attention" => Some(Self::NeedsAttention),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    /// The agent's turn has ended and nobody needs to act: it waits for input or is done.
    #[must_use]
    pub fn is_idle(self) -> bool {
        matches!(self, Self::Waiting | Self::Done)
    }

    /// The activity after a clean turn end (TSK-014): done when Claude Code recorded the task's
    /// goal as achieved and no prompt came since, waiting otherwise. Background shells do not
    /// change it; they are a live overlay (the task shows Working while one runs).
    #[must_use]
    pub fn after_clean_turn(goal_achieved: bool) -> Self {
        if goal_achieved {
            Self::Done
        } else {
            Self::Waiting
        }
    }
}

/// What a task's pull request says (SPEC-001 GIT-001), as the Git overview last read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullSignal {
    /// Open with checks green, or with none required: tested.
    Tested,
    /// Open while its checks run or cannot be read.
    InReview,
    /// Open with failing checks.
    Failing,
    /// Merged.
    Merged,
}

impl PullSignal {
    /// From a pull request's `state` and `checks` wire names (`PullView`); `None` for a closed
    /// pull request, which says nothing about the task.
    #[must_use]
    pub fn from_wire(state: &str, checks: &str) -> Option<Self> {
        match (state, checks) {
            ("merged", _) => Some(Self::Merged),
            ("open", "success" | "none") => Some(Self::Tested),
            ("open", "failure") => Some(Self::Failing),
            ("open", _) => Some(Self::InReview),
            _ => None,
        }
    }
}

/// Everything the "needs you" rule reads about one task (SPEC-001 §6.1, UI-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttentionFacts {
    /// Durable status.
    pub status: TaskStatus,
    /// Activity while running.
    pub activity: Option<Activity>,
    /// A prompt typed into the session is held for a person's decision (TSK-013).
    pub held_prompt: bool,
    /// The task asked a person to act in the sandbox browser and the request is pending
    /// (BRW-004).
    pub browser_request: bool,
    /// A chained task whose predecessor ended other than Done (TSK-012).
    pub chain_blocked: bool,
    /// The task has a plan with steps not done or skipped yet (STP-001).
    pub plan_open: bool,
    /// Its pull request, when one is known.
    pub pull: Option<PullSignal>,
}

impl AttentionFacts {
    /// Whether the task waits for a person: the sidebar's "Needs you" group, counted the same way
    /// on Home and the sandbox tabs (`taskInfo` in the web UI's `status.js` is the same rule).
    ///
    /// An ended task needs nobody. A held prompt or a pending browser request does, whatever the
    /// task's state. After a finished turn, failing checks do, and a tested, reviewed or merged
    /// pull request answers for a task whose plan is complete (Done or In review). Otherwise: a
    /// permission prompt (`needs_attention`), "Your turn" (`waiting`), or a chain whose
    /// predecessor ended otherwise.
    #[must_use]
    pub fn needs_you(&self) -> bool {
        if self.status.is_resting() {
            return false;
        }
        if self.held_prompt || self.browser_request {
            return true;
        }
        let finished =
            self.status == TaskStatus::Running && self.activity.is_some_and(Activity::is_idle);
        if finished && let Some(pull) = self.pull {
            if pull == PullSignal::Failing {
                return true;
            }
            if !self.plan_open {
                return false;
            }
        }
        match self.status {
            TaskStatus::Running => matches!(
                self.activity,
                Some(Activity::NeedsAttention | Activity::Waiting)
            ),
            TaskStatus::Chained => self.chain_blocked,
            _ => false,
        }
    }
}

/// Process run state (SPEC-002 §6.2). Exactly one terminal state is ever recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptState {
    /// Process host being created.
    Spawning,
    /// Process running.
    Live,
    /// Process exited by itself.
    Exited,
    /// Process was terminated by Yulius (cancel or switch).
    Killed,
    /// Process could not be confirmed.
    Lost,
}

impl AttemptState {
    /// Terminal states cannot change again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Killed | Self::Lost)
    }

    /// Compare-and-set semantics: may the state move to `next`?
    #[must_use]
    pub fn can_become(self, next: Self) -> bool {
        match (self, next) {
            (Self::Spawning, Self::Live) => true,
            (Self::Spawning | Self::Live, n) if n.is_terminal() => true,
            _ => false,
        }
    }

    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawning => "spawning",
            Self::Live => "live",
            Self::Exited => "exited",
            Self::Killed => "killed",
            Self::Lost => "lost",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "spawning" => Some(Self::Spawning),
            "live" => Some(Self::Live),
            "exited" => Some(Self::Exited),
            "killed" => Some(Self::Killed),
            "lost" => Some(Self::Lost),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_and_close() -> Result<(), TransitionError> {
        let s = TaskStatus::Queued.apply(TaskEvent::CapacityGranted)?;
        let s = s.apply(TaskEvent::Spawned)?;
        assert_eq!(s, TaskStatus::Running);
        let s = s.apply(TaskEvent::Exited { normal: true })?;
        assert_eq!(s, TaskStatus::Closed);
        assert_eq!(s.apply(TaskEvent::ContinueRequested)?, TaskStatus::Starting);
        Ok(())
    }

    /// ROT-010: a paused task resumes in place, starts, queues, or ends with its process.
    #[test]
    fn paused_tasks_resume_start_or_end() -> Result<(), TransitionError> {
        let paused = TaskStatus::Running.apply(TaskEvent::CapacityExhausted)?;
        assert_eq!(paused, TaskStatus::WaitingCapacity);
        assert_eq!(
            TaskStatus::Starting.apply(TaskEvent::CapacityExhausted)?,
            TaskStatus::WaitingCapacity
        );
        assert_eq!(
            TaskStatus::Queued.apply(TaskEvent::CapacityExhausted)?,
            TaskStatus::WaitingCapacity
        );
        assert_eq!(
            paused.apply(TaskEvent::CapacityReturned)?,
            TaskStatus::Running
        );
        assert_eq!(
            paused.apply(TaskEvent::CapacityGranted)?,
            TaskStatus::Starting
        );
        assert_eq!(
            paused.apply(TaskEvent::SwitchStarted)?,
            TaskStatus::Switching
        );
        assert_eq!(paused.apply(TaskEvent::ContinueQueued)?, TaskStatus::Queued);
        assert_eq!(
            paused.apply(TaskEvent::Exited { normal: true })?,
            TaskStatus::Closed
        );
        assert_eq!(
            paused.apply(TaskEvent::Lost(StopReason::ServerLost))?,
            TaskStatus::Stopped(StopReason::ServerLost)
        );
        assert_eq!(
            paused.apply(TaskEvent::CancelRequested)?,
            TaskStatus::Stopped(StopReason::Cancelled)
        );
        assert!(
            TaskStatus::Closed
                .apply(TaskEvent::CapacityReturned)
                .is_err()
        );
        Ok(())
    }

    /// Every way a session ends has one next state (F 2026-10-10: the task follows its session).
    #[test]
    fn every_session_end_has_one_next_state() -> Result<(), TransitionError> {
        let running = TaskStatus::Running;
        assert_eq!(
            running.apply(TaskEvent::Exited { normal: true })?,
            TaskStatus::Closed
        );
        assert_eq!(
            running.apply(TaskEvent::Exited { normal: false })?,
            TaskStatus::Stopped(StopReason::Crashed)
        );
        assert_eq!(
            running.apply(TaskEvent::Killed)?,
            TaskStatus::Stopped(StopReason::Killed)
        );
        assert_eq!(
            TaskStatus::WaitingCapacity.apply(TaskEvent::Killed)?,
            TaskStatus::Stopped(StopReason::Killed)
        );
        // A person's stop wins over whatever signal ended the process.
        let stopping = running.apply(TaskEvent::CancelRequested)?;
        assert_eq!(
            stopping.apply(TaskEvent::Killed)?,
            TaskStatus::Stopped(StopReason::Cancelled)
        );
        assert!(TaskStatus::Closed.apply(TaskEvent::Killed).is_err());
        assert_eq!(
            TaskStatus::parse("stopped_killed"),
            Some(TaskStatus::Stopped(StopReason::Killed))
        );
        Ok(())
    }

    #[test]
    fn cancel_races_exit_once() -> Result<(), TransitionError> {
        let stopping = TaskStatus::Running.apply(TaskEvent::CancelRequested)?;
        assert_eq!(stopping, TaskStatus::Stopping);
        let stopped = stopping.apply(TaskEvent::Exited { normal: true })?;
        assert_eq!(stopped, TaskStatus::Stopped(StopReason::Cancelled));
        // A late reap after the durable outcome is rejected.
        assert!(stopped.apply(TaskEvent::Reaped).is_err());
        Ok(())
    }

    #[test]
    fn switching_paths() -> Result<(), TransitionError> {
        let s = TaskStatus::Running.apply(TaskEvent::SwitchStarted)?;
        assert_eq!(s.apply(TaskEvent::SwitchCompleted)?, TaskStatus::Running);
        assert_eq!(
            s.apply(TaskEvent::CapacityExhausted)?,
            TaskStatus::WaitingCapacity
        );
        assert_eq!(
            s.apply(TaskEvent::SwitchAborted(StopReason::SwitchTimeout))?,
            TaskStatus::Stopped(StopReason::SwitchTimeout)
        );
        assert!(TaskStatus::Closed.apply(TaskEvent::SwitchStarted).is_err());
        Ok(())
    }

    #[test]
    fn wire_names_round_trip() {
        for status in ALL_STATUSES {
            assert_eq!(TaskStatus::parse(status.as_str()), Some(status));
        }
        assert!(TaskStatus::Running.is_live());
        assert!(!TaskStatus::Queued.is_live());
        assert!(TaskStatus::Failed(FailReason::SpawnError).is_resting());
    }

    #[test]
    fn chained_tasks_wait_until_released_or_cancelled() -> Result<(), TransitionError> {
        assert_eq!(
            TaskStatus::Chained.apply(TaskEvent::ChainReleased)?,
            TaskStatus::Queued
        );
        assert_eq!(
            TaskStatus::Chained.apply(TaskEvent::CancelRequested)?,
            TaskStatus::Stopped(StopReason::Cancelled)
        );
        // Nothing else starts it: no capacity grant, no continue.
        assert!(
            TaskStatus::Chained
                .apply(TaskEvent::CapacityGranted)
                .is_err()
        );
        assert!(
            TaskStatus::Chained
                .apply(TaskEvent::ContinueRequested)
                .is_err()
        );
        assert!(TaskStatus::Queued.apply(TaskEvent::ChainReleased).is_err());
        assert!(!TaskStatus::Chained.is_live());
        assert!(!TaskStatus::Chained.is_resting());
        Ok(())
    }

    #[test]
    fn activity_wire_names_and_clean_turn_end() {
        for activity in [
            Activity::Working,
            Activity::Waiting,
            Activity::NeedsAttention,
            Activity::Done,
        ] {
            assert_eq!(Activity::parse(activity.as_str()), Some(activity));
        }
        assert_eq!(Activity::parse("finished"), None);
        assert_eq!(Activity::after_clean_turn(true), Activity::Done);
        assert_eq!(Activity::after_clean_turn(false), Activity::Waiting);
        assert!(Activity::Done.is_idle() && Activity::Waiting.is_idle());
        assert!(!Activity::Working.is_idle() && !Activity::NeedsAttention.is_idle());
    }

    fn facts(status: TaskStatus, activity: Option<Activity>) -> AttentionFacts {
        AttentionFacts {
            status,
            activity,
            held_prompt: false,
            browser_request: false,
            chain_blocked: false,
            plan_open: false,
            pull: None,
        }
    }

    /// The sidebar's "Needs you" rule (UI-005, #147): the counts on Home and the tabs use it.
    #[test]
    fn needs_you_follows_the_sidebar_rule() {
        use Activity as A;
        use PullSignal as P;
        let running = |activity| facts(TaskStatus::Running, Some(activity));
        // The task's own state.
        assert!(running(A::NeedsAttention).needs_you());
        assert!(running(A::Waiting).needs_you(), "Your turn needs you");
        assert!(!running(A::Working).needs_you());
        assert!(!running(A::Done).needs_you());
        assert!(!facts(TaskStatus::Running, None).needs_you());
        // Paused for capacity, queued or switching: nobody needs to act.
        for status in [
            TaskStatus::WaitingCapacity,
            TaskStatus::Queued,
            TaskStatus::Switching,
            TaskStatus::Starting,
        ] {
            assert!(
                !facts(status, Some(A::NeedsAttention)).needs_you(),
                "{status:?}"
            );
        }
        // A chain waits by itself, unless its predecessor ended otherwise.
        let chained = facts(TaskStatus::Chained, None);
        assert!(!chained.needs_you());
        assert!(
            AttentionFacts {
                chain_blocked: true,
                ..chained
            }
            .needs_you()
        );
        // A held prompt needs a decision, except on an ended task.
        assert!(
            AttentionFacts {
                held_prompt: true,
                ..running(A::Working)
            }
            .needs_you()
        );
        assert!(
            !AttentionFacts {
                held_prompt: true,
                ..facts(TaskStatus::Closed, None)
            }
            .needs_you()
        );
        assert!(!facts(TaskStatus::Stopped(StopReason::Crashed), Some(A::Waiting)).needs_you());
        // A pending browser request needs a person whatever the activity or pull request says.
        let asked = |activity| AttentionFacts {
            browser_request: true,
            pull: Some(P::Tested),
            ..running(activity)
        };
        assert!(asked(A::Done).needs_you() && asked(A::Working).needs_you());
        assert!(
            AttentionFacts {
                browser_request: true,
                ..facts(TaskStatus::WaitingCapacity, None)
            }
            .needs_you()
        );
        assert!(
            !AttentionFacts {
                browser_request: true,
                ..facts(TaskStatus::Closed, None)
            }
            .needs_you()
        );
        // After a finished turn the pull request speaks: failing checks need you; a tested,
        // reviewed or merged one makes the task Done or In review while the plan is complete.
        let with = |activity, pull, plan_open| AttentionFacts {
            pull: Some(pull),
            plan_open,
            ..running(activity)
        };
        assert!(with(A::Waiting, P::Failing, false).needs_you());
        assert!(with(A::Done, P::Failing, false).needs_you());
        for pull in [P::Tested, P::InReview, P::Merged] {
            assert!(!with(A::Waiting, pull, false).needs_you(), "{pull:?}");
            // Open plan steps keep it out of Done: the task's own state shows (#139).
            assert!(
                with(A::Waiting, pull, true).needs_you(),
                "{pull:?} with open steps"
            );
            assert!(
                !with(A::Done, pull, true).needs_you(),
                "an achieved goal stays Done"
            );
        }
        // While the agent works, the pull request does not count.
        assert!(!with(A::Working, P::Failing, false).needs_you());
        assert!(with(A::NeedsAttention, P::Tested, false).needs_you());
    }

    #[test]
    fn pull_signals_from_wire_names() {
        assert_eq!(
            PullSignal::from_wire("merged", "none"),
            Some(PullSignal::Merged)
        );
        assert_eq!(
            PullSignal::from_wire("open", "success"),
            Some(PullSignal::Tested)
        );
        assert_eq!(
            PullSignal::from_wire("open", "none"),
            Some(PullSignal::Tested)
        );
        assert_eq!(
            PullSignal::from_wire("open", "failure"),
            Some(PullSignal::Failing)
        );
        assert_eq!(
            PullSignal::from_wire("open", "pending"),
            Some(PullSignal::InReview)
        );
        assert_eq!(
            PullSignal::from_wire("open", "unknown"),
            Some(PullSignal::InReview)
        );
        assert_eq!(PullSignal::from_wire("closed", "none"), None);
    }

    #[test]
    fn attempt_cas() {
        assert!(AttemptState::Spawning.can_become(AttemptState::Live));
        assert!(AttemptState::Live.can_become(AttemptState::Killed));
        assert!(!AttemptState::Exited.can_become(AttemptState::Lost));
        assert!(!AttemptState::Live.can_become(AttemptState::Spawning));
    }
}
