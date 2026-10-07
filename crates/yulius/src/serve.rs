//! `yulius serve` and `yulius backup`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use tokio::net::TcpListener;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_store::SqliteStore;
use yulius_store::crypto::{MasterKeyCrypto, write_private};

use crate::wiring::browser::ChromiumBrowsers;
use crate::wiring::clock::SystemClock;
use crate::wiring::editor::{UnavailableEditor, VsCodeEditors};
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
    /// openvscode-server executable for the sandbox editor.
    #[arg(
        long,
        env = "YULIUS_OPENVSCODE_SERVER",
        default_value = "openvscode-server"
    )]
    pub openvscode_server: PathBuf,
    /// Loopback port range for editors, `first-last`.
    #[arg(long, env = "YULIUS_EDITOR_PORTS", default_value = "45000-45999")]
    pub editor_ports: String,
    /// Allow the editor in contained mode although openvscode-server (and its integrated
    /// terminal) runs outside the containment boundary.
    #[arg(long, env = "YULIUS_EDITOR_UNCONTAINED", default_value_t = false)]
    pub editor_uncontained: bool,
    /// Login shell for sandbox shells (default: $SHELL, /bin/bash, /bin/sh).
    #[arg(long, env = "YULIUS_SHELL")]
    pub shell: Option<PathBuf>,
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
    Arc::new(ChromiumBrowsers::new(config))
}

/// The editor adapter, or a placeholder explaining why the editor is disabled.
fn editor_port(args: &ServeArgs) -> Result<Arc<dyn yulius_app::ports::editor::EditorPort>, String> {
    if args.contained && !args.editor_uncontained {
        return Ok(Arc::new(UnavailableEditor::new(
            "the editor is disabled in contained mode because openvscode-server would run outside the sandbox containment; start yulius with --editor-uncontained to accept that",
        )));
    }
    let ports = parse_port_range(&args.editor_ports)?;
    let config = yulius_editor::EditorConfig::new(
        args.openvscode_server.clone(),
        args.data_dir.join("editors"),
        ports,
    );
    Ok(Arc::new(VsCodeEditors::new(config)))
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
        editor: editor_port(args)?,
    };
    let app = App::new(config, deps);
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
    let workspace_reaper = app.start_workspace_services();
    let public_router = yulius_server::public_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let internal_router = yulius_server::internal_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let public_task = axum::serve(public, public_router).with_graceful_shutdown(shutdown_signal());
    let internal_task =
        axum::serve(internal, internal_router).with_graceful_shutdown(shutdown_signal());
    let (a, b) = tokio::join!(public_task, internal_task);
    // Browsers, editors and shells do not outlive the server (agent sessions do).
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
