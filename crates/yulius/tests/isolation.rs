//! Sandbox isolation end to end (SPEC-002 ADR-22, SPEC-001 PRJ-011, NFR-007; F 2026-10-10)
//! with the real server, the real ptyhost, `bwrap`, `newuidmap`, `slirp4netns` and a real base
//! copied from the host. Users' shells and agents' task sessions (the fake agent's `/sh`) run
//! inside the namespace holder.
//!
//! Opt-in, Linux only, as the service user with its `/etc/subuid` range:
//!
//! ```text
//! YULIUS_ISOLATION_TEST=1 [YULIUS_ISOLATION_TEST_SYSTEM=/path/to/a/data/system] \
//!   [YULIUS_TEST_TMP=/tmp/yqa-iso-home] <test binary> isolation:: --test-threads 1
//! ```
//!
//! `YULIUS_ISOLATION_TEST_SYSTEM` reuses a built `system/` directory (a base build copies about
//! 6 GB). The limits test needs a delegated cgroup: run it in a transient unit,
//! `systemd-run --uid=<user> -p Delegate=yes -p DelegateSubgroup=server --wait --pipe … isolation_limits`
//! with `YULIUS_ISOLATION_TEST_LIMITS=1`.

#![cfg(target_os = "linux")]

use crate::support;

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{
    Creds, Harness, HarnessOptions, Served, TestResult, call, harness_with, perms,
    seed_github_user, seed_sandbox, serve, ws_connect,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius::wiring::git::GitAdapter;
use yulius::wiring::sandbox::{Egress, IsolationArgs, NamespaceIsolation, resolve};
use yulius::wiring::session::PtySessionHost;
use yulius_app::model::{AccountRecord, AccountStatus, SandboxRecord};
use yulius_contracts::terminal::{KIND_OUTPUT, KIND_SNAPSHOT, decode_binary};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_sandbox::cgroup::{Delegation, Limits};
use yulius_sandbox::manager::Manager;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const ORIGIN: &str = "https://yulius.test";
/// apt over the network can be slow.
const APT_WAIT: Duration = Duration::from_secs(600);
const WAIT: Duration = Duration::from_secs(60);

/// Whether this run should exercise isolation; `Err` is the reason to skip.
fn enabled(test: &str) -> Result<(), String> {
    if std::env::var("YULIUS_ISOLATION_TEST").as_deref() != Ok("1") {
        return Err(format!(
            "SKIPPED {test}: set YULIUS_ISOLATION_TEST=1 (Linux, subordinate ids, network)"
        ));
    }
    Ok(())
}

struct Iso {
    h: Harness,
    alice: Creds,
    data: PathBuf,
    manager: Arc<Manager>,
    served: Served,
    s1: SandboxRecord,
    s2: SandboxRecord,
}

fn git(dir: &Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned().into())
    }
}

fn clone_repo(data: &Path, sandbox: &str) -> TestResult {
    let origin = data.join(format!("origin-{sandbox}.git"));
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join(format!("seed-{sandbox}"));
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), format!("# {sandbox}\n"))?;
    git(&seed, &["add", "README.md"])?;
    git(&seed, &["commit", "-q", "-m", "init"])?;
    git(
        &seed,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&seed, &["push", "-q", "origin", "main"])?;
    let repo = data.join("sandboxes").join(sandbox).join("repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        data,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )
}

fn ensure_base(data: &Path, exe: &Path) -> TestResult {
    if let Ok(system) = std::env::var("YULIUS_ISOLATION_TEST_SYSTEM") {
        let system = PathBuf::from(system);
        if yulius_sandbox::base::Bases::new(system.parent().ok_or("system parent")?)
            .current()
            .is_some()
        {
            std::os::unix::fs::symlink(&system, data.join("system"))?;
            return Ok(());
        }
        // Build it there once for later runs.
        let parent = system.parent().ok_or("system parent")?;
        build_base(parent, exe)?;
        std::os::unix::fs::symlink(&system, data.join("system"))?;
        return Ok(());
    }
    build_base(data, exe)
}

