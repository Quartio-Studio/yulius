//! CPU and memory budgets of the processes Yulius starts in a sandbox (SPEC-082).
//!
//! A sandbox has a capacity: the CPUs and memory its processes may use (its limits, else the
//! host's). Yulius gives every process group it starts a budget out of it and says so the same way
//! in any language, in the environment: [`CPUS_ENV`] (whole CPUs) and [`MEMORY_ENV`] (MiB), plus
//! the variables the repository's `[resources.env]` derives from them
//! (`CARGO_BUILD_JOBS = "{cpus}"`, `PYTEST_XDIST_AUTO_NUM_WORKERS = "{cpus}"`).
//!
//! - A CI job gets what its `[[ci.jobs]]` entry declares (`cpus`, `memory_gb`), else an equal share
//!   of the capacity among its run's jobs; the app starts it only once that budget is free, so CI
//!   jobs never ask for more than the machine has, whatever the number of lanes.
//! - An agent session, terminal or Cockpit gets the agent budget: a third of the capacity by
//!   default (three of them testing at once fill the sandbox), or what the operator configured.
//!
//! Pure: the caller measures the capacity and applies the budget.

use std::fmt;

/// Environment variable with a process's CPU budget in whole CPUs.
pub const CPUS_ENV: &str = "YULIUS_CPUS";
/// Environment variable with a process's memory budget in MiB.
pub const MEMORY_ENV: &str = "YULIUS_MEMORY_MB";
/// Most `[resources.env]` entries.
pub const MAX_ENV_ENTRIES: usize = 32;
/// Longest `[resources.env]` variable name.
pub const MAX_ENV_NAME_BYTES: usize = 64;
/// Longest `[resources.env]` template.
pub const MAX_ENV_VALUE_BYTES: usize = 256;
/// Largest `cpus` a CI job may declare.
pub const MAX_DECLARED_CPUS: u64 = 1024;
/// Largest `memory_gb` a CI job may declare.
pub const MAX_DECLARED_MEMORY_GB: u64 = 4096;
/// The default agent budget is the capacity divided by this: three agents running their tests or
/// builds at once fill the sandbox (SPEC-082 RES-005).
pub const AGENT_SHARE: u32 = 3;
/// The default agent budget is never below this many CPUs (when the sandbox has them).
pub const AGENT_MIN_CPUS: u32 = 2;
/// Variables never set from `[resources.env]`: what Yulius sets itself, and what changes how the
/// agent CLI, Git, TLS, proxies, shells or interpreters run. The repository's file reaches the
/// agent CLI's own process (it holds the provider credential), so a variable that loads code or
/// redirects requests there (`NODE_OPTIONS`, `ANTHROPIC_BASE_URL`, `HTTPS_PROXY`) is refused.
const RESERVED: [&str; 29] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "TZ",
    "TMPDIR",
    "PORT",
    "CI",
    "NO_COLOR",
    "ENV",
    "BASH_ENV",
    "PROMPT_COMMAND",
    "PS4",
    "SHELLOPTS",
    "BASHOPTS",
    "IFS",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "FTP_PROXY",
];
/// Prefixes of the same.
const RESERVED_PREFIXES: [&str; 20] = [
    "YULIUS_",
    "GIT_",
    "GH_",
    "GITHUB_",
    "SSH_",
    "LD_",
    "DYLD_",
    "LC_",
    "XDG_",
    "ANTHROPIC_",
    "CLAUDE_",
    "NODE_",
    "NPM_CONFIG_",
    "BUN_",
    "DENO_",
    "PYTHON",
    "PERL",
    "RUBY",
    "SSL_",
    "OPENSSL_",
];

/// What a sandbox's processes may use together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    /// Whole CPUs (at least 1).
    pub cpus: u32,
    /// Memory in MiB (at least 1).
    pub memory_mb: u64,
}

/// What one process group may use: the size it plans its parallelism for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Whole CPUs (at least 1).
    pub cpus: u32,
    /// Memory in MiB (at least 1).
    pub memory_mb: u64,
}

impl fmt::Display for Budget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cpus = if self.cpus == 1 { "CPU" } else { "CPUs" };
        write!(f, "{} {cpus}, {} MiB", self.cpus, self.memory_mb)
    }
}

