//! Forked tasks (SPEC-628): a new task continues a copy of another task's conversation from its
//! branch's head, on the same harness, and both go on separately.
//!
//! The source is only read: its record, branch ref, latest session and (Claude Code) transcript.
//! At fork time Yulius records the source's head commit and session ([`ForkRecord`]) and stores
//! the transcript's complete lines under `<data>/transcripts/<fork id>/`. The fork's first
//! launch starts its worktree at that commit, places the copy in its worktree's project folder
//! with `cwd` rewritten, and resumes it as a fork (`SessionMode::Resume { fork }`).

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_contracts::api::tasks::{ForkLink, ForkOrigin, ForkTask, TaskSummary};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, StepId};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::intake::ForkRecord;
use crate::lifecycle::{MAX_PROMPT_BYTES, NewTask, model_of_its_harness};
use crate::model::{StepRecord, TaskOrigin, TaskRecord, UserRecord};
use crate::ports::harness::SessionFork;

/// The largest transcript a fork copies (as a continued session's copy, 1 GiB).
pub(crate) const MAX_FORK_TRANSCRIPT_BYTES: u64 = 1 << 30;

/// What a fork needs written once its task exists and before it starts.
pub(crate) struct PendingFork {
    /// The record, its `task_id` filled in once the task exists.
    pub(crate) record: ForkRecord,
    /// The staged copy of the source transcript (Claude Code): a folder holding
    /// `<session>.jsonl` and its side files, moved to `<data>/transcripts/<fork id>`.
    pub(crate) staged: Option<PathBuf>,
    /// The source's steps, copied with their statuses and notes.
    pub(crate) steps: Vec<StepRecord>,
    /// The subscription the person named for its first run.
    pub(crate) account: Option<AccountId>,
}

/// The refusal for a harness whose CLI cannot fork at launch (SPEC-628 FRK-003).
fn unforkable(harness: Harness) -> Option<AppError> {
    match harness {
        Harness::Antigravity => Some(AppError::Invalid(
            "Antigravity cannot fork a conversation at launch: its /fork works only inside a \
             running session"
                .into(),
        )),
        Harness::ClaudeCode | Harness::Codex => None,
    }
}

/// "Fork of #N: <title>", within the 120 characters of a title.
fn fork_title(source: &TaskRecord) -> String {
    format!("Fork of #{}: {}", source.number, source.title)
        .chars()
        .take(120)
        .collect()
}

