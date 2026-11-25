//! The sandbox's Docker engine (ADR-0151, SPEC-187).
//!
//! The server describes the engine in the holder spec
//! ([`EngineSpec`](yulius_contracts::sandbox::EngineSpec)): the `bwrap` argv of its file view
//! ([`bwrap_argv`]) and its cgroup. The holder serves the sandbox's Docker socket and starts the
//! engine on the first connection ([`crate::engine_supervisor`]). Inside, `yulius sandbox-host
//! engine` ([`supervise`]) uncovers `/proc/sys` and runs `dockerd` until it exits or the holder
//! closes its stdin.

use std::io::Read;
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use yulius_contracts::sandbox::{
    DOCKER_DIR, DOCKER_DIR_IN_RUNS, DOCKER_SOCKET_LINK, ENGINE_CONTROL_DIR,
    ENGINE_CONTROL_IN_ENGINE, ENGINE_SOCKET_IN_RUNS, ENGINE_SYSFS_DIR, ROOT_LINKS,
    SYSTEM_LAYER_DIRS,
};

/// `PATH` inside the engine.
pub const ENGINE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// Where the engine keeps its images, containers and volumes.
pub const STORAGE_IN_ENGINE: &str = "/var/lib/docker";

/// What the engine's file view is made of (host paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineView {
    /// `bwrap`.
    pub bwrap: String,
    /// The `yulius` executable (bound read-only; it supervises `dockerd`).
    pub exe: String,
    /// Host name inside (the sandbox's slug).
    pub hostname: String,
    /// The holder's state directory (`runtime/sandboxes/<sandbox>`).
    pub state_dir: String,
    /// The merged system layer in the holder's mount namespace.
    pub system_root: String,
    /// Yulius's data directory, covered by an empty tmpfs as in runs.
    pub data_dir: String,
    /// The sandbox directory, bound at its host path as in runs.
    pub sandbox_dir: String,
    /// The engine's storage (`sysroots/<sandbox>/docker`).
    pub storage: String,
    /// The engine's cgroup (`sandboxes/<sandbox>/engine/inside`), its cgroup namespace's root,
    /// below the group that holds its limits (SPEC-483 C4).
    pub cgroup: String,
}

/// The masks the engine of `view` starts with (`sandbox-exec --guard`, SPEC-652 MEM-010): the
/// sandbox home's Claude Code configuration and its Codex homes, as empty read-only folders.
#[must_use]
pub fn engine_guard(view: &EngineView) -> Vec<yulius_contracts::ptyhost::GuardPath> {
    yulius_contracts::ptyhost::sandbox_masks(&view.sandbox_dir)
}

/// The engine's address pools for new networks (SPEC-497 HBA-003): Docker's own default ranges
/// (172.17.0.0 to 172.31.255.255 and 192.168.0.0/16), cut into /24 networks instead of /16 and
/// /20 ones. The engine runs in the sandbox's own network namespace, whose only other network is
/// its link (10.0.2.0/24), so these ranges are free there as they were by default; Docker's
/// default gave 31 networks, which four test runs creating networks at once used up.
pub const ADDRESS_POOLS: [(&str, u32); 5] = [
    ("172.17.0.0/16", 256),
    ("172.18.0.0/15", 512),
    ("172.20.0.0/14", 1024),
    ("172.24.0.0/13", 2048),
    ("192.168.0.0/16", 256),
];
/// The networks an engine can make from [`ADDRESS_POOLS`] (its default `bridge` takes one).
pub const ENGINE_NETWORKS: u32 = 4096;
/// The networks an engine makes from Docker's own default pools (172.17-31.0.0/16 in /16s and
/// 192.168.0.0/16 in /20s): an engine started before [`ADDRESS_POOLS`] (review round 1).
pub const DOCKER_DEFAULT_NETWORKS: u32 = 31;

/// The networks an engine's address pools hold, from its `/info` answer: its
/// `DefaultAddressPools` (`Base` a CIDR, `Size` the prefix of each network), or Docker's own
/// defaults when it names none.
///
/// # Examples
/// ```
/// use yulius_sandbox::engine::pool_networks;
/// let info = serde_json::json!({"DefaultAddressPools": [
///     {"Base": "172.17.0.0/16", "Size": 24}, {"Base": "192.168.0.0/16", "Size": 20}]});
/// assert_eq!(pool_networks(&info), 256 + 16);
/// assert_eq!(pool_networks(&serde_json::json!({})), 31);
/// ```
#[must_use]
pub fn pool_networks(info: &serde_json::Value) -> u32 {
    let Some(pools) = info["DefaultAddressPools"]
        .as_array()
        .filter(|pools| !pools.is_empty())
    else {
        return DOCKER_DEFAULT_NETWORKS;
    };
    pools
        .iter()
        .filter_map(|pool| {
            let prefix: u32 = pool["Base"].as_str()?.rsplit_once('/')?.1.parse().ok()?;
            let size = u32::try_from(pool["Size"].as_u64()?).ok()?;
            let bits = size.checked_sub(prefix).filter(|bits| *bits < 32)?;
            Some(1u32 << bits.min(24))
        })
        .fold(0, u32::saturating_add)
}

/// What an engine's networks take of its address pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Networks {
    /// The networks its pools hold ([`pool_networks`]).
    pub capacity: u32,
    /// The networks it has that take a range (all but `host` and `none`).
    pub used: u32,
}

/// `dockerd` and its arguments inside the engine: its control socket in the control directory
/// (the sandbox's socket at [`ENGINE_SOCKET_IN_RUNS`] is the Docker front's), cgroupfs (no systemd
/// inside), no IPv6 NAT (sandboxes have no IPv6 egress), and [`ADDRESS_POOLS`].
#[must_use]
pub fn dockerd_argv() -> Vec<String> {
    let mut argv: Vec<String> = [
        "dockerd",
        "--host",
        &format!("unix://{ENGINE_CONTROL_IN_ENGINE}/docker.sock"),
        "--data-root",
        STORAGE_IN_ENGINE,
        "--exec-root",
        "/run/docker",
        "--pidfile",
        "/run/docker.pid",
        "--exec-opt",
        "native.cgroupdriver=cgroupfs",
        "--ip6tables=false",
        "--log-level",
        "warn",
    ]
    .map(str::to_owned)
    .to_vec();
    for (base, _) in ADDRESS_POOLS {
        argv.push("--default-address-pool".to_owned());
        argv.push(format!("base={base},size=24"));
    }
    argv
}

