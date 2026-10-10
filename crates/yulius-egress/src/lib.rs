//! The credential-injecting egress proxy of agent runs (SPEC-002 ADR-18, SPEC-001 VLT-004).
//!
//! Agent runs send HTTP(S) through Yulius (`HTTPS_PROXY` with the run's task token). A host
//! bound to one of the run's agent secrets is intercepted: TLS ends here with a leaf
//! certificate of the instance's egress CA (trusted only by agent runs), the policy adds the
//! credential or replaces its placeholder, the request goes on over verified HTTPS, and the
//! response comes back with every value of the run's agent secrets redacted. Every other host
//! is an opaque tunnel. Loopback, link-local (cloud metadata) and other local destinations are
//! refused after resolution.
//!
//! The application decides through [`yulius_app::ports::egress::EgressPolicy`]; this adapter
//! owns sockets, TLS and HTTP.

mod ca;
mod proxy;

pub use ca::{
    CaError, CertificateAuthority, generate_ca_material, pem_certificates, run_ca_bundle,
};
pub use proxy::{BoxError, EgressBody, EgressConfig, EgressProxy, serve};
