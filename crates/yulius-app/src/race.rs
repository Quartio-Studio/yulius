//! Test-only pause points between a check of a folder a contained agent can change and its use
//! (SPEC-220, ADR-0174). A test registers a hook that swaps a folder for a link exactly there, so
//! the race is reproduced deterministically: no sleeps, no rename loops. The points of a stop, a
//! run's end, a switch and a close, between a read of the task and the change made from it, work
//! the same way, keyed by the task id (SPEC-250). Without the `test-support` feature (production
//! builds) the pause points do nothing.

use std::path::Path;

/// Where the server pauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// An owner requested continuation of an ended task.
    OwnerContinueRequested,
    /// The owner continuation has committed, before the supervisor prepares its launch.
    OwnerContinueAccepted,
    /// A start finished its preflight outside the launch lock.
    StartPreflightFinished,
    /// The exit reader recorded the attempt outcome before finishing terminal cleanup.
    RunReaped,
    /// A continuation is waiting for the previous reader's cleanup.
    ContinueWaitsForReader,
    /// A question’s answer passed task authorization, before the current task is read.
    AnswerAuthorized,
    /// A bulk message, Telegram reply or remote restart is about to dispatch a selected task.
    SelectedTaskDispatched,
    /// An owner's message selected its parked task before caller preflight.
    ParkedMessageSelected,
    /// The parked predecessor's local work is settled and its successor needs hosted PR facts.
    ParkedPredecessorChecksPr,
    /// SQLite committed this attempt's terminal outcome, before returning to its observer.
    TerminalOutcomeRecorded,
    /// A host exit was received, before the run reader enters finalization.
    HostExitReceived,
    /// A host exit reader finished finalizing its attempt.
    HostExitFinalized,
    /// A parked resume completed preflight, before it owns the launch lock.
    ParkedResumePrepared,
    /// The parked status is durable, just before the reader restores the saved grid.
    ParkStateRecorded,
    /// A new merge request owns the task’s arm writer before changing the slot.
    ArmWriteStarted,
    /// Continue waits for the previous attempt's reader to finish writing its task state.
    ParkedResumeWaitsForReader,
    /// A parked read classified the task, before the response renders its plan. Keyed by task ID.
    ParkedStatusRead,
    /// Local capacity is about to read the achieved owner’s delivery, keyed by task ID.
    CandidateCapacityRead,
    /// The free capacity has been verified, before the launch reserves its slot.
    CandidateCapacityCleared,
    /// A compound start rule is reading a named predecessor. Keyed by task ID.
    RuleOwnerRead,
    /// The watch started a hosted re-read of the owner's failed candidate. Keyed by task ID.
    CandidateObservationRead,
    /// A queued row reads session capacity to explain its wait. Keyed by task ID.
    QueuedRowCapacity,
    /// The capacity worker selected a saved start and has not yet claimed the task.
    CapacityStartReady,
    /// The capacity worker confirmed its pause before reading the task state.
    CapacityPauseObserved,
    /// A capacity worker read an ended task and is about to clean up its observed pause.
    CapacityStaleCleanup,
    /// A start is about to claim its observed task state. Keyed by task ID.
    StartClaimReady,
    /// The supervisor released the launch lock after the attempted claim. Keyed by task ID.
    StartLockReleased,
    /// An idle running row reads its pending PR news. Keyed by task ID.
    LiveRowPrNews,
    /// The live watch has read an open PR, before returning its advice. Keyed by task ID.
    LivePrFactsRead,
    /// The armed watcher completed its local publication lookup before final recipient admission.
    ArmedNoticePublicationRead,
    /// A live PR notice was pasted into the attempt; its submit is still pending.
    PrNoticePasted,
    /// A free arm has been taken by the queue, before its merge is decided.
    FreeArmTaken,
    /// A review starts reading its Git head. Keyed by worktree path.
    ReviewHead,
    /// `file_read`: the path was resolved and checked, the file is not opened yet.
    FileRead,
    /// `file_write`: the parent folder was resolved and checked, nothing is written yet.
    FileWrite,
    /// `files_list` (SPEC-265): the folder was resolved and checked, it is not opened yet.
    FilesList,
    /// `file_create` of a folder (SPEC-265): the parent was resolved and checked, nothing is
    /// created yet.
    FileCreate,
    /// `file_delete` (SPEC-265): the parent was resolved and checked, nothing is removed yet.
    FileDelete,
    /// Removing a task's or a shell's attachments (SPEC-265): the sandbox directory was found,
    /// nothing below it is opened yet. The path is the owner's attachments folder.
    AttachmentsRemove,
    /// Settling a deleted task's attachments (SPEC-396): the folders were listed and the tasks
    /// read; nothing is moved yet, and each folder's task is asked for again. The path is the
    /// sandbox's `attachments/tasks` folder. Reached on a blocking thread.
    AttachmentsSettle,
    /// Pruning old drafts (SPEC-265): the sandbox directory was found, the drafts folder is not
    /// opened yet. The path is that folder.
    DraftsPrune,
    /// An upload's folder was resolved and checked; its temporary file is not created yet.
    UploadFolder,
    /// `finish_upload`: the body is in the temporary file; the folder is not checked again yet.
    UploadReceived,
    /// `finish_upload`: the folder was checked again; the file has no name yet.
    UploadPlace,
    /// `project_change` (SPEC-247): the files are written in the scratch worktree, nothing is
    /// committed yet.
    ChangeWritten,
    /// `stop_task_now` (SPEC-250): the task was read and the run's end intent set; its status is
    /// not changed yet. The path is the task id.
    StopDecided,
    /// The end of a task's run (SPEC-250): the task was read and its next status chosen; it is
    /// not changed yet. The path is the task id.
    EndDecided,
    /// A switch (SPEC-250): the task is `Switching`; the switch's end intent for the old run is
    /// not set yet. The path is the task id.
    SwitchDecided,
    /// A switch passed preflight and has yet to claim the task status. The path is the task id.
    SwitchPrepared,
    /// A subscription move has read the original capacity pause. Keyed by task id.
    MovePauseRead,
    /// A queued limit move is about to check its reporting attempt. The path is the task id.
    LimitMoveReady,
    /// The pool stored the limit pause and is finishing its notices. The path is the task id.
    LimitPauseStored,
    /// The queued limit rotation returned.
    LimitMoveFinished,
    /// `close_task_now` (SPEC-250): the task was read `Running`; the close's end intent is not set
    /// yet. The path is the task id.
    CloseDecided,
    /// A park's process probe found the session with no background work, before the park
    /// waits for the launch lock. Keyed by task id.
    ParkProbed,
    /// A park passed the idle checks and is about to claim the run. Keyed by task id.
    ParkPrepared,
    /// A park (SPEC-512 PRK-003): its end intent is set under the launch lock; the session has
    /// not been asked to end yet. The path is the task id.
    ParkDecided,
    /// A cold terminal reader inspected the first snapshot filename. Keyed by run directory.
    ParkScreenRead,
    /// A live disconnected run was inspected and is about to be terminated. Keyed by run directory.
    LostRunTermination,
    /// A close or stop escalation has found its attempt live and is about to terminate it.
    /// The path is the task id.
    EndEscalating,
    /// `mark_lost`: the run's loss won its record; the run's end intent is not read yet. The
    /// path is the task id.
    LossRecorded,
    /// `answer_needs_you` (SPEC-512 ANS-004): the answer was typed into the session; the request
    /// it answers is not ended yet. The path is the task id.
    AnswerTyped,
    /// A switch (SPEC-250): the old run ended and the status the switch goes on from was read; the
    /// launch has not taken the launch lock yet. The path is the task id.
    SwitchLaunching,
    /// A switch (SPEC-250) knows it was overtaken; it has not said why yet. The path is the task
    /// id.
    SwitchOvertaken,
    /// Yulius typed a line of its own into a session (SPEC-483 TYP-002): the text is in the
    /// prompt box; its Enter is not sent yet. The path is the task id.
    LineTyped,
    /// Yulius is about to type a line of its own into a session (SPEC-483 TYP-002): the
    /// terminal is quiet and nobody typed; the typing mark is not taken yet. The path is the
    /// task id.
    LineReady,
    /// Yulius sent the Enter of a line it typed into a session (SPEC-483 TYP-002); what Claude
    /// Code does with it is not known yet. The path is the task id.
    LineEntered,
    /// The reader is about to take the run's end intent. The path is the task id.
    RunEndTaking,
    /// A run's end reached its recorder (SPEC-483): the task's end intent was taken; nothing is
    /// recorded yet. The path is the task id.
    RunEndTaken,
    /// A switch's wait for its old run ran out (SPEC-483); it has not looked whose the run's
    /// end is yet. The path is the task id.
    SwitchWaitOver,
    /// A switch gave its old run up (SPEC-483): the task's end intent says so; the run's host
    /// is not terminated yet. The path is the task id.
    SwitchEndsRun,
    /// A CI job is about to ask for admission (SPEC-497 HBA-002): its run's other jobs may ask
    /// first. The path is `<run reference>/<job key>` (`pr/1/b`).
    CiJobAdmission,
    /// A passed merge candidate (SPEC-497 MQW-002) comes to a member to merge: members before it
    /// merged; its own run is not read again yet. The path is the member's task id.
    TrainMemberMerging,
    /// A failed candidate has checked its members for failures and is deciding how to
    /// account for their runs. The path is the candidate run id.
    FailedTrainDeciding,
    /// Dissolution is about to write its CI and audit records. The path is the run id.
    TrainDissolving,
    /// Start onboarding (SPEC-256) read the sandbox `pending`; the start has not read it again
    /// yet. The path is the sandbox id.
    OnboardingApproved,
    /// The disk upkeep (SPEC-263) chose a cache to remove after its scan; it has not checked the
    /// path again nor removed it yet. The path is the cache's.
    CachePrune,
    /// A CI job's log (SPEC-285): its folder is open and the old log removed; the new one is not
    /// created yet. The path is the log's.
    CiLogCreate,
    /// A snapshot import (SPEC-296): its worktree's path is reserved; Git has not created the
    /// worktree there yet. The path is the reserved one.
    SnapshotWorktree,
    /// `attach_run` (SPEC-301): the run is the task's attached run, its screen and size the
    /// viewer's; a stop that came while it launched has not asked it to end yet. The path is the
    /// task id.
    RunAttached,
    /// A held tail of a task terminal's output is about to be shown, its quiet time over
    /// (SPEC-614, #1624). The path is the attempt id of the run that printed it.
    TerminalTailReady,
    /// A held tail's release is over, shown or dropped (SPEC-614). The path is the attempt id.
    TerminalTailDone,
    /// The leftovers view (SPEC-329's CI, #923): the kept sandboxes' store rows were read; the
    /// scan's state is not read yet. The path is the data directory.
    LeftoversView,
    /// A brokered run's row is stored (SPEC-626), before its command is read again and run. The
    /// path is the data directory.
    CommandRunStored,
    /// `files_roots` (SPEC-340): the sandbox repository was found; its `.git/HEAD` is not opened
    /// yet. The path is the repository's.
    RootsHead,
    /// A prompt is ending its draft, just before the old key's mark is removed.
    DraftMarkCleanup,
    /// A Stop hook (SPEC-346): the task was read; the Stop has not taken the turn lock yet. The
    /// path is the task id.
    StopTurnLock,
    /// The screen showed an empty box during an old-key hold. The typist is about to
    /// recheck under the draft lock before pasting. The path is the task id.
    BoxSeenEmpty,
    /// A terminal resize owns the run writer. The path is the task id.
    TerminalResizeOwnsWriter,
    /// A prompt hook (SPEC-346) passed its turn-end check under the turn lock; it has not set
    /// Working yet. The path is the task id.
    PromptTurnChecked,
    /// The transcript watcher (SPEC-453): a prompt it read passed its last-Stop check under the
    /// turn lock; nothing of the new turn is set yet. The path is the task id.
    UserTurnChecked,
    /// A Stop hook (SPEC-453) found its turn ended cleanly; it has not taken the turn lock to
    /// remember when, or settled the turn, yet. The path is the task id.
    StopClean,
    /// A verdict or timeout found its Stop and is about to claim the turn for settlement.
    GoalVerdictReady,
    /// The watcher read a native goal record. The path identifies its attempt.
    NativeVerdictRead,
    /// The watcher finished applying a native goal record for this attempt.
    NativeVerdictChecked,
    /// `validate_checkout` (SPEC-394): the checkout's root was resolved and the file's path
    /// worked out; the file is not opened yet. The path is the file's.
    CheckoutRead,
    /// `put_import_chunk` (SPEC-400): the job was read and the chunk checked; the file is not
    /// locked or written yet. The path is the import file's.
    ImportChunkRead,
    /// `put_import_chunk` (SPEC-400): the chunk holds its file and is about to write it; reached
    /// on a blocking thread. The path is the import file's.
    ImportChunkWrite,
    /// `put_import_chunk` (SPEC-400): another chunk holds the file, so this one waits for it.
    /// The path is the import file's.
    ImportChunkWait,
    /// A worktree's removal (SPEC-401): Git's checks passed; nothing is removed yet. The path is
    /// the worktree's, then its Git record's. Reached on a blocking thread.
    WorktreeRemove,
    /// `read_repo_file` (SPEC-409): the path was resolved and found inside the task worktree; the
    /// file is not opened yet. The path is the resolved one.
    RepoFileRead,
    /// Joining through a guest link (SPEC-410): the guest's account exists; their session does
    /// not yet. The path is the link's id.
    GuestJoined,
    /// `launch_attempt` (SPEC-447): a subscription was chosen and its credential opened; the run's
    /// attempt is not recorded yet. The path is the task id.
    AccountChosen,
    /// After a Codex run (SPEC-503 CDX-006): its write-back kept what it keeps and removed what
    /// it removes, or left the file for another run. The path is the task id.
    CodexWriteBackDone,
    /// A Codex run's `auth.json` was read and found a genuine refresh (SPEC-503 CDX-006); it is
    /// not stored yet. The path is the account id.
    CodexRefreshTaken,
    /// `launch_attempt` (SPEC-458): the run's attempt is recorded; its host is not started or
    /// attached yet. The path is the task id.
    RunRecorded,
    /// The start-up reconciliation (SPEC-458): every run on record was gone through; the sweep
    /// of tasks without a run has not begun. The path is the data directory.
    ReconcileSweep,
    /// A change of a subscription's stored usage (SPEC-464: a provider's reading, a session's
    /// reading, a limit Claude reported): the stored usage was read under the lock such changes
    /// take; nothing is written yet. The path is the account id.
    UsageRead,
    /// Reattachment read the answerable attention reason but has not installed its time.
    /// The path is the task id.
    AttentionRecoveryRead,
    /// A replacement notice reached a busy attention lock. The path is the task id.
    AttentionReplacementWaiting,
    /// A usage limit Claude reported (SPEC-464) is about to be recorded on its subscription: its
    /// stored usage is not read yet. The path is the account id.
    LimitRecord,
    /// A subscription's usage is about to be stored (SPEC-464, #1211): its status was read, to
    /// be stored with the usage; nothing is written yet. The path is the account id.
    UsageStatusRead,
    /// A subscription Claude refused for good is about to be marked as needing a new sign-in
    /// (#1211): nothing is read or written yet. The path is the account id.
    AccessLostRecord,
    /// An idle Cockpit finished looking for stray processes, including a probe with no usable
    /// answer. The path is its transcript.
    CockpitStraysProbed,
    /// A turn's end (SPEC-461) looked at the background work of a session on an older Claude
    /// Code, whatever it found; who typed and what pauses it are not read again yet. The path is
    /// the task id.
    ClaudeMoveProbed,
    /// A turn's end at which the pool wants the session on another subscription (SPEC-479) read
    /// the turn's lines and looked at its background work; whether a prompt waits is not asked
    /// yet. The path is the task id.
    SwitchProbed,
    /// A review round's move for a Fable reviewer (SPEC-493 REV-007) was decided and its switch
    /// spawned; whether the run it was decided for is still the live one is not asked yet. The
    /// path is the task id.
    ReviewMoveDecided,
    /// A CI sync of a sandbox read its runs and planned what to queue; nothing is queued yet.
    /// The path is the sandbox id for inventory, or the clone for a single opening.
    CiSyncPlanned,
    /// The pool found a prompt waiting in a turn held for capacity that it would move
    /// (SPEC-479): the turn's lines were read; the pause is not asked for again yet and nothing
    /// is released. The path is the task id.
    HeldTurnProbed,
    /// A turn's end decided to restart its session on a newer Claude Code (SPEC-468); the
    /// restart is not registered or asked for yet. The path is the task id.
    ClaudeMoveDecided,
    /// A restart on a newer Claude Code (SPEC-461) launched the session's new run; the version
    /// that run is taken to run until its transcript names one is not written yet. The path is
    /// the task id.
    ClaudeMoveRestarted,
    /// A session's start was noticed (its SessionStart hook; SPEC-468): a restart on a newer
    /// Claude Code that waited for it is off the breaker's watch. The path is the task id.
    ClaudeMoveStarted,
    /// A settled turn (SPEC-461) decided whether its idle session restarts on a newer Claude
    /// Code, and made that restart if so. The path is the task id.
    ClaudeMoveSettled,
    /// The time a session restarted on a newer Claude Code had to start there passed (SPEC-461),
    /// and whether that version gets other sessions was decided. The path is the task id.
    ClaudeMoveStartJudged,
    /// The skill catalog is about to open a `SKILL.md` it resolved and checked (SPEC-507 review
    /// round 2): nothing is opened yet. The path is the file. Reached on a blocking thread.
    SkillFileOpen,
    /// A refresh of a sandbox's installed skills has not asked for the launch lock yet, nor read
    /// its set (SPEC-507 SKR-005). The path is the sandbox directory.
    SkillsRefreshQueued,
    /// A refresh holds the launch lock; nothing of the home is read yet (SPEC-507 SKR-005). The
    /// path is the sandbox directory. Reached on a blocking thread.
    SkillsRefreshLocked,
    /// A fork's task exists; its record, transcript copy and plan are not written yet (SPEC-628
    /// FRK-001). The path is the sandbox directory.
    ForkInserted,
    /// A finished task's staging outcome is to be typed because its background work runs; its
    /// admission has not looked again yet (SPEC-631 SWK-001). Keyed by the task id.
    AwaitedOutcomeDecided,
    /// A hook reached `hook_event`; nothing of it is read yet, not even the time (SPEC-636). The
    /// path is the hook's session id joined with its event, so a test holds one hook of one run
    /// as a loaded server does after its client gave up.
    HookArrived,
}

#[cfg(feature = "test-support")]
type Hook = Box<dyn FnOnce() + Send>;

#[cfg(feature = "test-support")]
static HOOKS: std::sync::Mutex<Vec<(Point, std::path::PathBuf, Hook)>> =
    std::sync::Mutex::new(Vec::new());

/// Runs `hook` once, the next time the server reaches `point` for a path below `below` (so tests
/// running in parallel threads do not take each other's hooks).
#[cfg(feature = "test-support")]
pub fn on(point: Point, below: &Path, hook: impl FnOnce() + Send + 'static) {
    let below = std::fs::canonicalize(below).unwrap_or_else(|_| below.to_path_buf());
    HOOKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((point, below, Box::new(hook)));
}

/// The server reached `point` for `path`: runs the hook registered for it, if any.
pub fn reached(point: Point, path: &Path) {
    #[cfg(feature = "test-support")]
    {
        let hook = {
            let mut hooks = HOOKS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            hooks
                .iter()
                .position(|(at, below, _)| *at == point && path.starts_with(below))
                .map(|index| hooks.remove(index).2)
        };
        if let Some(hook) = hook {
            hook();
        }
    }
    #[cfg(not(feature = "test-support"))]
    let _ = (point, path);
}
