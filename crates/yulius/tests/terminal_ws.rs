//! Task terminal over the real public HTTP/WebSocket API with real processes: a task created
//! through `POST /api/sandboxes/{sid}/tasks` runs in the detached ptyhost; two viewers see the
//! same stream; a read-only viewer's input is refused; the writer's input reaches the agent
//! (SPEC-001 AC-TSK-2, AC-TSK-6; SPEC-002 §7.7).

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{
    HarnessOptions, TestResult, call, harness_with, perms, seed_github_user, seed_sandbox, serve,
    ws_connect,
};
use tokio_tungstenite::tungstenite::Message;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_contracts::terminal::{KIND_OUTPUT, KIND_SNAPSHOT, decode_binary};
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

const ORIGIN: &str = "https://yulius.test";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(())
}

/// Collected terminal traffic of one viewer.
#[derive(Default)]
struct Viewer {
    screen: String,
    texts: Vec<Value>,
}

impl Viewer {
    fn absorb(&mut self, message: Message) {
        match message {
            Message::Binary(bytes) => {
                if let Some((kind, _, data)) = decode_binary(&bytes)
                    && (kind == KIND_OUTPUT || kind == KIND_SNAPSHOT)
                {
                    self.screen.push_str(&String::from_utf8_lossy(data));
                }
            }
            Message::Text(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    self.texts.push(value);
                }
            }
            _ => {}
        }
    }
}

async fn read_until(
    socket: &mut Socket,
    viewer: &mut Viewer,
    what: &str,
    done: impl Fn(&Viewer) -> bool,
) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(viewer) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("timed out waiting for {what}; screen: {}", viewer.screen).into());
        }
        match tokio::time::timeout(left, socket.next()).await {
            Ok(Some(Ok(message))) => viewer.absorb(message),
            Ok(Some(Err(err))) => return Err(err.into()),
            Ok(None) => return Err(format!("socket closed while waiting for {what}").into()),
            Err(_) => {}
        }
    }
    Ok(())
}

/// A harness with the fake agent, a cloned fixture repository, an account, and two users of
/// sandbox `s1`: alice (write) and bob (read). `second` is alice on another device ("carol" in
/// the tests' labels): only a task's owner types into it (SPEC-070), from any of her windows.
struct Fixture {
    h: support::Harness,
    writer: support::Creds,
    reader: support::Creds,
    second: support::Creds,
    sandbox: String,
}

