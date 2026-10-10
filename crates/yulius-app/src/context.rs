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
    /// Idle time before a message wakes a waiting task (COM-005).
    pub wake_idle_ms: i64,
    /// Wait between two resumes of tasks paused for lack of subscription capacity (ROT-010).
    pub resume_stagger_ms: u64,
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
            wake_idle_ms: crate::background::WAKE_IDLE_MS,
            resume_stagger_ms: crate::capacity::RESUME_STAGGER_MS,
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
    /// Wakes idle tasks on new messages (set once by the supervisor).
    pub message_waker: OnceLock<Arc<dyn crate::coordination::MessageWaker>>,
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
    /// Live preview handshakes and grants (SPEC-002 ADR-14).
    pub(crate) previews: crate::preview::PreviewState,
    /// A random tag of this server process, sent with every response (`X-Yulius`): the preview
    /// proxy refuses an upstream that answers with it, i.e. Yulius itself (BRW-008).
    pub instance_tag: String,
    /// Task intake (Jev, split proposals); unset means nothing is checked (SPEC-002 ADR-16).
    pub(crate) intake: OnceLock<crate::intake::IntakeServices>,
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
            message_waker: OnceLock::new(),
            supervisor: crate::supervisor::SupervisorState::default(),
            logins: crate::accounts::LoginRegistry::default(),
            capacity: crate::capacity::CapacityState::default(),
            shells: crate::shells::ShellState::default(),
            workspace_grants: crate::identity::WorkspaceGrants::default(),
            recovery_lock: tokio::sync::Mutex::new(()),
            previews: crate::preview::PreviewState::default(),
            intake: OnceLock::new(),
        })
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
