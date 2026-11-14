//! A deleted task's attachments (SPEC-396, ADR-0253; amends TSK-009).
//!
//! Contract:
//! - when a task is deleted, every `attachments/tasks/<id>/` folder of its sandbox whose task no
//!   longer exists is settled: one that a remaining task's request (its prompt with its queued
//!   messages; archived tasks count) or an automation's prompt names by `attachments/tasks/<id>/`
//!   stays where it is, so those paths keep working; any other moves by one rename to
//!   `attachments/trash/<secs>-<number>-<id>/` (deletion time in Unix seconds, number `0` when
//!   unknown: a folder kept earlier, or one a parallel deletion moved first, which the task's own
//!   settling then renumbers). The folders are listed before the tasks are read, so a task created
//!   meanwhile (its record comes before its folder) is never taken for a deleted one;
//! - an entry is removed [`TRASH_DAYS`] days after its deletion, at most
//!   [`MAX_PURGED_PER_ROUND`] entries a round: at deletions, at listings, at start-up and in the
//!   workspace reaper's hourly round;
//! - `attachments_trash` lists the project's entries and restores one's regular files into a live
//!   task's folder under free names (write access, the folder bound, audited
//!   `attachment.restored`); an entry goes once nothing is left in it, so what could not be
//!   restored stays in the trash and is reported;
//! - the file system work runs off the async runtime, and every folder is reached through handles
//!   without following a link (ADR-0174, #660): a link planted on the way moves, removes or
//!   restores nothing.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;

use nix::errno::Errno;
use nix::fcntl::renameat;
use nix::sys::stat::SFlag;
use nix::unistd::{UnlinkatFlags, unlinkat};
use serde::Serialize;
use serde_json::{Value, json};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::auth::rfc3339;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::files::{errno_error, kind_at, names_in, open_dir_at, open_dirs_below, remove_tree};
use crate::manage::tool_detail;
use crate::model::TaskRecord;
use crate::projects::blocking;
use crate::uploads::{
    ATTACHMENTS_DIR, MAX_ATTACHMENTS_PER_FOLDER, MAX_FREE_NAME_ATTEMPTS, MAX_PRUNED_ENTRIES,
    attachment_name, candidate, count_entries, ensure_plain_dirs, link_new_in,
};

/// Days a deleted task's attachments can be restored.
pub const TRASH_DAYS: i64 = 7;
const TRASH_MAX_AGE_SECS: i64 = TRASH_DAYS * 24 * 3600;
/// Old entries one purge removes at most (a sandbox process can plant many).
pub const MAX_PURGED_PER_ROUND: usize = 100;
/// The trash folder inside a sandbox's attachments folder.
const TRASH_DIR: &str = "trash";
/// File names an entry's listing shows.
const MAX_LISTED_FILES: usize = 100;

/// What became of a deleted task's attachments (`task_delete`'s answer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeletedAttachments {
    /// They stay where they are: the numbers of the tasks whose requests name them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_for: Vec<u32>,
    /// They stay where they are: the names of the automations whose prompts name them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_for_automations: Vec<String>,
    /// They wait in the trash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash: Option<TrashEntry>,
    /// They stay where they are: moving them to the trash failed (a link planted on the way) or
    /// was not tried (an `attachments` or `tasks` link never looked behind, the tasks unreadable).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub not_moved: bool,
}

/// One deleted task's attachments in the trash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrashEntry {
    /// The deleted task's id.
    pub task_id: String,
    /// Its number, when still known.
    pub number: Option<u32>,
    /// When it went to the trash (RFC 3339).
    pub deleted_at: String,
    /// When it is removed (RFC 3339).
    pub restorable_until: String,
    /// Its files (at most 100 names).
    pub files: Vec<String>,
    #[serde(skip)]
    name: String,
}

/// What a restore moved and what it left in the trash.
#[derive(Debug, Default)]
struct Restored {
    paths: Vec<String>,
    left: Vec<String>,
}

/// `<secs>-<number>-<id>` read back.
fn parse_entry(name: &str) -> Option<(i64, u32, TaskId)> {
    let (secs, rest) = name.split_once('-')?;
    let (number, id) = rest.split_once('-')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(secs) || !digits(number) {
        return None;
    }
    Some((
        secs.parse().ok()?,
        number.parse().ok()?,
        TaskId::parse(id).ok()?,
    ))
}

