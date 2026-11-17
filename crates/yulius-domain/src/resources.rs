//! CPU and memory budgets of the processes Yulius starts in a sandbox (SPEC-082).
//!
//! A sandbox has a capacity: the CPUs and memory its processes may use (its limits, else the
//! host's). Yulius gives every process group it starts a budget out of it and says so the same way
//! in any language, in the environment: [`CPUS_ENV`] (whole CPUs) and [`MEMORY_ENV`] (MiB), plus
//! the variables the repository's `[resources.env]` derives from them
//! (`CARGO_BUILD_JOBS = "{cpus}"`, `PYTEST_XDIST_AUTO_NUM_WORKERS = "{cpus}"`).
//!
//! - A CI job gets what its `[[ci.jobs]]` entry declares (`cpus`, `memory_gb`), else an equal share
//!   of the capacity among its run's jobs: what it may use at once. The app starts it once the
//!   instance's CI budget has room for it by what CI was measured to use and the running jobs'
//!   budgets with its own stay within three times the CI budget
//!   ([`admit`], SPEC-497 HBA-002), so CI uses the host without taking more than it has,
//!   whatever the number of lanes or the declarations.
//! - An agent session, terminal or Cockpit gets the agent budget: a third of the capacity by
//!   default (three of them testing at once fill the sandbox), or what the operator configured.
//!
//! Pure: the caller measures the capacity and applies the budget.

use std::fmt;

use crate::ci::CiRef;

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

/// How many times the CI budget's CPUs the running jobs may use at once, their `cpus` or default
/// shares added up (SPEC-497 HBA-002): were every job to use all it may at the same moment, each
/// would still get a third of it. Measured use stops new jobs long before that: on 16–17 Nov 2026
/// twice the budget was the limit that held jobs back while the host was 45–77 % busy and CI's
/// processes waited for a CPU 1–6 % of the time.
pub const CI_OVERSUBSCRIPTION: u32 = 3;
/// A job that has just started counts, until the readings show it, as its budget but at most this
/// fraction of the CI budget (SPEC-497 HBA-002): its checkout and set-up come first, and jobs
/// starting together are never counted as more than the budget between two readings (four
/// starting at once may still use more until the next reading shows it).
pub const CI_START_SHARE: u32 = 4;

impl Capacity {
    /// What a job counts for between its start and the readings that show it (SPEC-497
    /// HBA-002): its budget, at most a [`CI_START_SHARE`]th of this CI budget, at least one CPU
    /// and one MiB.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::resources::{Budget, Capacity};
    /// let budget = Capacity::new(56, 448 * 1024);
    /// let whole = Budget { cpus: 56, memory_mb: 448 * 1024 };
    /// assert_eq!(budget.start_allowance(whole), Budget { cpus: 14, memory_mb: 112 * 1024 });
    /// let small = Budget { cpus: 1, memory_mb: 512 };
    /// assert_eq!(budget.start_allowance(small), small);
    /// ```
    #[must_use]
    pub fn start_allowance(&self, need: Budget) -> Budget {
        Budget {
            cpus: need.cpus.min(self.cpus / CI_START_SHARE).max(1),
            memory_mb: need
                .memory_mb
                .min(self.memory_mb / u64::from(CI_START_SHARE))
                .max(1),
        }
    }
}

/// Which waiting jobs start first (SPEC-497 HBA-002): those a merge waits for, then a pull
/// request's, then a branch's; in each, the one waiting longest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CiPriority {
    /// A merge train's or a playground task's merged result: a merge waits for it.
    Merge,
    /// A pull request's head: a review or an armed merge waits for it.
    PullRequest,
    /// A branch head after a push: nothing waits for it.
    Branch,
}

impl CiPriority {
    /// The priority of a run of `reference`.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::ci::CiRef;
    /// use yulius_domain::resources::CiPriority;
    /// assert_eq!(CiPriority::of(&CiRef::Train(vec![1, 2])), CiPriority::Merge);
    /// assert_eq!(CiPriority::of(&CiRef::PullRequest(7)), CiPriority::PullRequest);
    /// assert_eq!(CiPriority::of(&CiRef::Branch("dev".into())), CiPriority::Branch);
    /// ```
    #[must_use]
    pub fn of(reference: &CiRef) -> Self {
        match reference {
            CiRef::Train(_) | CiRef::Task(_) => Self::Merge,
            CiRef::PullRequest(_) => Self::PullRequest,
            CiRef::Branch(_) => Self::Branch,
        }
    }
}

