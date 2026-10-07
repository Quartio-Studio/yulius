//! Session supervisor: owns the lifecycle of every agent run (SPEC-002 §6.1–§6.3, §7.1).
//!
//! Invariants:
//! - at most one live attempt per task (store index + CAS transitions);
//! - capacity is released only after the run's exit was observed or the run was declared lost;
//! - every attempt records exactly one terminal outcome;
//! - a record that says "running" is never respawned automatically after a restart.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId};
use yulius_domain::rotation::{self, AccountView, Decision, RotationInput, Trigger};
use yulius_domain::task::{
    Activity, AttemptState, FailReason, STOP_INTERRUPT_GRACE_MS, STOP_KILL_WAIT_MS,
    STOP_TERM_GRACE_MS, StopReason, TaskEvent, TaskStatus,
};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::AccountUsage;

use crate::auth::hex_token;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::Hub;
use crate::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, MetricsRecord, ProviderSessionRecord,
    SandboxRecord, Secret, SwitchDecisionRecord, TaskMode, TaskRecord,
};
use crate::ports::harness::{HarnessLaunch, Observation, SessionMode, UsageSnapshot};
use crate::ports::session::{Containment, RunEvent, RunExit, RunInspection, RunWriter};
use crate::ports::store::{AttemptUpdate, TaskFieldUpdate};
use crate::usage_json;

/// Boxed `Send` future (breaks recursive async cycles between start, pump and finalize).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Default PTY size before the first viewer reports its size.
pub const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// Transcript poll interval.
pub const WATCH_INTERVAL_MS: u64 = 1_000;
/// Reattach attempts after an unexpected host disconnect.
pub const REATTACH_ATTEMPTS: u32 = 3;

/// Why Yulius is ending a run.
enum EndIntent {
    /// User or system stop.
    Cancel,
    /// Account/provider switch; the sender is told when the old run has exited.
    Switch(oneshot::Sender<()>),
}

struct ActiveRun {
    attempt_id: AttemptId,
    writer: Box<dyn RunWriter>,
}

/// Runtime state of one task with a hub that outlives individual runs.
pub struct TaskRuntime {
    /// Terminal fan-out.
    pub hub: Hub,
    run: tokio::sync::Mutex<Option<ActiveRun>>,
    intent: Mutex<Option<EndIntent>>,
    usage: Arc<Mutex<UsageSnapshot>>,
    watcher_alive: Arc<AtomicBool>,
    /// Last keyboard input from a viewer (ms).
    pub last_input_ms: AtomicI64,
    /// Proactive switch requested at the next turn boundary.
    pub proactive_switch: AtomicBool,
    /// Last seen limit reset time (ms, 0 = unknown).
    pub limit_resets_at: AtomicI64,
}

impl TaskRuntime {
    fn new() -> Self {
        Self {
            hub: Hub::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            run: tokio::sync::Mutex::new(None),
            intent: Mutex::new(None),
            usage: Arc::new(Mutex::new(UsageSnapshot::default())),
            watcher_alive: Arc::new(AtomicBool::new(false)),
            last_input_ms: AtomicI64::new(0),
            proactive_switch: AtomicBool::new(false),
            limit_resets_at: AtomicI64::new(0),
        }
    }
}

/// How the next run starts.
#[derive(Debug, Clone)]
pub enum RunStart {
    /// New session with the task prompt (or an explicit prompt).
    Fresh {
        /// Prompt override.
        prompt: Option<String>,
    },
    /// Resume the latest provider session of the task.
    Resume {
        /// Continuation prompt.
        prompt: Option<String>,
    },
}

/// Supervisor-owned per-process state.
#[derive(Default)]
pub struct SupervisorState {
    runtimes: Mutex<HashMap<TaskId, Arc<TaskRuntime>>>,
    launch: tokio::sync::Mutex<()>,
}

/// File in a run directory holding the run's final screen (`<cols> <rows>\n` + redraw bytes).
pub(crate) const FINAL_SCREEN_FILE: &str = "final-screen";

/// Writes `bytes` to `path` readable by the owner only.
pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

/// Reads a saved final screen: (cols, rows, redraw bytes).
pub(crate) fn read_final_screen(run_dir: &Path) -> Option<(u16, u16, Vec<u8>)> {
    let bytes = std::fs::read(run_dir.join(FINAL_SCREEN_FILE)).ok()?;
    let newline = bytes.iter().position(|b| *b == b'\n')?;
    let header = std::str::from_utf8(&bytes[..newline]).ok()?;
    let (cols, rows) = header.split_once(' ')?;
    Some((
        cols.parse().ok()?,
        rows.parse().ok()?,
        bytes[newline + 1..].to_vec(),
    ))
}

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

/// Quiet time after the TUI's first output before the goal is typed.
const GOAL_QUIET: Duration = Duration::from_millis(1500);
/// Upper bound for waiting until the TUI is ready for the goal.
const GOAL_READY_TIMEOUT: Duration = Duration::from_secs(45);
/// Longest prompt used as the goal itself (Claude Code caps goal conditions at 500 characters).
const GOAL_INLINE_MAX: usize = 450;
/// The goal of a task whose prompt is too long to be the goal: the prompt is the first message.
const GOAL_FOR_LONG_PROMPTS: &str =
    "Everything requested in the first message of this session is done and verified.";

/// The prompt as a one-line goal (control characters and runs of whitespace become single
/// spaces), or `None` when it is too long to be the goal.
pub(crate) fn goal_text(prompt: &str) -> Option<String> {
    let line = prompt
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (line.chars().count() <= GOAL_INLINE_MAX).then_some(line)
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
    } else {
        format!("feat/{number}-{slug}")
    }
}

impl App {
    /// Runtime for a task, created on demand.
    pub fn runtime(&self, task: &TaskId) -> Arc<TaskRuntime> {
        let Ok(mut map) = self.supervisor.runtimes.lock() else {
            return Arc::new(TaskRuntime::new());
        };
        map.entry(task.clone())
            .or_insert_with(|| Arc::new(TaskRuntime::new()))
            .clone()
    }

    /// Drops a deleted task's runtime state.
    pub(crate) fn forget_runtime(&self, task: &TaskId) {
        if let Ok(mut map) = self.supervisor.runtimes.lock() {
            map.remove(task);
        }
    }

