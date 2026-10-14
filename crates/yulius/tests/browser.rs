//! The shared sandbox browser end to end: the viewer WebSocket, agent MCP tools and the
//! user-action hand-off, the display's H.264 video with real input and its JPEG fallback, copy,
//! the shared viewport and the agent's cursor, over the real Chromium manager with a fake
//! Chromium launcher, an in-process fake DevTools server and a fake virtual display that records
//! input and streams synthetic frames (SPEC-001 BRW-001..008, AC-BRW-1..4, AC-BRW-6, AC-BRW-9..11;
//! SPEC-002 ADR-15).

use crate::support;

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
    FAKE_ARGV_FILE, FAKE_AVCC, FAKE_PID_FILE, FakeCdpServer, FakeChromium, FakeDisplay,
    write_fake_chromium,
};
use yulius_browser::{BrowserConfig, DisplayInput, StopTimeouts};
use yulius_contracts::browser::{decode_frame, decode_video_frame};
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
    display: Option<FakeDisplay>,
    writer: support::Creds,
    reader: support::Creds,
    _scripts: tempfile::TempDir,
}

/// Users alice (write) and bob (read) on sandboxes `s1` (`fixture`) and `s2` (`other`); task
/// `t1` of `s1` runs with a live MCP token. Browsers are headless unless `display` is given.
async fn fixture_with(display: Option<FakeDisplay>) -> Result<Fixture, Box<dyn std::error::Error>> {
    let fake = FakeCdpServer::start().await?;
    let scripts = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        scripts.path(),
        "fake-chromium",
        FakeChromium::Ready { port: fake.port() },
    )?;
    let mut config = browser_config(&exe, &scripts.path().join("unused"));
    if let Some(display) = &display {
        config.display = display.mode();
    }
    let browsers = ChromiumBrowsers::new(config);
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
        display,
        writer,
        reader,
        _scripts: scripts,
    })
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with(None).await
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

/// [`with_fixture`] with browsers on a fake virtual display.
async fn with_display<F>(display: FakeDisplay, body: impl FnOnce(Arc<Fixture>) -> F) -> TestResult
where
    F: std::future::Future<Output = TestResult>,
{
    let f = Arc::new(fixture_with(Some(display)).await?);
    let result = body(Arc::clone(&f)).await;
    f.h.app.stop_workspace_services().await;
    result
}