/// The regular files of the folder `dir` with UTF-8 names, sorted, at most `limit`.
fn files_in(dir: &OwnedFd, limit: usize) -> Vec<String> {
    let mut files: Vec<String> = names_in(dir, MAX_PRUNED_ENTRIES)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| kind_at(dir, name).is_ok_and(|(kind, _)| kind == SFlag::S_IFREG))
        .filter_map(|name| name.into_string().ok())
        .collect();
    files.sort();
    files.truncate(limit);
    files
}

fn entry_of(trash: &OwnedFd, name: &str) -> Option<TrashEntry> {
    let (secs, number, id) = parse_entry(name)?;
    let folder = open_dir_at(trash, OsStr::new(name), false).ok()?;
    Some(TrashEntry {
        task_id: id.as_str().to_owned(),
        number: (number > 0).then_some(number),
        deleted_at: rfc3339(Timestamp(secs.saturating_mul(1000))),
        restorable_until: rfc3339(Timestamp(
            secs.saturating_add(TRASH_MAX_AGE_SECS).saturating_mul(1000),
        )),
        files: files_in(&folder, MAX_LISTED_FILES),
        name: name.to_owned(),
    })
}

/// The trash's entries, newest first.
fn entries(trash: &OwnedFd) -> Vec<TrashEntry> {
    let mut found: Vec<TrashEntry> = names_in(trash, MAX_PRUNED_ENTRIES)
        .unwrap_or_default()
        .iter()
        .filter_map(|name| entry_of(trash, name.to_str()?))
        .collect();
    found.sort_by(|a, b| b.name.cmp(&a.name));
    found
}

/// Removes the trash's entries older than [`TRASH_DAYS`] at `now` (ATR-004), at most
/// [`MAX_PURGED_PER_ROUND`]. Blocks.
fn purge(attachments: &OwnedFd, now: i64) {
    let Ok(trash) = open_dir_at(attachments, OsStr::new(TRASH_DIR), false) else {
        return;
    };
    let old = names_in(&trash, MAX_PRUNED_ENTRIES)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| {
            name.to_str()
                .and_then(parse_entry)
                .is_some_and(|(secs, _, _)| now.saturating_sub(secs) > TRASH_MAX_AGE_SECS)
        })
        .take(MAX_PURGED_PER_ROUND);
    for name in old {
        if let Err(err) = remove_tree(&trash, &name) {
            tracing::warn!(error = %err, "could not purge the attachments trash");
        }
    }
}

/// The sandbox's `attachments` folder and its `tasks` folder (`None` when it cannot be opened),
/// opened from the sandbox directory without following a link; `Ok(None)` when no task's folder
/// can be there (no `attachments`, or a plain file), an error when one may be behind it (a link,
/// never followed). Blocks.
fn open_attachments(sandbox_dir: &Path) -> Result<Option<(OwnedFd, Option<OwnedFd>)>, Errno> {
    let Ok(dir) = std::fs::canonicalize(sandbox_dir) else {
        return Ok(None);
    };
    let Ok(sandbox) = open_dirs_below(&dir, &[], false) else {
        return Ok(None);
    };
    let name = OsStr::new(ATTACHMENTS_DIR);
    let attachments = match open_dir_at(&sandbox, name, false) {
        Ok(folder) => folder,
        Err(Errno::ENOENT) => return Ok(None),
        // A link opened with `O_NOFOLLOW | O_DIRECTORY` fails as a plain file does.
        Err(Errno::ENOTDIR)
            if kind_at(&sandbox, name).is_ok_and(|(kind, _)| kind != SFlag::S_IFLNK) =>
        {
            return Ok(None);
        }
        Err(err) => {
            tracing::warn!(error = %err, "attachments folder is not a plain folder");
            return Err(err);
        }
    };
    let tasks = open_dir_at(&attachments, OsStr::new("tasks"), false).ok();
    Ok(Some((attachments, tasks)))
}

