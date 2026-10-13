//! Sandbox shells end to end with the real detached ptyhost and `/bin/sh`: write access,
//! selected folder and the user's home, the terminal protocol, many terminals per user and
//! sandbox with names, privacy between users and between sandboxes (list, attach, rename,
//! close), the viewer's size before the first screen, exit, closing, idle reaping, and shells
//! that keep running across a server restart and are adopted again (SPEC-001 UI-015, UI-016,
//! AC-UI-7, AC-UI-8, HRN-002; F 2026-10-09).

use crate::support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::gated::GatedHost;
use support::{
    Creds, Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox, seed_task, serve, ws_connect, ws_refusal_status,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius_app::ports::session::SessionHost;
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
    /// The session host: holds the next start between its spawn and its attach when armed.
    gate: Arc<GatedHost>,
    writer: Creds,
    reader: Creds,
    data: PathBuf,
}

/// alice (write) and bob (read) on `s1` (repository folder, task `t1` with a worktree) and
/// `s2` (task `t2`). Shells run `/bin/sh` through the real ptyhost.
async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with(false, None).await
}

/// [`fixture`], optionally with bubblewrap containment (Linux), optionally on the data directory
/// of an earlier fixture (a server restart).
async fn fixture_with(
    contained: bool,
    data_dir: Option<PathBuf>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    // A short base: shell sockets live at <data>/shells/<16 hex>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ysh").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let gate = Arc::new(GatedHost::new(exe));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::clone(&gate) as Arc<dyn SessionHost>),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.contained = contained;
            if let Some(data) = data_dir {
                config.data_dir = data;
            }
        })),
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
        gate,
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
    // The viewer's size travels with the connection: the first state already has it.
    let mut socket = ws_connect(
        addr,
        &format!("/api/shells/{id}/terminal?cols=100&rows=30"),
        creds,
        ORIGIN,
    )
    .await?;
    let state = next_json(&mut socket).await?;
    assert_eq!(state["type"], "state");
    assert_eq!(state["can_write"], true);
    assert_eq!(state["status"], "running");
    assert_eq!(
        (state["cols"].as_u64(), state["rows"].as_u64()),
        (Some(100), Some(30))
    );
    socket
        .send(Message::text(
            json!({"type": "hello", "last_seq": null, "cols": 100, "rows": 30}).to_string(),
        ))
        .await?;
    Ok(socket)
}

async fn list(
    f: &Fixture,
    creds: &Creds,
    sandbox: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sandbox}/shells"),
        None,
        creds,
    )
    .await
}

/// The ids and names of a shell list.
fn listed(resp: &support::Resp) -> Vec<(String, String)> {
    resp.body
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["id"].as_str().unwrap_or_default().to_owned(),
                        item["name"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
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
    f.h.app.end_all_shells().await;
    result
}

/// ACC-012 (F 2026-10-09): the superadmin, without a GitHub identity, opens a shell in a
/// sandbox, and Git in the shell gets the App's token for that repository only.
#[tokio::test]
async fn the_superadmin_opens_shells_with_the_apps_git_credential() -> TestResult {
    let f = fixture().await?;
    let result = superadmin_scenario(&f).await;
    f.h.app.stop_workspace_services().await;
    result
}

/// IDN-101 (F 2026-10-12): a shell's workspace token stays valid when the server restarts. Shells,
/// the Cockpit and editors are adopted with their environment, and their `gh` and Git lost
/// GitHub after every deploy while tokens were random per server lifetime.
#[tokio::test]
async fn workspace_tokens_stay_valid_across_a_server_restart() -> TestResult {
    use yulius_app::ports::store::{SandboxStore, UserStore};
    use yulius_domain::ids::UserId;
    async fn token_of(h: &Harness) -> Result<String, Box<dyn std::error::Error>> {
        let user = h
            .store
            .user(&UserId::parse("u-alice")?)
            .await?
            .ok_or("alice")?;
        let sandbox = h
            .store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
        let space = h.app.user_space(&user, &sandbox, None)?;
        Ok(space
            .env
            .iter()
            .find(|(name, _)| name == "YULIUS_TASK_TOKEN")
            .map(|(_, value)| value.clone())
            .ok_or("workspace token")?)
    }
    let f = fixture().await?;
    let token = token_of(&f.h).await?;
    f.h.app.stop_workspace_services().await;

    // The next server process on the same data directory accepts the token it never issued.
    let g = fixture_with(false, Some(f.data.clone())).await?;
    let ask = |bearer: String| {
        let internal = g.h.internal.clone();
        async move {
            call(
                &internal,
                Method::POST,
                "/internal/git-credential",
                Some(
                    json!({"protocol": "https", "host": "github.com", "path": "acme/fixture.git"}),
                ),
                &Creds {
                    bearer: Some(bearer),
                    ..Creds::default()
                },
            )
            .await
        }
    };
    let credential = ask(token.clone()).await?;
    assert_eq!(credential.status, StatusCode::OK, "{:?}", credential.body);
    assert_eq!(
        token_of(&g.h).await?,
        token,
        "the same token for the same user and sandbox"
    );
    let forged = ask(format!("ws_{}", "0".repeat(64))).await?;
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED);
    g.h.app.stop_workspace_services().await;
    Ok(())
}

