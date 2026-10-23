//! CD (deployment): a project's environments and who may deploy them (SPEC-134, SPEC-257,
//! ADR-0109, ADR-0110, ADR-0195).
//!
//! `yulius.toml` defines CD with `[cd.<name>]` tables (parsed in
//! [`crate::spec_format::parse_yulius_toml_cd`]); `[cd.dev]` and `[cd.prod]` are the default
//! environments. Each has a [`CdStage`], which carries every rule: the branch it deploys, who may
//! deploy it, its containment and its secrets. Each has an optional `deploy` and `verify` command,
//! the agent secrets the commands need and a line for people; a production-stage environment may
//! promote the integration branch first. A missing table is "not defined"; a table with nothing to
//! run is "empty", a valid and explicit state. Yulius reads the tables only from protected
//! branches, so a task branch never changes what a deploy runs.
//!
//! [`authorize`] is the one rule of who may deploy: a dev-stage environment is a person's or an
//! agent's, a production-stage one only an administrator's who typed the project's slug.

use std::fmt;

/// Most secrets one environment names.
pub const MAX_CD_SECRETS: usize = 16;
/// Most agent states one environment names (SPEC-278).
pub const MAX_CD_STATES: usize = 8;
/// Longest `about` line, in characters.
pub const MAX_ABOUT_CHARS: usize = 300;
/// Most environments one `yulius.toml` defines.
pub const MAX_CD_ENVIRONMENTS: usize = 16;
/// Longest environment name, in characters.
pub const MAX_ENV_NAME_CHARS: usize = 32;

/// The two CD stages: what an environment's rules follow (ADR-0195).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CdStage {
    /// Deploys the integration branch; people and agents.
    Dev,
    /// Deploys the production branch; people only.
    Prod,
}

impl CdStage {
    /// Both, dev first.
    pub const ALL: [Self; 2] = [Self::Dev, Self::Prod];

    /// Wire name and the default environment's name (`dev`, `prod`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prod => "prod",
        }
    }

    /// Parses a wire name (`production` is accepted for `prod`).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "dev" => Some(Self::Dev),
            "prod" | "production" => Some(Self::Prod),
            _ => None,
        }
    }

    /// The name people read (`dev`, `production`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prod => "production",
        }
    }
}

impl fmt::Display for CdStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether `name` can name an environment: 1-32 lowercase letters, digits and `-`, starting with
/// a letter; not `production`, which is how requests name `prod` (review M1).
#[must_use]
pub fn valid_env_name(name: &str) -> bool {
    name != "production"
        && name.len() <= MAX_ENV_NAME_CHARS
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The environment a request names: `production` is `prod`; `None` for what cannot be a name.
#[must_use]
pub fn env_name(text: &str) -> Option<String> {
    match text.trim() {
        "production" => Some("prod".to_owned()),
        name if valid_env_name(name) => Some(name.to_owned()),
        _ => None,
    }
}

/// The stage a default environment's name fixes (`dev`, `prod`); other names take theirs from
/// their table.
#[must_use]
pub fn fixed_stage(name: &str) -> Option<CdStage> {
    match name {
        "dev" => Some(CdStage::Dev),
        "prod" => Some(CdStage::Prod),
        _ => None,
    }
}

/// The name people read: `production` for `prod`, else the name.
#[must_use]
pub fn env_label(name: &str) -> &str {
    if name == "prod" { "production" } else { name }
}

/// One environment's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdTarget {
    /// `[cd.<name>]`.
    pub name: String,
    /// Its stage.
    pub stage: CdStage,
    /// Deploy command, run first.
    pub deploy: Option<String>,
    /// Verification command (a smoke test of what is deployed), run after a successful deploy.
    pub verify: Option<String>,
    /// Agent secrets the commands get as environment variables.
    pub secrets: Vec<String>,
    /// Agent states the run restores before its deploy and saves after it (SPEC-278).
    pub states: Vec<String>,
    /// One line for people (what this environment is, or why it is empty).
    pub about: Option<String>,
    /// Production stage only: promote the integration branch before deploying.
    pub promote: bool,
}

impl CdTarget {
    /// An empty table.
    #[must_use]
    pub fn new(name: &str, stage: CdStage) -> Self {
        Self {
            name: name.to_owned(),
            stage,
            deploy: None,
            verify: None,
            secrets: Vec::new(),
            states: Vec::new(),
            about: None,
            promote: false,
        }
    }