fn build_base(data: &Path, exe: &Path) -> TestResult {
    let started = Instant::now();
    let out = std::process::Command::new(exe)
        .args(["sandbox-base", "build", "--data-dir"])
        .arg(data)
        .output()?;
    eprintln!(
        "base build: {} s: {}{}",
        started.elapsed().as_secs(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(())
    } else {
        Err("the base build failed".into())
    }
}

/// A server with isolation over a real base, users alice (write) on `s1` ("fixture") and `s2`
/// ("other"), a Claude account, and host loopback listeners: `forward` (relayed into every
/// sandbox) and `blocked` (not relayed).
async fn fixture(
    limits: Limits,
    delegation: Option<Delegation>,
    forward: u16,
) -> Result<Iso, Box<dyn std::error::Error>> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let fake_dir = fake.parent().ok_or("fake-agent dir")?.to_path_buf();
    let tmp = std::env::var("YULIUS_TEST_TMP").unwrap_or_else(|_| "/tmp".into());
    let dir = tempfile::Builder::new().prefix("yiso").tempdir_in(tmp)?;
    let yulius_exe = exe.clone();
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host: Some(Arc::new(PtySessionHost::new(exe.clone()))),
        harness: Some(Arc::new(FakeAgentHarness::new(
            exe.clone(),
            fake,
            FakeAgentOptions::default(),
        ))),
        dir: Some(dir),
        configure: Some(Box::new(move |c| {
            c.contained = true;
            c.contained_readonly = vec![fake_dir];
            c.yulius_exe = yulius_exe;
        })),
        ..HarnessOptions::default()
    })?;
    assert!(h.app.set_shell_program(PathBuf::from("/bin/bash")));
    let data = h.app.config.data_dir.clone();
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let s1 = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    let s2 = seed_sandbox(&h, "s2", "other", "u-alice").await?;
    for repo in ["acme/fixture", "acme/other"] {
        h.github.set_perm("alice", repo, perms("write"));
    }
    clone_repo(&data, "s1")?;
    clone_repo(&data, "s2")?;
    h.app
        .deps
        .store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a1")?,
            provider: Provider::Anthropic,
            label: "Claude".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-isolation-test")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    // Decoys of the host's secrets next to the sandboxes.
    std::fs::write(data.join("master.key"), [7u8; 32])?;
    std::fs::write(data.join("jev.key"), "decoy")?;
    std::fs::create_dir_all(data.join("users/u-bob/home"))?;
    std::fs::write(data.join("users/u-bob/home/private.txt"), "bob's")?;
    ensure_base(&data, &exe)?;
    let args = IsolationArgs {
        egress: Egress::Direct,
        ..IsolationArgs::default()
    };
    let mut config = resolve(&args, &data, &exe, forward).map_err(|m| m.join("; "))?;
    config.auto_build = false;
    config.delegation = delegation;
    config.limits = limits;
    config.idle.stop_after = Duration::from_secs(2);
    config.forwards = BTreeMap::from([(
        forward,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), forward),
    )]);
    let manager = Manager::new(config);
    assert!(h.app.set_isolation(Arc::new(NamespaceIsolation::new(
        Arc::clone(&manager),
        true
    ))));
    let served = serve(h.router.clone()).await?;
    Ok(Iso {
        h,
        alice,
        data,
        manager,
        served,
        s1,
        s2,
    })
}