/// The fixture's fake display.
fn display_of(f: &Fixture) -> Result<&FakeDisplay, Box<dyn std::error::Error>> {
    Ok(f.display.as_ref().ok_or("no display")?)
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

        // A write user gets the tab list; a headless browser has no video, so the viewer is
        // told why and gets JPEG frames of the active tab (the labelled fallback).
        let mut socket = ws_connect(served.addr, path, &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        assert_eq!(tabs["type"], "tabs");
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        let stream = wait_json(&mut socket, |m| m["type"] == "stream").await?;
        assert_eq!(stream["mode"], "pixels");
        assert!(
            stream["reason"].as_str().is_some_and(|r| r.contains("headless")),
            "{stream}"
        );
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
        send(&mut socket, json!({"type": "key", "tab_id": new_tab, "event": "down", "key": "h", "code": "KeyH"})).await?;
        // Text without a key (a paste) is inserted: a key event carries at most four characters.
        send(&mut socket, json!({"type": "key", "tab_id": new_tab, "event": "char", "text": "hi there"})).await?;
        assert!(f.fake.wait_for_calls("Input.dispatchMouseEvent", 1, WAIT).await);
        assert!(f.fake.wait_for_calls("Input.dispatchKeyEvent", 1, WAIT).await);
        assert!(f.fake.wait_for_calls("Input.insertText", 1, WAIT).await);
        let key = &f.fake.calls_for("Input.dispatchKeyEvent")[0].params;
        assert_eq!((key["type"].clone(), key["text"].clone()), (json!("keyDown"), json!("h")), "a printable key types its character");

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
        // The eight browser tools, plus typing an agent secret (SPEC-002 ADR-20).
        assert_eq!(names.len(), 9, "{names:?}");
        assert!(names.contains(&"browser_fill_secret"));

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

/// `browser_fill_secret` (SPEC-002 ADR-20, AC-VLT-6): an agent secret is typed into a page of a
/// bound host without the agent receiving it; another host's page is refused; the page text the
/// agent reads back is redacted.
#[tokio::test]
async fn agents_type_secrets_only_into_pages_of_their_hosts() -> TestResult {
    const VALUE: &str = "docs-pass-7731-xyz";
    with_fixture(|f| async move {
        let created = support::call(
            &f.h.router,
            axum::http::Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(json!({"name": "DOCS_LOGIN", "description": "Docs admin login",
                "hosts": ["docs.example"], "injection": "basic", "username": "alice-docs",
                "value": VALUE})),
            &f.writer,
        )
        .await?;
        assert_eq!(created.status, axum::http::StatusCode::CREATED, "{}", created.body);
        assert!(!created.body.to_string().contains(VALUE));

        let opened = mcp_tool(
            &f.h,
            TOKEN,
            "browser_open",
            json!({"url": "https://docs.example/login"}),
        )
        .await?;
        let tab = tool_text(&opened)["tab"]["id"]
            .as_str()
            .ok_or("tab id")?
            .to_owned();
        let filled = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN", "selector": "#password"}),
        )
        .await?;
        assert_eq!(filled["isError"], false, "{filled}");
        assert!(!filled.to_string().contains(VALUE), "the result holds no value");
        assert_eq!(tool_text(&filled)["filled"], true);
        let typed: Vec<String> = f
            .fake
            .calls_for("Input.insertText")
            .iter()
            .filter_map(|c| c.params["text"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(typed.last().map(String::as_str), Some(VALUE), "typed into the page");
        let user = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN", "field": "username"}),
        )
        .await?;
        assert_eq!(user["isError"], false, "{user}");

        // The page now shows the value (a filled field): what the agent reads is redacted.
        f.fake.set_ax_nodes(json!([
            {"nodeId": "1", "ignored": false, "role": {"type": "role", "value": "RootWebArea"},
             "name": {"type": "computedString", "value": "Login"}, "childIds": ["2"]},
            {"nodeId": "2", "parentId": "1", "ignored": false, "role": {"type": "role", "value": "textbox"},
             "name": {"type": "computedString", "value": "Password"},
             "value": {"type": "string", "value": VALUE}, "childIds": []}
        ]));
        let snapshot = mcp_tool(&f.h, TOKEN, "browser_snapshot", json!({"tab_id": tab})).await?;
        let text = snapshot.to_string();
        assert!(!text.contains(VALUE), "{text}");
        assert!(text.contains("[secret:DOCS_LOGIN]"), "{text}");

        // Another host's page is refused, with no value in the error.
        mcp_tool(
            &f.h,
            TOKEN,
            "browser_navigate",
            json!({"tab_id": tab, "url": "https://evil.example/login"}),
        )
        .await?;
        let before = f.fake.calls_for("Input.insertText").len();
        let refused = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN"}),
        )
        .await?;
        assert_eq!(refused["isError"], true);
        assert!(
            tool_text(&refused)
                .as_str()
                .is_some_and(|t| t.contains("may only be typed into pages of docs.example"))
        );
        assert_eq!(f.fake.calls_for("Input.insertText").len(), before);
        // A secret bound to no host yet (SPEC-121) is typed nowhere; the agent learns to ask.
        let unbound = support::call(
            &f.h.router,
            axum::http::Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(json!({"name": "NEW_LOGIN", "value": "new-pass-4411-abc"})),
            &f.writer,
        )
        .await?;
        assert_eq!(unbound.status, axum::http::StatusCode::CREATED, "{}", unbound.body);
        let refused = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "NEW_LOGIN"}),
        )
        .await?;
        assert_eq!(refused["isError"], true);
        assert!(
            tool_text(&refused)
                .as_str()
                .is_some_and(|t| t.contains("secret_request_hosts")),
            "{refused}"
        );
        assert_eq!(f.fake.calls_for("Input.insertText").len(), before);
        let unknown = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "NOPE"}),
        )
        .await?;
        assert_eq!(unknown["isError"], true);
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

