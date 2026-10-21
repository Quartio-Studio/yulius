//! Test-only pause points between a check of a folder a contained agent can change and its use
//! (SPEC-220, ADR-0174). A test registers a hook that swaps a folder for a link exactly there, so
//! the race is reproduced deterministically: no sleeps, no rename loops. A stop's point between its
//! read of the task and its status change works the same way, keyed by the task id (SPEC-250).
//! Without the
//! `test-support` feature (production builds) the pause points do nothing.

use std::path::Path;

/// Where the server pauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// `file_read`: the path was resolved and checked, the file is not opened yet.
    FileRead,
    /// `file_write`: the parent folder was resolved and checked, nothing is written yet.
    FileWrite,
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
