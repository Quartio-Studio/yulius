//! `yulius serve` and `yulius backup`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use tokio::net::TcpListener;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_store::SqliteStore;
use yulius_store::crypto::{MasterKeyCrypto, write_private};

use crate::wiring::clock::SystemClock;
use crate::wiring::github::UnavailableGitHub;
use crate::wiring::harness::HarnessAdapter;
use crate::wiring::pending::{UnavailableGit, UnavailableSessionHost};

/// Server options (flags or environment variables).
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Data directory (database, keys, sandboxes).
    #[arg(long, env = "YULIUS_DATA", default_value = "/var/lib/yulius")]
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
}

/// Prepares the data directory with owner-only permissions.
fn prepare_data_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create data directory {}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot restrict data directory: {e}"))
}

/// Builds the application over the data directory.
pub fn build_app(args: &ServeArgs) -> Result<Arc<App>, String> {
    prepare_data_dir(&args.data_dir)?;
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
        github: Arc::new(UnavailableGitHub),
        session_host: Arc::new(UnavailableSessionHost),
        harness: Arc::new(HarnessAdapter {
            yulius_exe: config.yulius_exe.clone(),
            yulius_url: config.internal_url.clone(),
        }),
        git: Arc::new(UnavailableGit),
    };
    Ok(App::new(config, deps))
}

/// Runs the server until SIGINT/SIGTERM.
pub async fn serve(args: ServeArgs) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if !args.internal_bind.ip().is_loopback() {
        return Err("the internal listener must bind a loopback address".into());
    }
    let app = build_app(&args)?;
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
    let public_router = yulius_server::public_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let internal_router = yulius_server::internal_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let public_task = axum::serve(public, public_router).with_graceful_shutdown(shutdown_signal());
    let internal_task =
        axum::serve(internal, internal_router).with_graceful_shutdown(shutdown_signal());
    let (a, b) = tokio::join!(public_task, internal_task);
    a.map_err(|e| e.to_string())?;
    b.map_err(|e| e.to_string())?;
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