/// A raw GET through the router: status, headers and body bytes.
async fn get_raw(
    router: &axum::Router,
    path: &str,
    creds: &support::Creds,
    headers: &[(&str, &str)],
) -> Result<(axum::http::StatusCode, axum::http::HeaderMap, Vec<u8>), Box<dyn std::error::Error>> {
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    let mut request = axum::http::Request::builder().uri(path);
    if let Some(cookie) = &creds.cookie {
        request = request.header("cookie", support::cookie_header(cookie));
    }
    if let Some(token) = &creds.bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(axum::body::Body::empty())?)
        .await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await?.to_bytes().to_vec();
    Ok((status, headers, body))
}

/// Next binary video frame, skipping JSON messages: (keyframe, payload).
async fn next_video(socket: &mut Socket) -> Result<(bool, Vec<u8>), Box<dyn std::error::Error>> {
    let frame = next_binary(socket).await?;
    let decoded = decode_video_frame(&frame)?;
    Ok((decoded.keyframe, decoded.data.to_vec()))
}

/// ADR-15: a browser on a virtual display is streamed as H.264 video: the decoder configuration
/// first, then access units from a keyframe; the viewport resizes the display and restarts the
/// encoder at the new size; a viewer that joins later starts at once from the cached group.
#[tokio::test]
async fn the_display_streams_video_from_a_keyframe_and_follows_the_viewport() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        use base64::Engine as _;
        let display = display_of(&f)?;
        let served = serve(f.h.router.clone()).await?;
        let path = "/api/sandboxes/s1/browser";
        let mut socket = ws_connect(served.addr, path, &f.writer, ORIGIN).await?;
        let stream = wait_json(&mut socket, |m| m["type"] == "stream").await?;
        assert_eq!(stream, json!({"type": "stream", "mode": "video"}));
        let config = wait_json(&mut socket, |m| m["type"] == "video_config").await?;
        assert_eq!(config["codec"], "avc1.42c01f");
        assert_eq!(
            (config["width"].clone(), config["height"].clone()),
            (json!(1280), json!(800))
        );
        let description = base64::engine::general_purpose::STANDARD
            .decode(config["description"].as_str().ok_or("description")?)?;
        assert_eq!(description, FAKE_AVCC);
        let (keyframe, data) = next_video(&mut socket).await?;
        assert!(keyframe, "the stream starts at a keyframe");
        assert!(data.starts_with(b"au-"));
        let mut deltas = 0;
        while deltas < 3 {
            let (keyframe, _) = next_video(&mut socket).await?;
            deltas += usize::from(!keyframe);
        }
        assert_eq!(display.running_encoders(), 1, "one encoder for the display");
        // No emulation on a display: the window has the size.
        assert!(
            f.fake
                .calls_for("Emulation.setDeviceMetricsOverride")
                .is_empty()
        );
        // Chrome runs on the display in kiosk mode without the automation flag.
        let profile = f.h.app.config.data_dir.join("sandboxes/s1/browser");
        let argv = std::fs::read_to_string(profile.join(FAKE_ARGV_FILE))?;
        for expected in [
            "--kiosk",
            "--disable-blink-features=AutomationControlled",
            "--window-size=1280,800",
        ] {
            assert!(
                argv.lines().any(|line| line == expected),
                "{expected} in {argv}"
            );
        }
        assert!(!argv.contains("--headless"), "{argv}");

        // A second viewer starts at once: configuration, then the cached group's keyframe.
        let mut late = ws_connect(served.addr, path, &f.writer, ORIGIN).await?;
        let config = wait_json(&mut late, |m| m["type"] == "video_config").await?;
        assert_eq!(config["width"], 1280);
        assert!(
            next_video(&mut late).await?.0,
            "the backlog starts at its keyframe"
        );
        assert_eq!(display.running_encoders(), 1, "still one encoder");
        drop(late);

        // The viewport resizes the display (device pixels) and the encoder restarts at it.
        send(
            &mut socket,
            json!({"type": "viewport", "width": 1001, "height": 700}),
        )
        .await?;
        let viewport = wait_json(&mut socket, |m| {
            m["type"] == "viewport" && m["width"] != 1280
        })
        .await?;
        assert_eq!(
            (viewport["width"].clone(), viewport["height"].clone()),
            (json!(1000), json!(700))
        );
        assert!(
            display
                .wait_for_input(
                    |i| *i
                        == DisplayInput::Resize {
                            width: 1000,
                            height: 700
                        },
                    WAIT
                )
                .await
        );
        let config = wait_json(&mut socket, |m| {
            m["type"] == "video_config" && m["width"] == 1000
        })
        .await?;
        assert_eq!(config["height"], 700);
        assert!(
            next_video(&mut socket).await?.0,
            "a new size starts at a keyframe"
        );
        assert!(
            display.encoder_sizes().contains(&(1000, 700)),
            "{:?}",
            display.encoder_sizes()
        );
        Ok(())
    })
    .await
}

