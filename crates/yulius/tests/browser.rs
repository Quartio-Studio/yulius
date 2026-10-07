//! The shared sandbox browser end to end: the viewer WebSocket, agent MCP tools and the
//! user-action hand-off, over the real Chromium manager with a fake Chromium launcher and an
//! in-process fake DevTools server (SPEC-001 BRW-001..004, AC-BRW-1..3).

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::{
    Harness, HarnessOptions, TestResult, harness_with, mcp_tool, perms, seed_github_user,
    seed_live_attempt, seed_sandbox, seed_task, serve, tool_text, ws_connect, ws_refusal_status,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius::wiring::browser::ChromiumBrowsers;
use yulius_app::ports::store::TaskStore;
use yulius_browser::fake::{
    FAKE_ARGV_FILE, FAKE_PID_FILE, FakeCdpServer, FakeChromium, write_fake_chromium,
};
use yulius_browser::{BrowserConfig, StopTimeouts};
use yulius_contracts::browser::decode_frame;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const ORIGIN: &str = "https://yulius.test";
const TOKEN: &str = "browser-task-token-0123456789abcdef";
const WAIT: Duration = Duration::from_secs(10);

fn browser_config(exe: &Path, root: &Path) -> BrowserConfig {
    BrowserConfig {
        // The first exec of a freshly written script can be slow on a loaded macOS host.
        startup_timeout: Duration::from_secs(30),
        stop: StopTimeouts {
            graceful: Duration::from_millis(100),
            term: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        },
        ..BrowserConfig::new(exe.to_path_buf(), root.to_path_buf())
    }
}

struct Fixture {
    h: Harness,
    fake: FakeCdpServer,
    writer: support::Creds,
    reader: support::Creds,
    _scripts: tempfile::TempDir,
}

/// Users alice (write) and bob (read) on sandboxes `s1` (`fixture`) and `s2` (`other`); task
/// `t1` of `s1` runs with a live MCP token.
async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let fake = FakeCdpServer::start().await?;
    let scripts = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        scripts.path(),
        "fake-chromium",
        FakeChromium::Ready { port: fake.port() },
    )?;
    let browsers = ChromiumBrowsers::new(browser_config(&exe, &scripts.path().join("unused")));
    let h = harness_with(HarnessOptions {
        browser: Some(Arc::new(browsers)),
        ..HarnessOptions::default()
    })?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    for repo in ["acme/fixture", "acme/other"] {
        h.github.set_perm("alice", repo, perms("write"));
        h.github.set_perm("bob", repo, perms("read"));
    }
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    Ok(Fixture {
        h,
        fake,
        writer,
        reader,
        _scripts: scripts,
    })
}

/// Next JSON message, skipping binary frames; fails after [`WAIT`].
async fn next_json(socket: &mut Socket) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "timed out waiting for a JSON message")?
            .ok_or("socket closed")??;
        match message {
            Message::Text(text) => return Ok(serde_json::from_str(text.as_str())?),
            Message::Close(frame) => return Err(format!("closed: {frame:?}").into()),
            _ => {}
        }
    }
}

/// Waits for a JSON message satisfying `predicate`.
async fn wait_json(
    socket: &mut Socket,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let message = next_json(socket).await?;
        if predicate(&message) {
            return Ok(message);
        }
    }
}

/// Next binary frame.
async fn next_binary(socket: &mut Socket) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "timed out waiting for a frame")?
            .ok_or("socket closed")??;
        if let Message::Binary(bytes) = message {
            return Ok(bytes.to_vec());
        }
    }
}

async fn send(socket: &mut Socket, value: Value) -> TestResult {
    socket.send(Message::text(value.to_string())).await?;
    Ok(())
}

fn pid_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