async fn superadmin_scenario(f: &Fixture) -> TestResult {
    use yulius_app::ports::store::{InstanceStore, SandboxStore, UserStore};
    support::setup_superadmin_creds(&f.h).await?;
    support::configure_github_app(&f.h).await?;
    f.h.store.set_installation(Some(1)).await?;
    let owner = support::superadmin_bearer(&f.h).await?;
    let served = serve(f.h.router.clone()).await?;

    let created = create(f, &owner, "s1", None).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let mut socket = open_terminal(served.addr, &id, &owner).await?;
    let mut screen = String::new();
    input(&mut socket, "echo \"sum=$((6*7))\"\r").await?;
    read_until(&mut socket, &mut screen, "sum=42").await?;

    // The shell's workspace token (as the credential helper sends it) maps to the App's
    // repository token, not to a GitHub account the superadmin does not have.
    let superadmin = f.h.store.superadmin().await?.ok_or("superadmin")?;
    let sandbox =
        f.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    let space = f.h.app.user_space(&superadmin, &sandbox, None)?;
    let token = space
        .env
        .iter()
        .find(|(name, _)| name == "YULIUS_TASK_TOKEN")
        .map(|(_, value)| value.clone())
        .ok_or("workspace token")?;
    let workspace = Creds {
        bearer: Some(token),
        ..Creds::default()
    };
    let credential = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/fixture.git"})),
        &workspace,
    )
    .await?;
    assert_eq!(credential.status, StatusCode::OK, "{:?}", credential.body);
    assert_eq!(
        credential.body["token"], "inst",
        "the App installation's token"
    );
    let other = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/other.git"})),
        &workspace,
    )
    .await?;
    assert_eq!(other.status, StatusCode::FORBIDDEN, "only this repository");
    drop(socket);
    Ok(())
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

    // Read users cannot list either; a new user has no terminals yet.
    assert_eq!(
        list(f, &f.reader, "s1").await?.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(listed(&list(f, &f.writer, "s1").await?), vec![]);

    // An empty body: a login shell in the sandbox repository, named "Terminal 1".
    let created = create(f, &f.writer, "s1", None).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["name"], "Terminal 1");
    assert_eq!(created.body["task_id"], Value::Null);
    assert!(created.body["created_at"].is_string());
    // The folder it started in: paths printed in it link to the editor (F 2026-10-10).
    let folder = created.body["folder"].as_str().ok_or("folder")?.to_owned();
    assert!(folder.ends_with("sandboxes/s1/repo"), "{folder}");
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
    // bash reports the working directory at each prompt (OSC 7, from PROMPT_COMMAND), so paths
    // printed after a `cd` link to the right files; `sh` here runs the command by hand.
    input(&mut socket, "eval \"$PROMPT_COMMAND\"\r").await?;
    read_until(&mut socket, &mut screen, "sandboxes/s1/repo\u{1b}\\").await?;
    assert!(screen.contains("\u{1b}]7;file://"), "{screen:?}");

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

    // A task worktree of the same sandbox can be selected; names count up.
    let created = create(f, &f.writer, "s1", Some(json!({"task_id": "t1"}))).await?;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(created.body["name"], "Terminal 2");
    assert_eq!(created.body["task_id"], "t1");
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

    // Many terminals per user and sandbox, up to the bound; the list is in creation order.
    let mut extra = Vec::new();
    for _ in 2..MAX_SHELLS_PER_SANDBOX {
        let created = create(f, &f.writer, "s1", None).await?;
        assert_eq!(created.status, StatusCode::CREATED);
        extra.push(created.body["id"].as_str().ok_or("id")?.to_owned());
    }
    let over = create(f, &f.writer, "s1", None).await?;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS);
    let mine = listed(&list(f, &f.writer, "s1").await?);
    assert_eq!(mine.len(), MAX_SHELLS_PER_SANDBOX);
    assert_eq!(mine[0], (repo_shell.clone(), "Terminal 1".to_owned()));
    assert_eq!(mine[1], (worktree_shell.clone(), "Terminal 2".to_owned()));
    assert_eq!(
        create(f, &f.writer, "s2", None).await?.status,
        StatusCode::CONFLICT,
        "s2 has no repository"
    );

    // Renaming: only valid names; the list shows the new one.
    let path = format!("/api/sandboxes/s1/shells/{worktree_shell}");
    let renamed = call(
        &f.h.router,
        Method::PATCH,
        &path,
        Some(json!({"name": "  tests  "})),
        &f.writer,
    )
    .await?;
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);
    assert_eq!(renamed.body["name"], "tests");
    for bad in [
        json!({"name": ""}),
        json!({"name": "x".repeat(41)}),
        json!({"name": "a\u{7}b"}),
        json!({}),
    ] {
        let refused = call(
            &f.h.router,
            Method::PATCH,
            &path,
            Some(bad.clone()),
            &f.writer,
        )
        .await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    assert!(
        listed(&list(f, &f.writer, "s1").await?)
            .contains(&(worktree_shell.clone(), "tests".to_owned()))
    );

    // Another user's terminals stay private (UI-016): carol, a writer of the same sandbox, lists
    // none of alice's, cannot rename, close or attach to them, and has her own bound.
    assert_eq!(listed(&list(f, &carol, "s1").await?), vec![]);
    for (method, body) in [
        (Method::PATCH, Some(json!({"name": "mine"}))),
        (Method::DELETE, None),
    ] {
        let refused = call(&f.h.router, method.clone(), &path, body, &carol).await?;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{method}");
    }
    let carols = create(f, &carol, "s1", None).await?;
    assert_eq!(
        carols.status,
        StatusCode::CREATED,
        "her own bound: {:?}",
        carols.body
    );
    assert_eq!(carols.body["name"], "Terminal 1");
    let carols = carols.body["id"].as_str().ok_or("id")?.to_owned();
    assert!(
        !listed(&list(f, &f.writer, "s1").await?)
            .iter()
            .any(|(id, _)| *id == carols)
    );
    let alice_on_carols = ws_connect(
        served.addr,
        &format!("/api/shells/{carols}/terminal"),
        &f.writer,
        ORIGIN,
    )
    .await;
    assert_eq!(
        alice_on_carols.as_ref().err().and_then(ws_refusal_status),
        Some(404)
    );

    // A terminal belongs to its sandbox: another sandbox's list never shows it, and it cannot be
    // renamed or closed through another sandbox.
    assert!(listed(&list(f, &f.writer, "s2").await?).is_empty());
    let elsewhere = format!("/api/sandboxes/s2/shells/{worktree_shell}");
    for (method, body) in [
        (Method::PATCH, Some(json!({"name": "moved"}))),
        (Method::DELETE, None),
    ] {
        let refused = call(&f.h.router, method.clone(), &elsewhere, body, &f.writer).await?;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{method}");
    }

    // Closing: gone from the list at once, its process ends and its directory goes; the next
    // terminal takes the lowest free name ("Terminal 2" was renamed).
    let closing = extra.pop().ok_or("extra")?;
    let closing_dir = f.data.join("shells").join(&closing);
    let (closing_host, closing_child) = run_pids(&closing_dir)?;
    let closed = call(
        &f.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/s1/shells/{closing}"),
        None,
        &f.writer,
    )
    .await?;
    assert_eq!(closed.status, StatusCode::NO_CONTENT);
    assert!(
        !listed(&list(f, &f.writer, "s1").await?)
            .iter()
            .any(|(id, _)| *id == closing)
    );
    wait_for("the closed shell to exit", || {
        !pid_alive(closing_child) && !pid_alive(closing_host)
    })
    .await?;
    wait_for("its directory to go", || !closing_dir.exists()).await?;
    let again = call(
        &f.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/s1/shells/{closing}"),
        None,
        &f.writer,
    )
    .await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    let reopened = create(f, &f.writer, "s1", None).await?;
    assert_eq!(reopened.status, StatusCode::CREATED);
    assert_eq!(reopened.body["name"], "Terminal 2");
    extra.push(reopened.body["id"].as_str().ok_or("id")?.to_owned());

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
    // alice's remaining terminals and carol's one.
    assert_eq!(f.h.app.sandbox_shells(&sid).len(), MAX_SHELLS_PER_SANDBOX);

    // Shells without viewers for a day are ended (injected clock).
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
    assert_eq!(f.h.app.reap_idle_shells().await, MAX_SHELLS_PER_SANDBOX);
    assert!(f.h.app.sandbox_shells(&sid).is_empty());

    // Shells never touch task state (HRN-002).
    let task_after = f.h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;
    assert_eq!(task_after, task_before);
    assert!(f.h.store.attempts(&TaskId::parse("t1")?).await?.is_empty());
    Ok(())
}

