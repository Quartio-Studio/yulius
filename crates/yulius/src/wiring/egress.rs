//! The egress proxy of agent runs on the internal listener (SPEC-002 ADR-20): a `CONNECT` or
//! an absolute-form request is a proxy request; everything else is the internal API.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock, Weak};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use yulius_app::context::App;
use yulius_app::ports::egress::EgressPolicy;
use yulius_browser::{BrowserNetwork, NetworkLease};
use yulius_domain::ids::SandboxId;
use yulius_egress::{
    BrowserEgress, CertificateAuthority, EgressConfig, EgressProxy, generate_ca_material,
    run_ca_bundle, serve_browser,
};

/// The proxy and the internal listener's own authority (requests to it are not proxied).
#[derive(Clone)]
pub struct EgressLayer {
    proxy: Arc<EgressProxy>,
    own_authority: String,
}

/// Builds the proxy: loads (or creates and seals) the egress CA, gives agent runs the CA bundle
/// and the proxy environment.
///
/// # Errors
/// The CA cannot be loaded or created, or the upstream client cannot be built.
pub async fn egress_proxy(
    app: &Arc<App>,
    config: EgressConfig,
) -> Result<Arc<EgressProxy>, String> {
    let material = app
        .egress_ca_material(|| generate_ca_material().map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("egress CA: {e}"))?;
    let ca = CertificateAuthority::from_material(&material).map_err(|e| e.to_string())?;
    let bundle = run_ca_bundle(ca.cert_pem());
    if bundle.is_none() {
        tracing::warn!(
            "the host's CA roots were not found: agent runs get no CA bundle, so HTTPS to hosts bound to agent secrets fails verification in their clients"
        );
    }
    app.enable_egress(bundle);
    let policy: Arc<dyn EgressPolicy> = app.clone();
    // Its troubles are the app's to show (SPEC-361).
    let config = EgressConfig {
        health: app.connection_health(),
        ..config
    };
    EgressProxy::new(policy, ca, config)
        .map(Arc::new)
        .map_err(|e| format!("egress proxy: {e}"))
}

/// Adds the proxy in front of the internal router.
pub fn with_egress(router: Router, proxy: Arc<EgressProxy>, internal_url: &str) -> Router {
    let own_authority = internal_url
        .split_once("://")
        .map_or(internal_url, |(_, rest)| rest)
        .trim_end_matches('/')
        .to_ascii_lowercase();
    router.layer(axum::middleware::from_fn_with_state(
        EgressLayer {
            proxy,
            own_authority,
        },
        dispatch,
    ))
}

async fn dispatch(State(layer): State<EgressLayer>, request: Request, next: Next) -> Response {
    let absolute = request.uri().scheme().is_some()
        && request
            .uri()
            .authority()
            .is_some_and(|a| !a.as_str().eq_ignore_ascii_case(&layer.own_authority));
    if request.method() != Method::CONNECT && !absolute {
        return next.run(request).await;
    }
    layer.proxy.handle(request).await.map(axum::body::Body::new)
}

/// The sandbox browsers' way out (ADR-0321): for each browser a listener of its own on the
/// loopback, which the egress proxy serves as that sandbox's browser. Made before the
/// application (the browser adapter needs it) and bound to it and to the proxy once both
/// exist; a browser asked for before that is not started.
pub struct BrowserProxies {
    /// The listener of Yulius's pages for the browser and the beginnings of their paths.
    pages: (SocketAddr, Vec<String>),
    bound: OnceLock<(Weak<App>, Arc<EgressProxy>)>,
}

impl std::fmt::Debug for BrowserProxies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserProxies")
            .field("pages", &self.pages)
            .field("bound", &self.bound.get().is_some())
            .finish()
    }
}

/// Stops a served proxy when the browser it was for ended.
struct Serving(tokio::task::JoinHandle<()>);

impl Drop for Serving {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl BrowserProxies {
    /// Proxies that give a browser, of Yulius itself, the plain paths beginning with one of
    /// `paths` on `listener` (the welcome page on the internal listener).
    #[must_use]
    pub fn new(listener: SocketAddr, paths: Vec<String>) -> Self {
        Self {
            pages: (listener, paths),
            bound: OnceLock::new(),
        }
    }

    /// Binds the proxies to the application (which knows a sandbox's ports) and to the egress
    /// proxy; the first binding stands.
    pub fn bind(&self, app: &Arc<App>, proxy: Arc<EgressProxy>) {
        let _ = self.bound.set((Arc::downgrade(app), proxy));
    }
}

impl BrowserNetwork for BrowserProxies {
    fn open(&self, sandbox: &SandboxId) -> std::io::Result<NetworkLease> {
        let Some((app, proxy)) = self.bound.get() else {
            return Err(std::io::Error::other("the egress proxy is not running yet"));
        };
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let (app, proxy) = (Weak::clone(app), Arc::clone(proxy));
        let (sandbox, pages) = (sandbox.clone(), self.pages.clone());
        let serving = tokio::spawn(async move {
            // The sandbox's ports are read once: they are given at its creation. Connections
            // made meanwhile wait in the listener's queue.
            let ports = match app.upgrade() {
                Some(app) => app.sandbox_ports(&sandbox).await,
                None => return,
            };
            let ports = match ports {
                Ok(Some(ports)) => ports,
                Ok(None) => return,
                Err(error) => {
                    // No port of the loopback then; the rest of the web still loads.
                    tracing::error!(%sandbox, %error, "a sandbox's ports could not be read for its browser");
                    #[allow(clippy::reversed_empty_ranges)]
                    {
                        1..=0
                    }
                }
            };
            let browser = BrowserEgress {
                sandbox: sandbox.as_str().to_owned(),
                ports,
                pages: Some(pages),
            };
            serve_browser(listener, proxy, Arc::new(browser)).await;
        });
        Ok(NetworkLease {
            proxy: addr,
            hold: Box::new(Serving(serving)),
        })
    }
}