async fn fixture(options: FakeAgentOptions) -> Result<Fixture, Box<dyn std::error::Error>> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host: Some(Arc::new(PtySessionHost::new(exe.clone()))),
        harness: Some(Arc::new(FakeAgentHarness::new(exe, fake, options))),
        dir: Some(dir),
        configure: Some(Box::new(|c| {
            c.contained = false;
            c.viewer_recheck_ms = 300;
        })),
        ..HarnessOptions::default()
    })?;
    let writer = seed_github_user(&h, "u-w", "alice").await?;
    let reader = seed_github_user(&h, "u-r", "bob").await?;
    let sandbox = seed_sandbox(&h, "s1", "fixture", "u-w").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let alice = h
        .app
        .deps
        .store
        .user(&yulius_domain::ids::UserId::parse("u-w")?)
        .await?
        .ok_or("alice")?;
    let second = support::Creds {
        bearer: Some(
            h.app
                .issue_api_token(&alice, "second device")
                .await?
                .expose()
                .to_owned(),
        ),
        ..support::Creds::default()
    };
    h.app
        .deps
        .store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a1")?,
            provider: Provider::Anthropic,
            label: "Claude".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-terminal-test")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
        })
        .await?;

    // The sandbox clone: a bare origin with one commit.
    let data = h.app.config.data_dir.clone();
    let origin = data.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join("seed");
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "# fixture\n")?;
    git(&seed, &["add", "README.md"])?;
    git(&seed, &["commit", "-q", "-m", "init"])?;
    git(
        &seed,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&seed, &["push", "-q", "origin", "main"])?;
    let repo = data
        .join("sandboxes")
        .join(sandbox.id.as_str())
        .join("repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        &data,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;

    Ok(Fixture {
        h,
        writer,
        reader,
        second,
        sandbox: sandbox.id.as_str().to_owned(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_viewers_share_the_terminal_and_only_writers_type() -> TestResult {
    let Fixture {
        h,
        writer,
        reader,
        second,
        sandbox,
    } = fixture(FakeAgentOptions::default()).await?;

    // Read users cannot create tasks; writers can.
    let path = format!("/api/sandboxes/{sandbox}/tasks");
    let body = json!({"prompt": "Terminal sharing check", "harness": "claude"});
    let refused = call(
        &h.router,
        axum::http::Method::POST,
        &path,
        Some(body.clone()),
        &reader,
    )
    .await?;
    assert_eq!(refused.status, 403, "{:?}", refused.body);
    let created = call(
        &h.router,
        axum::http::Method::POST,
        &path,
        Some(body),
        &writer,
    )
    .await?;
    assert_eq!(created.status, 201, "{:?}", created.body);
    let tid = created.body["id"].as_str().ok_or("task id")?.to_owned();

    let served = serve(h.router.clone()).await?;
    let terminal = format!("/api/tasks/{tid}/terminal");
    // The viewers' sizes travel with the connection (F 2026-10-09): the writer, whom nobody
    // controls yet, gets its size applied before its first state and screen; a reader's size
    // changes nothing.
    let mut w = ws_connect(
        served.addr,
        &format!("{terminal}?cols=91&rows=27"),
        &writer,
        ORIGIN,
    )
    .await?;
    let mut r = ws_connect(
        served.addr,
        &format!("{terminal}?cols=50&rows=10"),
        &reader,
        ORIGIN,
    )
    .await?;
    let (mut wv, mut rv) = (Viewer::default(), Viewer::default());
    for (socket, viewer, who) in [(&mut w, &mut wv, "writer"), (&mut r, &mut rv, "reader")] {
        read_until(socket, viewer, &format!("the {who} banner"), |v| {
            v.screen.contains("FAKE-AGENT READY")
        })
        .await?;
    }
    let state = |v: &Viewer| v.texts.iter().find(|t| t["type"] == "state").cloned();
    let first = state(&wv).ok_or("writer state")?;
    assert_eq!(first["can_write"], true);
    assert_eq!(
        (&first["cols"], &first["rows"], &first["controller"]),
        (&json!(91), &json!(27), &json!(true)),
        "the first state already has the writer's size: {first}"
    );
    let watcher = state(&rv).ok_or("reader state")?;
    assert_eq!(watcher["can_write"], false);
    assert_eq!(
        (&watcher["cols"], &watcher["rows"]),
        (&json!(91), &json!(27))
    );

    // The read-only viewer's keystrokes never reach the agent.
    r.send(Message::Text(
        json!({"type": "input", "data": "intruder\r"})
            .to_string()
            .into(),
    ))
    .await?;
    read_until(&mut r, &mut rv, "the refusal", |v| {
        v.texts
            .iter()
            .any(|t| t["type"] == "error" && t["code"] == "forbidden")
    })
    .await?;

    // The writer's input reaches the agent and both viewers see the answer.
    w.send(Message::Text(
        json!({"type": "input", "data": "hello viewers\r", "seq": 1})
            .to_string()
            .into(),
    ))
    .await?;
    read_until(&mut w, &mut wv, "the echo (writer)", |v| {
        v.screen.contains("ECHO: hello viewers")
    })
    .await?;
    read_until(&mut r, &mut rv, "the echo (reader)", |v| {
        v.screen.contains("ECHO: hello viewers")
    })
    .await?;
    assert!(!wv.screen.contains("ECHO: intruder") && !rv.screen.contains("ECHO: intruder"));

    // TSK-006: the last writer to type controls the terminal; every viewer learns who.
    let last_state = |v: &Viewer| v.texts.iter().rev().find(|t| t["type"] == "state").cloned();
    assert_eq!(
        last_state(&wv).ok_or("writer state")?["controller"],
        true,
        "alice typed last"
    );
    let mut c = ws_connect(served.addr, &terminal, &second, ORIGIN).await?;
    let mut cv = Viewer::default();
    read_until(&mut c, &mut cv, "carol's banner", |v| {
        v.screen.contains("ECHO: hello viewers")
    })
    .await?;
    assert_eq!(last_state(&cv).ok_or("carol state")?["controller"], false);
    c.send(Message::Text(
        json!({"type": "input", "data": "carol here\r", "seq": 1})
            .to_string()
            .into(),
    ))
    .await?;
    read_until(&mut c, &mut cv, "carol takes control", |v| {
        last_state(v).is_some_and(|s| s["controller"] == true)
    })
    .await?;
    read_until(&mut w, &mut wv, "alice loses control", |v| {
        last_state(v).is_some_and(|s| s["controller"] == false)
    })
    .await?;
    // An explicit "take control" hands it back.
    w.send(Message::Text(
        json!({"type": "take_control"}).to_string().into(),
    ))
    .await?;
    read_until(&mut w, &mut wv, "alice takes control back", |v| {
        last_state(v).is_some_and(|s| s["controller"] == true)
    })
    .await?;
    // #78: a wheel turn another writer forwards to a full-screen app (`scroll`) reaches the app
    // but neither takes control nor resizes the terminal to that viewer's window.
    let task_id = yulius_domain::ids::TaskId::parse(&tid)?;
    let controller = h.app.runtime(&task_id).hub.controller();
    c.send(Message::Text(
        json!({"type": "input", "data": "\u{1b}[<65;1;1M", "seq": 2, "scroll": true})
            .to_string()
            .into(),
    ))
    .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        h.app.runtime(&task_id).hub.controller(),
        controller,
        "scrolling does not take control"
    );
    drop(c);

    // AC-ACC-5: a downgrade on GitHub reaches open sockets once the permission cache expires.
    h.github.set_perm("alice", "acme/fixture", perms("read"));
    h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    read_until(&mut w, &mut wv, "the downgrade", |v| {
        v.texts
            .iter()
            .any(|t| t["type"] == "state" && t["can_write"] == false)
    })
    .await?;
    let refusals = |v: &Viewer| {
        v.texts
            .iter()
            .filter(|t| t["type"] == "error" && t["code"] == "forbidden")
            .count()
    };
    let before = refusals(&wv);
    w.send(Message::Text(
        json!({"type": "input", "data": "after downgrade\r", "seq": 2})
            .to_string()
            .into(),
    ))
    .await?;
    read_until(&mut w, &mut wv, "the downgraded refusal", |v| {
        refusals(v) > before
    })
    .await?;

    // Losing access entirely closes the viewer.
    if let Ok(mut map) = h.github.perms.lock() {
        map.remove(&("bob".to_owned(), "acme/fixture".to_owned()));
    }
    h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut closed = false;
    while Instant::now() < deadline && !closed {
        match tokio::time::timeout(Duration::from_secs(1), r.next()).await {
            Ok(Some(Ok(Message::Close(frame)))) => {
                closed = true;
                assert_eq!(frame.map(|f| u16::from(f.code)), Some(4003));
            }
            Ok(None | Some(Err(_))) => closed = true,
            _ => {}
        }
    }
    assert!(closed, "a viewer without access must be disconnected");

    // Stop through the API (write access restored); the run ends.
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    let stopped = call(
        &h.router,
        axum::http::Method::POST,
        &format!("/api/tasks/{tid}/stop"),
        None,
        &writer,
    )
    .await?;
    assert_eq!(stopped.status, 200, "{:?}", stopped.body);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let detail = call(
            &h.router,
            axum::http::Method::GET,
            &format!("/api/tasks/{tid}"),
            None,
            &writer,
        )
        .await?;
        let status = detail.body["status"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if status.starts_with("stopped") || status == "closed" {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!("task did not stop: {status}").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

/// SPEC-118 KEY-001, KEY-004, KEY-005 (task #103: "things should be written instantly"): once a
/// writer's access is confirmed, its keystrokes, resizes and presence reach the agent and the echo
/// comes back while every permission check hangs — GitHub stops answering and the permission cache
/// has expired, so the 300 ms re-check that starts meanwhile never finishes: the viewer's loop does
/// not wait for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keystrokes_and_echo_never_wait_for_a_permission_check() -> TestResult {
    let Fixture {
        h, writer, sandbox, ..
    } = fixture(FakeAgentOptions::default()).await?;
    let created = call(
        &h.router,
        axum::http::Method::POST,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        Some(json!({"prompt": "Typing latency check", "harness": "claude"})),
        &writer,
    )
    .await?;
    assert_eq!(created.status, 201, "{:?}", created.body);
    let tid = created.body["id"].as_str().ok_or("task id")?.to_owned();
    let served = serve(h.router.clone()).await?;
    let mut w = ws_connect(
        served.addr,
        &format!("/api/tasks/{tid}/terminal?cols=90&rows=30"),
        &writer,
        ORIGIN,
    )
    .await?;
    let mut wv = Viewer::default();
    read_until(&mut w, &mut wv, "the banner", |v| {
        v.screen.contains("FAKE-AGENT READY")
    })
    .await?;
    send(
        &mut w,
        json!({"type": "input", "data": "warm up\r", "seq": 1}),
    )
    .await?;
    read_until(&mut w, &mut wv, "the first echo", |v| {
        v.screen.contains("ECHO: warm up")
    })
    .await?;

    h.github
        .hang_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);
    h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    for i in 0..4u64 {
        send(&mut w, json!({"type": "active"})).await?;
        send(&mut w, json!({"type": "resize", "cols": 90, "rows": 30})).await?;
        let text = format!("key {i}");
        send(
            &mut w,
            json!({"type": "input", "data": format!("{text}\r"), "seq": i + 2}),
        )
        .await?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !wv.screen.contains(&format!("ECHO: {text}")) {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, w.next()).await {
                Ok(Some(Ok(message))) => wv.absorb(message),
                Ok(other) => return Err(format!("socket ended while typing: {other:?}").into()),
                Err(_) => {
                    return Err(format!(
                        "{text:?} was not echoed within 5 s while permission checks hang"
                    )
                    .into());
                }
            }
        }
        // Typing goes on after the 300 ms re-check has started (and hangs).
        tokio::time::sleep(Duration::from_millis(350)).await;
    }
    Ok(())
}

async fn send(socket: &mut Socket, message: Value) -> TestResult {
    socket
        .send(Message::Text(message.to_string().into()))
        .await?;
    Ok(())
}

fn last_state(viewer: &Viewer) -> Option<&Value> {
    viewer.texts.iter().rev().find(|t| t["type"] == "state")
}

/// The viewer's latest state says it controls (or not) a terminal of `cols`×`rows`.
fn sized(viewer: &Viewer, controller: bool, cols: u16, rows: u16) -> bool {
    last_state(viewer).is_some_and(|s| {
        s["controller"] == controller && s["cols"] == json!(cols) && s["rows"] == json!(rows)
    })
}

/// F 2026-10-10 ("each user should be able to control it on their side"): the PTY takes the
/// active viewer's window size, automatically. A viewer alone keeps it while idle; a viewer who
/// opens the task takes it from an idle one; typing (or `take_control`, a click) takes it at once;
/// a watcher's own size is kept for when it controls; an idle or leaving controller hands it to
/// the active viewer, whose last reported size the PTY takes at once (TSK-006, UI-019).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_active_viewer_sets_the_size_and_an_idle_one_hands_it_over() -> TestResult {
    let Fixture {
        h,
        writer,
        second,
        sandbox,
        ..
    } = fixture(FakeAgentOptions::default()).await?;
    let created = call(
        &h.router,
        axum::http::Method::POST,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        Some(json!({"prompt": "Size hand-over check", "harness": "claude"})),
        &writer,
    )
    .await?;
    assert_eq!(created.status, 201, "{:?}", created.body);
    let tid = created.body["id"].as_str().ok_or("task id")?.to_owned();
    let task_id = yulius_domain::ids::TaskId::parse(&tid)?;
    let served = serve(h.router.clone()).await?;
    let terminal = format!("/api/tasks/{tid}/terminal");
    let pty = || h.app.runtime(&task_id).hub.size();

    // Alice alone: her window's size, kept while she is idle (her window changes still apply).
    let mut a = ws_connect(
        served.addr,
        &format!("{terminal}?cols=100&rows=30"),
        &writer,
        ORIGIN,
    )
    .await?;
    let mut av = Viewer::default();
    read_until(&mut a, &mut av, "alice's banner", |v| {
        v.screen.contains("FAKE-AGENT READY") && sized(v, true, 100, 30)
    })
    .await?;
    send(&mut a, json!({"type": "idle"})).await?;
    send(&mut a, json!({"type": "resize", "cols": 104, "rows": 31})).await?;
    read_until(&mut a, &mut av, "alice keeps the size while idle", |v| {
        sized(v, true, 104, 31)
    })
    .await?;

    // Carol opens the task while alice is idle: the terminal takes carol's size at once.
    let mut c = ws_connect(
        served.addr,
        &format!("{terminal}?cols=140&rows=40"),
        &second,
        ORIGIN,
    )
    .await?;
    let mut cv = Viewer::default();
    let first = |v: &Viewer| v.texts.iter().find(|t| t["type"] == "state").cloned();
    read_until(&mut c, &mut cv, "carol's first state", |v| {
        first(v).is_some()
    })
    .await?;
    let first = first(&cv).ok_or("carol's state")?;
    assert_eq!(
        (&first["controller"], &first["cols"], &first["rows"]),
        (&json!(true), &json!(140), &json!(40)),
        "carol's first state already has her size: {first}"
    );
    read_until(&mut a, &mut av, "alice watches carol's size", |v| {
        sized(v, false, 140, 40)
    })
    .await?;

    // Alice comes back while carol is active: nothing moves; her own size is kept for later.
    send(&mut a, json!({"type": "active"})).await?;
    send(&mut a, json!({"type": "resize", "cols": 100, "rows": 30})).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(pty(), (140, 40), "an active viewer keeps the size");
    // Alice types: her window's size, at once.
    send(
        &mut a,
        json!({"type": "input", "data": "/size\r", "seq": 1}),
    )
    .await?;
    read_until(&mut a, &mut av, "alice takes the size by typing", |v| {
        sized(v, true, 100, 30) && v.screen.contains("SIZE 30 100")
    })
    .await?;
    read_until(&mut c, &mut cv, "carol watches alice's size", |v| {
        sized(v, false, 100, 30)
    })
    .await?;

    // Carol's window changes while she watches: kept, not applied.
    send(&mut c, json!({"type": "resize", "cols": 150, "rows": 45})).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(pty(), (100, 30), "a watcher's size waits");
    // Alice goes idle: carol, active, gets the size she reported last.
    send(&mut a, json!({"type": "idle"})).await?;
    read_until(
        &mut c,
        &mut cv,
        "carol gets the size when alice idles",
        |v| sized(v, true, 150, 45),
    )
    .await?;

    // A click (`take_control`) takes it back; leaving hands it to carol again.
    send(&mut a, json!({"type": "take_control"})).await?;
    read_until(&mut a, &mut av, "alice clicks into the terminal", |v| {
        sized(v, true, 100, 30)
    })
    .await?;
    // Carol's view must have seen the click first: her last state is still the earlier
    // 150×45 control, so waiting for that below would pass at once, before alice has left, and
    // carol's typing would race alice's departure (seen on a loaded CI runner).
    read_until(&mut c, &mut cv, "carol watches alice's click", |v| {
        sized(v, false, 100, 30)
    })
    .await?;
    drop(a);
    read_until(
        &mut c,
        &mut cv,
        "carol gets the size when alice leaves",
        |v| sized(v, true, 150, 45),
    )
    .await?;
    send(
        &mut c,
        json!({"type": "input", "data": "/size\r", "seq": 1}),
    )
    .await?;
    read_until(
        &mut c,
        &mut cv,
        "the agent's terminal is carol's size",
        |v| v.screen.contains("SIZE 45 150"),
    )
    .await?;

    let stopped = call(
        &h.router,
        axum::http::Method::POST,
        &format!("/api/tasks/{tid}/stop"),
        None,
        &writer,
    )
    .await?;
    assert_eq!(stopped.status, 200, "{:?}", stopped.body);
    Ok(())
}
