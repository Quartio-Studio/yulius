//! Leftovers on the server's disk (SPEC-215, ADR-0170): paths under the data directory that no
//! record owns, or whose owner's state says they are disposable. A scan lists them by kind
//! ([`Kind`]); a deletion checks each item's rule again on fresh state right before removing it
//! (a sandbox that is still running, a holder that is alive, a task that started working keeps
//! its files), audits it (`leftovers.delete`, `leftovers.failed`) and treats a missing path as
//! deleted, so an interrupted round resumes on the next one.
//!
//! The automatic round ([`App::start_leftovers_rounds`]) runs at every start, which is when an
//! update lands, and every hour; it deletes only the kinds that need no person. The superadmin
//! sees and deletes them in Settings → Instance, and root or the service user with
//! `yulius leftovers`.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yulius_contracts::api::leftovers::{
    DeleteLeftoversRequest, LeftoverJobView, LeftoverView, LeftoversView,
};
use yulius_domain::disk::human;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId};
use yulius_domain::leftovers::{
    AUTO_EVERY_MS, DRAFT_MAX_AGE_MS, ENDED_RUN_AFTER_MS, Kind, STALE_CHECKOUT_AFTER_MS,
    checkout_job_key, item_id, removed_long_enough, smoke_pid,
};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::disk::dir_size;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// First automatic round after a start: holders are adopted and runs recovered by then.
pub const AUTO_FIRST: Duration = Duration::from_secs(2 * 60);
/// A worktree directory no task owns is listed once it is this old (a task's worktree is made
/// before its record only for a moment).
const ORPHAN_WORKTREE_AFTER_MS: i64 = 60 * 60 * 1000;
/// CI runs read when looking for the jobs a sandbox still runs.
const CI_RUNS_READ: u32 = 1000;
/// The host's own Docker data, reported only (ADR-0170).
const HOST_DOCKER: [&str; 2] = ["/var/lib/docker", "/var/lib/containerd"];

/// The file in the data directory that holds the key of `yulius leftovers` (hex, mode 0600,
/// new at every start): root and the service user read it, sandboxed runs cannot.
pub const CLI_KEY_FILE: &str = "leftovers-cli.key";

/// One leftover found by a scan.
#[derive(Debug, Clone)]
pub struct Leftover {
    /// Stable id ([`item_id`] of the path).
    pub id: String,
    /// What it is.
    pub kind: Kind,
    /// The path deleted.
    pub path: PathBuf,
    /// Allocated bytes (with `extra`); `None` when unreadable.
    pub bytes: Option<u64>,
    /// Its sandbox.
    pub sandbox: Option<SandboxId>,
    /// Its sandbox's slug.
    pub slug: Option<String>,
    /// Its task.
    pub task: Option<TaskId>,
    /// Its task's number.
    pub task_number: Option<u32>,
    /// Why it is a leftover.
    pub reason: String,
    /// Further paths deleted with it (everything of a removed sandbox).
    extra: Vec<PathBuf>,
    /// The clone whose worktree records are pruned after it goes.
    repo: Option<PathBuf>,
}

impl Leftover {
    fn new(kind: Kind, path: PathBuf, reason: impl Into<String>) -> Self {
        Self {
            id: item_id(&path.to_string_lossy()),
            kind,
            path,
            bytes: None,
            sandbox: None,
            slug: None,
            task: None,
            task_number: None,
            reason: reason.into(),
            extra: Vec::new(),
            repo: None,
        }
    }

    fn of(mut self, sandbox: &SandboxRecord) -> Self {
        self.sandbox = Some(sandbox.id.clone());
        self.slug = Some(sandbox.slug.clone());
        self
    }

    fn view(&self) -> LeftoverView {
        LeftoverView {
            id: self.id.clone(),
            kind: self.kind.as_str().to_owned(),
            path: self.path.to_string_lossy().into_owned(),
            bytes: self.bytes,
            sandbox: self.slug.clone(),
            task: self.task_number.map(u64::from),
            reason: self.reason.clone(),
            auto: self.kind.auto(),
            deletable: self.kind.deletable(),
        }
    }
}

/// The last scan and the running or last job.
#[derive(Default)]
pub struct LeftoverState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Grows with every job started (a job's own [`JobEnd`] knows it).
    generation: u64,
    /// The CLI's key, as written to [`CLI_KEY_FILE`].
    key: Option<String>,
    items: Vec<Leftover>,
    scanned_at: Option<Timestamp>,
    job: Option<LeftoverJobView>,
    next_auto_at: Option<Timestamp>,
}

/// What a deletion did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteReport {
    /// Items deleted.
    pub deleted: usize,
    /// Bytes freed.
    pub freed: u64,
    /// Items kept: "path: why".
    pub kept: Vec<String>,
    /// Failures: "path: error".
    pub failed: Vec<String>,
}

/// Which items a deletion takes.
#[derive(Debug, Clone)]
pub enum Selection {
    /// These ids of the last scan.
    Ids(Vec<String>),
    /// Every item the automatic rounds delete.
    AllSafe,
}

impl From<DeleteLeftoversRequest> for Selection {
    fn from(request: DeleteLeftoversRequest) -> Self {
        if request.all_safe {
            Self::AllSafe
        } else {
            Self::Ids(request.ids)
        }
    }
}

/// A CI claim held while a sandbox's checkouts go, so no run starts in them half deleted.
struct CiClaim<'a> {
    app: &'a App,
    sandbox: SandboxId,
}

impl Drop for CiClaim<'_> {
    fn drop(&mut self) {
        self.app.ci_release(&self.sandbox);
    }
}

/// Ends the job when whatever ran it stopped without ending it (a panic, a dropped task), so
/// the slot never stays taken (later rounds and requests would be refused until a restart).
/// Disarmed by the normal end; it only ever ends its own job (its generation), never one started
/// after it.
struct JobEnd<'a>(&'a App, u64);

impl Drop for JobEnd<'_> {
    fn drop(&mut self) {
        let generation = self.1;
        self.0.with_leftovers(|inner| {
            if inner.generation == generation
                && let Some(job) = inner.job.as_mut().filter(|job| job.running)
            {
                job.running = false;
                job.failed.push("the job stopped before its end".into());
            }
        });
    }
}

/// What one deletion pass reads once: the CI job keys each sandbox used.
#[derive(Default)]
struct Pass {
    ci_keys: std::collections::HashMap<SandboxId, Option<HashSet<String>>>,
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The newest modification time (ms) of `path` and its direct entries.
fn touched_ms(path: &Path) -> Option<i64> {
    let own = std::fs::symlink_metadata(path).ok()?.mtime() * 1000;
    let newest = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok().map(|m| m.mtime() * 1000))
        .max()
        .unwrap_or(own);
    Some(own.max(newest))
}

fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    found.sort();
    found
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn pid_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Allocated bytes of `path` (a file or a tree); `None` when it cannot be read at all.
fn size_of(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_dir() {
        std::fs::read_dir(path).ok()?;
        Some(dir_size(path))
    } else {
        Some(meta.blocks().saturating_mul(512))
    }
}

/// Whether every directory from `root` (exclusive) down to `path`'s parent is a real directory,
/// not a link: a sandbox's agents and shells can replace a directory of their sandbox with a link,
/// which would steer a removal elsewhere (review of SPEC-215). `path` itself may be a link (it is
/// removed as a link). A path that is not below `root` fails.
pub(crate) fn no_link_between(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let parts: Vec<Component<'_>> = relative.components().collect();
    if parts.is_empty() || parts.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return false;
    }
    let mut walked = root.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        walked.push(part);
        match std::fs::symlink_metadata(&walked) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            _ => return false,
        }
    }
    true
}

