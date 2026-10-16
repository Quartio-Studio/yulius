//! `yulius doctor --docker-smoke` (SPEC-196 DKL-007): proves on this host that sandboxes get
//! Docker, end to end, the way an update meets them.
//!
//! It needs the service user and a delegated cgroup. Started as root, wherever, it runs itself
//! again (SPEC-202 RSB-001) through `systemd-run --uid=<the data directory's owner> -p
//! Delegate=yes -p DelegateSubgroup=server --wait --pipe` ([`systemd_run_argv`]); as anyone
//! else outside one it says so. Inside, with the current base, it creates a throwaway sandbox (`docker-smoke-<pid>`,
//! next to the real ones but adopted by nobody else) and:
//!
//! 1. starts its holder with Docker off, and a run that has no Docker directory (a run started
//!    before ADR-0158); the run must get the directory and the refusal must name the reason;
//! 2. adopts the holder with Docker on, as an updated server does: the old run must run
//!    `docker run --rm hello-world`, a `docker build` and a two-service `docker compose up`;
//! 3. kills the front: the next upkeep starts another, recreating the socket, and the old run
//!    must still reach the engine;
//! 4. starts a new run, which must have `DOCKER_HOST` and reach the same engine.
//!
//! One line per check; exit 0 only when every check passed. The sandbox is removed at the end.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use yulius_contracts::ptyhost::{BubblewrapSpec, SandboxJoin};
use yulius_sandbox::manager::{DOCKER_OFF, Manager, SandboxRequest};

use crate::wiring::sandbox::{Egress, IsolationArgs, resolve};

/// The throwaway sandbox's host name (and its engine's name).
pub const HOSTNAME: &str = "docker-smoke";
/// Bound on a check that pulls images.
const PULL_WAIT: Duration = Duration::from_secs(15 * 60);
/// Bound on a check that does not.
const WAIT: Duration = Duration::from_secs(90);

/// Why the smoke cannot run: its throwaway sandbox has only the current base's system. It builds
/// and composes, so the host install brings buildx and compose (SPEC-201 DBX-002).
const NO_DOCKERD_IN_BASE: &str = "the current sandbox base has no `dockerd`: an operator installs \
    Docker on the host (docker-ce with docker-buildx-plugin and docker-compose-plugin), waits \
    until `yulius doctor` shows the base rebuilt, and runs the smoke test again";

const DOCKERFILE: &str =
    "FROM busybox\nRUN echo built-in-the-sandbox > /built\nCMD [\"cat\", \"/built\"]\n";
const COMPOSE: &str = "services:\n  web:\n    image: busybox\n    command: [\"sh\", \"-c\", \"mkdir -p /w && echo compose-web > /w/index.html && httpd -f -p 8080 -h /w\"]\n  probe:\n    image: busybox\n    depends_on: [web]\n    command: [\"sh\", \"-c\", \"for i in $$(seq 30); do wget -qO- http://web:8080/ && exit 0; sleep 1; done; exit 1\"]\n";

/// The run started "before engines" (no Docker directory, no `DOCKER_HOST`): it waits for the
/// socket, records the refusal, then runs each phase when the smoke says go.
const OLD_RUN: &str = r#"P="$1"
log() { echo "$1" >> "$P/old.out"; }
wait_for() { i=0; while [ ! -e "$1" ] && [ $i -lt 1800 ]; do sleep 0.1; i=$((i+1)); done; [ -e "$1" ]; }
log started
wait_for /var/run/docker.sock && log socket || { log no-socket; exit 1; }
docker info > "$P/refused.txt" 2>&1; log refused
wait_for "$P/go-engine" || exit 1
docker run --rm hello-world > "$P/hello.txt" 2>&1 && log hello-ok || log hello-failed
{ docker build -q -t yulius-docker-smoke "$P" && docker run --rm yulius-docker-smoke; } > "$P/build.txt" 2>&1 && log build-ok || log build-failed
(cd "$P" && docker compose up --abort-on-container-exit --exit-code-from probe > "$P/compose.txt" 2>&1) && log compose-ok || log compose-failed
(cd "$P" && docker compose down > /dev/null 2>&1)
log engine-done
wait_for "$P/go-again" || exit 1
docker info --format '{{.Name}}' > "$P/again.txt" 2>&1 && log again-ok || log again-failed
log again-done
"#;

