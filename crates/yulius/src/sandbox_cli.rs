//! `yulius sandbox-host`, `sandbox-exec` and `sandbox-base` (SPEC-002 ADR-22, ADR-0151).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use yulius_sandbox::base::{BuildConfig, OwnerEntry};
use yulius_sandbox::userns::Mapper;

use crate::wiring::sandbox::{Egress, IsolationArgs, resolve};

/// `yulius sandbox-host …` (internal).
#[derive(Debug, Subcommand)]
pub enum HostAction {
    /// Create a sandbox's namespaces and holder, then exit (the holder stays).
    Start {
        /// The holder spec (`runtime/sandboxes/<sandbox>/holder.json`).
        #[arg(long)]
        spec: PathBuf,
    },
    /// The holder inside the namespaces.
    Inner {
        /// The holder spec.
        #[arg(long)]
        spec: PathBuf,
        /// Descriptor on which readiness is reported.
        #[arg(long)]
        ready_fd: i32,
    },
    /// The sandbox's Docker engine inside its namespaces: runs the command until it exits or
    /// stdin closes (ADR-0151).
    Engine {
        /// `dockerd` and its arguments, after `--`.
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    /// The sandbox's Docker front beside its holder: serves `docker/docker.sock` until stopped
    /// (ADR-0158).
    Docker {
        /// The front spec (`runtime/sandboxes/<sandbox>/docker.json`).
        #[arg(long)]
        spec: PathBuf,
    },
    /// Move the sandbox's Docker directory into running runs' mount namespaces (ADR-0158).
    BindDocker {
        /// The front spec.
        #[arg(long)]
        spec: PathBuf,
        /// A process of each run.
        #[arg(long = "pid", required = true)]
        pids: Vec<u32>,
    },
    /// Remove a tree as root of a mapped namespace (files owned by mapped ids).
    RemoveTree {
        /// The tree.
        path: PathBuf,
    },
}

/// `yulius sandbox-base …`.
#[derive(Debug, Subcommand)]
pub enum BaseAction {
    /// Build a new base from this host's system and make it current (as the service user).
    Build {
        /// Data directory.
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius"
        )]
        data_dir: PathBuf,
        /// More bases to keep (comma-separated) when old bases are removed. The current base
        /// and every base a live holder has mounted are always kept.
        #[arg(long, value_delimiter = ',')]
        keep: Vec<String>,
    },
    /// Show the current base.
    Status {
        /// Data directory.
        #[arg(
            long = "data-dir",
            visible_alias = "data",
            env = "YULIUS_DATA",
            default_value = "/var/lib/yulius"
        )]
        data_dir: PathBuf,
    },
    /// Internal: give the recorded files of a base their owners (inside a mapped namespace).
    #[command(hide = true)]
    Chown {
        /// The base being built.
        #[arg(long)]
        root: PathBuf,
        /// The owners to apply (JSON).
        #[arg(long)]
        manifest: PathBuf,
    },
}

fn fail(message: &str) -> ExitCode {
    eprintln!("yulius: {message}");
    ExitCode::FAILURE
}

/// Runs a `sandbox-host` action.
#[must_use]
pub fn host(action: HostAction) -> ExitCode {
    match action {
        HostAction::Start { spec } => match yulius_sandbox::holder::start(&spec) {
            Ok(record) => {
                println!("holder {} ready", record.inner_pid);
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error),
        },
        HostAction::Inner { spec, ready_fd } => {
            match yulius_sandbox::holder::inner(&spec, ready_fd) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => fail(&error),
            }
        }
        HostAction::Engine { argv } => match yulius_sandbox::engine::supervise(&argv) {
            Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            Err(error) => fail(&error),
        },
        HostAction::Docker { spec } => match yulius_sandbox::docker_front::run(&spec) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&error),
        },
        HostAction::BindDocker { spec, pids } => {
            let spec: yulius_contracts::sandbox::DockerFrontSpec =
                match yulius_sandbox::holder::read_json(&spec) {
                    Ok(spec) => spec,
                    Err(error) => return fail(&format!("{}: {error}", spec.display())),
                };
            if let Err(error) = spec.validate() {
                return fail(&error.to_string());
            }
            match yulius_sandbox::docker_bind::bind(&spec, &pids) {
                Ok(outcomes) => {
                    for (pid, outcome) in outcomes {
                        println!("{}", outcome.line(pid));
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error),
            }
        }
        HostAction::RemoveTree { path } => {
            yulius_sandbox::base::make_writable(&path);
            match std::fs::remove_dir_all(&path) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ExitCode::SUCCESS,
                Err(error) => fail(&format!("remove {}: {error}", path.display())),
            }
        }
    }
}

