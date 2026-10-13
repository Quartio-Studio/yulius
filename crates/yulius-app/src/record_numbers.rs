//! Numbered records given out by Yulius (SPEC-083 NUM-001 to NUM-004): decision records, specs,
//! migrations — any directory whose files start with a sequence number.
//!
//! Parallel tasks each took the next free number of their own checkout. The second to merge
//! failed its checks ("shares decision number"), renumbered, and often collided again with the
//! next one to merge: 81 decision records were renamed in 36 of 74 sessions, and 33 of 57 merges
//! needed a review round for a renumbering or a dev merge (F 2026-10-13). `record_number` gives
//! each task a number no one else holds: past every numbered file of the directory on the
//! integration branch, in every live task's checkout (uncommitted files included) or pushed
//! branch, and past every number given out before. A task asking again before it used its number
//! gets the same one back.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};
use yulius_domain::numbering::{self, Convention};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{RecordReservation, SandboxRecord, TaskRecord};

/// Most entries read from one directory of one checkout.
const MAX_ENTRIES: usize = 10_000;
/// Most numbers tried when another process reserved the next one first.
const MAX_ATTEMPTS: u64 = 16;
/// Longest accepted directory argument.
const MAX_DIR_BYTES: usize = 512;

/// A number given to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GivenNumber {
    /// How the directory numbers its files.
    pub convention: Convention,
    /// The number.
    pub number: u64,
    /// Given to this task before, and no file uses it yet.
    pub again: bool,
}

impl GivenNumber {
    /// The number as the directory writes it (`0066`, `SPEC-083`).
    pub(crate) fn formatted(&self) -> String {
        self.convention.format(self.number)
    }
}

/// Serializes reservations: the next free number is read and reserved as one step.
fn reserving() -> &'static tokio::sync::Mutex<()> {
    static RESERVING: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    RESERVING.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// A directory argument: repository-relative, normal components only, no trailing `/`.
fn directory(text: &str) -> Option<&str> {
    let dir = text.trim().trim_end_matches('/');
    (!dir.is_empty()
        && dir.len() <= MAX_DIR_BYTES
        && !dir.contains('\0')
        && Path::new(dir)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))))
    .then_some(dir)
}

/// File names directly in `dir` of a checkout on disk (bounded); none when it is missing.
async fn names_on_disk(checkout: &Path, dir: &str) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(checkout.join(dir)).await else {
        return names;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if names.len() >= MAX_ENTRIES {
            break;
        }
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names
}

impl App {
    /// The base the integration branch's files are read from: `origin/<integration>`, or the
    /// default branch while the integration branch does not exist.
    async fn record_base(&self, sandbox: &SandboxRecord, repo: &Path) -> String {
        let integration = crate::delivery::delivery_profile(sandbox).integration_branch;
        let exists = self
            .deps
            .git
            .branch_exists(repo, &integration)
            .await
            .unwrap_or(true);
        format!(
            "origin/{}",
            if exists {
                integration
            } else {
                sandbox.default_branch.clone()
            }
        )
    }

