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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId};
use yulius_domain::rotation::{self, AccountView, Decision, Trigger};
use yulius_domain::task::{
    Activity, AttemptState, FailReason, STOP_INTERRUPT_GRACE_MS, STOP_KILL_WAIT_MS,
    STOP_TERM_GRACE_MS, StopReason, TaskEvent, TaskStatus,
};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

use crate::auth::hex_token;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::finish::{SHELL_PROBE_MS, TranscriptFacts};
use crate::hub::Hub;
use crate::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, MetricsRecord, PauseKind,
    ProviderSessionRecord, SandboxRecord, Secret, SwitchDecisionRecord, TaskMode, TaskRecord,
};
use crate::ports::harness::{HarnessLaunch, Observation, SessionMode, UsageSnapshot};
use crate::ports::session::{Containment, RunEvent, RunExit, RunInspection, RunWriter};
use crate::ports::store::{AttemptUpdate, TaskFieldUpdate};
use crate::usage_json;

/// Boxed `Send` future (breaks recursive async cycles between start, pump and finalize).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The provider's short (session) window in minutes; longer limits are the secondary window.
const SESSION_WINDOW_MINUTES: u32 = 5 * 60;

/// Default PTY size before the first viewer reports its size.
pub const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// Transcript poll interval.
pub const WATCH_INTERVAL_MS: u64 = 1_000;
/// Reattach attempts after an unexpected host disconnect.
pub const REATTACH_ATTEMPTS: u32 = 3;
/// How long a closed run may take to be recorded as ended after the close escalation (SPEC-039).
const CLOSE_SETTLE_MS: i64 = 5_000;
/// How often a close waiting for its run looks again without an update (SPEC-039).
const CLOSE_RECHECK: Duration = Duration::from_secs(1);

/// Why Yulius is ending a run.
enum EndIntent {
    /// User or system stop.
    Cancel,
    /// A finished task is closed (archived when Done): it ends as a clean exit, `closed`.
    Close,
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
    /// The current run called a Yulius hook: its Stop hook is the turn boundary that decides
    /// switches and pauses, not the transcript (SPEC-002 §7.5).
    pub(crate) hooks_seen: AtomicBool,
    /// Last seen limit reset time (ms, 0 = unknown).
    pub limit_resets_at: AtomicI64,
    /// When the current turn started (ms, 0 = no turn in progress), for active agent time.
    turn_started_ms: AtomicI64,
    /// Active agent time of the current attempt: its finished turns (ms).
    active_ms: AtomicI64,
    /// The head Yulius last tried to open a pull request for by itself (#35), so a failed try is
    /// not repeated every turn.
    pub(crate) auto_pr_head: Mutex<Option<String>>,
    /// When a person last typed into the task's terminal (ms, 0 = never): a prompt submitted
    /// soon after is theirs (TSK-013).
    pub(crate) typed_at_ms: AtomicI64,
    /// Who typed last.
    pub(crate) typed_by: Mutex<Option<yulius_domain::ids::UserId>>,
    /// Claude Code recorded the session goal as achieved and no prompt came since (TSK-014).
    pub(crate) goal_achieved: AtomicBool,
    /// Claude Code recorded something about the goal since the `/goal` was typed: it was
    /// submitted, not left in the prompt box (a prompt alone proves nothing: it may be another).
    pub(crate) goal_submitted: AtomicBool,
    /// Claude Code's SessionStart hook arrived for the current attempt: its prompt box reads
    /// input (typing before it is lost or left unsubmitted, F 2026-10-12).
    pub(crate) session_started: AtomicBool,
    /// The goal-mode session opened with a first message (its attached files, SPEC-025) whose
    /// turn has not ended yet: its goal is typed after that turn's Stop hook.
    pub opening_turn: AtomicBool,
    /// A merge `delivery_merge` armed while the checks still ran (SPEC-011): Yulius performs it
    /// once they pass.
    pub(crate) armed_merge: Mutex<Option<crate::agent_wait::ArmedMerge>>,
    /// How the last armed merge ended, for `yulius wait` (SPEC-028 MSR-003).
    pub(crate) merge_outcome: Mutex<Option<crate::agent_wait::MergeOutcome>>,
    /// Armed-merge decisions in progress (the armed merge is taken out meanwhile): `yulius wait`
    /// still reads the merge as armed until it is decided or put back.
    pub(crate) merge_deciding: AtomicU32,
    /// When the last clean Stop ended a turn (ms), so a prompt of that turn read late from the
    /// transcript does not reopen it.
    pub(crate) last_stop_ms: AtomicI64,
    /// Background shells the agent left running, as last probed (-1 = not known).
    pub(crate) background_shells: AtomicI64,
    /// Since when (ms) the idle agent's background shells have been running (0 = none).
    pub(crate) shells_since_ms: AtomicI64,
    /// The run was reattached after a restart: the transcript it already wrote is history.
    pub(crate) replay_transcript: AtomicBool,
    /// Why Claude Code said the current session ends (`SessionEnd` hook), until the run exits.
    pub(crate) session_end: Mutex<Option<String>>,
    /// When a held hook of the current run last asked again (ms, 0 = not since this server
    /// started): a hold nobody polls any more was given up by the CLI (ADR-17).
    pub(crate) hold_polled_ms: AtomicI64,
    /// Turn-end step nudges already given to the agent (`Nudge::key`), so each is given once.
    pub(crate) nudged: Mutex<std::collections::BTreeSet<String>>,
    /// The terminal already said that this task acts on GitHub as the App, not its creator
    /// (F 2026-10-11), so it is said once.
    pub(crate) acting_as_app_said: AtomicBool,
    /// When a switch of this task was last refused before it started (ms, 0 = never): an
    /// automatic switch waits [`SWITCH_COOLDOWN_MS`] before it tries again.
    pub(crate) switch_refused_ms: AtomicI64,
}

/// After a switch was refused before it started, automatic switches of the task wait this long
/// (the agent keeps working on its current subscription meanwhile).
pub const SWITCH_COOLDOWN_MS: i64 = 10 * 60 * 1000;

impl TaskRuntime {
    /// Whether a run is attached (a process can redraw the terminal).
    pub(crate) async fn has_run(&self) -> bool {
        self.run.lock().await.is_some()
    }

    fn new() -> Self {
        Self {
            hub: Hub::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            run: tokio::sync::Mutex::new(None),
            intent: Mutex::new(None),
            usage: Arc::new(Mutex::new(UsageSnapshot::default())),
            watcher_alive: Arc::new(AtomicBool::new(false)),
            last_input_ms: AtomicI64::new(0),
            proactive_switch: AtomicBool::new(false),
            hooks_seen: AtomicBool::new(false),
            limit_resets_at: AtomicI64::new(0),
            turn_started_ms: AtomicI64::new(0),
            active_ms: AtomicI64::new(0),
            auto_pr_head: Mutex::new(None),
            typed_at_ms: AtomicI64::new(0),
            typed_by: Mutex::new(None),
            goal_achieved: AtomicBool::new(false),
            goal_submitted: AtomicBool::new(false),
            session_started: AtomicBool::new(false),
            opening_turn: AtomicBool::new(false),
            armed_merge: Mutex::new(None),
            merge_outcome: Mutex::new(None),
            merge_deciding: AtomicU32::new(0),
            last_stop_ms: AtomicI64::new(0),
            background_shells: AtomicI64::new(-1),
            shells_since_ms: AtomicI64::new(0),
            replay_transcript: AtomicBool::new(false),
            session_end: Mutex::new(None),
            hold_polled_ms: AtomicI64::new(0),
            nudged: Mutex::new(std::collections::BTreeSet::new()),
            acting_as_app_said: AtomicBool::new(false),
            switch_refused_ms: AtomicI64::new(0),
        }
    }

