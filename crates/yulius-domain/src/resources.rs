//! CPU and memory budgets of the processes Yulius starts in a sandbox (SPEC-082).
//!
//! A sandbox has a capacity: the CPUs and memory its processes may use (its limits, else the
//! host's). Yulius gives every process group it starts a budget out of it and says so the same way
//! in any language, in the environment: [`CPUS_ENV`] (whole CPUs) and [`MEMORY_ENV`] (MiB), plus
//! the variables the repository's `[resources.env]` derives from them
//! (`CARGO_BUILD_JOBS = "{cpus}"`, `PYTEST_XDIST_AUTO_NUM_WORKERS = "{cpus}"`).
//!
//! - A CI job gets what its `[[ci.jobs]]` entry declares (`cpus`, `memory_gb`), else an equal share
//!   of the capacity among its run's jobs; the app starts it only once that budget fits in the
//!   instance's CI budget beside what CI holds, reserved or measured ([`admit`], SPEC-497), so CI
//!   jobs never take more than the machine has, whatever the number of lanes or their
//!   declarations.
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
/// A CI job that declares no `cpus` gets at most this fraction of the capacity it is shared from
/// (SPEC-497: autoresearch's one-job `gate` reserved all 64 CPUs for a 0.1 s run).
pub const DEFAULT_SHARE_MAX: u32 = 4;
/// The largest overcommit factor of `YULIUS_CI_OVERCOMMIT` (SPEC-497).
pub const MAX_OVERCOMMIT: u32 = 8;
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

/// What one process group may use: the size it plans its parallelism for (or, summed, what
/// several hold: zero for none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    /// Whole CPUs (at least 1 for one process group).
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
    /// the capacity among the run's jobs, at most a [`DEFAULT_SHARE_MAX`]th of it (SPEC-497: a
    /// one-job run reserved the whole host and used a fraction), at least one CPU; memory not
    /// declared follows the CPUs. Never more than the capacity, so a job that asks for more runs
    /// alone.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::resources::{Budget, Capacity, Need};
    /// let sandbox = Capacity::new(12, 48 * 1024);
    /// assert_eq!(
    ///     sandbox.ci_job(Need::default(), 2),
    ///     Budget { cpus: 3, memory_mb: 12 * 1024 }
    /// );
    /// assert_eq!(sandbox.ci_job(Need::default(), 6).cpus, 2);
    /// let declared = Need { cpus: Some(4), memory_mb: None };
    /// assert_eq!(declared.cpus, Some(4));
    /// assert_eq!(sandbox.ci_job(declared, 2), Budget { cpus: 4, memory_mb: 16 * 1024 });
    /// ```
    #[must_use]
    pub fn ci_job(&self, need: Need, jobs: usize) -> Budget {
        let share = u32::try_from(jobs.max(1)).unwrap_or(u32::MAX);
        let cpus = need
            .cpus
            .unwrap_or((self.cpus / share).min(self.cpus / DEFAULT_SHARE_MAX))
            .clamp(1, self.cpus);
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

/// The share of the capacity CI never takes: the server and the agent sessions keep an eighth of
/// the CPUs and memory (SPEC-497 HBA-001), at least [`CI_RESERVE_MIN_CPUS`] and
/// [`CI_RESERVE_MIN_MB`].
pub const CI_RESERVE_SHARE: u32 = 8;
/// The fewest CPUs kept from CI.
pub const CI_RESERVE_MIN_CPUS: u32 = 1;
/// The least memory kept from CI, in MiB.
pub const CI_RESERVE_MIN_MB: u64 = 2048;

impl Capacity {
    /// What the CI jobs of every sandbox may hold together (SPEC-497 HBA-001): the capacity less
    /// the reserve of the server and the agent sessions ([`CI_RESERVE_SHARE`]), at least one CPU
    /// and one MiB.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::resources::Capacity;
    /// assert_eq!(Capacity::new(64, 512 * 1024).ci_budget(), Capacity::new(56, 448 * 1024));
    /// assert_eq!(Capacity::new(4, 8192).ci_budget(), Capacity::new(3, 6144));
    /// assert_eq!(Capacity::new(1, 2048).ci_budget(), Capacity::new(1, 1024));
    /// ```
    #[must_use]
    pub fn ci_budget(&self) -> Self {
        let cpus = (self.cpus / CI_RESERVE_SHARE).max(CI_RESERVE_MIN_CPUS);
        // At most half: a small host's jobs keep memory to run in (review round 2).
        let memory = (self.memory_mb / u64::from(CI_RESERVE_SHARE))
            .max(CI_RESERVE_MIN_MB)
            .min(self.memory_mb / 2);
        Self::new(
            self.cpus.saturating_sub(cpus),
            self.memory_mb.saturating_sub(memory),
        )
    }
}

/// The fewest networks a sandbox's Docker engine must still be able to make for a CI job of it
/// to start (SPEC-497 HBA-003): a test run creating networks side by side with others never
/// meets an engine that has none left.
pub const ENGINE_NETWORK_RESERVE: u32 = 32;

/// The fewest free networks a job needs from an engine whose pools hold `capacity` (SPEC-497
/// HBA-003): [`ENGINE_NETWORK_RESERVE`], or a quarter of a smaller engine's (one started with
/// Docker's default pools, 31 networks, needs 7), at least one.
///
/// # Examples
/// ```
/// use yulius_domain::resources::network_reserve;
/// assert_eq!(network_reserve(4096), 32);
/// assert_eq!(network_reserve(31), 7);
/// assert_eq!(network_reserve(2), 1);
/// ```
#[must_use]
pub fn network_reserve(capacity: u32) -> u32 {
    ENGINE_NETWORK_RESERVE.min(capacity / 4).max(1)
}

/// What CI holds when a job asks to start (SPEC-497 HBA-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held {
    /// CI jobs running now, in every sandbox.
    pub jobs: u32,
    /// The sum of their reservations.
    pub reserved: Budget,
    /// What the `ci` and `engine` groups of the sandboxes running them were measured to use (CPU
    /// time per second, rounded up; anonymous memory); `None` when nothing was measured.
    pub measured: Option<Budget>,
}

