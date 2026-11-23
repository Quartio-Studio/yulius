//! The application object: configuration, injected ports and shared runtime state.
//!
//! Use cases are `impl App` blocks in the area modules; the interface layer holds `Arc<App>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use yulius_domain::ids::{SandboxId, TaskId};

use crate::events::EventBus;
use crate::ports::git::GitPort;
use crate::ports::github::GitHubPort;
use crate::ports::harness::HarnessPort;
use crate::ports::session::SessionHost;
use crate::ports::store::Store;
use crate::ports::system::SystemProbe;
use crate::ports::{Clock, Crypto};

/// Static configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Data directory (SPEC-002 §4).
    pub data_dir: PathBuf,
    /// Public base URL (no trailing slash).
    pub public_url: String,
    /// Internal loopback base URL for MCP, hooks and git credentials.
    pub internal_url: String,
    /// Path of the running `yulius` executable (used for ptyhost, hooks, credential helper).
    pub yulius_exe: PathBuf,
    /// Whether sandboxes run in OS containment.
    pub contained: bool,
    /// Whether a server start brings onboarded sandboxes to the current Infra bootstrap revision
    /// with an upgrade task each (SPEC-575 MFS-012); the server's own setting, off in tests.
    pub bootstrap_upgrades: bool,
    /// Maximum concurrently live sessions.
    pub max_live_sessions: u32,
    /// How long a task whose turn ended and that waits for a person keeps its idle session
    /// before it is parked (SPEC-512 PRK-002, `YULIUS_PARK_AFTER_MINUTES`); 0: never.
    pub park_after_ms: u64,
    /// Budget for one parked task's PR status read.
    pub parked_pr_read_ms: u64,
    /// Maximum queued tasks.
    pub queue_max: u32,
    /// Web session lifetime.
    pub session_ttl_ms: i64,
    /// Permission cache freshness.
    pub permission_ttl_ms: i64,
    /// Accept stale permissions on GitHub failure up to this age.
    pub permission_stale_ms: i64,
    /// How long an idle task's strays (processes its sandbox init adopted) live once seen before
    /// they are killed (SPEC-353 LAP-004).
    pub stray_reap_after_ms: i64,
    /// Cookies carry `Secure`.
    pub secure_cookies: bool,
    /// How often open terminal viewers re-check their access (ACC-008).
    pub viewer_recheck_ms: u64,
    /// How often the UI event stream says it is alive with a heartbeat (SPEC-191 LIV-001).
    pub events_heartbeat_ms: u64,
    /// How often a person's pause asks the kernel again whether the run froze (SPEC-191 PER-001).
    pub person_freeze_pass_ms: u64,
    /// Wait between two resumes of tasks paused for lack of subscription capacity (ROT-010).
    pub resume_stagger_ms: u64,
    /// The rebalance threshold of each harness (SPEC-629 RBL-001,
    /// `YULIUS_REBALANCE_PERCENT_<HARNESS>`).
    pub rebalance: yulius_domain::rebalance::Thresholds,
    /// How long a stopped or closed run gets to exit (interrupt, TERM, kill) before its host is
    /// terminated (SPEC-065 END-008).
    pub stop_escalation_ms: u64,
    /// How long a switch waits for the session it asked to end before that session's host is
    /// terminated.
    pub switch_end_ms: u64,
    /// How long a sandbox shell or the Superagent asked to end gets to exit before its host is
    /// terminated ([`crate::shells::SHELL_EXIT_WAIT`], SPEC-146 SHE-001). Tests shorten it.
    pub shell_exit_ms: u64,
    /// CPUs an agent session, terminal or Cockpit plans for (`YULIUS_AGENT_CPUS`, SPEC-082
    /// RES-005); `None`: a third of the sandbox's.
    pub agent_cpus: Option<u32>,
    /// How long a goal session's TUI gets to become ready before the goal is typed anyway
    /// ([`crate::supervisor::GOAL_READY_TIMEOUT`]; a TUI without hooks gets half). Tests of the
    /// hookless path shorten it.
    pub goal_ready_ms: u64,
    /// How long a person gets to answer Claude Code's folder-trust prompt before the task is
    /// flagged ([`crate::supervisor::TRUST_ANSWER_TIMEOUT`], SPEC-106 OTR-002). Tests shorten it.
    pub trust_answer_ms: u64,
    /// How long the compaction of a full session waits for a person's possible draft to leave
    /// the prompt box before it is typed all the same
    /// ([`crate::context_full::COMPACT_DRAFT_HOLD`], SPEC-483 TYP-001). Tests shorten it.
    pub compact_hold_ms: u64,
    /// How long Claude Code gets to record a goal given as the launch prompt once its session
    /// reads input, before the task is flagged (the sum of the typed goal's submit waits,
    /// SPEC-117 ONE-001). Tests shorten it.
    pub launch_goal_ms: u64,
    /// How long a session restarted on a newer Claude Code gets to start there (its
    /// SessionStart hook) before that version gets no other running session (SPEC-461 CCM-013).
    /// Tests shorten it.
    pub claude_code_start_ms: u64,
    /// How long a turn's end may take to decide whether its session restarts on a newer Claude
    /// Code (SPEC-461 CCM-013): the decision is made inside the Stop hook's request, whose
    /// client gives up after 5 s and lets the session work on, so one that would take longer
    /// (the turn's lines not read yet, a slow look at the session's processes) is left to a
    /// later turn's end. Tests shorten it.
    pub claude_code_decision_ms: u64,
    /// How long after a prompt from a session's prompt box went on its record may still be
    /// missing from the transcript: Claude Code writes where the prompt went (a turn of its
    /// own, or its queue) once the prompt's hook returned, and the record reaches the file
    /// about 100 ms later (67 to 108 ms in the runs of 2.1.322). A turn's end within this time
    /// leaves the session where it is instead of restarting it on a newer Claude Code
    /// (SPEC-471 CCQ-004). Two seconds: some twenty times that write, for a loaded host; a
    /// turn that ends sooner after its prompt moves one turn later. Tests whose agent ends a
    /// turn milliseconds after its prompt set it to none.
    pub claude_code_queue_record_ms: u64,
    /// How long the Superagent's request to start a task in another sandbox waits for its
    /// person's answer ([`crate::cross_sandbox::CROSS_REQUEST_WAIT`], SPEC-335 XSR-003). Tests
    /// shorten it.
    pub cross_request_wait_ms: u64,
    /// How long a call waits for a brokered run before it answers its state
    /// ([`crate::commands_run::RUN_WAIT`], SPEC-626 BRK-014). Tests change it.
    pub command_wait_ms: u64,
    /// A minute of a brokered command's `approval_minutes` (SPEC-626 BRK-014): 60 000; tests
    /// shorten it.
    pub command_minute_ms: u64,
    /// Extra host paths visible read-only inside containment (for example agent CLI install
    /// directories outside `/usr` and `/opt`).
    pub contained_readonly: Vec<PathBuf>,
    /// Additional frontend origins (for example a desktop app) allowed for WebSockets and CORS
    /// besides the public URL (OPS-002, AC-OPS-2).
    pub allowed_origins: Vec<String>,
    /// Live preview origins of sandbox ports (BRW-005, SPEC-002 ADR-14); by default subdomains
    /// of the public URL's host, `None` when that host is an IP address or previews are off.
    pub preview: Option<crate::preview::PreviewConfig>,
    /// Loopback ports of Yulius's own listeners: never previewed, never opened by the sandbox
    /// browser and never listed on its welcome page (no recursion, BRW-008).
    pub own_ports: Vec<u16>,
    /// Largest file one upload may carry (SPEC-001 EDT-006, UI-048; `YULIUS_UPLOAD_MAX_MB`).
    pub upload_max_bytes: u64,
    /// Yulius-run CI (SPEC-001 DLV-006, SPEC-002 ADR-21).
    pub ci: crate::ci::CiSettings,
    /// Script watches (SPEC-630 WCH-003, WCH-009).
    pub watches: crate::watches::WatchSettings,
    /// The TLS reverse proxy's shared secret (`YULIUS_PROXY_SECRET_FILE`, SPEC-001 ACC-013): a
    /// request carrying it is keyed on the client address the proxy passes; `None` keys every
    /// request on its TCP peer. Formatting never shows it.
    pub proxy_secret: Option<crate::model::Secret>,
    /// Webhook signing key set by the owner of an existing GitHub App.
    pub github_webhook_secret: Option<crate::model::Secret>,
}

