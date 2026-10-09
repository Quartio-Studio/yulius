//! Task terminal over the real public HTTP/WebSocket API with real processes: a task created
//! through `POST /api/sandboxes/{sid}/tasks` runs in the detached ptyhost; two viewers see the
//! same stream; a read-only viewer's input is refused; the writer's input reaches the agent
//! (SPEC-001 AC-TSK-2, AC-TSK-6; SPEC-002 §7.7).

mod support;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_viewers_share_the_terminal_and_only_writers_type() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host: Some(Arc::new(PtySessionHost::new(exe.clone()))),
        harness: Some(Arc::new(FakeAgentHarness::new(
            exe,
            fake,
            FakeAgentOptions::default(),
        ))),
        dir: Some(dir),
        configure: Some(Box::new(|c| {
            c.contained = false;
            c.viewer_recheck_ms = 300;
        })),
        ..HarnessOptions::default()
    })?;
    let writer = seed_github_user(&h, "u-w", "alice").await?;
    let reader = seed_github_user(&h, "u-r", "bob").await?;
    let second = seed_github_user(&h, "u-c", "carol").await?;
    let sandbox = seed_sandbox(&h, "s1", "fixture", "u-w").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    h.github.set_perm("carol", "acme/fixture", perms("write"));
    h.app
        .deps
        .store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a1")?,
            provider: Provider::Anthropic,
            label: "Claude".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-terminal-test")?),
            priority: 0,
            enabled: true,
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
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

    // Read users cannot create tasks; writers can.
    let path = format!("/api/sandboxes/{}/tasks", sandbox.id.as_str());
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
    let mut w = ws_connect(served.addr, &terminal, &writer, ORIGIN).await?;
    let mut r = ws_connect(served.addr, &terminal, &reader, ORIGIN).await?;
    let (mut wv, mut rv) = (Viewer::default(), Viewer::default());
    for (socket, viewer, who) in [(&mut w, &mut wv, "writer"), (&mut r, &mut rv, "reader")] {
        read_until(socket, viewer, &format!("the {who} banner"), |v| {
            v.screen.contains("FAKE-AGENT READY")
        })
        .await?;
    }
    let state = |v: &Viewer| v.texts.iter().find(|t| t["type"] == "state").cloned();
    assert_eq!(state(&wv).ok_or("writer state")?["can_write"], true);
    assert_eq!(state(&rv).ok_or("reader state")?["can_write"], false);

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