/// Removes `path` (a file, a link or a tree) below `root` without ever following a link on the
/// way: each directory from `root` down is opened with `O_NOFOLLOW`, so a directory a sandbox
/// swapped for a link (also after the last check) fails the open instead of steering the removal
/// elsewhere. The removal then goes through the opened directory, handle by handle
/// (`remove_tree_at`), never following a link below it, and an error names the entry that
/// stayed (#675). A missing path is fine. It is also the body
/// of `yulius sandbox-host remove-tree --below`, run as root of a sandbox's mapped namespace for
/// files of its mapped ids (ADR-0176).
///
/// # Errors
/// `path` is not below `root`, a link lies on the way, or the removal failed.
pub fn remove_below(root: &Path, path: &Path) -> std::io::Result<()> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::Mode;
    use std::os::fd::AsRawFd as _;
    let refused = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not below {}", root.display()),
        )
    };
    let relative = path.strip_prefix(root).map_err(|_| refused())?;
    let parts: Vec<&std::ffi::OsStr> = relative
        .components()
        .map(|c| match c {
            Component::Normal(part) => Ok(part),
            _ => Err(refused()),
        })
        .collect::<Result<_, _>>()?;
    let (last, ancestors) = parts.split_last().ok_or_else(refused)?;
    let flags = OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut dir = open(root, OFlag::O_DIRECTORY | OFlag::O_CLOEXEC, Mode::empty())
        .map_err(std::io::Error::from)?;
    for part in ancestors {
        dir = match openat(&dir, *part, flags, Mode::empty()) {
            Ok(next) => next,
            Err(nix::errno::Errno::ENOENT) => return Ok(()),
            Err(err) => {
                return Err(std::io::Error::other(format!(
                    "{} on the way to {}: {err}",
                    part.to_string_lossy(),
                    path.display()
                )));
            }
        };
    }
    let through = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(last);
    let meta = match std::fs::symlink_metadata(&through) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let removed = if meta.is_dir() {
        // A tree a tool made read-only is made writable through directory handles (never by
        // path: a sandbox could swap a directory for a link between a check and a chmod), then
        // removed again.
        remove_tree_at(&dir, last, path, 0).or_else(|_| {
            if let Some(tree) = open_dir_handle(&dir, last) {
                make_writable_at(&tree, 0, &mut { MAX_DIRS_MADE_WRITABLE });
            }
            remove_tree_at(&dir, last, path, 0)
        })
    } else {
        // Only the item itself may have gone meanwhile; the tree's walk reports what stayed.
        match std::fs::remove_file(&through) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    };
    drop(dir);
    removed
}

/// Removes the directory `name` of `parent` and everything below it, each directory opened
/// through its parent's handle with `O_NOFOLLOW` and each entry removed with `unlinkat`, so a link
/// is removed itself, never followed. `shown` is the directory's path for errors, which name the
/// entry that could not be removed. Past [`MAX_WRITABLE_DEPTH`] levels (each holds a handle) std's
/// `remove_dir_all`, which never follows a link either, removes the rest through the parent's
/// handle. Entries gone meanwhile are fine.
fn remove_tree_at(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    shown: &Path,
    depth: usize,
) -> std::io::Result<()> {
    use nix::errno::Errno;
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;
    use nix::unistd::{UnlinkatFlags, unlinkat};
    use std::os::fd::AsRawFd as _;
    let named = |shown: &Path, err: std::io::Error| {
        std::io::Error::new(err.kind(), format!("{}: {err}", shown.display()))
    };
    if depth >= MAX_WRITABLE_DEPTH {
        let through = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
        return match std::fs::remove_dir_all(&through) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(named(shown, err)),
            _ => Ok(()),
        };
    }
    let flags = OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let dir = match openat(parent, name, flags, Mode::empty()) {
        Ok(dir) => dir,
        Err(Errno::ENOENT) => return Ok(()),
        // Swapped for a link or a file since it was listed: removed itself.
        Err(Errno::ELOOP | Errno::ENOTDIR) => {
            return match unlinkat(parent, name, UnlinkatFlags::NoRemoveDir) {
                Ok(()) | Err(Errno::ENOENT) => Ok(()),
                Err(err) => Err(named(shown, err.into())),
            };
        }
        Err(err) => return Err(named(shown, err.into())),
    };
    let through = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    for child in std::fs::read_dir(&through).map_err(|e| named(shown, e))? {
        let child = child.map_err(|e| named(shown, e))?;
        let child_name = child.file_name();
        let child_shown = shown.join(&child_name);
        if child.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_tree_at(&dir, &child_name, &child_shown, depth + 1)?;
        } else {
            match unlinkat(&dir, child_name.as_os_str(), UnlinkatFlags::NoRemoveDir) {
                Ok(()) | Err(Errno::ENOENT) => {}
                Err(err) => return Err(named(&child_shown, err.into())),
            }
        }
    }
    drop(dir);
    match unlinkat(parent, name, UnlinkatFlags::RemoveDir) {
        Ok(()) | Err(Errno::ENOENT) => Ok(()),
        Err(err) => Err(named(shown, err.into())),
    }
}

/// Directories [`make_writable_at`] changes at most in one removal.
const MAX_DIRS_MADE_WRITABLE: usize = 1_000_000;
/// Directories deep it goes at most (each level holds one open handle).
const MAX_WRITABLE_DEPTH: usize = 256;

/// Makes the directory `dir` and every directory below it writable and searchable for its owner
/// (a fixed 0o700) through their own handles: each subdirectory is opened relative to its
/// parent's handle by [`open_dir_handle`], entries are typed without following links, and the
/// mode is set by `chmod` on the handle's `/proc/self/fd` entry, which changes exactly the
/// directory the handle pinned (`fchmod` refuses an `O_PATH` handle), so a link never carries the
/// change outside the tree and a directory its owner cannot read (#680) is changed too. `budget`
/// bounds the directories changed.
fn make_writable_at(dir: &std::os::fd::OwnedFd, depth: usize, budget: &mut usize) {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::PermissionsExt as _;
    if depth > MAX_WRITABLE_DEPTH || *budget == 0 {
        return;
    }
    *budget -= 1;
    let through = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    let _ = std::fs::set_permissions(&through, std::fs::Permissions::from_mode(0o700));
    let Ok(children) = std::fs::read_dir(&through) else {
        return;
    };
    for child in children.flatten() {
        if !child.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let Some(opened) = open_dir_handle(dir, &child.file_name()) {
            make_writable_at(&opened, depth + 1, budget);
        }
    }
}