/// SPEC-077 DRS-002 (#250): the client goes away while its terminal's process runs but before the
/// start finished. The terminal still ends live, usable and saved, never listed yet unusable;
/// ending it leaves nothing.
#[tokio::test]
async fn a_terminal_start_whose_request_is_dropped_still_ends_live() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    f.gate.arm();
    {
        let start = create(&f, &f.writer, "s1", None);
        tokio::pin!(start);
        tokio::select! {
            resp = &mut start => {
                return Err(format!("the start ended before it was held: {:?}", resp.map(|r| r.status)).into());
            }
            () = f.gate.reached() => {}
        }
    }
    f.gate.release();
    let shells = f.data.join("shells");
    wait_for("the dropped start's terminal to be saved", || {
        std::fs::read_dir(&shells).is_ok_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().join("shell.json").is_file())
        })
    })
    .await?;
    let listed = listed(&list(&f, &f.writer, "s1").await?);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let id = listed.first().map(|(id, _)| id.clone()).ok_or("id")?;
    let mut socket = open_terminal(served.addr, &id, &f.writer).await?;
    let mut screen = String::new();
    input(&mut socket, "echo \"sum=$((6*7))\"\r").await?;
    read_until(&mut socket, &mut screen, "sum=42").await?;
    drop(socket);
    f.h.app.end_all_shells().await;
    let left = std::fs::read_dir(&shells)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "nothing left behind");
    Ok(())
}

