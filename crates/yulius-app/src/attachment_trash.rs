//! A deleted task's attachments (SPEC-396, ADR-0253; amends TSK-009).
//!
//! Contract:
//! - when a task is deleted, every `attachments/tasks/<id>/` folder of its sandbox whose task no
//!   longer exists is settled: one that a remaining task's request (its prompt with its queued
//!   messages; archived tasks count) names by `attachments/tasks/<id>/` stays where it is, so the
//!   request's paths keep working; any other moves by one rename to
//!   `attachments/trash/<secs>-<number>-<id>/` (deletion time in Unix seconds, number `0` once
//!   unknown);
//! - an entry is removed [`TRASH_DAYS`] days after its deletion: at deletions, at listings, at
//!   start-up and in the workspace reaper's hourly round;
//! - `attachments_trash` lists the project's entries and restores one's regular files into a live
//!   task's folder under free names (write access, the folder bound, audited
//!   `attachment.restored`);
//! - every folder is reached through handles without following a link (ADR-0174, #660): a link
//!   planted on the way moves, removes or restores nothing.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::path::Path;

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
use crate::uploads::{
    ATTACHMENTS_DIR, MAX_ATTACHMENTS_PER_FOLDER, MAX_FREE_NAME_ATTEMPTS, MAX_PRUNED_ENTRIES,
    candidate, count_entries, ensure_plain_dirs, link_new_in,
};

/// Days a deleted task's attachments can be restored.
pub const TRASH_DAYS: i64 = 7;
const TRASH_MAX_AGE_SECS: i64 = TRASH_DAYS * 24 * 3600;
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
    /// They wait in the trash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash: Option<TrashEntry>,
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
    /// Its files (at most [`MAX_LISTED_FILES`] names).
    pub files: Vec<String>,
    #[serde(skip)]
    name: String,
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

/// The regular files of the folder `dir`, sorted, at most `limit`.
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

impl App {
    fn now_secs(&self) -> i64 {
        self.now().0 / 1000
    }

    /// Settles the sandbox's attachment folders after `deleted` was deleted (ATR-002) and answers
    /// what became of its own. Store failures keep everything where it is.
    pub(crate) async fn settle_attachments(&self, deleted: &TaskRecord) -> DeletedAttachments {
        let sandbox = &deleted.sandbox_id;
        let mut fate = DeletedAttachments::default();
        let mut tasks = Vec::new();
        for archived in [false, true] {
            match self.deps.store.tasks(sandbox, archived).await {
                Ok(found) => tasks.extend(found),
                Err(err) => {
                    tracing::warn!(%sandbox, error = %err, "attachments kept: tasks unreadable");
                    return fate;
                }
            }
        }
        let Ok(dir) = std::fs::canonicalize(self.sandbox_dir(sandbox)) else {
            return fate;
        };
        crate::race::reached(
            crate::race::Point::AttachmentsRemove,
            &dir.join(ATTACHMENTS_DIR)
                .join("tasks")
                .join(deleted.id.as_str()),
        );
        let attachments = match open_dirs_below(&dir, &[OsStr::new(ATTACHMENTS_DIR)], false) {
            Ok(folder) => folder,
            Err(Errno::ENOENT) => return fate,
            Err(err) => {
                tracing::warn!(%sandbox, error = %err, "attachments folder is not a plain folder");
                return fate;
            }
        };
        self.purge_trash_in(sandbox, &attachments);
        let Ok(folders) = open_dir_at(&attachments, OsStr::new("tasks"), false) else {
            return fate;
        };
        let live: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        for name in names_in(&folders, MAX_PRUNED_ENTRIES).unwrap_or_default() {
            let Some(id) = name.to_str().filter(|id| !live.contains(id)) else {
                continue;
            };
            if !kind_at(&folders, &name).is_ok_and(|(kind, _)| kind == SFlag::S_IFDIR) {
                continue;
            }
            let path = format!("{ATTACHMENTS_DIR}/tasks/{id}/");
            let mut naming: Vec<u32> = tasks
                .iter()
                .filter(|t| t.prompt.contains(&path))
                .map(|t| t.number)
                .collect();
            let own = id == deleted.id.as_str();
            if !naming.is_empty() {
                if own {
                    naming.sort_unstable();
                    fate.kept_for = naming;
                }
                continue;
            }
            let number = if own { deleted.number } else { 0 };
            let entry = format!("{}-{number}-{id}", self.now_secs());
            let moved = open_dir_at(&attachments, OsStr::new(TRASH_DIR), true)
                .and_then(|trash| renameat(&folders, id, &trash, entry.as_str()).map(|()| trash));
            match moved {
                Ok(trash) if own => fate.trash = entry_of(&trash, &entry),
                Ok(_) | Err(Errno::ENOENT) => {}
                Err(err) => {
                    tracing::warn!(%sandbox, error = %err, "attachments not moved to the trash")
                }
            }
        }
        fate
    }

    /// Removes the entries of a sandbox's trash older than [`TRASH_DAYS`] (ATR-004).
    fn purge_trash_in(&self, sandbox: &SandboxId, attachments: &OwnedFd) {
        let Ok(trash) = open_dir_at(attachments, OsStr::new(TRASH_DIR), false) else {
            return;
        };
        let now = self.now_secs();
        for name in names_in(&trash, MAX_PRUNED_ENTRIES).unwrap_or_default() {
            let old = name
                .to_str()
                .and_then(parse_entry)
                .is_some_and(|(secs, _, _)| now.saturating_sub(secs) > TRASH_MAX_AGE_SECS);
            if old && let Err(err) = remove_tree(&trash, &name) {
                tracing::warn!(%sandbox, error = %err, "could not purge the attachments trash");
            }
        }
    }