/// The directory `name` of `parent`, opened as a handle that needs no permission on the directory
/// itself (`O_PATH` on Linux; elsewhere it must be readable) and with `O_NOFOLLOW`: `None` when it
/// is gone, a link or no directory.
fn open_dir_handle(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> Option<std::os::fd::OwnedFd> {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::{Mode, SFlag, fstat};
    #[cfg(target_os = "linux")]
    let handle_only = OFlag::O_PATH;
    #[cfg(not(target_os = "linux"))]
    let handle_only = OFlag::empty();
    let flags = handle_only | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let handle = openat(parent, name, flags, Mode::empty()).ok()?;
    let kind = SFlag::from_bits_truncate(fstat(&handle).ok()?.st_mode) & SFlag::S_IFMT;
    (kind == SFlag::S_IFDIR).then_some(handle)
}

/// The directory a worktree's Git record (`.git/worktrees/<name>/gitdir`) points at, when that
/// directory lies inside `sandbox_dir` (where the host sees it) and is gone. A worktree outside it
/// (an agent's own `/tmp`, which only its run sees) or a locked record is never gone.
fn gone_worktree(record: &Path, sandbox_dir: &Path) -> Option<PathBuf> {
    if exists(&record.join("locked")) {
        return None;
    }
    let gitdir = std::fs::read_to_string(record.join("gitdir")).ok()?;
    let dot_git = PathBuf::from(gitdir.trim());
    let worktree = dot_git.parent()?.to_path_buf();
    // Inside the sandbox directory, with no link on the part of the way that still exists (a
    // directory deleted by hand on the way is fine: nothing can be linked there).
    let Ok(relative) = worktree.strip_prefix(sandbox_dir) else {
        return None;
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let mut walked = sandbox_dir.to_path_buf();
    for part in relative.components() {
        walked.push(part);
        match std::fs::symlink_metadata(&walked) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Some(worktree),
            _ => return None,
        }
    }
    None
}

/// Removes the Git records of `repo` whose worktrees lay under `removed` and are gone now (what
/// `git worktree prune` does for them, without touching any other record).
fn drop_records_under(root: &Path, repo: &Path, removed: &Path, sandbox_dir: &Path) {
    for (_, record) in entries(&repo.join(".git").join("worktrees")) {
        if let Some(worktree) = gone_worktree(&record, sandbox_dir)
            && worktree.starts_with(removed)
            && let Err(err) = remove_below(root, &record)
        {
            tracing::warn!(record = %record.display(), error = %err, "a worktree record stays");
        }
    }
}

impl App {
    fn data_path(&self, parts: &[&str]) -> PathBuf {
        let mut path = self.config.data_dir.clone();
        for part in parts {
            path.push(part);
        }
        path
    }

    /// Whether `path` lies inside the data directory with no `..` and no link on the way (every
    /// item is checked against it when listed and again right before its deletion).
    fn inside_data(&self, path: &Path) -> bool {
        no_link_between(&self.config.data_dir, path)
    }

    /// Scans the data directory for leftovers, with their sizes.
    ///
    /// # Errors
    /// Store failures.
    pub async fn leftovers_scan(&self) -> AppResult<Vec<Leftover>> {
        // Nothing below a link is ever listed (a link could point anywhere): say so when part of
        // the data directory itself is one (moved to another disk), or its leftovers stay unseen.
        for top in [
            "sandboxes",
            "sysroots",
            "runtime",
            "runs",
            "cd",
            "displays",
            "transcripts",
            "system",
        ] {
            let path = self.data_path(&[top]);
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                tracing::warn!(path = %path.display(), "a link in the data directory: no leftover below it is listed; bind-mount it instead");
            }
        }
        let store = &self.deps.store;
        let live = store.sandboxes().await?;
        let removed = store.removed_sandboxes().await?;
        let mut found = Vec::new();
        for sandbox in &removed {
            self.scan_removed(sandbox, &mut found).await?;
        }
        for sandbox in &live {
            self.scan_live(sandbox, &mut found).await?;
        }
        for sandbox in live.iter().chain(&removed) {
            self.scan_git_records(sandbox, &mut found);
        }
        let known: HashSet<String> = live
            .iter()
            .chain(&removed)
            .map(|s| s.id.to_string())
            .collect();
        self.scan_instance(&known, &mut found).await?;
        // One item per path: a removed sandbox's run directory is also an old one. Nothing
        // reached through a link is listed (it would point elsewhere); the host's own data is
        // reported, never deleted.
        let mut seen = HashSet::new();
        found.retain(|item: &Leftover| {
            let reachable = !item.kind.deletable()
                || (self.inside_data(&item.path) && item.extra.iter().all(|p| self.inside_data(p)));
            reachable && seen.insert(item.id.clone())
        });
        let measured = tokio::task::spawn_blocking(move || {
            for item in &mut found {
                if item.bytes.is_none() {
                    item.bytes = std::iter::once(&item.path)
                        .chain(&item.extra)
                        .filter(|p| exists(p))
                        .map(|p| size_of(p))
                        .try_fold(0u64, |sum, bytes| bytes.map(|b| sum.saturating_add(b)));
                }
            }
            found
        })
        .await
        .map_err(AppError::internal)?;
        Ok(measured)
    }

    /// A removed sandbox: its disposable parts once its rule holds, and everything else of it
    /// as one manual item.
    async fn scan_removed(
        &self,
        sandbox: &SandboxRecord,
        out: &mut Vec<Leftover>,
    ) -> AppResult<()> {
        let id = sandbox.id.as_str();
        let dir = self.sandbox_dir(&sandbox.id);
        let mut parts = vec![
            (Kind::RemovedWorktrees, dir.join("worktrees")),
            (Kind::RemovedCi, dir.join("ci")),
            (Kind::RemovedBrowser, dir.join("browser")),
            (Kind::RemovedCd, self.data_path(&["cd", id])),
            (Kind::RemovedDisplay, self.data_path(&["displays", id])),
            (Kind::RemovedSystem, self.data_path(&["sysroots", id])),
            (
                Kind::RemovedRuntime,
                self.data_path(&["runtime", "sandboxes", id]),
            ),
        ];
        let mut tasks = self.deps.store.tasks(&sandbox.id, false).await?;
        tasks.extend(self.deps.store.tasks(&sandbox.id, true).await?);
        let runs = self.data_path(&["runs"]);
        for task in &tasks {
            for attempt in self.deps.store.attempts(&task.id).await? {
                let run = PathBuf::from(&attempt.run_dir);
                if run.starts_with(&runs) && run != runs {
                    parts.push((Kind::EndedRun, run));
                }
            }
        }
        // Listed once its rule holds (a removed sandbox still stopping is not a leftover yet);
        // the system layer and state wait for the holder too.
        let blocked = self.removed_blocker(sandbox, None).await?.is_some();
        let holder = match self.isolation.get() {
            Some(isolation) => isolation.holder_live(id).await,
            None => false,
        };
        for (kind, path) in &parts {
            let waits_for_holder = matches!(kind, Kind::RemovedSystem | Kind::RemovedRuntime);
            if !exists(path) || blocked || (holder && waits_for_holder) {
                continue;
            }
            let reason = match kind {
                Kind::EndedRun => "an agent run of a removed sandbox".to_owned(),
                _ => format!("the sandbox {} was removed", sandbox.slug),
            };
            let mut item = Leftover::new(*kind, path.clone(), reason).of(sandbox);
            if *kind == Kind::RemovedWorktrees {
                item.repo = Some(self.sandbox_repo(&sandbox.id));
            }
            out.push(item);
        }
        // Everything else PRJ-009 keeps: a person may delete it, files only.
        let mut extra: Vec<PathBuf> = parts.into_iter().map(|(_, path)| path).collect();
        if sandbox.is_playground() {
            extra.push(self.playground_origin(&sandbox.id));
        }
        for task in &tasks {
            extra.push(self.data_path(&["transcripts", task.id.as_str()]));
        }
        let prefix = format!("sandbox-{}-", sandbox.slug);
        for (name, path) in entries(&self.data_path(&["exports"])) {
            if name.starts_with(&prefix) && name.ends_with(".json") {
                extra.push(path);
            }
        }
        extra.retain(|path| exists(path) && !path.starts_with(&dir));
        if exists(&dir) || !extra.is_empty() {
            let mut item = Leftover::new(
                Kind::RemovedSandbox,
                dir,
                format!(
                    "everything of the removed sandbox {}: its clone, sessions, transcripts and \
                     export, kept unless a person deletes them",
                    sandbox.slug
                ),
            )
            .of(sandbox);
            item.extra = extra;
            out.push(item);
        }
        Ok(())
    }

    /// Why a removed sandbox's files (of `kind`, or any of them) must stay now; `None` when its
    /// rule holds: removed long enough, nothing of it runs, no CD run, and for its system layer
    /// and state, no live holder.
    async fn removed_blocker(
        &self,
        sandbox: &SandboxRecord,
        kind: Option<Kind>,
    ) -> AppResult<Option<String>> {
        let Some(record) = self.deps.store.sandbox(&sandbox.id).await? else {
            return Ok(Some("its record is gone".into()));
        };
        let Some(removed) = record.removed_at else {
            return Ok(Some("the sandbox is live again".into()));
        };
        if !removed_long_enough(removed.0, self.now().0) {
            return Ok(Some("it was removed less than 10 minutes ago".into()));
        }
        let running = self.still_running(&sandbox.id).await?;
        if !running.is_empty() {
            return Ok(Some(format!("still running: {}", running.join(", "))));
        }
        if self
            .deps
            .store
            .running_cd_runs()
            .await?
            .iter()
            .any(|run| run.sandbox_id == sandbox.id)
        {
            return Ok(Some("a deploy of it runs".into()));
        }
        let holder = matches!(
            kind,
            Some(Kind::RemovedSystem | Kind::RemovedRuntime | Kind::RemovedSandbox)
        );
        if holder
            && let Some(isolation) = self.isolation.get()
            && isolation.holder_live(sandbox.id.as_str()).await
        {
            return Ok(Some("its holder still runs".into()));
        }
        Ok(None)
    }

    /// A live sandbox: caches the upkeep would remove, worktrees no task owns, checkouts of jobs
    /// no run uses and old draft attachments.
    async fn scan_live(&self, sandbox: &SandboxRecord, out: &mut Vec<Leftover>) -> AppResult<()> {
        let now = self.now().0;
        for cache in self.ended_caches(sandbox).await? {
            let reason = match &cache.task {
                Some(task) => format!(
                    "{} of task #{}: {}",
                    cache.relative,
                    task.number,
                    task_state(task)
                ),
                None => format!("{} of {}: no task owns it", cache.relative, cache.label),
            };
            let mut item = Leftover::new(Kind::TaskCache, cache.path, reason).of(sandbox);
            item.bytes = Some(cache.bytes);
            if let Some(task) = cache.task {
                item.task = Some(task.id);
                item.task_number = Some(task.number);
            }
            out.push(item);
        }
        let mut owned: HashSet<String> = HashSet::new();
        for archived in [false, true] {
            for task in self.deps.store.tasks(&sandbox.id, archived).await? {
                owned.insert(task.id.to_string());
            }
        }
        let dir = self.sandbox_dir(&sandbox.id);
        for (name, path) in entries(&dir.join("worktrees")) {
            let old = touched_ms(&path).is_some_and(|t| now - t >= ORPHAN_WORKTREE_AFTER_MS);
            if owned.contains(&name) || !old {
                continue;
            }
            let mut item = Leftover::new(
                Kind::OrphanWorktree,
                path,
                "a worktree no task owns (it may hold uncommitted work)",
            )
            .of(sandbox);
            item.repo = Some(self.sandbox_repo(&sandbox.id));
            out.push(item);
        }
        self.scan_checkouts(sandbox, out).await?;
        let drafts = dir.join("attachments").join("drafts");
        for (_, user_dir) in entries(&drafts) {
            for (name, file) in entries(&user_dir) {
                if self.old_draft(&file) {
                    out.push(
                        Leftover::new(
                            Kind::OldDraft,
                            file,
                            format!("a draft attachment ({name}) older than 7 days"),
                        )
                        .of(sandbox),
                    );
                }
            }
        }
        Ok(())
    }

    /// Whether `file` is a regular draft attachment (never a link) older than
    /// [`DRAFT_MAX_AGE_MS`].
    fn old_draft(&self, file: &Path) -> bool {
        let now = self.now().0;
        std::fs::symlink_metadata(file)
            .is_ok_and(|m| m.file_type().is_file() && now - m.mtime() * 1000 >= DRAFT_MAX_AGE_MS)
    }

    /// The job keys runs of `sandbox` used in the last [`STALE_CHECKOUT_AFTER_MS`]; `None` when
    /// no run used any (its checkouts are idle, not stale) or when the runs read do not reach
    /// back that far (nothing can be said).
    async fn ci_keys_used(&self, sandbox: &SandboxId) -> AppResult<Option<HashSet<String>>> {
        let since = self.now().0 - STALE_CHECKOUT_AFTER_MS;
        let runs = self.deps.store.ci_runs(sandbox, CI_RUNS_READ).await?;
        let read_all = runs.len() < usize::try_from(CI_RUNS_READ).unwrap_or(usize::MAX);
        if !read_all && runs.last().is_some_and(|run| run.queued_at.0 >= since) {
            return Ok(None);
        }
        let mut used: HashSet<String> = HashSet::new();
        let mut recent = 0;
        for run in runs.iter().filter(|run| run.queued_at.0 >= since) {
            recent += 1;
            for job in self.deps.store.ci_jobs(&run.id, false).await? {
                used.insert(job.key);
            }
        }
        Ok((recent > 0).then_some(used))
    }

    /// Why the CI checkout `name` is stale given the job keys `used`; `None` when it is not.
    fn checkout_stale(&self, name: &str, used: &HashSet<String>) -> Option<String> {
        let lanes = self.config.ci.lanes.max(1);
        let (unlaned, lane) = match name.split_once('@') {
            Some((key, lane)) => (key, lane.parse::<u32>().ok()),
            None => (name, Some(0)),
        };
        let job_used = used.contains(unlaned) || used.contains(checkout_job_key(name));
        let lane_used = lane.is_some_and(|lane| lane < lanes);
        if job_used && lane_used {
            None
        } else if job_used {
            Some(format!(
                "CI checkout {name}: a lane past the {lanes} configured"
            ))
        } else {
            Some(format!(
                "CI checkout {name}: no run used its job for 14 days"
            ))
        }
    }

    /// CI checkouts whose job no run of the sandbox used for [`STALE_CHECKOUT_AFTER_MS`], or of a
    /// lane past the configured ones. A sandbox with no recent run keeps its checkouts (they are
    /// idle, not stale).
    async fn scan_checkouts(
        &self,
        sandbox: &SandboxRecord,
        out: &mut Vec<Leftover>,
    ) -> AppResult<()> {
        let checkouts = entries(&self.ci_dir(&sandbox.id).join("jobs"));
        if checkouts.is_empty() {
            return Ok(());
        }
        let Some(used) = self.ci_keys_used(&sandbox.id).await? else {
            return Ok(());
        };
        for (name, path) in checkouts {
            if let Some(reason) = self.checkout_stale(&name, &used) {
                out.push(Leftover::new(Kind::StaleCiCheckout, path, reason).of(sandbox));
            }
        }
        Ok(())
    }

    /// Git's records of worktrees whose directories are gone (a partly cleaned sandbox).
    fn scan_git_records(&self, sandbox: &SandboxRecord, out: &mut Vec<Leftover>) {
        let repo = self.sandbox_repo(&sandbox.id);
        for (name, record) in entries(&repo.join(".git").join("worktrees")) {
            if let Some(worktree) = gone_worktree(&record, &self.sandbox_dir(&sandbox.id)) {
                let mut item = Leftover::new(
                    Kind::GitMetadata,
                    record,
                    format!(
                        "Git's record of the worktree {name}, whose directory {} is gone",
                        worktree.display()
                    ),
                )
                .of(sandbox);
                item.repo = Some(repo.clone());
                out.push(item);
            }
        }
    }

    /// The instance: old run directories, transcripts of deleted tasks, directories no sandbox
    /// owns (a smoke test's whose process is gone), unused bases, the host's Docker data.
    async fn scan_instance(
        &self,
        known: &HashSet<String>,
        out: &mut Vec<Leftover>,
    ) -> AppResult<()> {
        for (name, path) in entries(&self.data_path(&["runs"])) {
            if self.run_dir_owned(&name, &path).await? {
                continue;
            }
            out.push(Leftover::new(
                Kind::EndedRun,
                path,
                "a run directory no attempt, CI job or deploy owns, untouched for a day",
            ));
        }
        for (name, path) in entries(&self.data_path(&["transcripts"])) {
            let Ok(task) = TaskId::parse(&name) else {
                continue;
            };
            if name.starts_with('.') || self.deps.store.task(&task).await?.is_some() {
                continue;
            }
            out.push(Leftover::new(
                Kind::OrphanTranscripts,
                path,
                "transcripts of a task that no longer exists",
            ));
        }
        for parent in [
            &["sandboxes"][..],
            &["sysroots"],
            &["runtime", "sandboxes"],
            &["cd"],
            &["displays"],
        ] {
            for (name, path) in entries(&self.data_path(parent)) {
                if known.contains(&name) || name.starts_with('.') {
                    continue;
                }
                let item = match smoke_pid(&name) {
                    Some(pid) if !pid_alive(pid) => Leftover::new(
                        Kind::StaleSmoke,
                        path,
                        format!("a Docker smoke test's sandbox; its process {pid} is gone"),
                    ),
                    Some(_) => continue,
                    None => Leftover::new(Kind::Unowned, path, "no sandbox record owns it"),
                };
                out.push(item);
            }
        }
        if let Some(isolation) = self.isolation.get() {
            for base in isolation.unused_bases().await {
                out.push(Leftover::new(
                    Kind::UnusedBase,
                    base,
                    "a base of sandboxes' systems that is not current and no live holder mounts",
                ));
            }
        }
        for host in HOST_DOCKER {
            let path = PathBuf::from(host);
            if exists(&path) {
                out.push(Leftover::new(
                    Kind::HostDocker,
                    path,
                    "the host's own Docker data (docker-ce): Yulius does not use it; remove it \
                     on the host if nothing else does",
                ));
            }
        }
        Ok(())
    }

    /// Whether the run directory `runs/<name>` is still someone's: touched within a day, an
    /// attempt record's (its last one holds the terminal's final screen, shown after a restart),
    /// a live attempt's, a CI job's of a queued or running run, a running deploy's, or a running
    /// delivery environment run's.
    async fn run_dir_owned(&self, name: &str, path: &Path) -> AppResult<bool> {
        let now = self.now().0;
        if !touched_ms(path).is_some_and(|t| now - t >= ENDED_RUN_AFTER_MS) {
            return Ok(true);
        }
        let store = &self.deps.store;
        if let Ok(attempt) = AttemptId::parse(name)
            && store.attempt(&attempt).await?.is_some()
        {
            return Ok(true);
        }
        let written = path.to_string_lossy();
        if store
            .live_attempts()
            .await?
            .iter()
            .any(|a| a.run_dir == written)
        {
            return Ok(true);
        }
        if let Some(job) = name.strip_prefix("ci-") {
            for run in store.active_ci_runs().await? {
                if store
                    .ci_jobs(&run.id, false)
                    .await?
                    .iter()
                    .any(|j| j.id == job)
                {
                    return Ok(true);
                }
            }
        }
        if let Some(run) = name.strip_prefix("cd-")
            && store.running_cd_runs().await?.iter().any(|r| r.id == run)
        {
            return Ok(true);
        }
        if let Some(run) = name.strip_prefix("env-")
            && store
                .running_environment_runs()
                .await?
                .iter()
                .any(|r| r.id == run)
        {
            return Ok(true);
        }
        Ok(false)
    }

    /// Checks `item`'s rule again on fresh state right before it is deleted (LFT-007). `Ok`
    /// with a CI claim when its sandbox's checkouts are touched; `Err` with why it stays.
    async fn leftover_guard(
        &self,
        item: &Leftover,
        pass: &mut Pass,
    ) -> Result<Option<CiClaim<'_>>, String> {
        if !item.kind.deletable() {
            return Err("reported only: the host's own data".into());
        }
        if !self.inside_data(&item.path) || item.extra.iter().any(|p| !self.inside_data(p)) {
            return Err("outside the data directory".into());
        }
        let store = &self.deps.store;
        let sandbox = match &item.sandbox {
            Some(id) => Some(
                store
                    .sandbox(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("its sandbox record is gone")?,
            ),
            None => None,
        };
        let claim = |sandbox: &SandboxRecord| {
            if self.ci_claim(&sandbox.id) {
                Ok(Some(CiClaim {
                    app: self,
                    sandbox: sandbox.id.clone(),
                }))
            } else {
                Err("a CI run of its sandbox runs".to_owned())
            }
        };
        match item.kind {
            Kind::RemovedWorktrees
            | Kind::RemovedCi
            | Kind::RemovedBrowser
            | Kind::RemovedCd
            | Kind::RemovedDisplay
            | Kind::RemovedSystem
            | Kind::RemovedRuntime
            | Kind::RemovedSandbox => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                if let Some(why) = self
                    .removed_blocker(&sandbox, Some(item.kind))
                    .await
                    .map_err(|e| e.to_string())?
                {
                    return Err(why);
                }
                if matches!(item.kind, Kind::RemovedCi | Kind::RemovedSandbox) {
                    return claim(&sandbox);
                }
                Ok(None)
            }
            Kind::EndedRun => {
                if let Some(sandbox) = &sandbox {
                    // A removed sandbox's agent run: its rule is the sandbox's.
                    let live = store.live_attempts().await.map_err(|e| e.to_string())?;
                    let path = item.path.to_string_lossy();
                    if live.iter().any(|a| a.run_dir == path) {
                        return Err("its run is live".into());
                    }
                    if let Some(why) = self
                        .removed_blocker(sandbox, None)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        return Err(why);
                    }
                    return Ok(None);
                }
                let name = file_name(&item.path);
                if self
                    .run_dir_owned(&name, &item.path)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    return Err("a run owns it now, or it was touched within a day".into());
                }
                Ok(None)
            }
            Kind::TaskCache => {
                if let Some(task) = &item.task
                    && !self.cache_still_ended(task).await
                {
                    return Err("its task works again".into());
                }
                Ok(None)
            }
            Kind::StaleCiCheckout => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                if !pass.ci_keys.contains_key(&sandbox.id) {
                    let used = self
                        .ci_keys_used(&sandbox.id)
                        .await
                        .map_err(|e| e.to_string())?;
                    pass.ci_keys.insert(sandbox.id.clone(), used);
                }
                let used = pass.ci_keys.get(&sandbox.id).cloned().flatten();
                let stale = used.is_some_and(|used| {
                    self.checkout_stale(&file_name(&item.path), &used).is_some()
                });
                if !stale {
                    return Err("a recent run used its job".into());
                }
                claim(&sandbox)
            }
            Kind::OrphanWorktree => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                let now = self.now().0;
                if !touched_ms(&item.path).is_some_and(|t| now - t >= ORPHAN_WORKTREE_AFTER_MS) {
                    return Err("it was touched within the hour".into());
                }
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                for archived in [false, true] {
                    let tasks = store
                        .tasks(&sandbox.id, archived)
                        .await
                        .map_err(|e| e.to_string())?;
                    if tasks.iter().any(|t| Some(t.id.to_string()) == name) {
                        return Err("a task owns it now".into());
                    }
                }
                Ok(None)
            }
            Kind::GitMetadata => {
                let sandbox = sandbox.ok_or("no sandbox")?;
                if gone_worktree(&item.path, &self.sandbox_dir(&sandbox.id)).is_none() {
                    return Err("its worktree is back, or it is locked".into());
                }
                Ok(None)
            }
            Kind::OrphanTranscripts => {
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                let task = name.and_then(|n| TaskId::parse(&n).ok());
                match task {
                    Some(task)
                        if store
                            .task(&task)
                            .await
                            .map_err(|e| e.to_string())?
                            .is_none() =>
                    {
                        Ok(None)
                    }
                    _ => Err("its task exists".into()),
                }
            }
            Kind::StaleSmoke | Kind::Unowned => {
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Ok(id) = SandboxId::parse(&name)
                    && store
                        .sandbox(&id)
                        .await
                        .map_err(|e| e.to_string())?
                        .is_some()
                {
                    return Err("a sandbox owns it now".into());
                }
                if smoke_pid(&name).is_some_and(pid_alive) {
                    return Err("its smoke test runs".into());
                }
                if let Some(isolation) = self.isolation.get()
                    && isolation.holder_live(&name).await
                {
                    return Err("a holder of it runs".into());
                }
                Ok(None)
            }
            Kind::UnusedBase => match self.isolation.get() {
                Some(isolation) if isolation.unused_bases().await.contains(&item.path) => Ok(None),
                _ => Err("the base is current or mounted".into()),
            },
            Kind::OldDraft => {
                if self.old_draft(&item.path) {
                    Ok(None)
                } else {
                    Err("not a draft older than 7 days any more".into())
                }
            }
            Kind::HostDocker => Err("reported only".into()),
        }
    }

    /// Removes one path of an item: a missing one is fine; a tree whose files may belong to a
    /// sandbox's mapped ids goes through the isolation. That is every path below `sandboxes/`:
    /// a sandbox's CI jobs and agents write there as its mapped ids, which the service user
    /// cannot delete (#675); the isolation removes it there without following a link
    /// (ADR-0176).
    async fn remove_leftover_path(&self, path: &Path) -> Result<(), String> {
        if !exists(path) {
            return Ok(());
        }
        let mapped = ["sysroots", "system", "sandboxes"]
            .iter()
            .map(|d| self.data_path(&[d]))
            .chain([self.data_path(&["runtime", "sandboxes"])])
            .any(|root| path.starts_with(root));
        if mapped && let Some(isolation) = self.isolation.get() {
            return isolation.remove_owned_tree(path).await;
        }
        let (root, target) = (self.config.data_dir.clone(), path.to_path_buf());
        tokio::task::spawn_blocking(move || remove_below(&root, &target))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Deletes one item after its guard: measured, removed, audited.
    async fn delete_leftover(
        &self,
        item: &Leftover,
        actor: &str,
        pass: &mut Pass,
        report: &mut DeleteReport,
    ) {
        let label = item.path.to_string_lossy().into_owned();
        let guard = match self.leftover_guard(item, pass).await {
            Ok(guard) => guard,
            Err(why) => {
                report.kept.push(format!("{label}: {why}"));
                return;
            }
        };
        let paths: Vec<PathBuf> = std::iter::once(item.path.clone())
            .chain(item.extra.iter().cloned())
            .collect();
        let measured = paths.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            measured
                .iter()
                .filter_map(|p| size_of(p))
                .fold(0u64, u64::saturating_add)
        })
        .await
        .unwrap_or(0);
        let mut result = Ok(());
        for path in &paths {
            if let Err(err) = self.remove_leftover_path(path).await {
                result = Err(err);
                break;
            }
        }
        // Git forgets the worktrees that went with it, and only those (`git worktree prune`
        // would also drop the records of worktrees an agent keeps in its own `/tmp`).
        if result.is_ok()
            && let (Some(repo), Some(sandbox)) = (&item.repo, &item.sandbox)
            && item.kind != Kind::GitMetadata
        {
            let root = self.config.data_dir.clone();
            let (repo, removed, dir) = (repo.clone(), item.path.clone(), self.sandbox_dir(sandbox));
            let _ = tokio::task::spawn_blocking(move || {
                drop_records_under(&root, &repo, &removed, &dir);
            })
            .await;
        }
        drop(guard);
        let target = item.slug.clone().unwrap_or_else(|| "instance".into());
        match result {
            Ok(()) => {
                report.deleted += 1;
                report.freed = report.freed.saturating_add(bytes);
                self.dlv_audit(
                    actor,
                    "leftovers.delete",
                    &target,
                    &format!(
                        "{label} ({}, {}): {}",
                        item.kind.as_str(),
                        human(bytes),
                        item.reason
                    ),
                )
                .await;
            }
            Err(err) => {
                tracing::warn!(path = %label, error = %err, "a leftover could not be deleted");
                report.failed.push(format!("{label}: {err}"));
                self.dlv_audit(
                    actor,
                    "leftovers.failed",
                    &target,
                    &format!("{label} ({}): {err}", item.kind.as_str()),
                )
                .await;
            }
        }
    }

    /// Deletes `items` one after the other, each after its guard, updating the job's progress.
    pub async fn delete_leftovers(&self, items: &[Leftover], actor: &str) -> DeleteReport {
        let mut report = DeleteReport::default();
        let mut pass = Pass::default();
        for item in items {
            let (kept, failed) = (report.kept.len(), report.failed.len());
            self.delete_leftover(item, actor, &mut pass, &mut report)
                .await;
            let (freed, deleted) = (report.freed, report.deleted);
            let new_kept = report.kept[kept..].to_vec();
            let new_failed = report.failed[failed..].to_vec();
            self.with_leftovers(|inner| {
                if let Some(job) = inner.job.as_mut() {
                    job.done += 1;
                    job.deleted = deleted;
                    job.freed = freed;
                    job.kept.extend(new_kept);
                    job.failed.extend(new_failed);
                }
            });
        }
        // The list no longer shows what went.
        let gone: HashSet<String> = items
            .iter()
            .filter(|item| !exists(&item.path))
            .map(|item| item.id.clone())
            .collect();
        self.with_leftovers(|inner| inner.items.retain(|item| !gone.contains(&item.id)));
        report
    }

    fn with_leftovers<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let mut inner = self
            .leftovers
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut inner)
    }

    /// Takes the job slot for `action` and returns the job's generation; `None` when a scan or a
    /// deletion runs.
    fn begin_leftover_job(&self, action: &str, actor: &str, total: usize) -> Option<u64> {
        let started_at = rfc3339(self.now());
        self.with_leftovers(|inner| {
            if inner.job.as_ref().is_some_and(|job| job.running) {
                return None;
            }
            inner.generation += 1;
            inner.job = Some(LeftoverJobView {
                action: action.to_owned(),
                running: true,
                actor: actor.to_owned(),
                total,
                started_at,
                ..LeftoverJobView::default()
            });
            Some(inner.generation)
        })
    }

    /// Ends the running job (once: a job already ended stays as it ended).
    fn end_leftover_job(&self, error: Option<String>) {
        let ended_at = rfc3339(self.now());
        self.with_leftovers(|inner| {
            if let Some(job) = inner.job.as_mut().filter(|job| job.running) {
                job.running = false;
                job.ended_at = Some(ended_at);
                if let Some(error) = error {
                    job.failed.push(error);
                }
            }
        });
    }

    async fn scan_into_state(&self) -> AppResult<()> {
        let items = self.leftovers_scan().await?;
        let now = self.now();
        self.with_leftovers(|inner| {
            inner.items = items;
            inner.scanned_at = Some(now);
        });
        Ok(())
    }

    /// One automatic round (LFT-009): scan, delete every item that needs no person. Skipped
    /// while a person's scan or deletion runs.
    pub async fn run_leftovers_round(&self) -> Option<DeleteReport> {
        let generation = self.begin_leftover_job("scan", "yulius", 0)?;
        let _end = JobEnd(self, generation);
        if let Err(err) = self.scan_into_state().await {
            tracing::warn!(error = %err, "the leftovers scan failed");
            self.end_leftover_job(Some(err.to_string()));
            return None;
        }
        let safe: Vec<Leftover> = self.with_leftovers(|inner| {
            inner
                .items
                .iter()
                .filter(|item| item.kind.auto())
                .cloned()
                .collect()
        });
        self.with_leftovers(|inner| {
            if let Some(job) = inner.job.as_mut() {
                job.action = "delete".into();
                job.total = safe.len();
            }
        });
        let report = self.delete_leftovers(&safe, "yulius").await;
        self.end_leftover_job(None);
        if report.deleted > 0 || !report.failed.is_empty() {
            tracing::info!(
                deleted = report.deleted,
                freed = report.freed,
                failed = report.failed.len(),
                "leftovers round"
            );
        }
        Some(report)
    }

    /// Writes a new key for `yulius leftovers` to [`CLI_KEY_FILE`] (0600, replaced atomically)
    /// and keeps it for [`App::leftovers_key_matches`].
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn write_leftovers_key(&self) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let key: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self.config.data_dir.join(CLI_KEY_FILE);
        let staged = self.config.data_dir.join(format!(".{CLI_KEY_FILE}.new"));
        let _ = std::fs::remove_file(&staged);
        let written = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&staged)?;
            file.write_all(key.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&staged, &path)
        })();
        if let Err(err) = written {
            // Never leave a copy of the key behind.
            let _ = std::fs::remove_file(&staged);
            return Err(err);
        }
        self.with_leftovers(|inner| inner.key = Some(key));
        Ok(())
    }

    /// Whether `presented` is the CLI's key: never an empty one, none before the key is written,
    /// and compared as SHA-256 digests in constant time (no length or prefix shows).
    #[must_use]
    pub fn leftovers_key_matches(&self, presented: &str) -> bool {
        let Some(key) = self.with_leftovers(|inner| inner.key.clone()) else {
            return false;
        };
        if presented.is_empty() {
            return false;
        }
        let ours = self.deps.crypto.sha256_hex(key.as_bytes());
        let theirs = self.deps.crypto.sha256_hex(presented.as_bytes());
        ours.len() == theirs.len()
            && ours
                .bytes()
                .zip(theirs.bytes())
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0
    }

    /// The automatic rounds: at start (after [`AUTO_FIRST`]) and every hour. The CLI's key is
    /// written first.
    pub fn start_leftovers_rounds(self: &Arc<Self>) {
        let every = Duration::from_millis(u64::try_from(AUTO_EVERY_MS).unwrap_or(u64::MAX));
        self.start_leftovers_rounds_with(AUTO_FIRST, every);
    }

    /// [`App::start_leftovers_rounds`] with its periods (tests shorten them).
    pub fn start_leftovers_rounds_with(self: &Arc<Self>, first: Duration, every: Duration) {
        if let Err(err) = self.write_leftovers_key() {
            tracing::warn!(error = %err, "cannot write the key of yulius leftovers");
        }
        let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        let next = Timestamp(self.now().0.saturating_add(ms(first)));
        self.with_leftovers(|inner| inner.next_auto_at = Some(next));
        let app = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(first).await;
            let mut ticks = tokio::time::interval(every);
            loop {
                ticks.tick().await;
                app.run_leftovers_round().await;
                let next = Timestamp(app.now().0.saturating_add(ms(every)));
                app.with_leftovers(|inner| inner.next_auto_at = Some(next));
            }
        });
    }

    /// The view without a person (the CLI's loopback route checks its key first).
    pub async fn leftovers_overview(&self) -> LeftoversView {
        let data_dir = self.config.data_dir.clone();
        // The disk probe reads OS counters: off the executor.
        let probe = Arc::clone(&self.deps.system);
        let dir = data_dir.clone();
        let disk = tokio::task::spawn_blocking(move || probe.snapshot(&dir))
            .await
            .unwrap_or_default();
        self.with_leftovers(|inner| {
            let deletable = inner.items.iter().filter(|i| i.kind.deletable());
            let total_bytes = deletable.clone().filter_map(|i| i.bytes).sum();
            let safe_bytes = deletable
                .filter(|i| i.kind.auto())
                .filter_map(|i| i.bytes)
                .sum();
            LeftoversView {
                items: inner.items.iter().map(Leftover::view).collect(),
                scanned_at: inner.scanned_at.map(rfc3339),
                total_bytes,
                safe_bytes,
                disk_used: disk.disk_used,
                disk_total: disk.disk_total,
                data_dir: data_dir.to_string_lossy().into_owned(),
                job: inner.job.clone(),
                next_auto_at: inner.next_auto_at.map(rfc3339),
            }
        })
    }

    /// `GET /api/admin/leftovers` (superadmin; LFT-010).
    ///
    /// # Errors
    /// `Forbidden` for anyone but the superadmin.
    pub async fn leftovers_view(&self, ctx: &AuthContext) -> AppResult<LeftoversView> {
        self.require_superadmin(ctx)?;
        Ok(self.leftovers_overview().await)
    }

    /// `POST /api/admin/leftovers/scan` (superadmin): rescans in the background.
    ///
    /// # Errors
    /// `Forbidden`; `Conflict` while a scan or deletion runs.
    pub async fn scan_leftovers(self: &Arc<Self>, ctx: &AuthContext) -> AppResult<LeftoversView> {
        self.require_superadmin(ctx)?;
        let actor = self.login_of(&ctx.user.id).await;
        self.start_leftovers_scan(&actor).await
    }

    /// Starts a background scan for `actor`.
    ///
    /// # Errors
    /// `Conflict` while a scan or deletion runs.
    pub async fn start_leftovers_scan(self: &Arc<Self>, actor: &str) -> AppResult<LeftoversView> {
        let Some(generation) = self.begin_leftover_job("scan", actor, 0) else {
            return Err(AppError::Conflict(
                "a leftovers scan or deletion runs".into(),
            ));
        };
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let _end = JobEnd(&app, generation);
            let result = app.scan_into_state().await;
            app.end_leftover_job(result.err().map(|e| e.to_string()));
        });
        Ok(self.leftovers_overview().await)
    }

    /// `POST /api/admin/leftovers/delete` (superadmin): deletes the selection in the background.
    ///
    /// # Errors
    /// `Forbidden`; `Conflict` while a scan or deletion runs; `Invalid` for unknown ids or
    /// an empty selection.
    pub async fn delete_leftovers_request(
        self: &Arc<Self>,
        ctx: &AuthContext,
        request: DeleteLeftoversRequest,
    ) -> AppResult<LeftoversView> {
        self.require_superadmin(ctx)?;
        let actor = self.login_of(&ctx.user.id).await;
        self.start_leftovers_delete(&actor, request.into()).await
    }

    /// Starts a background deletion of `selection` from the last scan for `actor`.
    ///
    /// # Errors
    /// `Conflict` while a scan or deletion runs; `Invalid` for unknown ids or nothing to
    /// delete.
    pub async fn start_leftovers_delete(
        self: &Arc<Self>,
        actor: &str,
        selection: Selection,
    ) -> AppResult<LeftoversView> {
        let chosen: Vec<Leftover> = self.with_leftovers(|inner| match &selection {
            Selection::AllSafe => inner
                .items
                .iter()
                .filter(|i| i.kind.auto())
                .cloned()
                .collect(),
            Selection::Ids(ids) => inner
                .items
                .iter()
                .filter(|i| ids.contains(&i.id))
                .cloned()
                .collect(),
        });
        if let Selection::Ids(ids) = &selection {
            let unknown: Vec<&String> = ids
                .iter()
                .filter(|id| !chosen.iter().any(|i| &i.id == *id))
                .collect();
            if !unknown.is_empty() {
                return Err(AppError::Invalid(format!(
                    "not in the last scan (scan again): {}",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        if chosen.is_empty() {
            return Err(AppError::Invalid("nothing to delete".into()));
        }
        let Some(generation) = self.begin_leftover_job("delete", actor, chosen.len()) else {
            return Err(AppError::Conflict(
                "a leftovers scan or deletion runs".into(),
            ));
        };
        let app = Arc::clone(self);
        let actor = actor.to_owned();
        tokio::spawn(async move {
            let _end = JobEnd(&app, generation);
            app.delete_leftovers(&chosen, &actor).await;
            app.end_leftover_job(None);
        });
        Ok(self.leftovers_overview().await)
    }
}

/// How a task's state reads in a reason.
fn task_state(task: &crate::model::TaskRecord) -> String {
    use yulius_domain::task::{Activity, TaskStatus};
    match (task.status, task.activity) {
        (TaskStatus::Running, Some(Activity::Done)) => "done, quiet for 30 minutes".into(),
        (TaskStatus::Running, _) => "idle for 4 hours".into(),
        (TaskStatus::Closed, _) => "closed".into(),
        (TaskStatus::Stopped(_), _) => "stopped".into(),
        (TaskStatus::Failed(_), _) => "failed".into(),
        _ => "idle for 4 hours".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Review round 2 (blocker): the removal itself refuses a link on the way, whenever it
    /// appears (a sandbox can swap a directory for a link right after the last check), so it
    /// never reaches outside; a real path goes, a missing one is fine.
    #[test]
    fn the_removal_never_goes_through_a_link_on_the_way() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let data = tmp.path().join("data");
        let drafts = data.join("sandboxes/s1/attachments/drafts");
        std::fs::create_dir_all(&drafts)?;
        std::fs::write(data.join("master.key"), "the key")?;
        std::os::unix::fs::symlink(&data, drafts.join("u"))?;
        assert!(remove_below(&data, &drafts.join("u/master.key")).is_err());
        assert!(data.join("master.key").exists(), "never through the link");

        let victim = data.join("sandboxes/victim/repo");
        std::fs::create_dir_all(&victim)?;
        std::fs::write(victim.join("README.md"), "kept")?;
        let jobs = data.join("sandboxes/s1/ci/jobs");
        std::fs::create_dir_all(jobs.parent().ok_or("parent")?)?;
        std::os::unix::fs::symlink(data.join("sandboxes/victim"), &jobs)?;
        assert!(remove_below(&data, &jobs.join("repo")).is_err());
        assert!(victim.join("README.md").exists());

        // The item itself may be a link: the link goes, not what it points at.
        let own = data.join("sandboxes/s1/attachments/drafts/old.png");
        std::os::unix::fs::symlink(data.join("master.key"), &own)?;
        remove_below(&data, &own)?;
        assert!(!exists(&own) && data.join("master.key").exists());

        let tree = data.join("sandboxes/s1/worktrees/t1/target/debug");
        std::fs::create_dir_all(&tree)?;
        std::fs::write(tree.join("blob"), "built")?;
        std::os::unix::fs::symlink(&victim, tree.join("escape"))?;
        remove_below(&data, &data.join("sandboxes/s1/worktrees/t1"))?;
        assert!(!data.join("sandboxes/s1/worktrees/t1").exists());
        assert!(
            victim.join("README.md").exists(),
            "a link inside goes as a link"
        );
        remove_below(&data, &data.join("sandboxes/s1/worktrees/t1"))?;
        assert!(remove_below(&data, &tmp.path().join("elsewhere")).is_err());
        Ok(())
    }

    /// #675: a removal that fails names the entry that stayed, not only the item. Nobody, root
    /// included, can unlink an entry of `/proc`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_removal_names_the_entry_that_stayed() {
        let item = Path::new("/proc/self/ns");
        let err = remove_below(Path::new("/proc/self"), item).err();
        let message = err.map(|e| e.to_string()).unwrap_or_default();
        assert!(
            message.starts_with("/proc/self/ns/") && message.contains("(os error"),
            "{message:?}"
        );
    }

    /// Review of SPEC-228 (major): a tree deeper than the handles the walk holds goes too.
    #[test]
    fn a_tree_deeper_than_the_walk_s_handles_goes() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let data = tmp.path().join("data");
        let item = data.join("sandboxes/s1/ci/jobs/deep");
        let mut deepest = item.clone();
        for _ in 0..MAX_WRITABLE_DEPTH + 40 {
            deepest.push("d");
        }
        std::fs::create_dir_all(&deepest)?;
        std::fs::write(deepest.join("f"), "x")?;
        remove_below(&data, &item)?;
        assert!(!item.exists() && data.join("sandboxes/s1/ci/jobs").exists());
        Ok(())
    }

    /// #680: a directory its owner cannot read (no `r`: 0o000, 0o300), the item itself or one
    /// below it, is made writable and goes too. The gate runs as root, whose DAC capabilities open
    /// any directory: the test drops them from its own thread (capabilities are per thread), so
    /// the kernel checks the owner's bits as it does for the service user.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_tree_with_unreadable_directories_goes() -> TestResult {
        use rustix::thread::{CapabilitySet, capabilities, set_capabilities};
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir()?;
        let data = tmp.path().join("data");
        let item = data.join("runs/old");
        std::fs::create_dir_all(item.join("b/c"))?;
        std::fs::create_dir(item.join("a"))?;
        std::fs::write(item.join("a/f"), "x")?;
        std::fs::write(item.join("b/c/f"), "x")?;
        for (dir, mode) in [("b/c", 0o000), ("b", 0o300), ("a", 0o000), ("", 0o000)] {
            std::fs::set_permissions(item.join(dir), std::fs::Permissions::from_mode(mode))?;
        }

        let held = capabilities(None)?;
        let mut owner_only = held;
        owner_only.effective -=
            CapabilitySet::DAC_OVERRIDE | CapabilitySet::DAC_READ_SEARCH | CapabilitySet::FOWNER;
        set_capabilities(None, owner_only)?;
        let unreadable = std::fs::read_dir(&item).err().map(|e| e.kind());
        let removed = remove_below(&data, &item);
        set_capabilities(None, held)?;

        assert_eq!(
            unreadable,
            Some(std::io::ErrorKind::PermissionDenied),
            "the owner's bits are checked"
        );
        removed?;
        assert!(!exists(&item) && data.join("runs").exists());
        Ok(())
    }

    /// Review of SPEC-232: a name swapped for a link (to a directory) or a file after it was
    /// listed is never opened as a directory handle, so its target is never changed or walked.
    #[test]
    fn a_directory_handle_is_never_opened_through_a_link() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir()?;
        let outside = tmp.path().join("runtime");
        std::fs::create_dir(&outside)?;
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o750))?;
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(tree.join("real"))?;
        std::fs::write(tree.join("file"), "x")?;
        std::os::unix::fs::symlink(&outside, tree.join("link"))?;
        let handle = nix::fcntl::open(
            &tree,
            nix::fcntl::OFlag::O_DIRECTORY | nix::fcntl::OFlag::O_NOFOLLOW,
            nix::sys::stat::Mode::empty(),
        )?;
        assert!(open_dir_handle(&handle, "link".as_ref()).is_none());
        assert!(open_dir_handle(&handle, "file".as_ref()).is_none());
        assert!(open_dir_handle(&handle, "gone".as_ref()).is_none());
        assert!(open_dir_handle(&handle, "real".as_ref()).is_some());
        let mode = std::fs::metadata(&outside)?.permissions().mode() & 0o7777;
        assert_eq!(mode, 0o750);
        Ok(())
    }

    /// Review round 3: the fallback that makes a read-only tree removable changes directories
    /// through their own handles to a fixed 0o700, never through a link to what lies outside.
    #[test]
    fn making_a_tree_writable_never_follows_a_link() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir()?;
        let outside_file = tmp.path().join("master.key");
        std::fs::write(&outside_file, "the key")?;
        std::fs::set_permissions(&outside_file, std::fs::Permissions::from_mode(0o600))?;
        let outside_dir = tmp.path().join("runtime");
        std::fs::create_dir(&outside_dir)?;
        std::fs::set_permissions(&outside_dir, std::fs::Permissions::from_mode(0o750))?;
        let tree = tmp.path().join("tree");
        let locked = tree.join("cache/locked");
        std::fs::create_dir_all(&locked)?;
        std::fs::write(locked.join("f"), "x")?;
        std::os::unix::fs::symlink(&outside_file, tree.join("cache/key"))?;
        std::os::unix::fs::symlink(&outside_dir, tree.join("cache/dir"))?;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o4555))?;

        let handle = nix::fcntl::open(
            &tree,
            nix::fcntl::OFlag::O_DIRECTORY | nix::fcntl::OFlag::O_NOFOLLOW,
            nix::sys::stat::Mode::empty(),
        )?;
        make_writable_at(&handle, 0, &mut 1_000);
        let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o7777);
        assert_eq!(mode(&locked)?, 0o700, "a fixed mode, setuid gone");
        assert_eq!(
            mode(&outside_file)?,
            0o600,
            "a link's target is never changed"
        );
        assert_eq!(mode(&outside_dir)?, 0o750);
        Ok(())
    }
}