/// The deleted task's fate when settling stopped before moving anything: `not_moved` when its
/// folder is in `folders`, or when `tasks` could not be opened but is there as a folder or a link
/// it may be behind (never followed). Blocks.
fn left_in_place(attachments: &OwnedFd, folders: Option<&OwnedFd>, id: &str) -> DeletedAttachments {
    let is = |dir: &OwnedFd, name: &str, wanted: &[SFlag]| {
        kind_at(dir, OsStr::new(name)).is_ok_and(|(kind, _)| wanted.contains(&kind))
    };
    DeletedAttachments {
        not_moved: match folders {
            Some(folders) => is(folders, id, &[SFlag::S_IFDIR]),
            None => is(attachments, "tasks", &[SFlag::S_IFDIR, SFlag::S_IFLNK]),
        },
        ..DeletedAttachments::default()
    }
}

/// The folders of `folders` to settle: at most `limit` listed ones, and the deleted task's own
/// whether or not the listing reached it (SPEC-413 ANR-003). Links are never taken. Blocks.
fn listed_folders(folders: &OwnedFd, deleted: &str, limit: usize) -> Vec<OsString> {
    let is_folder =
        |name: &OsStr| kind_at(folders, name).is_ok_and(|(kind, _)| kind == SFlag::S_IFDIR);
    let mut names: Vec<OsString> = names_in(folders, limit)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| is_folder(name))
        .collect();
    let own = OsStr::new(deleted);
    if !names.iter().any(|name| name == own) && is_folder(own) {
        names.push(own.to_owned());
    }
    names
}

/// What settling needs to know about the sandbox: the live tasks, and every text that may name a
/// folder (requests, automations' prompts). Shell folders and composer drafts ask it too
/// (SPEC-413, ADR-0259).
pub(crate) struct Namers {
    live: HashSet<String>,
    tasks: Vec<(u32, String)>,
    automations: Vec<(String, String)>,
}

impl Namers {
    /// Whether a task's request or an automation's prompt contains `path`.
    pub(crate) fn name(&self, path: &str) -> bool {
        self.tasks
            .iter()
            .map(|(_, text)| text)
            .chain(self.automations.iter().map(|(_, text)| text))
            .any(|text| text.contains(path))
    }
}

/// Moves `id`'s folder from `folders` to the trash as `<now>-<number>-<id>`. Blocks.
fn to_trash(
    (attachments, folders): (&OwnedFd, &OwnedFd),
    id: &str,
    number: u32,
    now: i64,
) -> Result<(OwnedFd, String), Errno> {
    let trash = open_dir_at(attachments, OsStr::new(TRASH_DIR), true)?;
    let entry = format!("{now}-{number}-{id}");
    renameat(folders, id, &trash, entry.as_str())?;
    Ok((trash, entry))
}

/// Settles the listed orphan folders (ATR-002) and answers the deleted task's fate; `exists`
/// says whether a task still exists now (a task the namers missed, being unarchived meanwhile).
/// Blocks.
fn settle(
    (attachments, folders): (&OwnedFd, &OwnedFd),
    names: &[OsString],
    (namers, exists): (&Namers, &dyn Fn(&str) -> bool),
    (deleted, number): (&str, u32),
    now: i64,
) -> DeletedAttachments {
    let mut fate = DeletedAttachments::default();
    let mut own_seen = false;
    for name in names {
        let Some(id) = name.to_str().filter(|id| !namers.live.contains(*id)) else {
            continue;
        };
        let own = id == deleted;
        let path = format!("{ATTACHMENTS_DIR}/tasks/{id}/");
        let mut tasks: Vec<u32> = namers
            .tasks
            .iter()
            .filter(|(_, text)| text.contains(&path))
            .map(|(n, _)| *n)
            .collect();
        let automations: Vec<String> = namers
            .automations
            .iter()
            .filter(|(_, text)| text.contains(&path))
            .map(|(n, _)| n.clone())
            .collect();
        own_seen |= own;
        if !tasks.is_empty() || !automations.is_empty() {
            if own {
                tasks.sort_unstable();
                fate.kept_for = tasks;
                fate.kept_for_automations = automations;
            }
            continue;
        }
        // Only a folder about to move costs a question (named ones stay anyway).
        if !own && exists(id) {
            continue;
        }
        match to_trash(
            (attachments, folders),
            id,
            if own { number } else { 0 },
            now,
        ) {
            Ok((trash, entry)) if own => fate.trash = entry_of(&trash, &entry),
            // A parallel deletion's settling moved it first: found in the trash below.
            Err(Errno::ENOENT) if own => own_seen = false,
            Ok(_) | Err(Errno::ENOENT) => {}
            Err(err) => {
                tracing::warn!(error = %err, "attachments not moved to the trash");
                fate.not_moved |= own;
            }
        }
    }
    // A parallel deletion's settling may have moved this task's folder first, without its number.
    if !own_seen && let Ok(trash) = open_dir_at(attachments, OsStr::new(TRASH_DIR), false) {
        let unnumbered = names_in(&trash, MAX_PRUNED_ENTRIES)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|name| name.into_string().ok())
            .find(|name| {
                parse_entry(name).is_some_and(|(_, n, id)| n == 0 && id.as_str() == deleted)
            });
        if let Some(found) = unnumbered
            && let Some((secs, _, _)) = parse_entry(&found)
        {
            let entry = format!("{secs}-{number}-{deleted}");
            if renameat(&trash, found.as_str(), &trash, entry.as_str()).is_ok() {
                fate.trash = entry_of(&trash, &entry);
            }
        }
    }
    fate
}

