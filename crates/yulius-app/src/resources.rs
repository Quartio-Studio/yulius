//! CPU and memory budgets of a sandbox's processes (SPEC-082, ADR-0065).
//!
//! The capacity is what a sandbox's processes may use together: its enforced limits (the
//! isolation adapter's `cpu.max` and `memory.max`), else the host's CPUs and 2 GiB of memory
//! per CPU. Yulius gives out budgets from it and says so in the environment
//! ([`yulius_domain::resources::budget_env`]): CI jobs get their declared or default share and
//! start once the host has room for them by what CI was measured to use, merge candidates' jobs
//! first and never behind a job that does not fit (`App::ci_reserve`, SPEC-497 HBA-002); agent
//! runs, terminals and the Cockpit get the agent budget
//! ([`App::agent_resource_env`]). The repository maps the budget to its tools in
//! `[resources.env]` of the `yulius.toml` where the process starts.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;
use yulius_domain::ids::SandboxId;
use yulius_domain::resources::{
    Admission, Budget, Capacity, CiPriority, EnvTemplate, Held, LoadMinute, admit, budget_env,
    busy_permille, default_templates, network_reserve,
};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml_resources_table};
use yulius_domain::time::Timestamp;

use crate::context::App;

/// Memory assumed per CPU when no limit says how much there is (development hosts).
const MEMORY_PER_CPU_MB: u64 = 2048;
/// The shortest time between two readings of a sandbox's CI and engine groups that gives their
/// CPU rate (SPEC-497 HBA-002), and how often a waiting job looks again.
pub(crate) const SAMPLE_WINDOW: Duration = Duration::from_secs(5);
/// A reading older than this is no base for a rate: what was used since is a new window. Twice
/// the dispatcher's tick, which reads the sandboxes running jobs at each pass.
const STALE_WINDOW: Duration = Duration::from_secs(30);
/// How long a started job counts with its start allowance beside the readings (SPEC-497
/// HBA-002): a reading spans 5 to 30 seconds, so within a minute what it uses is in them.
const START_WINDOW: Duration = Duration::from_secs(60);
/// How long a waiting job lets later ones that fit start before it (SPEC-497 HBA-002). Past
/// this, nothing after it in the order (its priority and lower) starts until it has; a job of a
/// higher priority still goes first.
const OVERTAKE_LIMIT: Duration = Duration::from_secs(10 * 60);

/// The CI budget of the whole instance and what CI holds of it (SPEC-082 RES-003, SPEC-497
/// HBA-001, HBA-002): one for every sandbox, sized once from the capacity.
pub(crate) struct CiAdmission {
    state: Mutex<AdmissionState>,
    /// Told when a job starts, ends or leaves the queue.
    changed: Notify,
}

struct AdmissionState {
    budget: Capacity,
    /// The running jobs and the sum of their budgets; what starts, is measured and waits for a
    /// CPU is filled in at each decision.
    held: Held,
    next_ticket: u64,
    /// The waiting jobs, in no order: [`Waiter::order`] orders them.
    queue: Vec<Waiter>,
    /// The running jobs' start allowances and when they started (SPEC-497 HBA-002).
    started: Vec<Started>,
    /// Each measured sandbox's last reading (when, CPU microseconds), its CPU rate over the
    /// window before it (CPUs, once two readings were far enough apart) and its memory.
    samples: HashMap<SandboxId, Sample>,
}

/// A waiting job.
#[derive(Debug, Clone, Copy)]
struct Waiter {
    id: u64,
    priority: CiPriority,
    since: Instant,
    need: Budget,
}

impl Waiter {
    /// The order waiting jobs start in (SPEC-497 HBA-002): by priority, then the longest waiting.
    fn order(&self) -> (CiPriority, Instant, u64) {
        (self.priority, self.since, self.id)
    }
}

/// A running job's start allowance and when it started (SPEC-497 HBA-002).
#[derive(Debug, Clone, Copy)]
struct Started {
    id: u64,
    /// None until the ticket collects its grant.
    at: Option<Instant>,
    need: Budget,
    allowance: Budget,
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    cpu_usec: u64,
    rate: Option<f64>,
    anon_bytes: u64,
    /// A new base after a stale one, with no rate since: what CI uses is not settled for anyone
    /// until a window passed (review round 2).
    rebased: bool,
}

/// What a CI job holds while it runs: its reservation out of the CI budget, given back when
/// dropped.
pub(crate) struct Reservation {
    admission: Arc<CiAdmission>,
    id: u64,
    need: Budget,
}

impl Reservation {
    /// The CPUs it holds.
    pub(crate) fn cpus(&self) -> u32 {
        self.need.cpus
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.admission.lock();
        state.release(self.id, self.need);
        drop(state);
        self.admission.changed.notify_waiters();
    }
}

/// A waiting job's place in the queue, left when dropped (a cancelled wait).
struct Ticket {
    admission: Arc<CiAdmission>,
    id: u64,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.admission.lock();
        let before = state.queue.len();
        state.queue.retain(|waiter| waiter.id != self.id);
        let left = state.queue.len() != before;
        let uncollected = state
            .started
            .iter()
            .find(|job| job.id == self.id && job.at.is_none())
            .copied();
        if let Some(job) = uncollected {
            state.release(job.id, job.need);
        }
        drop(state);
        if left || uncollected.is_some() {
            self.admission.changed.notify_waiters();
        }
    }
}