/// The full `bwrap` argv that starts the engine (SPEC-187 DKR-003): new PID, IPC, UTS and cgroup
/// namespaces inside the sandbox's user, network and mount namespaces, every capability of the
/// sandbox's user namespace, a cleared environment, and a run's file view plus only the engine's
/// storage, its control directory, its sysfs and cgroup, the sandbox's Docker socket and the
/// `yulius` executable. Then `yulius sandbox-host engine -- dockerd …`.
#[must_use]
pub fn bwrap_argv(view: &EngineView) -> Vec<String> {
    let state = view.state_dir.trim_end_matches('/');
    let root = view.system_root.trim_end_matches('/');
    let mut argv: Vec<String> = [
        view.bwrap.as_str(),
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--hostname",
        &view.hostname,
        "--cap-add",
        "ALL",
        "--clearenv",
        "--setenv",
        "PATH",
        ENGINE_PATH,
        "--setenv",
        "HOME",
        "/root",
        // Networks that restrict inter-container traffic (`enable_icc=false`) are created
        // without the host's `br_netfilter`, as Docker offers; the sandbox's network namespace
        // is its boundary, not ICC (SPEC-494 DFJ-012, ADR-0344).
        "--setenv",
        "DOCKER_IGNORE_BR_NETFILTER_ERROR",
        "1",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--perms",
        "1777",
        "--tmpfs",
        "/dev/shm",
        "--perms",
        "1777",
        "--tmpfs",
        "/tmp",
        "--perms",
        "0755",
        "--tmpfs",
        "/run",
        "--perms",
        "1777",
        "--dir",
        "/run/lock",
    ]
    .map(str::to_owned)
    .to_vec();
    for dir in SYSTEM_LAYER_DIRS {
        argv.extend(["--bind-try".into(), format!("{root}{dir}"), dir.to_owned()]);
    }
    for (target, link) in ROOT_LINKS {
        argv.extend(["--symlink".into(), target.into(), link.into()]);
    }
    argv.extend(["--tmpfs".into(), view.data_dir.clone()]);
    let pairs: [(&str, String, &str); 7] = [
        ("--bind", view.sandbox_dir.clone(), &view.sandbox_dir),
        ("--ro-bind", format!("{state}/{ENGINE_SYSFS_DIR}"), "/sys"),
        ("--bind", view.cgroup.clone(), "/sys/fs/cgroup"),
        ("--bind", view.storage.clone(), STORAGE_IN_ENGINE),
        (
            "--bind",
            format!("{state}/{ENGINE_CONTROL_DIR}"),
            ENGINE_CONTROL_IN_ENGINE,
        ),
        // The sandbox's Docker directory, as runs see it (ADR-0158), for `-v /var/run/docker.sock`.
        (
            "--ro-bind",
            format!("{state}/{DOCKER_DIR}"),
            DOCKER_DIR_IN_RUNS,
        ),
        ("--ro-bind", view.exe.clone(), &view.exe),
    ];
    for (option, source, dest) in pairs {
        argv.extend([option.to_owned(), source, dest.to_owned()]);
    }
    // The sandbox's harness folders, empty and read-only for the engine and so for every
    // container's `-v` (SPEC-652 MEM-010): `sandbox-exec --guard` writes the masks of
    // [`engine_guard`] there.
    argv.extend([
        "--args".into(),
        yulius_contracts::ptyhost::GUARD_ARGS_FD.to_string(),
    ]);
    argv.extend([
        "--symlink".into(),
        DOCKER_SOCKET_LINK.into(),
        ENGINE_SOCKET_IN_RUNS.into(),
    ]);
    argv.extend(["--chdir".into(), "/".into(), "--".into()]);
    argv.extend([view.exe.clone(), "sandbox-host".into(), "engine".into()]);
    argv.push("--".into());
    argv.extend(dockerd_argv());
    argv
}

/// Body of `yulius sandbox-host engine -- <command>` inside the engine's namespaces: uncovers
/// `/proc/sys` (bwrap covers it read-only; root of the sandbox's user namespace can write only its
/// own network's settings there, which `dockerd` needs), then [`supervise_with`] stdin.
///
/// # Errors
/// The command cannot be started.
pub fn supervise(argv: &[String]) -> Result<i32, String> {
    uncover_proc_sys();
    supervise_with(argv, std::io::stdin(), crate::engine_supervisor::STOP_GRACE)
}

#[cfg(target_os = "linux")]
fn uncover_proc_sys() {
    use nix::mount::{MntFlags, umount2};
    if let Err(error) = umount2("/proc/sys", MntFlags::MNT_DETACH) {
        eprintln!("yulius: /proc/sys stays read-only: {error}");
    }
}

#[cfg(not(target_os = "linux"))]
fn uncover_proc_sys() {}

/// Runs `argv` until it exits or `stop` reaches its end; then the command gets `SIGTERM` and,
/// after `grace`, `SIGKILL`. Returns its exit code (128 + the signal when a signal ended it).
///
/// # Errors
/// The command cannot be started or waited for.
pub fn supervise_with(
    argv: &[String],
    stop: impl Read + Send + 'static,
    grace: Duration,
) -> Result<i32, String> {
    let (program, args) = argv.split_first().ok_or("nothing to run")?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    let pid = i32::try_from(child.id()).map_err(|e| e.to_string())?;
    let exited = Arc::new(AtomicBool::new(false));
    let watching = Arc::clone(&exited);
    std::thread::spawn(move || {
        let mut stop = stop;
        let mut buf = [0u8; 256];
        while matches!(stop.read(&mut buf), Ok(n) if n > 0) {}
        let signal = |signal| {
            if !watching.load(Ordering::SeqCst) {
                let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal);
            }
        };
        signal(nix::sys::signal::Signal::SIGTERM);
        let deadline = Instant::now() + grace;
        while !watching.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        signal(nix::sys::signal::Signal::SIGKILL);
    });
    let status = child.wait().map_err(|e| e.to_string());
    exited.store(true, Ordering::SeqCst);
    let status = status?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

