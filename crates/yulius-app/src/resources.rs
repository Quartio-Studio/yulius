//! CPU and memory budgets of a sandbox's processes (SPEC-082, ADR-0065).
//!
//! The capacity is what a sandbox's processes may use together: its enforced limits (the
//! isolation adapter's `cpu.max` and `memory.max`), else the host's CPUs and 2 GiB of memory
//! per CPU. Yulius gives out budgets from it and says so in the environment
//! ([`yulius_domain::resources::budget_env`]): CI jobs get their declared or default share and
//! wait until it fits beside what CI really holds on the host (`App::ci_reserve`, SPEC-497
//! HBA-002); agent runs, terminals and the Cockpit get the agent budget
//! ([`App::agent_resource_env`]). The repository maps the budget to its tools in
//! `[resources.env]` of the `yulius.toml` where the process starts.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;
use yulius_domain::ids::SandboxId;
use yulius_domain::resources::{
    Admission, Budget, Capacity, EnvTemplate, Held, LoadMinute, admit, budget_env, busy_permille,
    default_templates, network_reserve,
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
/// How long a job that does not fit lets smaller jobs behind it start before it goes first: no
/// convoy behind a large one, and no large one overtaken for good (SPEC-497).
pub(crate) const PRIORITY_AFTER: Duration = Duration::from_secs(5 * 60);

/// The CI budget of the whole instance and what CI holds of it (SPEC-082 RES-003, SPEC-497
/// HBA-001, HBA-002): one for every sandbox, sized once from the capacity.
pub(crate) struct CiAdmission {
    state: Mutex<AdmissionState>,
    /// Told when a job ends or leaves the queue.
    changed: Notify,
}

struct AdmissionState {
    budget: Capacity,
    held: Held,
    next_ticket: u64,
    /// The waiting jobs' tickets and when each began to wait, in arrival order.
    queue: VecDeque<(u64, Instant)>,
    /// `YULIUS_CI_OVERCOMMIT`: reservations count at 1/factor ([`admit`]).
    overcommit: u32,
    /// Each measured sandbox's last reading (when, CPU microseconds), its CPU rate over the
    /// window before it (CPUs, once two readings were far enough apart) and its memory.
    samples: HashMap<SandboxId, Sample>,
    /// The running jobs' reservations with when each was admitted: those admitted after a
    /// reading are not in it, and count on top of it (review round 3).
    admitted: Vec<(u64, SandboxId, Instant, Budget)>,
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
        state.admitted.retain(|(id, _, _, _)| *id != self.id);
        state.held.jobs = state.held.jobs.saturating_sub(1);
        state.held.reserved.cpus = state.held.reserved.cpus.saturating_sub(self.need.cpus);
        state.held.reserved.memory_mb = state
            .held
            .reserved
            .memory_mb
            .saturating_sub(self.need.memory_mb);
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
        state.queue.retain(|(ticket, _)| *ticket != self.id);
        let left = state.queue.len() != before;
        drop(state);
        if left {
            self.admission.changed.notify_waiters();
        }
    }
}