    /// Whether a deploy would do anything.
    #[must_use]
    pub fn runs_anything(&self) -> bool {
        self.deploy.is_some() || self.verify.is_some() || self.promote
    }
}

/// Where an environment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CdState {
    /// No table: CD is not defined for it (a gap to fill).
    NotDefined,
    /// A table with nothing to run: explicitly nothing to deploy yet.
    Empty,
    /// Something runs.
    Ready,
}

impl CdState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotDefined => "not_defined",
            Self::Empty => "empty",
            Self::Ready => "ready",
        }
    }
}

/// The `[cd.<name>]` tables, in file order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CdConfig {
    /// One per table.
    pub environments: Vec<CdTarget>,
}

impl CdConfig {
    /// The table named `name`, whatever its stage.
    #[must_use]
    pub fn target(&self, name: &str) -> Option<&CdTarget> {
        self.environments.iter().find(|target| target.name == name)
    }

    /// The table named `name` when it has `stage`: a table of another stage does not define it
    /// for this one (SPEC-257 CDN-004).
    #[must_use]
    pub fn target_of(&self, name: &str, stage: CdStage) -> Option<&CdTarget> {
        self.target(name).filter(|target| target.stage == stage)
    }

    /// The stage of `name` here: fixed for `dev` and `prod`, else its table's.
    #[must_use]
    pub fn stage_of(&self, name: &str) -> Option<CdStage> {
        fixed_stage(name).or_else(|| self.target(name).map(|target| target.stage))
    }

    /// The state of `name` as an environment of `stage`.
    #[must_use]
    pub fn state(&self, name: &str, stage: CdStage) -> CdState {
        match self.target_of(name, stage) {
            None => CdState::NotDefined,
            Some(target) if target.runs_anything() => CdState::Ready,
            Some(_) => CdState::Empty,
        }
    }

    /// The names of `stage`'s environments: the default first, the others in file order.
    #[must_use]
    pub fn names(&self, stage: CdStage) -> Vec<String> {
        let mut names: Vec<String> = self
            .environments
            .iter()
            .filter(|target| target.stage == stage)
            .map(|target| target.name.clone())
            .collect();
        names.sort_by_key(|name| name != stage.as_str());
        names
    }

    /// Whether CD is defined: an environment of each stage (empty counts, SPEC-257 CDN-005).
    #[must_use]
    pub fn is_defined(&self) -> bool {
        CdStage::ALL
            .iter()
            .all(|stage| self.environments.iter().any(|t| t.stage == *stage))
    }
}

/// The stage of the environment `name` (SPEC-257 CDN-004): fixed for `dev` and `prod`; else the
/// stage the integration head's table gives it; else production when the production head has a
/// production-stage table of that name. `None`: not defined anywhere.
#[must_use]
pub fn resolve_stage(
    name: &str,
    integration: Option<&CdConfig>,
    production: Option<&CdConfig>,
) -> Option<CdStage> {
    fixed_stage(name)
        .or_else(|| integration.and_then(|config| config.stage_of(name)))
        .or_else(|| {
            production
                .and_then(|config| config.target_of(name, CdStage::Prod))
                .map(|_| CdStage::Prod)
        })
}

/// The environments to show (SPEC-257 CDN-007), each with its [`resolve_stage`]: the dev-stage
/// ones (the integration head's), then the production-stage ones (the integration head's, then
/// those only the production head has), the default first within each. A stage without any lists
/// its default, which is then not defined.
#[must_use]
pub fn environments_of(
    integration: Option<&CdConfig>,
    production: Option<&CdConfig>,
) -> Vec<(String, CdStage)> {
    let mut listed = Vec::new();
    for stage in CdStage::ALL {
        let mut names: Vec<String> = integration
            .map(|config| config.names(stage))
            .unwrap_or_default();
        if stage == CdStage::Prod {
            for name in production.map(|c| c.names(stage)).unwrap_or_default() {
                if !names.contains(&name)
                    && resolve_stage(&name, integration, production) == Some(stage)
                {
                    names.push(name);
                }
            }
            names.sort_by_key(|name| name != stage.as_str());
        }
        if names.is_empty() {
            names.push(stage.as_str().to_owned());
        }
        listed.extend(names.into_iter().map(|name| (name, stage)));
    }
    listed
}