/// Moves the regular files of the trash entry `from` into the folder `to` under free names,
/// answering their paths below `shown`; removes the entry once it is empty. Blocks.
fn restore(
    (trash, entry): (&OwnedFd, &str),
    to: &OwnedFd,
    shown: &std::path::Path,
) -> AppResult<Restored> {
    let from = open_dir_at(trash, OsStr::new(entry), false).map_err(errno_error)?;
    let files = files_in(&from, MAX_ATTACHMENTS_PER_FOLDER + 1);
    if count_entries(to, MAX_ATTACHMENTS_PER_FOLDER + 1)? + files.len() > MAX_ATTACHMENTS_PER_FOLDER
    {
        return Err(AppError::Conflict(format!(
            "the task would have more than {MAX_ATTACHMENTS_PER_FOLDER} attachments"
        )));
    }
    let mut restored = Restored::default();
    for file in files {
        let placed = (1..=MAX_FREE_NAME_ATTEMPTS).find_map(|n| {
            let name = candidate(&attachment_name(&file), n);
            match link_new_in((&from, &file), to, &name) {
                Ok(true) => Some(Ok(name)),
                Ok(false) => None,
                Err(err) => Some(Err(err)),
            }
        });
        match placed {
            Some(Ok(name)) => {
                let _ = unlinkat(&from, file.as_str(), UnlinkatFlags::NoRemoveDir);
                restored
                    .paths
                    .push(shown.join(&name).to_string_lossy().into_owned());
            }
            // A parallel restore moved it already.
            Some(Err(err)) if err.raw_os_error() == Some(Errno::ENOENT as i32) => {}
            Some(Err(err)) => {
                tracing::warn!(error = %err, file, "an attachment was not restored");
            }
            None => {}
        }
    }
    // Whatever is still in it (a folder, a file it could not move) keeps the entry, and is said.
    restored.left = names_in(&from, MAX_LISTED_FILES)
        .unwrap_or_default()
        .into_iter()
        .map(|name| {
            let folder = kind_at(&from, &name).is_ok_and(|(kind, _)| kind == SFlag::S_IFDIR);
            let name = name.to_string_lossy().into_owned();
            if folder { format!("{name}/") } else { name }
        })
        .collect();
    restored.left.sort();
    match unlinkat(trash, entry, UnlinkatFlags::RemoveDir) {
        Ok(()) | Err(Errno::ENOTEMPTY | Errno::EEXIST | Errno::ENOENT) => {}
        Err(err) => tracing::warn!(error = %err, "a restored trash entry was not removed"),
    }
    Ok(restored)
}

impl App {
    fn now_secs(&self) -> i64 {
        self.now().0 / 1000
    }