    fn existing_runtime(&self, task: &TaskId) -> Option<Arc<TaskRuntime>> {
        self.supervisor
            .runtimes
            .lock()
            .ok()
            .and_then(|m| m.get(task).cloned())
    }

    pub(crate) fn sandbox_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.config
            .data_dir
            .join("sandboxes")
            .join(sandbox.as_str())
    }

    pub(crate) fn sandbox_repo(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("repo")
    }

    pub(crate) fn sandbox_home(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("home")
    }

    /// Account views for the rotation policy.
    pub(crate) async fn account_views(&self) -> AppResult<Vec<AccountView>> {
        let accounts = self.deps.store.accounts().await?;
        let live = self.deps.store.live_attempts().await?;
        Ok(accounts
            .iter()
            .filter(|a| a.credential.is_some())
            .map(|a| AccountView {
                id: a.id.clone(),
                provider: a.provider,
                enabled: a.enabled,
                healthy: matches!(a.status, AccountStatus::Ok | AccountStatus::Limited),
                priority: a.priority,
                usage: a
                    .usage_json
                    .as_deref()
                    .map(usage_json::from_json)
                    .unwrap_or_default(),
                manual_resets_available: None,
                in_use: u32::try_from(
                    live.iter()
                        .filter(|r| r.account_id.as_ref() == Some(&a.id))
                        .count(),
                )
                .unwrap_or(u32::MAX),
            })
            .collect())
    }

    async fn choose_account(
        &self,
        provider: Provider,
        requested: Option<&AccountId>,
    ) -> AppResult<AccountRecord> {
        let views = self.account_views().await?;
        if !views.iter().any(|v| v.provider == provider) {
            return Err(AppError::Conflict(
                "no Claude account is connected; an instance administrator can add one".into(),
            ));
        }
        let trigger = match requested {
            Some(id) => Trigger::Manual {
                account: Some(id.clone()),
            },
            None => Trigger::Start,
        };
        let input = RotationInput {
            current: None,
            task_provider: provider,
            accounts: views,
            now: self.now(),
            threshold: rotation::DEFAULT_THRESHOLD,
        };
        match rotation::decide(&input, trigger) {
            (Decision::Switch { account, .. }, _) => self
                .deps
                .store
                .account(&account)
                .await?
                .ok_or(AppError::NotFound),
            (_, reason) => Err(AppError::Capacity(format!(
                "no account has capacity right now ({reason:?})"
            ))),
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
        let base = if self
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
            .install_push_guard(&worktree, &protected)
            .await?;
        let creator = self.deps.store.user(&task.created_by).await?;
        let login = creator
            .as_ref()
            .and_then(|u| u.github_login.clone())
            .unwrap_or_else(|| "yulius".into());
        let name = creator.map_or_else(|| "Yulius".to_owned(), |u| u.display_name);
        self.deps
            .git
            .set_identity(
                &worktree,
                &name,
                &format!("{login}@users.noreply.github.com"),
            )
            .await?;
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

    /// Installs the enabled skills into the agent home and, for Open Mercato skills, writes a
    /// compatible `.ai/agentic.config.json` into the worktree (git-excluded so it never counts as
    /// a change). Failures are logged; a run never fails because of them.
    async fn prepare_run_extras(&self, sandbox: &SandboxRecord, worktree: &Path, home: &Path) {
        match self.skills_for_run(sandbox).await {
            Ok(skills) if !skills.is_empty() => {
                let claude_dir = home.join(".claude").join("skills");
                if let Err(err) = self.deps.harness.install_skills(&skills, &claude_dir) {
                    tracing::warn!(error = %err, sandbox = %sandbox.id, "skill installation failed");
                }
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = %err, "cannot list skills for the run"),
        }
        match self.om_config_for(sandbox).await {
            Ok(Some(config)) => {
                let rel = ".ai/agentic.config.json";
                let target = worktree.join(rel);
                let exclude = self.sandbox_repo(&sandbox.id).join(".git/info/exclude");
                let written = (|| -> std::io::Result<()> {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if !target.exists() {
                        std::fs::write(&target, config.as_bytes())?;
                    }
                    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
                    if !current.lines().any(|l| l.trim() == format!("/{rel}")) {
                        if let Some(parent) = exclude.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        let mut text = current;
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        text.push_str(&format!("/{rel}\n"));
                        std::fs::write(&exclude, text)?;
                    }
                    Ok(())
                })();
                if let Err(err) = written {
                    tracing::warn!(error = %err, "cannot write the Open Mercato agent config");
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(error = %err, "cannot render the Open Mercato agent config"),
        }
    }

    pub(crate) fn profile_of(
        &self,
        sandbox: &SandboxRecord,
    ) -> yulius_contracts::api::sandboxes::Profile {
        serde_json::from_str(&sandbox.profile_json)
            .unwrap_or_else(|_| crate::sandboxes::standard_profile())
    }

    /// Starts (or queues) a run for a task currently in `Queued`, `Starting` or a resting state.
    ///
    /// # Errors
    /// Precondition failures (no repository, no account), spawn failures.
    pub fn start_run<'a>(
        self: &'a Arc<Self>,
        task_id: &'a TaskId,
        start: RunStart,
        account: Option<AccountId>,
    ) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async move {
            let guard = self.supervisor.launch.lock().await;
            let task = self
                .deps
                .store
                .task(task_id)
                .await?
                .ok_or(AppError::NotFound)?;
            let live =
                u32::try_from(self.deps.store.live_attempts().await?.len()).unwrap_or(u32::MAX);
            if live >= self.config.max_live_sessions && task.status != TaskStatus::Switching {
                let queued = self
                    .deps
                    .store
                    .tasks_with_status(&[TaskStatus::Queued])
                    .await?;
                if u32::try_from(queued.len()).unwrap_or(u32::MAX) >= self.config.queue_max
                    && task.status != TaskStatus::Queued
                {
                    return Err(AppError::Capacity(
                        "the session queue is full; try again later".into(),
                    ));
                }
                if task.status != TaskStatus::Queued {
                    let next = task
                        .status
                        .apply(TaskEvent::ContinueQueued)
                        .unwrap_or(TaskStatus::Queued);
                    self.deps
                        .store
                        .transition_task(&task.id, Some(task.status), next, None, self.now())
                        .await?;
                    self.publish_task(&task.id).await;
                }
                return Ok(());
            }
            let result = self.launch_attempt(&task, start, account).await;
            drop(guard);
            if let Err(err) = &result {
                let reason = match err {
                    AppError::Conflict(_) | AppError::Capacity(_) | AppError::Invalid(_) => {
                        FailReason::Precondition
                    }
                    _ => FailReason::SpawnError,
                };
                if let Ok(Some(current)) = self.deps.store.task(task_id).await
                    && matches!(current.status, TaskStatus::Starting | TaskStatus::Queued)
                {
                    let _ = self
                        .deps
                        .store
                        .transition_task(
                            task_id,
                            Some(current.status),
                            TaskStatus::Failed(reason),
                            None,
                            self.now(),
                        )
                        .await;
                }
                self.runtime(task_id)
                    .hub
                    .notice(format!("Could not start: {}", err.public_message()));
                self.publish_task(task_id).await;
            }
            result
        })
    }

    async fn launch_attempt(
        self: &Arc<Self>,
        task: &TaskRecord,
        start: RunStart,
        requested: Option<AccountId>,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        // Status → Starting (unless a switch drives this launch).
        if task.status != TaskStatus::Switching && task.status != TaskStatus::Starting {
            let next = match task.status {
                TaskStatus::Queued => TaskStatus::Starting,
                s if s.is_resting() => s
                    .apply(TaskEvent::ContinueRequested)
                    .unwrap_or(TaskStatus::Starting),
                other => return Err(AppError::Conflict(format!("task is {}", other.as_str()))),
            };
            if !store
                .transition_task(&task.id, Some(task.status), next, None, self.now())
                .await?
            {
                return Err(AppError::Conflict("task state changed concurrently".into()));
            }
            self.publish_task(&task.id).await;
        }
        if matches!(start, RunStart::Resume { .. }) {
            // Imported sessions get a native transcript and a worktree from the imported state.
            self.prepare_imported_resume(&task.id).await?;
        }
        let worktree = self.ensure_worktree(&sandbox, task).await?;
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        let lineage = store.provider_sessions(&task.id).await?;
        let previous = lineage.last().cloned();
        let harness = task.harness;
        let account = self
            .choose_account(harness.provider(), requested.as_ref())
            .await?;
        let credential = self.open_credential(&account)?;
        let home = self.sandbox_home(&sandbox.id);
        std::fs::create_dir_all(&home).map_err(AppError::internal)?;
        self.prepare_run_extras(&sandbox, &worktree, &home).await;

        // Goal mode (TSK-010): the prompt is set as the session goal through `/goal` once the
        // TUI is ready; Claude's goal is per session, so a resumed session gets it again.
        let goal_mode = task.mode == TaskMode::Goal;
        // Decide new / resume.
        let (mode, launch_kind) = match (&start, &previous) {
            (RunStart::Resume { prompt }, Some(prev)) => (
                SessionMode::Resume {
                    session_id: prev.session_id.clone(),
                    prompt: prompt.clone(),
                },
                LaunchKind::Resume,
            ),
            (RunStart::Resume { prompt }, None) | (RunStart::Fresh { prompt }, _) => {
                // In goal mode a short task prompt arrives as the goal itself; a long one is the
                // first message and the goal refers to it (Claude caps goal conditions).
                let prompt = prompt.clone().or_else(|| {
                    (!goal_mode || goal_text(&task.prompt).is_none()).then(|| task.prompt.clone())
                });
                (
                    SessionMode::New {
                        session_id: Some(self.deps.crypto.new_id()),
                        prompt,
                    },
                    LaunchKind::New,
                )
            }
        };
        let attempt_id =
            AttemptId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        let run_dir = self.config.data_dir.join("runs").join(attempt_id.as_str());
        std::fs::create_dir_all(&run_dir).map_err(AppError::internal)?;
        let plan = self.deps.harness.plan(&HarnessLaunch {
            harness,
            mode,
            worktree: worktree.clone(),
            sandbox_home: home.clone(),
            run_dir: run_dir.clone(),
            model: task.model.clone(),
            effort: task.effort.clone(),
            contained: self.config.contained,
            mcp: self.mcp_attachments(&sandbox).await?,
            credential,
        })?;
        let mut launch = plan.plan;
        let task_token = hex_token(&self.deps.crypto.random_bytes());
        let host_token = self.deps.crypto.random_bytes();
        launch
            .secret_env
            .push(("YULIUS_TASK_TOKEN".into(), task_token.clone()));
        launch.env.extend(self.base_env(&sandbox, &home));
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
                    .collect(),
                binds: Vec::new(),
                identity: None,
            }
        } else {
            Containment::None
        };
        let seq = u32::try_from(store.attempts(&task.id).await?.len() + 1).unwrap_or(u32::MAX);
        let attempt = AttemptRecord {
            id: attempt_id.clone(),
            task_id: task.id.clone(),
            seq,
            harness,
            account_id: Some(account.id.clone()),
            provider_session_id: plan.session_id.clone(),
            launch_kind,
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
        let runtime = self.runtime(&task.id);
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
        let current = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
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
                    Some(Activity::Working),
                    self.now(),
                )
                .await?;
        }
        if let Some(session_id) = &plan.session_id {
            self.record_lineage(&task, harness, session_id, "native")
                .await?;
        }
        self.attach_run(&task.id, &attempt, &host_token).await?;
        runtime.hub.state_changed();
        self.publish_task(&task.id).await;
        if goal_mode {
            self.spawn_goal(&task.id, &attempt_id, &task.prompt);
        }
        Ok(())
    }

    /// Types `/goal <prompt>` into the attempt once its TUI is ready: after the first output
    /// and [`GOAL_QUIET`] without new output, or at most [`GOAL_READY_TIMEOUT`] after the start.
    fn spawn_goal(self: &Arc<Self>, task_id: &TaskId, attempt_id: &AttemptId, prompt: &str) {
        let app = Arc::clone(self);
        let (task_id, attempt_id) = (task_id.clone(), attempt_id.clone());
        let command = format!(
            "/goal {}",
            goal_text(prompt).unwrap_or_else(|| GOAL_FOR_LONG_PROMPTS.to_owned())
        );
        tokio::spawn(async move {
            let runtime = app.runtime(&task_id);
            let started = tokio::time::Instant::now();
            let mut last = runtime.hub.output_seq();
            let mut quiet_since = started;
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let live = matches!(
                    app.deps.store.live_attempt(&task_id).await,
                    Ok(Some(a)) if a.id == attempt_id
                );
                if !live {
                    return;
                }
                let seq = runtime.hub.output_seq();
                if seq != last {
                    last = seq;
                    quiet_since = tokio::time::Instant::now();
                }
                let ready = last > 0 && quiet_since.elapsed() >= GOAL_QUIET;
                if ready || started.elapsed() >= GOAL_READY_TIMEOUT {
                    break;
                }
            }
            if let Err(err) = app.send_input(&task_id, command.as_bytes()).await {
                tracing::warn!(task = %task_id, error = %err, "could not set the task goal");
                return;
            }
            // A separate Enter, so the TUI sees typed text first and then a submit.
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = app.send_input(&task_id, b"\r").await;
        });
    }

    fn open_credential(&self, account: &AccountRecord) -> AppResult<Secret> {
        let sealed = account.credential.as_ref().ok_or_else(|| {
            AppError::Conflict(format!("account {} has no credential", account.label))
        })?;
        String::from_utf8(self.deps.crypto.open(sealed)?)
            .map(Secret)
            .map_err(AppError::internal)
    }

    fn base_env(&self, sandbox: &SandboxRecord, home: &Path) -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        let mut env = vec![
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("PATH".into(), path),
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("YULIUS_PORT_BASE".into(), sandbox.port_base.to_string()),
            ("YULIUS_PORT_COUNT".into(), sandbox.port_count.to_string()),
            ("PORT".into(), sandbox.port_base.to_string()),
            ("YULIUS_SANDBOX".into(), sandbox.slug.clone()),
        ];
        env.extend(git_credential_env(&self.config.yulius_exe));
        env
    }

    async fn record_lineage(
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

    fn transcript_path(
        &self,
        sandbox: &SandboxRecord,
        worktree: &Path,
        harness: Harness,
        session_id: &str,
        stored: Option<&str>,
    ) -> AppResult<Option<PathBuf>> {
        if let Some(path) = stored {
            return Ok(Some(PathBuf::from(path)));
        }
        match harness {
            Harness::ClaudeCode => Ok(Some(self.deps.harness.claude_transcript(
                &self.sandbox_home(&sandbox.id),
                worktree,
                session_id,
            ))),
        }
    }

    /// Attaches to a run's host, resets the hub and starts the output pump and transcript watcher.
    async fn attach_run(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        host_token: &[u8; 32],
    ) -> AppResult<()> {
        let runtime = self.runtime(task_id);
        let attached = self
            .deps
            .session_host
            .attach(Path::new(&attempt.run_dir), host_token)
            .await?;
        let welcome = attached.welcome.clone();
        runtime.hub.reset(
            welcome.seq,
            welcome.snapshot.as_deref().unwrap_or_default(),
            welcome.cols,
            welcome.rows,
        );
        *runtime.run.lock().await = Some(ActiveRun {
            attempt_id: attempt.id.clone(),
            writer: attached.writer,
        });
        let app = self.clone();
        let tid = task_id.clone();
        let aid = attempt.id.clone();
        let token = *host_token;
        let mut reader = attached.reader;
        tokio::spawn(async move {
            let mut reattaches = 0;
            loop {
                match reader.next().await {
                    Ok(Some(RunEvent::Output { seq, bytes })) => {
                        app.runtime(&tid).hub.output(seq, &bytes)
                    }
                    Ok(Some(RunEvent::Snapshot { seq, bytes })) => {
                        let (cols, rows) = app.runtime(&tid).hub.size();
                        app.runtime(&tid).hub.reset(seq, &bytes, cols, rows);
                    }
                    Ok(Some(RunEvent::Exit(exit))) => {
                        app.finalize_attempt(&tid, &aid, exit).await;
                        return;
                    }
                    Ok(None) | Err(_) => {
                        // Connection lost without an exit: reattach (the host may have dropped a slow client).
                        if reattaches < REATTACH_ATTEMPTS {
                            reattaches += 1;
                            tokio::time::sleep(Duration::from_millis(200 * u64::from(reattaches)))
                                .await;
                            if let Ok(Some(run_dir)) = app
                                .deps
                                .store
                                .attempt(&aid)
                                .await
                                .map(|a| a.map(|a| a.run_dir))
                                && let Ok(again) = app
                                    .deps
                                    .session_host
                                    .attach(Path::new(&run_dir), &token)
                                    .await
                            {
                                {
                                    let rt = app.runtime(&tid);
                                    rt.hub.reset(
                                        again.welcome.seq,
                                        again.welcome.snapshot.as_deref().unwrap_or_default(),
                                        again.welcome.cols,
                                        again.welcome.rows,
                                    );
                                    *rt.run.lock().await = Some(ActiveRun {
                                        attempt_id: aid.clone(),
                                        writer: again.writer,
                                    });
                                    reader = again.reader;
                                    continue;
                                }
                            }
                            continue;
                        }
                        app.resolve_lost_connection(&tid, &aid).await;
                        return;
                    }
                }
            }
        });
        self.start_watcher(task_id, attempt);
        Ok(())
    }

    async fn resolve_lost_connection(self: &Arc<Self>, task_id: &TaskId, attempt_id: &AttemptId) {
        let Ok(Some(attempt)) = self.deps.store.attempt(attempt_id).await else {
            return;
        };
        match self.deps.session_host.inspect(Path::new(&attempt.run_dir)) {
            RunInspection::Exited(exit) => self.finalize_attempt(task_id, attempt_id, exit).await,
            RunInspection::Recorded {
                host_alive: true, ..
            } => {
                let _ = self
                    .deps
                    .session_host
                    .terminate(Path::new(&attempt.run_dir));
                self.mark_lost(task_id, attempt_id, StopReason::Lost).await;
            }
            _ => self.mark_lost(task_id, attempt_id, StopReason::Lost).await,
        }
    }

    async fn mark_lost(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        reason: StopReason,
    ) {
        let now = self.now();
        let update = AttemptUpdate {
            ended_at: Some(now),
            exit_reason: Some("lost".into()),
            ..AttemptUpdate::default()
        };
        let _ = self
            .deps
            .store
            .transition_attempt(attempt_id, AttemptState::Lost, &update)
            .await;
        if let Ok(Some(task)) = self.deps.store.task(task_id).await
            && let Ok(next) = task.status.apply(TaskEvent::Lost(reason))
        {
            let _ = self
                .deps
                .store
                .transition_task(task_id, Some(task.status), next, None, now)
                .await;
        }
        if let Some(rt) = self.existing_runtime(task_id) {
            rt.watcher_alive.store(false, Ordering::SeqCst);
            *rt.run.lock().await = None;
            rt.hub.exit(TaskStatus::Stopped(reason).as_str(), None);
        }
        self.publish_task(task_id).await;
        self.drain_queue().await;
    }

    /// Records the end of a run exactly once and moves the task to its next state.
    async fn finalize_attempt(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        exit: RunExit,
    ) {
        let store = &self.deps.store;
        let runtime = self.runtime(task_id);
        let intent = runtime.intent.lock().ok().and_then(|mut g| g.take());
        runtime.watcher_alive.store(false, Ordering::SeqCst);
        // Keep the last screen so viewers still see how the run ended after a server restart.
        if let Ok(Some(attempt)) = store.attempt(attempt_id).await {
            let (screen, (cols, rows)) = runtime.hub.formatted_screen();
            let mut record = format!("{cols} {rows}\n").into_bytes();
            record.extend_from_slice(&screen);
            let path = PathBuf::from(&attempt.run_dir).join(FINAL_SCREEN_FILE);
            if let Err(err) = crate::supervisor::write_private_file(&path, &record) {
                tracing::debug!(error = %err, "cannot save the final screen");
            }
        }
        let state = if intent.is_some() {
            AttemptState::Killed
        } else {
            AttemptState::Exited
        };
        let now = self.now();
        let update = AttemptUpdate {
            ended_at: Some(now),
            exit_code: exit.code,
            exit_reason: Some(match (&intent, exit.spawn_failed) {
                (_, true) => "spawn_failed".into(),
                (Some(EndIntent::Cancel), _) => "cancelled".into(),
                (Some(EndIntent::Switch(_)), _) => "switched".into(),
                (None, _) if exit.is_clean() => "exited".into(),
                (None, _) => format!("exited code={:?} signal={:?}", exit.code, exit.signal),
            }),
            ..AttemptUpdate::default()
        };
        let first = store
            .transition_attempt(attempt_id, state, &update)
            .await
            .unwrap_or(false);
        if !first {
            return; // another observer already recorded the outcome
        }
        self.save_metrics(task_id, attempt_id, &runtime).await;
        {
            let mut run = runtime.run.lock().await;
            if run.as_ref().is_some_and(|r| &r.attempt_id == attempt_id) {
                *run = None;
            }
        }
        match intent {
            Some(EndIntent::Switch(done)) => {
                let _ = done.send(());
                return; // the switch controller launches the next run and drains the queue
            }
            Some(EndIntent::Cancel) => {
                if let Ok(Some(task)) = store.task(task_id).await
                    && let Ok(next) = task.status.apply(TaskEvent::Reaped)
                {
                    let _ = store
                        .transition_task(task_id, Some(task.status), next, None, now)
                        .await;
                }
            }
            None => {
                if let Ok(Some(task)) = store.task(task_id).await {
                    let event = TaskEvent::Exited {
                        normal: exit.is_clean(),
                    };
                    if let Ok(next) = task.status.apply(event) {
                        let _ = store
                            .transition_task(task_id, Some(task.status), next, None, now)
                            .await;
                    }
                }
            }
        }
        let status = store
            .task(task_id)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "stopped_lost".into(), |t| t.status.as_str().to_owned());
        runtime.hub.exit(&status, exit.code);
        self.publish_task(task_id).await;
        if let Ok(Some(task)) = self.deps.store.task(task_id).await
            && let Err(err) = self.refresh_onboarding(&task.sandbox_id).await
        {
            tracing::warn!(error = %err, task = %task_id, "onboarding refresh failed");
        }
        self.drain_queue().await;
    }

    async fn save_metrics(&self, task_id: &TaskId, attempt_id: &AttemptId, runtime: &TaskRuntime) {
        let snapshot = runtime.usage.lock().map(|u| u.clone()).unwrap_or_default();
        let metrics = MetricsRecord {
            model: snapshot.model,
            input_tokens: snapshot.tokens.input,
            output_tokens: snapshot.tokens.output,
            cache_read: snapshot.tokens.cache_read,
            cache_write: snapshot.tokens.cache_write,
            turns: u32::try_from(snapshot.turns).ok().filter(|t| *t > 0),
            active_ms: None,
        };
        let _ = self
            .deps
            .store
            .put_metrics(attempt_id, task_id, &metrics, self.now())
            .await;
    }

    /// Starts queued tasks while capacity allows (FIFO).
    pub async fn drain_queue(self: &Arc<Self>) {
        let Ok(queued) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Queued])
            .await
        else {
            return;
        };
        for task in queued {
            let live = self
                .deps
                .store
                .live_attempts()
                .await
                .map(|l| l.len())
                .unwrap_or(usize::MAX);
            if u32::try_from(live).unwrap_or(u32::MAX) >= self.config.max_live_sessions {
                return;
            }
            let has_lineage = self
                .deps
                .store
                .provider_sessions(&task.id)
                .await
                .map(|l| !l.is_empty())
                .unwrap_or(false);
            let start = if has_lineage {
                RunStart::Resume { prompt: None }
            } else {
                RunStart::Fresh { prompt: None }
            };
            let app = self.clone();
            let id = task.id.clone();
            tokio::spawn(async move {
                if let Err(err) = app.start_run(&id, start, None).await {
                    tracing::warn!(error = %err, task = %id, "queued task failed to start");
                }
            });
        }
    }

    /// Stops a task: graceful shutdown with escalation, then (if needed) termination and `lost`.
    ///
    /// # Errors
    /// `Conflict` when the task is not live.
    pub async fn stop_task_now(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        if task.status == TaskStatus::Queued
            || task.status == TaskStatus::WaitingCapacity
                && store.live_attempt(task_id).await?.is_none()
        {
            store
                .transition_task(
                    task_id,
                    Some(task.status),
                    TaskStatus::Stopped(StopReason::Cancelled),
                    None,
                    self.now(),
                )
                .await?;
            self.publish_task(task_id).await;
            return Ok(());
        }
        let next = task
            .status
            .apply(TaskEvent::CancelRequested)
            .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?;
        let runtime = self.runtime(task_id);
        if let Ok(mut intent) = runtime.intent.lock() {
            *intent = Some(EndIntent::Cancel);
        }
        if task.status != TaskStatus::Stopping {
            store
                .transition_task(task_id, Some(task.status), next, None, self.now())
                .await?;
        }
        self.publish_task(task_id).await;
        let total = STOP_INTERRUPT_GRACE_MS + STOP_TERM_GRACE_MS + STOP_KILL_WAIT_MS;
        if let Some(run) = runtime.run.lock().await.as_mut() {
            let _ = run
                .writer
                .shutdown(u32::try_from(STOP_INTERRUPT_GRACE_MS).unwrap_or(5000))
                .await;
        }
        let app = self.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(
                u64::try_from(total).unwrap_or(20_000),
            ))
            .await;
            if let Ok(Some(attempt)) = app.deps.store.live_attempt(&tid).await {
                tracing::warn!(task = %tid, "run did not exit after escalation; terminating host");
                let _ = app.deps.session_host.terminate(Path::new(&attempt.run_dir));
                app.mark_lost(&tid, &attempt.id, StopReason::Lost).await;
            }
        });
        Ok(())
    }

    /// Switches a running task to another account (manual or rotation-driven).
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn switch_task_now(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
        prompt: Option<String>,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let attempt = store.live_attempt(task_id).await?;
        let next = task
            .status
            .apply(TaskEvent::SwitchStarted)
            .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?;
        if !store
            .transition_task(task_id, Some(task.status), next, None, self.now())
            .await?
        {
            return Err(AppError::Conflict("task state changed concurrently".into()));
        }
        let from_desc = match &attempt {
            Some(a) => self.account_desc(a.harness, a.account_id.as_ref()).await,
            None => task.harness.as_str().to_owned(),
        };
        let decision_id = self.deps.crypto.new_id();
        let to_desc = self.account_desc(task.harness, account.as_ref()).await;
        store
            .insert_switch(&SwitchDecisionRecord {
                id: decision_id.clone(),
                task_id: task_id.clone(),
                at: self.now(),
                reason: reason.to_owned(),
                inputs: serde_json::json!({"account": account.as_ref().map(AccountId::as_str)})
                    .to_string(),
                from_desc: from_desc.clone(),
                to_desc: to_desc.clone(),
                outcome: "started".into(),
            })
            .await?;
        let runtime = self.runtime(task_id);
        runtime.hub.notice(format!(
            "↻ Yulius is switching this session from {from_desc} to {to_desc}"
        ));
        self.publish_task(task_id).await;
        if let Some(attempt) = attempt {
            let (tx, rx) = oneshot::channel();
            if let Ok(mut intent) = runtime.intent.lock() {
                *intent = Some(EndIntent::Switch(tx));
            }
            if let Some(run) = runtime.run.lock().await.as_mut() {
                let _ = run
                    .writer
                    .shutdown(u32::try_from(STOP_INTERRUPT_GRACE_MS).unwrap_or(5000))
                    .await;
            }
            let budget = Duration::from_millis(
                u64::try_from(yulius_domain::task::SWITCH_TIMEOUT_MS / 2).unwrap_or(60_000),
            );
            if tokio::time::timeout(budget, rx).await.is_err() {
                let _ = self
                    .deps
                    .session_host
                    .terminate(Path::new(&attempt.run_dir));
                let update = AttemptUpdate {
                    ended_at: Some(self.now()),
                    exit_reason: Some("switch_timeout".into()),
                    ..AttemptUpdate::default()
                };
                let _ = store
                    .transition_attempt(&attempt.id, AttemptState::Lost, &update)
                    .await;
            }
        }
        let start = RunStart::Resume { prompt };
        match self.start_run(task_id, start, account).await {
            Ok(()) => {
                store.set_switch_outcome(&decision_id, "completed").await?;
                runtime.hub.notice(format!("↻ Continuing on {to_desc}"));
                Ok(())
            }
            Err(err) => {
                store.set_switch_outcome(&decision_id, "failed").await?;
                if let Ok(Some(current)) = store.task(task_id).await
                    && current.status == TaskStatus::Switching
                {
                    let _ = store
                        .transition_task(
                            task_id,
                            Some(current.status),
                            TaskStatus::Stopped(StopReason::SwitchFailed),
                            None,
                            self.now(),
                        )
                        .await;
                }
                self.publish_task(task_id).await;
                Err(err)
            }
        }
    }

    async fn account_desc(&self, harness: Harness, account: Option<&AccountId>) -> String {
        let label = match account {
            Some(id) => self
                .deps
                .store
                .account(id)
                .await
                .ok()
                .flatten()
                .map(|a| a.label),
            None => None,
        };
        match label {
            Some(l) => format!("{} ({l})", harness.display_name()),
            None => harness.display_name().to_owned(),
        }
    }

    /// Writes viewer keyboard input to the run.
    ///
    /// # Errors
    /// `Conflict` when no run is attached.
    pub async fn send_input(&self, task_id: &TaskId, bytes: &[u8]) -> AppResult<()> {
        let runtime = self
            .existing_runtime(task_id)
            .ok_or_else(|| AppError::Conflict("the task is not running".into()))?;
        runtime.last_input_ms.store(self.now().0, Ordering::SeqCst);
        let mut run = runtime.run.lock().await;
        let run = run
            .as_mut()
            .ok_or_else(|| AppError::Conflict("the task is not running".into()))?;
        run.writer.input(bytes).await
    }

    /// Resizes the run's PTY.
    ///
    /// # Errors
    /// `Conflict` when no run is attached.
    pub async fn resize_run(&self, task_id: &TaskId, cols: u16, rows: u16) -> AppResult<()> {
        let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(5, 1000));
        let runtime = self.runtime(task_id);
        runtime.hub.resize(cols, rows);
        let mut run = runtime.run.lock().await;
        match run.as_mut() {
            Some(run) => run.writer.resize(cols, rows).await,
            None => Ok(()),
        }
    }

    /// Reconciles recorded live runs after a server start (SPEC-002 §7.1). Never respawns.
    ///
    /// # Errors
    /// Store failures.
    pub async fn reconcile(self: &Arc<Self>) -> AppResult<()> {
        let store = &self.deps.store;
        for attempt in store.live_attempts().await? {
            let host_token: Option<[u8; 32]> = self
                .deps
                .crypto
                .open(&attempt.host_token)
                .ok()
                .and_then(|b| b.try_into().ok());
            let reattached = match host_token {
                Some(token) => self
                    .attach_run(&attempt.task_id, &attempt, &token)
                    .await
                    .is_ok(),
                None => false,
            };
            if reattached {
                tracing::info!(task = %attempt.task_id, "reattached live run");
                if let Ok(Some(task)) = store.task(&attempt.task_id).await
                    && matches!(task.status, TaskStatus::Starting | TaskStatus::Switching)
                {
                    let _ = store
                        .transition_task(
                            &task.id,
                            Some(task.status),
                            TaskStatus::Running,
                            Some(Activity::Waiting),
                            self.now(),
                        )
                        .await;
                }
                continue;
            }
            match self.deps.session_host.inspect(Path::new(&attempt.run_dir)) {
                RunInspection::Exited(exit) => {
                    self.finalize_attempt(&attempt.task_id, &attempt.id, exit)
                        .await
                }
                RunInspection::Recorded { host_alive, .. } => {
                    if host_alive {
                        let _ = self
                            .deps
                            .session_host
                            .terminate(Path::new(&attempt.run_dir));
                    }
                    self.mark_lost(&attempt.task_id, &attempt.id, StopReason::ServerLost)
                        .await;
                }
                RunInspection::Missing => {
                    self.mark_lost(&attempt.task_id, &attempt.id, StopReason::ServerLost)
                        .await
                }
            }
        }
        // Tasks that claim to be starting/switching/stopping without a live attempt.
        for task in store
            .tasks_with_status(&[
                TaskStatus::Starting,
                TaskStatus::Switching,
                TaskStatus::Stopping,
                TaskStatus::Running,
            ])
            .await?
        {
            if store.live_attempt(&task.id).await?.is_none() {
                let _ = store
                    .transition_task(
                        &task.id,
                        Some(task.status),
                        TaskStatus::Stopped(StopReason::ServerLost),
                        None,
                        self.now(),
                    )
                    .await;
            }
        }
        self.drain_queue().await;
        Ok(())
    }

    /// Account usage update from transcripts or providers.
    pub(crate) async fn record_account_usage(&self, account: &AccountId, usage: &AccountUsage) {
        let status = if usage.limit_in_effect(self.now()) {
            AccountStatus::Limited
        } else {
            AccountStatus::Ok
        };
        let json = usage_json::to_json(usage);
        let _ = self
            .deps
            .store
            .set_account_state(account, status, Some(&json), None, self.now())
            .await;
        // At or above the threshold: tasks on this account rotate at their next turn boundary
        // (ROT-003).
        if usage.exhausted(self.now(), rotation::DEFAULT_THRESHOLD)
            && let Ok(live) = self.deps.store.live_attempts().await
        {
            for attempt in live
                .iter()
                .filter(|a| a.account_id.as_ref() == Some(account))
            {
                self.runtime(&attempt.task_id)
                    .proactive_switch
                    .store(true, Ordering::SeqCst);
            }
        }
    }

    fn start_watcher(self: &Arc<Self>, task_id: &TaskId, attempt: &AttemptRecord) {
        let runtime = self.runtime(task_id);
        runtime.watcher_alive.store(true, Ordering::SeqCst);
        if let Ok(mut u) = runtime.usage.lock() {
            *u = UsageSnapshot::default();
        }
        let app = self.clone();
        let tid = task_id.clone();
        let attempt = attempt.clone();
        let alive = runtime.watcher_alive.clone();
        tokio::spawn(async move { app.watch_loop(tid, attempt, alive).await });
    }

    async fn watch_loop(
        self: Arc<Self>,
        task_id: TaskId,
        attempt: AttemptRecord,
        alive: Arc<AtomicBool>,
    ) {
        let Ok(Some(task)) = self.deps.store.task(&task_id).await else {
            return;
        };
        let Ok(Some(sandbox)) = self.deps.store.sandbox(&task.sandbox_id).await else {
            return;
        };
        let Some(worktree) = task.worktree_path.clone().map(PathBuf::from) else {
            return;
        };
        let started = attempt.started_at;
        let mut session_id = attempt.provider_session_id.clone();
        let mut watch = None;
        let mut last_drift_check = Timestamp(0);
        while alive.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(WATCH_INTERVAL_MS)).await;
            if !alive.load(Ordering::SeqCst) {
                break;
            }
            let now = self.now();
            // Claude session-id drift (/clear, /resume inside the TUI): adopt into the lineage.
            if attempt.harness == Harness::ClaudeCode && now.millis_since(last_drift_check) > 5_000
            {
                last_drift_check = now;
                if let Ok(Some((id, _))) = self.deps.harness.newest_claude_session(
                    &self.sandbox_home(&sandbox.id),
                    &worktree,
                    started,
                ) && session_id.as_deref() != Some(id.as_str())
                {
                    let _ = self.deps.store.set_attempt_session(&attempt.id, &id).await;
                    let _ = self
                        .record_lineage(&task, Harness::ClaudeCode, &id, "native")
                        .await;
                    session_id = Some(id);
                    watch = None;
                }
            }
            if watch.is_none()
                && let Some(id) = &session_id
                && let Ok(Some(path)) =
                    self.transcript_path(&sandbox, &worktree, attempt.harness, id, None)
                && path.exists()
            {
                watch = self
                    .deps
                    .harness
                    .watch(
                        attempt.harness,
                        &path,
                        attempt.launch_kind == LaunchKind::New,
                    )
                    .ok();
            }
            let Some(w) = watch.as_mut() else { continue };
            let observations = match w.poll() {
                Ok(o) => o,
                Err(err) => {
                    tracing::debug!(error = %err, "transcript poll failed");
                    continue;
                }
            };
            let usage = w.usage();
            let runtime = self.runtime(&task_id);
            if let Ok(mut u) = runtime.usage.lock() {
                *u = usage;
            }
            for observation in observations {
                self.on_observation(&task_id, &attempt, observation).await;
            }
        }
    }

    async fn on_observation(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        observation: Observation,
    ) {
        let runtime = self.runtime(task_id);
        match observation {
            Observation::LimitReached { resets_at, message } => {
                tracing::info!(task = %task_id, %message, "usage limit reached");
                if let Some(account) = &attempt.account_id {
                    let mut usage = match self.deps.store.account(account).await {
                        Ok(Some(a)) => a
                            .usage_json
                            .as_deref()
                            .map(usage_json::from_json)
                            .unwrap_or_default(),
                        _ => AccountUsage::default(),
                    };
                    usage.limit_reached = true;
                    usage.limit_resets_at = resets_at;
                    usage.observed_at = Some(self.now());
                    self.record_account_usage(account, &usage).await;
                }
                runtime
                    .limit_resets_at
                    .store(resets_at.map_or(0, |t| t.0), Ordering::SeqCst);
                let app = self.clone();
                let tid = task_id.clone();
                let account = attempt.account_id.clone();
                tokio::spawn(async move { app.rotate(&tid, account, Trigger::LimitReached).await });
            }
            Observation::UserTurn => {
                let _ = self
                    .deps
                    .store
                    .set_activity(task_id, Some(Activity::Working), self.now())
                    .await;
                self.publish_task(task_id).await;
            }
            Observation::TurnComplete => {
                self.turn_ended(task_id, attempt.account_id.clone());
            }
            Observation::Session { .. } => {}
        }
    }

    /// Applies the rotation policy for a task and acts on the decision (SPEC-001 §7.4).
    pub async fn rotate(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
    ) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running {
            return;
        }
        let Ok(accounts) = self.account_views().await else {
            return;
        };
        let input = RotationInput {
            current: current.clone(),
            task_provider: task.harness.provider(),
            accounts,
            now: self.now(),
            threshold: rotation::DEFAULT_THRESHOLD,
        };
        let (decision, reason) = rotation::decide(&input, trigger.clone());
        let runtime = self.runtime(task_id);
        runtime.proactive_switch.store(false, Ordering::SeqCst);
        match decision {
            Decision::Stay => {}
            Decision::Switch { account } => {
                let prompt = matches!(trigger, Trigger::LimitReached).then(|| {
                    "Yulius moved this session to another account because the previous one reached its usage limit. Continue the task exactly where you left off.".to_owned()
                });
                if let Err(err) = self
                    .switch_task_now(task_id, Some(account), &format!("{reason:?}"), prompt)
                    .await
                {
                    tracing::warn!(error = %err, task = %task_id, "automatic switch failed");
                }
            }
            Decision::Wait { until } => {
                if let Ok(next) = task.status.apply(TaskEvent::CapacityExhausted) {
                    let _ = self
                        .deps
                        .store
                        .transition_task(
                            task_id,
                            Some(task.status),
                            next,
                            Some(Activity::NeedsAttention),
                            self.now(),
                        )
                        .await;
                }
                let when = until.map_or_else(|| "an unknown time".to_owned(), crate::auth::rfc3339);
                runtime.hub.notice(format!(
                    "All connected accounts are at their usage limit. Yulius will continue at {when}."
                ));
                self.publish_task(task_id).await;
                let app = self.clone();
                let tid = task_id.clone();
                let delay = until.map_or(300_000, |t| {
                    t.millis_since(self.now()).clamp(5_000, 3_600_000)
                });
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(
                        u64::try_from(delay).unwrap_or(300_000),
                    ))
                    .await;
                    app.retry_waiting(&tid).await;
                });
            }
        }
    }

    pub(crate) fn retry_waiting<'a>(self: &'a Arc<Self>, task_id: &'a TaskId) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Ok(Some(task)) = self.deps.store.task(task_id).await else {
                return;
            };
            if task.status != TaskStatus::WaitingCapacity {
                return;
            }
            let current = self
                .deps
                .store
                .live_attempt(task_id)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.account_id);
            let Ok(accounts) = self.account_views().await else {
                return;
            };
            let input = RotationInput {
                current: None,
                task_provider: task.harness.provider(),
                accounts,
                now: self.now(),
                threshold: rotation::DEFAULT_THRESHOLD,
            };
            match rotation::decide(&input, Trigger::Start) {
                (Decision::Switch { account }, reason) => {
                    let prompt = Some("Usage capacity is available again. Continue the task exactly where you left off.".to_owned());
                    if Some(&account) == current.as_ref() {
                        // Same account recovered: just nudge the live session.
                        let _ = self
                            .deps
                            .store
                            .transition_task(
                                task_id,
                                Some(task.status),
                                TaskStatus::Running,
                                Some(Activity::Working),
                                self.now(),
                            )
                            .await;
                        if let Some(p) = prompt {
                            let _ = self.send_input(task_id, format!("{p}\r").as_bytes()).await;
                        }
                        self.publish_task(task_id).await;
                    } else if let Err(err) = self
                        .switch_task_now(task_id, Some(account), &format!("{reason:?}"), prompt)
                        .await
                    {
                        tracing::warn!(error = %err, "resume after capacity wait failed");
                    }
                }
                (_, _) => {
                    let app = self.clone();
                    let tid = task_id.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(300_000)).await;
                        app.retry_waiting(&tid).await;
                    });
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn final_screen_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(super::FINAL_SCREEN_FILE);
        super::write_private_file(&path, b"120 40\n\x1b[1mbye")?;
        let (cols, rows, bytes) = super::read_final_screen(dir.path()).ok_or("unreadable")?;
        assert_eq!(
            (cols, rows, bytes.as_slice()),
            (120, 40, b"\x1b[1mbye".as_slice())
        );
        assert!(super::read_final_screen(&dir.path().join("missing")).is_none());
        Ok(())
    }

    use super::branch_for;

    #[test]
    fn goals_are_one_line_and_short() {
        assert_eq!(
            super::goal_text("Build the\n  widget\tnow").as_deref(),
            Some("Build the widget now")
        );
        assert_eq!(super::goal_text("a\u{1b}[31mb").as_deref(), Some("a [31mb"));
        assert!(super::goal_text(&"x".repeat(450)).is_some());
        assert!(
            super::goal_text(&"x".repeat(451)).is_none(),
            "long prompts go first"
        );
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
    }
}