impl AppConfig {
    /// Defaults for a data directory and public URL.
    #[must_use]
    pub fn new(
        data_dir: PathBuf,
        public_url: &str,
        internal_url: &str,
        yulius_exe: PathBuf,
    ) -> Self {
        let public_url = public_url.trim_end_matches('/').to_owned();
        let own_ports = [public_url.as_str(), internal_url]
            .iter()
            .filter_map(|url| {
                let authority = url.split_once("://")?.1.split('/').next()?;
                let (host, port) = authority.rsplit_once(':')?;
                let loopback = host == "localhost" || host.starts_with("127.") || host == "[::1]";
                loopback.then(|| port.parse::<u16>().ok()).flatten()
            })
            .collect();
        Self {
            preview: crate::preview::PreviewConfig::from_public_url(&public_url),
            own_ports,
            data_dir,
            secure_cookies: public_url.starts_with("https://"),
            viewer_recheck_ms: 60_000,
            events_heartbeat_ms: 20_000,
            person_freeze_pass_ms: 2_000,
            resume_stagger_ms: crate::capacity::RESUME_STAGGER_MS,
            rebalance: yulius_domain::rebalance::Thresholds::default(),
            stop_escalation_ms: u64::try_from(
                yulius_domain::task::STOP_INTERRUPT_GRACE_MS
                    + yulius_domain::task::STOP_TERM_GRACE_MS
                    + yulius_domain::task::STOP_KILL_WAIT_MS,
            )
            .unwrap_or(20_000),
            switch_end_ms: u64::try_from(yulius_domain::task::SWITCH_TIMEOUT_MS / 2)
                .unwrap_or(60_000),
            shell_exit_ms: u64::try_from(crate::shells::SHELL_EXIT_WAIT.as_millis())
                .unwrap_or(10_000),
            goal_ready_ms: u64::try_from(crate::supervisor::GOAL_READY_TIMEOUT.as_millis())
                .unwrap_or(90_000),
            compact_hold_ms: u64::try_from(crate::context_full::COMPACT_DRAFT_HOLD.as_millis())
                .unwrap_or(600_000),
            trust_answer_ms: u64::try_from(crate::supervisor::TRUST_ANSWER_TIMEOUT.as_millis())
                .unwrap_or(300_000),
            launch_goal_ms: crate::supervisor::launch_goal_bound_ms(),
            claude_code_start_ms: 120_000,
            claude_code_decision_ms: 3_500,
            claude_code_queue_record_ms: 2_000,
            cross_request_wait_ms: u64::try_from(
                crate::cross_sandbox::CROSS_REQUEST_WAIT.as_millis(),
            )
            .unwrap_or(240_000),
            command_wait_ms: u64::try_from(crate::commands_run::RUN_WAIT.as_millis())
                .unwrap_or(240_000),
            command_minute_ms: 60_000,
            agent_cpus: None,
            contained_readonly: Vec::new(),
            allowed_origins: Vec::new(),
            public_url,
            internal_url: internal_url.trim_end_matches('/').to_owned(),
            yulius_exe,
            contained: false,
            bootstrap_upgrades: false,
            max_live_sessions: 8,
            park_after_ms: crate::parking::DEFAULT_PARK_AFTER_MS,
            parked_pr_read_ms: 750,
            queue_max: 32,
            session_ttl_ms: 30 * 24 * 3600 * 1000,
            permission_ttl_ms: 5 * 60 * 1000,
            permission_stale_ms: 30 * 60 * 1000,
            stray_reap_after_ms: crate::finish::STRAY_REAP_AFTER_MS,
            upload_max_bytes: crate::uploads::DEFAULT_UPLOAD_MAX_BYTES,
            ci: crate::ci::CiSettings::default(),
            watches: crate::watches::WatchSettings::default(),
            proxy_secret: None,
            github_webhook_secret: None,
        }
    }
}