/// A run started now: `DOCKER_HOST` and the engine's name.
const NEW_RUN: &str = r#"P="$1"
{ echo "host=${DOCKER_HOST:-unset}"; docker info --format 'name={{.Name}}'; } > "$P/new.txt" 2>&1
echo done >> "$P/new.txt"
"#;

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What was checked.
    pub name: &'static str,
    /// Whether it held.
    pub ok: bool,
    /// What was seen.
    pub detail: String,
}

/// `systemd-run` running the smoke as `uid` in a delegated transient unit.
#[must_use]
pub fn systemd_run_argv(exe: &Path, data_dir: &Path, uid: u32) -> Vec<OsString> {
    let mut argv: Vec<OsString> = [
        "systemd-run",
        "--quiet",
        "--wait",
        "--pipe",
        "--collect",
        &format!("--uid={uid}"),
        "-p",
        "Delegate=yes",
        "-p",
        "DelegateSubgroup=server",
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    argv.push(exe.as_os_str().to_owned());
    argv.extend(["doctor", "--docker-smoke", "--data-dir"].map(OsString::from));
    argv.push(data_dir.as_os_str().to_owned());
    argv
}

/// How the smoke starts, by who runs it.
#[derive(Debug, PartialEq, Eq)]
enum Start<D> {
    /// The checks run here, in the delegation `D`.
    Here(D),
    /// The smoke runs again as this uid in a delegated transient unit.
    AsOwner(u32),
    /// It cannot run: why.
    Refused(String),
}

/// Decides how the smoke starts. `owner` is the data directory's owner, `delegation` this
/// process's delegated cgroup, prepared for sandboxes.
///
/// Root always runs it again as the owner, without looking at its own cgroup: root can write
/// any cgroup's files, so a root login slice would pass as delegated, get a `sandboxes/` group
/// and run the checks as uid 0, which has no subordinate ids (SPEC-202 RSB-001, #617).
fn start<D>(
    root: bool,
    owner: impl FnOnce() -> Result<u32, String>,
    delegation: impl FnOnce() -> Result<D, String>,
) -> Start<D> {
    if root {
        return match owner() {
            Ok(0) => Start::Refused(
                "the data directory is owned by root: sandboxes run as the service user that \
                 owns it, with a subordinate id range (docs/operations.md)"
                    .into(),
            ),
            Ok(uid) => Start::AsOwner(uid),
            Err(why) => Start::Refused(why),
        };
    }
    match delegation() {
        Ok(delegation) => Start::Here(delegation),
        Err(why) => Start::Refused(format!(
            "the smoke test needs a delegated cgroup ({why}); run it as root \
             (`sudo yulius doctor --docker-smoke`), which starts it as the service user in a \
             transient unit"
        )),
    }
}

/// Body of `yulius doctor --docker-smoke`.
#[must_use]
pub fn main(data_dir: &Path) -> ExitCode {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("yulius"));
    let owner = || {
        std::fs::metadata(data_dir)
            .map(|m| m.uid())
            .map_err(|e| format!("{}: {e}", data_dir.display()))
    };
    let delegation = match start(
        nix::unistd::geteuid().is_root(),
        owner,
        crate::wiring::sandbox::delegation,
    ) {
        Start::Here(delegation) => delegation,
        Start::AsOwner(owner) => {
            eprintln!("running in a delegated transient unit as uid {owner}");
            let argv = systemd_run_argv(&exe, data_dir, owner);
            return match std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .status()
            {
                Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
                Err(error) => {
                    eprintln!("yulius: cannot run systemd-run: {error}");
                    ExitCode::from(2)
                }
            };
        }
        Start::Refused(why) => {
            eprintln!("yulius: {why}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("yulius: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(smoke(data_dir, &exe, delegation, &mut |check| {
        let tag = if check.ok { "ok  " } else { "FAIL" };
        println!("{tag}  {:<44} {}", check.name, check.detail);
    })) {
        Ok(checks) if checks.iter().all(|c| c.ok) => {
            println!("sandboxes get Docker on this host");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("yulius: docker smoke test: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the smoke in `data_dir` with `exe` and the cgroup `delegation`, reporting each check
/// to `report` as it ends (see the module docs). The throwaway sandbox is removed whatever
/// happens.
///
/// # Errors
/// The smoke could not start (no base, Docker impossible here, the holder failed).
pub async fn smoke(
    data_dir: &Path,
    exe: &Path,
    delegation: yulius_sandbox::cgroup::Delegation,
    report: &mut dyn FnMut(&Check),
) -> Result<Vec<Check>, String> {
    let args = IsolationArgs {
        egress: Egress::Direct,
        ..IsolationArgs::default()
    };
    let mut config = resolve(&args, data_dir, exe, 1).map_err(|missing| missing.join("; "))?;
    config.forwards.clear();
    config.auto_build = false;
    config.delegation = Some(delegation);
    config.idle.stop_after = Duration::from_secs(60 * 60);
    if yulius_sandbox::base::Bases::new(data_dir)
        .current()
        .is_none()
    {
        return Err("no sandbox base yet: the server builds it in the background".into());
    }
    let docker_on = Manager::new(config.clone());
    if let Some(reason) = docker_on.docker_unavailable() {
        return Err(format!("sandboxes get no engine here: {reason}"));
    }
    drop(docker_on);
    // The smoke's holder runs on the current base (and a throwaway upper layer): one copied
    // without Docker on the host has no engine to start (review round 3; SPEC-199).
    let bases = yulius_sandbox::base::Bases::new(data_dir);
    let base = bases.current().map(|base| bases.path(&base.id));
    if base.is_some_and(|base| {
        !yulius_sandbox::manager::DOCKERD_PATHS
            .iter()
            .any(|path| base.join(path).is_file())
    }) {
        return Err(NO_DOCKERD_IN_BASE.into());
    }
    // A smoke test that was interrupted (Ctrl-C, a crash) left its sandbox: remove it first.
    for stale in stale_smokes(data_dir) {
        eprintln!("removing {stale}, left by an interrupted smoke test");
        remove_sandbox(&config, exe, &stale).await;
    }
    let id = format!("{SMOKE_PREFIX}{}", std::process::id());
    let request = SandboxRequest {
        id: id.clone(),
        hostname: HOSTNAME.into(),
        dir: data_dir.join("sandboxes").join(&id),
        port_first: 1,
        port_last: 1,
    };
    let mut runs = Vec::new();
    let mut checks = Vec::new();
    let result = phases(&config, exe, &request, &mut runs, &mut checks, report).await;
    for mut run in runs {
        let _ = run.kill();
        let _ = run.wait();
    }
    remove_sandbox(&config, exe, &id).await;
    result.map(|()| checks)
}

/// Sandbox names of smoke tests.
const SMOKE_PREFIX: &str = "docker-smoke-";

/// Smoke sandboxes in `data_dir` whose smoke process is gone.
fn stale_smokes(data_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(data_dir.join("runtime").join("sandboxes")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| {
            name.strip_prefix(SMOKE_PREFIX)
                .and_then(|pid| pid.parse::<i32>().ok())
                .is_some_and(|pid| {
                    pid != i32::try_from(std::process::id()).unwrap_or(0)
                        && nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err()
                })
        })
        .collect()
}

/// Stops the smoke sandbox `id`'s holder and front, and removes its files.
async fn remove_sandbox(config: &yulius_sandbox::manager::IsolationConfig, exe: &Path, id: &str) {
    let manager = Manager::new(config.clone());
    manager.adopt_one(id).await;
    manager.stop_all().await;
    let mapper = yulius_sandbox::userns::Mapper {
        bwrap: config.bwrap.clone(),
        newuidmap: config.newuidmap.clone(),
        newgidmap: config.newgidmap.clone(),
        uid: config.uid,
        gid: config.gid,
    };
    let data_dir = &config.data_dir;
    for tree in [
        data_dir.join("sandboxes").join(id),
        data_dir.join("sysroots").join(id),
        data_dir.join("runtime").join("sandboxes").join(id),
    ] {
        if let Err(error) = yulius_sandbox::base::remove_tree(&mapper, exe, &tree) {
            eprintln!("yulius: {error}");
        }
    }
}

async fn phases(
    config: &yulius_sandbox::manager::IsolationConfig,
    exe: &Path,
    request: &SandboxRequest,
    runs: &mut Vec<std::process::Child>,
    checks: &mut Vec<Check>,
    report: &mut dyn FnMut(&Check),
) -> Result<(), String> {
    let mut check = |name, ok, detail: String| {
        let detail = printable(&detail);
        let check = Check { name, ok, detail };
        report(&check);
        checks.push(check);
        ok
    };
    let probe = request.dir.join("probe");
    for dir in [request.dir.join("home"), probe.clone()] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    for (name, text) in [
        ("Dockerfile", DOCKERFILE),
        ("compose.yaml", COMPOSE),
        ("old.sh", OLD_RUN),
        ("new.sh", NEW_RUN),
    ] {
        std::fs::write(probe.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    }
    let out = probe.join("old.out");

    // 1. A holder that serves no engine, and a run started then, without the Docker directory.
    let mut off = config.clone();
    off.docker = Err(DOCKER_OFF.into());
    let before = Manager::new(off);
    let joined = before
        .prepare(request)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no sandbox base")?;
    let mut old_join = joined.join.clone();
    old_join.docker_dir = None;
    runs.push(spawn_run(exe, &old_join, request, &probe, "old.sh")?);
    before.release(&request.id).await;
    wait_line(&out, "started", WAIT).await?;
    before.tick().await;
    let bound = wait_line(&out, "refused", WAIT).await.is_ok();
    check(
        "a run older than the engine gets the Docker directory",
        bound,
        read(&out),
    );
    let refused = read(&probe.join("refused.txt"));
    check(
        "a sandbox without Docker answers why",
        refused.contains("not available") && refused.contains("YULIUS_SANDBOX_DOCKER"),
        first_line(&refused),
    );
    drop(before);
    // A holder from before engines has no engine storage either (review round 1): the front
    // must make it.
    let _ = std::fs::remove_dir(
        config
            .data_dir
            .join("sysroots")
            .join(&request.id)
            .join("docker"),
    );

    // 2. An updated server adopts the holder with Docker on: the old run uses the engine.
    let after = Manager::new(config.clone());
    let adopted = after.adopt_one(&request.id).await;
    let front = after.docker_front(&request.id).await;
    // A refusal's reason is the answer the operator needs (review round 3), as the sandbox is
    // removed at the end.
    let detail = match &front {
        Some(front) if front.mode == "engine" => format!("pid {}", front.pid),
        Some(front) => format!(
            "{}: {}",
            front.mode,
            front.reason.as_deref().unwrap_or("no reason")
        ),
        None => "no front: see docker-front.log of the sandbox".into(),
    };
    check(
        "the adopted holder gets an engine front",
        adopted && front.as_ref().is_some_and(|f| f.mode == "engine"),
        detail,
    );
    std::fs::write(probe.join("go-engine"), "").map_err(|e| e.to_string())?;
    let done = wait_line(&out, "engine-done", PULL_WAIT).await.is_ok();
    let log = read(&out);
    for (name, word, file, detail) in [
        (
            "docker run --rm hello-world in the old run",
            "hello-ok",
            "hello.txt",
            last_line as fn(&str) -> String,
        ),
        (
            "docker build in the old run",
            "build-ok",
            "build.txt",
            last_line,
        ),
        (
            "docker compose up (two services) in the old run",
            "compose-ok",
            "compose.txt",
            compose_detail,
        ),
    ] {
        check(
            name,
            done && log.contains(word),
            detail(&read(&probe.join(file))),
        );
    }

    // 3. The front ends; the next upkeep recreates the socket; the old run keeps working.
    if let Some(pid) = front.map(|f| f.pid) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap_or(0)),
            nix::sys::signal::Signal::SIGKILL,
        );
        let deadline = Instant::now() + WAIT;
        while after.docker_front(&request.id).await.map(|f| f.pid) == Some(pid)
            && Instant::now() < deadline
        {
            after.tick().await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    std::fs::write(probe.join("go-again"), "").map_err(|e| e.to_string())?;
    let again = wait_line(&out, "again-done", WAIT).await.is_ok();
    let name = read(&probe.join("again.txt"));
    check(
        "a recreated socket still reaches the old run",
        again && read(&out).contains("again-ok") && name.trim() == HOSTNAME,
        first_line(&name),
    );

    // 4. A new run: DOCKER_HOST, the same engine.
    let joined = after
        .prepare(request)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no sandbox base")?;
    runs.push(spawn_run(exe, &joined.join, request, &probe, "new.sh")?);
    after.release(&request.id).await;
    let new = probe.join("new.txt");
    let _ = wait_line(&new, "done", WAIT).await;
    let text = read(&new);
    check(
        "a new run has DOCKER_HOST and the engine",
        text.contains("host=unix:///run/yulius/docker/docker.sock")
            && text.contains(&format!("name={HOSTNAME}")),
        text.lines().take(2).collect::<Vec<_>>().join(" "),
    );
    Ok(())
}

/// Starts `script` in a run joined to the sandbox of `join`, as the ptyhost starts one.
fn spawn_run(
    exe: &Path,
    join: &SandboxJoin,
    request: &SandboxRequest,
    probe: &Path,
    script: &str,
) -> Result<std::process::Child, String> {
    let file = probe.join(format!("{script}.join.json"));
    yulius_sandbox::holder::write_json(&file, join).map_err(|e| e.to_string())?;
    let root = request.dir.to_string_lossy().into_owned();
    let spec = BubblewrapSpec {
        sandbox_root: root.clone(),
        home: format!("{root}/home"),
        writable: Vec::new(),
        readonly: Vec::new(),
        share_net: false,
        binds: Vec::new(),
        identity: None,
        join: Some(Box::new(join.clone())),
    };
    let probe_text = probe.to_string_lossy().into_owned();
    let inner = [
        "/bin/sh".to_owned(),
        format!("{probe_text}/{script}"),
        probe_text,
    ];
    let argv = yulius_ptyhost::bwrap_argv(&spec, &inner, &root);
    std::process::Command::new(exe)
        .arg("sandbox-exec")
        .arg("--join")
        .arg(&file)
        .arg("--")
        .args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start a run: {e}"))
}

/// Longest detail printed.
const DETAIL_CHARS: usize = 200;

/// `text` as one line a terminal prints as it is (SPEC-202 RSB-003): escape sequences,
/// control characters and invisible or direction-changing marks removed, of each line rewritten
/// with carriage returns only its last text (as a terminal shows it), lines joined by spaces,
/// then cut to [`DETAIL_CHARS`].
fn printable(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.split('\r')
                .map(|segment| plain(segment).trim().to_owned())
                .rfind(|segment| !segment.is_empty())
                .unwrap_or_default()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(DETAIL_CHARS)
        .collect()
}

/// `segment` (no line break or carriage return) without escape sequences, control characters and
/// invisible or direction-changing marks; tabs become spaces.
fn plain(segment: &str) -> String {
    let mut out = String::new();
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters up to a final byte.
                Some('[') => skip_csi(&mut chars),
                // OSC, DCS, SOS, PM, APC: a string up to BEL or ST (ESC \\).
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next().is_some()) {
                            break;
                        }
                    }
                }
                // Character set selection: one more character.
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                    chars.next();
                }
                _ => {}
            },
            // 8-bit CSI.
            '\u{9b}' => skip_csi(&mut chars),
            '\t' => out.push(' '),
            '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Skips a CSI sequence's parameters and final byte.
fn skip_csi(chars: &mut impl Iterator<Item = char>) {
    for c in chars {
        if ('@'..='~').contains(&c) {
            break;
        }
    }
}

/// The compose check's detail (SPEC-202 RSB-004): the probe service's last line (`probe-1 …`),
/// which says whether it reached the web service; compose's own last line is the web
/// container's expected stop by `--abort-on-container-exit`. Without one, the last line.
fn compose_detail(text: &str) -> String {
    let lines: Vec<String> = text.lines().map(printable).collect();
    let probe = lines.iter().rev().find(|line| {
        line.split_whitespace()
            .next()
            .and_then(|word| word.strip_prefix("probe-"))
            .is_some_and(|replica| replica.chars().all(|c| c.is_ascii_digit()))
    });
    probe
        .or_else(|| lines.iter().rev().find(|line| !line.is_empty()))
        .cloned()
        .unwrap_or_default()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn last_line(text: &str) -> String {
    text.lines().last().unwrap_or_default().to_owned()
}

/// Waits (polling a file the run writes) until `path` has the line `line`.
async fn wait_line(path: &Path, line: &str, limit: Duration) -> Result<(), String> {
    let deadline = Instant::now() + limit;
    while !read(path).lines().any(|l| l == line) {
        if Instant::now() >= deadline {
            return Err(format!(
                "no `{line}` in {} within {limit:?}",
                path.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-196 DKL-007: started as root outside a delegated cgroup, the smoke runs itself as
    /// the data directory's owner in a delegated transient unit, and waits for it.
    #[test]
    fn root_runs_the_smoke_as_the_service_user_in_a_delegated_unit() {
        let argv = systemd_run_argv(
            Path::new("/usr/local/bin/yulius"),
            Path::new("/var/lib/yulius"),
            999,
        );
        let text: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(text[0], "systemd-run");
        for flag in [
            "--wait",
            "--pipe",
            "--uid=999",
            "Delegate=yes",
            "DelegateSubgroup=server",
        ] {
            assert!(text.iter().any(|a| a == flag), "{flag} in {text:?}");
        }
        let tail = text.iter().position(|a| a == "--").unwrap_or(0);
        assert_eq!(
            &text[tail + 1..],
            [
                "/usr/local/bin/yulius",
                "doctor",
                "--docker-smoke",
                "--data-dir",
                "/var/lib/yulius"
            ]
        );
    }

    /// SPEC-202 RSB-001 (#617): root runs the smoke as the data directory's owner even when its
    /// own cgroup (a root login slice, whose files root can always write) would pass as
    /// delegated, and never prepares that cgroup.
    #[test]
    fn root_always_runs_the_smoke_as_the_owner_without_touching_its_own_cgroup() {
        let looked = std::cell::Cell::new(false);
        let started = start(
            true,
            || Ok(999),
            || {
                looked.set(true);
                Ok("user.slice/user-0.slice")
            },
        );
        assert_eq!(started, Start::AsOwner(999));
        assert!(!looked.get(), "root prepared its own cgroup");
    }

    /// SPEC-202 RSB-001: a root-owned data directory is refused, not run again as root (which
    /// has no subordinate ids, and would start itself again forever).
    #[test]
    fn root_refuses_a_data_directory_owned_by_root() {
        let started = start(true, || Ok(0), || Ok(()));
        let Start::Refused(why) = started else {
            panic!("{started:?}");
        };
        assert!(why.contains("owned by root"), "{why}");
    }

    /// A user other than root runs the checks in its delegated cgroup, or is asked for root.
    #[test]
    fn a_service_user_runs_here_or_is_asked_for_root() {
        assert_eq!(start(false, || Ok(999), || Ok("unit")), Start::Here("unit"));
        let started = start::<()>(false, || Ok(999), || Err("not delegated".into()));
        let Start::Refused(why) = started else {
            panic!("{started:?}");
        };
        assert!(
            why.contains("not delegated") && why.contains("sudo"),
            "{why}"
        );
    }

    /// An interrupted smoke test's sandbox is found by name and dead process; a live one, this
    /// one and real sandboxes are left alone.
    #[test]
    fn only_sandboxes_of_finished_smoke_tests_are_stale() -> Result<(), Box<dyn std::error::Error>>
    {
        let data = tempfile::tempdir()?;
        let root = data.path().join("runtime/sandboxes");
        let mut child = std::process::Command::new("sleep").arg("30").spawn()?;
        let alive = format!("{SMOKE_PREFIX}{}", child.id());
        let mine = format!("{SMOKE_PREFIX}{}", std::process::id());
        // Pids are below 2^22 on Linux.
        let dead = format!("{SMOKE_PREFIX}{}", 1u32 << 23);
        for name in [
            &alive,
            &mine,
            &dead,
            &"s1".to_owned(),
            &format!("{SMOKE_PREFIX}x"),
        ] {
            std::fs::create_dir_all(root.join(name))?;
        }
        assert_eq!(stale_smokes(data.path()), vec![dead]);
        let _ = child.kill();
        let _ = child.wait();
        Ok(())
    }

    /// SPEC-201 DBX-002 (#610): the smoke builds and composes, so the host install it asks for
    /// brings buildx and compose to the base.
    #[test]
    fn the_host_install_it_asks_for_brings_buildx_and_compose() {
        assert!(
            NO_DOCKERD_IN_BASE
                .contains("docker-ce with docker-buildx-plugin and docker-compose-plugin"),
            "{NO_DOCKERD_IN_BASE}"
        );
        // Review round 1 (nit): one host install everywhere, as the refusal and doctor name it.
        assert!(
            !NO_DOCKERD_IN_BASE.contains("docker.io"),
            "{NO_DOCKERD_IN_BASE}"
        );
    }

    /// SPEC-202 RSB-003: a detail never moves the terminal's cursor: escapes and control
    /// characters go, and a line rewritten with carriage returns keeps its last text.
    #[test]
    fn details_are_one_printable_line() {
        assert_eq!(
            printable("\u{1b}[36mprobe-1  |\u{1b}[0m compose-web"),
            "probe-1  | compose-web"
        );
        assert_eq!(printable("Pulling 10%\rPulling 100%\r"), "Pulling 100%");
        assert_eq!(
            printable("\u{1b}]0;title\u{7}a\u{1b}[2K\u{1b}[1Ab\u{8}c\td\u{7f}"),
            "abc d"
        );
        assert_eq!(printable(&"x".repeat(500)).len(), 200);
    }

    /// SPEC-202 RSB-003, review round 1: carriage returns rewrite their own line only, every
    /// kind of escape goes whole, and a long rewritten line is cleaned before it is cut.
    #[test]
    fn details_survive_every_escape_and_line_ending() {
        // Windows line endings and a rewrite on the second line only.
        assert_eq!(printable("x\r\ny\r\n"), "x y");
        assert_eq!(printable("a\nb\rc"), "a c");
        // OSC ended by ESC \, escapes left open at the end.
        assert_eq!(printable("\u{1b}]0;t\u{1b}\\ok"), "ok");
        assert_eq!(printable("ok\u{1b}]0;unterminated"), "ok");
        assert_eq!(printable("ok\u{1b}[1"), "ok");
        // Charset selection (`tput sgr0`), device control strings, 8-bit CSI.
        assert_eq!(printable("\u{1b}(Bok\u{1b}[m"), "ok");
        assert_eq!(
            printable("a\u{1b}Pq#0;2;0;0;0\u{1b}\\b\u{1b}_apc\u{7}c"),
            "abc"
        );
        assert_eq!(printable("a\u{9b}2Kb"), "ab");
        // An ESC before a line break keeps the break.
        assert_eq!(printable("a\u{1b}\nb"), "a b");
        // Text direction overrides and invisible marks go.
        assert_eq!(printable("a\u{202e}b\u{200b}c\u{2066}d"), "abcd");
        // The hello and build details: the last line, cleaned before it is cut.
        let progress = "Pulling 1%\r".repeat(50) + "done";
        assert_eq!(printable(&last_line(&progress)), "done");
    }

    /// SPEC-202 RSB-004: the compose check shows the probe's result, not the web container's
    /// expected stop by `--abort-on-container-exit`, even when compose colours and rewrites
    /// its lines.
    #[test]
    fn the_compose_detail_is_the_probes_result() {
        let output = " Network probe_default  Creating\r Network probe_default  Created\n\
             \u{1b}[1A\u{1b}[2K Container probe-web-1  Started\n\
             Attaching to probe-1, web-1\n\
             \u{1b}[36mprobe-1  |\u{1b}[0m compose-web\n\
             \u{1b}[36mprobe-1 exited with code 0\u{1b}[0m\n\
             Aborting on container exit...\n \
             Container probe-probe-1  Stopped\n\
             \u{1b}[33mweb-1 exited with code 137\u{1b}[0m\n";
        assert_eq!(compose_detail(output), "probe-1 exited with code 0");
        // Without the probe (compose failed before it ran): the last line, printable.
        assert_eq!(
            compose_detail("no configuration file provided: not found\r\n"),
            "no configuration file provided: not found"
        );
    }

    /// The scripts end each phase with a line the smoke waits for.
    #[test]
    fn every_phase_ends_with_a_line_the_smoke_waits_for() {
        for line in [
            "log started",
            "log refused",
            "log engine-done",
            "log again-done",
        ] {
            assert!(OLD_RUN.contains(line), "{line}");
        }
        assert!(NEW_RUN.contains("echo done"));
    }
}