/// Bound on pruning one engine, both calls together: the disk upkeep visits sandboxes one after
/// another, and the engine (or a container that replaced its control socket) decides when it
/// answers.
pub const PRUNE_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// Longest prune answer read.
const PRUNE_ANSWER_MAX: u64 = 16 << 20;

/// Prunes the engine behind its control socket `socket` (SPEC-187 DKR-006): the build cache,
/// then images no container uses. Containers and volumes are never touched. Returns the bytes the
/// engine reclaimed (each call's `SpaceReclaimed`). The socket is reached without following a
/// link (the engine can write its directory), and only while the engine runs: nothing here
/// starts it.
///
/// # Errors
/// Both calls failed (the engine is not reachable or refused), or [`PRUNE_TIMEOUT`] passed.
pub async fn prune(socket: &std::path::Path) -> std::io::Result<u64> {
    prune_within(socket, PRUNE_TIMEOUT).await
}

/// [`prune`] with both calls bounded together by `limit`.
///
/// # Errors
/// As [`prune`].
pub async fn prune_within(socket: &std::path::Path, limit: Duration) -> std::io::Result<u64> {
    let deadline = tokio::time::Instant::now() + limit;
    let calls = [
        "/build/prune?all=1",
        // `{"dangling":["false"]}`: every unused image, not only untagged ones.
        "/images/prune?filters=%7B%22dangling%22%3A%5B%22false%22%5D%7D",
    ];
    let mut freed = 0u64;
    let mut last_error = None;
    let mut succeeded = false;
    for path in calls {
        match tokio::time::timeout_at(deadline, post(socket, path)).await {
            Ok(Ok(bytes)) => {
                succeeded = true;
                freed = freed.saturating_add(bytes);
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => last_error = Some(std::io::Error::other(format!("{path} timed out"))),
        }
    }
    match (succeeded, last_error) {
        (false, Some(error)) => Err(error),
        _ => Ok(freed),
    }
}

/// How long a count or prune of the engine's networks may take.
pub const NETWORKS_TIMEOUT: Duration = Duration::from_secs(10);

/// The networks of the engine behind `socket` against its address pools ([`Networks`]), both
/// read within [`NETWORKS_TIMEOUT`] (SPEC-497 HBA-003).
///
/// # Errors
/// The engine is not reachable, refused, answered something else or took too long.
pub async fn networks(socket: &std::path::Path) -> std::io::Result<Networks> {
    let read = async {
        let info = request(socket, "GET", "/info").await?;
        let list = request(socket, "GET", "/networks").await?;
        Ok::<_, std::io::Error>((info, list))
    };
    let (info, list) = tokio::time::timeout(NETWORKS_TIMEOUT, read)
        .await
        .map_err(|_| std::io::Error::other("/networks timed out"))??;
    let list = list
        .as_array()
        .ok_or_else(|| std::io::Error::other("/networks: not a list"))?;
    let ranged = list
        .iter()
        .filter(|network| !matches!(network["Driver"].as_str(), Some("host" | "null")))
        .count();
    Ok(Networks {
        capacity: pool_networks(&info),
        used: u32::try_from(ranged).unwrap_or(u32::MAX),
    })
}

/// `{"until":["10m"]}`: networks made at least 10 minutes ago.
const PRUNE_UNTIL: &str = "%7B%22until%22%3A%5B%2210m%22%5D%7D";

/// Removes the networks of the engine behind `socket` that no container uses and that were made
/// at least 10 minutes ago (one a job or an agent made a moment ago, not attached yet, stays;
/// review round 1), within [`NETWORKS_TIMEOUT`]; how many it removed.
///
/// # Errors
/// As [`networks`].
pub async fn prune_networks(socket: &std::path::Path) -> std::io::Result<u32> {
    let path = format!("/networks/prune?filters={PRUNE_UNTIL}");
    let json = tokio::time::timeout(NETWORKS_TIMEOUT, request(socket, "POST", &path))
        .await
        .map_err(|_| std::io::Error::other("/networks/prune timed out"))??;
    let deleted = json["NetworksDeleted"].as_array().map_or(0, Vec::len);
    Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
}

/// `POST path` over HTTP/1.0 (the engine closes the connection after its answer); the
/// `SpaceReclaimed` of a 200 answer.
async fn post(socket: &std::path::Path, path: &str) -> std::io::Result<u64> {
    let json = request(socket, "POST", path).await?;
    Ok(json["SpaceReclaimed"].as_u64().unwrap_or(0))
}

/// `method path` over HTTP/1.0; the JSON of a 200 answer (at most [`PRUNE_ANSWER_MAX`]).
async fn request(
    socket: &std::path::Path,
    method: &str,
    path: &str,
) -> std::io::Result<serde_json::Value> {
    let (status, body) = call(socket, method, path).await?;
    if status != 200 {
        return Err(std::io::Error::other(format!(
            "{path}: {status} {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    serde_json::from_str(body.trim()).map_err(std::io::Error::other)
}

/// `method path` over HTTP/1.0: the answer's status and body (at most [`PRUNE_ANSWER_MAX`]).
async fn call(
    socket: &std::path::Path,
    method: &str,
    path: &str,
) -> std::io::Result<(u16, String)> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = crate::engine_supervisor::connect_control(socket).await?;
    let request = format!("{method} {path} HTTP/1.0\r\nHost: docker\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut answer = Vec::new();
    stream
        .take(PRUNE_ANSWER_MAX)
        .read_to_end(&mut answer)
        .await?;
    let text = String::from_utf8_lossy(&answer);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("no answer from the engine"))?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("{path}: no status")))?;
    Ok((status, body.to_owned()))
}

/// How long removing one engine's CI leftovers may take, every call together (SPEC-504).
pub const LEFTOVERS_TIMEOUT: Duration = Duration::from_secs(60);
/// Containers or networks one removal looks at, at most.
const LEFTOVERS_MAX: usize = 1000;

/// What [`remove_ci_leftovers`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Leftovers {
    /// Containers removed.
    pub containers: u32,
    /// Networks removed.
    pub networks: u32,
}

impl Leftovers {
    /// Whether nothing was removed.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.containers == 0 && self.networks == 0
    }
}