impl CiAdmission {
    fn new(capacity: Capacity) -> Self {
        Self {
            state: Mutex::new(AdmissionState {
                budget: capacity.ci_budget(),
                held: Held::default(),
                next_ticket: 0,
                queue: Vec::new(),
                started: Vec::new(),
                samples: HashMap::new(),
            }),
            changed: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdmissionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A place in the queue for a job of a run of `priority` needing `need`, waiting from `now`.
    fn enqueue(self: &Arc<Self>, priority: CiPriority, need: Budget, now: Instant) -> Ticket {
        let mut state = self.lock();
        let id = state.next_ticket;
        state.next_ticket += 1;
        state.queue.push(Waiter {
            id,
            priority,
            since: now,
            need,
        });
        Ticket {
            admission: Arc::clone(self),
            id,
        }
    }

    /// Reads the sandboxes running CI jobs (`running`) with `read` (CPU microseconds, anonymous
    /// bytes) at `now`, under the lock, so readings are applied in the order they were taken
    /// (review round 1: two waiters reading at once could reset a sandbox's rate). A sandbox's
    /// rate is computed once its last reading is [`SAMPLE_WINDOW`] old; one older than
    /// [`STALE_WINDOW`] is a new base, and what CI uses is then not settled until the next look
    /// (SPEC-497 HBA-002: the use over the last window, not since long ago). A counter that went
    /// back (its groups were made again) is a new base too. Sandboxes no longer running jobs are
    /// forgotten.
    fn measure(
        &self,
        now: impl FnOnce() -> Instant,
        running: impl FnOnce() -> Vec<SandboxId>,
        read: impl Fn(&SandboxId) -> Option<(u64, u64)>,
    ) -> Measured {
        let mut state = self.lock();
        // The clock and the list are read under the lock too: a caller with an older list must
        // not drop a base a job's start just made (review round 2).
        let (now, running) = (now(), running());
        state.samples.retain(|sandbox, _| running.contains(sandbox));
        for sandbox in &running {
            let Some((cpu_usec, anon_bytes)) = read(sandbox) else {
                continue;
            };
            let fresh = Sample {
                at: now,
                cpu_usec,
                rate: None,
                anon_bytes,
                rebased: false,
            };
            let Some(sample) = state.samples.get_mut(sandbox) else {
                state.samples.insert(sandbox.clone(), fresh);
                continue;
            };
            sample.anon_bytes = anon_bytes;
            let elapsed = now.saturating_duration_since(sample.at);
            if cpu_usec < sample.cpu_usec || elapsed > STALE_WINDOW {
                // Its groups were made again, or its base is old: what it uses is not known
                // until a window passed, and no job starts on its absence (review of the CI
                // admission fix: a restarted engine read as 0 CPUs).
                *sample = Sample {
                    rebased: true,
                    ..fresh
                };
            } else if elapsed >= SAMPLE_WINDOW {
                #[allow(clippy::cast_precision_loss, reason = "microseconds of CPU time")]
                let rate = (cpu_usec - sample.cpu_usec) as f64 / elapsed.as_micros() as f64;
                *sample = Sample {
                    // A tenth of a CPU: the clocks of two readings differ by microseconds, which
                    // must not round 3.0000002 CPUs up to 4 (review round 1).
                    rate: Some((rate * 10.0).round() / 10.0),
                    ..fresh
                };
            }
        }
        let settled = state.samples.values().all(|s| !s.rebased);
        let rates: Vec<f64> = state.samples.values().filter_map(|s| s.rate).collect();
        let anon: u64 = state
            .samples
            .values()
            .fold(0, |sum, s| sum.saturating_add(s.anon_bytes));
        let used = (!rates.is_empty() || anon > 0).then(|| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a sum of CPU rates on a host, rounded up"
            )]
            let cpus = rates.iter().sum::<f64>().ceil().max(0.0) as u32;
            Budget {
                cpus,
                memory_mb: anon >> 20,
            }
        });
        Measured { used, settled }
    }

    /// A woken waiter reserves for eligible tickets in priority and arrival order, until
    /// the job limit or an overdue ticket blocks the rest. Independent wakeups and writes of
    /// queued reasons need not serialize grants that fit together.
    fn try_admit(
        self: &Arc<Self>,
        ticket: &Ticket,
        measured: Measured,
        now: Instant,
        slots: usize,
    ) -> Result<Reservation, Wait> {
        let mut state = self.lock();
        let mut wait = Wait::Behind;
        if state
            .queue
            .iter()
            .any(|candidate| candidate.id == ticket.id)
        {
            // Reserve in one pass. Each decision sees all grants already made in this pass.
            let mut candidates = state.queue.clone();
            candidates.sort_by_key(Waiter::order);
            let mut changed = false;
            for candidate in candidates {
                match state.decide(candidate.need, measured, now, slots) {
                    Ok(need) => {
                        state.grant(candidate.id, need);
                        changed = true;
                    }
                    Err(reason) => {
                        if candidate.id == ticket.id || matches!(reason, Wait::Slots(_)) {
                            wait = reason;
                        }
                        if now.saturating_duration_since(candidate.since) >= OVERTAKE_LIMIT {
                            if candidate.id != ticket.id && wait == Wait::Behind {
                                wait = Wait::Overdue;
                            }
                            break;
                        }
                        if matches!(reason, Wait::Slots(_)) {
                            break;
                        }
                    }
                }
            }
            if changed {
                self.changed.notify_waiters();
            }
        }
        let granted = state
            .started
            .iter_mut()
            .find(|s| s.id == ticket.id && s.at.is_none())
            .ok_or(wait)?;
        granted.at = Some(now);
        Ok(Reservation {
            admission: Arc::clone(self),
            id: granted.id,
            need: granted.need,
        })
    }
}

impl AdmissionState {
    fn grant(&mut self, id: u64, need: Budget) {
        self.queue.retain(|waiter| waiter.id != id);
        self.held.jobs += 1;
        self.held.reserved.cpus = self.held.reserved.cpus.saturating_add(need.cpus);
        self.held.reserved.memory_mb = self.held.reserved.memory_mb.saturating_add(need.memory_mb);
        self.started.push(Started {
            id,
            at: None,
            need,
            allowance: self.budget.start_allowance(need),
        });
    }