/// ADR-15: a viewer's pointer and keys are real input on the display (XTest), the wheel scrolls
/// the shown tab by CSS pixels through CDP, text no key types is inserted, copy answers with the
/// selection, and everything the viewer held is released when it leaves.
#[tokio::test]
async fn viewer_input_is_real_display_input_and_copy_reaches_the_clipboard() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let display = display_of(&f)?;
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let tab = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();

        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "down", "x": 10.4, "y": 20, "button": "left"})).await?;
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "up", "x": 12, "y": 20, "button": "left"})).await?;
        assert!(display.wait_for_input(|i| *i == DisplayInput::Button { button: 1, press: false }, WAIT).await);
        // An uppercase letter without Shift held: Shift is pressed around the key.
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "down", "key": "A", "code": "KeyA", "modifiers": 0})).await?;
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "up", "key": "A", "code": "KeyA"})).await?;
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "down", "key": "Enter", "code": "Enter"})).await?;
        assert!(display.wait_for_input(|i| *i == DisplayInput::Key { keycode: 36, press: true }, WAIT).await);
        let inputs: Vec<DisplayInput> = display
            .inputs()
            .into_iter()
            .filter(|i| !matches!(i, DisplayInput::Resize { .. }))
            .collect();
        assert_eq!(
            inputs,
            [
                DisplayInput::Motion { x: 10, y: 20 },
                DisplayInput::Button { button: 1, press: true },
                DisplayInput::Motion { x: 12, y: 20 },
                DisplayInput::Button { button: 1, press: false },
                DisplayInput::Key { keycode: 50, press: true },
                DisplayInput::Key { keycode: 38, press: true },
                DisplayInput::Key { keycode: 50, press: false },
                DisplayInput::Key { keycode: 38, press: false },
                DisplayInput::Key { keycode: 36, press: true },
            ]
        );
        assert!(f.fake.calls_for("Input.dispatchMouseEvent").is_empty(), "clicks are not CDP");
        assert!(f.fake.calls_for("Input.dispatchKeyEvent").is_empty(), "keys are not CDP");

        // The wheel scrolls the shown tab by CSS pixels. A Polish letter and short text are
        // typed as real key presses (in order with the other keys); a long paste is inserted.
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "wheel", "x": 5, "y": 6, "dx": 0, "dy": 37.5})).await?;
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "down", "key": "ą", "code": "KeyA"})).await?;
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "char", "text": "zażółć 🙂\n"})).await?;
        let paste = "x".repeat(100);
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "char", "text": paste})).await?;
        assert!(f.fake.wait_for_calls("Input.insertText", 1, WAIT).await);
        // The wheel is dispatched without making the viewer socket wait (F 2026-10-10).
        assert!(f.fake.wait_for_calls("Input.dispatchMouseEvent", 1, WAIT).await);
        let wheel = f.fake.calls_for("Input.dispatchMouseEvent");
        assert_eq!(wheel[0].params["type"], "mouseWheel");
        assert_eq!(wheel[0].params["deltaY"], 37.5);
        let typed: String = display
            .inputs()
            .into_iter()
            .filter_map(|i| match i {
                DisplayInput::Type(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(typed, "ązażółć 🙂\n");
        let inserted = f.fake.calls_for("Input.insertText");
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].params["text"], paste.as_str());

        // Copy answers this viewer with the page's selection.
        f.fake.set_selection("copied text");
        send(&mut socket, json!({"type": "copy", "tab_id": tab})).await?;
        let clipboard = wait_json(&mut socket, |m| m["type"] == "clipboard").await?;
        assert_eq!(clipboard["text"], "copied text");

        // A key held when the viewer leaves is released.
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "down", "key": "Shift", "code": "ShiftLeft"})).await?;
        assert!(display.wait_for_input(|i| *i == DisplayInput::Key { keycode: 50, press: true }, WAIT).await);
        drop(socket);
        assert!(display.wait_for_input(|i| *i == DisplayInput::ReleaseAll, WAIT).await);
        Ok(())
    })
    .await
}

