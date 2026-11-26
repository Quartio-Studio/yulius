//! Servers with real processes: the fake agent behind a real ptyhost, as `yulius serve` runs them.

pub mod first_message;
pub mod pool;
pub mod supervision;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use yulius::wiring::clock::SystemClock;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::ports::Clock;
use yulius_app::ports::browser::BrowserPort;
use yulius_app::ports::session::SessionHost;
use yulius_app::ports::usage::{NoUsageProbe, UsageProbe};
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

use super::FakeGitHub;
use super::agents::{FakeAgentHarness, FakeAgentOptions};

type BoxError = Box<dyn std::error::Error>;

/// Where a run reaches Yulius when no listener serves it.
pub const UNSERVED: &str = "http://127.0.0.1:9";

/// What a real-process app is built from. [`Live::new`] gives what most tests use: the fake agent,
/// the system clock, a fresh fake GitHub, the real ptyhost, no browser and no usage probe.
pub struct Live {
    /// The fake agent; `None` launches the real agent CLIs.
    pub fake: Option<PathBuf>,
    pub agent: FakeAgentOptions,
    pub clock: Arc<dyn Clock>,
    pub github: Arc<FakeGitHub>,
    /// The process host; `None` is the real ptyhost.
    pub host: Option<Arc<dyn SessionHost>>,
    pub browser: Option<Arc<dyn BrowserPort>>,
    pub usage: Arc<dyn UsageProbe>,
}

impl Live {
    /// The fake agent at `fake` with `agent`'s options, and every other part at its default.
    pub fn new(fake: Option<impl AsRef<Path>>, agent: FakeAgentOptions) -> Self {
        Self {
            fake: fake.map(|fake| fake.as_ref().to_path_buf()),
            agent,
            clock: Arc::new(SystemClock),
            github: Arc::new(FakeGitHub::default()),
            host: None,
            browser: None,
            usage: Arc::new(NoUsageProbe),
        }
    }

    /// The app over `data`'s store, its runs reaching Yulius at `url`, uncontained unless `tune`
    /// says otherwise.
    pub fn app(
        self,
        data: &Path,
        url: &str,
        tune: impl FnOnce(&mut AppConfig),
    ) -> Result<Arc<App>, BoxError> {
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        let mut config =
            AppConfig::new(data.to_path_buf(), "https://yulius.test", url, exe.clone());
        config.contained = false;
        tune(&mut config);
        let inner = HarnessAdapter::new(exe.clone(), url);
        let deps = Deps {
            store: Arc::new(SqliteStore::open(&data.join("yulius.db"))?),
            clock: self.clock,
            crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
            github: self.github,
            session_host: self
                .host
                .unwrap_or_else(|| Arc::new(PtySessionHost::new(exe.clone()))),
            harness: match self.fake {
                Some(fake) => Arc::new(FakeAgentHarness {
                    inner,
                    fake,
                    options: self.agent,
                    yulius_exe: exe,
                }),
                None => Arc::new(inner),
            },
            git: Arc::new(super::confined_git()),
            system: Arc::new(super::FixedSystem),
            browser: self
                .browser
                .unwrap_or_else(|| Arc::new(yulius::wiring::browser::UnavailableBrowser)),
            usage: self.usage,
        };
        Ok(App::new(config, deps))
    }

    /// [`Live::app`] served on `listener` as `yulius serve`'s internal listener, so the agents'
    /// hooks reach it; `layer` wraps its router.
    pub fn serve(
        self,
        data: &Path,
        listener: tokio::net::TcpListener,
        tune: impl FnOnce(&mut AppConfig),
        layer: impl FnOnce(Router) -> Router,
    ) -> Result<Arc<App>, BoxError> {
        let app = self.app(data, &url_of(&listener)?, tune)?;
        serve_internal(&app, listener, layer);
        Ok(app)
    }
}

impl Live {
    /// [`Live::serve`] on a fresh listener at `port` (0 = any free port); returns the port.
    pub async fn start(
        self,
        data: &Path,
        port: u16,
        tune: impl FnOnce(&mut AppConfig),
    ) -> Result<(Arc<App>, u16), BoxError> {
        let listener = listen(port)?;
        let port = listener.local_addr()?.port();
        Ok((self.serve(data, listener, tune, |router| router)?, port))
    }
}

/// A two-thread runtime for a test that blocks on it.
pub fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

/// What task `id`'s terminal shows now.
pub fn screen(app: &App, id: &yulius_domain::ids::TaskId) -> String {
    app.runtime(id).hub.screen_text()
}

/// Task `id`'s record as the store holds it now.
pub async fn task(
    app: &App,
    id: &yulius_domain::ids::TaskId,
) -> Result<yulius_app::model::TaskRecord, BoxError> {
    Ok(app.deps.store.task(id).await?.ok_or("task vanished")?)
}

/// Where runs reach a server on `listener`.
pub fn url_of(listener: &tokio::net::TcpListener) -> Result<String, BoxError> {
    Ok(format!(
        "http://127.0.0.1:{}",
        listener.local_addr()?.port()
    ))
}

/// Serves `app`'s internal router, wrapped by `layer`, on `listener` as `yulius serve` does.
pub fn serve_internal(
    app: &Arc<App>,
    listener: tokio::net::TcpListener,
    layer: impl FnOnce(Router) -> Router,
) {
    let internal = layer(yulius_server::internal_router(app.clone()))
        .into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, internal).await });
}

/// A listener on 127.0.0.1:`port` (0 = any free port) that a restarted server can bind again.
pub fn listen(port: u16) -> Result<tokio::net::TcpListener, BoxError> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    Ok(socket.listen(64)?)
}