    fn release(&mut self, id: u64, need: Budget) {
        self.held.jobs = self.held.jobs.saturating_sub(1);
        self.held.reserved.cpus = self.held.reserved.cpus.saturating_sub(need.cpus);
        self.held.reserved.memory_mb = self.held.reserved.memory_mb.saturating_sub(need.memory_mb);
        self.started.retain(|started| started.id != id);
    }

    /// Whether a job needing `need` may start at `now` beside what CI holds (SPEC-497 HBA-002):
    /// fewer running jobs than `slots`, a settled reading while CI runs, and room in the budget
    /// ([`admit`]: what CI's processes wait for, the worst case, what CI uses and what jobs
    /// started in the last [`START_WINDOW`] may still bring). Its need, at most the CI budget,
    /// when it may.
    fn decide(
        &self,
        need: Budget,
        measured: Measured,
        now: Instant,
        slots: usize,
    ) -> Result<Budget, Wait> {
        if usize::try_from(self.held.jobs).unwrap_or(usize::MAX) >= slots.max(1) {
            return Err(Wait::Slots(slots));
        }
        if !measured.settled && self.held.jobs > 0 {
            return Err(Wait::Measuring);
        }
        let need = Budget {
            cpus: need.cpus.min(self.budget.cpus),
            memory_mb: need.memory_mb.min(self.budget.memory_mb),
        };
        let starting = self
            .started
            .iter()
            .filter(|started| {
                started
                    .at
                    .is_none_or(|at| now.saturating_duration_since(at) < START_WINDOW)
            })
            .fold(Budget::default(), |sum, started| Budget {
                cpus: sum.cpus.saturating_add(started.allowance.cpus),
                memory_mb: sum.memory_mb.saturating_add(started.allowance.memory_mb),
            });
        let held = Held {
            starting,
            measured: measured.used,
            ..self.held
        };
        match admit(self.budget, &held, need) {
            Admission::Now => Ok(need),
            wait => Err(Wait::Budget(wait)),
        }
    }
}

/// What the readings of the sandboxes running CI jobs say (SPEC-497 HBA-002).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Measured {
    /// What they use together, once anything was measured.
    used: Option<Budget>,
    /// Whether every reading had a recent base: a job is not admitted on a reading that only
    /// became a new base now.
    settled: bool,
}

impl Measured {
    /// Nothing measured: where nothing measures sandboxes, the reservations decide.
    const NOTHING: Self = Self {
        used: None,
        settled: true,
    };
}

/// Why a job is not admitted now.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wait {
    /// A job before it in the order can start now: it goes first.
    Behind,
    /// A job before it in the order has waited past [`OVERTAKE_LIMIT`]: nothing behind it
    /// starts until it has.
    Overdue,
    /// As many jobs run as the instance has job slots.
    Slots(usize),
    /// What CI uses is being measured again.
    Measuring,
    /// It does not fit.
    Budget(Admission),
}

impl std::fmt::Display for Wait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Behind => f.write_str("waits for a CI job before it in line, which starts first"),
            Self::Overdue => {
                f.write_str("waits for a CI job that has waited 10 minutes to start before it")
            }
            Self::Slots(slots) => write!(f, "waits for one of the instance's {slots} CI job slots"),
            Self::Measuring => f.write_str("waits for a reading of what CI uses now"),
            Self::Budget(wait) => write!(f, "waits for the host's CI budget: {wait}"),
        }
    }
}

/// Where a repository sets a default variable's knob itself, and the key it sets there: the
/// variable would override it (review round 1), so the default is left out.
const OWN_SETTINGS: [(&str, &[&str], &str); 3] = [
    // Cargo's `[env]` sets it only where it is not set already (review round 2).
    (
        "RUST_TEST_THREADS",
        &[".cargo/config.toml", ".cargo/config"],
        "RUST_TEST_THREADS",
    ),
    (
        "CARGO_BUILD_JOBS",
        &[".cargo/config.toml", ".cargo/config"],
        "jobs",
    ),
    (
        "NEXTEST_TEST_THREADS",
        &[".config/nextest.toml"],
        "test-threads",
    ),
];

/// The dotted key paths a TOML line names, their components unquoted: a table header's
/// (`[env.RUST_TEST_THREADS]`, `[[x.y]]`), a key's before `=` (`build.jobs = 2`,
/// `RUST_TEST_THREADS.value = "1"`) and the keys of an inline table in its value
/// (`build = { jobs = 2 }`, review of SPEC-272). None for a comment or a blank line; a line inside
/// a multi-line array may name a key that is a string there (a harmless false positive).
fn key_paths(line: &str) -> Vec<Vec<&str>> {
    fn split(path: &str) -> Vec<&str> {
        path.split('.')
            .map(|part| part.trim().trim_matches(|c| c == '"' || c == '\''))
            .collect()
    }
    let line = line.trim();
    if let Some(header) = line.strip_prefix('[') {
        return header
            .trim_start_matches('[')
            .split(']')
            .next()
            .map(split)
            .into_iter()
            .collect();
    }
    if line.starts_with('#') {
        return Vec::new();
    }
    let Some((key, value)) = line.split_once('=') else {
        return Vec::new();
    };
    let inline = value
        .split(['{', ','])
        .skip(1)
        .filter_map(|piece| piece.split_once('=').map(|(key, _)| key))
        .filter(|key| !key.contains(['[', ']', '{', '}']));
    std::iter::once(key).chain(inline).map(split).collect()
}

