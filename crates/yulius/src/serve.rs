//! `yulius serve` and `yulius backup`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::serve::ListenerExt as _;
use clap::Args;
use tokio::net::{TcpListener, TcpStream};
use yulius_app::context::{App, AppConfig, Deps};
use yulius_store::SqliteStore;
use yulius_store::crypto::{MasterKeyCrypto, write_private};

use crate::wiring::browser::ChromiumBrowsers;
use crate::wiring::clock::SystemClock;
use crate::wiring::git::GitAdapter;
use crate::wiring::github_client::GitHubAdapter;
use crate::wiring::harness::HarnessAdapter;
use crate::wiring::session::PtySessionHost;
use crate::wiring::system::SysinfoProbe;

/// Server options (flags or environment variables).
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Data directory (database, keys, sandboxes).
    #[arg(
        long,
        visible_alias = "data",
        env = "YULIUS_DATA",
        default_value = "/var/lib/yulius"
    )]
    pub data_dir: PathBuf,
    /// Public base URL users open (behind the TLS reverse proxy).
    #[arg(
        long,
        env = "YULIUS_PUBLIC_URL",
        default_value = "http://127.0.0.1:8080"
    )]
    pub public_url: String,
    /// Public listener address.
    #[arg(long, env = "YULIUS_BIND", default_value = "127.0.0.1:8080")]
    pub bind: SocketAddr,
    /// Internal listener address (must be loopback).
    #[arg(long, env = "YULIUS_INTERNAL_BIND", default_value = "127.0.0.1:8081")]
    pub internal_bind: SocketAddr,
    /// Maximum concurrently live agent sessions.
    #[arg(long, env = "YULIUS_MAX_SESSIONS", default_value_t = 8)]
    pub max_live_sessions: u32,
    /// Run sandboxes inside bubblewrap containment (Linux).
    #[arg(long, env = "YULIUS_CONTAINED", default_value_t = false)]
    pub contained: bool,
    /// Chromium executable for sandbox browsers (default: found on PATH or in the usual
    /// install locations).
    #[arg(long, env = "YULIUS_CHROMIUM")]
    pub chromium: Option<PathBuf>,
    /// Pass `--no-sandbox` to Chromium (only where Chromium's own sandbox cannot run).
    #[arg(long, env = "YULIUS_CHROMIUM_NO_SANDBOX", default_value_t = false)]
    pub chromium_no_sandbox: bool,
    /// How sandbox browsers render (SPEC-002 ADR-15): `xvfb` (a headful Chrome on a virtual
    /// display, streamed as H.264 video with real input), `headless` (JPEG frames, CDP input) or
    /// `auto` (`xvfb` on Linux when Xvfb and ffmpeg are installed, else `headless`).
    #[arg(long, env = "YULIUS_BROWSER_DISPLAY", value_enum, default_value_t = BrowserDisplay::Auto)]
    pub browser_display: BrowserDisplay,
    /// The Xvfb executable (a path or a name on `PATH`).
    #[arg(long, env = "YULIUS_XVFB", default_value = "Xvfb")]
    pub xvfb: PathBuf,
    /// The ffmpeg executable (with libx264 and x11grab).
    #[arg(long, env = "YULIUS_FFMPEG", default_value = "ffmpeg")]
    pub ffmpeg: PathBuf,
    /// Frames per second of the browser video (5–60).
    #[arg(long, env = "YULIUS_BROWSER_FPS", default_value_t = 60,
          value_parser = clap::value_parser!(u32).range(5..=60))]
    pub browser_fps: u32,
    /// Largest browser viewport in CSS pixels, `WIDTHxHEIGHT`; larger panels show the page scaled
    /// down.
    #[arg(long, env = "YULIUS_BROWSER_MAX_SIZE", default_value = "1920x1200", value_parser = parse_size)]
    pub browser_max_size: (u32, u32),
    /// Device pixels per CSS pixel of the browser (1–3): 2 makes text crisp on HiDPI screens and
    /// costs about three times the encoding CPU and bandwidth.
    #[arg(long, env = "YULIUS_BROWSER_SCALE", default_value_t = 1.0, value_parser = parse_scale)]
    pub browser_scale: f64,
    /// First X display number for browsers (numbers in use are skipped).
    #[arg(long, env = "YULIUS_BROWSER_DISPLAY_BASE", default_value_t = 100)]
    pub browser_display_base: u32,
    /// Extra host paths visible read-only inside containment, colon-separated (for example agent
    /// CLI install directories outside /usr and /opt).
    #[arg(long, env = "YULIUS_CONTAINED_READONLY", value_delimiter = ':')]
    pub contained_readonly: Vec<PathBuf>,
    /// Additional frontend origins allowed for WebSockets and CORS (comma-separated), e.g. a
    /// desktop app's origin.
    #[arg(
        long = "allowed-origin",
        env = "YULIUS_ALLOWED_ORIGINS",
        value_delimiter = ','
    )]
    pub allowed_origins: Vec<String>,
    /// Login shell for sandbox shells (default: $SHELL, /bin/bash, /bin/sh).
    #[arg(long, env = "YULIUS_SHELL")]
    pub shell: Option<PathBuf>,
    /// Parent domain of the live preview origins `<port>-<key>.<domain>` (SPEC-002 ADR-14):
    /// default the public URL's host; `off` disables live previews. Needs wildcard DNS and TLS
    /// (docs/operations.md).
    #[arg(long, env = "YULIUS_PREVIEW_DOMAIN")]
    pub preview_domain: Option<String>,
    /// Largest file one upload may carry, in MiB (files dropped onto the editor or a terminal,
    /// SPEC-001 EDT-006, UI-048). The reverse proxy's body limit must allow it.
    #[arg(long, env = "YULIUS_UPLOAD_MAX_MB", default_value_t = 25,
          value_parser = clap::value_parser!(u64).range(1..=1024))]
    pub upload_max_mb: u64,
    /// File holding the TypeSafe Jev API key (owner-only, mode 0600) for task intake: split
    /// checks of new tasks and scope checks of prompts typed into running tasks (SPEC-002
    /// ADR-16). Unset or unreadable: intake checks nothing and blocks nothing.
    #[arg(long, env = "YULIUS_JEV_API_KEY_FILE")]
    pub jev_api_key_file: Option<PathBuf>,
    /// Probability of "several workstreams" Jev must give before a split is proposed (0.5–1).
    #[arg(long, env = "YULIUS_JEV_SPLIT_THRESHOLD", default_value_t = yulius_domain::intake::DEFAULT_SPLIT_THRESHOLD, value_parser = parse_threshold)]
    pub jev_split_threshold: f64,
    /// Probability Jev must give before a prompt typed into a running task is held (0.5–1).
    #[arg(long, env = "YULIUS_JEV_SCOPE_THRESHOLD", default_value_t = yulius_domain::intake::DEFAULT_SCOPE_THRESHOLD, value_parser = parse_threshold)]
    pub jev_scope_threshold: f64,
}