/// Whether `name` can name an agent secret (capital letters, digits and `_`, starting with a
/// letter, at most 64 characters).
#[must_use]
pub fn valid_secret_name(name: &str) -> bool {
    crate::vault::SecretName::parse(name).is_ok_and(|parsed| parsed.as_str() == name)
}

/// The environment variable a run reads a restored state's folder from (SPEC-278 CDS-003).
#[must_use]
pub fn state_variable(name: &str) -> String {
    format!("YULIUS_STATE_{name}")
}

/// Who asks for a deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requester<'a> {
    /// A signed-in person; `administrator` is their access to the project, `confirmation` what
    /// they typed.
    Person {
        /// Administrator access to the project.
        administrator: bool,
        /// The typed confirmation, when the request carries one.
        confirmation: Option<&'a str>,
    },
    /// A task's agent (MCP), including an automation's task.
    Agent,
}

/// Why a deploy is refused. Messages are stable and shown to people and agents.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CdRefusal {
    /// Agents never deploy production (ADR-0110).
    #[error(
        "only a person deploys production: ask one with needs_you (they deploy it on the CD page)"
    )]
    ProductionIsAPersons,
    /// Production needs administrator access.
    #[error("deploying production needs administrator access to the project")]
    ProductionNeedsAdministrator,
    /// The typed confirmation is missing or wrong.
    #[error("type the project's slug '{expected}' to confirm a production deploy")]
    ConfirmationMismatch {
        /// What must be typed.
        expected: String,
    },
    /// No table for the environment (label, name).
    #[error("CD {0} is not defined: add [cd.{1}] to yulius.toml on the protected branch")]
    NotDefined(String, String),
    /// The table runs nothing (label, name).
    #[error("CD {0} is empty: [cd.{1}] has no deploy, verify or promote yet")]
    NothingToRun(String, String),
}

impl CdRefusal {
    /// [`CdRefusal::NotDefined`] for `name`.
    #[must_use]
    pub fn not_defined(name: &str) -> Self {
        Self::NotDefined(env_label(name).to_owned(), name.to_owned())
    }
}

/// Whether `requester` may deploy an environment of `stage` of the project `slug` at all,
/// whatever it defines: production is a person's, an administrator's, with the slug typed exactly
/// (surrounding spaces ignored). Checked before anything of the environment is read.
///
/// # Errors
/// [`CdRefusal::ProductionIsAPersons`], [`CdRefusal::ProductionNeedsAdministrator`] or
/// [`CdRefusal::ConfirmationMismatch`].
pub fn may_deploy(stage: CdStage, requester: Requester<'_>, slug: &str) -> Result<(), CdRefusal> {
    if stage == CdStage::Dev {
        return Ok(());
    }
    match requester {
        Requester::Agent => Err(CdRefusal::ProductionIsAPersons),
        Requester::Person {
            administrator: false,
            ..
        } => Err(CdRefusal::ProductionNeedsAdministrator),
        Requester::Person { confirmation, .. } => {
            if slug.is_empty() || confirmation.map(str::trim) != Some(slug) {
                return Err(CdRefusal::ConfirmationMismatch {
                    expected: slug.to_owned(),
                });
            }
            Ok(())
        }
    }
}