/// What CI holds when a job asks to start (SPEC-497 HBA-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held {
    /// CI jobs running now, in every sandbox.
    pub jobs: u32,
    /// What they may use at once: the sum of their budgets (`cpus` or default share; memory).
    pub reserved: Budget,
    /// What the jobs that started too recently to show in the readings may still bring: the sum
    /// of their start allowances ([`Capacity::start_allowance`]).
    pub starting: Budget,
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
    /// The running jobs and this one could use more CPUs at once than [`CI_OVERSUBSCRIPTION`]
    /// times the budget.
    WorstCase {
        /// What the running jobs may use at once.
        reserved: u64,
        /// This job's CPUs.
        need: u64,
        /// The most they may add up to.
        limit: u64,
    },
    /// What CI uses and what starting jobs may still bring leave no room for this job's start,
    /// all in CPUs or MiB.
    Load {
        /// What is short.
        short: Short,
        /// What CI uses: measured, or the running jobs' reservations where nothing is measured.
        used: u64,
        /// What jobs that just started may still bring.
        starting: u64,
        /// What this job starts with.
        need: u64,
        /// The CI budget.
        budget: u64,
    },
}

impl fmt::Display for Admission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Now => f.write_str("it fits"),
            Self::WorstCase {
                reserved,
                need,
                limit,
            } => write!(
                f,
                "the running jobs may use {reserved} CPUs at once, and with this job's {need} \
                 that is more than {limit}, three times the host's CPUs for CI"
            ),
            Self::Load {
                short,
                used,
                starting,
                need,
                budget,
            } => {
                let unit = match short {
                    Short::Cpu => "CPUs",
                    Short::Memory => "MiB of memory",
                };
                write!(f, "CI uses {used} of the host's {budget} {unit} for CI")?;
                if starting > 0 {
                    write!(f, " and jobs that just started may bring {starting} more")?;
                }
                write!(f, "; this job starts with {need}")
            }
        }
    }
}

/// Whether a job needing `need` starts beside what CI holds (SPEC-497 HBA-002). With no CI job
/// running anything starts. Otherwise, in this order:
///
/// 1. the CPUs the running jobs and this one may use at once are at most
///    [`CI_OVERSUBSCRIPTION`] times the budget;
/// 2. what CI was measured to use, what jobs that just started may still bring and this job's
///    start allowance fit in the budget, in CPUs and in memory. Where nothing is measured the
///    reservations decide: the running jobs' budgets and this job's whole budget.
///
/// A need larger than the budget counts as the whole budget. CPU pressure (`cpu.pressure`) is no
/// part of it: a job that confines itself to a few CPUs (`taskset`) shows pressure while the
/// host idles, and held every sandbox's jobs back (17 Nov 2026); measured use already stops new
/// jobs when CI really fills the host.
///
/// # Examples
/// ```
/// use yulius_domain::resources::{Admission, Budget, Capacity, Held, admit};
/// let budget = Capacity::new(56, 448 * 1024);
/// let need = Budget { cpus: 16, memory_mb: 32 * 1024 };
/// let reserved = Budget { cpus: 48, memory_mb: 96 * 1024 };
/// let used = |cpus| Some(Budget { cpus, memory_mb: 8 * 1024 });
/// // Three jobs reserved 48 CPUs but use 12: a fourth starts.
/// let held = Held { jobs: 3, reserved, measured: used(12), ..Held::default() };
/// assert_eq!(admit(budget, &held, need), Admission::Now);
/// // They use 50: it waits.
/// let held = Held { jobs: 3, reserved, measured: used(50), ..Held::default() };
/// assert!(matches!(admit(budget, &held, need), Admission::Load { used: 50, .. }));
/// ```
#[must_use]
pub fn admit(budget: Capacity, held: &Held, need: Budget) -> Admission {
    if held.jobs == 0 {
        return Admission::Now;
    }
    let need = Budget {
        cpus: need.cpus.clamp(1, budget.cpus.max(1)),
        memory_mb: need.memory_mb.clamp(1, budget.memory_mb.max(1)),
    };
    let reserved = u64::from(held.reserved.cpus);
    let limit = u64::from(budget.cpus) * u64::from(CI_OVERSUBSCRIPTION);
    if reserved + u64::from(need.cpus) > limit {
        return Admission::WorstCase {
            reserved,
            need: u64::from(need.cpus),
            limit,
        };
    }
    // Measured: the readings, what starting jobs may still bring, and this job's start. Not
    // measured: the reservations and this job's whole budget.
    let (used, starting, start) = match held.measured {
        Some(measured) => (measured, held.starting, budget.start_allowance(need)),
        None => (held.reserved, Budget::default(), need),
    };
    let checks = [
        (
            Short::Cpu,
            u64::from(used.cpus),
            u64::from(starting.cpus),
            u64::from(start.cpus),
            u64::from(budget.cpus),
        ),
        (
            Short::Memory,
            used.memory_mb,
            starting.memory_mb,
            start.memory_mb,
            budget.memory_mb,
        ),
    ];
    for (short, used, starting, need, budget) in checks {
        if used.saturating_add(starting).saturating_add(need) > budget {
            return Admission::Load {
                short,
                used,
                starting,
                need,
                budget,
            };
        }
    }
    Admission::Now
}