/// Whether `dir` sets the knob of the default variable `name` in its tools' own files: `<key>`
/// as a component of a key path there ([`key_paths`]: `<key> = …`, `build.<key> = …`,
/// `<key>.value = …`, `[env.<key>]`, `build = { <key> = … }`, SPEC-272 SHR-005) in one of them
/// (a regular file of at most [`MAX_CONFIG_BYTES`]).
fn sets_own(dir: &Path, name: &str) -> bool {
    let Some((_, files, key)) = OWN_SETTINGS.iter().find(|(n, _, _)| *n == name) else {
        return false;
    };
    files.iter().any(|file| {
        let path = dir.join(file);
        let regular = std::fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_CONFIG_BYTES as u64);
        regular
            && std::fs::read_to_string(&path).is_ok_and(|text| {
                text.lines()
                    .flat_map(key_paths)
                    .any(|parts| parts.contains(key))
            })
    })
}

/// [`default_templates`] but those whose knob `dir` sets itself ([`sets_own`]).
fn defaults_for(dir: &Path) -> Vec<EnvTemplate> {
    default_templates()
        .into_iter()
        .filter(|template| !sets_own(dir, &template.name))
        .collect()
}

/// The `[resources.env]` of the `yulius.toml` in `dir` (a worktree, a checkout). Without the
/// file or without the table there, the Rust defaults ([`default_templates`], SPEC-268 PID-007)
/// but those whose knob the repository sets in its tools' own files;
/// none when the file is not a regular file, too large or invalid (the budget's own variables
/// are set anyway).
#[must_use]
pub fn repository_templates(dir: &Path) -> Vec<EnvTemplate> {
    let path = dir.join("yulius.toml");
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return defaults_for(dir);
        }
        Err(_) => return Vec::new(),
    };
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES as u64 {
        return Vec::new();
    }
    match std::fs::read_to_string(&path).map(|text| parse_yulius_toml_resources_table(&text)) {
        Ok(Ok(Some(templates))) => templates,
        Ok(Ok(None)) => defaults_for(dir),
        Ok(Err(_)) | Err(_) => Vec::new(),
    }
}