/// The CI jobs whose leftovers [`remove_ci_leftovers`] removes.
pub enum Jobs<'a> {
    /// The job that just ended.
    One(&'a str),
    /// That job, while this says it ended, asked again for each container and network: a job
    /// id that runs again (a requeued run) keeps its new attempt's (review round 2).
    OneEnded(&'a str, &'a (dyn Fn(&str) -> bool + Sync)),
    /// Every labelled job this says ended.
    Ended(&'a (dyn Fn(&str) -> bool + Sync)),
}

impl Jobs<'_> {
    fn filter(&self) -> String {
        let label = crate::docker_label::CI_JOB_LABEL;
        let value = match self {
            Self::One(job) | Self::OneEnded(job, _) => format!("{label}={job}"),
            Self::Ended(_) => label.to_owned(),
        };
        query_value(&serde_json::json!({ "label": [value] }).to_string())
    }

    fn takes(&self, job: &str) -> bool {
        match self {
            Self::One(one) => *one == job,
            Self::OneEnded(one, ended) => *one == job && ended(job),
            Self::Ended(ended) => ended(job),
        }
    }
}

/// `text` percent-encoded for a query value.
fn query_value(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Whether `id` (an answer of the engine, which runs code its sandbox controls) is safe in a
/// path: 1 to 128 ASCII letters, digits and `:`.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':')
}

/// What [`remove_ci_leftovers`] did: what it removed, and why it stopped early, if it did.
#[derive(Debug, Default)]
pub struct Removal {
    /// Removed before it ended or stopped.
    pub removed: Leftovers,
    /// Why it stopped or skipped something (the first error, or its time bound).
    pub error: Option<std::io::Error>,
}

/// Removes from the engine behind `socket` the containers and then the networks labelled with
/// the CI jobs `jobs` names (SPEC-504 CLB-004, CLB-005), within [`LEFTOVERS_TIMEOUT`]; what was
/// removed counts also when a later call fails or the bound passes (review round 1).
/// Containers are removed forced and keep their volumes; images and volumes are never removed.
/// A container whose labels came from its image (its
/// [`crate::docker_label::CI_CREATE_LABEL`] value is its image's: an image committed from a
/// job's container) was not made by a job's create, and is kept; so is one whose image cannot
/// be read, as nothing then tells it from an agent's (ADR-0359). A network still in use (a
/// container the job did not make joined it) stays. A call that fails skips its container or
/// network, not the rest. Reached only while the engine runs: nothing here starts it.
pub async fn remove_ci_leftovers(socket: &std::path::Path, jobs: &Jobs<'_>) -> Removal {
    let mut removal = Removal::default();
    let bounded = tokio::time::timeout(
        LEFTOVERS_TIMEOUT,
        remove_leftovers(socket, jobs, &mut removal),
    );
    if bounded.await.is_err() {
        removal
            .error
            .get_or_insert_with(|| std::io::Error::other("removing CI leftovers timed out"));
    }
    removal
}

/// The labels of a listed container, network or image's configuration.
fn label<'a>(labels: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    labels[name].as_str()
}

