//! CD (deployment): a project's dev and production environments and who may deploy them
//! (SPEC-134, ADR-0109, ADR-0110).
//!
//! `yulius.toml` defines CD with `[cd.dev]` and `[cd.prod]` (parsed in
//! [`crate::spec_format::parse_yulius_toml_cd`]). Each has an optional `deploy` and `verify`
//! command, the agent secrets the commands need and a line for people; production may promote the
//! integration branch first. A missing table is "not defined"; a table with nothing to run is
//! "empty", a valid and explicit state. Yulius reads the tables only from protected branches, so
//! a task branch never changes what a deploy runs.
//!
//! [`authorize`] is the one rule of who may deploy: dev is a person's or an agent's, production
//! only an administrator's who typed the project's slug.

use std::fmt;

/// Most secrets one environment names.
pub const MAX_CD_SECRETS: usize = 16;
/// Longest `about` line, in characters.
pub const MAX_ABOUT_CHARS: usize = 300;

/// The two CD environments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CdEnv {
    /// Deploys the integration branch; people and agents.
    Dev,
    /// Deploys the production branch; people only.
    Prod,
}

impl CdEnv {
    /// Both, dev first.
    pub const ALL: [Self; 2] = [Self::Dev, Self::Prod];

    /// Wire and table name (`dev`, `prod`).
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

impl fmt::Display for CdEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One environment's table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CdTarget {
    /// Deploy command, run first.
    pub deploy: Option<String>,
    /// Verification command (a smoke test of what is deployed), run after a successful deploy.
    pub verify: Option<String>,
    /// Agent secrets the commands get as environment variables.
    pub secrets: Vec<String>,
    /// One line for people (what this environment is, or why it is empty).
    pub about: Option<String>,
    /// Production only: promote the integration branch before deploying.
    pub promote: bool,
}

impl CdTarget {
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

/// `[cd.dev]` and `[cd.prod]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CdConfig {
    /// `[cd.dev]`.
    pub dev: Option<CdTarget>,
    /// `[cd.prod]`.
    pub prod: Option<CdTarget>,
}

impl CdConfig {
    /// The environment's table.
    #[must_use]
    pub fn target(&self, env: CdEnv) -> Option<&CdTarget> {
        match env {
            CdEnv::Dev => self.dev.as_ref(),
            CdEnv::Prod => self.prod.as_ref(),
        }
    }

    /// The environment's state.
    #[must_use]
    pub fn state(&self, env: CdEnv) -> CdState {
        match self.target(env) {
            None => CdState::NotDefined,
            Some(target) if target.runs_anything() => CdState::Ready,
            Some(_) => CdState::Empty,
        }
    }

    /// Whether both environments are defined (empty counts).
    #[must_use]
    pub fn is_defined(&self) -> bool {
        self.dev.is_some() && self.prod.is_some()
    }
}

/// Whether `name` can name an agent secret (capital letters, digits and `_`, starting with a
/// letter, at most 64 characters).
#[must_use]
pub fn valid_secret_name(name: &str) -> bool {
    crate::vault::SecretName::parse(name).is_ok_and(|parsed| parsed.as_str() == name)
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
    /// No table for the environment.
    #[error("CD {0} is not defined: add [cd.{1}] to yulius.toml on the protected branch")]
    NotDefined(&'static str, &'static str),
    /// The table runs nothing.
    #[error("CD {0} is empty: [cd.{1}] has no deploy, verify or promote yet")]
    NothingToRun(&'static str, &'static str),
}