    /// A turn started now unless one is already in progress (active agent time).
    pub(crate) fn start_turn(&self, now: Timestamp) {
        let _ = self
            .turn_started_ms
            .compare_exchange(0, now.0, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Ends the turn in progress, if any, adding its time to the attempt's active time.
    pub(crate) fn end_turn(&self, now: Timestamp) {
        let started = self.turn_started_ms.swap(0, Ordering::SeqCst);
        if started > 0 {
            self.active_ms
                .fetch_add(now.0.saturating_sub(started).max(0), Ordering::SeqCst);
        }
    }

    /// Active agent time so far: finished turns plus the one in progress.
    fn active_ms_at(&self, now: Timestamp) -> i64 {
        let started = self.turn_started_ms.load(Ordering::SeqCst);
        let running = if started > 0 {
            now.0.saturating_sub(started).max(0)
        } else {
            0
        };
        self.active_ms
            .load(Ordering::SeqCst)
            .saturating_add(running)
    }
}

/// Longest gap between live metrics saves of a run in progress (#31).
const LIVE_METRICS_MS: i64 = 30_000;

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
    /// Resume the latest provider session past the usage limit that cut its turn (SPEC-034):
    /// the transcript steps back over the limit and the agent redoes the lost turn by itself,
    /// with nothing typed or sent.
    AfterLimit,
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
/// Upper bound for waiting until the TUI is ready for the goal (its SessionStart hook and quiet
/// output); a busy host starts Claude Code slowly.
const GOAL_READY_TIMEOUT: Duration = Duration::from_secs(90);
/// Upper bound for waiting until the turn on a goal session's first message ended; after it the
/// goal is typed anyway (Claude Code queues it behind a running turn).
const GOAL_OPENING_TIMEOUT: Duration = Duration::from_secs(300);
/// How long Claude Code may take to record the goal after each submit before Yulius tries again:
/// Enter alone (the Enter was lost), the line again over a cleared prompt box (the text was
/// lost), then Enter alone, waiting longer each time.
const GOAL_SUBMIT_WAITS: [Duration; 5] = [
    Duration::from_secs(5),
    Duration::from_secs(8),
    Duration::from_secs(12),
    Duration::from_secs(20),
    Duration::from_secs(30),
];
/// The attempt (index into [`GOAL_SUBMIT_WAITS`]) that types the whole line again.
const GOAL_RETYPE_ATTEMPT: usize = 2;
/// Upper bound for waiting until the TUI took one paste of a typed first message (SPEC-030).
const PASTE_TIMEOUT: Duration = Duration::from_secs(5);
/// Quiet time after a paste changed the screen before the next one is written.
const PASTE_QUIET: Duration = Duration::from_millis(500);

/// The pastes that type a first message with attached images (SPEC-030), each a bracketed paste
/// (xterm `ESC [200~ … ESC [201~`) with what the screen shows once Claude Code took it: each
/// image's path (`[Image #N]`), then the text. The text ends with a newline, so the cursor is
/// never on a mention: Claude Code's file autocomplete would take the Enter (measured).
fn opening_pastes(images: Vec<String>, text: &str) -> Vec<(Vec<u8>, Option<String>)> {
    let paste = |content: &str| format!("\x1b[200~{content}\x1b[201~").into_bytes();
    let mut pastes: Vec<(Vec<u8>, Option<String>)> = images
        .iter()
        .enumerate()
        .map(|(i, path)| (paste(path), Some(format!("[Image #{}]", i + 1))))
        .collect();
    if !text.is_empty() {
        pastes.push((paste(&format!("{text}\n")), None));
    }
    pastes
}

/// Rust build jobs of one agent: a third of the host's CPUs, at least 2.
fn agent_build_jobs() -> usize {
    std::thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .div_ceil(3)
        .max(2)
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

/// Exit codes of a process ended by SIGHUP, SIGINT, SIGKILL or SIGTERM as a shell — or
/// bubblewrap for its sandboxed child — reports them (128 + the signal); Claude Code's own
/// handlers exit with 129 (SIGHUP) and 143 (SIGTERM).
const KILLED_EXIT_CODES: [i32; 4] = [129, 130, 137, 143];
/// SIGHUP, SIGINT, SIGKILL and SIGTERM.
const KILL_SIGNALS: [i32; 4] = [1, 2, 9, 15];

/// The task event of a run that ended without Yulius ending it (F 2026-10-10: the task follows
/// its session). The person left the session — `/exit`, Ctrl+C twice, Ctrl+D or `/logout`, which
/// Claude Code reports through its `SessionEnd` hook — or the process exited cleanly: `closed`.
/// Ended by a signal from outside (`kill`, the OOM killer): `stopped(killed)`. Anything else:
/// `stopped(crashed)`.
#[must_use]
pub fn exit_event(exit: &RunExit, session_end: Option<&str>) -> TaskEvent {
    use yulius_contracts::hooks::session_end::{LOGOUT, PROMPT_INPUT_EXIT};
    let left = matches!(session_end, Some(PROMPT_INPUT_EXIT | LOGOUT));
    if !exit.spawn_failed && (left || exit.is_clean()) {
        TaskEvent::Exited { normal: true }
    } else if exit.signal.is_some_and(|s| KILL_SIGNALS.contains(&s))
        || exit.code.is_some_and(|c| KILLED_EXIT_CODES.contains(&c))
    {
        TaskEvent::Killed
    } else {
        TaskEvent::Exited { normal: false }
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

    pub(crate) fn existing_runtime(&self, task: &TaskId) -> Option<Arc<TaskRuntime>> {
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
                healthy: matches!(a.status, AccountStatus::Ok | AccountStatus::Limited),
                usage: a
                    .usage_json
                    .as_deref()
                    .map(usage_json::from_json)
                    .unwrap_or_default(),
                in_use: u32::try_from(
                    live.iter()
                        .filter(|r| r.account_id.as_ref() == Some(&a.id))
                        .count(),
                )
                .unwrap_or(u32::MAX),
            })
            .collect())
    }

    /// When a paused task resumes: the earliest known time a subscription has capacity for it
    /// again, as the pool policy decides now (ROT-010). `None` while the task is not paused,
    /// once capacity is back, or when no reset time is known.
    pub(crate) async fn capacity_returns_at(&self, task: &TaskRecord) -> Option<Timestamp> {
        if task.status != TaskStatus::WaitingCapacity {
            return None;
        }
        let input = self.rotation_input(task, None).await.ok()?;
        match rotation::decide(&input, Trigger::Start).0 {
            Decision::Wait { until } => until,
            Decision::Switch { .. } | Decision::Stay => None,
        }
    }

    /// The account a new attempt of `task` runs on (placement, ROT-004), or `None` when no
    /// subscription has capacity and the task must pause (ROT-010). A requested account is
    /// honoured when it is usable and refused otherwise (ROT-007).
    pub(crate) async fn choose_account(
        &self,
        task: &TaskRecord,
        requested: Option<&AccountId>,
    ) -> AppResult<Option<AccountRecord>> {
        let trigger = match requested {
            Some(id) => Trigger::Manual {
                account: Some(id.clone()),
            },
            None => Trigger::Start,
        };
        let input = self.rotation_input(task, None).await?;
        match rotation::decide(&input, trigger) {
            (Decision::Switch { account, .. }, _) => Ok(Some(
                self.deps
                    .store
                    .account(&account)
                    .await?
                    .ok_or(AppError::NotFound)?,
            )),
            (Decision::Wait { .. }, _) => Ok(None),
            (Decision::Stay, reason) => Err(AppError::Capacity(format!(
                "the requested account cannot take the task right now ({reason:?})"
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
            // A worktree from before (an older identity, F 2026-10-11) commits as the creator now.
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
            .install_push_guard(&worktree, &protected)
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

    /// Commits in `task`'s worktree are its creator's: their name and the no-reply address GitHub
    /// links to their account (F 2026-10-11, [`crate::projects::identity`]).
    async fn set_task_identity(&self, worktree: &Path, task: &TaskRecord) -> AppResult<()> {
        let creator = self.deps.store.user(&task.created_by).await?;
        let (name, email) = crate::projects::commit_identity(creator.as_ref());
        self.deps.git.set_identity(worktree, &name, &email).await
    }

    /// Installs the enabled skills into the agent home. Failures are logged; a run never fails
    /// because of them.
    async fn prepare_run_extras(&self, sandbox: &SandboxRecord, home: &Path) {
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
            // A fresh start is for a task that waits to start (queued, paused for capacity). One
            // that ended meanwhile — closed before the start spawned at its creation ran — stays
            // ended: only Continue (a resume) brings a resting task back.
            if task.status.is_resting() && matches!(start, RunStart::Fresh { .. }) {
                tracing::info!(
                    task = %task.id,
                    status = task.status.as_str(),
                    "fresh start of an ended task skipped"
                );
                return Ok(());
            }
            let live =
                u32::try_from(self.deps.store.live_attempts().await?.len()).unwrap_or(u32::MAX);
            // While an update pauses the tasks, starts queue like at the session limit and run
            // once it is over (SPEC-002 ADR-18).
            let full = live >= self.config.max_live_sessions || self.update_holds_starts();
            if full && task.status != TaskStatus::Switching {
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
                TaskStatus::WaitingCapacity => TaskStatus::WaitingCapacity
                    .apply(TaskEvent::CapacityGranted)
                    .unwrap_or(TaskStatus::Starting),
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
        if matches!(start, RunStart::Resume { .. } | RunStart::AfterLimit) {
            // Imported sessions get a native transcript and a worktree from the imported state.
            self.prepare_imported_resume(&task.id).await?;
        }
        let worktree = self.ensure_worktree(&sandbox, task).await?;
        // The spec path is given out once, so parallel tasks never pick the same number.
        if let Ok(Some(fresh)) = store.task(&task.id).await {
            self.allocate_spec_path(&fresh, &worktree).await;
        }
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        let lineage = store.provider_sessions(&task.id).await?;
        let previous = lineage.last().cloned();
        let harness = task.harness;
        let Some(account) = self.choose_account(&task, requested.as_ref()).await? else {
            // No subscription has capacity: the task waits before starting, without a process
            // (ROT-010); the resume loop starts it when capacity returns. A turn a limit cut
            // keeps its `limit` pause, so it still resumes past the limit (SPEC-034).
            let kind = if matches!(start, RunStart::AfterLimit) {
                PauseKind::Limit
            } else {
                PauseKind::Start
            };
            self.pause_task(&task.id, kind, None, None).await?;
            return Ok(());
        };
        let credential = self.open_credential(&account)?;
        let home = self.sandbox_home(&sandbox.id);
        std::fs::create_dir_all(&home).map_err(AppError::internal)?;
        self.prepare_run_extras(&sandbox, &home).await;

        // Goal mode (TSK-010): the prompt is set as the session goal through `/goal` once the
        // TUI is ready; Claude's goal is per session, so a resumed session gets it again.
        let goal_mode = task.mode == TaskMode::Goal;
        // A first message with attached images is typed into the TUI, not given on the command
        // line: Claude Code attaches only a pasted image natively (SPEC-030).
        let mut typed_opening = None;
        // Decide new / resume.
        let (mode, launch_kind) = match (&start, &previous) {
            (RunStart::Resume { prompt }, Some(prev)) => (
                SessionMode::Resume {
                    session_id: prev.session_id.clone(),
                    prompt: prompt.clone(),
                    continue_interrupted: false,
                },
                LaunchKind::Resume,
            ),
            (RunStart::AfterLimit, Some(prev)) => (
                SessionMode::Resume {
                    session_id: prev.session_id.clone(),
                    prompt: None,
                    continue_interrupted: true,
                },
                LaunchKind::Resume,
            ),
            // Without a recorded session there is nothing to resume: the task starts afresh.
            (RunStart::Resume { .. } | RunStart::AfterLimit, None)
            | (RunStart::Fresh { .. }, _) => {
                let prompt = match &start {
                    RunStart::Resume { prompt } | RunStart::Fresh { prompt } => prompt.clone(),
                    RunStart::AfterLimit => None,
                };
                // In goal mode the task starts with its `/goal` (the request and the definition
                // of done, SPEC-011), after a first message with its attached files when it has
                // any; otherwise the request is the first message. Images are pasted into it and
                // other files mentioned, so Claude Code attaches them natively (SPEC-025,
                // SPEC-030).
                use yulius_domain::request::{Opening, Request};
                let prompt = match prompt {
                    Some(prompt) => Some(prompt),
                    None => match Request::parse(&task.prompt).opening(goal_mode) {
                        Some(Opening::Prompt(prompt)) => Some(prompt),
                        Some(Opening::Typed { images, text }) => {
                            typed_opening = Some((images, text));
                            None
                        }
                        None => None,
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
        };
        // A continued turn opens the run like a first message: the goal is set after its Stop.
        let opens_with_turn = goal_mode
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
        // The plan steps this session's transcript back over a usage limit (SPEC-034).
        let stepped_back = match &mode {
            SessionMode::Resume {
                session_id,
                continue_interrupted: true,
                ..
            } => Some(session_id.clone()),
            _ => None,
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
        if let Some(session_id) = &stepped_back {
            self.reindex_session(&task.sandbox_id, session_id).await;
        }
        let mut launch = plan.plan;
        let task_token = hex_token(&self.deps.crypto.random_bytes());
        let host_token = self.deps.crypto.random_bytes();
        launch
            .secret_env
            .push(("YULIUS_TASK_TOKEN".into(), task_token.clone()));
        launch.env.extend(self.base_env(&sandbox, &home));
        // Agent secrets: placeholders, the Bash redaction wrapper and the egress proxy
        // (SPEC-002 ADR-20). A vault that cannot be read fails the start, never a run without
        // redaction.
        self.vault_run_setup(&sandbox, &run_dir, &task_token, &mut launch)
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
                    .chain(join.iter().flat_map(|j| j.extra_readonly.iter().cloned()))
                    .chain(run_files)
                    .collect(),
                binds: Vec::new(),
                identity: None,
                join: join.map(Box::new),
            }
        } else {
            Containment::None
        };
        // `gh` in the agent's shell acts as the task's creator, like its Git (F 2026-10-11).
        self.add_agent_gh(&mut launch)?;
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
        // Before the spawn, so the opening turn's Stop hook can never come first.
        runtime
            .opening_turn
            .store(opens_with_turn, Ordering::SeqCst);
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
            match self.write_task_request(&sandbox, &task, &home).await {
                Ok(condition) => {
                    self.spawn_goal(&task.id, &attempt_id, typed_opening, &condition);
                }
                Err(err) => {
                    tracing::error!(task = %task.id, error = %err, "could not write the task request");
                    self.goal_not_taken(&task.id, "Yulius could not write the task's request file")
                        .await;
                }
            }
        } else if let Some(opening) = typed_opening {
            let app = Arc::clone(self);
            let (task_id, attempt_id) = (task.id.clone(), attempt_id.clone());
            tokio::spawn(async move {
                if app
                    .wait_input_ready(&task_id, &attempt_id, tokio::time::Instant::now(), false)
                    .await
                {
                    app.type_opening(&task_id, &attempt_id, opening).await;
                }
            });
        }
        Ok(())
    }

    /// Writes the task's request file into the sandbox home (`.yulius/tasks/<number>.md`: the
    /// request, its attached files and the definition of done, SPEC-011) and returns the `/goal`
    /// condition that names it.
    async fn write_task_request(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        home: &Path,
    ) -> AppResult<String> {
        use yulius_domain::request::{DoneDefinition, Request, goal_condition, request_markdown};
        let profile = crate::delivery::delivery_profile(sandbox);
        let done = DoneDefinition {
            integration_branch: profile.integration_branch.clone(),
            production_branch: profile.production_branch.clone(),
            integration_environments: profile
                .environments
                .iter()
                .filter(|e| {
                    e.after == yulius_domain::delivery::Stage::Integration && e.command.is_some()
                })
                .map(|e| e.name.clone())
                .collect(),
        };
        let author = match self.deps.store.user(&task.created_by).await {
            Ok(Some(user)) => user.github_login.unwrap_or(user.display_name),
            _ => "a person".to_owned(),
        };
        let request = Request::parse(&task.prompt);
        let dir = home.join(".yulius").join("tasks");
        let path = dir.join(format!("{}.md", task.number));
        let body = request_markdown(task.number, &task.title, &author, &request, &done);
        let write_path = path.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&write_path, body)
        })
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
        Ok(goal_condition(&request, &path.to_string_lossy(), &done))
    }

    /// The goal was never taken by the session: the task needs a person, said in its terminal.
    async fn goal_not_taken(&self, task_id: &TaskId, why: &str) {
        self.runtime(task_id).hub.notice(format!(
            "{why}. The task has no goal yet: type `/goal` and the request in the terminal, or stop \
             and continue the task."
        ));
        if let Ok(Some(task)) = self.deps.store.task(task_id).await
            && let Err(err) = self
                .set_task_activity(&task, Activity::NeedsAttention)
                .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not flag the task");
        }
    }

    /// Waits until the attempt's TUI reads input: Claude Code's SessionStart hook arrived and
    /// [`GOAL_QUIET`] passed without new output (or at most [`GOAL_READY_TIMEOUT`] after
    /// `started`); with `after_opening`, also until the turn on a first message ended (its Stop
    /// hook, at most [`GOAL_OPENING_TIMEOUT`]). `false` once the attempt is no longer live.
    async fn wait_input_ready(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        started: tokio::time::Instant,
        after_opening: bool,
    ) -> bool {
        let runtime = self.runtime(task_id);
        let mut last = runtime.hub.output_seq();
        let mut quiet_since = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if !self.attempt_live(task_id, attempt_id).await {
                return false;
            }
            let seq = runtime.hub.output_seq();
            if seq != last {
                last = seq;
                quiet_since = tokio::time::Instant::now();
            }
            // Without hooks (an old CLI) the first output is the only sign of life.
            let hooked = runtime.session_started.load(Ordering::SeqCst)
                || started.elapsed() >= GOAL_READY_TIMEOUT / 2;
            // A turn on the first message (the attached files) runs: quiet output is no sign
            // its end came, its Stop hook is (SPEC-025).
            let opening = after_opening && runtime.opening_turn.load(Ordering::SeqCst);
            let ready = hooked && !opening && last > 0 && quiet_since.elapsed() >= GOAL_QUIET;
            let limit = if opening {
                GOAL_OPENING_TIMEOUT
            } else {
                GOAL_READY_TIMEOUT
            };
            if ready || started.elapsed() >= limit {
                return true;
            }
        }
    }

    async fn attempt_live(&self, task_id: &TaskId, attempt_id: &AttemptId) -> bool {
        matches!(
            self.deps.store.live_attempt(task_id).await,
            Ok(Some(a)) if a.id == *attempt_id
        )
    }

    /// Types a session's first message that carries images (SPEC-030): each image's path as a
    /// bracketed paste of its own, then the text as one paste, then Enter. Each paste is taken
    /// before the next is written (pastes written in one burst lose the image): the screen shows
    /// its `[Image #N]`, or the output changed and settled. Typed once, never retried, so the
    /// request is never sent twice. Typing stops once the attempt is no longer live: input goes
    /// to the task's current run, which would then be another session.
    async fn type_opening(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        (images, text): (Vec<String>, String),
    ) {
        let runtime = self.runtime(task_id);
        for (paste, shown) in opening_pastes(images, &text) {
            if !self.attempt_live(task_id, attempt_id).await {
                return;
            }
            let before = runtime.hub.output_seq();
            if let Err(err) = self.send_input(task_id, &paste).await {
                tracing::warn!(task = %task_id, error = %err, "could not type the first message");
                return;
            }
            let started = tokio::time::Instant::now();
            let mut last = before;
            let mut quiet_since = started;
            while started.elapsed() < PASTE_TIMEOUT {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if shown
                    .as_deref()
                    .is_some_and(|s| runtime.hub.screen_text().contains(s))
                {
                    break;
                }
                let seq = runtime.hub.output_seq();
                if seq != last {
                    last = seq;
                    quiet_since = tokio::time::Instant::now();
                }
                if last != before && quiet_since.elapsed() >= PASTE_QUIET {
                    break;
                }
            }
        }
        if !self.attempt_live(task_id, attempt_id).await {
            return;
        }
        if let Err(err) = self.send_input(task_id, b"\r").await {
            tracing::warn!(task = %task_id, error = %err, "could not submit the first message");
        }
    }

    /// Types `/goal <condition>` into the attempt once its TUI reads input: after a typed first
    /// message with attached images (SPEC-030), Claude Code's SessionStart hook, the turn on a
    /// first message ended and quiet output ([`Self::wait_input_ready`]). The submit is confirmed
    /// by what Claude Code records about the goal and retried with growing waits; a goal never
    /// taken flags the task.
    fn spawn_goal(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        opening: Option<(Vec<String>, String)>,
        condition: &str,
    ) {
        let app = Arc::clone(self);
        let (task_id, attempt_id) = (task_id.clone(), attempt_id.clone());
        let command = format!("/goal {condition}");
        tokio::spawn(async move {
            let runtime = app.runtime(&task_id);
            let live = || app.attempt_live(&task_id, &attempt_id);
            let started = tokio::time::Instant::now();
            if let Some(opening) = opening {
                if !app
                    .wait_input_ready(&task_id, &attempt_id, started, false)
                    .await
                {
                    return;
                }
                app.type_opening(&task_id, &attempt_id, opening).await;
            }
            if !app
                .wait_input_ready(&task_id, &attempt_id, started, true)
                .await
            {
                return;
            }
            // The opening turn is over, or given up on: a later Stop is the goal's own.
            runtime.opening_turn.store(false, Ordering::SeqCst);
            runtime.goal_submitted.store(false, Ordering::SeqCst);
            let typed_before = runtime.typed_at_ms.load(Ordering::SeqCst);
            for (attempt, wait) in GOAL_SUBMIT_WAITS.iter().enumerate() {
                // A person typing meanwhile owns the prompt box: nothing is cleared or retyped.
                if runtime.typed_at_ms.load(Ordering::SeqCst) != typed_before {
                    return;
                }
                let mut input = Vec::new();
                if attempt == 0 || attempt == GOAL_RETYPE_ATTEMPT {
                    if attempt > 0 {
                        input.push(0x15); // Ctrl-U: clear what is in the prompt box
                    }
                    input.extend_from_slice(command.as_bytes());
                }
                if !input.is_empty()
                    && let Err(err) = app.send_input(&task_id, &input).await
                {
                    tracing::warn!(task = %task_id, error = %err, "could not set the task goal");
                    return;
                }
                // A separate Enter, so the TUI sees typed text first and then a submit.
                tokio::time::sleep(Duration::from_millis(300)).await;
                if let Err(err) = app.send_input(&task_id, b"\r").await {
                    tracing::warn!(task = %task_id, error = %err, "could not submit the task goal");
                    return;
                }
                let waited = tokio::time::Instant::now();
                while waited.elapsed() < *wait {
                    if runtime.goal_submitted.load(Ordering::SeqCst) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if !live().await {
                        return;
                    }
                }
                tracing::warn!(task = %task_id, attempt, "the task goal was not taken; submitting again");
            }
            if live().await && !runtime.goal_submitted.load(Ordering::SeqCst) {
                tracing::error!(task = %task_id, "the task goal was never taken by the agent");
                app.goal_not_taken(&task_id, "Claude Code did not take the task's goal")
                    .await;
            }
        });
    }

    pub(crate) fn open_credential(&self, account: &AccountRecord) -> AppResult<Secret> {
        let sealed = account.credential.as_ref().ok_or_else(|| {
            AppError::Conflict(format!("account {} has no credential", account.label))
        })?;
        String::from_utf8(self.deps.crypto.open(sealed)?)
            .map(Secret)
            .map_err(AppError::internal)
    }

    fn base_env(&self, sandbox: &SandboxRecord, home: &Path) -> Vec<(String, String)> {
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
            // Parallel tasks share the sandbox's memory: each Rust build uses a third of the
            // CPUs (at least 2), so several tasks linking at once do not run it out (OOM kills
            // of 2026-10-12).
            ("CARGO_BUILD_JOBS".into(), agent_build_jobs().to_string()),
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
        let sandbox = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?
            .sandbox_id;
        // The terminal people see is redacted of agent secrets (SPEC-002 ADR-20).
        let scrub = crate::vault::TerminalScrub::default();
        let attached = self
            .deps
            .session_host
            .attach(Path::new(&attempt.run_dir), host_token)
            .await?;
        let welcome = attached.welcome.clone();
        let screen = self
            .terminal_snapshot(
                &scrub,
                &sandbox,
                welcome.snapshot.as_deref().unwrap_or_default(),
            )
            .await;
        runtime
            .hub
            .reset(welcome.seq, &screen, welcome.cols, welcome.rows);
        *runtime.run.lock().await = Some(ActiveRun {
            attempt_id: attempt.id.clone(),
            writer: attached.writer,
        });
        // A viewer who took the size while the run was starting (the PTY spawned at the size before
        // it): the terminal takes that viewer's size now (TSK-006, F 2026-10-10).
        if let Some((cols, rows)) = runtime.hub.controller_size()
            && (cols, rows) != (welcome.cols, welcome.rows)
            && let Err(err) = self.resize_run(task_id, cols, rows).await
        {
            tracing::debug!(task = %task_id, error = %err, "viewer size after attach failed");
        }
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
                        app.terminal_output(&scrub, &tid, &sandbox, seq, &bytes)
                            .await;
                    }
                    Ok(Some(RunEvent::Snapshot { seq, bytes })) => {
                        let (cols, rows) = app.runtime(&tid).hub.size();
                        let screen = app.terminal_snapshot(&scrub, &sandbox, &bytes).await;
                        app.runtime(&tid).hub.reset(seq, &screen, cols, rows);
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
                                    let screen = app
                                        .terminal_snapshot(
                                            &scrub,
                                            &sandbox,
                                            again.welcome.snapshot.as_deref().unwrap_or_default(),
                                        )
                                        .await;
                                    let rt = app.runtime(&tid);
                                    rt.hub.reset(
                                        again.welcome.seq,
                                        &screen,
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
        self.end_pause(task_id, "ended").await;
        if let Some(rt) = self.existing_runtime(task_id) {
            rt.watcher_alive.store(false, Ordering::SeqCst);
            *rt.run.lock().await = None;
            rt.hub.exit(TaskStatus::Stopped(reason).as_str(), None);
        }
        self.publish_task(task_id).await;
        self.advance_chains(task_id).await;
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
        let session_end = runtime.session_end.lock().ok().and_then(|mut g| g.take());
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
                (Some(EndIntent::Close), _) => "closed".into(),
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
            Some(EndIntent::Close) => {
                // The task was finished: whatever the exit code of the ended session, it is
                // closed, like a session the user left.
                if let Ok(Some(task)) = store.task(task_id).await
                    && let Ok(next) = task.status.apply(TaskEvent::Exited { normal: true })
                {
                    let _ = store
                        .transition_task(task_id, Some(task.status), next, None, now)
                        .await;
                }
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
                    let event = exit_event(&exit, session_end.as_deref());
                    if let Ok(next) = task.status.apply(event) {
                        let _ = store
                            .transition_task(task_id, Some(task.status), next, None, now)
                            .await;
                    }
                }
            }
        }
        // A paused run that ended (the user left the session, or it was stopped) is no longer
        // paused.
        self.end_pause(task_id, "ended").await;
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
        // Tasks chained after this one start (it is Done) or need a person (it ended).
        self.advance_chains(task_id).await;
        self.drain_queue().await;
    }

    /// Stores a run's metrics: while it goes (live, #31) and once more when it ends. The store
    /// upserts by attempt, so the last save wins and nothing is counted twice.
    async fn save_metrics(&self, task_id: &TaskId, attempt_id: &AttemptId, runtime: &TaskRuntime) {
        let snapshot = runtime.usage.lock().map(|u| u.clone()).unwrap_or_default();
        let active = runtime.active_ms_at(self.now());
        let metrics = MetricsRecord {
            model: snapshot.model,
            input_tokens: snapshot.tokens.input,
            output_tokens: snapshot.tokens.output,
            cache_read: snapshot.tokens.cache_read,
            cache_write: snapshot.tokens.cache_write,
            turns: u32::try_from(snapshot.turns).ok().filter(|t| *t > 0),
            active_ms: u64::try_from(active).ok().filter(|ms| *ms > 0),
        };
        let _ = self
            .deps
            .store
            .put_metrics(attempt_id, task_id, &metrics, self.now())
            .await;
    }

    /// Starts queued tasks while capacity allows (FIFO); none while an update pauses tasks.
    pub async fn drain_queue(self: &Arc<Self>) {
        if self.update_holds_starts() {
            return;
        }
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

    /// Closes a finished, running task (TSK-014): its session is shut down gracefully and the
    /// task ends `closed`, not stopped. It stays Running until the session is gone.
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn close_task_now(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        let task = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if task.status != TaskStatus::Running {
            return Err(AppError::Conflict(format!(
                "task is {}",
                task.status.as_str()
            )));
        }
        let runtime = self.runtime(task_id);
        if let Ok(mut intent) = runtime.intent.lock() {
            *intent = Some(EndIntent::Close);
        }
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
                tracing::warn!(task = %tid, "closed run did not exit after escalation; terminating host");
                let _ = app.deps.session_host.terminate(Path::new(&attempt.run_dir));
                app.mark_lost(&tid, &attempt.id, StopReason::Lost).await;
            }
        });
        Ok(())
    }

