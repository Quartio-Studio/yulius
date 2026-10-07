//! Sandbox shells end to end with the real detached ptyhost and `/bin/sh`: write access,
//! selected folder and sandbox HOME, the terminal protocol, the per-sandbox bound, exit, idle
//! reaping, shutdown and orphan cleanup, and isolation from task state (SPEC-001 UI-015,
//! AC-UI-7, HRN-002).

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::{
    Creds, Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox, seed_task, serve, ws_connect, ws_refusal_status,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius::wiring::session::PtySessionHost;
use yulius_app::ports::store::TaskStore;
use yulius_app::shells::{MAX_SHELLS_PER_SANDBOX, SHELL_IDLE_MS};
use yulius_contracts::terminal::{KIND_OUTPUT, KIND_SNAPSHOT, decode_binary};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::TaskStatus;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const ORIGIN: &str = "https://yulius.test";
const WAIT: Duration = Duration::from_secs(20);

struct Fixture {
    h: Harness,
    writer: Creds,
    reader: Creds,
    data: PathBuf,
}

/// alice (write) and bob (read) on `s1` (repository folder, task `t1` with a worktree) and
/// `s2` (task `t2`). Shells run `/bin/sh` through the real ptyhost.
async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with(false).await
}

/// [`fixture`], optionally with bubblewrap containment (Linux).
async fn fixture_with(contained: bool) -> Result<Fixture, Box<dyn std::error::Error>> {
    // A short base: shell sockets live at <data>/shells/<16 hex>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ysh").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| config.contained = contained)),
        ..HarnessOptions::default()
    })?;
    let data = h.app.config.data_dir.clone();
    assert!(h.app.set_shell_program(PathBuf::from("/bin/sh")));
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    for repo in ["acme/fixture", "acme/other"] {
        h.github.set_perm("alice", repo, perms("write"));
        h.github.set_perm("bob", repo, perms("read"));
    }
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;
    let worktree = data.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    let foreign = data.join("sandboxes/s2/worktrees/t2");
    std::fs::create_dir_all(&foreign)?;
    seed_task(
        &h,
        "t2",
        "s2",
        "u-alice",
        TaskStatus::Running,
        Some(&foreign),
    )
    .await?;
    Ok(Fixture {
        h,
        writer,
        reader,
        data,
    })
}

async fn create(
    f: &Fixture,
    creds: &Creds,
    sandbox: &str,
    body: Option<Value>,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/shells", sid = sandbox),
        body,
        creds,
    )
    .await
}

/// Reads frames until the accumulated terminal text contains `needle`; returns the text.
async fn read_until(
    socket: &mut Socket,
    text: &mut String,
    needle: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !text.contains(needle) {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| format!("timed out waiting for {needle:?}; screen so far: {text:?}"))?
            .ok_or("socket closed")??;
        if let Message::Binary(bytes) = message
            && let Some((kind, _, payload)) = decode_binary(&bytes)
            && (kind == KIND_OUTPUT || kind == KIND_SNAPSHOT)
        {
            text.push_str(&String::from_utf8_lossy(payload));
        }
    }
    Ok(())
}

/// Next JSON message, skipping binary frames.
async fn next_json(socket: &mut Socket) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "timed out waiting for a JSON message")?
            .ok_or("socket closed")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(text.as_str())?);
        }
    }
}

async fn input(socket: &mut Socket, data: &str) -> TestResult {
    socket
        .send(Message::text(
            json!({"type": "input", "data": data}).to_string(),
        ))
        .await?;
    Ok(())
}

async fn open_terminal(
    addr: std::net::SocketAddr,
    id: &str,
    creds: &Creds,
) -> Result<Socket, Box<dyn std::error::Error>> {
    let mut socket = ws_connect(addr, &format!("/api/shells/{id}/terminal"), creds, ORIGIN).await?;
    let state = next_json(&mut socket).await?;
    assert_eq!(state["type"], "state");
    assert_eq!(state["can_write"], true);
    assert_eq!(state["status"], "running");
    socket
        .send(Message::text(
            json!({"type": "hello", "last_seq": null, "cols": 100, "rows": 30}).to_string(),
        ))
        .await?;
    Ok(socket)
}

fn pid_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

async fn wait_for(what: &str, mut check: impl FnMut() -> bool) -> TestResult {
    let deadline = tokio::time::Instant::now() + WAIT;
    while tokio::time::Instant::now() < deadline {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

/// Host and child pids recorded by the ptyhost.
fn run_pids(run_dir: &Path) -> Result<(i32, i32), Box<dyn std::error::Error>> {
    let status: Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("status.json"))?)?;
    let host = status["ptyhost_pid"].as_i64().ok_or("ptyhost_pid")?;
    let child = status["child_pid"].as_i64().ok_or("child_pid")?;
    Ok((i32::try_from(host)?, i32::try_from(child)?))
}