/// Whether `requester` may deploy `env` of the project `slug`, given its configuration.
///
/// # Errors
/// The [`CdRefusal`] that applies first: who (production is a person's, an administrator's, with
/// the slug typed exactly), then what (a defined environment with something to run).
pub fn authorize(
    config: &CdConfig,
    env: CdEnv,
    requester: Requester<'_>,
    slug: &str,
) -> Result<(), CdRefusal> {
    if env == CdEnv::Prod {
        match requester {
            Requester::Agent => return Err(CdRefusal::ProductionIsAPersons),
            Requester::Person {
                administrator: false,
                ..
            } => return Err(CdRefusal::ProductionNeedsAdministrator),
            Requester::Person { confirmation, .. } => {
                if confirmation.map(str::trim) != Some(slug) || slug.is_empty() {
                    return Err(CdRefusal::ConfirmationMismatch {
                        expected: slug.to_owned(),
                    });
                }
            }
        }
    }
    match config.state(env) {
        CdState::NotDefined => Err(CdRefusal::NotDefined(env.label(), env.as_str())),
        CdState::Empty => Err(CdRefusal::NothingToRun(env.label(), env.as_str())),
        CdState::Ready => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(deploy: Option<&str>, promote: bool) -> CdTarget {
        CdTarget {
            deploy: deploy.map(str::to_owned),
            promote,
            ..CdTarget::default()
        }
    }

    fn person(administrator: bool, confirmation: Option<&str>) -> Requester<'_> {
        Requester::Person {
            administrator,
            confirmation,
        }
    }

    #[test]
    fn an_environment_is_not_defined_empty_or_ready() {
        let config = CdConfig {
            dev: Some(CdTarget::default()),
            prod: None,
        };
        assert_eq!(config.state(CdEnv::Dev), CdState::Empty);
        assert_eq!(config.state(CdEnv::Prod), CdState::NotDefined);
        assert!(!config.is_defined(), "production is still a gap");
        let config = CdConfig {
            dev: Some(CdTarget::default()),
            prod: Some(target(None, true)),
        };
        assert_eq!(
            config.state(CdEnv::Prod),
            CdState::Ready,
            "promotion alone runs"
        );
        assert!(config.is_defined(), "both defined, one of them empty");
        let verify_only = CdTarget {
            verify: Some("scripts/cd/verify.sh".into()),
            ..CdTarget::default()
        };
        assert!(verify_only.runs_anything());
    }

    #[test]
    fn agents_deploy_dev_and_never_production() {
        let config = CdConfig {
            dev: Some(target(Some("scripts/cd/deploy.sh dev"), false)),
            prod: Some(target(Some("scripts/cd/deploy.sh prod"), true)),
        };
        assert_eq!(
            authorize(&config, CdEnv::Dev, Requester::Agent, "app"),
            Ok(())
        );
        assert_eq!(
            authorize(&config, CdEnv::Dev, person(false, None), "app"),
            Ok(())
        );
        assert_eq!(
            authorize(&config, CdEnv::Prod, Requester::Agent, "app"),
            Err(CdRefusal::ProductionIsAPersons)
        );
    }

    #[test]
    fn production_needs_an_administrator_who_types_the_slug() {
        let config = CdConfig {
            dev: None,
            prod: Some(target(None, true)),
        };
        assert_eq!(
            authorize(&config, CdEnv::Prod, person(false, Some("app")), "app"),
            Err(CdRefusal::ProductionNeedsAdministrator)
        );
        for typed in [None, Some(""), Some("App"), Some("yes"), Some("ap")] {
            assert_eq!(
                authorize(&config, CdEnv::Prod, person(true, typed), "app"),
                Err(CdRefusal::ConfirmationMismatch {
                    expected: "app".into()
                }),
                "{typed:?}"
            );
        }
        assert_eq!(
            authorize(&config, CdEnv::Prod, person(true, Some(" app ")), "app"),
            Ok(()),
            "surrounding spaces are not the person's intent"
        );
    }

    #[test]
    fn an_undefined_or_empty_environment_runs_nothing() {
        let config = CdConfig {
            dev: Some(CdTarget {
                about: Some("no dev machine yet".into()),
                ..CdTarget::default()
            }),
            prod: None,
        };
        let dev = authorize(&config, CdEnv::Dev, Requester::Agent, "app").unwrap_err();
        assert_eq!(dev, CdRefusal::NothingToRun("dev", "dev"));
        assert!(dev.to_string().contains("[cd.dev]"));
        assert_eq!(
            authorize(&config, CdEnv::Prod, person(true, Some("app")), "app"),
            Err(CdRefusal::NotDefined("production", "prod"))
        );
        // Who comes before what: an agent learns that production is a person's.
        assert_eq!(
            authorize(&config, CdEnv::Prod, Requester::Agent, "app"),
            Err(CdRefusal::ProductionIsAPersons)
        );
    }

    #[test]
    fn environment_names_round_trip() {
        for env in CdEnv::ALL {
            assert_eq!(CdEnv::parse(env.as_str()), Some(env));
        }
        assert_eq!(CdEnv::parse("production"), Some(CdEnv::Prod));
        assert_eq!(CdEnv::parse("staging"), None);
        assert!(valid_secret_name("DEV_SSH_KEY"));
        assert!(!valid_secret_name("dev_key") && !valid_secret_name("1KEY"));
    }
}