async fn wait_dead(pid: i32) -> bool {
    let deadline = tokio::time::Instant::now() + WAIT;
    while tokio::time::Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Runs `body`, then always stops browsers so no fake Chromium outlives the test.
async fn with_fixture<F>(body: impl FnOnce(Arc<Fixture>) -> F) -> TestResult
where
    F: std::future::Future<Output = TestResult>,
{
    let f = Arc::new(fixture().await?);
    let result = body(Arc::clone(&f)).await;
    f.h.app.stop_workspace_services().await;
    result
}

#[tokio::test]
async fn viewers_need_write_access_and_drive_a_per_sandbox_browser() -> TestResult {
    with_fixture(|f| async move {
        let served = serve(f.h.router.clone()).await?;
        let path = "/api/sandboxes/s1/browser";

        // Read users and cross-origin pages are refused before the upgrade.
        let refused = ws_connect(served.addr, path, &f.reader, ORIGIN).await;
        assert_eq!(refused.as_ref().err().and_then(ws_refusal_status), Some(403));
        let cross = ws_connect(served.addr, path, &f.writer, "https://evil.test").await;
        assert_eq!(cross.as_ref().err().and_then(ws_refusal_status), Some(403));
        let anonymous = ws_connect(served.addr, path, &support::Creds::default(), ORIGIN).await;
        assert_eq!(anonymous.as_ref().err().and_then(ws_refusal_status), Some(401));

        // A write user gets the tab list, then JPEG frames of the active tab.
        let mut socket = ws_connect(served.addr, path, &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        assert_eq!(tabs["type"], "tabs");
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        let frame = next_binary(&mut socket).await?;
        let (tab, jpeg) = decode_frame(&frame)?;
        assert_eq!(tab.as_str(), first);
        assert!(jpeg.starts_with(&[0xFF, 0xD8]), "JPEG payload");

        // Typing an address opens a tab; it becomes active and its frames follow.
        send(&mut socket, json!({"type": "open", "url": "example.com"})).await?;
        let opened = wait_json(&mut socket, |m| {
            m["type"] == "tabs" && m["tabs"].as_array().is_some_and(|t| t.len() == 2)
        })
        .await?;
        let active: Vec<&Value> = opened["tabs"]
            .as_array()
            .ok_or("tabs")?
            .iter()
            .filter(|t| t["active"] == true)
            .collect();
        assert_eq!(active.len(), 1);
        let new_tab = active[0]["id"].as_str().ok_or("id")?.to_owned();
        assert!(active[0]["url"].as_str().is_some_and(|u| u.starts_with("https://example.com")));
        assert_ne!(new_tab, first);

        send(&mut socket, json!({"type": "navigate", "tab_id": new_tab, "url": "http://localhost:43000/"}))
            .await?;
        wait_json(&mut socket, |m| {
            m["type"] == "tabs"
                && m["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter().any(|t| t["id"] == new_tab.as_str() && t["url"] == "http://localhost:43000/")
                })
        })
        .await?;
        send(&mut socket, json!({"type": "mouse", "tab_id": new_tab, "event": "down", "x": 10, "y": 20, "button": "left"}))
            .await?;
        send(&mut socket, json!({"type": "key", "tab_id": new_tab, "event": "char", "text": "hi"})).await?;
        assert!(f.fake.wait_for_calls("Input.dispatchMouseEvent", 1, WAIT).await);
        assert!(f.fake.wait_for_calls("Input.dispatchKeyEvent", 1, WAIT).await);

        // Refused addresses and malformed messages are reported, not fatal.
        send(&mut socket, json!({"type": "open", "url": "file:///etc/passwd"})).await?;
        let refused = wait_json(&mut socket, |m| m["type"] == "error").await?;
        assert_eq!(refused["code"], "url_refused");
        socket.send(Message::text("{\"type\":\"teleport\"}")).await?;
        let invalid = wait_json(&mut socket, |m| m["type"] == "error").await?;
        assert_eq!(invalid["code"], "invalid");

        // The profile lives in the sandbox directory; another sandbox gets its own profile.
        let data = f.h.app.config.data_dir.clone();
        let profile = data.join("sandboxes/s1/browser");
        let argv = std::fs::read_to_string(profile.join(FAKE_ARGV_FILE))?;
        assert!(argv.contains(&format!("--user-data-dir={}", profile.display())), "{argv}");
        let mut other = ws_connect(served.addr, "/api/sandboxes/s2/browser", &f.writer, ORIGIN).await?;
        assert_eq!(next_json(&mut other).await?["type"], "tabs");
        let other_profile = data.join("sandboxes/s2/browser");
        let other_argv = std::fs::read_to_string(other_profile.join(FAKE_ARGV_FILE))?;
        assert!(other_argv.contains(&format!("--user-data-dir={}", other_profile.display())));
        let pids: Vec<i32> = [&profile, &other_profile]
            .iter()
            .map(|dir| std::fs::read_to_string(dir.join(FAKE_PID_FILE)).map_err(|e| e.to_string()))
            .map(|text| text.and_then(|t| t.trim().parse::<i32>().map_err(|e| e.to_string())))
            .collect::<Result<_, _>>()?;
        assert_ne!(pids[0], pids[1], "one browser process per sandbox");

        // Server shutdown stops and reaps both browsers.
        drop(socket);
        drop(other);
        f.h.app.stop_workspace_services().await;
        for pid in pids {
            assert!(wait_dead(pid).await, "browser {pid} reaped");
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn agent_tools_drive_the_tabs_the_viewer_sees() -> TestResult {
    with_fixture(|f| async move {
        let listed = support::call(
            &f.h.internal,
            axum::http::Method::POST,
            "/mcp",
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})),
            &support::Creds {
                bearer: Some(TOKEN.into()),
                ..support::Creds::default()
            },
        )
        .await?;
        let names: Vec<&str> = listed.body["result"]["tools"]
            .as_array()
            .ok_or("tools")?
            .iter()
            .filter_map(|t| t["name"].as_str())
            .filter(|n| n.starts_with("browser_"))
            .collect();
        assert_eq!(names.len(), 8, "{names:?}");

        let served = serve(f.h.router.clone()).await?;
        let mut viewer =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        assert_eq!(next_json(&mut viewer).await?["type"], "tabs");

        // The agent opens a tab; the viewer sees the same tab appear.
        let opened = mcp_tool(
            &f.h,
            TOKEN,
            "browser_open",
            json!({"url": "https://docs.example"}),
        )
        .await?;
        assert_eq!(opened["isError"], false, "{opened}");
        let navigation = tool_text(&opened);
        assert_eq!(navigation["loaded"], true);
        let tab = navigation["tab"]["id"].as_str().ok_or("tab id")?.to_owned();
        wait_json(&mut viewer, |m| {
            m["type"] == "tabs"
                && m["tabs"]
                    .as_array()
                    .is_some_and(|tabs| tabs.iter().any(|t| t["id"] == tab.as_str()))
        })
        .await?;

        let tabs = tool_text(&mcp_tool(&f.h, TOKEN, "browser_tabs", json!({})).await?);
        assert!(
            tabs["tabs"]
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t["id"] == tab.as_str()))
        );
        let navigated = mcp_tool(
            &f.h,
            TOKEN,
            "browser_navigate",
            json!({"tab_id": tab, "url": "https://docs.example/login"}),
        )
        .await?;
        assert_eq!(
            tool_text(&navigated)["tab"]["url"],
            "https://docs.example/login"
        );
        let clicked = tool_text(
            &mcp_tool(
                &f.h,
                TOKEN,
                "browser_click",
                json!({"tab_id": tab, "selector": "#login"}),
            )
            .await?,
        );
        assert_eq!(clicked["x"], 100.5);
        let typed = tool_text(
            &mcp_tool(
                &f.h,
                TOKEN,
                "browser_type",
                json!({"tab_id": tab, "text": "alice\n"}),
            )
            .await?,
        );
        assert_eq!(typed["chars"], 6);
        assert!(f.fake.wait_for_calls("Input.insertText", 1, WAIT).await);
        let snapshot =
            tool_text(&mcp_tool(&f.h, TOKEN, "browser_snapshot", json!({"tab_id": tab})).await?);
        assert!(
            snapshot["outline"]
                .as_str()
                .is_some_and(|o| o.contains("button \"Sign in\""))
        );

        // Screenshots are image content within the 256 KiB result cap.
        let small = mcp_tool(&f.h, TOKEN, "browser_screenshot", json!({"tab_id": tab})).await?;
        assert_eq!(small["content"][0]["type"], "image");
        assert_eq!(small["content"][0]["mimeType"], "image/png");
        f.fake.set_screenshot_base64_len(1_500_000);
        let large = mcp_tool(&f.h, TOKEN, "browser_screenshot", json!({"tab_id": tab})).await?;
        assert_eq!(large["isError"], false);
        assert_eq!(large["content"][0]["mimeType"], "image/jpeg");
        assert!(serde_json::to_string(&large)?.len() <= yulius_app::mcp::MAX_RESULT_BYTES);
        assert!(
            large["content"][1]["text"]
                .as_str()
                .is_some_and(|t| t.contains("scale"))
        );

        // Failures are tool errors with actionable text.
        let unknown = mcp_tool(&f.h, TOKEN, "browser_snapshot", json!({"tab_id": "NOPE"})).await?;
        assert_eq!(unknown["isError"], true);
        assert!(
            tool_text(&unknown)
                .as_str()
                .is_some_and(|t| t.contains("unknown tab"))
        );
        let bad = mcp_tool(&f.h, TOKEN, "browser_click", json!({"tab_id": tab})).await?;
        assert_eq!(bad["isError"], true);
        let refused = mcp_tool(
            &f.h,
            TOKEN,
            "browser_open",
            json!({"url": "javascript:alert(1)"}),
        )
        .await?;
        assert_eq!(refused["isError"], true);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn user_action_requests_mark_the_task_notify_viewers_and_return_a_link() -> TestResult {
    with_fixture(|f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut viewer =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut viewer).await?;
        let tab = tabs["tabs"][0]["id"].as_str().ok_or("tab")?.to_owned();
        let mut events = f.h.app.events.subscribe();

        let empty = mcp_tool(
            &f.h,
            TOKEN,
            "browser_request_user_action",
            json!({"tab_id": tab, "reason": "  "}),
        )
        .await?;
        assert_eq!(empty["isError"], true);

        let result = mcp_tool(
            &f.h,
            TOKEN,
            "browser_request_user_action",
            json!({"tab_id": tab, "reason": "Log in to AWS"}),
        )
        .await?;
        assert_eq!(result["isError"], false, "{result}");
        let body = tool_text(&result);
        assert_eq!(
            body["link"],
            format!("https://yulius.test/s/fixture/browser?tab={tab}")
        );

        // The task needs attention and the UI stream carries the request.
        let task = f.h.store.task(&TaskId::parse("t1")?).await?.ok_or("task")?;
        assert_eq!(task.activity, Some(Activity::NeedsAttention));
        let mut saw_request = false;
        let mut saw_task = false;
        while let Ok(Ok(envelope)) =
            tokio::time::timeout(Duration::from_secs(1), events.recv()).await
        {
            match envelope.event {
                UiEvent::BrowserActionRequested {
                    sandbox_id,
                    tab_id,
                    reason,
                    task_id,
                } => {
                    assert_eq!(
                        (sandbox_id.as_str(), tab_id.as_str(), reason.as_str()),
                        ("s1", tab.as_str(), "Log in to AWS")
                    );
                    assert_eq!(task_id.as_deref(), Some("t1"));
                    assert_eq!(envelope.sandbox.as_ref().map(|s| s.as_str()), Some("s1"));
                    saw_request = true;
                }
                UiEvent::TaskUpdated { task } => {
                    saw_task |=
                        task.id == "t1" && task.activity.as_deref() == Some("needs_attention");
                }
                _ => {}
            }
            if saw_request && saw_task {
                break;
            }
        }
        assert!(
            saw_request && saw_task,
            "request {saw_request}, task {saw_task}"
        );

        // The viewer gets the request and a tab list highlighting the tab (in either order).
        let (mut requested, mut highlighted) = (false, false);
        while !(requested && highlighted) {
            let message = next_json(&mut viewer).await?;
            if message["type"] == "action_requested" {
                assert_eq!(message["tab_id"], tab.as_str());
                assert_eq!(message["task_id"], "t1");
                assert_eq!(message["reason"], "Log in to AWS");
                requested = true;
            }
            if message["type"] == "tabs" {
                highlighted |= message["tabs"][0]["action_requested"]["reason"] == "Log in to AWS";
            }
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_missing_chromium_is_a_clear_error_for_viewers_and_agents() -> TestResult {
    let browsers = ChromiumBrowsers::new(browser_config(
        Path::new("/nonexistent/yulius-test/chromium"),
        Path::new("/nonexistent/yulius-test/profiles"),
    ));
    let h = harness_with(HarnessOptions {
        browser: Some(Arc::new(browsers)),
        ..HarnessOptions::default()
    })?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;

    let result = mcp_tool(&h, TOKEN, "browser_tabs", json!({})).await?;
    assert_eq!(result["isError"], true);
    assert!(
        tool_text(&result)
            .as_str()
            .is_some_and(|t| t.contains("Chromium executable not found")),
        "{result}"
    );

    let served = serve(h.router.clone()).await?;
    let mut socket = ws_connect(served.addr, "/api/sandboxes/s1/browser", &writer, ORIGIN).await?;
    let error = next_json(&mut socket).await?;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "upstream");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("Chromium executable not found"))
    );
    let closed = tokio::time::timeout(WAIT, socket.next()).await?;
    assert!(
        matches!(closed, Some(Ok(Message::Close(_))) | None),
        "{closed:?}"
    );
    Ok(())
}