#[tokio::test]
async fn shells_keep_running_across_a_server_restart_and_are_adopted() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let created = create(&f, &f.writer, "s1", Some(json!({"name": "dev server"}))).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let run_dir = f.data.join("shells").join(&id);
    let (host_pid, child_pid) = run_pids(&run_dir)?;
    let mut socket = open_terminal(served.addr, &id, &f.writer).await?;
    let mut screen = String::new();
    input(&mut socket, "MARK=kept; echo \"before-$((40+2))\"\r").await?;
    read_until(&mut socket, &mut screen, "before-42").await?;
    drop(socket);

    // The server stops: the shell keeps running in its detached host.
    f.h.app.stop_workspace_services().await;
    assert!(f.h.app.sandbox_shells(&SandboxId::parse("s1")?).is_empty());
    assert!(
        pid_alive(child_pid) && pid_alive(host_pid),
        "the shell outlives the server"
    );
    assert!(run_dir.join("shell.json").exists());

    // Leftovers the next server cannot adopt are ended and removed: a directory without a
    // record, and a shell of a sandbox that was removed meanwhile.
    let orphan = f.data.join("shells").join("0123456789abcdef");
    std::fs::create_dir_all(&orphan)?;
    let gone_sandbox = create(&f, &f.writer, "s1", None).await?;
    let gone_id = gone_sandbox.body["id"].as_str().ok_or("id")?.to_owned();
    let gone_dir = f.data.join("shells").join(&gone_id);
    let (gone_host, gone_child) = run_pids(&gone_dir)?;
    f.h.app.stop_workspace_services().await;
    let record: Value = serde_json::from_slice(&std::fs::read(gone_dir.join("shell.json"))?)?;
    assert_eq!(record["sandbox"], "s1");
    let mut edited = record.clone();
    edited["sandbox"] = json!("s9");
    std::fs::write(gone_dir.join("shell.json"), serde_json::to_vec(&edited)?)?;

    // The next server process on the same data directory adopts it: same id, name and owner,
    // the screen so far, and the same shell process (its variable is still set).
    let g = fixture_with(false, Some(f.data.clone())).await?;
    let (adopted, removed) = g.h.app.adopt_shells().await;
    assert_eq!((adopted, removed), (1, 2));
    assert!(!orphan.exists());
    wait_for("the unadoptable shell to end", || {
        !pid_alive(gone_child) && !pid_alive(gone_host)
    })
    .await?;
    assert!(!gone_dir.exists());
    let listed_after = listed(&list(&g, &g.writer, "s1").await?);
    assert_eq!(listed_after, vec![(id.clone(), "dev server".to_owned())]);
    assert_eq!(
        list(&g, &g.reader, "s1").await?.status,
        StatusCode::FORBIDDEN
    );
    let served = serve(g.h.router.clone()).await?;
    let mut socket = open_terminal(served.addr, &id, &g.writer).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "before-42").await?;
    input(&mut socket, "echo \"mark=$MARK\"\r").await?;
    read_until(&mut socket, &mut screen, "mark=kept").await?;
    assert!(pid_alive(child_pid), "the same process");

    g.h.app.end_all_shells().await;
    wait_for("the shell processes to exit", || {
        !pid_alive(child_pid) && !pid_alive(host_pid)
    })
    .await?;
    assert!(!run_dir.exists());
    Ok(())
}

/// A contained shell runs as the user's sandbox identity: their login, root inside the
/// sandbox, `sudo` works, and files written to `~` land in their own persistent home
/// (UI-016, AC-UI-8).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn contained_shells_run_as_the_user_with_root_in_their_own_home() -> TestResult {
    // Like supervision's containment test: skip where bubblewrap is missing or cannot create
    // namespaces (CI runners), instead of failing on the environment.
    if let Err(why) = bwrap_runnable() {
        eprintln!("SKIPPED contained_shells_run_as_the_user_with_root_in_their_own_home: {why}");
        return Ok(());
    }
    let f = fixture_with(true, None).await?;
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
    f.h.app.end_all_shells().await;
    result
}

/// Whether bubblewrap runs here and can create the namespaces containment needs.
#[cfg(target_os = "linux")]
fn bwrap_runnable() -> Result<(), String> {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    match probe {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "bwrap cannot create namespaces: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(err) => Err(format!("bwrap not runnable: {err}")),
    }
}
