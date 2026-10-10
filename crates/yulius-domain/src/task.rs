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
    /// No account has capacity. Owner: rotation controller; re-evaluated at resets.
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
            (S::Running, E::SwitchStarted) | (S::WaitingCapacity, E::SwitchStarted) => S::Switching,
            (S::Running, E::CapacityExhausted) | (S::Switching, E::CapacityExhausted) => {
                S::WaitingCapacity
            }
            (S::Switching, E::SwitchCompleted) => S::Running,
            (S::Switching, E::SwitchAborted(reason)) => S::Stopped(reason),
            (S::WaitingCapacity, E::CancelRequested) => S::Stopped(StopReason::Cancelled),
            (S::Stopping, E::Reaped | E::Exited { .. }) => S::Stopped(StopReason::Cancelled),
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
pub const ALL_STATUSES: [TaskStatus; 17] = [
    TaskStatus::Queued,
    TaskStatus::Starting,
    TaskStatus::Running,
    TaskStatus::Switching,
    TaskStatus::WaitingCapacity,
    TaskStatus::Stopping,
    TaskStatus::Closed,
    TaskStatus::Stopped(StopReason::Cancelled),
    TaskStatus::Stopped(StopReason::Crashed),
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
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "working" => Some(Self::Working),
            "waiting" => Some(Self::Waiting),
            "needs_attention" => Some(Self::NeedsAttention),
            _ => None,
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
    fn attempt_cas() {
        assert!(AttemptState::Spawning.can_become(AttemptState::Live));
        assert!(AttemptState::Live.can_become(AttemptState::Killed));
        assert!(!AttemptState::Exited.can_become(AttemptState::Lost));
        assert!(!AttemptState::Live.can_become(AttemptState::Spawning));
    }
}