/// Parses a probability threshold (0.5–1).
pub fn parse_threshold(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|t| (0.5..=1.0).contains(t))
        .ok_or_else(|| format!("invalid threshold {text:?}: expected a number from 0.5 to 1"))
}

/// The task-intake services (SPEC-002 ADR-16): Jev when its key file is usable, and then split
/// proposals by headless Claude Code runs in `<data>/runtime/intake`.
fn intake_services(args: &ServeArgs) -> yulius_app::intake::IntakeServices {
    use yulius_app::ports::intake::{ChoiceEvaluator, SplitProposer};
    let jev: Option<Arc<dyn ChoiceEvaluator>> = match &args.jev_api_key_file {
        None => {
            tracing::info!("task intake is off: no Jev key file (YULIUS_JEV_API_KEY_FILE)");
            None
        }
        Some(path) => match crate::wiring::jev::read_key_file(path)
            .and_then(crate::wiring::jev::JevClient::new)
        {
            Ok(client) => {
                tracing::info!(key_file = %path.display(), "task intake is on (Jev jev-1.13.0)");
                Some(Arc::new(client))
            }
            Err(err) => {
                tracing::warn!("task intake is off: {err}");
                None
            }
        },
    };
    let proposer: Option<Arc<dyn SplitProposer>> = jev.as_ref().map(|_| {
        Arc::new(crate::wiring::split::ClaudeSplitProposer::new(
            yulius_harness::launch::CLAUDE_PROGRAM,
            args.data_dir.join("runtime").join("intake"),
        )) as Arc<dyn SplitProposer>
    });
    yulius_app::intake::IntakeServices::new(jev, proposer)
        .with_thresholds(args.jev_split_threshold, args.jev_scope_threshold)
}