    /// Settles the sandbox's attachment folders after `deleted` was deleted (ATR-002) and answers
    /// what became of its own. Store failures keep everything where it is.
    pub(crate) async fn settle_attachments(&self, deleted: &TaskRecord) -> DeletedAttachments {
        let sandbox = &deleted.sandbox_id;
        let sandbox_dir = self.sandbox_dir(sandbox);
        crate::race::reached(
            crate::race::Point::AttachmentsRemove,
            &sandbox_dir
                .join(ATTACHMENTS_DIR)
                .join("tasks")
                .join(deleted.id.as_str()),
        );
        let now = self.now_secs();
        let own = deleted.id.as_str().to_owned();
        // Listed before the tasks are read: a task created meanwhile is then among them.
        let listed = blocking(move || {
            let (attachments, folders) = match open_attachments(&sandbox_dir) {
                Ok(Some(opened)) => opened,
                Ok(None) => return Ok(Ok(None)),
                Err(err) => return Ok(Err(err)),
            };
            purge(&attachments, now);
            let names = folders
                .as_ref()
                .map(|folders| listed_folders(folders, &own, MAX_PRUNED_ENTRIES))
                .unwrap_or_default();
            Ok(Ok(Some((attachments, folders, names))))
        })
        .await;
        let (attachments, folders, names) = match listed {
            Ok(Ok(Some(listed))) => listed,
            // Never looked behind (ADR-0174): the task's folder may be there, and nothing moved it.
            Ok(Err(_)) => {
                return DeletedAttachments {
                    not_moved: true,
                    ..DeletedAttachments::default()
                };
            }
            Ok(Ok(None)) | Err(_) => return DeletedAttachments::default(),
        };
        let (id, number) = (deleted.id.as_str().to_owned(), deleted.number);
        let namers = match self.namers(sandbox).await {
            Ok(namers) => namers,
            Err(err) => {
                tracing::warn!(%sandbox, error = %err, "attachments kept: tasks unreadable");
                return blocking(move || Ok(left_in_place(&attachments, folders.as_ref(), &id)))
                    .await
                    .unwrap_or_default();
            }
        };
        let (store, runtime) = (
            Arc::clone(&self.deps.store),
            tokio::runtime::Handle::current(),
        );
        let tasks_dir = self
            .sandbox_dir(sandbox)
            .join(ATTACHMENTS_DIR)
            .join("tasks");
        blocking(move || {
            let Some(folders) = folders else {
                return Ok(left_in_place(&attachments, None, &id));
            };
            crate::race::reached(crate::race::Point::AttachmentsSettle, &tasks_dir);
            // Asked again for each folder about to move: an unreadable answer keeps it.
            let exists = |id: &str| match TaskId::parse(id) {
                Ok(id) => !matches!(runtime.block_on(store.task(&id)), Ok(None)),
                Err(_) => true,
            };
            Ok(settle(
                (&attachments, &folders),
                &names,
                (&namers, &exists),
                (&id, number),
                now,
            ))
        })
        .await
        .unwrap_or_default()
    }

    /// The sandbox's tasks (archived ones included) and automations, read for what they name.
    pub(crate) async fn namers(&self, sandbox: &SandboxId) -> AppResult<Namers> {
        let mut namers = Namers {
            live: HashSet::new(),
            tasks: Vec::new(),
            automations: Vec::new(),
        };
        for archived in [false, true] {
            for task in self.deps.store.tasks(sandbox, archived).await? {
                namers.live.insert(task.id.as_str().to_owned());
                namers.tasks.push((task.number, task.prompt));
            }
        }
        for automation in self.deps.store.automations(sandbox).await? {
            namers
                .automations
                .push((automation.name, automation.prompt));
        }
        Ok(namers)
    }

    /// The trash of every sandbox purged (start-up and the reaper's hourly round).
    pub(crate) async fn purge_attachment_trash(&self) {
        let (sandboxes, now) = (self.config.data_dir.join("sandboxes"), self.now_secs());
        let _ = blocking(move || {
            let Ok(found) = std::fs::read_dir(&sandboxes) else {
                return Ok(());
            };
            for sandbox in found.flatten().take(MAX_PRUNED_ENTRIES) {
                if SandboxId::parse(&sandbox.file_name().to_string_lossy()).is_err() {
                    continue;
                }
                if let Ok(Some((attachments, _))) = open_attachments(&sandbox.path()) {
                    purge(&attachments, now);
                }
            }
            Ok(())
        })
        .await;
    }

