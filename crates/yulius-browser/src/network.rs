//! Where a sandbox's browser sends its traffic (SPEC-483 E2, ADR-0321).
//!
//! Chrome runs on the host, outside its sandbox's network. With a [`BrowserNetwork`] configured,
//! each browser is started with a proxy of its own as its only way out, for every address, the
//! loopback included; what that proxy reaches is its owner's rule (the egress proxy's: what the
//! sandbox's own processes reach). A browser whose way out cannot be opened is not started.

use std::any::Any;
use std::ffi::OsString;
use std::net::SocketAddr;

use yulius_domain::ids::SandboxId;

/// Opens the way out of a sandbox's browser.
pub trait BrowserNetwork: Send + Sync + std::fmt::Debug {
    /// A proxy for `sandbox`'s browser, serving until the lease's `hold` is dropped.
    ///
    /// # Errors
    /// The proxy cannot be opened: the browser is then not started.
    fn open(&self, sandbox: &SandboxId) -> std::io::Result<NetworkLease>;
}

/// One browser's way out: an HTTP proxy on the loopback and what keeps it serving.
pub struct NetworkLease {
    /// Where the proxy listens.
    pub proxy: SocketAddr,
    /// Dropped when the browser ended: the proxy stops then.
    pub hold: Box<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for NetworkLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkLease")
            .field("proxy", &self.proxy)
            .finish_non_exhaustive()
    }
}

/// The Chromium switches that send everything through `proxy`.
///
/// - `--proxy-bypass-list=<-loopback>`: Chrome reaches `localhost` and `127.0.0.1` directly
///   unless told otherwise, whatever proxy is set (measured on Chrome 157);
/// - the resolver rule: no name is resolved on the host (the proxy resolves), so nothing can
///   be reached directly by name either;
/// - WebRTC sends no UDP past the proxy, and QUIC is off (UDP again).
#[must_use]
pub fn proxy_args(proxy: SocketAddr) -> Vec<OsString> {
    vec![
        format!("--proxy-server=http://{proxy}").into(),
        "--proxy-bypass-list=<-loopback>".into(),
        "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1".into(),
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
        "--disable-quic".into(),
    ]
}