/// ADR-15: a page opening a page (a link to a new tab, an OAuth popup) brings it to the front of
/// the display, so the panel shows it as the active tab; watching a tab brings it to the front
/// for every viewer.
#[tokio::test]
async fn pages_opened_by_pages_and_watched_tabs_come_to_the_front() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        let popup = f
            .fake
            .open_from_page(&first, "https://accounts.example/oauth")
            .await;
        let shown = wait_json(&mut socket, |m| {
            m["type"] == "tabs"
                && m["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter()
                        .any(|t| t["id"] == popup.as_str() && t["active"] == true)
                })
        })
        .await?;
        assert_eq!(shown["tabs"].as_array().map(Vec::len), Some(2));
        send(&mut socket, json!({"type": "watch", "tab_id": first})).await?;
        assert!(
            f.fake
                .wait_for_calls("Target.activateTarget", 1, WAIT)
                .await
        );
        assert_eq!(
            f.fake.calls_for("Target.activateTarget")[0].params["targetId"],
            first.as_str()
        );
        // Back, forward beyond the end (nothing) and reload.
        send(
            &mut socket,
            json!({"type": "history", "tab_id": first, "delta": -1}),
        )
        .await?;
        send(
            &mut socket,
            json!({"type": "history", "tab_id": first, "delta": 1}),
        )
        .await?;
        send(&mut socket, json!({"type": "reload", "tab_id": first})).await?;
        assert!(f.fake.wait_for_calls("Page.reload", 1, WAIT).await);
        let entries = f.fake.calls_for("Page.navigateToHistoryEntry");
        assert_eq!(
            entries.len(),
            1,
            "forward past the newest entry does nothing"
        );
        assert_eq!(entries[0].params["entryId"], 7);
        // Closing the shown tab brings the next one to the front.
        send(&mut socket, json!({"type": "close", "tab_id": first})).await?;
        assert!(
            f.fake
                .wait_for_calls("Target.activateTarget", 2, WAIT)
                .await
        );
        assert_eq!(
            f.fake.calls_for("Target.activateTarget")[1].params["targetId"],
            popup.as_str()
        );
        Ok(())
    })
    .await
}