async fn shell(iso: &Iso, sandbox: &str) -> Result<Socket, Box<dyn std::error::Error>> {
    let created = call(
        &iso.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/shells"),
        None,
        &iso.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = created.body["id"].as_str().ok_or("shell id")?.to_owned();
    let mut socket = ws_connect(
        iso.served.addr,
        &format!("/api/shells/{id}/terminal?cols=200&rows=50"),
        &iso.alice,
        ORIGIN,
    )
    .await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "no terminal state")?
            .ok_or("socket closed")??;
        if let Message::Text(text) = message {
            let state: Value = serde_json::from_str(text.as_str())?;
            if state["type"] == "state" {
                break;
            }
        }
    }
    socket
        .send(Message::text(
            json!({"type": "hello", "last_seq": null, "cols": 200, "rows": 50}).to_string(),
        ))
        .await?;
    Ok(socket)
}

/// Types `command` followed by an end marker and waits (within `wait`) for the marker; returns
/// the terminal text received meanwhile (carriage returns removed). The marker is computed by
/// the shell, so the echo of the typed line never matches it.
async fn run(
    socket: &mut Socket,
    command: &str,
    wait: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    socket
        .send(Message::text(
            json!({"type": "input", "data": format!("{command}; echo YQA-END-$((6*7))\r")})
                .to_string(),
        ))
        .await?;
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + wait;
    while !text.contains("YQA-END-42") {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| format!("timed out running {command:?}; screen: {text}"))?
            .ok_or("socket closed")??;
        if let Message::Binary(bytes) = message
            && let Some((kind, _, payload)) = decode_binary(&bytes)
            && (kind == KIND_OUTPUT || kind == KIND_SNAPSHOT)
        {
            text.push_str(&String::from_utf8_lossy(payload).replace('\r', ""));
        }
    }
    // Only what the command printed: everything after the echo of the typed line (which ends
    // with the unexpanded marker), so expectations never match the command's own text.
    let output = text
        .rfind("YQA-END-$((6*7))")
        .and_then(|at| {
            text[at..]
                .find('\n')
                .map(|nl| text[at + nl + 1..].to_owned())
        })
        .unwrap_or(text);
    Ok(strip_escapes(&output))
}

/// Terminal text without CSI (`ESC [ … letter`) and OSC (`ESC ] … BEL`/`ESC \\`) sequences.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

async fn agent(iso: &Iso, sandbox: &str) -> Result<TaskId, Box<dyn std::error::Error>> {
    let created = call(
        &iso.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        Some(json!({"prompt": "isolation probe", "harness": "claude"})),
        &iso.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = TaskId::parse(created.body["id"].as_str().ok_or("task id")?)?;
    wait_screen(iso, &id, "FAKE-AGENT READY", WAIT).await?;
    Ok(id)
}

async fn wait_screen(
    iso: &Iso,
    id: &TaskId,
    needle: &str,
    wait: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + wait;
    loop {
        let screen = iso.h.app.runtime(id).hub.screen_text();
        if screen.contains(needle) {
            return Ok(screen);
        }
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {needle:?}; screen:\n{screen}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Sends `/sh <command>` to the agent and returns the screen once `marker` shows.
async fn agent_sh(
    iso: &Iso,
    id: &TaskId,
    command: &str,
    marker: &str,
    wait: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    iso.h
        .app
        .send_input(id, format!("/sh {command}\r").as_bytes())
        .await?;
    wait_screen(iso, id, marker, wait).await
}

/// A loopback listener answering "hello" to every connection.
async fn hello_server() -> Result<(u16, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    Ok((
        port,
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(b"hello\n").await;
            }
        }),
    ))
}

fn connect_probe(port: u16, label: &str) -> String {
    format!(
        "python3 -c \"import socket;socket.create_connection(('127.0.0.1',{port}),3);print('{label}-open')\" 2>/dev/null || echo {label}-closed"
    )
}

/// One fixture, every property in turn: the base build is the expensive part.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolation_end_to_end() -> TestResult {
    if let Err(why) = enabled("isolation_end_to_end") {
        eprintln!("{why}");
        return Ok(());
    }
    assert!(
        !Path::new("/usr/games/cowsay").exists() && !Path::new("/usr/games/sl").exists(),
        "the host must not have the test packages"
    );
    let (forward, _hello) = hello_server().await?;
    let (blocked, _other) = hello_server().await?;
    let iso = fixture(Limits::default(), None, forward).await?;
    guarded(&iso, end_to_end(&iso, forward, blocked)).await
}

