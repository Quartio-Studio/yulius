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
    /// No Claude account was usable to run it (SPEC-246 CLA-005).
    NoAccount,
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
    /// Written now, started later (SPEC-105): it waits until a person starts it ("when I
    /// decide") or until its scheduled time. Owner: the person, or the background tick for a
    /// time. Exit: Start now or the time → `queued`; cancel → `stopped(cancelled)`. No timeout:
    /// without a time it never starts by itself.
    Prepared,
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
    /// A prepared task's time came, or a person started it now (SPEC-105).
    PreparedReleased,
    /// Before its first run a person changed how a waiting task starts: it now waits for a task
    /// or a rule (SPEC-126).
    ChainChosen,
    /// Before its first run a person changed how a waiting task starts: it now waits for them or
    /// a time (SPEC-126).
    PreparedChosen,
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
            (S::Queued | S::Chained | S::Prepared, E::CancelRequested) => {
                S::Stopped(StopReason::Cancelled)
            }
            (S::Chained, E::ChainReleased) | (S::Prepared, E::PreparedReleased) => S::Queued,
            (S::Chained | S::Prepared, E::ChainChosen) => S::Chained,
            (S::Chained | S::Prepared, E::PreparedChosen) => S::Prepared,
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

    /// The status after the run of a task whose activity was `activity` was lost for `reason`
    /// (SPEC-229 RBT-001): a running task whose goal was achieved and whose turn ended (`Done`)
    /// lost nothing with its idle session and is Closed, as a person's close leaves it; any other
    /// takes [`TaskEvent::Lost`].
    ///
    /// # Errors
    /// As [`TaskStatus::apply`] for [`TaskEvent::Lost`].
    pub fn after_loss(
        self,
        activity: Option<Activity>,
        reason: StopReason,
    ) -> Result<Self, TransitionError> {
        if self == Self::Running && activity == Some(Activity::Done) {
            return Ok(Self::Closed);
        }
        self.apply(TaskEvent::Lost(reason))
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
            Self::Failed(FailReason::NoAccount) => "failed_no_account",
            Self::Chained => "chained",
            Self::Prepared => "prepared",
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
pub const ALL_STATUSES: [TaskStatus; 20] = [
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
    TaskStatus::Failed(FailReason::NoAccount),
    TaskStatus::Chained,
    TaskStatus::Prepared,
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
    /// Open while Yulius CI could not run its checks (disk, a timeout, the host): not the code
    /// (SPEC-050 STA-101).
    Errored,
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
            ("open", "error") => Some(Self::Errored),
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
    /// The task asked a person to act and the request is pending: in the sandbox browser
    /// (BRW-004) or through `needs_you` (SPEC-134 CDT-004).
    pub browser_request: bool,
    /// A chained task whose predecessor ended other than Done (TSK-012).
    pub chain_blocked: bool,
    /// The task has a plan with steps not done or skipped yet (STP-001).
    pub plan_open: bool,
    /// The idle agent left background shells running: it wakes by itself when they report.
    pub background_shells: bool,
    /// The idle agent waits in `yulius wait` (its checks, its delivery or an armed merge): it
    /// goes on by itself when Yulius has news (SPEC-085 STT-002).
    pub yulius_wait: bool,
    /// Its pull request, when one is known.
    pub pull: Option<PullSignal>,
}

/// A task's group in the web UI's sidebar (SPEC-001 UI-050). Home's cards and the sandbox tabs
/// count three of them: "needs you", and "working" plus "queued", which together are the
/// sidebar's Working group (#170).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskGroup {
    /// Waits for a person ("Needs you").
    NeedsYou,
    /// Running or on its way: working, kept going by background shells, in review, starting,
    /// continuing on another subscription or paused for capacity.
    Working,
    /// The rest of the sidebar's Working group: in line for a slot, or chained after another task.
    Queued,
    /// Finished (TSK-014).
    Done,
    /// Stopping, closed, stopped or failed.
    Ended,
}