#[tokio::test]
async fn shells_serve_write_users_in_the_selected_folder_without_touching_tasks() -> TestResult {
    let f = fixture().await?;
    let result = scenario(&f).await;
    f.h.app.stop_workspace_services().await;
    result
}

async fn scenario(f: &Fixture) -> TestResult {
    let served = serve(f.h.router.clone()).await?;
    let task_before = f.h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;

    // Read users cannot open a shell (AC-UI-7); foreign tasks are invisible.
    assert_eq!(
        create(f, &f.reader, "s1", None).await?.status,
        StatusCode::FORBIDDEN
    );
    let foreign = create(f, &f.writer, "s1", Some(json!({"task_id": "t2"}))).await?;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    let bad = create(f, &f.writer, "s1", Some(json!({"cwd": "/"}))).await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);

    // The UI posts without a body: a login shell in the sandbox repository, sandbox HOME.
    let created = create(f, &f.writer, "s1", None).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let repo_shell = created.body["id"].as_str().ok_or("id")?.to_owned();
    let mut socket = open_terminal(served.addr, &repo_shell, &f.writer).await?;
    let mut screen = String::new();
    input(
        &mut socket,
        "echo \"sum=$((6*7))\"; pwd; echo \"home=$HOME\"\r",
    )
    .await?;
    read_until(&mut socket, &mut screen, "sum=42").await?;
    read_until(
        &mut socket,
        &mut screen,
        &f.data.join("sandboxes/s1/repo").display().to_string(),
    )
    .await?;
    read_until(
        &mut socket,
        &mut screen,
        // The user's own persistent home, not the sandbox's (UI-016).
        &format!("home={}", f.data.join("users/u-alice/home").display()),
    )
    .await?;

    // Shells are private: read users and other writers cannot see them (UI-016).
    let refused = ws_connect(
        served.addr,
        &format!("/api/shells/{repo_shell}/terminal"),
        &f.reader,
        ORIGIN,
    )
    .await;
    assert_eq!(
        refused.as_ref().err().and_then(ws_refusal_status),
        Some(404)
    );
    let carol = seed_github_user(&f.h, "u-carol", "carol").await?;
    f.h.github.set_perm("carol", "acme/fixture", perms("write"));
    let other_writer = ws_connect(
        served.addr,
        &format!("/api/shells/{repo_shell}/terminal"),
        &carol,
        ORIGIN,
    )
    .await;
    assert_eq!(
        other_writer.as_ref().err().and_then(ws_refusal_status),
        Some(404),
        "another writer cannot attach"
    );
    let cross = ws_connect(
        served.addr,
        &format!("/api/shells/{repo_shell}/terminal"),
        &f.writer,
        "https://evil.test",
    )
    .await;
    assert_eq!(cross.as_ref().err().and_then(ws_refusal_status), Some(403));

    // A task worktree of the same sandbox can be selected.
    let created = create(f, &f.writer, "s1", Some(json!({"task_id": "t1"}))).await?;
    assert_eq!(created.status, StatusCode::CREATED);
    let worktree_shell = created.body["id"].as_str().ok_or("id")?.to_owned();
    let mut worktree_socket = open_terminal(served.addr, &worktree_shell, &f.writer).await?;
    let mut worktree_screen = String::new();
    input(&mut worktree_socket, "pwd\r").await?;
    read_until(
        &mut worktree_socket,
        &mut worktree_screen,
        &f.data
            .join("sandboxes/s1/worktrees/t1")
            .display()
            .to_string(),
    )
    .await?;
    drop(worktree_socket);

    // At most four live shells per sandbox.
    let mut extra = Vec::new();
    for _ in 2..MAX_SHELLS_PER_SANDBOX {
        let created = create(f, &f.writer, "s1", None).await?;
        assert_eq!(created.status, StatusCode::CREATED);
        extra.push(created.body["id"].as_str().ok_or("id")?.to_owned());
    }
    let over = create(f, &f.writer, "s1", None).await?;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        create(f, &f.writer, "s2", None).await?.status,
        StatusCode::CONFLICT,
        "s2 has no repository"
    );

    // Leaving the shell ends it: exit frame, close, then the shell is gone.
    let sid = SandboxId::parse("s1")?;
    let runtime =
        f.h.app
            .sandbox_shells(&sid)
            .into_iter()
            .find(|shell| shell.id() == repo_shell)
            .ok_or("shell runtime")?;
    let run_dir = f.data.join("shells").join(&repo_shell);
    let (host_pid, child_pid) = run_pids(&run_dir)?;
    input(&mut socket, "exit 3\r").await?;
    let exit = loop {
        let message = next_json(&mut socket).await?;
        if message["type"] == "exit" {
            break message;
        }
    };
    assert_eq!(exit, json!({"type": "exit", "status": "closed", "code": 3}));
    assert!(matches!(
        tokio::time::timeout(WAIT, socket.next()).await?,
        Some(Ok(Message::Close(_))) | None
    ));
    wait_for("the shell to end", || runtime.has_ended()).await?;
    wait_for("the host and shell to exit", || {
        !pid_alive(child_pid) && !pid_alive(host_pid)
    })
    .await?;
    assert!(!run_dir.exists(), "run directory removed");
    let gone = ws_connect(
        served.addr,
        &format!("/api/shells/{repo_shell}/terminal"),
        &f.writer,
        ORIGIN,
    )
    .await;
    assert_eq!(gone.as_ref().err().and_then(ws_refusal_status), Some(404));
    assert_eq!(
        f.h.app.sandbox_shells(&sid).len(),
        MAX_SHELLS_PER_SANDBOX - 1
    );

    // Shells without viewers for 30 minutes are ended (injected clock).
    wait_for("viewers to leave", || {
        f.h.app
            .sandbox_shells(&sid)
            .iter()
            .all(|shell| shell.viewer_count() == 0)
    })
    .await?;
    f.h.clock
        .0
        .fetch_add(SHELL_IDLE_MS - 1_000, Ordering::SeqCst);
    assert_eq!(f.h.app.reap_idle_shells().await, 0, "not idle long enough");
    f.h.clock.0.fetch_add(2_000, Ordering::SeqCst);
    assert_eq!(f.h.app.reap_idle_shells().await, MAX_SHELLS_PER_SANDBOX - 1);
    assert!(f.h.app.sandbox_shells(&sid).is_empty());

    // Shells never touch task state (HRN-002).
    let task_after = f.h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;
    assert_eq!(task_after, task_before);
    assert!(f.h.store.attempts(&TaskId::parse("t1")?).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn server_shutdown_ends_shells_and_restart_removes_orphans() -> TestResult {
    let f = fixture().await?;
    let created = create(&f, &f.writer, "s1", None).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let run_dir = f.data.join("shells").join(&id);
    let (host_pid, child_pid) = run_pids(&run_dir)?;
    assert!(pid_alive(child_pid));

    f.h.app.stop_workspace_services().await;
    wait_for("the shell processes to exit", || {
        !pid_alive(child_pid) && !pid_alive(host_pid)
    })
    .await?;
    assert!(f.h.app.sandbox_shells(&SandboxId::parse("s1")?).is_empty());
    assert!(!run_dir.exists());

    // Directories left by a crashed server are cleaned at the next start.
    let orphan = f.data.join("shells").join("0123456789abcdef");
    std::fs::create_dir_all(&orphan)?;
    assert_eq!(f.h.app.reap_orphan_shells(), 1);
    assert!(!orphan.exists());
    Ok(())
}

/// A contained shell runs as the user's sandbox identity: their login, root inside the
/// sandbox, `sudo` works, and files written to `~` land in their own persistent home
/// (UI-016, AC-UI-8).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn contained_shells_run_as_the_user_with_root_in_their_own_home() -> TestResult {
    let f = fixture_with(true).await?;
    let result = async {
        let served = serve(f.h.router.clone()).await?;
        let created = create(&f, &f.writer, "s1", None).await?;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        let id = created.body["id"].as_str().ok_or("id")?.to_owned();
        let mut socket = open_terminal(served.addr, &id, &f.writer).await?;
        let mut screen = String::new();
        input(
            &mut socket,
            "echo \"who=$(whoami) uid=$(id -u) sudo=$(sudo id -u) home=$HOME host=$(hostname)\"; echo kept > ~/mine.txt; echo done-$((1+1))\r",
        )
        .await?;
        read_until(&mut socket, &mut screen, "done-2").await?;
        assert!(
            screen.contains("who=alice uid=0 sudo=0 home=/home/alice host=fixture"),
            "{screen}"
        );
        assert_eq!(
            std::fs::read_to_string(f.data.join("users/u-alice/home/mine.txt"))?,
            "kept\n"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    f.h.app.stop_workspace_services().await;
    result
}
