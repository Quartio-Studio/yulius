//! The application object: configuration, injected ports and shared runtime state.
//!
//! Use cases are `impl App` blocks in the area modules; the interface layer holds `Arc<App>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

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
    /// Maximum concurrently live sessions.
    pub max_live_sessions: u32,
    /// Maximum queued tasks.
    pub queue_max: u32,
    /// Web session lifetime.
    pub session_ttl_ms: i64,
    /// Permission cache freshness.
    pub permission_ttl_ms: i64,
    /// Accept stale permissions on GitHub failure up to this age.
    pub permission_stale_ms: i64,
    /// Cookies carry `Secure`.
    pub secure_cookies: bool,
    /// How often open terminal viewers re-check their access (ACC-008).
    pub viewer_recheck_ms: u64,
    /// Wait between two resumes of tasks paused for lack of subscription capacity (ROT-010).
    pub resume_stagger_ms: u64,
    /// How long a stopped or closed run gets to exit (interrupt, TERM, kill) before its host is
    /// terminated (SPEC-065 END-008).
    pub stop_escalation_ms: u64,
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
    /// How long Claude Code gets to record a goal given as the launch prompt once its session
    /// reads input, before the task is flagged (the sum of the typed goal's submit waits,
    /// SPEC-117 ONE-001). Tests shorten it.
    pub launch_goal_ms: u64,
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
    /// The TLS reverse proxy's shared secret (`YULIUS_PROXY_SECRET_FILE`, SPEC-001 ACC-013): a
    /// request carrying it is keyed on the client address the proxy passes; `None` keys every
    /// request on its TCP peer. Formatting never shows it.
    pub proxy_secret: Option<crate::model::Secret>,
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
            resume_stagger_ms: crate::capacity::RESUME_STAGGER_MS,
            stop_escalation_ms: u64::try_from(
                yulius_domain::task::STOP_INTERRUPT_GRACE_MS
                    + yulius_domain::task::STOP_TERM_GRACE_MS
                    + yulius_domain::task::STOP_KILL_WAIT_MS,
            )
            .unwrap_or(20_000),
            goal_ready_ms: u64::try_from(crate::supervisor::GOAL_READY_TIMEOUT.as_millis())
                .unwrap_or(90_000),
            trust_answer_ms: u64::try_from(crate::supervisor::TRUST_ANSWER_TIMEOUT.as_millis())
                .unwrap_or(300_000),
            launch_goal_ms: crate::supervisor::launch_goal_bound_ms(),
            agent_cpus: None,
            contained_readonly: Vec::new(),
            allowed_origins: Vec::new(),
            public_url,
            internal_url: internal_url.trim_end_matches('/').to_owned(),
            yulius_exe,
            contained: false,
            max_live_sessions: 8,
            queue_max: 32,
            session_ttl_ms: 30 * 24 * 3600 * 1000,
            permission_ttl_ms: 5 * 60 * 1000,
            permission_stale_ms: 30 * 60 * 1000,
            upload_max_bytes: crate::uploads::DEFAULT_UPLOAD_MAX_BYTES,
            ci: crate::ci::CiSettings::default(),
            proxy_secret: None,
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
    /// Live preview handshakes and grants (SPEC-002 ADR-14).
    pub(crate) previews: crate::preview::PreviewState,
    /// Uploads in progress (SPEC-001 EDT-006, UI-048).
    pub(crate) uploads: crate::uploads::UploadState,
    /// Files of original prompts being read (SPEC-080).
    pub(crate) attachment_reads: crate::task_prompt::AttachmentReads,
    /// A random tag of this server process, sent with every response (`X-Yulius`): the preview
    /// proxy refuses an upstream that answers with it, i.e. Yulius itself (BRW-008).
    pub instance_tag: String,
    /// Task intake (Jev, split proposals); unset means nothing is checked (SPEC-002 ADR-16).
    pub(crate) intake: OnceLock<crate::intake::IntakeServices>,
    /// Instance updates: checks of `main` and the update run (SPEC-002 ADR-18).
    pub(crate) updates: crate::updates::UpdateState,
    /// Secrets vault runtime state (SPEC-002 ADR-20).
    pub(crate) vault: crate::vault::VaultState,
    /// Yulius-run CI and repository hygiene (SPEC-002 ADR-21).
    pub(crate) ci: crate::ci::CiState,
    /// Sandbox isolation (SPEC-002 ADR-22); unset: contained runs keep the previous containment.
    pub(crate) isolation: OnceLock<Arc<dyn crate::ports::isolation::SandboxIsolation>>,
    /// Pull requests the Git overview last observed, for the "needs you" counts (#147).
    pub(crate) observed_pulls: crate::sandboxes::ObservedPulls,
    /// Tasks created by tools in the last hour, per project (SPEC-037 MGT-003).
    pub(crate) tool_tasks: crate::manage::ToolTaskWindow,
    /// What each queued task a supertask holds waits for, as last published (SPEC-071).
    pub(crate) supertask_holds: crate::supertask::Holds,
}

impl App {
    /// Builds the application.
    #[must_use]
    pub fn new(config: AppConfig, deps: Deps) -> Arc<Self> {
        let instance_tag = deps.crypto.random_bytes()[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Arc::new(Self {
            instance_tag,
            config,
            deps,
            events: EventBus::new(1024),
            login_attempts: Mutex::new(HashMap::new()),
            device_starts: Mutex::new(HashMap::new()),
            supervisor: crate::supervisor::SupervisorState::default(),
            logins: crate::accounts::LoginRegistry::default(),
            capacity: crate::capacity::CapacityState::default(),
            shells: crate::shells::ShellState::default(),
            workspace_grants: crate::identity::WorkspaceGrants::default(),
            recovery_lock: tokio::sync::Mutex::new(()),
            automation_template_lock: tokio::sync::Mutex::new(()),
            onboarding_task_lock: tokio::sync::Mutex::new(()),
            previews: crate::preview::PreviewState::default(),
            uploads: crate::uploads::UploadState::default(),
            attachment_reads: crate::task_prompt::AttachmentReads::default(),
            intake: OnceLock::new(),
            updates: crate::updates::UpdateState::default(),
            vault: crate::vault::VaultState::default(),
            ci: crate::ci::CiState::default(),
            isolation: OnceLock::new(),
            observed_pulls: crate::sandboxes::ObservedPulls::default(),
            tool_tasks: crate::manage::ToolTaskWindow::default(),
            supertask_holds: crate::supertask::Holds::default(),
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