async fn remove_leftovers(socket: &std::path::Path, jobs: &Jobs<'_>, removal: &mut Removal) {
    use crate::docker_label::{CI_CREATE_LABEL, CI_JOB_LABEL};
    let failed = |error: std::io::Error, removal: &mut Removal| {
        removal.error.get_or_insert(error);
    };
    let filter = jobs.filter();
    match request(
        socket,
        "GET",
        &format!("/containers/json?all=1&filters={filter}"),
    )
    .await
    {
        Ok(listed) => {
            // Each image's labels once: `None` when it cannot be read.
            let mut images: std::collections::HashMap<String, Option<serde_json::Value>> =
                std::collections::HashMap::new();
            for container in listed.as_array().into_iter().flatten().take(LEFTOVERS_MAX) {
                let labels = &container["Labels"];
                let (Some(id), Some(job)) = (container["Id"].as_str(), label(labels, CI_JOB_LABEL))
                else {
                    continue;
                };
                let image = container["ImageID"].as_str().unwrap_or_default();
                if !plain_id(id) || !plain_id(image) || !jobs.takes(job) {
                    continue;
                }
                if !images.contains_key(image) {
                    let read = match call(socket, "GET", &format!("/images/{image}/json")).await {
                        Ok((200, body)) => serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .map(|json| json["Config"]["Labels"].clone()),
                        Ok(_) => None,
                        Err(error) => {
                            failed(error, removal);
                            None
                        }
                    };
                    images.insert(image.to_owned(), read);
                }
                let Some(Some(image_labels)) = images.get(image) else {
                    // The image cannot be read: kept (ADR-0359).
                    continue;
                };
                let own = label(labels, CI_CREATE_LABEL);
                let inherited = match own {
                    Some(value) => label(image_labels, CI_CREATE_LABEL) == Some(value),
                    None => label(image_labels, CI_JOB_LABEL) == Some(job),
                };
                if inherited {
                    continue;
                }
                match call(socket, "DELETE", &format!("/containers/{id}?force=1")).await {
                    Ok((204, _)) => removal.removed.containers += 1,
                    Ok(_) => {}
                    Err(error) => failed(error, removal),
                }
            }
        }
        Err(error) => failed(error, removal),
    }
    match request(socket, "GET", &format!("/networks?filters={filter}")).await {
        Ok(listed) => {
            for network in listed.as_array().into_iter().flatten().take(LEFTOVERS_MAX) {
                let (Some(id), Some(job)) = (
                    network["Id"].as_str(),
                    label(&network["Labels"], CI_JOB_LABEL),
                ) else {
                    continue;
                };
                if !plain_id(id) || !jobs.takes(job) {
                    continue;
                }
                match call(socket, "DELETE", &format!("/networks/{id}")).await {
                    Ok((204, _)) => removal.removed.networks += 1,
                    Ok(_) => {}
                    Err(error) => failed(error, removal),
                }
            }
        }
        Err(error) => failed(error, removal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    /// Closes `write` (the command's stop signal) once the command wrote `ready`: its trap is
    /// set by then, so the signal never races it.
    fn close_when_ready(
        write: std::os::fd::OwnedFd,
        ready: std::path::PathBuf,
    ) -> std::thread::JoinHandle<bool> {
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !ready.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(write);
            ready.exists()
        })
    }

    #[test]
    fn closing_stdin_stops_the_command_gracefully() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (marker, ready) = (dir.path().join("term"), dir.path().join("ready"));
        let (read, write) = crate::userns::cloexec_pipe()?;
        let script = format!(
            "trap 'echo term > {}; exit 0' TERM; touch {}; while :; do sleep 0.05; done",
            marker.display(),
            ready.display()
        );
        let closer = close_when_ready(write, ready);
        let code = supervise_with(
            &sh(&script),
            std::fs::File::from(read),
            Duration::from_secs(20),
        )?;
        assert!(
            closer.join().map_err(|_| "closer panicked")?,
            "the command got ready"
        );
        assert_eq!(code, 0);
        assert_eq!(std::fs::read_to_string(&marker)?.trim(), "term");
        Ok(())
    }

    #[test]
    fn a_command_that_ignores_term_is_killed_after_the_grace()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ready = dir.path().join("ready");
        let (read, write) = crate::userns::cloexec_pipe()?;
        let closer = close_when_ready(write, ready.clone());
        let started = Instant::now();
        let code = supervise_with(
            &sh(&format!(
                "trap '' TERM; touch {}; while :; do sleep 0.05; done",
                ready.display()
            )),
            std::fs::File::from(read),
            Duration::from_millis(300),
        )?;
        assert!(
            closer.join().map_err(|_| "closer panicked")?,
            "the command got ready"
        );
        assert_eq!(code, 128 + 9);
        assert!(started.elapsed() < Duration::from_secs(20));
        Ok(())
    }

    #[test]
    fn the_commands_own_exit_ends_the_supervision() -> Result<(), Box<dyn std::error::Error>> {
        let (read, write) = crate::userns::cloexec_pipe()?;
        let code = supervise_with(
            &sh("exit 3"),
            std::fs::File::from(read),
            Duration::from_secs(20),
        )?;
        drop(write);
        assert_eq!(code, 3);
        assert!(supervise_with(&[], std::io::empty(), Duration::ZERO).is_err());
        Ok(())
    }

    /// A fake engine API answering each prune with `answers[path]` (status, body) and recording
    /// the request heads.
    async fn fake_api(
        socket: std::path::PathBuf,
        answers: Vec<(&'static str, u16, &'static str)>,
    ) -> std::io::Result<(
        tokio::task::JoinHandle<()>,
        Arc<std::sync::Mutex<Vec<String>>>,
    )> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let heads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&heads);
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                if let Ok(mut heads) = seen.lock() {
                    heads.push(head);
                }
                if answers.iter().any(|(p, s, _)| *p == path && *s == 0) {
                    // Closed unanswered.
                    continue;
                }
                let (status, body) = answers
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map_or((404, "{}"), |(_, s, b)| (*s, *b));
                let response =
                    format!("HTTP/1.0 {status} X\r\nContent-Type: application/json\r\n\r\n{body}");
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok((server, heads))
    }

    const ONE: &str = "%7B%22label%22%3A%5B%22yulius.ci.job%3Dj1%22%5D%7D";
    const ANY: &str = "%7B%22label%22%3A%5B%22yulius.ci.job%22%5D%7D";

    /// SPEC-504 CLB-004: a job's end removes its containers, forced and keeping their volumes,
    /// then its networks; a container whose labels came from its image (its create value is the
    /// image's), one whose image cannot be read, another job's, an id unsafe in a path and a
    /// network still in use stay; images and volumes are never removed. A container the job made
    /// from an image committed from its own container (another create value) goes (review round
    /// 1).
    #[tokio::test]
    async fn a_jobs_end_removes_its_containers_then_its_networks()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let containers = format!("/containers/json?all=1&filters={ONE}");
        let networks = format!("/networks?filters={ONE}");
        let (server, heads) = fake_api(
            socket.clone(),
            vec![
                (
                    Box::leak(containers.into_boxed_str()),
                    200,
                    r#"[{"Id":"c1","ImageID":"sha256:i1","Labels":{"yulius.ci.job":"j1","yulius.ci.create":"k1"}},
                        {"Id":"c2","ImageID":"sha256:i2","Labels":{"yulius.ci.job":"j1","yulius.ci.create":"k0"}},
                        {"Id":"c5","ImageID":"sha256:i2","Labels":{"yulius.ci.job":"j1","yulius.ci.create":"k5"}},
                        {"Id":"c4","ImageID":"sha256:gone","Labels":{"yulius.ci.job":"j1","yulius.ci.create":"k4"}},
                        {"Id":"c3","ImageID":"sha256:i1","Labels":{"yulius.ci.job":"j2"}},
                        {"Id":"../x","ImageID":"sha256:i1","Labels":{"yulius.ci.job":"j1"}}]"#,
                ),
                ("/images/sha256:i1/json", 200, r#"{"Config":{"Labels":null}}"#),
                ("/images/sha256:gone/json", 404, r#"{"message":"gone"}"#),
                (
                    "/images/sha256:i2/json",
                    200,
                    r#"{"Config":{"Labels":{"yulius.ci.job":"j1","yulius.ci.create":"k0"}}}"#,
                ),
                ("/containers/c1?force=1", 204, ""),
                ("/containers/c5?force=1", 204, ""),
                (
                    Box::leak(networks.into_boxed_str()),
                    200,
                    r#"[{"Id":"n1","Labels":{"yulius.ci.job":"j1"}},
                        {"Id":"n2","Labels":{"yulius.ci.job":"j1"}}]"#,
                ),
                ("/networks/n1", 204, ""),
                ("/networks/n2", 403, r#"{"message":"has active endpoints"}"#),
            ],
        )
        .await?;
        let removal = remove_ci_leftovers(&socket, &Jobs::One("j1")).await;
        assert!(removal.error.is_none(), "{:?}", removal.error);
        assert_eq!(
            removal.removed,
            Leftovers {
                containers: 2,
                networks: 1
            }
        );
        let heads = heads.lock().map_err(|_| "poisoned")?.clone();
        let deletes: Vec<&str> = heads
            .iter()
            .filter(|h| h.starts_with("DELETE "))
            .map(|h| h.split_whitespace().nth(1).unwrap_or_default())
            .collect();
        assert_eq!(
            deletes,
            [
                "/containers/c1?force=1",
                "/containers/c5?force=1",
                "/networks/n1",
                "/networks/n2"
            ],
            "containers first, never volumes or images; c2 got its labels from its image (the \
             same create value), c4's image cannot be read: {heads:?}"
        );
        server.abort();
        Ok(())
    }

    /// SPEC-504 CLB-005: the sweep lists every CI job's leftovers and removes those of jobs that
    /// ended; a running job's stay. An engine that is not there is an error, not a removal.
    #[tokio::test]
    async fn the_sweep_removes_only_ended_jobs_leftovers() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let containers = format!("/containers/json?all=1&filters={ANY}");
        let networks = format!("/networks?filters={ANY}");
        let (server, heads) = fake_api(
            socket.clone(),
            vec![
                (
                    Box::leak(containers.into_boxed_str()),
                    200,
                    r#"[{"Id":"a","ImageID":"sha256:i","Labels":{"yulius.ci.job":"gone"}},
                        {"Id":"b","ImageID":"sha256:i","Labels":{"yulius.ci.job":"running"}},
                        {"Id":"c","ImageID":"sha256:i","Labels":{}}]"#,
                ),
                (
                    "/images/sha256:i/json",
                    200,
                    r#"{"Config":{"Labels":null}}"#,
                ),
                ("/containers/a?force=1", 204, ""),
                (
                    Box::leak(networks.into_boxed_str()),
                    200,
                    r#"[{"Id":"n","Labels":{"yulius.ci.job":"running"}}]"#,
                ),
            ],
        )
        .await?;
        let ended = |job: &str| job == "gone";
        let removal = remove_ci_leftovers(&socket, &Jobs::Ended(&ended)).await;
        assert!(removal.error.is_none(), "{:?}", removal.error);
        assert_eq!(
            removal.removed,
            Leftovers {
                containers: 1,
                networks: 0
            }
        );
        let heads = heads.lock().map_err(|_| "poisoned")?.clone();
        assert_eq!(
            heads.iter().filter(|h| h.starts_with("DELETE ")).count(),
            1,
            "{heads:?}"
        );
        server.abort();
        let none = remove_ci_leftovers(&dir.path().join("none.sock"), &Jobs::One("j1")).await;
        assert!(none.error.is_some() && none.removed.is_empty());
        Ok(())
    }

    /// Review round 1 (minor): what was removed before a call failed counts, and a failed call
    /// skips only its own container: the networks are still removed.
    #[tokio::test]
    async fn removals_before_a_failure_count() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let containers = format!("/containers/json?all=1&filters={ONE}");
        let networks = format!("/networks?filters={ONE}");
        let (server, _) = fake_api(
            socket.clone(),
            vec![
                (
                    Box::leak(containers.into_boxed_str()),
                    200,
                    r#"[{"Id":"c1","ImageID":"sha256:i","Labels":{"yulius.ci.job":"j1","yulius.ci.create":"a"}},
                        {"Id":"c2","ImageID":"sha256:i","Labels":{"yulius.ci.job":"j1","yulius.ci.create":"b"}}]"#,
                ),
                ("/images/sha256:i/json", 200, r#"{"Config":{"Labels":{}}}"#),
                ("/containers/c1?force=1", 204, ""),
                // No answer at all: the call fails.
                ("/containers/c2?force=1", 0, ""),
                (
                    Box::leak(networks.into_boxed_str()),
                    200,
                    r#"[{"Id":"n1","Labels":{"yulius.ci.job":"j1"}}]"#,
                ),
                ("/networks/n1", 204, ""),
            ],
        )
        .await?;
        let removal = remove_ci_leftovers(&socket, &Jobs::One("j1")).await;
        assert!(removal.error.is_some());
        assert_eq!(
            removal.removed,
            Leftovers {
                containers: 1,
                networks: 1
            }
        );
        server.abort();
        Ok(())
    }

    /// SPEC-497 HBA-003: the networks that take an address range are counted (not `host` and
    /// `none`), and unused ones pruned with a GET and a POST; a refusal is an error.
    #[tokio::test]
    async fn the_engines_networks_are_counted_and_pruned() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let (server, heads) = fake_api(
            socket.clone(),
            vec![
                (
                    "/info",
                    200,
                    r#"{"DefaultAddressPools":[{"Base":"172.17.0.0/16","Size":24},{"Base":"192.168.0.0/16","Size":24}]}"#,
                ),
                (
                    "/networks",
                    200,
                    r#"[{"Name":"bridge","Driver":"bridge"},{"Name":"host","Driver":"host"},{"Name":"none","Driver":"null"},{"Name":"t1","Driver":"bridge"},{"Name":"o","Driver":"overlay"}]"#,
                ),
                (
                    "/networks/prune?filters=%7B%22until%22%3A%5B%2210m%22%5D%7D",
                    200,
                    r#"{"NetworksDeleted":["t1","t2"]}"#,
                ),
            ],
        )
        .await?;
        assert_eq!(
            networks(&socket).await?,
            Networks {
                capacity: 512,
                used: 3
            }
        );
        assert_eq!(prune_networks(&socket).await?, 2);
        let heads = heads.lock().map_err(|_| "poisoned")?.clone();
        assert!(heads[0].starts_with("GET /info "), "{heads:?}");
        assert!(heads[1].starts_with("GET /networks "), "{heads:?}");
        // Only networks made at least 10 minutes ago.
        assert!(
            heads[2]
                .starts_with("POST /networks/prune?filters=%7B%22until%22%3A%5B%2210m%22%5D%7D "),
            "{heads:?}"
        );
        server.abort();
        // An engine with Docker's default pools (it names none) holds 31.
        let old = dir.path().join("old.sock");
        let (server, _) = fake_api(
            old.clone(),
            vec![
                ("/info", 200, "{}"),
                ("/networks", 200, r#"[{"Name":"bridge","Driver":"bridge"}]"#),
            ],
        )
        .await?;
        assert_eq!(
            networks(&old).await?,
            Networks {
                capacity: 31,
                used: 1
            }
        );
        server.abort();
        let refusing = dir.path().join("refusing.sock");
        let (server, _) = fake_api(refusing.clone(), vec![("/info", 500, "{}")]).await?;
        assert!(networks(&refusing).await.is_err());
        server.abort();
        Ok(())
    }

    const BUILD_PRUNE: &str = "/build/prune?all=1";
    const IMAGE_PRUNE: &str = "/images/prune?filters=%7B%22dangling%22%3A%5B%22false%22%5D%7D";

    #[tokio::test]
    async fn pruning_frees_the_build_cache_and_images_no_container_uses()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let (server, heads) = fake_api(
            socket.clone(),
            vec![
                (
                    BUILD_PRUNE,
                    200,
                    r#"{"CachesDeleted":["a","b"],"SpaceReclaimed":1000}"#,
                ),
                (
                    IMAGE_PRUNE,
                    200,
                    r#"{"ImagesDeleted":[{"Untagged":"x"}],"SpaceReclaimed":234}"#,
                ),
            ],
        )
        .await?;
        assert_eq!(prune(&socket).await?, 1234);
        let heads = heads.lock().map_err(|_| "poisoned")?.clone();
        assert_eq!(heads.len(), 2, "{heads:?}");
        for head in &heads {
            assert!(head.starts_with("POST "), "{head}");
        }
        // Never containers or volumes.
        assert!(
            heads
                .iter()
                .all(|h| !h.contains("/containers/") && !h.contains("/volumes/"))
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn one_failed_prune_keeps_the_other_and_two_are_an_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let (server, _) = fake_api(
            socket.clone(),
            vec![
                (BUILD_PRUNE, 500, r#"{"message":"boom"}"#),
                (IMAGE_PRUNE, 200, r#"{"SpaceReclaimed":234}"#),
            ],
        )
        .await?;
        assert_eq!(prune(&socket).await?, 234);
        server.abort();
        let none = dir.path().join("none.sock");
        assert!(prune(&none).await.is_err());
        Ok(())
    }

    /// Review round 3 (low): an engine (or a container that replaced its control socket) that
    /// accepts and never answers holds the upkeep only until one short deadline for both calls.
    #[tokio::test]
    async fn a_silent_engine_holds_pruning_only_until_its_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let started = Instant::now();
        assert!(
            prune_within(&socket, Duration::from_millis(400))
                .await
                .is_err()
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(PRUNE_TIMEOUT <= Duration::from_secs(120));
        silent.abort();
        Ok(())
    }

    fn view() -> EngineView {
        EngineView {
            bwrap: "/usr/bin/bwrap".into(),
            exe: "/usr/local/bin/yulius".into(),
            hostname: "fixture".into(),
            state_dir: "/data/runtime/sandboxes/s1".into(),
            system_root: "/data/runtime/sandboxes/s1/root".into(),
            data_dir: "/data".into(),
            sandbox_dir: "/data/sandboxes/s1".into(),
            storage: "/data/sysroots/s1/docker".into(),
            cgroup: "/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1/engine".into(),
        }
    }

    fn position(argv: &[String], window: &[&str]) -> Option<usize> {
        argv.windows(window.len())
            .position(|w| w.iter().zip(window).all(|(a, b)| a == b))
    }

    /// Sources of every bind, as (option, source, destination).
    fn binds(argv: &[String]) -> Vec<(String, String, String)> {
        let options = [
            "--bind",
            "--ro-bind",
            "--bind-try",
            "--dev-bind",
            "--ro-bind-try",
        ];
        argv.windows(3)
            .filter(|w| options.contains(&w[0].as_str()))
            .map(|w| (w[0].clone(), w[1].clone(), w[2].clone()))
            .collect()
    }

    #[test]
    fn the_engine_gets_its_own_namespaces_capabilities_and_environment() {
        let argv = bwrap_argv(&view());
        assert_eq!(argv[0], "/usr/bin/bwrap");
        for flag in [
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--clearenv",
        ] {
            assert!(argv.contains(&flag.to_owned()), "{flag}: {argv:?}");
        }
        // Never a new user or network namespace: the sandbox's own are the boundary.
        for flag in [
            "--unshare-user",
            "--unshare-net",
            "--unshare-all",
            "--share-net",
        ] {
            assert!(!argv.contains(&flag.to_owned()), "{flag}: {argv:?}");
        }
        assert!(position(&argv, &["--cap-add", "ALL"]).is_some());
        assert!(!argv.contains(&"--cap-drop".to_owned()));
        assert!(position(&argv, &["--setenv", "PATH", ENGINE_PATH]).is_some());
        assert!(position(&argv, &["--setenv", "HOME", "/root"]).is_some());
        // SPEC-494 DFJ-012: networks that restrict inter-container traffic are created without
        // the host's `br_netfilter`.
        assert!(
            position(
                &argv,
                &["--setenv", "DOCKER_IGNORE_BR_NETFILTER_ERROR", "1"]
            )
            .is_some(),
            "{argv:?}"
        );
        assert!(position(&argv, &["--hostname", "fixture"]).is_some());
    }

    #[test]
    fn the_engine_sees_a_runs_view_plus_only_its_own_parts() {
        let argv = bwrap_argv(&view());
        let found = binds(&argv);
        let state = "/data/runtime/sandboxes/s1";
        let mut expected: Vec<(String, String, String)> = SYSTEM_LAYER_DIRS
            .iter()
            .map(|d| ("--bind-try".into(), format!("{state}/root{d}"), (*d).into()))
            .collect();
        for (option, source, dest) in [
            ("--bind", "/data/sandboxes/s1", "/data/sandboxes/s1"),
            ("--ro-bind", "/data/runtime/sandboxes/s1/sys", "/sys"),
            (
                "--bind",
                "/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1/engine",
                "/sys/fs/cgroup",
            ),
            ("--bind", "/data/sysroots/s1/docker", "/var/lib/docker"),
            (
                "--bind",
                "/data/runtime/sandboxes/s1/engine",
                "/run/yulius-engine",
            ),
            (
                "--ro-bind",
                "/data/runtime/sandboxes/s1/docker",
                "/run/yulius/docker",
            ),
            (
                "--ro-bind",
                "/usr/local/bin/yulius",
                "/usr/local/bin/yulius",
            ),
        ] {
            expected.push((option.into(), source.into(), dest.into()));
        }
        assert_eq!(found, expected);
        // Nothing else of the holder's state (relay sockets, records, the overlay's internals),
        // no host `/sys`, no host cgroup tree, no device binds.
        for (_, source, _) in &found {
            assert_ne!(source, "/sys");
            assert_ne!(source, "/sys/fs/cgroup");
            assert_ne!(source, "/");
            for private in [
                "in.sock",
                "out.sock",
                "holder.json",
                "host.json",
                "upper",
                "work",
            ] {
                assert!(!source.ends_with(private), "{source}");
            }
        }
    }

    #[test]
    fn mounts_come_in_an_order_that_keeps_each_one_visible() {
        let argv = bwrap_argv(&view());
        let state = "/data/runtime/sandboxes/s1";
        let var = position(&argv, &["--bind-try", &format!("{state}/root/var"), "/var"]);
        let run = position(&argv, &["--tmpfs", "/run"]);
        let private = position(&argv, &["--tmpfs", "/data"]);
        let sandbox = position(&argv, &["--bind", "/data/sandboxes/s1"]);
        let storage = position(&argv, &["--bind", "/data/sysroots/s1/docker"]);
        let sys = position(&argv, &["--ro-bind", &format!("{state}/sys"), "/sys"]);
        let cgroup = position(&argv, &["--bind", &view().cgroup, "/sys/fs/cgroup"]);
        let socket = position(&argv, &["--ro-bind", &format!("{state}/docker")]);
        let link = position(
            &argv,
            &["--symlink", "yulius/docker/docker.sock", "/run/docker.sock"],
        );
        assert!(socket < link, "{argv:?}");
        let exe = position(&argv, &["--ro-bind", "/usr/local/bin/yulius"]);
        assert!(run < var && var < private && private < sandbox, "{argv:?}");
        assert!(var < storage && sys < cgroup && run < socket, "{argv:?}");
        assert!(
            private < exe,
            "the executable may live in the data directory: {argv:?}"
        );
    }

    /// SPEC-652 MEM-010: the masks of the sandbox's harness folders come after the sandbox's
    /// bind (which they cover) and before the engine starts; they are the home's `.claude` and
    /// the Codex homes, as empty read-only folders.
    #[test]
    fn the_engine_sees_the_harness_folders_masked() {
        let argv = bwrap_argv(&view());
        let sandbox = position(&argv, &["--bind", "/data/sandboxes/s1"]);
        let masks = position(&argv, &["--args", "199"]);
        let chdir = position(&argv, &["--chdir", "/"]);
        assert!(sandbox < masks && masks < chdir, "{argv:?}");
        let guard = engine_guard(&view());
        let shown: Vec<(&str, yulius_contracts::ptyhost::GuardKind)> =
            guard.iter().map(|p| (p.dest.as_str(), p.kind)).collect();
        assert_eq!(
            shown,
            [
                (
                    "/data/sandboxes/s1/home/.claude",
                    yulius_contracts::ptyhost::GuardKind::Mask
                ),
                (
                    "/data/sandboxes/s1/codex",
                    yulius_contracts::ptyhost::GuardKind::Mask
                ),
            ]
        );
    }

    #[test]
    fn the_engine_command_is_the_supervised_dockerd() {
        let argv = bwrap_argv(&view());
        let tail = position(
            &argv,
            &[
                "--chdir",
                "/",
                "--",
                "/usr/local/bin/yulius",
                "sandbox-host",
                "engine",
                "--",
            ],
        );
        assert!(tail.is_some(), "{argv:?}");
        let dockerd = dockerd_argv();
        assert_eq!(&argv[argv.len() - dockerd.len()..], dockerd.as_slice());
        assert!(
            position(
                &dockerd,
                &["--host", "unix:///run/yulius-engine/docker.sock"]
            )
            .is_some()
        );
        assert!(position(&dockerd, &["--data-root", "/var/lib/docker"]).is_some());
        assert!(position(&dockerd, &["--exec-opt", "native.cgroupdriver=cgroupfs"]).is_some());
        // SPEC-497 HBA-003: /24 networks within Docker's own default ranges.
        let pools: Vec<&String> = dockerd
            .iter()
            .skip_while(|a| *a != "--default-address-pool")
            .step_by(2)
            .collect();
        assert_eq!(pools.len(), ADDRESS_POOLS.len(), "{dockerd:?}");
        assert!(
            position(
                &dockerd,
                &["--default-address-pool", "base=172.24.0.0/13,size=24"]
            )
            .is_some()
        );
        let configured = serde_json::json!({"DefaultAddressPools": ADDRESS_POOLS
            .iter()
            .map(|(base, _)| serde_json::json!({"Base": base, "Size": 24}))
            .collect::<Vec<_>>()});
        assert_eq!(pool_networks(&configured), ENGINE_NETWORKS);
        let total: u32 = ADDRESS_POOLS
            .iter()
            .map(|(base, networks)| {
                let bits: u32 = base
                    .rsplit('/')
                    .next()
                    .and_then(|b| b.parse().ok())
                    .unwrap_or(0);
                assert_eq!(1 << (24 - bits), *networks, "{base}");
                networks
            })
            .sum();
        assert_eq!(total, ENGINE_NETWORKS);
        assert!(
            !dockerd.iter().any(|a| a.contains("/run/docker.sock")),
            "{dockerd:?}"
        );
    }
}
