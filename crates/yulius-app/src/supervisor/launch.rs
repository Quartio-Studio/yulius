//! Launching a run: the sandbox's folders and worktree, the run's home, environment and
//! credential, and `launch_attempt`, which spawns it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::harness::{DEFAULT_CLAUDE_MODEL, Harness};
use yulius_domain::ids::{AccountId, AttemptId, SandboxId};
use yulius_domain::task::{Activity, AttemptState, TaskEvent, TaskStatus};
use yulius_domain::usage::ModelFamily;

use super::antigravity::{
    ANTIGRAVITY_GIVEN, AntigravityToken, antigravity_bind, antigravity_token_path,
    tokened_servers_left_out,
};
use super::codex::{CodexLeftover, write_codex_files};
use super::start::IdleRest;
use super::{
    CLAUDE_CREDENTIAL_DIGEST, GOAL_COMMAND, InputReady, LAUNCH_GOAL_FILE, OPENING_PENDING_FILE,
    PERSONAL_MCP_FILE, PendingGoal, Placed, PromptReceipts, RunStart, TYPED_GOAL_FILE,
    TYPED_GOAL_OPENS, model_change_notice, sets_goal, switched_back_notice, write_private_file,
};
use crate::auth::hex_token;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{
    AccountRecord, AttemptRecord, LaunchKind, PauseKind, ProviderSessionRecord, SandboxRecord,
    Secret, TaskMode, TaskRecord,
};
use crate::ports::harness::{HarnessLaunch, SessionMode};
use crate::ports::session::Containment;
use crate::ports::store::{AttemptUpdate, TaskFieldUpdate};

/// The Git credential helper command line for `exe` (shell-quoted for Git's `!` form).
fn credential_helper(exe: &Path) -> String {
    let quoted = exe.to_string_lossy().replace('\'', "'\\''");
    format!("!'{quoted}' git-credential")
}

/// Git configuration (environment form) that makes Git ask Yulius for GitHub credentials
/// through the helper (never stored on disk): agents get installation tokens, user processes
/// the user's own token (SPEC-002 ADR-12).
pub(crate) fn git_credential_env(exe: &Path) -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".into(), "3".into()),
        (
            "GIT_CONFIG_KEY_0".into(),
            "credential.https://github.com.helper".into(),
        ),
        ("GIT_CONFIG_VALUE_0".into(), String::new()),
        (
            "GIT_CONFIG_KEY_1".into(),
            "credential.https://github.com.helper".into(),
        ),
        ("GIT_CONFIG_VALUE_1".into(), credential_helper(exe)),
        (
            "GIT_CONFIG_KEY_2".into(),
            "credential.https://github.com.useHttpPath".into(),
        ),
        ("GIT_CONFIG_VALUE_2".into(), "true".into()),
    ]
}

/// Branch name for a task: `feat/<number>-<slug>`.
#[must_use]
pub fn branch_for(number: u32, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("feat/{number}-task")
    } else if !slug.bytes().any(|b| b.is_ascii_lowercase()) {
        // The branch policy wants a word after the number (SPEC-069).
        format!("feat/{number}-task-{slug}")
    } else {
        format!("feat/{number}-{slug}")
    }
}

/// Writes a planned file of the server's own run directory (no agent wrote there yet), with its
/// mode from its creation on.
fn write_private(file: &crate::ports::session::PlannedFile) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(file.mode)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(&file.path)?;
    out.set_permissions(std::fs::Permissions::from_mode(file.mode))?;
    out.write_all(&file.contents)
}

/// Makes the sandbox's home, `<sandbox_dir>/home`, when it is missing. The home is the agent's:
/// it is made with `mkdirat` from the sandbox directory and opened without following a link, so a
/// `home` link, dangling or to a host folder, fails the start and is never taken as the home
/// (#661, ADR-0174).
fn make_home(sandbox_dir: &Path) -> AppResult<()> {
    std::fs::create_dir_all(sandbox_dir).map_err(AppError::internal)?;
    crate::files::open_dirs_below(sandbox_dir, &[std::ffi::OsStr::new("home")], true)
        .map(drop)
        .map_err(|err| match err {
            nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => AppError::internal(
                "the sandbox's home is a link or not a folder: Yulius follows no links",
            ),
            other => crate::files::errno_error(other),
        })
}

