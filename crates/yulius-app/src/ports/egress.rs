//! Egress policy port (SPEC-002 ADR-18): what the credential-injecting proxy asks the
//! application. The proxy adapter owns sockets, TLS and HTTP; the application decides who is
//! calling, which hosts carry agent secrets and how a request to one is rewritten.

use std::sync::Arc;

use async_trait::async_trait;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::vault::redact::PlainMatcher;

/// The agent run behind a proxy credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressClient {
    /// The run (attempt) id.
    pub attempt_id: String,
    /// Its task.
    pub task_id: TaskId,
    /// Task number (audit actor `task:<n>`).
    pub task_number: u32,
    /// The task's sandbox: its secrets and the instance-wide ones apply.
    pub sandbox_id: SandboxId,
}

/// A request to a bound host, as the proxy received it (lowercase header names).
#[derive(Clone, PartialEq, Eq)]
pub struct EgressRequest {
    /// Destination host (lowercase, no brackets).
    pub host: String,
    /// Destination port.
    pub port: u16,
    /// Path and query.
    pub path_and_query: String,
    /// Headers in order (hop-by-hop headers already removed).
    pub headers: Vec<(String, Vec<u8>)>,
}

impl std::fmt::Debug for EgressRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("EgressRequest")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("headers", &names)
            .finish_non_exhaustive()
    }
}

/// What to do with a request to a bound host.
#[derive(Clone, PartialEq, Eq)]
pub enum EgressDecision {
    /// Forward it over verified HTTPS with these parts.
    Forward {
        /// Path and query (placeholders replaced).
        path_and_query: String,
        /// Headers to send.
        headers: Vec<(String, Vec<u8>)>,
        /// Secret names applied (never values).
        used: Vec<String>,
    },
    /// Answer the client with this status and text.
    Refuse {
        /// HTTP status.
        status: u16,
        /// Plain-text explanation (no values).
        message: String,
    },
}

impl std::fmt::Debug for EgressDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forward { used, .. } => f
                .debug_struct("Forward")
                .field("used", used)
                .finish_non_exhaustive(),
            Self::Refuse { status, message } => f
                .debug_struct("Refuse")
                .field("status", status)
                .field("message", message)
                .finish(),
        }
    }
}

/// The application side of the egress proxy.
#[async_trait]
pub trait EgressPolicy: Send + Sync {
    /// The live agent run whose task token this is; `None` refuses the client (407).
    async fn egress_client(&self, token: &str) -> Option<EgressClient>;
    /// Whether `host:port` is bound to an agent secret that applies to the client: the proxy
    /// then terminates TLS and rewrites requests; otherwise it only tunnels.
    async fn egress_intercepts(&self, client: &EgressClient, host: &str, port: u16) -> bool;
    /// Rewrites a request to a bound host (placeholders, injected header).
    async fn egress_apply(&self, client: &EgressClient, request: EgressRequest) -> EgressDecision;
    /// The values (and their encodings) to redact from responses the client receives.
    async fn egress_redactor(&self, client: &EgressClient) -> Arc<PlainMatcher>;
}