    /// Gives `task` the next free number of `dir`'s numbered files (with `prefix`, or the
    /// directory's main series) and reserves it (NUM-002). `fetch` refreshes the sandbox clone
    /// first, so the integration branch read is current. `Ok(None)` when no file is numbered
    /// that way anywhere.
    ///
    /// # Errors
    /// The integration branch cannot be read; store failures.
    pub(crate) async fn reserve_record_number(
        &self,
        task: &TaskRecord,
        dir: &str,
        prefix: Option<&str>,
        fetch: bool,
    ) -> AppResult<Option<GivenNumber>> {
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let repo = self.sandbox_repo(&sandbox.id);
        // Outside the lock: a slow fetch never holds up another task's number or start.
        if fetch && let Err(err) = self.dlv_refresh_clone(&sandbox).await {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "the clone could not be refreshed before giving out a record number");
        }
        let _one_at_a_time = reserving().lock().await;
        let base = self.record_base(&sandbox, &repo).await;
        let mut names = self.deps.git.list_dir(&repo, &base, dir).await?;
        // Every live task's files: its checkout (uncommitted files too), else its pushed branch.
        for other in self.deps.store.tasks(&task.sandbox_id, false).await? {
            let mut found = match other.worktree_path.as_deref().map(Path::new) {
                Some(worktree) if worktree.is_dir() => names_on_disk(worktree, dir).await,
                _ => match other.branch.as_deref() {
                    Some(branch) => self
                        .deps
                        .git
                        .list_dir(&repo, &format!("origin/{branch}"), dir)
                        .await
                        .unwrap_or_default(),
                    None => Vec::new(),
                },
            };
            // A spec path Yulius gave out before reservations existed (SPEC-020 SPC-001).
            if let Some((parent, name)) =
                other.spec_path.as_deref().and_then(|p| p.rsplit_once('/'))
                && parent == dir
            {
                found.push(name.to_owned());
            }
            names.extend(found);
        }
        let Some(convention) = numbering::convention(names.iter().map(String::as_str), prefix)
        else {
            return Ok(None);
        };
        let in_files: BTreeSet<u64> =
            numbering::numbers(names.iter().map(String::as_str), &convention.prefix)
                .into_iter()
                .collect();
        let reserved: Vec<RecordReservation> = self
            .deps
            .store
            .record_reservations(&task.sandbox_id, dir)
            .await?
            .into_iter()
            .filter(|r| r.prefix == convention.prefix)
            .collect();
        // Asked again before using it: the same number (NUM-003).
        if let Some(unused) = reserved
            .iter()
            .filter(|r| r.task_id == task.id && !in_files.contains(&r.number))
            .map(|r| r.number)
            .min()
        {
            return Ok(Some(GivenNumber {
                convention,
                number: unused,
                again: true,
            }));
        }
        let mut number = numbering::next_free(
            in_files
                .iter()
                .copied()
                .chain(reserved.iter().map(|r| r.number)),
        );
        for _ in 0..MAX_ATTEMPTS {
            let reservation = RecordReservation {
                sandbox_id: task.sandbox_id.clone(),
                dir: dir.to_owned(),
                prefix: convention.prefix.clone(),
                number,
                task_id: task.id.clone(),
                reserved_at: self.now(),
            };
            if self.deps.store.reserve_record_number(&reservation).await? {
                tracing::info!(task = %task.id, dir, number = %convention.format(number), "record number given out");
                return Ok(Some(GivenNumber {
                    convention,
                    number,
                    again: false,
                }));
            }
            number = number.saturating_add(1);
        }
        Err(AppError::Conflict(format!(
            "no free number was found in `{dir}`; ask again"
        )))
    }

    /// `record_number`: the number for a new numbered file in `dir` (NUM-001), reserved for this
    /// task.
    ///
    /// # Errors
    /// `Invalid` arguments or a directory with no numbered files; the integration branch cannot
    /// be read; store failures.
    pub async fn tool_record_number(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let dir = args
            .get("dir")
            .and_then(Value::as_str)
            .and_then(directory)
            .ok_or_else(|| {
                AppError::Invalid(
                    "give `dir`, the repository-relative directory of the numbered files (for \
                     example `docs/decisions`)"
                        .into(),
                )
            })?;
        let prefix = match args.get("prefix") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if text.len() <= numbering::MAX_PREFIX_CHARS => {
                Some(text.as_str())
            }
            Some(_) => {
                return Err(AppError::Invalid(format!(
                    "`prefix` is the text before the number (for example `SPEC-`), at most {} \
                     characters",
                    numbering::MAX_PREFIX_CHARS
                )));
            }
        };
        let Some(given) = self
            .reserve_record_number(&ctx.task, dir, prefix, true)
            .await?
        else {
            return Err(AppError::Invalid(format!(
                "no file in `{dir}` starts with a sequence number{}, on the integration branch or \
                 in any task; a new series needs no reservation: start it at 1",
                prefix.map(|p| format!(" after `{p}`")).unwrap_or_default()
            )));
        };
        let formatted = given.formatted();
        let file = format!("{dir}/{}", given.convention.example(given.number));
        let next = if given.again {
            format!(
                "{formatted} is still reserved for this task and no file uses it yet: name the file \
                 `{file}`."
            )
        } else {
            format!(
                "{formatted} is reserved for this task: name the file `{file}` now, and write \
                 {formatted} wherever the record is numbered (its title, an index). Parallel tasks \
                 get other numbers, so never renumber it yourself. Ask again for each further file \
                 once this one exists."
            )
        };
        Ok(json!({
            "dir": dir,
            "number": formatted,
            "file": file,
            "again": given.again,
            "next": next,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::directory;

    #[test]
    fn directories_are_repository_relative() {
        assert_eq!(directory("docs/decisions/"), Some("docs/decisions"));
        assert_eq!(directory(" .ai/specs "), Some(".ai/specs"));
        for bad in ["", "/", "/etc", "../x", "docs/../x", "./docs", "a\0b"] {
            assert_eq!(directory(bad), None, "{bad:?}");
        }
    }
}
