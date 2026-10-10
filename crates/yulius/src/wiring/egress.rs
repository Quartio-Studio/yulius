//! The egress proxy of agent runs on the internal listener (SPEC-002 ADR-18): a `CONNECT` or
//! an absolute-form request is a proxy request; everything else is the internal API.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use yulius_app::context::App;
use yulius_app::ports::egress::EgressPolicy;
use yulius_egress::{
    CertificateAuthority, EgressConfig, EgressProxy, generate_ca_material, run_ca_bundle,
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
pub async fn egress_proxy(app: &Arc<App>, config: EgressConfig) -> Result<Arc<EgressProxy>, String> {
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
    layer
        .proxy
        .handle(request)
        .await
        .map(axum::body::Body::new)
}