/// One minute's reading of the host for CI (SPEC-497): how busy its CPUs were over the minute
/// (per mille), the CPUs running CI jobs reserved, what their sandboxes' CI and engine groups
/// were measured to use, and how many CI jobs waited to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadMinute {
    /// The minute's start.
    pub at: crate::time::Timestamp,
    /// The host's CPUs busy over the minute, per mille; `None` where the host does not say.
    pub busy_permille: Option<u32>,
    /// CPUs the running CI jobs reserved.
    pub held_cpus: u32,
    /// CPUs their sandboxes' CI and engine groups used, when measured.
    pub measured_cpus: Option<u32>,
    /// CI jobs of running runs that had not started (waiting for a turn, networks, a slot or
    /// the budget).
    pub waiting_jobs: u32,
}

/// One hour of [`LoadMinute`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadHour {
    /// `YYYY-MM-DDTHH` (UTC).
    pub hour: String,
    /// Minutes read.
    pub minutes: u32,
    /// Job-minutes CI jobs waited to start (each read minute's waiting jobs).
    pub wait_job_minutes: u32,
    /// The host's CPUs busy, in percent, averaged over the minutes that say.
    pub busy_percent: Option<f64>,
    /// CPUs reserved by running CI jobs, averaged.
    pub held_cpus: f64,
    /// CPUs measured, averaged over the minutes that say.
    pub measured_cpus: Option<f64>,
}

/// The host's CPUs busy between two readings of `(busy, total)` time counters, per mille;
/// `None` when the counters did not move forward.
///
/// # Examples
/// ```
/// use yulius_domain::resources::busy_permille;
/// assert_eq!(busy_permille((100, 1000), (400, 2000)), Some(300));
/// assert_eq!(busy_permille((100, 1000), (100, 1000)), None);
/// ```
#[must_use]
pub fn busy_permille(before: (u64, u64), after: (u64, u64)) -> Option<u32> {
    let total = after.1.checked_sub(before.1).filter(|t| *t > 0)?;
    let busy = after.0.checked_sub(before.0)?.min(total);
    u32::try_from(busy * 1000 / total).ok()
}