/// Rendering of the sandbox browsers (SPEC-002 ADR-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BrowserDisplay {
    /// A virtual display when the prerequisites are installed (Linux), else headless.
    Auto,
    /// A headful Chrome on a virtual display (Xvfb), streamed as video.
    Xvfb,
    /// A headless Chrome shown as JPEG frames.
    Headless,
}

/// Parses `WIDTHxHEIGHT` (each 320–4096).
pub fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let (width, height) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("invalid size {text:?}: expected WIDTHxHEIGHT"))?;
    let parse = |value: &str| {
        value
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (320..=4096).contains(n))
            .ok_or_else(|| format!("invalid size {text:?}: each edge must be 320..=4096"))
    };
    Ok((parse(width)?, parse(height)?))
}

/// Parses a device scale factor (1–3).
pub fn parse_scale(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|scale| (1.0..=3.0).contains(scale))
        .ok_or_else(|| format!("invalid scale {text:?}: expected a number from 1 to 3"))
}

/// The display mode for the browser options: `auto` picks a virtual display on Linux when Xvfb
/// and ffmpeg resolve, else headless (development hosts, CI).
pub fn browser_display_mode(args: &ServeArgs) -> yulius_browser::DisplayMode {
    let installed = || {
        yulius_browser::resolve_executable(&args.xvfb).is_ok()
            && yulius_browser::resolve_executable(&args.ffmpeg).is_ok()
    };
    let virtual_display = match args.browser_display {
        BrowserDisplay::Headless => false,
        BrowserDisplay::Xvfb => true,
        BrowserDisplay::Auto => cfg!(target_os = "linux") && installed(),
    };
    if !virtual_display {
        if args.browser_display == BrowserDisplay::Auto && cfg!(target_os = "linux") {
            tracing::info!("sandbox browsers run headless: Xvfb or ffmpeg is not installed");
        }
        return yulius_browser::DisplayMode::Headless;
    }
    let mut config = yulius_browser::DisplayConfig::new(args.data_dir.join("displays"));
    // Displays a killed predecessor left running hold memory and display numbers.
    let orphans = yulius_browser::stop_orphaned_displays(&config.runtime_root);
    if orphans > 0 {
        tracing::info!(orphans, "stopped virtual displays left by a previous run");
    }
    config.xvfb.clone_from(&args.xvfb);
    config.ffmpeg.clone_from(&args.ffmpeg);
    config.fps = args.browser_fps;
    config.max_size = args.browser_max_size;
    config.scale = args.browser_scale;
    config.first_display = args.browser_display_base;
    yulius_browser::DisplayMode::Virtual(config)
}

/// Parses `first-last` into an inclusive port range.
pub fn parse_port_range(text: &str) -> Result<std::ops::RangeInclusive<u16>, String> {
    let (first, last) = text
        .split_once('-')
        .ok_or_else(|| format!("invalid port range {text:?}: expected first-last"))?;
    let first: u16 = first
        .trim()
        .parse()
        .map_err(|_| format!("invalid port range {text:?}"))?;
    let last: u16 = last
        .trim()
        .parse()
        .map_err(|_| format!("invalid port range {text:?}"))?;
    if first == 0 || first > last {
        return Err(format!(
            "invalid port range {text:?}: first must be 1..=last"
        ));
    }
    Ok(first..=last)
}

/// The browser adapter for the data directory (profiles live in each sandbox directory).
fn browser_port(args: &ServeArgs) -> Arc<dyn yulius_app::ports::browser::BrowserPort> {
    let exe = args.chromium.clone().unwrap_or_else(|| {
        // An undiscoverable Chromium surfaces as a typed "not found" error on first use.
        yulius_browser::discover_chromium().unwrap_or_else(|_| PathBuf::from("chromium"))
    });
    let mut config = yulius_browser::BrowserConfig::new(exe, args.data_dir.join("sandboxes"));
    config.no_sandbox = args.chromium_no_sandbox;
    config.display = browser_display_mode(args);
    // New and blank tabs show the welcome page from the internal listener (BRW-006).
    let welcome = format!("http://{}/browser/welcome", args.internal_bind);
    config.new_tab_url = Some(format!("{welcome}?sandbox={{sandbox}}"));
    // Never Yulius itself (BRW-008): its public origin, preview origins and listeners; the
    // welcome page stays reachable.
    config.deny = yulius_browser::UrlDenyList::yulius(
        &args.public_url,
        &[args.bind, args.internal_bind],
        vec![format!("{welcome}?"), format!("{welcome}/ports?")],
    );
    Arc::new(ChromiumBrowsers::new(config))
}