impl AttentionFacts {
    /// The task's sidebar group: the one rule of `taskInfo` in the web UI's `status.js`.
    ///
    /// A task being stopped or ended is Ended, whatever else holds. A held prompt or a pending
    /// browser request needs you. After a finished turn the pull request speaks: failing checks
    /// need you once the agent stopped (SPEC-085 STT-003: while its background work runs or it
    /// waits in `yulius wait` it goes on by itself, and the task is Working), a CI run that
    /// could not run keeps it Working (In review); with the plan complete, background shells
    /// keep it Working, an achieved goal makes it Done, an agent waiting in `yulius wait` (its
    /// armed merge) or checks still running (In review) keep it Working, and otherwise a tested or
    /// merged pull request makes it Done. Otherwise the task's own state: a
    /// permission prompt or "Your turn" needs you (unless the agent's background shells still
    /// run or it waits in `yulius wait`: it goes on by itself), an achieved goal is Done, a
    /// queued, chained or prepared task is Queued (a chain whose predecessor ended otherwise needs you), and
    /// the rest is Working.
    #[must_use]
    pub fn group(&self) -> TaskGroup {
        use TaskGroup as G;
        if self.status.is_resting() || self.status == TaskStatus::Stopping {
            return G::Ended;
        }
        if self.held_prompt || self.browser_request {
            return G::NeedsYou;
        }
        let finished =
            self.status == TaskStatus::Running && self.activity.is_some_and(Activity::is_idle);
        // The idle agent goes on by itself: its background work wakes it, or Yulius does.
        let goes_on = self.background_shells || self.yulius_wait;
        if finished && let Some(pull) = self.pull {
            if pull == PullSignal::Failing {
                return if goes_on { G::Working } else { G::NeedsYou };
            }
            // CI could not run: In review, nothing for a person yet (the agent runs it again).
            if pull == PullSignal::Errored {
                return G::Working;
            }
            if !self.plan_open {
                return if self.background_shells {
                    G::Working
                } else if self.activity == Some(Activity::Done) {
                    G::Done
                } else if self.yulius_wait || pull == PullSignal::InReview {
                    // Its agent waits for its checks or its armed merge (SPEC-130 DMD-003).
                    G::Working
                } else {
                    G::Done
                };
            }
        }
        match self.status {
            TaskStatus::Running => match self.activity {
                Some(Activity::Waiting | Activity::Done) if self.background_shells => G::Working,
                Some(Activity::Waiting) if self.yulius_wait => G::Working,
                Some(Activity::NeedsAttention | Activity::Waiting) => G::NeedsYou,
                Some(Activity::Done) => G::Done,
                Some(Activity::Working) | None => G::Working,
            },
            TaskStatus::Chained if self.chain_blocked => G::NeedsYou,
            TaskStatus::Queued | TaskStatus::Chained | TaskStatus::Prepared => G::Queued,
            TaskStatus::Starting | TaskStatus::Switching | TaskStatus::WaitingCapacity => {
                G::Working
            }
            TaskStatus::Stopping
            | TaskStatus::Closed
            | TaskStatus::Stopped(_)
            | TaskStatus::Failed(_) => G::Ended,
        }
    }