impl CiAdmission {
    fn new(capacity: Capacity, overcommit: u32) -> Self {
        Self {
            state: Mutex::new(AdmissionState {
                budget: capacity.ci_budget(),
                held: Held::default(),
                next_ticket: 0,
                queue: VecDeque::new(),
                overcommit,
                samples: HashMap::new(),
                admitted: Vec::new(),
            }),
            changed: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdmissionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A place at the end of the queue, waiting since `now`.
    fn enqueue(self: &Arc<Self>, now: Instant) -> Ticket {
        let mut state = self.lock();
        let id = state.next_ticket;
        state.next_ticket += 1;
        state.queue.push_back((id, now));
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
    /// back (its groups were made again) starts over. Sandboxes no longer running jobs are
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
            if cpu_usec < sample.cpu_usec {
                *sample = fresh;
            } else if elapsed > STALE_WINDOW {
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
        // Where each sandbox's rate ends: a job of it admitted after that is in no rate yet.
        let rated = state
            .samples
            .iter()
            .filter(|(_, s)| s.rate.is_some())
            .map(|(sandbox, s)| (sandbox.clone(), s.at))
            .collect();
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
        Measured {
            used,
            settled,
            rated,
        }
    }

    /// Admits the job of `sandbox` holding `ticket` at `now` when what CI uses is settled and
    /// `need` fits beside what CI holds ([`admit`]): the larger of the reservations and
    /// `measured` plus the reservations admitted in each sandbox after its reading (several jobs
    /// arriving within one window each add to it, review rounds 3 and 1); otherwise what it
    /// waits for. A job before it in line that does not fit holds it back only once it has
    /// waited [`PRIORITY_AFTER`]: until then a smaller job that fits starts (no convoy behind a
    /// large one, SPEC-497), from then on the large one goes first.
    fn try_admit(
        self: &Arc<Self>,
        ticket: &Ticket,
        sandbox: &SandboxId,
        need: Budget,
        measured: &Measured,
        now: Instant,
    ) -> Result<Reservation, Wait> {
        let mut state = self.lock();
        let Some(place) = state.queue.iter().position(|(id, _)| *id == ticket.id) else {
            return Err(Wait::Priority);
        };
        if state
            .queue
            .iter()
            .take(place)
            .any(|(_, since)| now.saturating_duration_since(*since) >= PRIORITY_AFTER)
        {
            return Err(Wait::Priority);
        }
        if !measured.settled && state.held.jobs > 0 {
            return Err(Wait::Measuring);
        }
        state.held.measured = measured.used.map(|used| {
            let since = state
                .admitted
                .iter()
                .filter(|(_, admitted_in, at, _)| {
                    !measured
                        .rated
                        .iter()
                        .any(|(rated, until)| rated == admitted_in && until >= at)
                })
                .fold(Budget::default(), |sum, (_, _, _, need)| Budget {
                    cpus: sum.cpus.saturating_add(need.cpus),
                    memory_mb: sum.memory_mb.saturating_add(need.memory_mb),
                });
            Budget {
                cpus: used.cpus.saturating_add(since.cpus),
                memory_mb: used.memory_mb.saturating_add(since.memory_mb),
            }
        });
        let need = Budget {
            cpus: need.cpus.min(state.budget.cpus),
            memory_mb: need.memory_mb.min(state.budget.memory_mb),
        };
        match admit(state.budget, &state.held, need, state.overcommit) {
            Admission::Now => {
                state.queue.remove(place);
                state.admitted.push((ticket.id, sandbox.clone(), now, need));
                state.held.jobs += 1;
                state.held.reserved.cpus = state.held.reserved.cpus.saturating_add(need.cpus);
                state.held.reserved.memory_mb =
                    state.held.reserved.memory_mb.saturating_add(need.memory_mb);
                drop(state);
                // The next one in line may fit too.
                self.changed.notify_waiters();
                Ok(Reservation {
                    admission: Arc::clone(self),
                    id: ticket.id,
                    need,
                })
            }
            wait => Err(Wait::Budget(wait)),
        }
    }
}

/// What the readings of the sandboxes running CI jobs say (SPEC-497 HBA-002).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Measured {
    /// What they use together, once anything was measured.
    used: Option<Budget>,
    /// Whether every reading had a recent base: a job is not admitted on a reading that only
    /// became a new base now.
    settled: bool,
    /// Where each measured sandbox's rate ends.
    rated: Vec<(SandboxId, Instant)>,
}

impl Measured {
    /// Nothing measured: where nothing measures sandboxes, the reservations decide.
    const NOTHING: Self = Self {
        used: None,
        settled: true,
        rated: Vec::new(),
    };
}

/// Why a job is not admitted now.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wait {
    /// A job before it in line has waited [`PRIORITY_AFTER`] and goes first.
    Priority,
    /// What CI uses is being measured again.
    Measuring,
    /// It does not fit.
    Budget(Admission),
}

impl std::fmt::Display for Wait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Priority => f.write_str(
                "waits for a larger CI job that has waited 5 minutes for the host's CI budget and \
                 now goes first",
            ),
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