/// The part of the CI budget a job waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Short {
    /// CPUs.
    Cpu,
    /// Memory.
    Memory,
}

/// Whether a job starts now or waits, and for what (SPEC-497 HBA-002, HBA-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// It starts.
    Now,
    /// It waits: CI holds `held` (the larger of `reserved` and `measured`) of `budget`, and the
    /// job needs `need`, all in CPUs or MiB.
    Wait {
        /// What is short.
        short: Short,
        /// What CI holds.
        held: u64,
        /// The running jobs' reservations.
        reserved: u64,
        /// What was measured, if anything.
        measured: Option<u64>,
        /// The job's need.
        need: u64,
        /// The CI budget.
        budget: u64,
    },
}

impl fmt::Display for Admission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Now => f.write_str("it fits"),
            Self::Wait {
                short,
                held,
                reserved,
                measured,
                need,
                budget,
            } => {
                let unit = match short {
                    Short::Cpu => "CPUs",
                    Short::Memory => "MiB of memory",
                };
                write!(
                    f,
                    "CI holds {held} of the host's {budget} {unit} for CI ({reserved} reserved"
                )?;
                if let Some(measured) = measured {
                    write!(f, ", {measured} measured")?;
                }
                write!(f, "); this job needs {need}")
            }
        }
    }
}