    /// Whether the task waits for a person: the sidebar's "Needs you" group, counted the same way
    /// on Home and the sandbox tabs.
    #[must_use]
    pub fn needs_you(&self) -> bool {
        self.group() == TaskGroup::NeedsYou
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

    /// SPEC-229 RBT-001: a lost run closes a finished task and stops any other.
    #[test]
    fn a_lost_run_closes_a_finished_task_and_stops_the_others() -> Result<(), TransitionError> {
        let lost = StopReason::ServerLost;
        assert_eq!(
            TaskStatus::Running.after_loss(Some(Activity::Done), lost)?,
            TaskStatus::Closed
        );
        for activity in [
            None,
            Some(Activity::Working),
            Some(Activity::Waiting),
            Some(Activity::NeedsAttention),
        ] {
            assert_eq!(
                TaskStatus::Running.after_loss(activity, lost)?,
                TaskStatus::Stopped(lost),
                "{activity:?}"
            );
        }
        // Only a running task is finished: a switch or a stop in flight was cut.
        for status in [TaskStatus::Switching, TaskStatus::Stopping] {
            assert_eq!(
                status.after_loss(Some(Activity::Done), StopReason::Lost)?,
                TaskStatus::Stopped(StopReason::Lost)
            );
        }
        assert!(
            TaskStatus::Closed
                .after_loss(Some(Activity::Done), lost)
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
    fn a_waiting_task_changes_how_it_starts() -> Result<(), TransitionError> {
        // SPEC-126 CHS-001: chained and prepared tasks swap waits; started ones keep theirs.
        for from in [TaskStatus::Chained, TaskStatus::Prepared] {
            assert_eq!(from.apply(TaskEvent::ChainChosen)?, TaskStatus::Chained);
            assert_eq!(from.apply(TaskEvent::PreparedChosen)?, TaskStatus::Prepared);
        }
        for from in [
            TaskStatus::Queued,
            TaskStatus::WaitingCapacity,
            TaskStatus::Starting,
            TaskStatus::Running,
            TaskStatus::Closed,
            TaskStatus::Stopped(StopReason::Cancelled),
        ] {
            for event in [TaskEvent::ChainChosen, TaskEvent::PreparedChosen] {
                assert!(from.apply(event).is_err(), "{from:?} {event:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn prepared_tasks_wait_for_a_person_or_their_time() -> Result<(), TransitionError> {
        // SPEC-105 PRP-003/PRP-004: Start now or the time releases it; Cancel ends it.
        assert_eq!(
            TaskStatus::Prepared.apply(TaskEvent::PreparedReleased)?,
            TaskStatus::Queued
        );
        assert_eq!(
            TaskStatus::Prepared.apply(TaskEvent::CancelRequested)?,
            TaskStatus::Stopped(StopReason::Cancelled)
        );
        for event in [
            TaskEvent::CapacityGranted,
            TaskEvent::ContinueRequested,
            TaskEvent::ChainReleased,
        ] {
            assert!(TaskStatus::Prepared.apply(event).is_err(), "{event:?}");
        }
        assert!(
            TaskStatus::Queued
                .apply(TaskEvent::PreparedReleased)
                .is_err()
        );
        assert!(
            TaskStatus::Chained
                .apply(TaskEvent::PreparedReleased)
                .is_err()
        );
        assert!(!TaskStatus::Prepared.is_live());
        assert!(!TaskStatus::Prepared.is_resting());
        assert_eq!(TaskStatus::parse("prepared"), Some(TaskStatus::Prepared));
        // In line with the queued tasks, never "Needs you" (PRP-005).
        let prepared = facts(TaskStatus::Prepared, None);
        assert_eq!(prepared.group(), TaskGroup::Queued);
        assert!(!prepared.needs_you());
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
            background_shells: false,
            yulius_wait: false,
            pull: None,
        }
    }

    /// SPEC-085 STT-002, STT-003 (task #63 on 2026-10-13: "Needs you" with failing checks while
    /// its agent had fixed them and waited for its review agent): "Needs you" means the agent
    /// stopped and cannot go on by itself. Its background work or a `yulius wait` keeps the idle
    /// task Working, whatever its checks or plan say; once nothing runs, failing checks and Your
    /// turn need you again.
    #[test]
    fn an_agent_that_goes_on_by_itself_needs_nobody() {
        use Activity as A;
        use PullSignal as P;
        use TaskGroup as G;
        let idle = |activity, pull, plan_open| AttentionFacts {
            pull,
            plan_open,
            ..facts(TaskStatus::Running, Some(activity))
        };
        let shells = |facts: AttentionFacts| AttentionFacts {
            background_shells: true,
            ..facts
        };
        let waits = |facts: AttentionFacts| AttentionFacts {
            yulius_wait: true,
            ..facts
        };
        // Failing checks while a background agent or shell runs, or while the agent waits for
        // Yulius: it sees them itself.
        for activity in [A::Waiting, A::Done] {
            for plan_open in [false, true] {
                let failing = idle(activity, Some(P::Failing), plan_open);
                assert_eq!(failing.group(), G::NeedsYou, "{activity:?}: stopped");
                assert_eq!(shells(failing).group(), G::Working, "{activity:?}");
                assert_eq!(waits(failing).group(), G::Working, "{activity:?}");
            }
        }
        // Waiting in `yulius wait` with steps still open (the merge step): not Your turn, with or
        // without a pull request.
        for pull in [None, Some(P::InReview), Some(P::Tested), Some(P::Merged)] {
            let waiting = idle(A::Waiting, pull, true);
            assert_eq!(waiting.group(), G::NeedsYou, "{pull:?}: your turn");
            assert_eq!(waits(waiting).group(), G::Working, "{pull:?}");
        }
        // An achieved goal stays Done while a leftover wait runs; a permission prompt still needs
        // you.
        assert_eq!(waits(idle(A::Done, None, false)).group(), G::Done);
        assert_eq!(
            waits(idle(A::NeedsAttention, Some(P::Failing), false)).group(),
            G::NeedsYou
        );
    }

    /// #474 (task #109 on 2026-10-14: Done while its agent waited in `yulius wait` for the merge
    /// it had armed, its pull request open with green checks): a complete plan and a tested pull
    /// request do not make the task Done while its agent goes on by itself. Only an achieved goal
    /// does.
    #[test]
    fn a_wait_for_the_merge_is_not_done_before_it() {
        use Activity as A;
        use PullSignal as P;
        use TaskGroup as G;
        for pull in [P::Tested, P::InReview, P::Merged] {
            let waiting = AttentionFacts {
                pull: Some(pull),
                yulius_wait: true,
                ..facts(TaskStatus::Running, Some(A::Waiting))
            };
            assert_eq!(waiting.group(), G::Working, "{pull:?}");
            let achieved = AttentionFacts {
                activity: Some(A::Done),
                ..waiting
            };
            assert_eq!(achieved.group(), G::Done, "{pull:?}: the goal is achieved");
            let stopped = AttentionFacts {
                yulius_wait: false,
                ..waiting
            };
            let expected = if pull == P::InReview {
                G::Working
            } else {
                G::Done
            };
            assert_eq!(stopped.group(), expected, "{pull:?}: no wait");
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
        // Background shells keep an idle agent going: it wakes by itself, so it is not "Your turn".
        let busy = |activity| AttentionFacts {
            background_shells: true,
            ..running(activity)
        };
        assert!(
            !busy(A::Waiting).needs_you(),
            "shells running: not your turn"
        );
        assert!(
            busy(A::NeedsAttention).needs_you(),
            "a prompt still needs you"
        );
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

    /// The sidebar's groups (`taskInfo`), which Home's cards and the sandbox tabs split into
    /// "needs you", "working" and "queued" (#170).
    #[test]
    fn group_follows_the_sidebar_rule() {
        use Activity as A;
        use PullSignal as P;
        use TaskGroup as G;
        let running = |activity| facts(TaskStatus::Running, activity);
        // The task's own state.
        assert_eq!(running(Some(A::Working)).group(), G::Working);
        assert_eq!(
            running(None).group(),
            G::Working,
            "running, no activity yet"
        );
        assert_eq!(running(Some(A::NeedsAttention)).group(), G::NeedsYou);
        assert_eq!(running(Some(A::Waiting)).group(), G::NeedsYou);
        assert_eq!(running(Some(A::Done)).group(), G::Done);
        // Background shells keep a finished turn going: Working, not Your turn or Done.
        for activity in [A::Waiting, A::Done] {
            let busy = AttentionFacts {
                background_shells: true,
                ..running(Some(activity))
            };
            assert_eq!(busy.group(), G::Working, "{activity:?}");
        }
        // On its way: Working; in line or after another task: Queued.
        for status in [
            TaskStatus::Starting,
            TaskStatus::Switching,
            TaskStatus::WaitingCapacity,
        ] {
            assert_eq!(facts(status, Some(A::NeedsAttention)).group(), G::Working);
        }
        assert_eq!(facts(TaskStatus::Queued, None).group(), G::Queued);
        let chained = facts(TaskStatus::Chained, None);
        assert_eq!(chained.group(), G::Queued);
        let blocked = AttentionFacts {
            chain_blocked: true,
            ..chained
        };
        assert_eq!(blocked.group(), G::NeedsYou);
        // A stop asked for, or an end: Ended, even with a held prompt or a browser request.
        for status in [
            TaskStatus::Stopping,
            TaskStatus::Closed,
            TaskStatus::Stopped(StopReason::Crashed),
            TaskStatus::Failed(FailReason::SpawnError),
        ] {
            let held = AttentionFacts {
                held_prompt: true,
                browser_request: true,
                ..facts(status, Some(A::Waiting))
            };
            assert_eq!(held.group(), G::Ended, "{status:?}");
        }
        // A held prompt or a browser request needs you, also while queued or paused.
        for status in [TaskStatus::Queued, TaskStatus::WaitingCapacity] {
            let held = AttentionFacts {
                held_prompt: true,
                ..facts(status, None)
            };
            assert_eq!(held.group(), G::NeedsYou, "{status:?}");
        }
        // After a finished turn the pull request speaks (#35, #139).
        let with = |activity, pull, plan_open| AttentionFacts {
            pull: Some(pull),
            plan_open,
            ..running(Some(activity))
        };
        assert_eq!(with(A::Waiting, P::Failing, false).group(), G::NeedsYou);
        assert_eq!(with(A::Waiting, P::Tested, false).group(), G::Done);
        assert_eq!(with(A::Waiting, P::Merged, false).group(), G::Done);
        assert_eq!(
            with(A::Waiting, P::InReview, false).group(),
            G::Working,
            "In review"
        );
        assert_eq!(
            with(A::Done, P::InReview, false).group(),
            G::Done,
            "an achieved goal"
        );
        assert_eq!(with(A::Waiting, P::Tested, true).group(), G::NeedsYou);
        let shells = AttentionFacts {
            background_shells: true,
            ..with(A::Waiting, P::Tested, false)
        };
        assert_eq!(shells.group(), G::Working);
        // Yulius CI could not run (not the code): In review, before the plan or an achieved goal
        // speaks (#352); nothing for a person yet.
        assert_eq!(with(A::Waiting, P::Errored, true).group(), G::Working);
        assert_eq!(with(A::Done, P::Errored, false).group(), G::Working);
        assert_eq!(with(A::Waiting, P::Errored, false).group(), G::Working);
        // While the agent works, the pull request does not count.
        assert_eq!(with(A::Working, P::Failing, false).group(), G::Working);
    }

    /// A task being stopped is with the ended ones, as in the sidebar: a held prompt or a browser
    /// request on it no longer needs you (#170).
    #[test]
    fn a_task_being_stopped_needs_nobody() {
        let stopping = AttentionFacts {
            held_prompt: true,
            browser_request: true,
            ..facts(TaskStatus::Stopping, Some(Activity::NeedsAttention))
        };
        assert!(!stopping.needs_you());
        assert_eq!(stopping.group(), TaskGroup::Ended);
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
        assert_eq!(
            PullSignal::from_wire("open", "error"),
            Some(PullSignal::Errored)
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
