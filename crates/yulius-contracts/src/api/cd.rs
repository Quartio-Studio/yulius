//! CD (deployment): a project's environments and their runs (SPEC-134, SPEC-257; docs/api.md
//! "CD").

use serde::{Deserialize, Serialize};

/// One secret an environment names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdSecret {
    /// Agent-secret name.
    pub name: String,
    /// `ready` (a person allowed it for this environment), `not_allowed` (it exists, nobody allowed
    /// it yet), `missing` (no agent secret of the project has this name), `production` (dev
    /// names a production secret, which it never gets) or `agents_use` (production names a secret
    /// agents use, which it never gets and nobody can allow, SPEC-237).
    pub state: String,
}

/// One agent state an environment's runs restore and save (SPEC-278).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdAgentState {
    /// Agent-state name.
    pub name: String,
    /// The vault's newest version; `None`: no agent state of the project has this name (a deploy
    /// is refused).
    #[serde(default)]
    pub version: Option<u32>,
    /// The running deploy that holds it, as `<environment> run <id>`.
    #[serde(default)]
    pub held_by: Option<String>,
}

/// One environment as its protected branch defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdEnvironment {
    /// `[cd.<name>]`: `dev`, `prod` or another name.
    pub name: String,
    /// `production` for `prod`, else the name.
    pub label: String,
    /// `dev` or `prod`: what its rules follow (ADR-0195).
    pub stage: String,
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
    /// Agent states its runs restore before the deploy and save after it.
    #[serde(default)]
    pub states: Vec<CdAgentState>,
    /// The commit of the last passed run.
    #[serde(default)]
    pub deployed: Option<String>,
    /// The latest run.
    #[serde(default)]
    pub last_run: Option<CdRun>,
    /// Its rollback (SPEC-415).
    #[serde(default)]
    pub rollback: CdRollback,
}

/// An environment's rollback (SPEC-415, ADR-0263).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdRollback {
    /// The `rollback` command; `None`: rollback is not configured.
    #[serde(default)]
    pub command: Option<String>,
    /// How schema and data changes stay compatible with the previous release.
    #[serde(default)]
    pub migrations: Option<String>,
    /// Each part's releases (one unnamed part without `parts`).
    #[serde(default)]
    pub parts: Vec<CdPart>,
    /// The dev-stage environment whose drill proves this one (itself for a dev-stage one).
    pub drill_environment: String,
    /// The newest drill of that environment.
    #[serde(default)]
    pub drill: Option<CdRun>,
    /// Production stage: why a deploy is refused for its rollback now; `None`: it is not.
    #[serde(default)]
    pub blocked: Option<String>,
}

/// One part's kept releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdPart {
    /// The part's name (empty: the environment's only part).
    pub name: String,
    /// The release deployed now.
    #[serde(default)]
    pub current: Option<String>,
    /// What "Roll back to previous" goes to.
    #[serde(default)]
    pub previous: Option<String>,
    /// The kept releases, newest first (at most 10).
    pub releases: Vec<CdRelease>,
}

/// One kept release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdRelease {
    /// Release id.
    pub release: String,
    /// The commit it was deployed from.
    pub commit: String,
    /// When its deploy ended (RFC 3339).
    pub deployed_at: String,
    /// Who deployed it: a login, `task #N` or `yulius`.
    pub requested_by: String,
    /// It is deployed now.
    pub current: bool,
    /// Why rolling back to it is refused (a barrier).
    #[serde(default)]
    pub blocked: Option<String>,
    /// Its deploy said rolling back past it is unsafe, and why.
    #[serde(default)]
    pub barrier: Option<String>,
}

/// One CD run (lists leave the output out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdRun {
    /// Run id.
    pub run_id: String,
    /// `deploy`, `rollback` or `drill` (SPEC-415).
    #[serde(default = "deploy_kind")]
    pub kind: String,
    /// A rollback's or a drill's releases, in order.
    #[serde(default)]
    pub steps: Vec<CdStep>,
    /// The environment's name.
    pub environment: String,
    /// Its stage when it ran: `dev` or `prod`.
    pub stage: String,
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

fn deploy_kind() -> String {
    "deploy".to_owned()
}

/// One release a rollback or a drill goes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdStep {
    /// The part (empty: the environment's only one).
    pub part: String,
    /// The release id.
    pub release: String,
}

/// `GET /api/sandboxes/{sid}/cd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdOverview {
    /// The project's slug: what a person types to deploy production.
    pub slug: String,
    /// The dev-stage environments, then the production-stage ones (SPEC-257 CDN-007).
    pub environments: Vec<CdEnvironment>,
    /// Recent runs, newest first.
    pub runs: Vec<CdRun>,
    /// The caller may deploy dev-stage environments (write access).
    pub can_deploy_dev: bool,
    /// The caller may deploy production-stage environments (administrator access).
    pub can_deploy_prod: bool,
}

/// `POST /api/sandboxes/{sid}/cd/{env}/deploy`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdDeploy {
    /// Production: the project's slug, typed by the person.
    #[serde(default)]
    pub confirm: Option<String>,
    /// The commit the person was shown (at least 7 hex digits): the one it deploys, or with a
    /// promotion the integration commit it promotes. Refused (409) when the branch moved since.
    #[serde(default)]
    pub expect: Option<String>,
    /// Production: how long the person kept the confirmation open, in milliseconds, as the page
    /// measured it (SPEC-137 STG-302); kept up to an hour, recorded as the run's confirmation wait.
    #[serde(default)]
    pub confirm_ms: Option<u64>,
}

/// `POST /api/sandboxes/{sid}/cd/{env}/rollback` (SPEC-415).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdRollbackRequest {
    /// The part; required when the environment has several.
    #[serde(default)]
    pub part: Option<String>,
    /// The kept release to go to; default the previous one.
    #[serde(default)]
    pub release: Option<String>,
    /// Production: the project's slug, typed by the person.
    #[serde(default)]
    pub confirm: Option<String>,
    /// Production: how long the person kept the confirmation open, in milliseconds.
    #[serde(default)]
    pub confirm_ms: Option<u64>,
}