    fn ci_admission(&self) -> Arc<CiAdmission> {
        Arc::clone(
            self.ci.budget.get_or_init(|| {
                Arc::new(CiAdmission::new(self.capacity(), self.config.ci.overcommit))
            }),
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
                Some((used.cpu_usec, used.anon_bytes))
            },
        )
    }

    /// Waits until `budget` fits beside what CI holds on the whole host and holds it (SPEC-082
    /// RES-003, SPEC-497 HBA-001, HBA-002): the larger of the running jobs' reservations and
    /// their sandboxes' measured use. Waiters are served in order, so a large job is never
    /// overtaken for good; a budget larger than the CI budget takes all of it. The queued job
    /// `job` says what it waits for whenever that changes (HBA-005).
    pub(crate) async fn ci_reserve(
        &self,
        sandbox: &SandboxId,
        job: &str,
        budget: Budget,
    ) -> Option<Reservation> {
        let admission = self.ci_admission();
        let ticket = admission.enqueue(Instant::now());
        let mut told: Option<String> = None;
        loop {
            let changed = admission.changed.notified();
            let measured = self.ci_measured();
            match admission.try_admit(&ticket, sandbox, budget, &measured, Instant::now()) {
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

    /// SPEC-497 HBA-002 (review round 1): no rate before a window passed; two sandboxes' rates
    /// add up, rounded up after a tenth; memory in MiB; a counter that goes back starts over; a
    /// sandbox no longer running is forgotten; a base older than the stale window is not
    /// settled.
    #[test]
    fn what_ci_uses_is_measured_over_a_recent_window() {
        let admission = CiAdmission::new(Capacity::new(64, 64 * 1024), 1);
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
            (used.used, used.settled),
            (
                Some(Budget {
                    cpus: 4,
                    memory_mb: 4
                }),
                true
            )
        );
        let mut rated = used.rated.clone();
        rated.sort();
        assert_eq!(rated, [(a.clone(), at(5)), (b.clone(), at(5))]);
        // A rate of exactly 3 measured with clock noise stays 3.
        let noisy = CiAdmission::new(Capacity::new(64, 64 * 1024), 1);
        let one = [a.clone()];
        let _ = noisy.measure(|| at(0), || one.to_vec(), readings(&[(&a, 0, 0)]));
        let three = noisy.measure(|| at(5), || one.to_vec(), readings(&[(&a, 15_000_001, 0)]));
        assert_eq!(three.used.map(|u| u.cpus), Some(3));
        // `b` no longer runs a job: only `a` counts.
        let alone = admission.measure(|| at(6), || one.to_vec(), readings(&[(&a, 10_000_001, 0)]));
        assert_eq!(alone.used.map(|u| u.cpus), Some(2));
        // A counter that went back starts over: no rate until the next window.
        let again = admission.measure(|| at(11), || one.to_vec(), readings(&[(&a, 5, 0)]));
        assert_eq!(again, Measured::NOTHING);
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

    /// SPEC-497 HBA-002 (review round 3 of slice A, round 1 of slice B): jobs admitted after a
    /// reading add to it, each against its own sandbox's reading. One job of `a` reserved 16
    /// CPUs and was measured at 40: of five 8-CPU jobs arriving together two start (56 of 56),
    /// not five; a job of `b`, which has no rate yet, counts on top too; a later reading does
    /// not count them twice.
    #[test]
    fn jobs_admitted_after_a_reading_count_on_top_of_it() {
        let admission = Arc::new(CiAdmission::new(Capacity::new(64, 512 * 1024), 1));
        let (a, b) = (sandbox("a"), sandbox("b"));
        let t0 = Instant::now();
        let first = admission.enqueue(t0);
        let running = admission.try_admit(&first, &a, need(16), &Measured::NOTHING, t0);
        assert!(running.is_ok());
        let reading = |rated: Vec<(SandboxId, Instant)>| Measured {
            used: Some(Budget {
                cpus: 40,
                memory_mb: 1,
            }),
            settled: true,
            rated,
        };
        let reading_at_5 = reading(vec![(a.clone(), t0 + Duration::from_secs(5))]);
        let mut started = Vec::new();
        for n in 1..=5 {
            let at = t0 + Duration::from_secs(6) + Duration::from_millis(n);
            let ticket = admission.enqueue(at);
            match admission.try_admit(&ticket, &a, need(8), &reading_at_5, at) {
                Ok(held) => started.push(held),
                Err(wait) => {
                    assert!(matches!(wait, Wait::Budget(_)), "{wait:?}");
                    break;
                }
            }
        }
        assert_eq!(started.len(), 2);
        drop(started);
        // A job of `b`, admitted before `a`'s reading but in no rate of its own, counts too.
        let early = t0 + Duration::from_secs(3);
        let ticket = admission.enqueue(early);
        let b_job = admission.try_admit(&ticket, &b, need(8), &Measured::NOTHING, early);
        assert!(b_job.is_ok());
        let late = t0 + Duration::from_secs(7);
        let ticket = admission.enqueue(late);
        let one = admission.try_admit(&ticket, &a, need(8), &reading_at_5, late);
        assert!(one.is_ok(), "40 + 8 + 8 of 56");
        let ticket = admission.enqueue(late);
        assert!(
            admission
                .try_admit(&ticket, &a, need(8), &reading_at_5, late)
                .is_err(),
            "40 + 8 + 8 + 8 > 56"
        );
        drop(ticket);
        // Readings of both after them count them in it.
        let later = reading(vec![
            (a.clone(), t0 + Duration::from_secs(60)),
            (b.clone(), t0 + Duration::from_secs(60)),
        ]);
        let at = t0 + Duration::from_secs(61);
        let ticket = admission.enqueue(at);
        assert!(
            admission
                .try_admit(&ticket, &a, need(8), &later, at)
                .is_ok(),
            "16 + 8 + 8 reserved, 40 measured: 40 + 8 of 56"
        );
    }

    /// SPEC-497 (production, 16 Nov 2026: a one-job run needing the whole host held 8 jobs
    /// behind it): a job that does not fit lets smaller ones behind it start; once it has waited
    /// 5 minutes it goes first and holds them; a job leaving the queue lets the next in; an
    /// unsettled reading holds a job back only while CI runs.
    #[test]
    fn a_large_job_holds_no_convoy_and_goes_first_after_its_bound() {
        // 9 CPUs: a CI budget of 8.
        let admission = Arc::new(CiAdmission::new(Capacity::new(9, 9 * 1024), 1));
        let s1 = sandbox("s1");
        let t0 = Instant::now();
        let measured = Measured::NOTHING;
        let a = admission.enqueue(t0);
        let held = admission.try_admit(&a, &s1, need(6), &measured, t0).ok();
        assert!(held.is_some());
        let large = admission.enqueue(t0);
        let small = admission.enqueue(t0);
        assert!(matches!(
            admission.try_admit(&large, &s1, need(4), &measured, t0),
            Err(Wait::Budget(_))
        ));
        // Within the bound the small one starts beside it.
        let small_held = admission
            .try_admit(&small, &s1, need(1), &measured, t0)
            .ok();
        assert!(small_held.is_some(), "no convoy behind the large one");
        // Past the bound the large one goes first: a small one behind it waits, though it fits.
        let past = t0 + PRIORITY_AFTER;
        let behind = admission.enqueue(past);
        assert_eq!(
            admission
                .try_admit(&behind, &s1, need(1), &measured, past)
                .err(),
            Some(Wait::Priority)
        );
        // It leaves (its job was cancelled): the small one starts.
        drop(large);
        let behind_held = admission
            .try_admit(&behind, &s1, need(1), &measured, past)
            .ok();
        assert!(behind_held.is_some());
        // While a job runs, a reading that only became a new base holds the next one back.
        let next = admission.enqueue(past);
        let unsettled = Measured {
            used: None,
            settled: false,
            rated: Vec::new(),
        };
        assert_eq!(
            admission
                .try_admit(&next, &s1, need(1), &unsettled, past)
                .err(),
            Some(Wait::Measuring)
        );
        drop((held, small_held, behind_held));
        assert!(
            admission
                .try_admit(&next, &s1, need(1), &unsettled, past)
                .is_ok(),
            "no job runs"
        );
    }

    /// SPEC-497 (the production hotfix carried forward): with `YULIUS_CI_OVERCOMMIT=2` idle
    /// 21-CPU jobs count 11 each: four start in a budget of 56, against two at 1.
    #[test]
    fn the_overcommit_factor_counts_reservations_at_one_over_it() {
        let s1 = sandbox("s1");
        let t0 = Instant::now();
        let started = |factor: u32| {
            let admission = Arc::new(CiAdmission::new(Capacity::new(64, 512 * 1024), factor));
            let mut held = Vec::new();
            for _ in 0..4 {
                let ticket = admission.enqueue(t0);
                match admission.try_admit(&ticket, &s1, need(21), &Measured::NOTHING, t0) {
                    Ok(reservation) => held.push(reservation),
                    Err(_) => break,
                }
            }
            held.len()
        };
        assert_eq!(started(1), 2, "42 of 56; a third would be 63");
        assert_eq!(started(2), 4, "11 each: 44 of 56");
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