    /// The trash of every sandbox purged (start-up and the reaper's hourly round).
    pub(crate) fn purge_attachment_trash(&self) {
        let Ok(sandboxes) = std::fs::read_dir(self.config.data_dir.join("sandboxes")) else {
            return;
        };
        for sandbox in sandboxes.flatten().take(MAX_PRUNED_ENTRIES) {
            let Ok(id) = SandboxId::parse(&sandbox.file_name().to_string_lossy()) else {
                continue;
            };
            if let Ok(attachments) =
                open_dirs_below(&sandbox.path(), &[OsStr::new(ATTACHMENTS_DIR)], false)
            {
                self.purge_trash_in(&id, &attachments);
            }
        }
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
        let dir = std::fs::canonicalize(self.sandbox_dir(sandbox))
            .map_err(|_| AppError::Conflict("the sandbox has no folder yet".into()))?;
        let attachments = ensure_plain_dirs(&dir, &[ATTACHMENTS_DIR])?.1;
        self.purge_trash_in(sandbox, &attachments);
        let trash = open_dir_at(&attachments, OsStr::new(TRASH_DIR), true).map_err(errno_error)?;
        let list = |trash: &OwnedFd| -> Vec<TrashEntry> {
            let mut entries: Vec<TrashEntry> = names_in(trash, MAX_PRUNED_ENTRIES)
                .unwrap_or_default()
                .iter()
                .filter_map(|name| entry_of(trash, name.to_str()?))
                .collect();
            entries.sort_by(|a, b| b.name.cmp(&a.name));
            entries
        };
        let mut out = json!({});
        if let Some(wanted) = args.get("restore").filter(|v| !v.is_null()) {
            self.manage_authority(caller).await?;
            let matches = |e: &TrashEntry| match wanted {
                Value::String(id) => e.task_id == *id,
                Value::Number(n) => n
                    .as_u64()
                    .is_some_and(|n| e.number.map(u64::from) == Some(n)),
                _ => false,
            };
            let found: Vec<TrashEntry> = list(&trash).into_iter().filter(matches).collect();
            let entry = match found.as_slice() {
                [entry] => entry.clone(),
                [] => {
                    return Err(AppError::Invalid(format!(
                        "no deleted task {wanted} has attachments in the trash"
                    )));
                }
                _ => {
                    return Err(AppError::Invalid(format!(
                        "several trash entries match {wanted}: name it by its task id"
                    )));
                }
            };
            let into = match args.get("into").filter(|v| !v.is_null()) {
                Some(number) => self.task_number_arg(caller, number, "into").await?,
                None => caller.task.clone().ok_or_else(|| {
                    AppError::Invalid("name the task to restore into with `into`".into())
                })?,
            };
            let paths = self.restore_entry((&dir, &trash), &entry, &into)?;
            let detail = json!({"from_task": entry.task_id, "files": paths.len()});
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
            out["restored"] = json!({"into": into.number, "paths": paths});
        }
        out["trash"] = json!(list(&trash));
        out["note"] = json!(format!(
            "A deleted task's attachments stay {TRASH_DAYS} days in the trash; restored files \
             belong to the task they were restored into."
        ));
        Ok(out)
    }

    /// Moves `entry`'s regular files into `into`'s attachments folder under free names and removes
    /// the entry; answers the files' new paths.
    fn restore_entry(
        &self,
        (dir, trash): (&Path, &OwnedFd),
        entry: &TrashEntry,
        into: &TaskRecord,
    ) -> AppResult<Vec<String>> {
        let from = open_dir_at(trash, OsStr::new(&entry.name), false).map_err(errno_error)?;
        let parts = [ATTACHMENTS_DIR, "tasks", into.id.as_str()];
        let (_, to) = ensure_plain_dirs(dir, &parts)?;
        let files = files_in(&from, MAX_ATTACHMENTS_PER_FOLDER + 1);
        if count_entries(&to, MAX_ATTACHMENTS_PER_FOLDER + 1)? + files.len()
            > MAX_ATTACHMENTS_PER_FOLDER
        {
            return Err(AppError::Conflict(format!(
                "task #{} would have more than {MAX_ATTACHMENTS_PER_FOLDER} attachments",
                into.number
            )));
        }
        let shown = parts
            .iter()
            .fold(self.sandbox_dir_absolute(&into.sandbox_id), |path, part| {
                path.join(part)
            });
        let mut paths = Vec::new();
        for file in &files {
            for n in 1..=MAX_FREE_NAME_ATTEMPTS {
                let name = candidate(file, n);
                if link_new_in((&from, file), &to, &name).map_err(|e| crate::files::io_error(&e))? {
                    let _ = unlinkat(&from, file.as_str(), UnlinkatFlags::NoRemoveDir);
                    paths.push(shown.join(&name).to_string_lossy().into_owned());
                    break;
                }
            }
        }
        // An entry keeps what could not be restored; once emptied, it goes with what is left in it
        // that is not a regular file.
        if paths.len() == files.len() {
            remove_tree(trash, OsStr::new(&entry.name))?;
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_entry;

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