/// Live preview origins: under the public host by default, under `domain` when given, none for
/// `off` (SPEC-002 ADR-14).
pub fn preview_config(
    public_url: &str,
    domain: Option<&str>,
) -> Result<Option<yulius_app::preview::PreviewConfig>, String> {
    use yulius_app::preview::PreviewConfig;
    let derived = PreviewConfig::from_public_url(public_url);
    match domain.map(str::trim) {
        None | Some("") => Ok(derived),
        Some(off) if off.eq_ignore_ascii_case("off") => Ok(None),
        Some(domain) => {
            let scheme = if public_url.starts_with("https://") {
                "https"
            } else {
                "http"
            };
            // The previews are served by the same listener behind the same proxy: same port.
            let port = public_url
                .split_once("://")
                .and_then(|(_, rest)| rest.split('/').next())
                .and_then(|authority| authority.rsplit_once(':'))
                .filter(|(host, _)| !host.ends_with('[') && !host.contains('['))
                .and_then(|(_, port)| port.parse::<u16>().ok());
            PreviewConfig::new(scheme, domain, port)
                .map(Some)
                .ok_or_else(|| format!("invalid preview domain {domain:?}: expected a DNS name"))
        }
    }
}

/// Prepares the data directory with owner-only permissions.
fn prepare_data_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create data directory {}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot restrict data directory: {e}"))
}

/// Longest Unix socket path every supported OS accepts (macOS `sun_path` is 104 bytes).
const MAX_SOCKET_PATH: usize = 103;

/// Rejects data directories whose run sockets (`<data>/runs/<uuid>/pty.sock`, `<data>/logins/<uuid>/pty.sock`) would exceed the
/// Unix socket path limit, so the failure happens at startup and not at the first task launch.
pub fn check_socket_paths(data_dir: &std::path::Path) -> Result<(), String> {
    let resolved = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    let longest = resolved.as_os_str().len() + "/logins/".len() + 36 + "/pty.sock".len();
    if longest > MAX_SOCKET_PATH {
        return Err(format!(
            "the data directory path {} is too long for session sockets ({longest} > {MAX_SOCKET_PATH} bytes); use a shorter --data path",
            resolved.display()
        ));
    }
    Ok(())
}

/// Builds the application over the data directory.
pub fn build_app(args: &ServeArgs) -> Result<Arc<App>, String> {
    prepare_data_dir(&args.data_dir)?;
    check_socket_paths(&args.data_dir)?;
    let store = SqliteStore::open(&args.data_dir.join("yulius.db")).map_err(|e| e.to_string())?;
    let crypto = MasterKeyCrypto::load_or_create(&args.data_dir.join("master.key"))
        .map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot resolve executable: {e}"))?;
    let internal_url = format!("http://{}", args.internal_bind);
    let mut config = AppConfig::new(args.data_dir.clone(), &args.public_url, &internal_url, exe);
    config.max_live_sessions = args.max_live_sessions;
    config.contained = args.contained;
    config.upload_max_bytes = args.upload_max_mb * 1024 * 1024;
    config.own_ports = vec![args.bind.port(), args.internal_bind.port()];
    config.preview = preview_config(&args.public_url, args.preview_domain.as_deref())?;
    config
        .contained_readonly
        .clone_from(&args.contained_readonly);
    let deps = Deps {
        store: Arc::new(store),
        clock: Arc::new(SystemClock),
        crypto: Arc::new(crypto),
        github: Arc::new(GitHubAdapter::new(
            yulius_github::GitHubConfig::github_com().map_err(|e| e.to_string())?,
        )?),
        session_host: Arc::new(PtySessionHost::new(config.yulius_exe.clone())),
        harness: Arc::new(HarnessAdapter::new(
            config.yulius_exe.clone(),
            config.internal_url.clone(),
        )),
        git: Arc::new(GitAdapter::new("git")),
        system: Arc::new(SysinfoProbe::new()),
        browser: browser_port(args),
        usage: Arc::new(crate::wiring::usage::ProviderUsage::new()?),
    };
    let app = App::new(config, deps);
    app.set_intake(intake_services(args));
    if let Some(shell) = &args.shell {
        app.set_shell_program(shell.clone());
    }
    Ok(app)
}

