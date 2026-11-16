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
    Admission, Budget, Capacity, ENGINE_NETWORK_RESERVE, EnvTemplate, Held, admit, budget_env,
    default_templates,
};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml_resources_table};

use crate::context::App;

/// Memory assumed per CPU when no limit says how much there is (development hosts).
const MEMORY_PER_CPU_MB: u64 = 2048;
/// The shortest time between two readings of a sandbox's CI and engine groups that gives their
/// CPU rate (SPEC-497 HBA-002), and how often a waiting job looks again.
pub(crate) const SAMPLE_WINDOW: Duration = Duration::from_secs(5);

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
    /// The waiting jobs' tickets, first come first served.
    queue: VecDeque<u64>,
    /// Each measured sandbox's last reading (when, CPU microseconds), its CPU rate over the
    /// window before it (CPUs, once two readings were far enough apart) and its memory.
    samples: HashMap<SandboxId, Sample>,
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    cpu_usec: u64,
    rate: Option<f64>,
    anon_bytes: u64,
}

/// What a CI job holds while it runs: its reservation out of the CI budget, given back when
/// dropped.
pub(crate) struct Reservation {
    admission: Arc<CiAdmission>,
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
        state.queue.retain(|ticket| *ticket != self.id);
        let left = state.queue.len() != before;
        drop(state);
        if left {
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
                queue: VecDeque::new(),
                samples: HashMap::new(),
            }),
            changed: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AdmissionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the readings of the sandboxes running CI jobs (`readings`, taken at `now`): a
    /// sandbox's rate is computed once its last reading is [`SAMPLE_WINDOW`] old; sandboxes no
    /// longer running jobs are forgotten. Returns what they use together.
    fn measure(&self, now: Instant, readings: Vec<(SandboxId, u64, u64)>) -> Option<Budget> {
        let mut state = self.lock();
        let running: Vec<&SandboxId> = readings.iter().map(|(sandbox, _, _)| sandbox).collect();
        state
            .samples
            .retain(|sandbox, _| running.contains(&sandbox));
        for (sandbox, cpu_usec, anon_bytes) in readings {
            let fresh = Sample {
                at: now,
                cpu_usec,
                rate: None,
                anon_bytes,
            };
            let sample = state.samples.entry(sandbox).or_insert(fresh);
            sample.anon_bytes = anon_bytes;
            let elapsed = now.saturating_duration_since(sample.at);
            if cpu_usec < sample.cpu_usec {
                // Its groups were made again: start over.
                *sample = fresh;
            } else if elapsed >= SAMPLE_WINDOW {
                #[allow(clippy::cast_precision_loss)]
                let rate = (cpu_usec - sample.cpu_usec) as f64 / elapsed.as_micros() as f64;
                *sample = Sample {
                    rate: Some(rate),
                    ..fresh
                };
            }
        }
        let rates: Vec<f64> = state.samples.values().filter_map(|s| s.rate).collect();
        let anon: u64 = state
            .samples
            .values()
            .fold(0, |sum, s| sum.saturating_add(s.anon_bytes));
        if rates.is_empty() && anon == 0 {
            return None;
        }
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a sum of CPU rates on a host, rounded up"
        )]
        let cpus = rates.iter().sum::<f64>().ceil().max(0.0) as u32;
        Some(Budget {
            cpus,
            memory_mb: anon >> 20,
        })
    }

    /// Admits the job holding `ticket` when it is first in the queue and `need` fits beside
    /// what CI holds and `measured` ([`admit`]); otherwise what it waits for.
    fn try_admit(
        self: &Arc<Self>,
        ticket: &Ticket,
        need: Budget,
        measured: Option<Budget>,
    ) -> Result<Reservation, Option<Admission>> {
        let mut state = self.lock();
        if state.queue.front() != Some(&ticket.id) {
            return Err(None);
        }
        state.held.measured = measured;
        let need = Budget {
            cpus: need.cpus.min(state.budget.cpus),
            memory_mb: need.memory_mb.min(state.budget.memory_mb),
        };
        match admit(state.budget, &state.held, need) {
            Admission::Now => {
                state.queue.pop_front();
                state.held.jobs += 1;
                state.held.reserved.cpus = state.held.reserved.cpus.saturating_add(need.cpus);
                state.held.reserved.memory_mb =
                    state.held.reserved.memory_mb.saturating_add(need.memory_mb);
                drop(state);
                // The next one in line may fit too.
                self.changed.notify_waiters();
                Ok(Reservation {
                    admission: Arc::clone(self),
                    need,
                })
            }
            wait => Err(Some(wait)),
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

    /// Waits while `sandbox`'s running Docker engine has fewer than
    /// [`ENGINE_NETWORK_RESERVE`] networks left to make (SPEC-497 HBA-003), saying so on the
    /// queued job `job`; with none of the sandbox's CI jobs running, its unused networks are
    /// pruned first. No wait without a running engine (nothing starts one here).
    pub(crate) async fn ci_network_room(&self, sandbox: &SandboxId, job: &str) {
        let Some(isolation) = self.isolation.get() else {
            return;
        };
        let mut told = false;
        loop {
            let Some(free) = isolation.engine_networks_free(sandbox).await else {
                break;
            };
            if free >= ENGINE_NETWORK_RESERVE {
                break;
            }
            if !self.ci_sandboxes_running().contains(sandbox)
                && let Some(removed) = isolation.prune_engine_networks(sandbox).await
                && removed > 0
            {
                continue;
            }
            if !told {
                let why = format!(
                    "waits for Docker networks: its sandbox's engine can make {free} more, and a \
                     job starts with at least {ENGINE_NETWORK_RESERVE} free"
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

    fn ci_admission(&self) -> Arc<CiAdmission> {
        Arc::clone(
            self.ci
                .budget
                .get_or_init(|| Arc::new(CiAdmission::new(self.capacity()))),
        )
    }

    /// What the `ci` and `engine` groups of the sandboxes running CI jobs use now (SPEC-497
    /// HBA-002), as the isolation reads them; `None` where nothing is measured.
    pub(crate) fn ci_measured(&self) -> Option<Budget> {
        let isolation = self.isolation.get()?;
        let readings = self
            .ci_sandboxes_running()
            .into_iter()
            .filter_map(|sandbox| {
                let used = isolation.ci_use(&sandbox)?;
                Some((sandbox, used.cpu_usec, used.anon_bytes))
            })
            .collect();
        self.ci_admission().measure(Instant::now(), readings)
    }

    /// Waits until `budget` fits beside what CI holds on the whole host and holds it (SPEC-082
    /// RES-003, SPEC-497 HBA-001, HBA-002): the larger of the running jobs' reservations and
    /// their sandboxes' measured use. Waiters are served in order, so a large job is never
    /// overtaken for good; a budget larger than the CI budget takes all of it. The queued job
    /// `job` says what it waits for whenever that changes (HBA-005).
    pub(crate) async fn ci_reserve(&self, job: &str, budget: Budget) -> Option<Reservation> {
        let admission = self.ci_admission();
        let ticket = {
            let mut state = admission.lock();
            let id = state.next_ticket;
            state.next_ticket += 1;
            state.queue.push_back(id);
            Ticket {
                admission: Arc::clone(&admission),
                id,
            }
        };
        let mut told: Option<String> = None;
        loop {
            let changed = admission.changed.notified();
            match admission.try_admit(&ticket, budget, self.ci_measured()) {
                Ok(reservation) => {
                    if told.is_some() {
                        self.ci_job_waits(job, None).await;
                    }
                    return Some(reservation);
                }
                Err(wait) => {
                    let why = wait.map_or_else(
                        || "waits for the CI jobs before it in line".to_owned(),
                        |wait| format!("waits for the host's CI budget: {wait}"),
                    );
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