    /// `attachments_trash` (ATR-005): the project's trash, after restoring the entry `restore`
    /// names (a deleted task's id or number) into the task `into` names (the caller's own task by
    /// default).
    ///
    /// # Errors
    /// `Invalid` arguments or an unknown entry; `Forbidden` without write access to restore;
    /// `Conflict` past the folder bound; I/O errors.
    pub async fn tool_attachments_trash(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let sandbox = &caller.sandbox;
        let (sandbox_dir, now) = (self.sandbox_dir(sandbox), self.now_secs());
        // Listing creates nothing: a missing trash is an empty one, a link is refused.
        let (dir, trash, listed) = blocking(move || {
            let dir = std::fs::canonicalize(&sandbox_dir)
                .map_err(|_| AppError::Conflict("the sandbox has no folder yet".into()))?;
            let not_plain =
                || AppError::Forbidden("the attachments trash is not a plain folder".into());
            let attachments = match open_dirs_below(&dir, &[OsStr::new(ATTACHMENTS_DIR)], false) {
                Ok(folder) => folder,
                Err(Errno::ENOENT) => return Ok((dir, None, Vec::new())),
                Err(Errno::ELOOP | Errno::ENOTDIR) => return Err(not_plain()),
                Err(err) => return Err(errno_error(err)),
            };
            purge(&attachments, now);
            match open_dir_at(&attachments, OsStr::new(TRASH_DIR), false) {
                Ok(trash) => {
                    let listed = entries(&trash);
                    Ok((dir, Some(trash), listed))
                }
                Err(Errno::ENOENT) => Ok((dir, None, Vec::new())),
                Err(Errno::ELOOP | Errno::ENOTDIR) => Err(not_plain()),
                Err(err) => Err(errno_error(err)),
            }
        })
        .await?;
        let mut out = json!({});
        if let Some(wanted) = args.get("restore").filter(|v| !v.is_null()) {
            self.manage_authority(caller).await?;
            let (id, number) = match wanted {
                Value::String(text) if !text.trim().is_empty() => (
                    Some(text.trim().to_owned()),
                    text.trim().parse::<u32>().ok(),
                ),
                Value::Number(n) => (None, n.as_u64().and_then(|n| u32::try_from(n).ok())),
                _ => (None, None),
            };
            if id.is_none() && number.is_none() {
                return Err(AppError::Invalid(
                    "`restore` must be a deleted task's id or number".into(),
                ));
            }
            let trash = trash.ok_or_else(|| {
                AppError::Invalid(
                    "the trash is empty: no deleted task's attachments wait there".into(),
                )
            })?;
            let shown_wanted = id
                .clone()
                .or(number.map(|n| format!("#{n}")))
                .unwrap_or_default();
            let found: Vec<&TrashEntry> = listed
                .iter()
                .filter(|e| {
                    id.as_deref() == Some(e.task_id.as_str())
                        || (number.is_some() && e.number == number)
                })
                .collect();
            let entry = match found.as_slice() {
                [entry] => (*entry).clone(),
                [] => {
                    return Err(AppError::Invalid(format!(
                        "no deleted task {shown_wanted} has attachments in the trash"
                    )));
                }
                _ => {
                    return Err(AppError::Invalid(format!(
                        "several trash entries match {shown_wanted}: name it by its task id"
                    )));
                }
            };
            let into = match args.get("into").filter(|v| !v.is_null()) {
                Some(number) => self.task_number_arg(caller, number, "into").await?,
                None => caller.task.clone().ok_or_else(|| {
                    AppError::Invalid("name the task to restore into with `into`".into())
                })?,
            };
            // Files put among a task's attachments are an attachment to it: its owner's
            // (SPEC-490 OWN-004), as `POST /api/tasks/{tid}/attachments` is.
            let ctx = self.manage_context(caller).await?;
            self.require_control(&ctx, &into).await?;
            let folder = into.id.as_str().to_owned();
            let shown = [ATTACHMENTS_DIR, "tasks", &folder]
                .iter()
                .fold(self.sandbox_dir_absolute(&into.sandbox_id), |path, part| {
                    path.join(part)
                });
            let name = entry.name.clone();
            let restored = blocking(move || {
                let (_, to) = ensure_plain_dirs(&dir, &[ATTACHMENTS_DIR, "tasks", &folder])?;
                restore((&trash, &name), &to, &shown)
            });
            let restored = restored.await?;
            let detail = json!({"from_task": entry.task_id, "files": restored.paths.len()});
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    &caller.actor,
                    "attachment.restored",
                    Some(&format!("task:{}", into.id.as_str())),
                    Some(&tool_detail(caller, &detail)),
                )
                .await
            {
                tracing::warn!(task = %into.id, error = %err, "restore audit failed");
            }
            out["restored"] = json!({"into": into.number, "paths": restored.paths});
            if !restored.left.is_empty() {
                out["restored"]["left_in_trash"] = json!(restored.left);
            }
        }
        out["trash"] = json!(self.trash_entries(sandbox).await?);
        out["note"] = json!(format!(
            "A deleted task's attachments stay {TRASH_DAYS} days in the trash; restored files \
             belong to the task they were restored into."
        ));
        Ok(out)
    }

    async fn trash_entries(&self, sandbox: &SandboxId) -> AppResult<Vec<TrashEntry>> {
        let dir = self.sandbox_dir(sandbox);
        blocking(move || {
            Ok(open_attachments(&dir)
                .ok()
                .flatten()
                .and_then(|(attachments, _)| {
                    open_dir_at(&attachments, OsStr::new(TRASH_DIR), false).ok()
                })
                .map(|trash| entries(&trash))
                .unwrap_or_default())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{DeletedAttachments, left_in_place, listed_folders, parse_entry};
    use crate::files::open_dirs_below;

    /// SPEC-413 ANR-003 (#1031): the deleted task's folder is settled even when the listing's cap
    /// stops before it; a link or a file of its name is never taken for it.
    #[test]
    fn the_deleted_tasks_folder_is_listed_past_the_cap() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("t1"))?;
        std::fs::create_dir_all(dir.path().join("t2"))?;
        std::fs::write(dir.path().join("t3"), "")?;
        std::os::unix::fs::symlink(dir.path().join("t2"), dir.path().join("t4"))?;
        let folders = open_dirs_below(dir.path(), &[], false)?;
        assert_eq!(listed_folders(&folders, "t1", 0), ["t1"]);
        for id in ["t3", "t4", "t5"] {
            assert!(listed_folders(&folders, id, 0).is_empty(), "{id}");
        }
        let mut all = listed_folders(&folders, "t1", 10);
        all.sort();
        assert_eq!(all, ["t1", "t2"], "listed once, links and files left out");
        Ok(())
    }

    /// SPEC-397 NMV-002: settling stopped early (the tasks unreadable) says the deleted task's
    /// folder stayed only when it is there as a folder.
    #[test]
    fn settling_stopped_early_says_whether_the_folder_stayed()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let tasks = dir.path().join("tasks");
        std::fs::create_dir_all(tasks.join("t1"))?;
        std::fs::write(tasks.join("t2"), "")?;
        std::os::unix::fs::symlink(tasks.join("t1"), tasks.join("t3"))?;
        let attachments = open_dirs_below(dir.path(), &[], false)?;
        let folders = open_dirs_below(&tasks, &[], false)?;
        let stayed = |id| left_in_place(&attachments, Some(&folders), id).not_moved;
        assert!(stayed("t1"));
        for id in ["t2", "t3", "t4"] {
            assert!(!stayed(id), "{id}");
        }
        assert!(
            left_in_place(&attachments, None, "t1").not_moved,
            "a `tasks` folder that could not be opened may hold it"
        );
        let empty = tempfile::tempdir()?;
        std::fs::write(empty.path().join("tasks"), "")?;
        let empty = open_dirs_below(empty.path(), &[], false)?;
        assert_eq!(
            left_in_place(&empty, None, "t1"),
            DeletedAttachments::default(),
            "a plain file `tasks` holds nothing"
        );
        let linked = tempfile::tempdir()?;
        std::os::unix::fs::symlink(&tasks, linked.path().join("tasks"))?;
        let linked = open_dirs_below(linked.path(), &[], false)?;
        assert!(
            left_in_place(&linked, None, "t1").not_moved,
            "a linked `tasks` may hold it"
        );
        Ok(())
    }

    #[test]
    fn entries_read_back_their_time_number_and_task() {
        let parsed = parse_entry("1762128000-61-7c46cdac-cc5a")
            .map(|(secs, number, id)| (secs, number, id.as_str().to_owned()));
        assert_eq!(
            parsed,
            Some((1_762_128_000, 61, "7c46cdac-cc5a".to_owned()))
        );
        for name in ["x-1-t1", "1--t1", "1-2-", "1-2-a/b", "-1-t1", "12"] {
            assert!(parse_entry(name).is_none(), "{name}");
        }
    }
}
