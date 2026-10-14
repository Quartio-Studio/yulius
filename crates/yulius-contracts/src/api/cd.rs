//! CD (deployment): a project's dev and production environments and their runs (SPEC-134;
//! docs/api.md "CD").

use serde::{Deserialize, Serialize};

/// One secret an environment names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdSecret {
    /// Agent-secret name.
    pub name: String,
    /// `ready` (a person allowed it outside the proxy), `not_allowed` (it exists, nobody allowed
    /// it yet) or `missing` (no agent secret of the project has this name).
    pub state: String,
}

/// One environment as its protected branch defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdEnvironment {
    /// `dev` or `prod`.
    pub name: String,
    /// `dev` or `production`.
    pub label: String,
    /// `not_defined`, `empty` or `ready`.
    pub state: String,
    /// The branch its configuration and scripts are read from.
    pub branch: String,
    /// That branch's head.
    #[serde(default)]
    pub head: Option<String>,
    /// Why the configuration cannot be read (`yulius.toml` invalid on the branch).
    #[serde(default)]
    pub problem: Option<String>,
    /// The line for people.
    #[serde(default)]
    pub about: Option<String>,
    /// Deploy command.
    #[serde(default)]
    pub deploy: Option<String>,
    /// Verify command.
    #[serde(default)]
    pub verify: Option<String>,
    /// Production promotes the integration branch first.
    pub promote: bool,
    /// With `promote`: the integration head a deploy now would promote (as last fetched).
    #[serde(default)]
    pub promotes_from: Option<String>,
    /// Secrets the commands get.
    pub secrets: Vec<CdSecret>,
    /// The commit of the last passed run.
    #[serde(default)]
    pub deployed: Option<String>,
    /// The latest run.
    #[serde(default)]
    pub last_run: Option<CdRun>,
}

/// One CD run (lists leave the output out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdRun {
    /// Run id.
    pub run_id: String,
    /// `dev` or `prod`.
    pub environment: String,
    /// `running`, `passed`, `failed`, `error` or `cancelled`.
    pub state: String,
    /// Branch deployed.
    pub branch: String,
    /// Commit deployed.
    pub commit: String,
    /// Who asked: a login or `task #N`.
    pub requested_by: String,
    /// Deploy command run.
    #[serde(default)]
    pub deploy: Option<String>,
    /// Verify command run.
    #[serde(default)]
    pub verify: Option<String>,
    /// What happened before the commands (the promotion).
    #[serde(default)]
    pub note: Option<String>,
    /// Why it errored or was cancelled.
    #[serde(default)]
    pub reason: Option<String>,
    /// Exit code.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Start (RFC 3339).
    pub started_at: String,
    /// End (RFC 3339).
    #[serde(default)]
    pub ended_at: Option<String>,
    /// The last 512 KiB of output, agent secrets redacted (detail only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// `GET /api/sandboxes/{sid}/cd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdOverview {
    /// The project's slug: what a person types to deploy production.
    pub slug: String,
    /// Dev, then production.
    pub environments: Vec<CdEnvironment>,
    /// Recent runs, newest first.
    pub runs: Vec<CdRun>,
    /// The caller may deploy dev (write access).
    pub can_deploy_dev: bool,
    /// The caller may deploy production (administrator access).
    pub can_deploy_prod: bool,
}

/// `POST /api/sandboxes/{sid}/cd/{env}/deploy`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdDeploy {
    /// Production: the project's slug, typed by the person.
    #[serde(default)]
    pub confirm: Option<String>,
}