/// `yulius sandbox-exec [--engine] [--cgroup DIR] [--join FILE] -- bwrap …`: joins `cgroup` and
/// the sandbox of `join` (still single-threaded), then execs `bwrap` with the run's seccomp
/// filter, or the engine's (ADR-0151).
#[must_use]
pub fn exec(
    engine: bool,
    cgroup: Option<&Path>,
    join: Option<&Path>,
    argv: &[OsString],
) -> ExitCode {
    if let Some(path) = join {
        let joined =
            yulius_sandbox::holder::read_json::<yulius_contracts::ptyhost::SandboxJoin>(path)
                .map_err(|e| e.to_string())
                .and_then(|join| yulius_sandbox::enter::join(&join).map_err(|e| e.to_string()));
        if let Err(error) = joined {
            return fail(&format!("cannot join the sandbox: {error}"));
        }
    }
    if let Some(cgroup) = cgroup
        && let Err(error) = yulius_sandbox::cgroup::join(cgroup)
    {
        return fail(&format!("cannot join {}: {error}", cgroup.display()));
    }
    let profile = if engine {
        yulius_sandbox::seccomp::Profile::Engine
    } else {
        yulius_sandbox::seccomp::Profile::Run
    };
    let error = yulius_sandbox::enter::exec_bwrap_with(argv, profile);
    fail(&format!("cannot start bwrap: {error}"))
}

fn mapper_for(data_dir: &Path, exe: &Path) -> Result<Mapper, String> {
    let args = IsolationArgs {
        egress: Egress::None,
        ..IsolationArgs::default()
    };
    let config = resolve(&args, data_dir, exe, 1).map_err(|missing| missing.join("; "))?;
    Ok(Mapper {
        bwrap: config.bwrap,
        newuidmap: config.newuidmap,
        newgidmap: config.newgidmap,
        uid: config.uid,
        gid: config.gid,
    })
}

/// Runs a `sandbox-base` action.
#[must_use]
pub fn base(action: BaseAction) -> ExitCode {
    match action {
        BaseAction::Build { data_dir, keep } => {
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(error) => return fail(&error.to_string()),
            };
            let mapper = match mapper_for(&data_dir, &exe) {
                Ok(mapper) => mapper,
                Err(error) => return fail(&format!("sandbox isolation unavailable: {error}")),
            };
            let config = BuildConfig {
                host_root: PathBuf::from("/"),
                data_dir: data_dir.clone(),
                sudo_shim: yulius_app::identity::SUDO_SHIM.to_owned(),
                mapper: mapper.clone(),
                exe: exe.clone(),
            };
            let bases = yulius_sandbox::base::Bases::new(&data_dir);
            let started = std::time::Instant::now();
            match yulius_sandbox::base::build(&config, &bases) {
                Ok(info) => {
                    // Under the build lock, so another build's base is never collected between
                    // its rename and its `current.json` (review of SPEC-203).
                    match yulius_sandbox::base::lock_builds(&bases.dir) {
                        Ok(_lock) => yulius_sandbox::base::collect(&bases, &keep, &mapper, &exe),
                        Err(error) => eprintln!("yulius: old bases kept: the build lock: {error}"),
                    }
                    println!(
                        "sandbox base {} ready in {} s",
                        info.id,
                        started.elapsed().as_secs()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error),
            }
        }
        BaseAction::Status { data_dir } => {
            let bases = yulius_sandbox::base::Bases::new(&data_dir);
            match bases.current() {
                Some(info) => {
                    let fresh = info.fingerprint
                        == yulius_sandbox::base::fingerprint(
                            Path::new("/"),
                            yulius_app::identity::SUDO_SHIM,
                        );
                    println!(
                        "current base: {} ({})",
                        info.id,
                        if fresh {
                            "matches this host's system"
                        } else {
                            "the host's system changed since; a rebuild is due"
                        }
                    );
                    ExitCode::SUCCESS
                }
                None => fail("no sandbox base yet (yulius sandbox-base build)"),
            }
        }
        BaseAction::Chown { root, manifest } => {
            let owners: Vec<OwnerEntry> =
                match yulius_sandbox::holder::read_json_limited(&manifest, 256 << 20) {
                    Ok(owners) => owners,
                    Err(error) => return fail(&format!("manifest: {error}")),
                };
            let failed = yulius_sandbox::base::apply_owners(&root, &owners);
            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                fail(&format!(
                    "{failed} of {} owners could not be applied",
                    owners.len()
                ))
            }
        }
    }
}