impl App {
    /// What a sandbox's processes may use together (SPEC-082 RES-001): its enforced CPU and
    /// memory limits within the host's CPUs; without a memory limit, 2 GiB per CPU.
    #[must_use]
    pub fn capacity(&self) -> Capacity {
        let host = std::thread::available_parallelism()
            .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX));
        let limits = self
            .isolation
            .get()
            .map(|isolation| isolation.limits())
            .unwrap_or_default();
        let cpus = limits.cpus.map_or(host, |cpus| cpus.min(host));
        let memory_mb = limits
            .memory_bytes
            .map_or(u64::from(cpus) * MEMORY_PER_CPU_MB, |bytes| bytes >> 20);
        Capacity::new(cpus, memory_mb)
    }

    /// The budget of an agent run, terminal or Cockpit (SPEC-082 RES-005).
    #[must_use]
    pub fn agent_budget(&self) -> Budget {
        self.capacity().agent(self.config.agent_cpus)
    }

    /// The environment that tells an agent run, terminal or Cockpit starting in `dir` its budget:
    /// `YULIUS_CPUS`, `YULIUS_MEMORY_MB` and the repository's `[resources.env]` there.
    #[must_use]
    pub fn agent_resource_env(&self, dir: &Path) -> Vec<(String, String)> {
        budget_env(self.agent_budget(), &repository_templates(dir))
    }

    /// Says on the queued job `job` what it waits for, or nothing once it no longer waits.
    pub(crate) async fn ci_job_waits(&self, job: &str, why: Option<&str>) {
        if let Err(err) = self.deps.store.set_queued_ci_job_reason(job, why).await {
            tracing::warn!(job, error = %err, "recording what a CI job waits for failed");
        }
    }

    /// Waits while `sandbox`'s running Docker engine has fewer networks left to make than a job
    /// needs ([`network_reserve`]) (SPEC-497 HBA-003), saying so on the
    /// queued job `job`; networks no container used for 10 minutes are pruned first (an agent's
    /// or a job's network made a moment ago is kept). No wait without a running engine (nothing
    /// starts one here). It is looked at before the job waits for a slot and its budget, so a
    /// job held by one sandbox's engine holds nothing the others need (review round 1).
    pub(crate) async fn ci_network_room(&self, sandbox: &SandboxId, job: &str) {
        let Some(isolation) = self.isolation.get() else {
            return;
        };
        let mut told = false;
        loop {
            let Some(networks) = isolation.engine_networks(sandbox).await else {
                break;
            };
            let free = networks.capacity.saturating_sub(networks.used);
            let reserve = network_reserve(networks.capacity);
            if free >= reserve {
                break;
            }
            if isolation
                .prune_engine_networks(sandbox)
                .await
                .is_some_and(|removed| removed > 0)
            {
                continue;
            }
            if !told {
                let why = format!(
                    "waits for Docker networks: its sandbox's engine can make {free} more of {}, \
                     and a job starts with at least {reserve} free",
                    networks.capacity
                );
                self.ci_job_waits(job, Some(&why)).await;
                told = true;
            }
            tokio::time::sleep(SAMPLE_WINDOW).await;
        }
        if told {
            self.ci_job_waits(job, None).await;
        }
    }

    /// Keeps one reading a minute of the host for CI (SPEC-497): its CPUs busy since the last
    /// reading, the CPUs running jobs reserved and measured, and the jobs waiting to start; the
    /// readings of 30 days are kept. The first reading after a start has no busy figure.
    pub(crate) async fn ci_record_load(&self) {
        const MINUTE: i64 = 60_000;
        const KEEP: i64 = 30 * 24 * 60 * MINUTE;
        let now = self.now().0;
        let minute = now - now.rem_euclid(MINUTE);
        let ticks = self.deps.system.cpu_ticks();
        let before = {
            let Ok(mut last) = self.ci.load.lock() else {
                return;
            };
            if last.is_some_and(|(at, _)| at == minute) {
                return;
            }
            last.replace((minute, ticks))
        };
        let busy = match (before.and_then(|(_, ticks)| ticks), ticks) {
            (Some(before), Some(after)) => busy_permille(before, after),
            _ => None,
        };
        let measured = self.ci_measured().used.map(|used| used.cpus);
        let held = self.ci_admission().lock().held.reserved.cpus;
        let waiting = self.deps.store.ci_jobs_waiting().await.unwrap_or(0);
        let reading = LoadMinute {
            at: Timestamp(minute),
            busy_permille: busy,
            held_cpus: held,
            measured_cpus: measured,
            waiting_jobs: waiting,
        };
        if let Err(err) = self
            .deps
            .store
            .record_ci_load(reading, Timestamp(now - KEEP))
            .await
        {
            tracing::warn!(error = %err, "recording the host's CI load failed");
        }
    }

    /// Wakes the jobs waiting for admission: something they decide by changed.
    pub(crate) fn ci_admission_changed(&self) {
        if let Some(admission) = self.ci.budget.get() {
            admission.changed.notify_waiters();
        }
    }

    fn ci_admission(&self) -> Arc<CiAdmission> {
        Arc::clone(
            self.ci
                .budget
                .get_or_init(|| Arc::new(CiAdmission::new(self.capacity()))),
        )
    }

    /// What the `ci` and `engine` groups of the sandboxes running CI jobs use now (SPEC-497
    /// HBA-002), as the isolation reads them; nothing where nothing is measured. Also the
    /// dispatcher's reading at each pass, which keeps the bases recent.
    pub(crate) fn ci_measured(&self) -> Measured {
        let Some(isolation) = self.isolation.get() else {
            return Measured::NOTHING;
        };
        self.ci_admission().measure(
            Instant::now,
            || self.ci_sandboxes_running(),
            |sandbox| {
                let used = isolation.ci_use(sandbox)?;
                // For diagnosis only (ADR-0353's amendment): admission does not use it.
                if let Some(pressure) = used.cpu_pressure {
                    tracing::debug!(
                        %sandbox,
                        cpu_pressure_permille = pressure,
                        "CI's CPU pressure"
                    );
                }
                Some((used.cpu_usec, used.anon_bytes))
            },
        )
    }

    /// Waits until the job `job` of a run of `priority` may start with `budget` beside what CI
    /// holds on the whole host, and holds it (SPEC-082 RES-003, SPEC-497 HBA-001, HBA-002):
    /// room by what CI was measured to use and what jobs that just started may still bring, the
    /// running jobs' budgets within three times the CI budget, and a job slot free. Merge
    /// candidates' jobs go first, then the longest waiting; a job that cannot start lets the ones
    /// behind it that can, for [`OVERTAKE_LIMIT`] at most. The queued job says what it waits for
    /// whenever that changes (HBA-005).
    pub(crate) async fn ci_reserve(
        &self,
        job: &str,
        budget: Budget,
        priority: CiPriority,
    ) -> Option<Reservation> {
        let admission = self.ci_admission();
        let ticket = admission.enqueue(priority, budget, Instant::now());
        let mut told: Option<String> = None;
        loop {
            let changed = admission.changed.notified();
            match admission.try_admit(
                &ticket,
                self.ci_measured(),
                Instant::now(),
                self.ci_job_slots(),
            ) {
                Ok(reservation) => {
                    if told.is_some() {
                        self.ci_job_waits(job, None).await;
                    }
                    return Some(reservation);
                }
                Err(wait) => {
                    let why = wait.to_string();
                    if told.as_deref() != Some(why.as_str()) {
                        self.ci_job_waits(job, Some(&why)).await;
                        told = Some(why);
                    }
                }
            }
            tokio::select! {
                () = changed => {}
                () = tokio::time::sleep(SAMPLE_WINDOW) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> SandboxId {
        SandboxId::parse(name).unwrap_or_else(|_| unreachable!())
    }

    fn need(cpus: u32) -> Budget {
        Budget { cpus, memory_mb: 1 }
    }

    #[test]
    fn a_skipped_cpu_reading_keeps_the_last_complete_base() {
        let admission = CiAdmission::new(Capacity::new(8, 1024));
        let first = Instant::now();
        let measure = |seconds, cpu: Option<u64>| {
            admission.measure(
                || first + Duration::from_secs(seconds),
                || vec![sandbox("a")],
                |_| cpu.map(|usec| (usec, 0)),
            )
        };
        assert_eq!(measure(0, Some(900_000_000)), Measured::NOTHING);
        assert_eq!(measure(5, None).used, None);
        assert_eq!(
            measure(10, Some(910_000_000)).used.map(|used| used.cpus),
            Some(1)
        );
    }

    /// SPEC-497 HBA-002 (review round 1): no rate before a window passed; two sandboxes' rates
    /// add up, rounded up after a tenth; memory in MiB; a counter that goes back starts over; a
    /// sandbox no longer running is forgotten; a base older than the stale window is not
    /// settled.
    #[test]
    fn what_ci_uses_is_measured_over_a_recent_window() {
        let admission = CiAdmission::new(Capacity::new(64, 64 * 1024));
        let (a, b) = (sandbox("a"), sandbox("b"));
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let readings = |values: &[(&SandboxId, u64, u64)]| {
            let values: Vec<(SandboxId, u64, u64)> = values
                .iter()
                .map(|(s, cpu, anon)| ((*s).clone(), *cpu, *anon))
                .collect();
            move |s: &SandboxId| {
                values
                    .iter()
                    .find(|(id, _, _)| id == s)
                    .map(|(_, cpu, anon)| (*cpu, *anon))
            }
        };
        let both = [a.clone(), b.clone()];
        let first = admission.measure(
            || at(0),
            || both.to_vec(),
            readings(&[(&a, 0, 0), (&b, 0, 0)]),
        );
        assert_eq!(first, Measured::NOTHING, "a base, no rate");
        let early = admission.measure(
            || at(2),
            || both.to_vec(),
            readings(&[(&a, 9_000_000, 0), (&b, 0, 0)]),
        );
        assert_eq!(early.used, None, "two seconds are no window");
        // 2 CPUs and 1.5 CPUs over 5 s (with a microsecond more of the clock): 4 after a tenth.
        let used = admission.measure(
            || at(5),
            || both.to_vec(),
            readings(&[(&a, 10_000_001, 3 << 20), (&b, 7_500_000, 1 << 20)]),
        );
        assert_eq!(
            used,
            Measured {
                used: Some(Budget {
                    cpus: 4,
                    memory_mb: 4
                }),
                settled: true
            }
        );
        // A rate of exactly 3 measured with clock noise stays 3.
        let noisy = CiAdmission::new(Capacity::new(64, 64 * 1024));
        let one = [a.clone()];
        let _ = noisy.measure(|| at(0), || one.to_vec(), readings(&[(&a, 0, 0)]));
        let three = noisy.measure(|| at(5), || one.to_vec(), readings(&[(&a, 15_000_001, 0)]));
        assert_eq!(three.used.map(|u| u.cpus), Some(3));
        // `b` no longer runs a job: only `a` counts.
        let alone = admission.measure(|| at(6), || one.to_vec(), readings(&[(&a, 10_000_001, 0)]));
        assert_eq!(alone.used.map(|u| u.cpus), Some(2));
        // A counter that went back (its groups made again) is a new base: no rate, and not
        // settled until a window passed (review of the CI admission fix: a restarted engine
        // read as 0 CPUs).
        let again = admission.measure(|| at(11), || one.to_vec(), readings(&[(&a, 5, 0)]));
        assert_eq!((again.used, again.settled), (None, false));
        let settled =
            admission.measure(|| at(16), || one.to_vec(), readings(&[(&a, 10_000_005, 0)]));
        assert_eq!(
            (settled.used.map(|u| u.cpus), settled.settled),
            (Some(2), true)
        );
        // A base older than the stale window: a new base, not settled, for whoever reads next
        // within the window too (review round 2).
        let stale = admission.measure(
            || at(60),
            || one.to_vec(),
            readings(&[(&a, 500_000_000, 0)]),
        );
        assert!(!stale.settled, "{stale:?}");
        let soon = admission.measure(
            || at(61),
            || one.to_vec(),
            readings(&[(&a, 501_000_000, 0)]),
        );
        assert!(!soon.settled, "{soon:?}");
        let next = admission.measure(
            || at(65),
            || one.to_vec(),
            readings(&[(&a, 505_000_000, 0)]),
        );
        assert_eq!(next.used.map(|u| u.cpus), Some(1));
        assert!(next.settled);
    }

    #[test]
    fn one_wakeup_reserves_all_earlier_eligible_jobs_up_to_the_live_limit() {
        let admission = Arc::new(CiAdmission::new(Capacity::new(16, 16384)));
        let now = Instant::now();
        let tickets: Vec<_> = (0..4)
            .map(|_| admission.enqueue(CiPriority::PullRequest, need(1), now))
            .collect();
        // Wake the last waiter first. The first three can be reserved immediately in order.
        assert!(
            admission
                .try_admit(&tickets[3], Measured::NOTHING, now, 3)
                .is_err()
        );
        assert_eq!(admission.lock().held.jobs, 3);
        let mut tickets = tickets.into_iter();
        drop(tickets.next()); // cancelled after the grant, before collecting it
        let remaining: Vec<_> = tickets.collect();
        let held: Vec<_> = remaining
            .iter()
            .map(|ticket| admission.try_admit(ticket, Measured::NOTHING, now, 3).ok())
            .collect();
        assert!(held.iter().all(Option::is_some));
        assert_eq!(admission.lock().held.jobs, 3);
        drop(held);
        assert_eq!(admission.lock().held.jobs, 0);
    }

    #[test]
    fn the_first_wakeup_also_reserves_for_later_eligible_jobs() {
        let admission = Arc::new(CiAdmission::new(Capacity::new(9, 9216)));
        let now = Instant::now();
        let tickets: Vec<_> = (0..3)
            .map(|_| admission.enqueue(CiPriority::Branch, need(2), now))
            .collect();
        let first = admission
            .try_admit(&tickets[0], Measured::NOTHING, now, 3)
            .ok();
        assert!(first.is_some());
        assert_eq!(admission.lock().held.jobs, 3);
        // Pending grants still count after the startup window. A fresh 3-CPU arrival on
        // a host already using 3 cannot start beside the two pending 2-CPU grants.
        let later = now + START_WINDOW;
        let fourth = admission.enqueue(CiPriority::Branch, need(3), later);
        let measured = Measured {
            used: Some(need(3)),
            settled: true,
        };
        assert!(matches!(
            admission.try_admit(&fourth, measured, later, 12),
            Err(Wait::Budget(_))
        ));
    }

    /// SPEC-497 HBA-002: a job that cannot start lets a later one that can; one before it that
    /// can start goes first; merge candidates' jobs go before older pull requests'; a job that
    /// waited past the overtaking limit holds back those behind it until it starts or leaves;
    /// the job slots bound the running jobs; an unsettled reading holds a job back only while
    /// CI runs.
    #[test]
    fn jobs_that_fit_pass_one_that_does_not() {
        // 9 CPUs: a CI budget of 8. Nothing measured: the reservations decide.
        let admission = Arc::new(CiAdmission::new(Capacity::new(9, 9 * 1024)));
        let measured = Measured::NOTHING;
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let pr = CiPriority::PullRequest;
        let first = admission.enqueue(pr, need(6), at(0));
        let held = admission.try_admit(&first, measured, at(0), 12).ok();
        assert!(held.is_some());
        let large = admission.enqueue(pr, need(4), at(1));
        let small = admission.enqueue(pr, need(1), at(2));
        assert!(matches!(
            admission.try_admit(&large, measured, at(3), 12),
            Err(Wait::Budget(_))
        ));
        let small_held = admission.try_admit(&small, measured, at(3), 12).ok();
        assert!(
            small_held.is_some(),
            "the small one fits and passes the large one"
        );
        // The first job ends: the large one fits now and goes before a later one.
        drop(held);
        let later = admission.enqueue(pr, need(1), at(4));
        let later_held = admission.try_admit(&later, measured, at(4), 12).ok();
        assert!(later_held.is_some(), "both fit on the same wakeup");
        let large_held = admission.try_admit(&large, measured, at(4), 12).ok();
        assert!(
            large_held.is_some(),
            "the earlier job received its reservation first"
        );
        drop((small_held, large_held, later_held));
        // A merge candidate's job goes before an older pull request's.
        let running = admission.enqueue(pr, need(7), at(10));
        let running = admission.try_admit(&running, measured, at(10), 12).ok();
        assert!(running.is_some());
        let review = admission.enqueue(pr, need(1), at(11));
        let merge = admission.enqueue(CiPriority::Merge, need(1), at(12));
        assert!(matches!(
            admission.try_admit(&review, measured, at(12), 12),
            Err(Wait::Budget(_))
        ));
        let merge_held = admission.try_admit(&merge, measured, at(12), 12).ok();
        assert!(merge_held.is_some());
        drop((merge_held, review));
        // A job that waited past the overtaking limit holds back those behind it.
        let big = admission.enqueue(pr, need(4), at(20));
        let quick = admission.enqueue(pr, need(1), at(21));
        let overdue = at(20) + OVERTAKE_LIMIT;
        assert!(matches!(
            admission.try_admit(&big, measured, overdue, 12),
            Err(Wait::Budget(_))
        ));
        assert_eq!(
            admission.try_admit(&quick, measured, overdue, 12).err(),
            Some(Wait::Overdue)
        );
        assert_eq!(
            Wait::Overdue.to_string(),
            "waits for a CI job that has waited 10 minutes to start before it"
        );
        // It leaves (its run was cancelled): the next one starts.
        drop(big);
        let quick_held = admission.try_admit(&quick, measured, overdue, 12).ok();
        assert!(quick_held.is_some());
        drop((running, quick_held));
        // Two job slots: a third job waits for one, holding none.
        let x = admission.enqueue(pr, need(1), at(30));
        let x = admission.try_admit(&x, measured, at(30), 2).ok();
        let y = admission.enqueue(pr, need(1), at(30));
        let y = admission.try_admit(&y, measured, at(30), 2).ok();
        assert!(x.is_some() && y.is_some());
        let z = admission.enqueue(pr, need(1), at(31));
        assert_eq!(
            admission.try_admit(&z, measured, at(31), 2).err(),
            Some(Wait::Slots(2))
        );
        // While a job runs, a reading that only became a new base holds the next one back.
        let unsettled = Measured {
            used: None,
            settled: false,
        };
        assert_eq!(
            admission.try_admit(&z, unsettled, at(31), 12).err(),
            Some(Wait::Measuring)
        );
        drop((x, y));
        assert!(
            admission.try_admit(&z, unsettled, at(31), 12).is_ok(),
            "no job runs"
        );
    }

    /// SPEC-497 HBA-002: with readings, a job that just started counts with its start allowance
    /// (a quarter of the budget at most) for a minute; then what is measured decides.
    #[test]
    fn a_started_job_counts_until_the_readings_show_it() {
        // 9 CPUs: a CI budget of 8; a 2-CPU job starts with 2.
        let admission = Arc::new(CiAdmission::new(Capacity::new(9, 9 * 1024)));
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let reading = |cpus: u32| Measured {
            used: Some(Budget { cpus, memory_mb: 0 }),
            settled: true,
        };
        let pr = CiPriority::PullRequest;
        let mut running = Vec::new();
        for secs in 0..4 {
            let ticket = admission.enqueue(pr, need(2), at(secs));
            running.push(admission.try_admit(&ticket, reading(0), at(secs), 12).ok());
        }
        assert!(running.iter().all(Option::is_some));
        let fifth = admission.enqueue(pr, need(2), at(4));
        let wait = admission.try_admit(&fifth, reading(0), at(4), 12).err();
        assert!(
            matches!(
                wait,
                Some(Wait::Budget(Admission::Load {
                    used: 0,
                    starting: 8,
                    ..
                }))
            ),
            "{wait:?}"
        );
        // A minute on, the readings decide: 7 used leaves no room for 2, 5 does.
        assert!(matches!(
            admission.try_admit(&fifth, reading(7), at(64), 12),
            Err(Wait::Budget(Admission::Load {
                used: 7,
                starting: 0,
                ..
            }))
        ));
        assert!(admission.try_admit(&fifth, reading(5), at(64), 12).is_ok());

        // A job that ends within its first minute no longer counts as starting (review of the CI
        // admission fix): of four 2-CPU starts one ends, and a fifth starts at once.
        let admission = Arc::new(CiAdmission::new(Capacity::new(9, 9 * 1024)));
        let mut running = Vec::new();
        for secs in 0..4 {
            let ticket = admission.enqueue(pr, need(2), at(secs));
            running.push(admission.try_admit(&ticket, reading(0), at(secs), 12).ok());
        }
        let fifth = admission.enqueue(pr, need(2), at(4));
        assert!(admission.try_admit(&fifth, reading(0), at(4), 12).is_err());
        drop(running.pop());
        assert!(
            admission.try_admit(&fifth, reading(0), at(4), 12).is_ok(),
            "the ended job's start is no longer counted"
        );
    }

    fn names(templates: &[EnvTemplate]) -> Vec<&str> {
        templates.iter().map(|t| t.name.as_str()).collect()
    }

    /// SPEC-268 PID-007: no table (or no `yulius.toml`) gives the Rust defaults; a declared table,
    /// even an empty one, replaces them; an invalid one sets none.
    #[test]
    fn a_repository_without_a_mapping_gets_the_rust_defaults()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let defaults = [
            "CARGO_BUILD_JOBS",
            "NEXTEST_TEST_THREADS",
            "RUST_TEST_THREADS",
        ];
        assert_eq!(names(&repository_templates(dir.path())), defaults);
        let toml = dir.path().join("yulius.toml");
        std::fs::write(&toml, "[docs]\nproduct = \"docs/product.md\"\n")?;
        assert_eq!(names(&repository_templates(dir.path())), defaults);
        assert!(
            repository_templates(dir.path())
                .iter()
                .all(|t| t.template == "{cpus}")
        );
        // A knob the repository sets in its tools' own files keeps its value (review round 1).
        std::fs::create_dir_all(dir.path().join(".config"))?;
        std::fs::write(
            dir.path().join(".config/nextest.toml"),
            "[profile.default]\ntest-threads = 1\n",
        )?;
        std::fs::create_dir_all(dir.path().join(".cargo"))?;
        std::fs::write(dir.path().join(".cargo/config.toml"), "[build]\njobs = 2\n")?;
        assert_eq!(
            names(&repository_templates(dir.path())),
            ["RUST_TEST_THREADS"]
        );
        // Review round 2: Cargo's `[env]` does not replace a variable that is set.
        std::fs::write(
            dir.path().join(".cargo/config.toml"),
            "[build]\njobs = 2\n\n[env]\nRUST_TEST_THREADS = \"1\"\n",
        )?;
        assert!(repository_templates(dir.path()).is_empty());
        std::fs::write(&toml, "[resources.env]\n")?;
        assert!(repository_templates(dir.path()).is_empty());
        std::fs::write(&toml, "[resources.env]\nMAKEFLAGS = \"-j{cpus}\"\n")?;
        assert_eq!(names(&repository_templates(dir.path())), ["MAKEFLAGS"]);
        std::fs::write(&toml, "[resources.env]\nPATH = \"/x\"\n")?;
        assert!(repository_templates(dir.path()).is_empty(), "invalid");
        std::fs::write(&toml, "[resources.env\n")?;
        assert!(repository_templates(dir.path()).is_empty(), "unparsable");
        Ok(())
    }

    /// SPEC-272 SHR-005 (SPEC-268 review round 3): a knob set through a dotted key or a table
    /// header keeps its value; a value or a comment naming the key sets nothing.
    #[test]
    fn a_knob_set_through_a_dotted_key_keeps_its_value() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join(".cargo"))?;
        std::fs::create_dir_all(dir.path().join(".config"))?;
        let cargo = dir.path().join(".cargo/config.toml");
        let nextest = dir.path().join(".config/nextest.toml");
        for (file, text, name) in [
            (
                &cargo,
                "[env]\nRUST_TEST_THREADS.value = \"1\"\n",
                "RUST_TEST_THREADS",
            ),
            (
                &cargo,
                "[env]\nRUST_TEST_THREADS = { value = \"1\" }\n",
                "RUST_TEST_THREADS",
            ),
            (
                &cargo,
                "env.RUST_TEST_THREADS = \"1\"\n",
                "RUST_TEST_THREADS",
            ),
            (
                &cargo,
                "[env.RUST_TEST_THREADS]\nvalue = \"1\"\n",
                "RUST_TEST_THREADS",
            ),
            (
                &cargo,
                "[env]\n\"RUST_TEST_THREADS\" = \"1\"\n",
                "RUST_TEST_THREADS",
            ),
            // Review: keys of an inline table are keys too.
            (&cargo, "build = { jobs = 2 }\n", "CARGO_BUILD_JOBS"),
            (
                &cargo,
                "env = { FOO = \"x\", RUST_TEST_THREADS = \"1\" }\n",
                "RUST_TEST_THREADS",
            ),
            (&cargo, "build.jobs = 2\n", "CARGO_BUILD_JOBS"),
            (&cargo, "[build]\njobs.x = 2\n", "CARGO_BUILD_JOBS"),
            (
                &nextest,
                "profile.default.test-threads = 1\n",
                "NEXTEST_TEST_THREADS",
            ),
            (
                &nextest,
                "[profile.ci]\n'test-threads' = 1\n",
                "NEXTEST_TEST_THREADS",
            ),
        ] {
            std::fs::write(file, text)?;
            assert!(sets_own(dir.path(), name), "{text}");
            std::fs::remove_file(file)?;
        }
        for (file, text, name) in [
            (
                &cargo,
                "[env]\nFOO = \"RUST_TEST_THREADS\"\n",
                "RUST_TEST_THREADS",
            ),
            (
                &cargo,
                "env = { FOO = \"RUST_TEST_THREADS\" }\n",
                "RUST_TEST_THREADS",
            ),
            (&cargo, "# jobs = 2\n", "CARGO_BUILD_JOBS"),
            (&cargo, "[build]\njobsx = 2\n", "CARGO_BUILD_JOBS"),
            (
                &nextest,
                "[profile.default]\nslow-timeout = \"test-threads\"\n",
                "NEXTEST_TEST_THREADS",
            ),
        ] {
            std::fs::write(file, text)?;
            assert!(!sets_own(dir.path(), name), "{text}");
            std::fs::remove_file(file)?;
        }
        Ok(())
    }
}