/// What a CI job declares in `[[ci.jobs]]`; `None` takes the default share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Need {
    /// `cpus`.
    pub cpus: Option<u32>,
    /// `memory_gb`, in MiB.
    pub memory_mb: Option<u64>,
}

impl Capacity {
    /// A capacity of at least one CPU and one MiB.
    #[must_use]
    pub fn new(cpus: u32, memory_mb: u64) -> Self {
        Self {
            cpus: cpus.max(1),
            memory_mb: memory_mb.max(1),
        }
    }

    /// Memory in proportion to `cpus` of the capacity's CPUs.
    fn memory_for(&self, cpus: u32) -> u64 {
        let share = u128::from(self.memory_mb) * u128::from(cpus) / u128::from(self.cpus);
        u64::try_from(share).unwrap_or(self.memory_mb).max(1)
    }

    /// The budget of one CI job of a run of `jobs` jobs: what it declares, else an equal share of
    /// the capacity among the run's jobs (at least one CPU); memory not declared follows the
    /// CPUs. Never more than the capacity, so a job that asks for more runs alone.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::resources::{Budget, Capacity, Need};
    /// let sandbox = Capacity::new(12, 48 * 1024);
    /// assert_eq!(
    ///     sandbox.ci_job(Need::default(), 2),
    ///     Budget { cpus: 6, memory_mb: 24 * 1024 }
    /// );
    /// let declared = Need { cpus: Some(4), memory_mb: None };
    /// assert_eq!(declared.cpus, Some(4));
    /// assert_eq!(sandbox.ci_job(declared, 2), Budget { cpus: 4, memory_mb: 16 * 1024 });
    /// ```
    #[must_use]
    pub fn ci_job(&self, need: Need, jobs: usize) -> Budget {
        let share = u32::try_from(jobs.max(1)).unwrap_or(u32::MAX);
        let cpus = need.cpus.unwrap_or(self.cpus / share).clamp(1, self.cpus);
        let memory_mb = need
            .memory_mb
            .unwrap_or_else(|| self.memory_for(cpus))
            .clamp(1, self.memory_mb);
        Budget { cpus, memory_mb }
    }

    /// The budget of an agent session, terminal or Cockpit: `configured` CPUs, else a third of
    /// the capacity ([`AGENT_SHARE`], rounded up, at least [`AGENT_MIN_CPUS`]); never more than
    /// the capacity. Memory follows the CPUs.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::resources::{Budget, Capacity};
    /// let sandbox = Capacity::new(12, 48 * 1024);
    /// assert_eq!(sandbox.agent(None), Budget { cpus: 4, memory_mb: 16 * 1024 });
    /// assert_eq!(sandbox.agent(Some(6)).cpus, 6);
    /// assert_eq!(Capacity::new(1, 2048).agent(None).cpus, 1);
    /// ```
    #[must_use]
    pub fn agent(&self, configured: Option<u32>) -> Budget {
        let cpus = configured
            .unwrap_or_else(|| self.cpus.div_ceil(AGENT_SHARE).max(AGENT_MIN_CPUS))
            .clamp(1, self.cpus);
        Budget {
            cpus,
            memory_mb: self.memory_for(cpus),
        }
    }
}

/// One `[resources.env]` entry: a variable the repository's tools read and its template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvTemplate {
    /// Variable name (`[A-Z_][A-Z0-9_]*`).
    pub name: String,
    /// Value with `{cpus}`, `{memory_mb}` or `{memory_gb}` placeholders.
    pub template: String,
}

/// Why a `[resources.env]` entry is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnvTemplateError {
    /// Not `[A-Z_][A-Z0-9_]*` or too long.
    #[error("use 1-64 uppercase letters, digits and '_', not starting with a digit")]
    InvalidName,
    /// A name Yulius sets itself or that changes how the agent CLI, Git, TLS, proxies, shells or
    /// interpreters run.
    #[error(
        "Yulius sets it, or it changes how the agent CLI, Git, TLS, proxies, shells or interpreters run (PATH, HOME, CI, YULIUS_*, GIT_*, ANTHROPIC_*, CLAUDE_*, NODE_*, PYTHON*, *_PROXY, LD_* and similar)"
    )]
    Reserved,
    /// Too long, a control character, or an unknown or unclosed placeholder.
    #[error(
        "use at most 256 bytes without control characters; placeholders: {{cpus}}, {{memory_mb}}, {{memory_gb}}"
    )]
    InvalidTemplate,
    /// More than [`MAX_ENV_ENTRIES`] entries.
    #[error("at most 32 variables")]
    TooMany,
}