/// Whether a job needing `need` starts beside what CI holds (SPEC-497 HBA-002): it does when it
/// fits in `budget` beside the larger of the running jobs' reservations and the measured use, in
/// CPUs and in memory. A need larger than the budget counts as the whole budget, so such a job
/// runs alone; with no CI job running anything starts (what is measured then is not CI's).
///
/// # Examples
/// ```
/// use yulius_domain::resources::{Admission, Budget, Capacity, Held, admit};
/// let budget = Capacity::new(56, 448 * 1024);
/// let need = Budget { cpus: 16, memory_mb: 32 * 1024 };
/// let reserved = Budget { cpus: 32, memory_mb: 64 * 1024 };
/// // The two running jobs reserved 32 CPUs but use 50: a third does not fit.
/// let held = Held { jobs: 2, reserved, measured: Some(Budget { cpus: 50, memory_mb: 1024 }) };
/// assert!(matches!(admit(budget, &held, need, 1), Admission::Wait { held: 50, .. }));
/// // Measured below their reservation, the reservation counts: 32 + 16 fit.
/// let held = Held { jobs: 2, reserved, measured: Some(Budget { cpus: 20, memory_mb: 1024 }) };
/// assert_eq!(admit(budget, &held, need, 1), Admission::Now);
/// ```
#[must_use]
pub fn admit(budget: Capacity, held: &Held, need: Budget, overcommit: u32) -> Admission {
    if held.jobs == 0 {
        return Admission::Now;
    }
    // Reservations count at 1/overcommit (`YULIUS_CI_OVERCOMMIT`, the production hotfix of
    // 16 Nov 2026 carried forward): what jobs declare is about four times what they use, and
    // measured use, at face value, still guards the host.
    let factor = u64::from(overcommit.clamp(1, MAX_OVERCOMMIT));
    let scaled = |n: u64| n.div_ceil(factor);
    let measured = held.measured;
    let checks = [
        (
            Short::Cpu,
            u64::from(held.reserved.cpus),
            measured.map(|m| u64::from(m.cpus)),
            u64::from(need.cpus),
            u64::from(budget.cpus),
        ),
        (
            Short::Memory,
            held.reserved.memory_mb,
            measured.map(|m| m.memory_mb),
            need.memory_mb,
            budget.memory_mb,
        ),
    ];
    for (short, reserved, measured, need, budget) in checks {
        let need = scaled(need.min(budget));
        let reserved = scaled(reserved);
        let held = reserved.max(measured.unwrap_or(0));
        if held.saturating_add(need) > budget {
            return Admission::Wait {
                short,
                held,
                reserved,
                measured,
                need,
                budget,
            };
        }
    }
    Admission::Now
}

/// What a repository without a `[resources.env]` table gets (SPEC-268 PID-007): Cargo's build
/// jobs, nextest's and libtest's test threads at the budget's CPUs. Other tools ignore them, so a
/// repository in another language is not affected; one with a table, even empty, gets its own.
pub const DEFAULT_ENV: [(&str, &str); 3] = [
    ("CARGO_BUILD_JOBS", "{cpus}"),
    ("NEXTEST_TEST_THREADS", "{cpus}"),
    ("RUST_TEST_THREADS", "{cpus}"),
];