/// Injected adapters.
#[derive(Clone)]
pub struct Deps {
    /// Persistence.
    pub store: Arc<dyn Store>,
    /// Clock.
    pub clock: Arc<dyn Clock>,
    /// Crypto.
    pub crypto: Arc<dyn Crypto>,
    /// GitHub.
    pub github: Arc<dyn GitHubPort>,
    /// Detached process hosts.
    pub session_host: Arc<dyn SessionHost>,
    /// Claude Code adapter.
    pub harness: Arc<dyn HarnessPort>,
    /// Git.
    pub git: Arc<dyn GitPort>,
    /// Machine metrics for the status panel.
    pub system: Arc<dyn SystemProbe>,
    /// Per-sandbox shared browser (SPEC-001 BRW-001..004).
    pub browser: Arc<dyn crate::ports::browser::BrowserPort>,
    /// Provider usage windows of accounts (ROT-009).
    pub usage: Arc<dyn crate::ports::usage::UsageProbe>,
}

/// The application.
pub struct App {
    /// Configuration.
    pub config: AppConfig,
    /// Adapters.
    pub deps: Deps,
    /// UI event bus.
    pub events: EventBus,
    /// Password-login attempts per client key (rate limiting).
    pub(crate) login_attempts: Mutex<HashMap<String, Vec<i64>>>,
    /// CLI device sign-in starts per client key (rate limiting, #169).
    pub(crate) device_starts: Mutex<HashMap<String, Vec<i64>>>,
    /// Where each watched transcript's reads go on (SPEC-483 slice 36): a poll reads only what
    /// was written since; bounded by [`crate::session_docs::MAX_TRANSCRIPT_CURSORS`].
    pub(crate) transcript_cursors:
        Mutex<HashMap<std::path::PathBuf, crate::transcripts::ReadCursor>>,
    /// GitHub sign-in starts per client key in the last minute (SPEC-483 A4).
    pub(crate) github_starts: Mutex<HashMap<String, Vec<i64>>>,
    /// Looks at and joins through playground guest links per client key (SPEC-410 GST-002).
    pub(crate) guest_attempts: Mutex<HashMap<String, Vec<i64>>>,
    /// Remote MCP requests per API token in the last minute (SPEC-247 RMT-014).
    pub(crate) remote_calls: Mutex<HashMap<String, Vec<i64>>>,
    /// Remote file changes being committed at once (SPEC-247 RMT-025).
    pub(crate) change_slots: tokio::sync::Semaphore,
    /// At most 16 webhook scheduling attempts, including waits for a sandbox sync.
    pub(crate) webhook_slots: Arc<tokio::sync::Semaphore>,
    /// Notepad presence updates per person and notepad (rate limiting, SPEC-112 NOT-019).
    pub(crate) note_presence: Mutex<HashMap<String, Vec<i64>>>,
    /// Ids held by browser proxies and their connections while they can reach a cached port range.
    pub(crate) browser_port_leases:
        Mutex<HashMap<yulius_domain::ids::SandboxId, std::sync::Weak<str>>>,
    /// The Editor's shared texts (SPEC-411 EDT-018), in memory only.
    pub(crate) live_docs: crate::live_files::LiveDocs,
    /// Session supervisor runtime state.
    pub(crate) supervisor: crate::supervisor::SupervisorState,
    /// Account login terminals.
    pub(crate) logins: crate::accounts::LoginRegistry,
    /// The subscription pool's resume loop (SPEC-002 ADR-17).
    pub(crate) capacity: crate::capacity::CapacityState,
    /// Sandbox shell sessions (SPEC-001 UI-015).
    pub(crate) shells: crate::shells::ShellState,
    /// Workspace tokens of user processes (SPEC-002 ADR-12).
    pub(crate) workspace_grants: crate::identity::WorkspaceGrants,
    /// Serializes recovery-code checks so parallel guesses cannot exceed the limit (ADR-13).
    pub(crate) recovery_lock: tokio::sync::Mutex<()>,
    /// Serializes Enable on ready-made automations so two presses never create two (SPEC-042).
    pub(crate) automation_template_lock: tokio::sync::Mutex<()>,
    /// Serializes creating a sandbox's onboarding task so Start onboarding and a waiting new task
    /// never create two (SPEC-097).
    pub(crate) onboarding_task_lock: tokio::sync::Mutex<()>,
    /// Serializes giving an empty repository its first commit, so two starts never share its
    /// scratch clone or push twice (SPEC-241).
    pub(crate) repository_seed_lock: tokio::sync::Mutex<()>,
    /// Serializes changes of how waiting tasks start, so two never make tasks wait for each other
    /// (SPEC-126).
    pub(crate) start_change_lock: tokio::sync::Mutex<()>,
    /// Live preview handshakes and grants (SPEC-002 ADR-14).
    pub(crate) previews: crate::preview::PreviewState,
    /// Uploads in progress (SPEC-001 EDT-006, UI-048).
    pub(crate) uploads: crate::uploads::UploadState,
    /// Import files a chunk is being applied to, one at a time each (SPEC-400 VTV-001).
    pub(crate) import_chunks: crate::imports::ChunkLocks,
    /// Files of original prompts being read (SPEC-080).
    pub(crate) attachment_reads: crate::task_prompt::AttachmentReads,
    /// A random tag of this server process, sent with every response (`X-Yulius`): the preview
    /// proxy refuses an upstream that answers with it, i.e. Yulius itself (BRW-008).
    pub instance_tag: String,
    /// Task intake (Jev, split proposals); unset means nothing is checked (SPEC-002 ADR-16).
    pub(crate) intake: OnceLock<crate::intake::IntakeServices>,
    /// The instance Vidoc key (SPEC-195); unset means Vidoc is off.
    pub(crate) vidoc: OnceLock<crate::vidoc_key::VidocServices>,
    /// The personal notification channel (SPEC-498); unset means the channel is off.
    pub(crate) channel: OnceLock<crate::telegram::ChannelServices>,
    /// Serializes writes of sandbox profiles, so the Vidoc switch and a profile update never undo
    /// each other (SPEC-338 VOF-001).
    pub(crate) profile_writes: tokio::sync::Mutex<()>,
    /// Instance updates: checks of `main` and the update run (SPEC-002 ADR-18).
    pub(crate) updates: crate::updates::UpdateState,
    /// The root move helper's files (SPEC-470); unset: no move is asked from this server.
    pub(crate) move_helper: OnceLock<Arc<dyn crate::ports::migration::MoveHelper>>,
    /// One ask for a move at a time: what it finds and what it writes belong together.
    pub(crate) move_asks: tokio::sync::Mutex<()>,
    /// Woken whenever a brokered run changes state: calls waiting on one read it again
    /// (SPEC-626 BRK-014).
    pub(crate) command_runs_changed: tokio::sync::Notify,
    /// Rolls queued for the next harness draws ([`App::queue_harness_rolls`], tests); empty:
    /// each draw uses unbiased rejection sampling over random bytes (SPEC-520 RHM-001).
    pub(crate) harness_rolls: Mutex<std::collections::VecDeque<u8>>,
    /// Secrets vault runtime state (SPEC-002 ADR-20).
    pub(crate) vault: crate::vault::VaultState,
    /// Yulius-run CI and repository hygiene (SPEC-002 ADR-21).
    pub(crate) ci: crate::ci::CiState,
    /// The resource limits in force (SPEC-459): sessions, CI jobs and lanes, a CD run's disk.
    pub(crate) limits: crate::limits::LimitState,
    /// Sandbox isolation (SPEC-002 ADR-22); unset: contained runs keep the previous containment.
    pub(crate) isolation: OnceLock<Arc<dyn crate::ports::isolation::SandboxIsolation>>,
    /// The Claude Code sandboxes get (SPEC-461); unset: runs use the host's as it is.
    pub(crate) agent_cli: OnceLock<crate::claude_code::AgentCliServices>,
    /// Pull requests the Git overview last observed, for the "needs you" counts (#147).
    pub(crate) terminal_scan_after: Mutex<HashMap<SandboxId, TaskId>>,
    pub(crate) observed_pulls: crate::sandboxes::ObservedPulls,
    /// Local Git checks can outlive the page read that requested them.
    pub(crate) parked_git: Arc<tokio::sync::Semaphore>,
    /// A parked Git worker owns the application until its dispatched work finishes.
    pub(crate) parked_git_app: std::sync::Weak<Self>,
    /// What the pull request watch told each task, as its file has it (SPEC-444 PRN-007); read
    /// from the file on first use.
    pub(crate) pr_told: crate::pr_watch::ToldCache,
    /// The armed merges saved before this server started were put back
    /// (`App::restore_armed_merges`): until then their file answers (SPEC-487 CHN-001).
    pub(crate) armed_restored: std::sync::atomic::AtomicBool,
    /// Tasks created by tools in the last hour, per project (SPEC-037 MGT-003).
    pub(crate) tool_tasks: crate::manage::ToolTaskWindow,
    /// The Superagents' requests to start tasks in other sandboxes, waiting for their person
    /// (SPEC-335).
    pub(crate) cross_requests: crate::cross_sandbox::CrossRequests,
    /// The Superagents' scheduled checks (SPEC-487 CHK-001).
    pub(crate) superagent_checks: crate::superagent_checks::SuperagentChecks,
    /// Script watches' scheduler state (SPEC-630).
    pub(crate) watches: crate::watches::WatchRuntime,
    /// Missing abilities reported in the last hour (SPEC-487 ABL-004).
    pub(crate) ability_reports: crate::ability_reports::AbilityReports,
    /// What each queued task a supertask holds waits for, as last published (SPEC-071).
    pub(crate) supertask_holds: crate::supertask::Holds,
    /// The last Docker smoke test started from Settings → Instance (SPEC-203 HST-015).
    pub(crate) docker_smoke: crate::sandbox_docker::SmokeState,
    /// The last leftovers scan and the running or last deletion (SPEC-215).
    pub(crate) leftovers: crate::leftovers::LeftoverState,
    /// The data disk's last level (SPEC-276).
    pub(crate) disk_watch: crate::pressure::DiskWatch,
    /// Sandboxes whose files a purge deletes right now (SPEC-276 RDA-011).
    pub(crate) purging: Mutex<std::collections::HashSet<yulius_domain::ids::SandboxId>>,
    /// Sandboxes whose disk upkeep runs right now.
    pub(crate) upkeeps: Mutex<std::collections::HashSet<yulius_domain::ids::SandboxId>>,
    /// The egress proxy's and the sandbox relays' open connection troubles (SPEC-361).
    pub(crate) connection_health: Arc<yulius_domain::connection_health::ConnectionHealth>,
    /// GitHub's last answer about the instance's sign-in callback (SPEC-466).
    pub(crate) github_callback: crate::github_callback::CallbackKept,
    /// What the tasks were told of their sandbox's Docker trouble (SPEC-494 DFJ-007).
    pub(crate) docker_told: crate::docker_trouble::DockerTold,
    /// The overlaps a holding task was told of (or is being told), by holder, claimer and paths:
    /// one message each (SPEC-493 SCO-001).
    pub(crate) scope_told: std::sync::Mutex<std::collections::BTreeSet<String>>,
    /// Per-sandbox browser session notice watchers `(watcher_id, AbortHandle)` so
    /// `ActionCleared` / `Disconnected` run once per sandbox regardless of connected viewer count
    /// or `BrowserPort::ensure` wrapper allocations (SPEC-578).
    pub(crate) browser_watchers:
        Mutex<HashMap<yulius_domain::ids::SandboxId, (u64, tokio::task::AbortHandle)>>,
    /// Monotonic id generator for [`Self::browser_watchers`] entries.
    pub(crate) next_browser_watcher_id: std::sync::atomic::AtomicU64,
    /// Counts of `(sandbox, tab_id)` browser action clears already persisted and broadcast by
    /// `App` before `session.clear_action` emitted `BrowserNotice::ActionCleared`, so the sandbox
    /// watcher skips duplicate DB scans and duplicate `UiEvent::BrowserActionCleared` (SPEC-578).
    pub(crate) handled_browser_clears:
        Mutex<HashMap<(yulius_domain::ids::SandboxId, String), usize>>,
    /// True once server shutdown begins (`stop_workspace_services`), so persisted browser
    /// handoffs survive orderly restarts instead of being cleared on browser disconnect (SPEC-578).
    pub(crate) shutting_down: std::sync::atomic::AtomicBool,
}