/// Checks one `[resources.env]` entry.
///
/// # Errors
/// [`EnvTemplateError`] for the name or the template.
pub fn check_env_template(name: &str, template: &str) -> Result<(), EnvTemplateError> {
    let mut bytes = name.bytes();
    let valid_name = name.len() <= MAX_ENV_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if !valid_name {
        return Err(EnvTemplateError::InvalidName);
    }
    if RESERVED.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Err(EnvTemplateError::Reserved);
    }
    if template.len() > MAX_ENV_VALUE_BYTES
        || template.chars().any(char::is_control)
        || expand(
            template,
            Budget {
                cpus: 1,
                memory_mb: 1,
            },
        )
        .is_none()
    {
        return Err(EnvTemplateError::InvalidTemplate);
    }
    Ok(())
}

/// `template` with its placeholders replaced; `None` for an unknown or unclosed placeholder or
/// a stray `}`.
fn expand(template: &str, budget: Budget) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        if rest[open..].starts_with('}') {
            return None;
        }
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}')?;
        let value = match &after[..close] {
            "cpus" => budget.cpus.to_string(),
            "memory_mb" => budget.memory_mb.to_string(),
            "memory_gb" => (budget.memory_mb / 1024).max(1).to_string(),
            _ => return None,
        };
        out.push_str(&value);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The environment that tells a process its budget: [`CPUS_ENV`], [`MEMORY_ENV`] and each