/// Whether `requester` may deploy the environment `name` of `stage` of the project `slug`, given
/// its configuration: who ([`may_deploy`]), then what (a table of that stage with something to
/// run).
///
/// # Errors
/// The [`CdRefusal`] that applies first.
pub fn authorize(
    config: &CdConfig,
    name: &str,
    stage: CdStage,
    requester: Requester<'_>,
    slug: &str,
) -> Result<(), CdRefusal> {
    may_deploy(stage, requester, slug)?;
    match config.state(name, stage) {
        CdState::NotDefined => Err(CdRefusal::not_defined(name)),
        CdState::Empty => Err(CdRefusal::NothingToRun(
            env_label(name).to_owned(),
            name.to_owned(),
        )),
        CdState::Ready => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str, stage: CdStage, deploy: Option<&str>, promote: bool) -> CdTarget {
        CdTarget {
            deploy: deploy.map(str::to_owned),
            promote,
            ..CdTarget::new(name, stage)
        }
    }

    fn config(environments: Vec<CdTarget>) -> CdConfig {
        CdConfig { environments }
    }

    fn person(administrator: bool, confirmation: Option<&str>) -> Requester<'_> {
        Requester::Person {
            administrator,
            confirmation,
        }
    }

    #[test]
    fn an_environment_is_not_defined_empty_or_ready() {
        let cd = config(vec![CdTarget::new("dev", CdStage::Dev)]);
        assert_eq!(cd.state("dev", CdStage::Dev), CdState::Empty);
        assert_eq!(cd.state("prod", CdStage::Prod), CdState::NotDefined);
        assert!(!cd.is_defined(), "production is still a gap");
        let cd = config(vec![
            CdTarget::new("dev", CdStage::Dev),
            target("prod", CdStage::Prod, None, true),
        ]);
        assert_eq!(
            cd.state("prod", CdStage::Prod),
            CdState::Ready,
            "promotion alone runs"
        );
        assert!(cd.is_defined(), "both defined, one of them empty");
        let verify_only = CdTarget {
            verify: Some("scripts/cd/verify.sh".into()),
            ..CdTarget::new("dev", CdStage::Dev)
        };
        assert!(verify_only.runs_anything());
    }

    /// SPEC-257 CDN-004/005: named environments of each stage define CD; a table of another stage
    /// does not define a name for this one.
    #[test]
    fn named_environments_take_their_stage() {
        let cd = config(vec![
            CdTarget::new("staging", CdStage::Dev),
            target("platform", CdStage::Prod, Some("deploy.sh"), false),
            CdTarget::new("selfserve", CdStage::Prod),
            CdTarget::new("prod", CdStage::Prod),
        ]);
        assert!(cd.is_defined(), "a dev-stage and a production-stage one");
        assert_eq!(cd.stage_of("platform"), Some(CdStage::Prod));
        assert_eq!(cd.stage_of("staging"), Some(CdStage::Dev));
        assert_eq!(cd.stage_of("dev"), Some(CdStage::Dev), "fixed by its name");
        assert_eq!(cd.stage_of("preview"), None);
        assert_eq!(cd.state("platform", CdStage::Prod), CdState::Ready);
        assert_eq!(cd.state("platform", CdStage::Dev), CdState::NotDefined);
        assert_eq!(cd.names(CdStage::Prod), ["prod", "platform", "selfserve"]);
        assert_eq!(cd.names(CdStage::Dev), ["staging"]);
        let dev_only = config(vec![
            CdTarget::new("dev", CdStage::Dev),
            CdTarget::new("staging", CdStage::Dev),
        ]);
        assert!(!dev_only.is_defined());
    }

    /// SPEC-257 CDN-004/007: the integration head decides a name's stage; production adds the
    /// production-stage names only it has; a stage without any shows its default.
    #[test]
    fn stages_resolve_from_the_integration_head_first() {
        let integration = config(vec![
            CdTarget::new("dev", CdStage::Dev),
            CdTarget::new("platform", CdStage::Dev),
            CdTarget::new("selfserve", CdStage::Prod),
        ]);
        let production = config(vec![
            CdTarget::new("platform", CdStage::Prod),
            CdTarget::new("legacy", CdStage::Prod),
            CdTarget::new("prod", CdStage::Prod),
            CdTarget::new("old-staging", CdStage::Dev),
        ]);
        let (i, p) = (Some(&integration), Some(&production));
        assert_eq!(
            resolve_stage("platform", i, p),
            Some(CdStage::Dev),
            "relabelled on dev"
        );
        assert_eq!(resolve_stage("legacy", i, p), Some(CdStage::Prod));
        assert_eq!(
            resolve_stage("legacy", None, p),
            Some(CdStage::Prod),
            "dev unreadable"
        );
        assert_eq!(
            resolve_stage("old-staging", i, p),
            None,
            "dev stage is dev's"
        );
        assert_eq!(resolve_stage("prod", None, None), Some(CdStage::Prod));
        let listed = environments_of(i, p);
        let listed: Vec<(&str, CdStage)> = listed
            .iter()
            .map(|(name, stage)| (name.as_str(), *stage))
            .collect();
        assert_eq!(
            listed,
            [
                ("dev", CdStage::Dev),
                ("platform", CdStage::Dev),
                ("prod", CdStage::Prod),
                ("selfserve", CdStage::Prod),
                ("legacy", CdStage::Prod),
            ]
        );
        let defaults = environments_of(None, None);
        assert_eq!(
            defaults,
            [
                ("dev".to_owned(), CdStage::Dev),
                ("prod".to_owned(), CdStage::Prod)
            ]
        );
    }

    #[test]
    fn agents_deploy_dev_and_never_production() {
        let cd = config(vec![
            target("dev", CdStage::Dev, Some("scripts/cd/deploy.sh dev"), false),
            target(
                "prod",
                CdStage::Prod,
                Some("scripts/cd/deploy.sh prod"),
                true,
            ),
            target("staging", CdStage::Dev, Some("deploy.sh staging"), false),
            target("platform", CdStage::Prod, Some("deploy.sh platform"), false),
        ]);
        for name in ["dev", "staging"] {
            assert_eq!(
                authorize(&cd, name, CdStage::Dev, Requester::Agent, "app"),
                Ok(())
            );
            assert_eq!(
                authorize(&cd, name, CdStage::Dev, person(false, None), "app"),
                Ok(())
            );
        }
        for name in ["prod", "platform"] {
            assert_eq!(
                authorize(&cd, name, CdStage::Prod, Requester::Agent, "app"),
                Err(CdRefusal::ProductionIsAPersons),
                "{name}"
            );
            assert_eq!(
                authorize(&cd, name, CdStage::Prod, person(true, Some("app")), "app"),
                Ok(())
            );
        }
    }

    #[test]
    fn production_needs_an_administrator_who_types_the_slug() {
        let cd = config(vec![target("prod", CdStage::Prod, None, true)]);
        assert_eq!(
            authorize(
                &cd,
                "prod",
                CdStage::Prod,
                person(false, Some("app")),
                "app"
            ),
            Err(CdRefusal::ProductionNeedsAdministrator)
        );
        for typed in [None, Some(""), Some("App"), Some("yes"), Some("ap")] {
            assert_eq!(
                authorize(&cd, "prod", CdStage::Prod, person(true, typed), "app"),
                Err(CdRefusal::ConfirmationMismatch {
                    expected: "app".into()
                }),
                "{typed:?}"
            );
        }
        assert_eq!(
            authorize(
                &cd,
                "prod",
                CdStage::Prod,
                person(true, Some(" app ")),
                "app"
            ),
            Ok(()),
            "surrounding spaces are not the person's intent"
        );
    }

    #[test]
    fn an_undefined_or_empty_environment_runs_nothing() {
        let cd = config(vec![
            CdTarget {
                about: Some("no dev machine yet".into()),
                ..CdTarget::new("dev", CdStage::Dev)
            },
            target("platform", CdStage::Dev, Some("deploy.sh"), false),
        ]);
        let dev = authorize(&cd, "dev", CdStage::Dev, Requester::Agent, "app").err();
        assert_eq!(
            dev,
            Some(CdRefusal::NothingToRun("dev".into(), "dev".into()))
        );
        assert!(dev.is_some_and(|refusal| refusal.to_string().contains("[cd.dev]")));
        let admin = person(true, Some("app"));
        assert_eq!(
            authorize(&cd, "prod", CdStage::Prod, admin, "app"),
            Err(CdRefusal::NotDefined("production".into(), "prod".into()))
        );
        // A dev-stage table does not define a production environment of that name.
        assert_eq!(
            authorize(&cd, "platform", CdStage::Prod, admin, "app"),
            Err(CdRefusal::not_defined("platform"))
        );
        // Who comes before what: an agent learns that production is a person's.
        assert_eq!(
            authorize(&cd, "prod", CdStage::Prod, Requester::Agent, "app"),
            Err(CdRefusal::ProductionIsAPersons)
        );
    }

    #[test]
    fn environment_names_round_trip() {
        for stage in CdStage::ALL {
            assert_eq!(CdStage::parse(stage.as_str()), Some(stage));
            assert_eq!(fixed_stage(stage.as_str()), Some(stage));
        }
        assert_eq!(CdStage::parse("production"), Some(CdStage::Prod));
        assert_eq!(CdStage::parse("staging"), None);
        assert_eq!(env_name("production").as_deref(), Some("prod"));
        assert_eq!(env_name(" self-serve2 ").as_deref(), Some("self-serve2"));
        for bad in ["", "Prod", "2nd", "-x", "a_b", "a.b", &"a".repeat(33)] {
            assert_eq!(env_name(bad), None, "{bad}");
        }
        // Review M1: `production` is how requests name `prod`, never a table of its own.
        assert!(!valid_env_name("production"));
        assert_eq!(env_label("prod"), "production");
        assert_eq!(env_label("platform"), "platform");
        assert!(valid_secret_name("DEV_SSH_KEY"));
        assert!(!valid_secret_name("dev_key") && !valid_secret_name("1KEY"));
    }
}
