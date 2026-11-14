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
    /// `close_task_now` (SPEC-250): the task was read `Running`; the close's end intent is not set
    /// yet. The path is the task id.
    CloseDecided,
    /// A switch (SPEC-250): the old run ended and the status the switch goes on from was read; the
    /// launch has not taken the launch lock yet. The path is the task id.
    SwitchLaunching,
    /// A switch (SPEC-250) knows it was overtaken; it has not said why yet. The path is the task
    /// id.
    SwitchOvertaken,
    /// A run's end reached its recorder (SPEC-483): the task's end intent was taken; nothing is
    /// recorded yet. The path is the task id.
    RunEndTaken,
    /// A switch's wait for its old run ran out (SPEC-483); it has not looked whose the run's
    /// end is yet. The path is the task id.
    SwitchWaitOver,
    /// A switch gave its old run up (SPEC-483): the task's end intent says so; the run's host
    /// is not terminated yet. The path is the task id.
    SwitchEndsRun,
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
    /// The leftovers view (SPEC-329's CI, #923): the kept sandboxes' store rows were read; the
    /// scan's state is not read yet. The path is the data directory.
    LeftoversView,
    /// `files_roots` (SPEC-340): the sandbox repository was found; its `.git/HEAD` is not opened
    /// yet. The path is the repository's.
    RootsHead,
    /// A Stop hook (SPEC-346): the task was read; the Stop has not taken the turn lock yet. The
    /// path is the task id.
    StopTurnLock,
    /// A prompt hook (SPEC-346) passed its turn-end check under the turn lock; it has not set
    /// Working yet. The path is the task id.
    PromptTurnChecked,
    /// The transcript watcher (SPEC-453): a prompt it read passed its last-Stop check under the
    /// turn lock; nothing of the new turn is set yet. The path is the task id.
    UserTurnChecked,
    /// A Stop hook (SPEC-453) found its turn ended cleanly; it has not taken the turn lock to
    /// remember when, or settled the turn, yet. The path is the task id.
    StopClean,
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
    /// A usage limit Claude reported (SPEC-464) is about to be recorded on its subscription: its
    /// stored usage is not read yet. The path is the account id.
    LimitRecord,
    /// A subscription's usage is about to be stored (SPEC-464, #1211): its status was read, to
    /// be stored with the usage; nothing is written yet. The path is the account id.
    UsageStatusRead,
    /// A subscription Claude refused for good is about to be marked as needing a new sign-in
    /// (#1211): nothing is read or written yet. The path is the account id.
    AccessLostRecord,
    /// A turn's end (SPEC-461) looked at the background work of a session on an older Claude
    /// Code, whatever it found; who typed and what pauses it are not read again yet. The path is
    /// the task id.
    ClaudeMoveProbed,
    /// A turn's end at which the pool wants the session on another subscription (SPEC-479) read
    /// the turn's lines and looked at its background work; whether a prompt waits is not asked
    /// yet. The path is the task id.
    SwitchProbed,
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
pub(crate) fn reached(point: Point, path: &Path) {
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