impl App {
    /// Builds the application.
    #[must_use]
    pub fn new(config: AppConfig, deps: Deps) -> Arc<Self> {
        let instance_tag = deps.crypto.random_bytes()[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let events = EventBus::new(1024);
        let connection_health = crate::connection_health::connection_health(events.clone());
        let limits = crate::limits::LimitState::new(&config);
        let watch_runs = config.watches.max_runs;
        Arc::new_cyclic(|app| Self {
            instance_tag,
            limits,
            config,
            deps,
            events,
            login_attempts: Mutex::new(HashMap::new()),
            device_starts: Mutex::new(HashMap::new()),
            transcript_cursors: Mutex::new(HashMap::new()),
            github_starts: Mutex::new(HashMap::new()),
            guest_attempts: Mutex::new(HashMap::new()),
            remote_calls: Mutex::new(HashMap::new()),
            webhook_slots: Arc::new(tokio::sync::Semaphore::new(16)),
            change_slots: tokio::sync::Semaphore::new(crate::remote_files::MAX_CONCURRENT_CHANGES),
            note_presence: Mutex::new(HashMap::new()),
            browser_port_leases: Mutex::new(HashMap::new()),
            live_docs: crate::live_files::LiveDocs::default(),
            supervisor: crate::supervisor::SupervisorState::default(),
            logins: crate::accounts::LoginRegistry::default(),
            capacity: crate::capacity::CapacityState::default(),
            shells: crate::shells::ShellState::default(),
            workspace_grants: crate::identity::WorkspaceGrants::default(),
            recovery_lock: tokio::sync::Mutex::new(()),
            automation_template_lock: tokio::sync::Mutex::new(()),
            onboarding_task_lock: tokio::sync::Mutex::new(()),
            repository_seed_lock: tokio::sync::Mutex::new(()),
            start_change_lock: tokio::sync::Mutex::new(()),
            previews: crate::preview::PreviewState::default(),
            uploads: crate::uploads::UploadState::default(),
            import_chunks: crate::imports::ChunkLocks::default(),
            attachment_reads: crate::task_prompt::AttachmentReads::default(),
            intake: OnceLock::new(),
            vidoc: OnceLock::new(),
            channel: OnceLock::new(),
            profile_writes: tokio::sync::Mutex::new(()),
            updates: crate::updates::UpdateState::default(),
            move_helper: OnceLock::new(),
            move_asks: tokio::sync::Mutex::new(()),
            command_runs_changed: tokio::sync::Notify::new(),
            harness_rolls: Mutex::default(),
            vault: crate::vault::VaultState::default(),
            ci: crate::ci::CiState::default(),
            isolation: OnceLock::new(),
            agent_cli: OnceLock::new(),
            terminal_scan_after: Mutex::default(),
            observed_pulls: crate::sandboxes::ObservedPulls::default(),
            parked_git: Arc::new(tokio::sync::Semaphore::new(crate::sandboxes::PARKED_READS)),
            parked_git_app: app.clone(),
            pr_told: crate::pr_watch::ToldCache::default(),
            armed_restored: std::sync::atomic::AtomicBool::new(false),
            tool_tasks: crate::manage::ToolTaskWindow::default(),
            cross_requests: crate::cross_sandbox::CrossRequests::default(),
            superagent_checks: crate::superagent_checks::SuperagentChecks::default(),
            watches: crate::watches::WatchRuntime::new(watch_runs),
            ability_reports: crate::ability_reports::AbilityReports::default(),
            supertask_holds: crate::supertask::Holds::default(),
            docker_smoke: crate::sandbox_docker::SmokeState::default(),
            leftovers: crate::leftovers::LeftoverState::default(),
            disk_watch: crate::pressure::DiskWatch::default(),
            purging: Mutex::default(),
            upkeeps: Mutex::default(),
            connection_health,
            github_callback: crate::github_callback::CallbackKept::default(),
            docker_told: crate::docker_trouble::DockerTold::default(),
            scope_told: std::sync::Mutex::default(),
            browser_watchers: Mutex::default(),
            next_browser_watcher_id: std::sync::atomic::AtomicU64::new(1),
            handled_browser_clears: Mutex::default(),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Installs sandbox isolation (once; SPEC-002 ADR-22). Returns whether it was set.
    pub fn set_isolation(
        &self,
        isolation: Arc<dyn crate::ports::isolation::SandboxIsolation>,
    ) -> bool {
        self.isolation.set(isolation).is_ok()
    }

    /// Whether a browser `Origin` may use the API: the public URL or an allowlisted origin.
    #[must_use]
    pub fn origin_allowed(&self, origin: &str) -> bool {
        let origin = origin.trim_end_matches('/');
        origin.eq_ignore_ascii_case(self.config.public_url.trim_end_matches('/'))
            || self
                .config
                .allowed_origins
                .iter()
                .any(|o| o.trim_end_matches('/').eq_ignore_ascii_case(origin))
    }

    /// Current time.
    #[must_use]
    pub fn now(&self) -> yulius_domain::time::Timestamp {
        self.deps.clock.now()
    }
}