impl App {
    /// `POST /api/tasks/{tid}/fork` (SPEC-628 FRK-001).
    ///
    /// # Errors
    /// Access errors; `Invalid` for a source that cannot be forked; as task creation.
    pub async fn fork_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ForkTask,
    ) -> AppResult<TaskSummary> {
        // The source only needs to be seen: nothing of it changes, and the fork is the
        // forker's, who needs work access to the project (checked by `fork_task_as`).
        let source = self
            .load_task_to(ctx, id, yulius_domain::task_access::TaskAction::View)
            .await?;
        let task = self
            .fork_task_as(&ctx.user, &source, req, TaskOrigin::Ui)
            .await?;
        self.task_summary(&task).await
    }

    /// Forks `source` on `user`'s authority, from the page or an agent tool (FRK-001..004). The
    /// caller checked that `user` may work on `source`.
    ///
    /// # Errors
    /// `Invalid` for an Antigravity source, a source without a session or a branch commit, a
    /// message over 64 KiB, a model of another harness or an unknown account; `Capacity` for a
    /// subscription of another provider; as task creation.
    pub(crate) async fn fork_task_as(
        self: &Arc<Self>,
        user: &UserRecord,
        source: &TaskRecord,
        req: &ForkTask,
        origin: TaskOrigin,
    ) -> AppResult<TaskRecord> {
        let store = &self.deps.store;
        self.require(user, &source.sandbox_id, Action::Work).await?;
        if let Some(refusal) = unforkable(source.harness) {
            return Err(refusal);
        }
        let message = req
            .message
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        if message.is_some_and(|m| m.len() > MAX_PROMPT_BYTES) {
            return Err(AppError::Invalid(
                "the message must be at most 64 KiB".into(),
            ));
        }
        let model = req
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        model_of_its_harness(source.harness, model)?;
        let account = self.fork_account(source.harness, req).await?;
        let session = store
            .provider_sessions(&source.id)
            .await?
            .into_iter()
            .rev()
            .find(|entry| entry.harness == source.harness)
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "#{} has no conversation to fork yet: start it first",
                    source.number
                ))
            })?;
        let branch = source.branch.clone().ok_or_else(|| no_commit(source))?;
        let repo = self.sandbox_repo(&source.sandbox_id);
        let commit = if self.deps.git.valid_branch(&branch) {
            self.deps
                .git
                .resolve_commit(&repo, &format!("refs/heads/{branch}"))
                .await?
        } else {
            None
        }
        .ok_or_else(|| no_commit(source))?;
        self.ensure_tasks_may_start(&source.sandbox_id, origin)
            .await?;
        self.ensure_agent_account(source.harness.provider()).await?;
        let staged = match source.harness {
            Harness::ClaudeCode => Some(
                self.stage_fork_transcript(source, &session.session_id)
                    .await?,
            ),
            Harness::Codex | Harness::Antigravity => None,
        };
        let steps = store.steps(&source.id).await?;
        let pending = PendingFork {
            record: ForkRecord {
                task_id: source.id.clone(),
                source_task_id: Some(source.id.clone()),
                source_number: source.number,
                source_title: source.title.clone(),
                source_branch: branch,
                source_commit: commit,
                source_session_id: session.session_id.clone(),
                message: message.map(str::to_owned),
                transcript_ref: None,
                created_at: self.now(),
            },
            staged: staged.clone(),
            steps,
            account,
        };
        let req = NewTask {
            sandbox: source.sandbox_id.clone(),
            created_by: user.id.clone(),
            prompt: source.prompt.clone(),
            harness: source.harness,
            title: req
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .or_else(|| Some(fork_title(source))),
            workflow_id: source.workflow_id.clone(),
            steps: Vec::new(),
            model: model.map(str::to_owned),
            effort: req.effort.clone().filter(|e| !e.trim().is_empty()),
            origin,
            automation_run: None,
            start: true,
            mode: source.mode,
            supertask: false,
        };
        let created = self.create_task_forked(req, pending).await;
        if created.is_err()
            && let Some(dir) = staged
        {
            let _ = crate::projects::blocking(move || {
                fs::remove_dir_all(&dir).map_err(AppError::internal)
            })
            .await;
        }
        created
    }

    /// The subscription a fork asks for: one of its harness's provider (as Continue on… checks
    /// it, SPEC-447 MOV-002), or the pool's choice.
    async fn fork_account(&self, harness: Harness, req: &ForkTask) -> AppResult<Option<AccountId>> {
        let Some(raw) = req
            .account_id
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        else {
            return Ok(None);
        };
        let id = AccountId::parse(raw).map_err(|_| AppError::Invalid("invalid account".into()))?;
        match self.deps.store.account(&id).await? {
            None => Err(AppError::Capacity(
                "That subscription no longer exists.".into(),
            )),
            Some(record) if record.provider != harness.provider() => {
                Err(AppError::Capacity(format!(
                    "{} is not {} subscription, so it cannot take the fork.",
                    record.label,
                    crate::account_gate::a_kind(harness.provider())
                )))
            }
            Some(_) => Ok(Some(id)),
        }
    }

    /// Copies the complete lines of the source's Claude Code transcript and its side files to a
    /// staging folder of the server's (FRK-004). The transcript is in the sandbox home, which
    /// agents write: it is read through handles (ADR-0315).
    async fn stage_fork_transcript(
        &self,
        source: &TaskRecord,
        session: &str,
    ) -> AppResult<PathBuf> {
        let worktree = source
            .worktree_path
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| no_conversation(source))?;
        let path = self.deps.harness.claude_transcript(
            &self.sandbox_home(&source.sandbox_id),
            &worktree,
            session,
        );
        let staging = self
            .config
            .data_dir
            .join("transcripts")
            .join(".staging")
            .join(self.deps.crypto.new_id());
        let anchor = self.sandbox_dir(&source.sandbox_id);
        let main = staging.join(format!("{session}.jsonl"));
        let aux = (path.with_extension(""), staging.join(session));
        let number = source.number;
        let stage_dir = staging.clone();
        crate::projects::blocking(move || {
            let file = crate::transcripts::open(&anchor, &path).map_err(|err| {
                if crate::transcripts::unreadable(&err) {
                    AppError::Invalid(format!(
                        "#{number} has no conversation to fork yet: its transcript is not there"
                    ))
                } else {
                    AppError::internal(err)
                }
            })?;
            let copied = copy_complete_lines(file, &main, MAX_FORK_TRANSCRIPT_BYTES)?;
            crate::session_docs::copy_dir_below(
                &anchor,
                &aux.0,
                &aux.1,
                MAX_FORK_TRANSCRIPT_BYTES.saturating_sub(copied),
            )
        })
        .await
        .inspect_err(|_| {
            let _ = fs::remove_dir_all(&stage_dir);
        })?;
        Ok(staging)
    }

    /// Writes what a new fork needs before it starts (called by task creation once the task
    /// exists): its stored transcript, its record and its copied plan.
    pub(crate) async fn record_fork(
        &self,
        task: &TaskRecord,
        mut pending: PendingFork,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let now = self.now();
        pending.record.task_id = task.id.clone();
        if let Some(staged) = pending.staged.take() {
            let dest = self
                .config
                .data_dir
                .join("transcripts")
                .join(task.id.as_str());
            let (from, to) = (staged, dest.clone());
            crate::projects::blocking(move || crate::imports::move_tree(&from, &to)).await?;
            pending.record.transcript_ref = Some(
                dest.join(format!("{}.jsonl", pending.record.source_session_id))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        store.insert_fork(&pending.record).await?;
        if !pending.steps.is_empty() {
            let mut steps = Vec::with_capacity(pending.steps.len());
            for step in pending.steps {
                steps.push(StepRecord {
                    id: StepId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
                    task_id: task.id.clone(),
                    updated_by: Some("system".into()),
                    updated_at: Some(now),
                    ..step
                });
            }
            store.replace_steps(&task.id, &steps, "system", now).await?;
        }
        Ok(())
    }

    /// The commit a fork's worktree starts from, while it has none (FRK-005).
    pub(crate) async fn fork_base(&self, task: &TaskRecord) -> Option<String> {
        let fork = self.deps.store.fork_of(&task.id).await.ok()??;
        Some(fork.source_commit)
    }

    /// The fork to launch: a fork whose own session has not started yet (FRK-006). A Claude
    /// Code fork gets its stored copy placed for its worktree first, with `cwd` rewritten.
    ///
    /// # Errors
    /// I/O failures placing the copy.
    pub(crate) async fn fork_to_launch(
        &self,
        task: &TaskRecord,
        worktree: &Path,
    ) -> AppResult<Option<ForkRecord>> {
        let store = &self.deps.store;
        if !store.provider_sessions(&task.id).await?.is_empty() {
            return Ok(None);
        }
        let Some(fork) = store.fork_of(&task.id).await? else {
            return Ok(None);
        };
        if task.harness == Harness::ClaudeCode
            && let Some(stored) = fork.transcript_ref.clone()
        {
            let sandbox_dir = self.sandbox_dir(&task.sandbox_id);
            let dest = self.deps.harness.claude_transcript(
                &sandbox_dir.join("home"),
                worktree,
                &fork.source_session_id,
            );
            let cwd = worktree.to_string_lossy().into_owned();
            let random = self.deps.crypto.random_bytes();
            let suffix = crate::auth::hex_token(random.get(..8).unwrap_or(&random));
            crate::projects::blocking(move || {
                crate::imports::place_claude_transcript(
                    &sandbox_dir,
                    &dest,
                    Path::new(&stored),
                    &cwd,
                    &suffix,
                )
            })
            .await?;
        }
        Ok(Some(fork))
    }

    /// How a fork's first launch resumes: a copy of the source's session, under an id Yulius
    /// names where the harness lets it (FRK-006).
    pub(crate) fn fork_session(&self, harness: Harness) -> SessionFork {
        SessionFork {
            session_id: (harness == Harness::ClaudeCode).then(|| self.deps.crypto.new_id()),
        }
    }

    /// The fork links of a task's summary (FRK-008).
    pub(crate) async fn fork_links(
        &self,
        task: &TaskRecord,
    ) -> (Option<Box<ForkOrigin>>, Vec<ForkLink>) {
        let store = &self.deps.store;
        let origin = store.fork_of(&task.id).await.ok().flatten().map(|f| {
            Box::new(ForkOrigin {
                task_id: f.source_task_id.map(|id| id.to_string()),
                number: f.source_number,
                title: f.source_title,
                branch: f.source_branch,
                commit: f.source_commit,
                session_id: f.source_session_id,
                forked_at: crate::auth::rfc3339(f.created_at),
            })
        });
        let mut forks = Vec::new();
        for f in store.forks_from(&task.id).await.unwrap_or_default() {
            if let Ok(Some(fork)) = store.task(&f.task_id).await {
                forks.push(ForkLink {
                    task_id: fork.id.to_string(),
                    number: fork.number,
                    title: fork.title,
                    status: fork.status.as_str().to_owned(),
                    commit: f.source_commit,
                });
            }
        }
        (origin, forks)
    }
}

fn no_commit(source: &TaskRecord) -> AppError {
    AppError::Invalid(format!(
        "#{} has no commit on its branch to fork from yet",
        source.number
    ))
}

fn no_conversation(source: &TaskRecord) -> AppError {
    AppError::Invalid(format!(
        "#{} has no conversation to fork yet: start it first",
        source.number
    ))
}

/// The first message of a fork's first run (FRK-007).
pub(crate) fn fork_opening(task: &TaskRecord, fork: &ForkRecord) -> String {
    opening_text(
        task.number,
        task.branch.as_deref(),
        task.spec_path.as_deref(),
        fork,
    )
}

fn opening_text(
    number: u32,
    branch: Option<&str>,
    spec: Option<&str>,
    fork: &ForkRecord,
) -> String {
    let short: String = fork.source_commit.chars().take(12).collect();
    let branch = branch.unwrap_or("its own branch");
    let spec = spec.map_or_else(String::new, |path| format!(" Your spec is `{path}`."));
    let ask = fork
        .message
        .as_deref()
        .unwrap_or("Continue from where the conversation stopped.");
    format!(
        "You are now task #{number}, a fork of task #{source}: you continue this conversation \
         separately, on your own branch `{branch}`, which starts at #{source}'s commit {short} \
         (its uncommitted changes did not come with you). Task #{source} goes on by itself on \
         `{source_branch}`: its branch, pull request and spec are not yours, so leave them \
         alone.{spec} Your steps are a copy of #{source}'s plan.\n\n{ask}",
        source = fork.source_number,
        source_branch = fork.source_branch,
    )
}

/// Copies the lines of `file` up to its last newline to `dest` (a new file), at most `max`
/// bytes: a transcript being written ends in a line still being written, which is left out.
/// Returns the bytes copied.
///
/// # Errors
/// `Invalid` past `max`; I/O failures.
pub(crate) fn copy_complete_lines(file: fs::File, dest: &Path, max: u64) -> AppResult<u64> {
    let too_large = || {
        AppError::Invalid(
            "this conversation's transcript is larger than 1 GiB; it is not copied".into(),
        )
    };
    if file.metadata().map_err(AppError::internal)?.len() > max {
        return Err(too_large());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(AppError::internal)?;
    }
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(AppError::internal)?;
    let mut reader = BufReader::new(file.take(max.saturating_add(1)));
    let (mut written, mut line) = (0_u64, Vec::new());
    let mut copy = || -> io::Result<Option<u64>> {
        loop {
            line.clear();
            let n = reader.read_until(b'\n', &mut line)?;
            if n == 0 || !line.ends_with(b"\n") {
                return Ok(Some(written));
            }
            written += n as u64;
            if written > max {
                return Ok(None);
            }
            out.write_all(&line)?;
        }
    };
    match copy().map_err(AppError::internal)? {
        Some(copied) => Ok(copied),
        None => {
            let _ = fs::remove_file(dest);
            Err(too_large())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::time::Timestamp;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_copy_leaves_out_the_line_still_being_written() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("s.jsonl");
        fs::write(&src, "{\"a\":1}\n{\"b\":2}\n{\"c\":")?;
        let dest = dir.path().join("out/s.jsonl");
        let copied = copy_complete_lines(fs::File::open(&src)?, &dest, 1 << 20)?;
        assert_eq!(fs::read_to_string(&dest)?, "{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(copied, 16);
        let refused = copy_complete_lines(fs::File::open(&src)?, &dir.path().join("x"), 10);
        assert!(matches!(refused, Err(AppError::Invalid(_))));
        assert!(!dir.path().join("x").exists(), "nothing is left behind");
        Ok(())
    }

    #[test]
    fn the_opening_names_the_fork_its_source_and_the_ask() {
        let fork = ForkRecord {
            task_id: yulius_domain::ids::TaskId::parse("t2").unwrap_or_else(|_| unreachable!()),
            source_task_id: None,
            source_number: 3,
            source_title: "Parser".into(),
            source_branch: "feat/3-parser".into(),
            source_commit: "0123456789abcdef0123".into(),
            source_session_id: "s".into(),
            message: Some("Try a hand-written parser instead.".into()),
            transcript_ref: None,
            created_at: Timestamp(1),
        };
        let (branch, spec) = (
            Some("feat/7-fork-of-3-parser"),
            Some(".ai/specs/SPEC-9-fork.md"),
        );
        let opening = opening_text(7, branch, spec, &fork);
        for part in [
            "task #7, a fork of task #3",
            "`feat/7-fork-of-3-parser`",
            "commit 0123456789ab ",
            "`feat/3-parser`",
            "Your spec is `.ai/specs/SPEC-9-fork.md`.",
            "\n\nTry a hand-written parser instead.",
        ] {
            assert!(opening.contains(part), "{part} in {opening}");
        }
        let plain = opening_text(
            7,
            branch,
            spec,
            &ForkRecord {
                message: None,
                ..fork
            },
        );
        assert!(plain.ends_with("Continue from where the conversation stopped."));
    }
}
