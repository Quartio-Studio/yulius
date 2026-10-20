//! Egress policy port (SPEC-002 ADR-20): what the credential-injecting proxy asks the
//! application. The proxy adapter owns sockets, TLS and HTTP; the application decides who is
//! calling, which hosts carry agent secrets and how a request to one is rewritten.

use std::sync::Arc;

use async_trait::async_trait;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::vault::redact::PlainMatcher;

/// The run behind a proxy credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressClient {
    /// An agent run, a CI job or a person's Superagent.
    pub caller: EgressCaller,
    /// Its sandbox: the sandbox's secrets and the global ones switched on for it apply.
    pub sandbox_id: SandboxId,
}

/// Who holds a proxy credential. Whose personal secrets apply (SPEC-245 RES-002) is no field of
/// an agent run's: the policy reads its task's current holder per request, so a hand-over or a
/// deleted task applies also to credentials resolved before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressCaller {
    /// An agent run: every agent secret of its sandbox applies.
    Agent {
        /// The run (attempt) id.
        attempt_id: String,
        /// Its task.
        task_id: TaskId,
        /// Task number (audit actor `task:<n>`).
        task_number: u32,
    },
    /// A CI job (SPEC-142): only the secrets its integration branch's `[[ci.jobs]]` entry
    /// granted apply, never a production-deploy one.
    Ci {
        /// The CI run (audit actor `ci:<run>`).
        run_id: String,
        /// The job.
        job_id: String,
        /// The granted secret names.
        secrets: Vec<String>,
    },
    /// A person's Superagent (SPEC-186 SAV-001): every agent secret of its sandbox applies but a
    /// production-deploy one, as for an agent run.
    Superagent {
        /// Its shell (process), which its uses are audited once per.
        shell_id: String,
        /// Audit actor `cockpit:<login>`.
        actor: String,
        /// Its person, whose personal secrets switched on for the sandbox apply.
        person: UserId,
    },
}

impl EgressClient {
    /// The audit actor: `task:<n>`, `ci:<run>` or `cockpit:<login>`.
    #[must_use]
    pub fn actor(&self) -> String {
        match &self.caller {
            EgressCaller::Agent { task_number, .. } => format!("task:{task_number}"),
            EgressCaller::Ci { run_id, .. } => format!("ci:{run_id}"),
            EgressCaller::Superagent { actor, .. } => actor.clone(),
        }
    }

    /// What its uses are audited once per: the agent run, the CI job or the Superagent's shell.
    #[must_use]
    pub fn use_key(&self) -> &str {
        match &self.caller {
            EgressCaller::Agent { attempt_id, .. } => attempt_id,
            EgressCaller::Ci { job_id, .. } => job_id,
            EgressCaller::Superagent { shell_id, .. } => shell_id,
        }
    }

    /// Whether agent secret `name` (`cd_prod`: allowed for production deploys) applies to it.
    #[must_use]
    pub fn may_use(&self, name: &str, cd_prod: bool) -> bool {
        match &self.caller {
            EgressCaller::Agent { .. } => true,
            EgressCaller::Superagent { .. } => !cd_prod,
            EgressCaller::Ci { secrets, .. } => !cd_prod && secrets.iter().any(|s| s == name),
        }
    }
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