impl App {
    pub(crate) fn sandbox_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.config
            .data_dir
            .join("sandboxes")
            .join(sandbox.as_str())
    }

    pub(crate) fn sandbox_repo(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("repo")
    }

    /// The lock every launch holds; a skill refresh takes it too, so it never installs into a
    /// home while a launch does (SPEC-507 SKR-005). Owned: the install that holds it runs on the
    /// blocking pool and keeps it until it ends, even when the request that started it is gone.
    pub(crate) async fn launch_lock(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.supervisor.launch).lock_owned().await
    }

    pub(crate) fn sandbox_home(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("home")
    }

    /// The memory guard of a contained task run (SPEC-652 MEM-001): Claude Code's memory folder,
    /// `CLAUDE.md` and `rules` in the sandbox home's `.claude`, Codex's instruction files and
    /// memories in its home, or Antigravity's rules in the run's own home.
    fn task_memory_guard(
        &self,
        sandbox: &SandboxId,
        harness: Harness,
        home: &Path,
        codex_home: Option<&Path>,
        antigravity_home: Option<&Path>,
    ) -> AppResult<Vec<yulius_contracts::ptyhost::GuardPath>> {
        let root = self.sandbox_dir(sandbox);
        match (harness, codex_home, antigravity_home) {
            (Harness::ClaudeCode, ..) => {
                let config = home.join(".claude");
                let base = crate::memory_guard::sandbox_memory_base(&root);
                crate::memory_guard::claude_guard(&root, &config, &config, &base)
            }
            (Harness::Codex, Some(codex), _) => crate::memory_guard::codex_guard(&root, codex),
            (Harness::Antigravity, _, Some(agy)) => {
                crate::memory_guard::antigravity_guard(agy, &home.join(".gemini"))
            }
            (Harness::Codex | Harness::Antigravity, ..) => Err(AppError::internal(
                "the run's harness home is missing: its memory cannot be guarded",
            )),
        }
    }

    async fn ensure_worktree(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
    ) -> AppResult<PathBuf> {
        let worktree = self
            .sandbox_dir(&sandbox.id)
            .join("worktrees")
            .join(task.id.as_str());
        if worktree.join(".git").exists() {
            // A worktree from before (an older identity, F 2026-10-11) commits as the owner now.
            if let Err(err) = self.set_task_identity(&worktree, task).await {
                tracing::warn!(task = %task.id, error = %err, "refreshing the commit identity failed");
            }
            return Ok(worktree);
        }
        let repo = self.sandbox_repo(&sandbox.id);
        if !repo.join(".git").exists() {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        let branch = task
            .branch
            .clone()
            .unwrap_or_else(|| branch_for(task.number, &task.title));
        if !self.deps.git.valid_branch(&branch) {
            return Err(AppError::Invalid(format!("invalid branch name {branch}")));
        }
        let profile = self.profile_of(sandbox);
        // A chained task starts where its predecessor ended, until that is merged (TSK-012).
        let base = if let Some(branch) = self.chain_base(task).await {
            branch
        } else if let Some(commit) = self.fork_base(task).await? {
            // A fork starts at the source's commit it recorded (SPEC-628 FRK-005).
            commit
        } else if self
            .deps
            .git
            .branch_exists(&repo, &profile.integration_branch)
            .await?
        {
            format!("origin/{}", profile.integration_branch)
        } else {
            format!("origin/{}", sandbox.default_branch)
        };
        if let Some(parent) = worktree.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::internal)?;
        }
        self.deps
            .git
            .add_worktree(&repo, &worktree, &branch, &base)
            .await?;
        let protected = vec![
            profile.integration_branch.clone(),
            profile.production_branch.clone(),
        ];
        self.deps
            .git
            .install_push_guard(&repo, &worktree, &protected)
            .await?;
        self.set_task_identity(&worktree, task).await?;
        let update = TaskFieldUpdate {
            branch: Some(branch),
            worktree_path: Some(worktree.to_string_lossy().into_owned()),
            ..TaskFieldUpdate::default()
        };
        self.deps
            .store
            .update_task(&task.id, &update, self.now())
            .await?;
        Ok(worktree)
    }

    /// Commits in `task`'s worktree are its owner's (SPEC-070): their name and the no-reply address GitHub
    /// links to their account (F 2026-10-11, [`crate::projects::identity`]).
    pub(crate) async fn set_task_identity(
        &self,
        worktree: &Path,
        task: &TaskRecord,
    ) -> AppResult<()> {
        let owner = self.deps.store.user(&task.owner).await?;
        let (name, email) = crate::projects::commit_identity(owner.as_ref());
        self.deps.git.set_identity(worktree, &name, &email).await
    }

    /// Installs the enabled skills into the agent home. Failures are logged; a run never fails
    /// because of them.
    async fn prepare_run_extras(&self, sandbox: &SandboxRecord, home: &Path) {
        match self.skills_for_run(sandbox).await {
            Ok(skills) if !skills.is_empty() => {
                // Under the launch lock (held by the launch): a refresh never meets this
                // install in the same home (SPEC-507 SKR-005).
                let root = self.sandbox_dir(&sandbox.id);
                let skills_dir = crate::skills::refresh::skills_dir_below(&root, home);
                if let Err(err) = self
                    .deps
                    .harness
                    .install_skills(&skills, &root, &skills_dir)
                {
                    tracing::warn!(error = %err, sandbox = %sandbox.id, "skill installation failed");
                }
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = %err, "cannot list skills for the run"),
        }
    }

    pub(crate) fn profile_of(
        &self,
        sandbox: &SandboxRecord,
    ) -> yulius_contracts::api::sandboxes::Profile {
        serde_json::from_str(&sandbox.profile_json)
            .unwrap_or_else(|_| crate::sandboxes::standard_profile())
    }

    pub(super) async fn launch_attempt(
        self: &Arc<Self>,
        task: &TaskRecord,
        start: RunStart,
        requested: Option<AccountId>,
        rest: Option<IdleRest>,
        pause_outcome: &'static str,
        admission: Option<crate::capacity::ContinuationAdmission>,
    ) -> AppResult<bool> {
        let store = &self.deps.store;
        let relaunch = start.clone();
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        // Status → Starting (unless a switch drives this launch).
        if admission.is_none()
            && !matches!(task.status, TaskStatus::Starting | TaskStatus::Switching)
        {
            let next = match task.status {
                TaskStatus::Queued => TaskStatus::Starting,
                TaskStatus::WaitingCapacity => TaskStatus::WaitingCapacity
                    .apply(TaskEvent::CapacityGranted)
                    .unwrap_or(TaskStatus::Starting),
                // A parked task resumes as a resting one continues (SPEC-512 PRK-005).
                s if s.is_resting() || s == TaskStatus::Parked => s
                    .apply(TaskEvent::ContinueRequested)
                    .unwrap_or(TaskStatus::Starting),
                other => return Err(AppError::Conflict(format!("task is {}", other.as_str()))),
            };
            self.prepare_start_claim(&task.id).await?;
            let claimed = store
                .transition_task(&task.id, Some(task.status), next, None, self.now())
                .await?;
            if !claimed {
                // Stop won the claim. This start owns neither the task nor a replacement pause.
                return Ok(false);
            }
            self.publish_task(&task.id).await;
        }
        if matches!(
            start,
            RunStart::Resume { .. } | RunStart::GoalResume { .. } | RunStart::AfterLimit
        ) {
            // Imported sessions get a native transcript and a worktree from the imported state.
            self.prepare_imported_resume(&task.id).await?;
        }
        let worktree = self.ensure_worktree(&sandbox, task).await?;
        // The logs every task appends to merge by union in this repository (SPEC-083 MRG-001).
        self.keep_union_merges(&sandbox.id, &worktree).await;
        // The spec path is given out once, so parallel tasks never pick the same number.
        if let Ok(Some(fresh)) = store.task(&task.id).await {
            self.allocate_spec_path(&fresh, &worktree).await;
        }
        let mut task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        let lineage = store.provider_sessions(&task.id).await?;
        let previous = lineage.last().cloned();
        // A fork's first launch continues a copy of its source's session (SPEC-628 FRK-006).
        let fork = self.fork_to_launch(&task, &worktree).await?;
        let harness = task.harness;
        let placement = if let Some(crate::capacity::ContinuationAdmission::Named {
            account,
            model,
            fallback,
        }) = admission
        {
            // A later model edit is for the next start. This run uses the accepted placement.
            task.model = model;
            Some(Placed {
                account: *account,
                fallback,
            })
        } else {
            // If a subscription disappeared after a switch's preflight, the pool chooses again (SPEC-447).
            let requested = match requested {
                Some(id) if store.account(&id).await?.is_none() => None,
                other => other,
            };
            self.choose_account(&task, requested.as_ref()).await?
        };
        let Some(Placed { account, fallback }) = placement else {
            // No subscription has capacity: the task waits before starting, without a process
            // (ROT-010); the resume loop starts it when capacity returns. A turn a limit cut
            // keeps its `limit` pause, so it still resumes past the limit (SPEC-034).
            let (kind, continuation) = match &start {
                RunStart::GoalFresh { prompt } => (PauseKind::GoalFreshStart, Some(prompt.clone())),
                RunStart::GoalResume { prompt } => {
                    (PauseKind::GoalResumeStart, Some(prompt.clone()))
                }
                RunStart::AfterLimit => (PauseKind::Limit, None),
                RunStart::Fresh { prompt } if harness == Harness::Codex => {
                    (PauseKind::FreshStart, prompt.clone())
                }
                RunStart::Resume { prompt } => (PauseKind::Start, prompt.clone()),
                RunStart::Fresh { .. } => (PauseKind::Start, None),
            };
            self.pause_task(&task.id, kind, None, continuation).await?;
            return Ok(true);
        };
        // The run has its subscription: a pause the task waited in is over. Only here, so that
        // a start the full queue refused, and one that finds no capacity after all (the branch
        // above, where an open pause keeps its kind and its text), leave it as it was
        // (SPEC-483 QST-002).
        self.end_pause(&task.id, pause_outcome).await;
        let credential = self.open_credential(&account)?;
        crate::race::reached(
            crate::race::Point::AccountChosen,
            Path::new(task.id.as_str()),
        );
        let home = self.sandbox_home(&sandbox.id);
        make_home(&self.sandbox_dir(&sandbox.id))?;
        // A Codex run has the account's own Codex home in the sandbox (SPEC-503 CDX-003). Another
        // run of the account may have refreshed its tokens there: that copy is kept, never
        // overwritten with the vault's older one (CDX-006).
        let codex_home = match harness {
            Harness::Codex => Some(self.codex_home(&sandbox.id, &account.id)?),
            Harness::ClaudeCode | Harness::Antigravity => None,
        };
        // The Antigravity CLI Yulius provides, by its absolute path (SPEC-505 AGY-007); under the
        // launch lock only what the install before it left.
        let antigravity_cli = match harness {
            Harness::Antigravity => self.deps.harness.provide_antigravity(false).await?,
            Harness::ClaudeCode | Harness::Codex => None,
        };
        // The Codex CLI Yulius provides, by its absolute path (SPEC-503 CLI-002).
        let codex_cli = match harness {
            // Under the launch lock: only what the install before it left, never a download.
            Harness::Codex => self.deps.harness.provide_codex(false).await?,
            Harness::ClaudeCode | Harness::Antigravity => None,
        };
        let credential = match &codex_home {
            Some(_) => self
                .keep_codex_refresh(&self.sandbox_dir(&sandbox.id), &account.id)
                .await
                .holds
                .unwrap_or(credential),
            None => credential,
        };
        self.prepare_run_extras(&sandbox, &home).await;

        // Goal mode (TSK-010): the request is the session goal, set through `/goal`. A fresh
        // session gets it as its launch prompt (SPEC-117); Claude's goal is per session, so a
        // resumed session gets it typed again once its TUI is ready.
        // Codex takes `/goal` only typed, never as its launch prompt, and records goals its own
        // way: a Codex task gets its whole request, the definition of done included, as its first
        // message, and its turns end it as a prompt-mode task's do (SPEC-503).
        // Antigravity has its own `/goal`, never given by Yulius: an Antigravity task gets its
        // request as its first message, as a Codex task does (SPEC-505).
        let first_message_only = matches!(harness, Harness::Codex | Harness::Antigravity);
        let goal_mode = task.follows_goal();
        // A prompt-mode first message with attached images is typed into the TUI, not given on
        // the command line: Claude Code attaches only a pasted image natively (SPEC-030).
        let mut typed_opening = None;
        // The task's request file could not be written, so a fresh goal session has no goal.
        let mut request_unwritten = false;
        // Decide new / resume.
        // Workflows the old process left unfinished end with it: the session is told to resume
        // them by their run ids, which Claude Code does not do by itself. Past a limit that
        // prompt is the turn the launch opens with, in place of the silent continuation
        // (SPEC-486 WFL-004).
        // They stay known until the new process's transcript is read, so a launch that fails
        // here, or runs again for another subscription, still tells its session. A fresh
        // session has none: a run is resumed in its own session only.
        let unfinished = match (&start, &previous) {
            (
                RunStart::Resume { .. } | RunStart::GoalResume { .. } | RunStart::AfterLimit,
                Some(_),
            ) => self.runtime(&task.id).with_workflows(|workflows| {
                workflows.relaunched();
                workflows.runs()
            }),
            _ => {
                self.runtime(&task.id)
                    .with_workflows(|workflows| *workflows = Default::default());
                Vec::new()
            }
        };
        // A reviewer a usage limit cut off is resumed in the new process, not started again
        // (SPEC-493 RLM-002).
        // Only while its round is still open; a session that starts afresh has none (review
        // round 1 of fix 7).
        let reviewer = match (&start, &previous) {
            (
                RunStart::Resume { .. } | RunStart::GoalResume { .. } | RunStart::AfterLimit,
                Some(_),
            ) => {
                let cut = self.runtime(&task.id).cut_off_reviewer.lock().await.clone();
                match cut {
                    Some(cut) => {
                        self.open_round_reviewer(&task.id, &cut.agent_id, Some(cut.cut_off_at))
                            .await
                    }
                    None => None,
                }
            }
            _ => None,
        };
        if reviewer.is_none() {
            self.forget_cut_off_reviewer(&task.id).await;
        }
        let resumes = |prompt: Option<&str>| {
            let prompt = if unfinished.is_empty() {
                prompt.map(str::to_owned)
            } else {
                Some(crate::dynamic_workflow::resume_prompt(prompt, &unfinished))
            };
            match &reviewer {
                Some(reviewer) => Some(crate::review::reviewer_resume_prompt(
                    prompt.as_deref(),
                    reviewer,
                )),
                None => prompt,
            }
        };
        let told = !unfinished.is_empty() || reviewer.is_some();
        let (mode, launch_kind) = match (&start, &previous, &fork) {
            // Its opening message is the first turn; a goal is set after it, as on a resume, and
            // the transcript is read from the run's start, past the source's copied history.
            (_, None, Some(fork)) => (
                SessionMode::Resume {
                    session_id: fork.source_session_id.clone(),
                    prompt: Some(crate::forks::fork_opening(&task, fork)),
                    continue_interrupted: false,
                    fork: Some(self.fork_session(harness)),
                },
                LaunchKind::Resume,
            ),
            (start, previous, _) => match (start, previous) {
                (RunStart::Resume { .. } | RunStart::GoalResume { .. }, Some(prev)) => (
                    SessionMode::Resume {
                        session_id: prev.session_id.clone(),
                        prompt: resumes(start.prompt()),
                        continue_interrupted: false,
                        fork: None,
                    },
                    LaunchKind::Resume,
                ),
                (RunStart::AfterLimit, Some(prev)) => (
                    SessionMode::Resume {
                        session_id: prev.session_id.clone(),
                        prompt: told.then(|| resumes(None)).flatten(),
                        continue_interrupted: !told,
                        fork: None,
                    },
                    LaunchKind::Resume,
                ),
                // Without a recorded session there is nothing to resume: the task starts afresh.
                (
                    RunStart::Resume { .. } | RunStart::GoalResume { .. } | RunStart::AfterLimit,
                    None,
                )
                | (RunStart::Fresh { .. } | RunStart::GoalFresh { .. }, _) => {
                    let prompt = start.prompt().map(str::to_owned);
                    // In goal mode the task starts with its `/goal` as the launch prompt: the whole
                    // request, each attached file at its place, and the definition of done in one
                    // first message, with nothing typed (SPEC-011, SPEC-117, SPEC-212). Otherwise the
                    // request is the first message: images are pasted into it and other files
                    // mentioned, so Claude Code attaches them natively (SPEC-025, SPEC-030).
                    use yulius_domain::request::{Opening, Request};
                    let prompt = match prompt {
                        Some(prompt) => Some(prompt),
                        None if first_message_only && task.mode == TaskMode::Goal => {
                            match self.write_task_request(&sandbox, &task, &home).await {
                                Ok(goal) => Some(goal.launch),
                                Err(err) => {
                                    tracing::error!(task = %task.id, error = %err, "could not write the task request");
                                    Some(Request::parse(&task.prompt).first_message())
                                }
                            }
                        }
                        // Codex attaches no pasted image: each file is mentioned where it sits.
                        None if first_message_only => {
                            Some(Request::parse(&task.prompt).first_message())
                        }
                        None if goal_mode => {
                            match self.write_task_request(&sandbox, &task, &home).await {
                                Ok(goal) => Some(format!("{GOAL_COMMAND}{}", goal.launch)),
                                Err(err) => {
                                    tracing::error!(task = %task.id, error = %err, "could not write the task request");
                                    request_unwritten = true;
                                    None
                                }
                            }
                        }
                        None => match Request::parse(&task.prompt).opening() {
                            Opening::Prompt(prompt) => Some(prompt),
                            Opening::Typed { pastes } => {
                                typed_opening = Some(pastes);
                                None
                            }
                        },
                    };
                    (
                        SessionMode::New {
                            session_id: Some(self.deps.crypto.new_id()),
                            prompt,
                        },
                        LaunchKind::New,
                    )
                }
            },
        };
        if task.mode == TaskMode::Goal {
            *self
                .runtime(&task.id)
                .prompt_receipts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = PromptReceipts::default();
            let (SessionMode::New { prompt, .. } | SessionMode::Resume { prompt, .. }) = &mode;
            if let Some(prompt) = prompt {
                self.note_issued_prompt(&task.id, prompt.as_bytes());
            }
        }
        let activity = match (harness, &mode) {
            // Opening either conversation without a message starts no turn to settle.
            (
                Harness::Codex | Harness::Antigravity,
                SessionMode::Resume {
                    prompt,
                    continue_interrupted: false,
                    ..
                },
            ) if prompt.as_deref().unwrap_or_default().is_empty() => {
                rest.map_or(Activity::Waiting, |rest| rest.activity)
            }
            (Harness::Codex | Harness::Antigravity, _) => Activity::Working,
            (Harness::ClaudeCode, _) => rest.map_or(Activity::Working, |rest| rest.activity),
        };
        let goal_in_prompt = sets_goal(&mode);
        // A continued turn opens the run like a first message: the goal is set after its Stop.
        let opens_with_turn = goal_mode
            && !goal_in_prompt
            && (typed_opening.is_some()
                || matches!(
                    &mode,
                    SessionMode::New {
                        prompt: Some(_),
                        ..
                    } | SessionMode::Resume {
                        prompt: Some(_),
                        ..
                    } | SessionMode::Resume {
                        continue_interrupted: true,
                        ..
                    }
                ));
        // This session's transcript steps back over a usage limit before the run (SPEC-034),
        // also when the run opens with the prompt that resumes its workflows (SPEC-486); a Codex
        // rollout has no step-back (SPEC-503).
        let stepped_back = match (&start, &mode) {
            (
                RunStart::AfterLimit,
                SessionMode::Resume {
                    session_id,
                    fork: None,
                    ..
                },
            ) if !first_message_only => Some(session_id.clone()),
            _ => None,
        };
        let attempt_id =
            AttemptId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        let run_dir = self.config.data_dir.join("runs").join(attempt_id.as_str());
        std::fs::create_dir_all(&run_dir).map_err(AppError::internal)?;
        if harness == Harness::ClaudeCode
            && let Some(sealed) = &account.credential
        {
            let digest = self.deps.crypto.sha256_hex(&sealed.0);
            write_private_file(&run_dir.join(CLAUDE_CREDENTIAL_DIGEST), digest.as_bytes())
                .map_err(AppError::internal)?;
        }
        // An Antigravity run has a home of its own in its run directory (SPEC-505 AGY-003).
        let antigravity_home = match harness {
            Harness::Antigravity => Some(self.antigravity_home(&sandbox.id, &run_dir)?),
            Harness::ClaudeCode | Harness::Codex => None,
        };
        // The repository's `.mcp.json` servers join the sandbox's first (SPEC-095 PMC-001).
        self.import_project_mcp(&sandbox).await;
        // One read of the servers: what the run gets is what it is recorded to hold (MSC-009).
        let (mcp, personal_mcp) = self
            .mcp_launch(&sandbox, crate::vault::Holder::of_task(&task))
            .await?;
        if let Ok(mut held) = self.runtime(&task.id).personal_mcp_of.lock() {
            *held = personal_mcp.then(|| task.owner.clone());
        }
        // The Antigravity CLI expands no variable in an MCP server's headers, so a server that
        // needs a token is left out of its runs rather than written to a file (SPEC-505): said.
        if harness == Harness::Antigravity
            && let Some(said) = tokened_servers_left_out(&mcp)
        {
            self.runtime(&task.id).hub.notice(said);
        }
        // The run directory says it too: a restarted server has no other record (RST-001). A
        // mark that cannot be written fails the launch: the run would hold a person's servers
        // that a later server knows nothing of.
        if personal_mcp {
            write_private_file(
                &run_dir.join(PERSONAL_MCP_FILE),
                task.owner.as_str().as_bytes(),
            )
            .map_err(AppError::internal)?;
        }
        let plan = self.deps.harness.plan(&HarnessLaunch {
            harness,
            mode,
            worktree: worktree.clone(),
            sandbox_home: home.clone(),
            run_dir: run_dir.clone(),
            // The fallback while no subscription can run the task's own model (SPEC-482).
            model: fallback.map(str::to_owned).or_else(|| task.model.clone()),
            effort: task.effort.clone(),
            contained: self.config.contained,
            mcp,
            credential,
            codex_home: codex_home.as_ref().map(|(path, _)| path.clone()),
            codex_program: codex_cli.as_ref().map(|cli| cli.program.clone()),
            antigravity_home: antigravity_home.clone(),
            antigravity_program: antigravity_cli.as_ref().map(|cli| cli.program.clone()),
        })?;
        if let Some(session_id) = &stepped_back {
            self.step_back_transcript(&task.sandbox_id, &home, &worktree, session_id)
                .await;
        }
        let mut launch = plan.plan;
        // The Codex home's files (its `auth.json` among them) are written through the home's
        // handle, never by a path the agent could have turned into a link (#657).
        let mut codex_leftover = None;
        if let Some((path, dir)) = &codex_home {
            // Written now: a launch that ends before its run is live takes them away again.
            codex_leftover = Some(CodexLeftover::new(self, &task.id, &account.id));
            write_codex_files(dir, path, &mut launch.files)?;
        }
        // The Antigravity home's files are the run's own, in the server's run directory: written
        // now and taken out of the plan (a contained run binds the whole `.gemini`). A launch that
        // ends before its run is live takes the sign-in's token file away again (AGY-008).
        let mut antigravity_token = None;
        if let Some(agy_home) = &antigravity_home {
            let (own, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut launch.files)
                .into_iter()
                .partition(|f| f.path.starts_with(agy_home));
            launch.files = rest;
            for file in own {
                write_private(&file).map_err(AppError::internal)?;
            }
            antigravity_token = Some(AntigravityToken(Some(antigravity_token_path(&run_dir))));
            // What this run was given, beside its home (out of the bound `.gemini`): its end
            // takes back a refresh only of this, and only while the account still holds it.
            if let Some(given) = &account.credential {
                write_private(&crate::ports::session::PlannedFile {
                    path: run_dir.join(ANTIGRAVITY_GIVEN),
                    contents: given.0.clone(),
                    mode: 0o600,
                })
                .map_err(AppError::internal)?;
            }
        }
        let task_token = hex_token(&self.deps.crypto.random_bytes());
        let host_token = self.deps.crypto.random_bytes();
        launch
            .secret_env
            .push(("YULIUS_TASK_TOKEN".into(), task_token.clone()));
        // Project variables first (SPEC-275 VAR-004): Yulius's own environment and the agent
        // secret placeholders after them win over a variable of the same name.
        launch
            .env
            .extend(self.variables_env(&task.sandbox_id).await?);
        launch.env.extend(self.base_env(&sandbox, &home, &worktree));
        // An uncontained Antigravity run reads its `.gemini` from its own home (AGY-003).
        if let (Some(agy_home), false) = (&antigravity_home, self.config.contained) {
            launch
                .env
                .push(("HOME".into(), agy_home.to_string_lossy().into_owned()));
        }
        // Agent secrets: placeholders, the Bash redaction wrapper and the egress proxy
        // (SPEC-002 ADR-20). A vault that cannot be read fails the start, never a run without
        // redaction.
        self.vault_run_setup(
            &sandbox,
            crate::vault::Holder::of_task(&task),
            &run_dir,
            &task_token,
            &mut launch,
        )
        .await?;
        let (join, _lease) = self.isolation_join(&sandbox).await?.unzip();
        if join.is_some() {
            // Root of its sandbox (ADR-22): Claude Code accepts --dangerously-skip-permissions as
            // root only inside a declared sandbox; package tools live in sbin.
            launch.env.retain(|(k, _)| k != "PATH");
            launch.env.extend([
                ("IS_SANDBOX".into(), "1".into()),
                (
                    "PATH".into(),
                    crate::isolation::with_home_tools(
                        &home.to_string_lossy(),
                        crate::isolation::SANDBOX_PATH,
                    ),
                ),
            ]);
        }
        launch.containment = if self.config.contained {
            let sandbox_root = self.sandbox_dir(&sandbox.id);
            // Generated per-run config (Claude settings and MCP config live in the run directory,
            // which stays hidden): bind exactly those files read-only.
            let run_files: Vec<PathBuf> = launch
                .files
                .iter()
                .map(|f| f.path.clone())
                .filter(|p| !p.starts_with(&sandbox_root))
                .collect();
            Containment::Bubblewrap {
                sandbox_root,
                home: home.clone(),
                writable: Vec::new(),
                readonly: std::iter::once(self.config.yulius_exe.clone())
                    .chain(self.config.contained_readonly.iter().cloned())
                    .chain(run_files)
                    // The provided Codex CLI, at its own path (SPEC-503 CLI-002).
                    .chain(codex_cli.iter().map(|cli| cli.dir.clone()))
                    // The provided Antigravity CLI, at its own path (SPEC-505 AGY-007).
                    .chain(antigravity_cli.iter().map(|cli| cli.dir.clone()))
                    .collect(),
                binds: join
                    .iter()
                    .flat_map(|j| j.extra_binds.iter().cloned())
                    // The run's `.gemini` over the sandbox home's: the agent's shell keeps the
                    // sandbox's HOME (SPEC-505 AGY-003).
                    .chain(
                        antigravity_home
                            .iter()
                            .map(|agy| antigravity_bind(agy, &home)),
                    )
                    .collect(),
                identity: None,
                join: join.map(Box::new),
                guard: self.task_memory_guard(
                    &sandbox.id,
                    harness,
                    &home,
                    codex_home.as_ref().map(|(path, _)| path.as_path()),
                    antigravity_home.as_deref(),
                )?,
            }
        } else {
            Containment::None
        };
        // Its hooks say why a write of its read-only paths fails (SPEC-652 MEM-006).
        if let Containment::Bubblewrap { guard, .. } = &launch.containment
            && let Some(var) = crate::memory_guard::readonly_paths_env(guard)
        {
            launch.env.push(var);
        }
        // `gh` in the agent's shell acts as the task's owner, like its Git (F 2026-10-11).
        self.add_agent_gh(&mut launch)?;
        let earlier = store.attempts(&task.id).await?;
        let seq = u32::try_from(earlier.len() + 1).unwrap_or(u32::MAX);
        let own = task.model.as_deref().unwrap_or(DEFAULT_CLAUDE_MODEL);
        let model = fallback.unwrap_or(own);
        // A Codex task runs OpenAI's models and an Antigravity task Gemini: its own model, else
        // the default (SPEC-511 CXD-002, SPEC-505 AGY-002), which its launch plan passes too.
        let model = match harness {
            Harness::Codex => task
                .model
                .as_deref()
                .unwrap_or(yulius_domain::harness::DEFAULT_CODEX_MODEL),
            Harness::Antigravity => task
                .model
                .as_deref()
                .unwrap_or(yulius_domain::harness::DEFAULT_ANTIGRAVITY_MODEL),
            Harness::ClaudeCode => model,
        };
        // Said once per change of the model (SPEC-482 FBK-008): at the launch that falls back,
        // and at the first one that runs the task's own model again. Only Claude models fall
        // back.
        let before = earlier.last().and_then(|run| run.model.as_deref());
        if let Some(said) =
            model_change_notice(own, before, model).filter(|_| harness == Harness::ClaudeCode)
        {
            self.runtime(&task.id).hub.notice(said);
        }
        // Past the limit of a model the session's person switched it to (SPEC-493 MDL-002): said
        // with the model this run is launched with, unless that is of the refused family again
        // (its usage changed since the decision), which the line above says. Every launch takes
        // it, so a start that is not the one past that limit leaves nothing for a later one.
        let kept = self
            .runtime(&task.id)
            .switched_limit
            .lock()
            .ok()
            .and_then(|mut kept| kept.take());
        let ran = kept
            .filter(|(id, _)| {
                matches!(start, RunStart::AfterLimit)
                    && earlier.last().is_some_and(|run| &run.id == id)
            })
            .map(|(_, family)| family);
        if let Some(ran) = ran.filter(|ran| ModelFamily::of(Some(model)) != *ran) {
            self.runtime(&task.id)
                .hub
                .notice(switched_back_notice(ran, model));
        }
        let attempt = AttemptRecord {
            id: attempt_id.clone(),
            task_id: task.id.clone(),
            seq,
            harness,
            account_id: Some(account.id.clone()),
            provider_session_id: plan.session_id.clone(),
            launch_kind,
            model: Some(model.to_owned()),
            state: AttemptState::Spawning,
            run_dir: run_dir.to_string_lossy().into_owned(),
            token_hash: self.deps.crypto.sha256_hex(task_token.as_bytes()),
            host_token: self.deps.crypto.seal(&host_token)?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: self.now(),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        };
        store.insert_attempt(&attempt).await?;
        crate::race::reached(crate::race::Point::RunRecorded, Path::new(task.id.as_str()));
        // A pin ends once the task runs on another subscription, however it was placed there
        // (SPEC-629 RBL-005); a person's move pins the new one after this.
        if matches!(store.task_pin(&task.id).await, Ok(Some(pin)) if pin != account.id) {
            self.pin_task(&task.id, None).await;
        }
        // Removed since it was chosen (SPEC-447 MOV-003): the removal looked for runs on it before
        // this one was recorded, so this launch never starts on it and chooses again.
        if store.account(&account.id).await?.is_none() {
            let update = AttemptUpdate {
                ended_at: Some(self.now()),
                exit_reason: Some("account_removed".into()),
                ..AttemptUpdate::default()
            };
            store
                .transition_attempt(&attempt_id, AttemptState::Exited, &update)
                .await?;
            return Box::pin(self.launch_attempt(&task, relaunch, None, rest, pause_outcome, None))
                .await;
        }
        // Before the spawn, so the first prompt's hook always finds it (SPEC-068).
        if typed_opening.is_some()
            && let Err(err) = write_private_file(&run_dir.join(OPENING_PENDING_FILE), b"")
        {
            tracing::warn!(task = %task.id, error = %err, "could not record the pending first message");
        }
        // Before the spawn too: a restart from now on watches the goal again (SPEC-146 GWR-001).
        if goal_in_prompt && let Err(err) = write_private_file(&run_dir.join(LAUNCH_GOAL_FILE), b"")
        {
            tracing::warn!(task = %task.id, error = %err, "could not record the pending launch goal");
        }
        // And for a goal Yulius types once the session reads input (SPEC-483 RST-002): the
        // mark says whether a turn comes first, which a restarted server has to wait out.
        if goal_mode && !goal_in_prompt && !request_unwritten {
            let opens = if opens_with_turn {
                TYPED_GOAL_OPENS
            } else {
                ""
            };
            if let Err(err) = write_private_file(&run_dir.join(TYPED_GOAL_FILE), opens.as_bytes()) {
                tracing::warn!(task = %task.id, error = %err, "could not record the goal still to be typed");
            }
        }
        let runtime = self.runtime(&task.id);
        self.initialize_run_goal(
            &task,
            &attempt,
            rest.is_some_and(|rest| rest.goal_unfinished),
        )
        .await;
        // Before the spawn, so the opening turn's Stop hook can never come first.
        runtime
            .opening_turn
            .store(opens_with_turn, Ordering::SeqCst);
        // Before the spawn too: a person's keys never reach a first message Yulius types.
        runtime
            .typing_opening
            .store(typed_opening.is_some(), Ordering::SeqCst);
        runtime.typing_opening_said.store(false, Ordering::SeqCst);
        // So is the new session's SessionStart: its hook can be answered before the run is
        // attached and watched, and must not be forgotten then (#243).
        runtime.session_started.store(false, Ordering::SeqCst);
        runtime.spawned_ms.store(self.now().0, Ordering::SeqCst);
        // A Stop that moved or waited for capacity never returned a block to its old
        // process. Charge its continuation here, after admission and before the new
        // process can submit it. A queued start or a capacity poll spends no attempt.
        if task.mode == TaskMode::Goal
            && matches!(
                start,
                RunStart::GoalFresh { .. } | RunStart::GoalResume { .. }
            )
        {
            store
                .consume_stop_attempt(&task.id, crate::hooks::MAX_GATE_BLOCKS, self.now())
                .await?;
        }
        let size = runtime.hub.size();
        let spawned = match self
            .deps
            .session_host
            .spawn(
                &run_dir,
                attempt_id.as_str(),
                task.id.as_str(),
                &launch,
                &host_token,
                size,
            )
            .await
        {
            Ok(s) => s,
            Err(err) => {
                let update = AttemptUpdate {
                    ended_at: Some(self.now()),
                    exit_reason: Some("spawn_failed".into()),
                    ..AttemptUpdate::default()
                };
                store
                    .transition_attempt(&attempt_id, AttemptState::Exited, &update)
                    .await?;
                return Err(err);
            }
        };
        store
            .transition_attempt(
                &attempt_id,
                AttemptState::Live,
                &AttemptUpdate {
                    ptyhost_pid: Some(spawned.host_pid),
                    child_pid: Some(spawned.child_pid),
                    ..AttemptUpdate::default()
                },
            )
            .await?;
        // The run is live: its end takes the Codex home's credentials back (CDX-006).
        if let Some(leftover) = &mut codex_leftover {
            leftover.disarm();
        }
        if let Some(token) = &mut antigravity_token {
            token.0 = None;
        }
        let current = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        // Control passed during the launch, after its fingerprint file was computed: the file a
        // hand-over found nothing of yet becomes the new holder's (SPEC-248 FUP-002).
        if crate::vault::Holder::of_task(&current) != crate::vault::Holder::of_task(&task)
            && let Err(err) = self.rewrite_run_fingerprints(&task.id).await
        {
            tracing::warn!(task = %task.id, error = %err, "run's fingerprint file not rewritten");
        }
        let event = if current.status == TaskStatus::Switching {
            TaskEvent::SwitchCompleted
        } else {
            TaskEvent::Spawned
        };
        if let Ok(next) = current.status.apply(event) {
            store
                .transition_task(
                    &task.id,
                    Some(current.status),
                    next,
                    Some(activity),
                    self.now(),
                )
                .await?;
        }
        if let Some(session_id) = &plan.session_id {
            self.record_lineage(&task, harness, session_id, "native")
                .await?;
        }
        // Typing from now on, once viewers can write to this run, is a person's draft in it
        // (SPEC-068): its prompt box starts empty (SPEC-468 CCF-010).
        let typed_before = runtime.typed_at_ms.load(Ordering::SeqCst);
        self.consume_viewer_submits(&task.id, u64::MAX);
        runtime.run_starts(self.now().0).await;
        self.attach_run(&task.id, &attempt, &host_token).await?;
        runtime.hub.state_changed();
        // The move starts a working timer. Probe the idle replacement before publishing it
        // so that timer ends when the new session has no background work (SPEC-611 CQS-001).
        if harness == Harness::Codex
            && current.status == TaskStatus::Switching
            && activity.is_idle()
        {
            self.probe_idle_shells(&task.id).await;
        }
        self.publish_task(&task.id).await;
        // Control passed during the launch, after this run took its owner's personal MCP servers
        // and before a hand-over could see it live: it resumes for the new owner (SPEC-332). The
        // owner is read after the status changed, so no hand-over falls between the two checks.
        let held = runtime
            .personal_mcp_of
            .lock()
            .ok()
            .and_then(|held| held.clone());
        let owner_now = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        if held
            .is_some_and(|owner| crate::vault::Holder::of_task(&owner_now).person() != Some(&owner))
        {
            let app = Arc::clone(self);
            let (task_id, account) = (task.id.clone(), Some(account.id.clone()));
            tokio::spawn(async move { app.restart_for_new_owner(&task_id, account).await });
        }
        if goal_in_prompt {
            // Nothing to type: the goal is watched until Claude Code records it.
            self.spawn_goal_watch(&task.id, &attempt_id, &run_dir, PendingGoal::Launch);
        } else if request_unwritten {
            self.goal_not_taken(&task.id, "Yulius could not write the task's request file")
                .await;
        } else if goal_mode {
            match self.write_task_request(&sandbox, &task, &home).await {
                Ok(goal) => {
                    self.spawn_goal(&task.id, (&attempt_id, &run_dir), &goal.typed, typed_before);
                }
                Err(err) => {
                    tracing::error!(task = %task.id, error = %err, "could not write the task request");
                    self.goal_not_taken(&task.id, "Yulius could not write the task's request file")
                        .await;
                    // Said: no later server has to say it again.
                    PendingGoal::Typed.settled(&run_dir);
                }
            }
        } else if let Some(opening) = typed_opening {
            let app = Arc::clone(self);
            let (task_id, attempt_id) = (task.id.clone(), attempt_id.clone());
            tokio::spawn(async move {
                let mut typed_before = 0;
                let ready = app
                    .wait_input_ready(
                        &task_id,
                        &attempt_id,
                        tokio::time::Instant::now(),
                        false,
                        &mut typed_before,
                    )
                    .await;
                if matches!(ready, InputReady::Ready) {
                    app.type_opening(&task_id, &attempt_id, opening).await;
                }
                // A run that is gone leaves the flag to the task's next start, which sets it
                // for its own first message (this one may already be that run's).
                if app.attempt_live(&task_id, &attempt_id).await {
                    app.runtime(&task_id)
                        .typing_opening
                        .store(false, Ordering::SeqCst);
                }
                // The typing is over before the task says it was given up: a message sent on
                // seeing the flag is answered with why nothing is typed, not with "Yulius is
                // still typing".
                match ready {
                    InputReady::Ready | InputReady::Gone => {}
                    InputReady::TrustUnanswered => {
                        app.trust_unanswered(&task_id, ", then type the task's first message.")
                            .await;
                    }
                    InputReady::ScreenWithheld => app.screen_withheld(&task_id).await,
                }
            });
        }
        Ok(true)
    }

    pub(crate) fn open_credential(&self, account: &AccountRecord) -> AppResult<Secret> {
        // Signed out since it was chosen: the start needs a connected account (SPEC-246 CLA-005).
        let sealed = account.credential.as_ref().ok_or_else(|| {
            AppError::AccountRequired(format!(
                "Connect {} account first: {} is not signed in. An instance administrator \
                 signs it in again in Subscriptions ({}).",
                crate::account_gate::a_kind(account.provider),
                account.label,
                crate::account_gate::CONNECT_PATH
            ))
        })?;
        String::from_utf8(self.deps.crypto.open(sealed)?)
            .map(Secret)
            .map_err(AppError::internal)
    }

    fn base_env(
        &self,
        sandbox: &SandboxRecord,
        home: &Path,
        worktree: &Path,
    ) -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        let home_text = home.to_string_lossy().into_owned();
        let mut env = vec![
            ("HOME".into(), home_text.clone()),
            (
                "PATH".into(),
                crate::isolation::with_home_tools(&home_text, &path),
            ),
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("YULIUS_PORT_BASE".into(), sandbox.port_base.to_string()),
            ("YULIUS_PORT_COUNT".into(), sandbox.port_count.to_string()),
            ("PORT".into(), sandbox.port_base.to_string()),
            ("YULIUS_SANDBOX".into(), sandbox.slug.clone()),
            // The base of `yulius security-scan`'s change scan (SPEC-195).
            (
                "YULIUS_INTEGRATION_BRANCH".into(),
                self.profile_of(sandbox).integration_branch,
            ),
        ];
        // A project whose Vidoc scans a person switched off, or every project while Vidoc is off
        // for the instance: the scan says "disabled" (SPEC-338, SPEC-356).
        if !self.vidoc_scans_on(sandbox) {
            env.push(("YULIUS_VIDOC".into(), "off".into()));
        }
        // Where its switch is, for a scan whose key fails to point to (SPEC-359 VSB-006).
        env.extend(self.vidoc_switch_env(sandbox));
        // Parallel tasks share the sandbox: each plans for its budget (a third of the sandbox by
        // default), in YULIUS_CPUS and the repository's own variables (SPEC-082 RES-005). It
        // replaces the CARGO_BUILD_JOBS Yulius set for every repository (OOM kills of
        // 2026-10-12): a repository maps it in its yulius.toml, and one without the table gets
        // the Rust defaults (SPEC-268 PID-007).
        env.extend(self.agent_resource_env(worktree));
        env.extend(git_credential_env(&self.config.yulius_exe));
        env
    }

    pub(crate) async fn record_lineage(
        &self,
        task: &TaskRecord,
        harness: Harness,
        session_id: &str,
        origin: &str,
    ) -> AppResult<()> {
        let now = self.now();
        self.deps
            .store
            .upsert_provider_session(&ProviderSessionRecord {
                id: self.deps.crypto.new_id(),
                sandbox_id: task.sandbox_id.clone(),
                task_id: task.id.clone(),
                harness,
                session_id: session_id.to_owned(),
                transcript_ref: None,
                origin: origin.to_owned(),
                first_seen: now,
                last_seen: now,
            })
            .await
    }

    /// The native transcript of `session_id`, which the harness writes below the sandbox home.
    pub(super) fn transcript_path(
        &self,
        sandbox: &SandboxRecord,
        worktree: &Path,
        harness: Harness,
        session_id: &str,
    ) -> PathBuf {
        match harness {
            Harness::ClaudeCode => self.deps.harness.claude_transcript(
                &self.sandbox_home(&sandbox.id),
                worktree,
                session_id,
            ),
            Harness::Codex => self.codex_rollout(&sandbox.id, session_id),
            Harness::Antigravity => self.antigravity_transcript(&sandbox.id, session_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::branch_for;

    /// #661: a `home` link fails the start, dangling (nothing is made where it points) or to a
    /// host folder (which is not taken as the home); a missing home is made, an existing one
    /// kept (SPEC-266 STB-004).
    #[test]
    fn the_home_is_made_without_following_a_link() -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let target = outside.path().join("made/by/the/link");
        std::os::unix::fs::symlink(&target, sandbox.path().join("home"))?;
        assert!(super::make_home(sandbox.path()).is_err());
        assert!(
            !outside.path().join("made").exists(),
            "nothing made outside"
        );
        std::fs::remove_file(sandbox.path().join("home"))?;
        std::os::unix::fs::symlink(outside.path(), sandbox.path().join("home"))?;
        let err = super::make_home(sandbox.path())
            .err()
            .ok_or("a home linked to a host folder is refused")?;
        assert!(err.to_string().contains("link"), "{err}");
        std::fs::remove_file(sandbox.path().join("home"))?;
        super::make_home(sandbox.path())?;
        std::fs::write(sandbox.path().join("home/kept"), "")?;
        super::make_home(sandbox.path())?;
        assert!(
            sandbox.path().join("home/kept").exists(),
            "an existing home is kept"
        );
        Ok(())
    }

    #[test]
    fn branch_names_are_safe_and_bounded() {
        assert_eq!(
            branch_for(3, "Make me an online store!"),
            "feat/3-make-me-an-online-store"
        );
        assert_eq!(branch_for(4, "  ***  "), "feat/4-task");
        assert!(branch_for(5, &"x".repeat(200)).len() <= 50);
        assert_eq!(branch_for(6, "Ünïcödé ąę"), "feat/6-n-c-d");
        // SPEC-069 BPL-006: the branch policy wants a letter after the number.
        assert_eq!(branch_for(7, "2026 / 42"), "feat/7-task-2026-42");
        let policy = yulius_domain::branch_policy::BranchPolicy::standard();
        for title in [
            "Make me an online store!",
            "  ***  ",
            "2026",
            &"x".repeat(200),
            "Ünïcödé ąę",
        ] {
            let branch = branch_for(9, title);
            assert_eq!(policy.branch_type(&branch), Some("feat"), "{branch}");
        }
    }
}
