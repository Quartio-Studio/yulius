//! CPU and memory budgets of a sandbox's processes (SPEC-082, ADR-0065).
//!
//! The capacity is what a sandbox's processes may use together: its enforced limits (the
//! isolation adapter's `cpu.max` and `memory.max`), else the host's CPUs and 2 GiB of memory
//! per CPU. Yulius gives out budgets from it and says so in the environment
//! ([`yulius_domain::resources::budget_env`]): CI jobs get their declared or default share and
//! wait for it (`App::ci_reserve`); agent runs, terminals and the Cockpit get the agent budget
//! ([`App::agent_resource_env`]). The repository maps the budget to its tools in
//! `[resources.env]` of the `yulius.toml` where the process starts.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use yulius_domain::resources::{Budget, Capacity, EnvTemplate, budget_env, default_templates};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml_resources_table};

use crate::context::App;

/// Memory assumed per CPU when no limit says how much there is (development hosts).
const MEMORY_PER_CPU_MB: u64 = 2048;

/// The CPUs and MiB of memory CI jobs share (SPEC-082 RES-003), sized once from the capacity.
pub(crate) struct CiPools {
    cpus: Arc<Semaphore>,
    cpus_total: u32,
    memory: Arc<Semaphore>,
    memory_total: u32,
}

impl CiPools {
    fn new(capacity: Capacity) -> Self {
        let memory_total = u32::try_from(capacity.memory_mb).unwrap_or(u32::MAX);
        Self {
            cpus: Arc::new(Semaphore::new(capacity.cpus as usize)),
            cpus_total: capacity.cpus,
            memory: Arc::new(Semaphore::new(memory_total as usize)),
            memory_total,
        }
    }
}

/// What a CI job holds while it runs: its CPUs and memory out of the CI pools.
pub(crate) struct Reservation {
    _cpus: OwnedSemaphorePermit,
    _memory: OwnedSemaphorePermit,
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
/// `<key>.value = …`, `[env.<key>]`, `build = { <key> = … }`, SPEC-272 SHR-005) in one of them (a regular file of at most
/// [`MAX_CONFIG_BYTES`]).
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

    /// Waits until `budget` is free in the CI pools and holds it (SPEC-082 RES-003). Waiters are
    /// served in order, so a large job is never overtaken for good. A budget larger than a pool
    /// (the capacity grew since the pools were sized) takes the whole pool.
    pub(crate) async fn ci_reserve(&self, budget: Budget) -> Option<Reservation> {
        let pools = self.ci.budget.get_or_init(|| CiPools::new(self.capacity()));
        let cpus = budget.cpus.clamp(1, pools.cpus_total);
        let memory = u32::try_from(budget.memory_mb)
            .unwrap_or(u32::MAX)
            .clamp(1, pools.memory_total);
        let cpus = Arc::clone(&pools.cpus)
            .acquire_many_owned(cpus)
            .await
            .ok()?;
        let memory = Arc::clone(&pools.memory)
            .acquire_many_owned(memory)
            .await
            .ok()?;
        Some(Reservation {
            _cpus: cpus,
            _memory: memory,
        })
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