/// `minutes` by UTC hour, oldest first.
#[must_use]
pub fn load_by_hour(minutes: &[LoadMinute]) -> Vec<LoadHour> {
    let mut hours: std::collections::BTreeMap<String, Vec<&LoadMinute>> =
        std::collections::BTreeMap::new();
    for minute in minutes {
        let hour = crate::time::format_rfc3339(minute.at)
            .chars()
            .take(13)
            .collect::<String>();
        hours.entry(hour).or_default().push(minute);
    }
    let mean = |values: Vec<f64>| {
        #[expect(clippy::cast_precision_loss, reason = "minutes in an hour")]
        let n = values.len() as f64;
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / n)
    };
    hours
        .into_iter()
        .map(|(hour, minutes)| LoadHour {
            hour,
            minutes: u32::try_from(minutes.len()).unwrap_or(u32::MAX),
            wait_job_minutes: minutes.iter().map(|m| m.waiting_jobs).sum(),
            busy_percent: mean(
                minutes
                    .iter()
                    .filter_map(|m| m.busy_permille)
                    .map(|p| f64::from(p) / 10.0)
                    .collect(),
            ),
            held_cpus: mean(minutes.iter().map(|m| f64::from(m.held_cpus)).collect())
                .unwrap_or(0.0),
            measured_cpus: mean(
                minutes
                    .iter()
                    .filter_map(|m| m.measured_cpus)
                    .map(f64::from)
                    .collect(),
            ),
        })
        .collect()
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

    /// SPEC-497: a host's minutes by hour: waiting job-minutes add up, busy and CPUs average
    /// over the minutes that say.
    #[test]
    fn the_hosts_minutes_add_up_by_hour() {
        let minute =
            |at: i64, busy: Option<u32>, held: u32, measured: Option<u32>, waiting: u32| {
                LoadMinute {
                    at: crate::time::Timestamp(at * 60_000),
                    busy_permille: busy,
                    held_cpus: held,
                    measured_cpus: measured,
                    waiting_jobs: waiting,
                }
            };
        let hours = load_by_hour(&[
            minute(0, Some(250), 50, Some(16), 8),
            minute(1, Some(350), 56, None, 10),
            minute(2, None, 56, Some(20), 4),
            minute(60, Some(100), 0, None, 0),
        ]);
        assert_eq!(hours.len(), 2);
        assert_eq!(
            hours[0],
            LoadHour {
                hour: "1970-01-01T00".to_owned(),
                minutes: 3,
                wait_job_minutes: 22,
                busy_percent: Some(30.0),
                held_cpus: 54.0,
                measured_cpus: Some(18.0),
            }
        );
        assert_eq!(hours[1].measured_cpus, None);
    }

    /// SPEC-497 HBA-001, HBA-002: the reserve of the server and agents; what CI uses decides,
    /// with what starting jobs may still bring; the worst case is bounded at three times the
    /// budget; memory as well as CPUs; without readings the reservations decide; a job larger
    /// than the budget counts as the whole budget.
    #[test]
    fn ci_jobs_are_admitted_by_what_ci_uses_within_a_bounded_worst_case() {
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
            ..Held::default()
        };
        // 16 Nov 2026: running jobs reserved 48 of 56 CPUs and used about 12: a fourth starts.
        assert_eq!(admit(budget, &held(3, 48, Some(12)), need), Admission::Now);
        // Without readings the reservations decide, as before.
        assert!(matches!(
            admit(budget, &held(3, 48, None), need),
            Admission::Load {
                short: Short::Cpu,
                used: 48,
                ..
            }
        ));
        assert_eq!(admit(budget, &held(2, 32, None), need), Admission::Now);
        // Measured use leaves no room for its start (a quarter of the budget at most: 14).
        let busy = admit(budget, &held(2, 32, Some(45)), need);
        assert_eq!(
            busy,
            Admission::Load {
                short: Short::Cpu,
                used: 45,
                starting: 0,
                need: 14,
                budget: 56
            }
        );
        assert_eq!(
            busy.to_string(),
            "CI uses 45 of the host's 56 CPUs for CI; this job starts with 14"
        );
        assert_eq!(admit(budget, &held(2, 32, Some(42)), need), Admission::Now);
        // Jobs that just started count until the readings show them.
        let starting = Held {
            starting: Budget {
                cpus: 28,
                memory_mb: 2048,
            },
            ..held(4, 64, Some(20))
        };
        let wait = admit(budget, &starting, need);
        assert!(
            matches!(wait, Admission::Load { starting: 28, .. }),
            "{wait:?}"
        );
        assert_eq!(
            wait.to_string(),
            "CI uses 20 of the host's 56 CPUs for CI and jobs that just started may bring 28 \
             more; this job starts with 14"
        );
        // The worst case: the running jobs may use 160 CPUs at once; 16 more is over 168.
        let worst = admit(budget, &held(7, 160, Some(10)), need);
        assert_eq!(
            worst,
            Admission::WorstCase {
                reserved: 160,
                need: 16,
                limit: 168
            }
        );
        assert_eq!(
            worst.to_string(),
            "the running jobs may use 160 CPUs at once, and with this job's 16 that is more \
             than 168, three times the host's CPUs for CI"
        );
        assert_eq!(admit(budget, &held(7, 152, Some(10)), need), Admission::Now);
        // 17 Nov 2026: three quartio-v2 jobs of 30 CPUs held back a fourth at twice the budget
        // (90 + 30 > 112) while the host was 55 % busy; at three times it starts.
        assert_eq!(
            admit(
                budget,
                &held(3, 90, Some(20)),
                Budget {
                    cpus: 30,
                    memory_mb: 1024
                }
            ),
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
            ..Held::default()
        };
        let wait = admit(
            budget,
            &big,
            Budget {
                cpus: 1,
                memory_mb: 16 * 1024,
            },
        );
        assert!(
            matches!(
                wait,
                Admission::Load {
                    short: Short::Memory,
                    ..
                }
            ),
            "{wait:?}"
        );
        // A job larger than the budget counts as the whole budget: it starts on an idle CI, and
        // beside running jobs by the readings, starting with a quarter of it.
        let huge = Budget {
            cpus: 200,
            memory_mb: 1,
        };
        assert_eq!(admit(budget, &held(0, 0, Some(64)), huge), Admission::Now);
        assert_eq!(admit(budget, &held(1, 21, Some(5)), huge), Admission::Now);
        assert!(matches!(
            admit(budget, &held(1, 1, None), huge),
            Admission::Load { need: 56, .. }
        ));
        assert!(matches!(
            admit(budget, &held(5, 120, Some(5)), huge),
            Admission::WorstCase { need: 56, .. }
        ));
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
                assert_eq!(admit(budget, &held, share), Admission::Now, "{jobs} jobs");
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