/// [`DEFAULT_ENV`] as templates.
#[must_use]
pub fn default_templates() -> Vec<EnvTemplate> {
    DEFAULT_ENV
        .iter()
        .map(|(name, template)| EnvTemplate {
            name: (*name).to_owned(),
            template: (*template).to_owned(),
        })
        .collect()
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

    /// SPEC-497 HBA-001, HBA-002: the reserve of the server and agents, the larger of reserved
    /// and measured, memory as well as CPUs, and a job larger than the budget alone.
    #[test]
    fn ci_jobs_are_admitted_by_what_ci_really_holds() {
        let budget = Capacity::new(64, 512 * 1024).ci_budget();
        assert_eq!(budget, Capacity::new(56, 448 * 1024));
        let need = Budget {
            cpus: 16,
            memory_mb: 1024,
        };
        let held = |jobs, reserved: u32, measured: Option<u32>| Held {
            jobs,
            reserved: Budget {
                cpus: reserved,
                memory_mb: 1024,
            },
            measured: measured.map(|cpus| Budget {
                cpus,
                memory_mb: 1024,
            }),
        };
        // Four 16-CPU jobs by their declarations would take 64; the budget is 56.
        assert_eq!(admit(budget, &held(2, 32, None), need, 1), Admission::Now);
        assert!(matches!(
            admit(budget, &held(3, 48, None), need, 1),
            Admission::Wait {
                short: Short::Cpu,
                held: 48,
                ..
            }
        ));
        // What two jobs really use decides when it is more than they reserved.
        let measured = admit(budget, &held(2, 32, Some(45)), need, 1);
        assert!(
            matches!(
                measured,
                Admission::Wait {
                    held: 45,
                    measured: Some(45),
                    ..
                }
            ),
            "{measured:?}"
        );
        assert_eq!(
            measured.to_string(),
            "CI holds 45 of the host's 56 CPUs for CI (32 reserved, 45 measured); this job needs 16"
        );
        assert_eq!(
            admit(budget, &held(2, 32, Some(40)), need, 1),
            Admission::Now
        );
        // Memory too.
        let big = Held {
            jobs: 1,
            reserved: Budget {
                cpus: 1,
                memory_mb: 1024,
            },
            measured: Some(Budget {
                cpus: 1,
                memory_mb: 440 * 1024,
            }),
        };
        let wait = admit(
            budget,
            &big,
            Budget {
                cpus: 1,
                memory_mb: 16 * 1024,
            },
            1,
        );
        assert!(
            matches!(
                wait,
                Admission::Wait {
                    short: Short::Memory,
                    ..
                }
            ),
            "{wait:?}"
        );
        // A job larger than the budget runs alone, and nothing running admits anything.
        let huge = Budget {
            cpus: 200,
            memory_mb: 1024,
        };
        assert_eq!(
            admit(budget, &held(0, 0, Some(64)), huge, 1),
            Admission::Now
        );
        assert!(matches!(
            admit(budget, &held(1, 1, None), huge, 1),
            Admission::Wait { need: 56, .. }
        ));
    }

    /// SPEC-497 (the production hotfix carried forward): with an overcommit factor of 2, idle
    /// reservations block half as much, the need counts at half, and measured use still at face
    /// value; the factor is at most 8.
    #[test]
    fn reservations_count_at_one_over_the_overcommit_factor() {
        let budget = Capacity::new(64, 512 * 1024).ci_budget();
        let need = Budget {
            cpus: 21,
            memory_mb: 1,
        };
        let held = |reserved: u32, measured: Option<u32>| Held {
            jobs: 3,
            reserved: Budget {
                cpus: reserved,
                memory_mb: 1,
            },
            measured: measured.map(|cpus| Budget { cpus, memory_mb: 1 }),
        };
        // 63 reserved by three idle jobs: nothing more by reservations alone.
        assert!(matches!(
            admit(budget, &held(63, Some(10)), need, 1),
            Admission::Wait { .. }
        ));
        // At 2: 32 + 11 of 56.
        assert_eq!(admit(budget, &held(63, Some(10)), need, 2), Admission::Now);
        // Measured use still guards the host: 50 measured + 11 > 56.
        let wait = admit(budget, &held(63, Some(50)), need, 2);
        assert!(
            matches!(
                wait,
                Admission::Wait {
                    held: 50,
                    need: 11,
                    reserved: 32,
                    ..
                }
            ),
            "{wait:?}"
        );
        // 0 counts as 1, and more than 8 as 8.
        assert_eq!(
            admit(budget, &held(63, Some(10)), need, 0),
            admit(budget, &held(63, Some(10)), need, 1)
        );
        assert_eq!(
            admit(budget, &held(440, Some(1)), need, 100),
            admit(budget, &held(440, Some(1)), need, 8)
        );
    }

    /// SPEC-497 (review round 1): a run's default shares are of the CI budget, so they start
    /// together on an idle host; shares of the capacity added up to more than the budget.
    #[test]
    fn a_runs_default_shares_start_together() {
        let budget = Capacity::new(64, 512 * 1024).ci_budget();
        for jobs in 1..=8 {
            let share = budget.ci_job(Need::default(), jobs);
            let mut held = Held::default();
            for _ in 0..jobs {
                assert_eq!(
                    admit(budget, &held, share, 1),
                    Admission::Now,
                    "{jobs} jobs"
                );
                held.jobs += 1;
                held.reserved.cpus += share.cpus;
                held.reserved.memory_mb += share.memory_mb;
            }
        }
    }

    #[test]
    fn ci_jobs_share_the_capacity_unless_they_declare_their_need() {
        let sandbox = Capacity::new(10, 38 * 1024);
        assert_eq!(
            sandbox.ci_job(Need::default(), 5),
            Budget {
                cpus: 2,
                memory_mb: 38 * 1024 / 5
            },
            "five jobs share it equally"
        );
        assert_eq!(
            sandbox.ci_job(Need::default(), 0).cpus,
            2,
            "one job takes a quarter at most (SPEC-497)"
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
