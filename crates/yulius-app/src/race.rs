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