/// Runs the server until SIGINT/SIGTERM.
pub async fn serve(args: ServeArgs) -> Result<(), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if !args.internal_bind.ip().is_loopback() {
        return Err("the internal listener must bind a loopback address".into());
    }
    let app = build_app(&args)?;
    app.cleanup_stale_logins().await;
    match app.reconcile_environment_runs().await {
        Ok(0) => {}
        Ok(n) => tracing::warn!(
            runs = n,
            "environment runs interrupted by a restart were closed"
        ),
        Err(err) => tracing::error!(error = %err, "environment run reconciliation failed"),
    }
    if let Some(token) = app.prepare_setup().await.map_err(|e| e.to_string())? {
        let path = args.data_dir.join("setup.token");
        let _ = std::fs::remove_file(&path);
        write_private(&path, token.expose().as_bytes())
            .map_err(|e| format!("cannot write setup token: {e}"))?;
        println!(
            "Setup required. Open {} and enter the one-time setup token stored in {}",
            args.public_url,
            path.display()
        );
    }
    let public = TcpListener::bind(args.bind)
        .await
        .map_err(|e| format!("cannot bind {}: {e}", args.bind))?;
    let internal = TcpListener::bind(args.internal_bind)
        .await
        .map_err(|e| format!("cannot bind {}: {e}", args.internal_bind))?;
    tracing::info!(public = %args.bind, internal = %args.internal_bind, "yulius listening");
    app.start_background();
    match app.install_skill_catalog().await {
        Ok(files) => tracing::debug!(files, "skill catalog ready"),
        Err(err) => tracing::error!(error = %err, "cannot install the skill catalog"),
    }
    let scheduler = app.start_automation_scheduler();
    // Shells left running by the previous server process are adopted before serving.
    let workspace_reaper = app.start_workspace_services().await;
    let public_router = yulius_server::public_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let internal_router = yulius_server::internal_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let public_task = axum::serve(public.tap_io(no_delay), public_router)
        .with_graceful_shutdown(shutdown_signal());
    let internal_task = axum::serve(internal.tap_io(no_delay), internal_router)
        .with_graceful_shutdown(shutdown_signal());
    let (a, b) = tokio::join!(public_task, internal_task);
    // Browsers do not outlive the server; shells and agent sessions keep running detached.
    drop(workspace_reaper);
    app.stop_workspace_services().await;
    a.map_err(|e| e.to_string())?;
    b.map_err(|e| e.to_string())?;
    scheduler.abort();
    tracing::info!("yulius stopped; detached sessions keep running");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}

/// `yulius backup`.
pub async fn backup(data_dir: PathBuf, dest: PathBuf) -> Result<(), String> {
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    let store = SqliteStore::open(&data_dir.join("yulius.db")).map_err(|e| e.to_string())?;
    store.backup_to(&dest).await.map_err(|e| e.to_string())?;
    println!("database copied to {}", dest.display());
    Ok(())
}

/// Sends small writes at once (#29). A keystroke's echo is one small WebSocket frame; with
/// Nagle's algorithm on, it waited for the proxy's delayed ACK whenever earlier output was still
/// unacknowledged — tens of milliseconds per character while an agent streams output.
fn no_delay(tcp: &mut TcpStream) {
    if let Err(err) = tcp.set_nodelay(true) {
        tracing::debug!(error = %err, "TCP_NODELAY could not be set on a connection");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepted_connections_send_small_writes_at_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let client = TcpStream::connect(address).await?;
        let (mut accepted, _) = listener.accept().await?;
        assert!(!accepted.nodelay()?, "Nagle is on by default");
        no_delay(&mut accepted);
        assert!(accepted.nodelay()?);
        drop(client);
        Ok(())
    }
}