    /// Closes a finished, running task like [`Self::close_task_now`] and waits until its run has
    /// ended and the task rests (SPEC-039), so its records and directories can go. A run that
    /// already ended by itself is not a conflict. The close and the wait are bounded together by
    /// the close escalation, after which the run is terminated and marked lost, plus
    /// `CLOSE_SETTLE_MS`; the wait follows the task's update events and looks again every
    /// `CLOSE_RECHECK` in case an update is never published.
    ///
    /// # Errors
    /// `Conflict` when the run is still there at the bound.
    pub async fn close_task_and_wait(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        // Subscribed before closing, so the update that says the run ended is never missed.
        let mut events = self.events.subscribe();
        let total =
            STOP_INTERRUPT_GRACE_MS + STOP_TERM_GRACE_MS + STOP_KILL_WAIT_MS + CLOSE_SETTLE_MS;
        let bound = Duration::from_millis(u64::try_from(total).unwrap_or(25_000));
        let ended = tokio::time::timeout(bound, async {
            match self.close_task_now(task_id).await {
                // Not running any more: whether it rests is checked below.
                Ok(()) | Err(AppError::Conflict(_)) => {}
                Err(err) => return Err(err),
            }
            loop {
                let store = &self.deps.store;
                let rests = store
                    .task(task_id)
                    .await?
                    .is_none_or(|t| t.status.is_resting());
                if rests && store.live_attempt(task_id).await?.is_none() {
                    return Ok::<bool, AppError>(true);
                }
                loop {
                    match tokio::time::timeout(CLOSE_RECHECK, events.recv()).await {
                        Ok(Ok(envelope)) => {
                            if matches!(&envelope.event, UiEvent::TaskUpdated { task }
                                if task.id == task_id.as_str())
                            {
                                break;
                            }
                        }
                        // No update for a while, or updates were missed: look again.
                        Err(_) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => break,
                        Ok(Err(broadcast::error::RecvError::Closed)) => return Ok(false),
                    }
                }
            }
        })
        .await;
        match ended {
            Ok(Ok(true)) => Ok(()),
            Ok(Err(err)) => Err(err),
            Ok(Ok(false)) | Err(_) => Err(AppError::Conflict(
                "the task's session is still closing; try again".into(),
            )),
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
            || task.status == TaskStatus::Chained
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
            self.end_pause(task_id, "cancelled").await;
            self.publish_task(task_id).await;
            self.advance_chains(task_id).await;
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
        self.end_pause(task_id, "cancelled").await;
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
        self.switch_with(task_id, account, reason, RunStart::Resume { prompt })
            .await
    }

    /// Switches a task whose turn a usage limit cut (SPEC-034): the session resumes on `account`
    /// (the policy's choice when `None`) past the limit, with nothing typed or sent.
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn switch_after_limit(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
    ) -> AppResult<()> {
        self.switch_with(task_id, account, reason, RunStart::AfterLimit)
            .await
    }

    async fn switch_with(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
        start: RunStart,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let attempt = store.live_attempt(task_id).await?;
        let next = task
            .status
            .apply(TaskEvent::SwitchStarted)
            .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?;
        // The new session must be able to start before the running one ends (ROT-014): a
        // refusal leaves the task as it is.
        if let Err(err) = self.preflight_start(&task, account.as_ref()).await {
            self.refuse_switch(&task, account.as_ref(), reason, &err)
                .await;
            return Err(err);
        }
        if !store
            .transition_task(task_id, Some(task.status), next, None, self.now())
            .await?
        {
            return Err(AppError::Conflict("task state changed concurrently".into()));
        }
        // A paused task that moves to another subscription is no longer paused.
        self.end_pause(task_id, "switched").await;
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
                from_desc,
                to_desc,
                outcome: "started".into(),
            })
            .await?;
        let runtime = self.runtime(task_id);
        // The subscription changes under the hood (F 2026-10-10): viewers see that the session
        // continues, not which accounts are involved (Task details lists the switches).
        // The policy may pick the same subscription (a limit that reset): no "another" then.
        let elsewhere = account.is_some()
            && attempt.as_ref().map(|a| a.account_id.as_ref()) != Some(account.as_ref());
        runtime.hub.notice(if elsewhere {
            "↻ Yulius continues this session on another Claude subscription…"
        } else {
            "↻ Yulius continues this session…"
        });
        self.publish_task(task_id).await;
        if let Some(attempt) = attempt {
            let (tx, rx) = oneshot::channel();
            if let Ok(mut intent) = runtime.intent.lock() {
                *intent = Some(EndIntent::Switch(tx));
            }
            if let Some(run) = runtime.run.lock().await.as_mut() {
                // An automatic switch happens at a boundary (or a held hook, or a limit): the
                // agent is idle, so it is terminated without an interrupt that could start or
                // cancel anything. A person's switch interrupts first, so the transcript keeps
                // the interrupted turn.
                if reason != "manual" {
                    let _ = run
                        .writer
                        .signal(crate::ports::session::RunSignal::Terminate)
                        .await;
                }
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
        match self.start_run(task_id, start, account).await {
            Ok(()) => {
                // The switch found no subscription with capacity: the task paused instead.
                let paused = matches!(
                    store.task(task_id).await,
                    Ok(Some(t)) if t.status == TaskStatus::WaitingCapacity
                );
                if paused {
                    store.set_switch_outcome(&decision_id, "waiting").await?;
                } else {
                    store.set_switch_outcome(&decision_id, "completed").await?;
                    runtime.switch_refused_ms.store(0, Ordering::SeqCst);
                    runtime
                        .hub
                        .notice("↻ The session continues where it left off.");
                }
                Ok(())
            }
            Err(err) => {
                store.set_switch_outcome(&decision_id, "failed").await?;
                tracing::warn!(task = %task_id, error = %err, "the session could not continue after a switch");
                runtime.hub.notice(format!(
                    "↻ The session could not continue: {}",
                    err.public_message()
                ));
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

    /// What a launch would need, checked without changing anything: the repository, a usable
    /// subscription credential and the sandbox's isolation (disk quota, holder). A switch runs
    /// it before it ends the running session (ROT-014).
    ///
    /// # Errors
    /// The reason the launch would be refused.
    pub async fn preflight_start(
        &self,
        task: &TaskRecord,
        requested: Option<&AccountId>,
    ) -> AppResult<()> {
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let worktree = self
            .sandbox_dir(&sandbox.id)
            .join("worktrees")
            .join(task.id.as_str());
        if !worktree.join(".git").exists() && !self.sandbox_repo(&sandbox.id).join(".git").exists()
        {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        // No subscription with capacity is not a refusal: the switch pauses the task instead.
        if let Some(account) = self.choose_account(task, requested).await? {
            self.open_credential(&account)?;
        }
        self.isolation_join(&sandbox).await?;
        Ok(())
    }

    /// Whether a switch of `task` was refused so recently that an automatic one waits.
    pub(crate) fn switch_cooling(&self, task: &TaskId) -> bool {
        let at = self.runtime(task).switch_refused_ms.load(Ordering::SeqCst);
        at > 0 && self.now().0 - at < SWITCH_COOLDOWN_MS
    }

    /// Records and shows a switch that was refused before it ended the running session.
    async fn refuse_switch(
        &self,
        task: &TaskRecord,
        account: Option<&AccountId>,
        reason: &str,
        err: &AppError,
    ) {
        let message = err.public_message();
        // A repeat of an automatic switch inside the cooldown is only logged: one record and
        // one notice per cooldown, not one per retry.
        let repeat = reason != "manual" && self.switch_cooling(&task.id);
        self.runtime(&task.id)
            .switch_refused_ms
            .store(self.now().0, Ordering::SeqCst);
        tracing::warn!(task = %task.id, error = %err, "switch refused; the running session stays");
        if repeat {
            return;
        }
        self.runtime(&task.id).hub.notice(format!(
            "↻ Switching subscriptions was not possible, so this session stays as it is: {message}"
        ));
        let record = SwitchDecisionRecord {
            id: self.deps.crypto.new_id(),
            task_id: task.id.clone(),
            at: self.now(),
            reason: reason.to_owned(),
            inputs: serde_json::json!({
                "account": account.map(AccountId::as_str),
                "refused": message,
            })
            .to_string(),
            from_desc: self.account_desc(task.harness, None).await,
            to_desc: self.account_desc(task.harness, account).await,
            outcome: "refused".into(),
        };
        if let Err(err) = self.deps.store.insert_switch(&record).await {
            tracing::warn!(task = %task.id, error = %err, "the refused switch was not recorded");
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
            // The run's transcript so far is history: its watcher catches up on it (TSK-014).
            self.runtime(&attempt.task_id)
                .replay_transcript
                .store(true, Ordering::SeqCst);
            let reattached = match host_token {
                Some(token) => self
                    .attach_run(&attempt.task_id, &attempt, &token)
                    .await
                    .is_ok(),
                None => false,
            };
            if !reattached {
                self.runtime(&attempt.task_id)
                    .replay_transcript
                    .store(false, Ordering::SeqCst);
            }
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
        // New usage may let paused tasks resume, or pause the pool (ROT-010).
        self.capacity_changed();
        // At or above the threshold: tasks on this account rotate at their next turn boundary
        // (ROT-003); with hooks the Stop hook decides this itself.
        if usage.exhausted(self.now(), rotation::THRESHOLD)
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
        runtime.hooks_seen.store(false, Ordering::SeqCst);
        if let Ok(mut u) = runtime.usage.lock() {
            *u = UsageSnapshot::default();
        }
        runtime.turn_started_ms.store(0, Ordering::SeqCst);
        runtime.active_ms.store(0, Ordering::SeqCst);
        // A new attempt (or the same one after a restart) learns the goal from its transcript.
        runtime.goal_achieved.store(false, Ordering::SeqCst);
        runtime.goal_submitted.store(false, Ordering::SeqCst);
        runtime.session_started.store(false, Ordering::SeqCst);
        runtime.background_shells.store(-1, Ordering::SeqCst);
        if let Ok(mut end) = runtime.session_end.lock() {
            *end = None;
        }
        runtime.hold_polled_ms.store(0, Ordering::SeqCst);
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
        let mut saved: Option<UsageSnapshot> = None;
        let mut saved_at = Timestamp(0);
        // After a restart, the transcript the reattached run already wrote is history until the
        // first poll reaches its end: it is reduced to its final state, not replayed event by
        // event (TSK-014; a replayed prompt used to put a finished task back to Working after
        // every restart). A run started by this server is watched live from its first line.
        let mut replay = self
            .runtime(&task_id)
            .replay_transcript
            .swap(false, Ordering::SeqCst);
        let mut catching_up = false;
        let mut facts = TranscriptFacts::default();
        let mut probed_at = Timestamp(0);
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
                catching_up = replay && watch.is_some();
                replay &= watch.is_none();
                facts = TranscriptFacts::default();
            }
            // Background shells of an idle task, at most every SHELL_PROBE_MS (TSK-014).
            if now.millis_since(probed_at) >= SHELL_PROBE_MS {
                probed_at = now;
                self.probe_idle_shells(&task_id).await;
            }
            let Some(w) = watch.as_mut() else { continue };
            let observations = match w.poll() {
                Ok(o) => o,
                Err(err) => {
                    tracing::debug!(error = %err, "transcript poll failed");
                    continue;
                }
            };
            for observation in &observations {
                facts.fold(observation);
            }
            let usage = w.usage();
            let runtime = self.runtime(&task_id);
            if let Ok(mut u) = runtime.usage.lock() {
                *u = usage.clone();
            }
            let turn_ended = observations
                .iter()
                .any(|o| matches!(o, Observation::TurnComplete));
            if catching_up {
                // Usage limits keep their immediate effect; turns and the goal are applied once,
                // as the replay's final state.
                for observation in observations {
                    if matches!(observation, Observation::LimitReached { .. }) {
                        self.on_observation(&task_id, &attempt, observation).await;
                    }
                }
                if w.caught_up() {
                    catching_up = false;
                    self.apply_transcript_facts(&task_id, facts).await;
                }
            } else {
                for observation in observations {
                    self.on_observation(&task_id, &attempt, observation).await;
                }
            }
            // Live metrics (#31): the dashboard counts a run while it goes, saved after each turn
            // and at most every 30 s while something changes.
            let now = self.now();
            let working = runtime.turn_started_ms.load(Ordering::SeqCst) > 0;
            if (saved.as_ref() != Some(&usage) || working)
                && (turn_ended || now.millis_since(saved_at) >= LIVE_METRICS_MS)
            {
                self.save_metrics(&task_id, &attempt.id, &runtime).await;
                saved = Some(usage);
                saved_at = now;
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
            Observation::LimitReached {
                resets_at,
                message,
                window_minutes,
            } => {
                tracing::info!(task = %task_id, %message, "usage limit reached");
                runtime.end_turn(self.now());
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
                    // What the CLI was told is the account's state (ROT-009): the limited window
                    // full until its reset, even when the provider's usage request does not
                    // answer.
                    let full = |minutes: u32| UsageWindow {
                        used_percent: Some(100.0),
                        window_minutes: Some(minutes),
                        resets_at,
                    };
                    match window_minutes {
                        Some(minutes) if minutes <= SESSION_WINDOW_MINUTES => {
                            usage.primary = Some(full(minutes));
                        }
                        Some(minutes) => usage.secondary = Some(full(minutes)),
                        None => {}
                    }
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
            Observation::UserTurn { at } => {
                // A new prompt: an achieved goal no longer makes the task done (TSK-014).
                runtime.goal_achieved.store(false, Ordering::SeqCst);
                // A prompt of a turn whose clean Stop already came is not a new turn.
                if at.is_some_and(|at| at.0 <= runtime.last_stop_ms.load(Ordering::SeqCst)) {
                    return;
                }
                // A prompt reached the agent while its turn or prompt was held for capacity:
                // the CLI gave the hold up (an interrupt, its own hook timeout), so the task
                // runs again (F 2026-10-10: the task follows its session).
                self.hold_overtaken(task_id, at, Activity::Working).await;
                runtime.start_turn(self.now());
                let _ = self
                    .deps
                    .store
                    .set_activity(task_id, Some(Activity::Working), self.now())
                    .await;
                self.publish_task(task_id).await;
            }
            Observation::TurnComplete => {
                runtime.end_turn(self.now());
                // With hooks the Stop hook is the boundary that switches or pauses; the
                // transcript decides only for a run without them.
                if !runtime.hooks_seen.load(Ordering::SeqCst) {
                    self.turn_ended(task_id, attempt.account_id.clone());
                }
            }
            Observation::TurnInterrupted { at } => {
                // Esc or Ctrl+C stopped the turn: no Stop hook runs, the agent waits for input.
                runtime.end_turn(self.now());
                self.turn_interrupted(task_id, at).await;
            }
            Observation::Goal(outcome) => self.goal_observed(task_id, outcome).await,
            Observation::Session { .. } | Observation::TurnEnded => {}
        }
    }

    /// Applies the pool policy for a running task and acts on the decision (SPEC-001 §7.4):
    /// stay, move to another subscription by native resume, or pause (ROT-010).
    pub async fn rotate(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
    ) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let limit = matches!(trigger, Trigger::LimitReached);
        // A limit can also end a turn that is already held at its boundary.
        if task.status != TaskStatus::Running
            && !(limit && task.status == TaskStatus::WaitingCapacity)
        {
            return;
        }
        let Ok(input) = self.rotation_input(&task, current.clone()).await else {
            return;
        };
        let (decision, reason) = rotation::decide(&input, trigger);
        self.runtime(task_id)
            .proactive_switch
            .store(false, Ordering::SeqCst);
        match decision {
            Decision::Stay => {}
            Decision::Switch { account } => {
                // A turn the limit cut goes on past it, with nothing typed (SPEC-034).
                let reason = format!("{reason:?}");
                let switched = if limit {
                    self.switch_after_limit(task_id, Some(account), &reason)
                        .await
                } else {
                    self.switch_task_now(task_id, Some(account), &reason, None)
                        .await
                };
                if let Err(err) = switched {
                    tracing::warn!(error = %err, task = %task_id, "automatic switch failed");
                }
            }
            Decision::Wait { .. } => {
                let kind = if limit {
                    PauseKind::Limit
                } else {
                    PauseKind::Turn
                };
                if let Err(err) = self.pause_task(task_id, kind, current, None).await {
                    tracing::warn!(error = %err, task = %task_id, "could not pause the task");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_typed_first_message_pastes_each_image_then_the_text_ending_in_a_newline() {
        let pastes = super::opening_pastes(
            vec!["/s/a.png".into(), "/s/my b.png".into()],
            "Look\n\n@\"/s/c.txt\"",
        );
        let shown: Vec<(String, Option<String>)> = pastes
            .into_iter()
            .map(|(bytes, shown)| (String::from_utf8_lossy(&bytes).into_owned(), shown))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "\x1b[200~/s/a.png\x1b[201~".to_owned(),
                    Some("[Image #1]".to_owned())
                ),
                (
                    "\x1b[200~/s/my b.png\x1b[201~".to_owned(),
                    Some("[Image #2]".to_owned())
                ),
                ("\x1b[200~Look\n\n@\"/s/c.txt\"\n\x1b[201~".to_owned(), None),
            ]
        );
        assert_eq!(super::opening_pastes(vec!["/s/a.png".into()], "").len(), 1);
    }
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

    /// F 2026-10-10: every way a session ends maps to one task event. Leaving the session
    /// (`SessionEnd` `prompt_input_exit`/`logout`) is a normal close whatever the exit code; a
    /// signal from outside (or Claude Code's 128 + signal exit) is a kill; the rest is a crash.
    #[test]
    fn session_ends_map_to_one_task_event() {
        use yulius_domain::task::TaskEvent;
        let exit = |code: Option<i32>, signal: Option<i32>| crate::ports::session::RunExit {
            code,
            signal,
            spawn_failed: false,
        };
        let normal = TaskEvent::Exited { normal: true };
        let crashed = TaskEvent::Exited { normal: false };
        assert_eq!(super::exit_event(&exit(Some(0), None), None), normal);
        assert_eq!(
            super::exit_event(&exit(Some(1), None), Some("prompt_input_exit")),
            normal
        );
        assert_eq!(
            super::exit_event(&exit(Some(1), None), Some("logout")),
            normal
        );
        assert_eq!(super::exit_event(&exit(Some(3), None), None), crashed);
        assert_eq!(
            super::exit_event(&exit(Some(3), None), Some("other")),
            crashed
        );
        assert_eq!(super::exit_event(&exit(None, Some(11)), None), crashed);
        for killed in [
            exit(Some(143), None),
            exit(Some(137), None),
            exit(Some(129), None),
        ] {
            assert_eq!(super::exit_event(&killed, Some("other")), TaskEvent::Killed);
        }
        assert_eq!(
            super::exit_event(&exit(None, Some(9)), None),
            TaskEvent::Killed
        );
        assert_eq!(
            super::exit_event(&exit(None, Some(15)), None),
            TaskEvent::Killed
        );
        let spawn = crate::ports::session::RunExit {
            code: Some(0),
            signal: None,
            spawn_failed: true,
        };
        assert_eq!(
            super::exit_event(&spawn, Some("prompt_input_exit")),
            crashed
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