/// template expanded, in order. A template that does not expand (never one that passed
/// [`check_env_template`]) is left out.
///
/// # Examples
/// ```
/// use yulius_domain::resources::{Budget, EnvTemplate, budget_env};
/// let env = budget_env(
///     Budget { cpus: 4, memory_mb: 8192 },
///     &[EnvTemplate { name: "MAKEFLAGS".into(), template: "-j{cpus}".into() }],
/// );
/// assert_eq!(
///     env,
///     [
///         ("YULIUS_CPUS".to_owned(), "4".to_owned()),
///         ("YULIUS_MEMORY_MB".to_owned(), "8192".to_owned()),
///         ("MAKEFLAGS".to_owned(), "-j4".to_owned()),
///     ]
/// );
/// ```
#[must_use]
pub fn budget_env(budget: Budget, templates: &[EnvTemplate]) -> Vec<(String, String)> {
    let mut env = vec![
        (CPUS_ENV.to_owned(), budget.cpus.to_string()),
        (MEMORY_ENV.to_owned(), budget.memory_mb.to_string()),
    ];
    for entry in templates.iter().take(MAX_ENV_ENTRIES) {
        if check_env_template(&entry.name, &entry.template).is_ok()
            && let Some(value) = expand(&entry.template, budget)
        {
            env.push((entry.name.clone(), value));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_jobs_share_the_capacity_unless_they_declare_their_need() {
        let sandbox = Capacity::new(10, 38 * 1024);
        assert_eq!(
            sandbox.ci_job(Need::default(), 2),
            Budget {
                cpus: 5,
                memory_mb: 19 * 1024
            },
            "two jobs share it equally"
        );
        assert_eq!(
            sandbox.ci_job(Need::default(), 0).cpus,
            10,
            "one job takes it"
        );
        assert_eq!(
            sandbox.ci_job(Need::default(), 40).cpus,
            1,
            "at least one CPU each"
        );
        let declared = Need {
            cpus: Some(6),
            memory_mb: Some(16 * 1024),
        };
        assert_eq!(
            sandbox.ci_job(declared, 2),
            Budget {
                cpus: 6,
                memory_mb: 16 * 1024
            }
        );
        let greedy = Need {
            cpus: Some(64),
            memory_mb: Some(1 << 30),
        };
        assert_eq!(
            sandbox.ci_job(greedy, 2),
            Budget {
                cpus: 10,
                memory_mb: 38 * 1024
            },
            "never more than the sandbox: such a job runs alone"
        );
    }

    #[test]
    fn an_agent_plans_for_a_third_of_the_sandbox() {
        assert_eq!(
            Capacity::new(10, 38 * 1024).agent(None),
            Budget {
                cpus: 4,
                memory_mb: 15_564
            }
        );
        assert_eq!(Capacity::new(4, 8192).agent(None).cpus, 2, "at least two");
        assert_eq!(
            Capacity::new(1, 8192).agent(None).cpus,
            1,
            "never more than there is"
        );
        assert_eq!(Capacity::new(12, 8192).agent(Some(0)).cpus, 1);
        assert_eq!(Capacity::new(12, 8192).agent(Some(99)).cpus, 12);
    }

    #[test]
    fn templates_name_the_budget_in_the_repositorys_terms() {
        let budget = Budget {
            cpus: 6,
            memory_mb: 3000,
        };
        let templates = [
            EnvTemplate {
                name: "NEXTEST_TEST_THREADS".into(),
                template: "{cpus}".into(),
            },
            EnvTemplate {
                name: "GRADLE_OPTS".into(),
                template: "-Xmx{memory_mb}m".into(),
            },
            EnvTemplate {
                name: "PYTEST_XDIST_AUTO_NUM_WORKERS".into(),
                template: "{cpus}".into(),
            },
            EnvTemplate {
                name: "BUILD_MEMORY".into(),
                template: "{memory_gb}G".into(),
            },
            EnvTemplate {
                name: "PATH".into(),
                template: "/evil".into(),
            },
            EnvTemplate {
                name: "NODE_OPTIONS".into(),
                template: "--require ./x.js".into(),
            },
        ];
        let env = budget_env(budget, &templates);
        assert_eq!(
            env,
            [
                ("YULIUS_CPUS", "6"),
                ("YULIUS_MEMORY_MB", "3000"),
                ("NEXTEST_TEST_THREADS", "6"),
                ("GRADLE_OPTS", "-Xmx3000m"),
                ("PYTEST_XDIST_AUTO_NUM_WORKERS", "6"),
                ("BUILD_MEMORY", "2G"),
            ]
            .map(|(a, b)| (a.to_owned(), b.to_owned())),
            "a reserved name is never set"
        );
    }

    #[test]
    fn invalid_templates_are_refused() {
        for (name, template, problem) in [
            ("lower", "{cpus}", EnvTemplateError::InvalidName),
            ("1ST", "{cpus}", EnvTemplateError::InvalidName),
            ("", "{cpus}", EnvTemplateError::InvalidName),
            ("PATH", "{cpus}", EnvTemplateError::Reserved),
            ("YULIUS_CPUS", "{cpus}", EnvTemplateError::Reserved),
            ("GIT_DIR", "x", EnvTemplateError::Reserved),
            ("LD_PRELOAD", "x", EnvTemplateError::Reserved),
            ("ANTHROPIC_BASE_URL", "x", EnvTemplateError::Reserved),
            ("CLAUDE_CODE_USE_BEDROCK", "1", EnvTemplateError::Reserved),
            ("NODE_OPTIONS", "-r x", EnvTemplateError::Reserved),
            ("HTTPS_PROXY", "x", EnvTemplateError::Reserved),
            ("PYTHONPATH", "x", EnvTemplateError::Reserved),
            ("BASH_ENV", "x", EnvTemplateError::Reserved),
            ("JOBS", "{threads}", EnvTemplateError::InvalidTemplate),
            ("JOBS", "{cpus", EnvTemplateError::InvalidTemplate),
            ("JOBS", "cpus}", EnvTemplateError::InvalidTemplate),
            ("JOBS", "a\nb", EnvTemplateError::InvalidTemplate),
        ] {
            assert_eq!(
                check_env_template(name, template),
                Err(problem),
                "{name}={template}"
            );
        }
        assert_eq!(check_env_template("MAKEFLAGS", "-j{cpus} -l{cpus}"), Ok(()));
        assert_eq!(check_env_template("PLAIN", "no placeholder"), Ok(()));
    }
}