/// Runs `body`, then always ends the shells, stops the holders and removes the system layers
/// (they hold files owned by mapped ids, which the temporary directory cannot remove), also
/// when `body` panics.
async fn guarded(iso: &Iso, body: impl Future<Output = TestResult>) -> TestResult {
    use futures::FutureExt as _;
    let outcome = std::panic::AssertUnwindSafe(body).catch_unwind().await;
    iso.h.app.end_all_shells().await;
    iso.manager.stop_all().await;
    let config = iso.manager.config();
    let mapper = yulius_sandbox::userns::Mapper {
        bwrap: config.bwrap.clone(),
        newuidmap: config.newuidmap.clone(),
        newgidmap: config.newgidmap.clone(),
        uid: config.uid,
        gid: config.gid,
    };
    if let Err(error) =
        yulius_sandbox::base::remove_tree(&mapper, &config.exe, &iso.data.join("sysroots"))
    {
        eprintln!("cleanup: {error}");
    }
    match outcome {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[allow(clippy::too_many_lines)]
async fn end_to_end(iso: &Iso, forward: u16, blocked: u16) -> TestResult {
    let data = iso.data.display().to_string();
    let started = Instant::now();
    let mut sh = shell(iso, "s1").await?;
    eprintln!(
        "first shell (holder start) in {} ms",
        started.elapsed().as_millis()
    );

    // AC-UI-8 inside the holder: the user's login, root, sudo, host name, home.
    let out = run(
        &mut sh,
        "echo \"who=$(whoami) uid=$(id -u) sudo=$(sudo id -u) host=$(hostname) home=$HOME\"",
        WAIT,
    )
    .await?;
    assert!(
        out.contains("who=alice uid=0 sudo=0 host=fixture home=/home/alice"),
        "{out}"
    );

    // apt from a user's shell, persisting in the sandbox's system layer.
    let started = Instant::now();
    let out = run(
        &mut sh,
        "sudo apt-get update -qq >/tmp/upd.log 2>&1; echo upd=$?; tail -4 /tmp/upd.log; sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q cowsay >/tmp/apt.log 2>&1; echo apt=$?; tail -3 /tmp/apt.log; /usr/games/cowsay moo | head -2; echo cowsay-done",
        APT_WAIT,
    )
    .await?;
    eprintln!("apt in {} s:\n{out}", started.elapsed().as_secs());
    // `apt-get update` may exit 100 because of a broken third-party source the host itself has
    // (the base copies the host's sources); the install is what must work.
    assert!(out.contains("upd=") && out.contains("apt=0"), "{out}");
    assert!(out.contains("< moo >"), "{out}");
    // Explicit forward in, host loopback and the gateway out; abstract sockets; X displays.
    let abstract_name = format!("yqa-iso-{}", std::process::id());
    let abstract_socket = std::os::unix::net::UnixListener::bind_addr(
        &<std::os::unix::net::SocketAddr as std::os::linux::net::SocketAddrExt>::from_abstract_name(
            abstract_name.as_bytes(),
        )?,
    )?;
    let probes = [
        connect_probe(forward, "forward"),
        connect_probe(blocked, "blocked"),
        format!(
            "python3 -c \"import socket;socket.create_connection(('10.0.2.2',{blocked}),3);print('gateway-open')\" 2>/dev/null || echo gateway-closed"
        ),
        format!(
            "python3 -c \"import socket;s=socket.socket(socket.AF_UNIX);s.connect('\\0{abstract_name}');print('abstract-open')\" 2>/dev/null || echo abstract-closed"
        ),
        "test -e /tmp/.X11-unix && echo x11-visible || echo x11-hidden".to_owned(),
    ];
    let out = run(
        &mut sh,
        &format!("{}; echo net-done", probes.join("; ")),
        WAIT,
    )
    .await?;
    drop(abstract_socket);
    eprintln!("network and display probes:\n{}", out.trim());
    for expected in [
        "forward-open",
        "blocked-closed",
        "gateway-closed",
        "abstract-closed",
        "x11-hidden",
    ] {
        assert!(out.contains(expected), "{expected}: {out}");
    }

    // Host secrets, other sandboxes and users are not in the mount namespace.
    let out = run(
        &mut sh,
        &format!(
            "for p in {data}/master.key {data}/jev.key {data}/sandboxes/s2 {data}/users/u-bob {data}/runtime {data}/system {data}/sysroots; do test -e $p && echo \"seen $p\" || echo \"hidden $p\"; done; echo files-done"
        ),
        WAIT,
    )
    .await?;
    assert!(!out.lines().any(|l| l.starts_with("seen ")), "{out}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("hidden ")).count(),
        7,
        "{out}"
    );

    // The syscall filter and the capability set.
    let out = run(
        &mut sh,
        "python3 -c 'import os; os.unshare(os.CLONE_NEWUSER)' 2>/dev/null && echo userns-allowed || echo userns-refused; sudo mount -t tmpfs none /mnt 2>/dev/null && echo mount-allowed || echo mount-refused; unshare -U true 2>/dev/null && echo unshare-allowed || echo unshare-refused; echo filter-done",
        WAIT,
    )
    .await?;
    eprintln!("filter probes:\n{}", out.trim());
    for expected in ["userns-refused", "mount-refused", "unshare-refused"] {
        assert!(out.contains(expected), "{expected}: {out}");
    }

    // Dev servers inside are reachable from the host's loopback (IPv4 and IPv6-only).
    let (p4, p6) = (iso.s1.port_base, iso.s1.port_base + 1);
    let _ = run(
        &mut sh,
        &format!(
            "(cd /tmp && python3 -m http.server {p4} --bind 127.0.0.1 >/dev/null 2>&1 &) ; (cd /tmp && python3 -m http.server {p6} --bind ::1 >/dev/null 2>&1 &) ; echo servers-started"
        ),
        WAIT,
    )
    .await?;
    for port in [p4, p6] {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let reply = async {
                let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
                stream.write_all(b"GET / HTTP/1.0\r\n\r\n").await?;
                let mut text = String::new();
                stream.read_to_string(&mut text).await?;
                Ok::<_, std::io::Error>(text)
            }
            .await;
            if reply.as_ref().is_ok_and(|t| t.starts_with("HTTP/1.0 200")) {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("port {port} not relayed: {reply:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    // Another sandbox: no cowsay, no access to s1's dev server.
    let mut other = shell(iso, "s2").await?;
    let out = run(
        &mut other,
        &format!(
            "test -e /usr/games/cowsay && echo cowsay-in-other || echo cowsay-not-in-other; {}; echo other-done",
            connect_probe(p4, "s1-server")
        ),
        WAIT,
    )
    .await?;
    assert!(out.contains("cowsay-not-in-other"), "{out}");
    assert!(out.contains("s1-server-closed"), "{out}");
    assert!(!Path::new("/usr/games/cowsay").exists(), "not on the host");
    eprintln!("other sandbox:\n{}", out.trim());

    // An agent's session in s1: same system layer, root, IS_SANDBOX, its own apt install.
    let task = agent(iso, "s1").await?;
    let screen = agent_sh(
        iso,
        &task,
        "test -x /usr/games/cowsay && echo agent-sees-cowsay; echo agent-uid=$(id -u) sandbox=$IS_SANDBOX; apt-get install -y -q sl >/dev/null 2>&1; echo agent-apt=$?; useradd --system yqa-svc && getent passwd yqa-svc >/dev/null && echo agent-useradd=ok",
        "SH EXIT",
        APT_WAIT,
    )
    .await?;
    eprintln!("agent session:\n{}", screen.trim());
    for expected in [
        "agent-sees-cowsay",
        "agent-uid=0 sandbox=1",
        "agent-apt=0",
        "agent-useradd=ok",
    ] {
        assert!(screen.contains(expected), "{expected}: {screen}");
    }
    // Attachments and the worktree are at their host paths; host secrets are not.
    let attachment = iso
        .data
        .join("sandboxes/s1/attachments/tasks")
        .join(task.as_str());
    std::fs::create_dir_all(&attachment)?;
    std::fs::write(attachment.join("brief.pdf"), b"%PDF-1.4 probe\n")?;
    for (path, expected) in [
        (attachment.join("brief.pdf"), "READ brief.pdf OK 15"),
        (iso.data.join("master.key"), "READ master.key ERR"),
        (
            iso.data.join("sandboxes/s2/repo/README.md"),
            "READ README.md ERR",
        ),
    ] {
        iso.h
            .app
            .send_input(&task, format!("/read {}\r", path.display()).as_bytes())
            .await?;
        wait_screen(iso, &task, expected, WAIT).await?;
    }
    iso.h.app.send_input(&task, b"/exit\r").await?;

    // The holder restarts (idle stop or crash): the system layer persists.
    iso.h.app.end_all_shells().await;
    let deadline = Instant::now() + Duration::from_secs(30);
    let state = iso.data.join("runtime/sandboxes/s1/host.json");
    while state.exists() {
        tokio::time::sleep(Duration::from_millis(500)).await;
        iso.manager.tick().await;
        if Instant::now() > deadline {
            return Err("the idle holder was not stopped".into());
        }
    }
    let mut again = shell(iso, "s1").await?;
    let out = run(
        &mut again,
        "test -x /usr/games/cowsay && test -x /usr/games/sl && echo still-installed; echo again-done",
        WAIT,
    )
    .await?;
    assert!(out.contains("still-installed"), "{out}");
    let _ = iso.s2.id.as_str();
    Ok(())
}

/// Per-sandbox limits (needs a delegated cgroup; see the module docs).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolation_limits() -> TestResult {
    if let Err(why) = enabled("isolation_limits") {
        eprintln!("{why}");
        return Ok(());
    }
    let delegation = match yulius::wiring::sandbox::delegation() {
        Ok(delegation) => delegation,
        Err(why) if std::env::var("YULIUS_ISOLATION_TEST_LIMITS").as_deref() != Ok("1") => {
            eprintln!("SKIPPED isolation_limits: {why}");
            return Ok(());
        }
        Err(why) => return Err(why.into()),
    };
    let limits = Limits {
        memory_max: Some(256 << 20),
        cpus: Some(1),
        pids_max: Some(64),
    };
    let (forward, _hello) = hello_server().await?;
    let iso = fixture(limits, Some(delegation.clone()), forward).await?;
    // SPEC-053 CPU-202: a process left in the sandbox group by an older server.
    let group = delegation.unit.join("sandboxes/s1");
    std::fs::create_dir_all(&group)?;
    let mut stray = std::process::Command::new("sleep").arg("300").spawn()?;
    std::fs::write(group.join("cgroup.procs"), stray.id().to_string())?;
    let body = async {
        let mut sh = shell(&iso, "s1").await?;
        let stray_group = std::fs::read_to_string(format!("/proc/{}/cgroup", stray.id()))?;
        assert_eq!(
            stray_group.trim().rsplit('/').next(),
            Some("agents"),
            "a stray moves to agents/: {stray_group}"
        );
        assert_eq!(
            std::fs::read_to_string(group.join("pids.max"))?.trim(),
            "64"
        );
        assert_eq!(
            std::fs::read_to_string(group.join("memory.max"))?.trim(),
            (256u64 << 20).to_string()
        );
        assert_eq!(
            std::fs::read_to_string(group.join("cpu.max"))?.trim(),
            "100000 100000"
        );
        // SPEC-053: the CPU goes to the leaves by weight, and the shell runs in `agents`.
        if delegation.controllers.iter().any(|c| c == "cpu") {
            assert!(std::fs::read_to_string(group.join("cgroup.subtree_control"))?.contains("cpu"));
            for (leaf, weight) in [("system", "100"), ("agents", "100"), ("ci", "400")] {
                assert_eq!(
                    std::fs::read_to_string(group.join(leaf).join("cpu.weight"))?.trim(),
                    weight,
                    "{leaf}"
                );
            }
            assert!(
                !std::fs::read_to_string(group.join("agents/cgroup.procs"))?
                    .trim()
                    .is_empty(),
                "the shell runs in agents/"
            );
            assert!(
                !std::fs::read_to_string(group.join("system/cgroup.procs"))?
                    .trim()
                    .is_empty(),
                "the holder runs in system/"
            );
        }
        // Children block on a pipe until the parent stops forking (EAGAIN at pids.max).
        let forker = "import os\nr, w = os.pipe()\nn = 0\ntry:\n    while n < 500:\n        if os.fork() == 0:\n            os.close(w); os.read(r, 1); os._exit(0)\n        n += 1\nexcept OSError as e:\n    print('fork-refused-after', n, e.errno)\nos.close(w)\nfor _ in range(n):\n    os.wait()\n";
        std::fs::write(iso.data.join("sandboxes/s1/forker.py"), forker)?;
        let out = run(
            &mut sh,
            &format!(
                "cat /proc/self/cgroup; python3 {}/sandboxes/s1/forker.py",
                iso.data.display()
            ),
            WAIT,
        )
        .await?;
        assert!(
            out.contains("0::/"),
            "the run sees its own cgroup root: {out}"
        );
        let refused_after = out
            .split("fork-refused-after ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|n| n.parse::<u32>().ok())
            .ok_or_else(|| format!("forks were not limited: {out}"))?;
        assert!(refused_after < 64, "at most pids.max tasks: {out}");
        eprintln!("pids.max 64: {}", out.trim());
        let out = run(
            &mut sh,
            "python3 -c 'b = bytearray(512 << 20); print(\"allocated\")'; echo mem-exit=$?",
            WAIT,
        )
        .await?;
        assert!(!out.lines().any(|l| l == "allocated"), "{out}");
        assert!(
            out.contains("mem-exit=137"),
            "killed by the OOM killer: {out}"
        );
        let events = std::fs::read_to_string(group.join("memory.events"))?;
        let kills = events
            .lines()
            .find_map(|l| l.strip_prefix("oom_kill "))
            .and_then(|n| n.trim().parse::<u64>().ok())
            .unwrap_or(0);
        assert!(kills >= 1, "{events}");
        eprintln!("memory.max 256 MiB: {} oom_kill={kills}", out.trim());
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let result = guarded(&iso, body).await;
    let _ = stray.kill();
    let _ = stray.wait();
    result
}

/// A sandbox over its disk quota starts no new runs; the message names the figures.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolation_disk_quota_refuses_new_runs() -> TestResult {
    if let Err(why) = enabled("isolation_disk_quota_refuses_new_runs") {
        eprintln!("{why}");
        return Ok(());
    }
    let (forward, _hello) = hello_server().await?;
    let iso = fixture(Limits::default(), None, forward).await?;
    iso.manager.set_usage("s1", 200_000_000_000).await;
    let created = call(
        &iso.h.router,
        Method::POST,
        "/api/sandboxes/s1/shells",
        None,
        &iso.alice,
    )
    .await?;
    iso.manager.stop_all().await;
    assert_eq!(created.status, StatusCode::CONFLICT, "{:?}", created.body);
    let text = created.body.to_string();
    assert!(
        text.contains("200.0 GB of its 100.0 GB disk quota"),
        "{text}"
    );
    Ok(())
}