/// ADR-15: when the display's encoder cannot run, viewers are told why and get JPEG frames.
#[tokio::test]
async fn a_broken_encoder_falls_back_to_labelled_jpeg_frames() -> TestResult {
    let display = FakeDisplay::new();
    display.fail_encoder();
    with_display(display, |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        let stream = wait_json(&mut socket, |m| {
            m["type"] == "stream" && m["mode"] == "pixels"
        })
        .await?;
        assert!(
            stream["reason"]
                .as_str()
                .is_some_and(|r| r.contains("broken")),
            "{stream}"
        );
        let frame = next_binary(&mut socket).await?;
        assert_eq!(decode_frame(&frame)?.0.as_str(), first);
        // A viewer may also ask for pixels itself.
        let mut second =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        send(&mut second, json!({"type": "view", "mode": "pixels"})).await?;
        let frame = next_binary(&mut second).await?;
        assert!(decode_frame(&frame).is_ok());
        Ok(())
    })
    .await
}

/// ADR-15: the browser's display is stopped with the browser (encoder and X server reaped).
#[tokio::test]
async fn the_display_stops_with_the_browser() -> TestResult {
    let display = FakeDisplay::new();
    let observed = display.clone();
    with_display(display, |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        wait_json(&mut socket, |m| m["type"] == "video_config").await?;
        assert_eq!(observed.lifecycle(), (1, 0));
        drop(socket);
        f.h.app.stop_workspace_services().await;
        assert_eq!(observed.lifecycle(), (1, 1));
        assert_eq!(observed.running_encoders(), 0);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn the_welcome_page_names_the_sandbox_and_links_its_listening_ports() -> TestResult {
    use yulius_app::ports::store::SandboxStore as _;
    let h = support::harness()?;
    seed_github_user(&h, "u-alice", "alice").await?;
    // A sandbox whose port range starts at a port that listens (an OS-assigned one).
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let mut sandbox = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    sandbox.id = yulius_domain::ids::SandboxId::parse("s9")?;
    sandbox.slug = "welcomed".into();
    sandbox.repo_name = "welcomed".into();
    sandbox.port_base = port;
    sandbox.port_count = 1;
    h.store.insert_sandbox(&sandbox).await?;

    let anonymous = support::Creds::default();
    let (status, headers, body) =
        get_raw(&h.internal, "/browser/welcome?sandbox=s9", &anonymous, &[]).await?;
    assert_eq!(status, 200);
    assert!(headers["content-type"].to_str()?.starts_with("text/html"));
    let page = String::from_utf8(body)?;
    assert!(page.contains("Browser of welcomed"), "{page}");
    assert!(
        page.contains(&format!("href=\"http://localhost:{port}/\"")),
        "{page}"
    );
    let (_, _, ports) = get_raw(
        &h.internal,
        "/browser/welcome/ports?sandbox=s9",
        &anonymous,
        &[],
    )
    .await?;
    assert!(String::from_utf8(ports)?.contains(&format!("localhost:{port}")));

    // Without a listener the page says how to start one; unknown sandboxes are not found.
    drop(listener);
    let (_, _, body) = get_raw(&h.internal, "/browser/welcome?sandbox=s9", &anonymous, &[]).await?;
    assert!(String::from_utf8(body)?.contains("No dev server is listening"));
    let (status, _, _) = get_raw(
        &h.internal,
        "/browser/welcome?sandbox=nope",
        &anonymous,
        &[],
    )
    .await?;
    assert_eq!(status, 404);
    // The public listener does not serve it (unknown paths get the app shell).
    let (_, _, public) = get_raw(&h.router, "/browser/welcome?sandbox=s9", &anonymous, &[]).await?;
    assert!(!String::from_utf8(public)?.contains("Browser of welcomed"));
    Ok(())
}

/// A viewer that watches nothing (its panel shows no browser tab) gets no frames, even when an
/// agent opens a tab; watching a tab brings it to the front and streams it.
#[tokio::test]
async fn a_viewer_watches_the_tab_it_chose_or_none() -> TestResult {
    with_fixture(|f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        let _ = next_binary(&mut socket).await?;
        // Watching nothing stops the screencast.
        send(&mut socket, json!({"type": "watch", "tab_id": null})).await?;
        assert!(f.fake.wait_for_calls("Page.stopScreencast", 1, WAIT).await);
        // Frames already queued for this viewer may still arrive; let them drain.
        let drain = tokio::time::Instant::now() + Duration::from_millis(300);
        while let Ok(Some(message)) = tokio::time::timeout_at(drain, socket.next()).await {
            message?;
        }
        // An agent opens a tab (it becomes the shared active tab): this viewer still gets no
        // frames, only the tab list.
        let browser =
            f.h.app
                .browser_session(&yulius_domain::ids::SandboxId::parse("s1")?)
                .await?;
        let opened = browser
            .tool_open(&yulius_contracts::browser::OpenParams {
                url: "https://example.com/".into(),
            })
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        let mut saw_tabs = false;
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, socket.next()).await {
            match message? {
                Message::Binary(_) => return Err("a frame streamed while watching nothing".into()),
                Message::Text(text) => saw_tabs |= text.as_str().contains(opened.tab.id.as_str()),
                _ => {}
            }
        }
        assert!(saw_tabs, "the new tab is listed");
        // Watching the first tab brings it to the front (the agent's tab was active) and
        // streams it.
        send(&mut socket, json!({"type": "watch", "tab_id": first})).await?;
        let frame = next_binary(&mut socket).await?;
        let (tab, _) = decode_frame(&frame)?;
        assert_eq!(tab.as_str(), first);
        assert!(
            f.fake
                .calls_for("Target.activateTarget")
                .iter()
                .any(|call| call.params["targetId"] == first.as_str())
        );
        Ok(())
    })
    .await
}

/// Messages that arrive within `window`, without failing when none do.
async fn drain_for(socket: &mut Socket, window: Duration) -> Vec<Message> {
    let deadline = tokio::time::Instant::now() + window;
    let mut messages = Vec::new();
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, socket.next()).await {
        messages.push(message);
    }
    messages
}

fn errors_in(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Text(text) if text.as_str().contains("\"error\"") => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// F 2026-10-10: Chrome answers a CDP wheel event only after the renderer handled it, so wheel
/// input dispatched one by one queued up for seconds. With Chrome not answering at all, at most
/// two wheel dispatches are outstanding, the rest is summed for later, and other input (a click)
/// still goes through at once, without an error for the viewer.
#[tokio::test]
async fn a_wheel_flood_never_queues_up_behind_chrome_and_clicks_still_go_through() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let display = display_of(&f)?;
        f.fake.mute("Input.dispatchMouseEvent");
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = wait_json(&mut socket, |m| m["type"] == "tabs").await?;
        let tab = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        for _ in 0..50 {
            send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "wheel", "x": 5, "y": 6, "dx": 0, "dy": 10})).await?;
        }
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "down", "x": 30, "y": 40, "button": "left"})).await?;
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "up", "x": 30, "y": 40, "button": "left"})).await?;
        assert!(
            display
                .wait_for_input(|i| *i == DisplayInput::Button { button: 1, press: false }, Duration::from_secs(2))
                .await,
            "the click is not stuck behind the wheel"
        );
        assert!(f.fake.wait_for_calls("Input.dispatchMouseEvent", 2, WAIT).await);
        let messages = drain_for(&mut socket, Duration::from_millis(300)).await;
        let wheel = f.fake.calls_for("Input.dispatchMouseEvent");
        assert_eq!(wheel.len(), 2, "two outstanding, the rest summed: {wheel:?}");
        assert!(wheel.iter().all(|call| call.params["deltaY"] == 10.0));
        assert_eq!(errors_in(&messages), Vec::<String>::new());
        Ok(())
    })
    .await
}

/// F 2026-10-10: a flood of pointer moves is never an error for the viewer (moves beyond the
/// input budget are dropped silently and the display coalesces the rest), and a click after it
/// lands at the last position, after it.
#[tokio::test]
async fn a_flood_of_moves_is_silent_and_a_click_lands_at_the_last_position() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let display = display_of(&f)?;
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = wait_json(&mut socket, |m| m["type"] == "tabs").await?;
        let tab = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        for n in 0..1200 {
            let x = 100 + n % 900;
            send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "move", "x": x, "y": 300})).await?;
        }
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "move", "x": 777, "y": 333})).await?;
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "down", "x": 777, "y": 333, "button": "left"})).await?;
        assert!(
            display
                .wait_for_input(|i| *i == DisplayInput::Button { button: 1, press: true }, WAIT)
                .await
        );
        let inputs = display.inputs();
        let press = inputs
            .iter()
            .position(|i| *i == DisplayInput::Button { button: 1, press: true })
            .ok_or("no press")?;
        assert_eq!(inputs[press - 1], DisplayInput::Motion { x: 777, y: 333 });
        let messages = drain_for(&mut socket, Duration::from_millis(300)).await;
        assert_eq!(errors_in(&messages), Vec::<String>::new(), "moves are never an error");
        Ok(())
    })
    .await
}

/// F 2026-10-10: a viewer that acknowledges its frames gets only about a round trip of video
/// while it stops acknowledging (its path is full), falls back to a fresh keyframe the server
/// asks the encoder for, and its congestion report moves the stream to a lower bitrate tier.
#[tokio::test]
async fn an_acknowledging_viewer_is_paced_and_its_congestion_lowers_the_tier() -> TestResult {
    let display = FakeDisplay::new();
    display.set_frame_interval(Duration::from_millis(5));
    display.set_keyframe_every(u64::MAX);
    with_display(display, |f| async move {
        let display = display_of(&f)?;
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let _ = wait_json(&mut socket, |m| m["type"] == "video_config").await?;
        assert!(next_video(&mut socket).await?.0, "starts at a keyframe");
        let mut received = 1_u64;
        send(&mut socket, json!({"type": "ack", "frames": received})).await?;
        // The viewer reads but stops acknowledging for a second: 200 frames are produced, about
        // the round trip plus the slack (120 ms, some 25 frames) reach it.
        let messages = drain_for(&mut socket, Duration::from_secs(1)).await;
        let unacked = messages
            .iter()
            .filter(|m| matches!(m, Message::Binary(b) if b.first() == Some(&4)))
            .count() as u64;
        received += unacked;
        assert!(unacked < 60, "paced while unacknowledged: {unacked} frames");
        // It fell behind: the server asked the encoder for a keyframe (a restart).
        let deadline = tokio::time::Instant::now() + WAIT;
        while display.encoder_sizes().len() < 2 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            display.encoder_sizes().len() >= 2,
            "a keyframe was asked for"
        );
        // Acknowledging again resumes the stream at a keyframe; the late acknowledgements are
        // reported as congestion and the encoder restarts at a lower tier.
        send(&mut socket, json!({"type": "ack", "frames": received})).await?;
        let deadline = tokio::time::Instant::now() + WAIT;
        let mut keyframe = false;
        while tokio::time::Instant::now() < deadline {
            let frame = next_binary(&mut socket).await?;
            received += 1;
            send(&mut socket, json!({"type": "ack", "frames": received})).await?;
            keyframe |= decode_video_frame(&frame)?.keyframe;
            let lower = display.encoder_tiers().iter().any(|t| t.max_kbps < 5_000);
            if keyframe && lower {
                break;
            }
        }
        assert!(keyframe, "resumed at a keyframe");
        let tiers: Vec<u32> = display.encoder_tiers().iter().map(|t| t.max_kbps).collect();
        assert!(
            tiers.iter().any(|kbps| *kbps < 5_000),
            "a lower tier: {tiers:?}"
        );
        Ok(())
    })
    .await
}
