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

use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::{
    Harness, HarnessOptions, TestResult, call, harness_with, mcp_tool, perms, seed_github_user,
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
use yulius_domain::ids::{SandboxId, TaskId};
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
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
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

/// SPEC-206 RMV-003 (#624): removing a sandbox stops its browser at once (it waited for the
/// 30-minute idle reaper); another sandbox's browser goes on.
#[tokio::test]
async fn removing_the_sandbox_stops_its_browser() -> TestResult {
    with_fixture(|f| async move {
        f.h.github.set_perm("alice", "acme/fixture", perms("admin"));
        let data = f.h.app.config.data_dir.clone();
        let mut pids = Vec::new();
        for sandbox in ["s1", "s2"] {
            let profile = data.join("sandboxes").join(sandbox).join("browser");
            f.h.app
                .deps
                .browser
                .ensure(&SandboxId::parse(sandbox)?, &profile)
                .await
                .map_err(|e| format!("{e:?}"))?;
            pids.push(
                std::fs::read_to_string(profile.join(FAKE_PID_FILE))?
                    .trim()
                    .parse::<i32>()?,
            );
        }

        let removed = call(
            &f.h.router,
            Method::DELETE,
            "/api/sandboxes/s1",
            None,
            &f.writer,
        )
        .await?;
        assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);

        assert!(
            wait_dead(pids[0]).await,
            "the removed sandbox's browser is stopped"
        );
        assert!(pid_alive(pids[1]), "the other sandbox's browser goes on");
        assert!(
            f.h.app
                .deps
                .browser
                .running(&SandboxId::parse("s2")?)
                .await
                .is_some()
        );
        Ok(())
    })
    .await
}

/// SPEC-206 RMV-003 (review of #193): a task of a removed sandbox still stopping can call the
/// browser tools; they start no browser for the removed sandbox (it lived until the 30-minute
/// reaper).
#[tokio::test]
async fn a_removed_sandbox_gets_no_browser_from_its_stopping_task() -> TestResult {
    with_fixture(|f| async move {
        f.h.github.set_perm("alice", "acme/fixture", perms("admin"));
        let removed = call(
            &f.h.router,
            Method::DELETE,
            "/api/sandboxes/s1",
            None,
            &f.writer,
        )
        .await?;
        assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);

        let opened = mcp_tool(
            &f.h,
            TOKEN,
            "browser_open",
            json!({"url": "https://docs.example"}),
        )
        .await?;
        assert_eq!(opened["isError"], true, "{opened}");
        assert!(
            f.h.app
                .deps
                .browser
                .running(&SandboxId::parse("s1")?)
                .await
                .is_none(),
            "no browser of the removed sandbox"
        );
        Ok(())
    })
    .await
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

        // SPEC-483 E3: the tab went on to another site and Yulius has not heard of it yet (it
        // still lists the bound host's page). The page is asked where the typing begins:
        // nothing is typed, and the answer names where the tab is.
        mcp_tool(
            &f.h,
            TOKEN,
            "browser_navigate",
            json!({"tab_id": tab, "url": "https://docs.example/login"}),
        )
        .await?;
        f.fake
            .move_target_unannounced(&tab, "https://evil.example/collect");
        let listed = tool_text(&mcp_tool(&f.h, TOKEN, "browser_tabs", json!({})).await?);
        assert!(
            listed.to_string().contains("https://docs.example/login"),
            "Yulius still lists the page it knew: {listed}"
        );
        let moved = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN", "selector": "#password"}),
        )
        .await?;
        assert_eq!(moved["isError"], true, "{moved}");
        assert!(
            tool_text(&moved).as_str().is_some_and(|t| {
                t.contains("nothing was typed") && t.contains("evil.example") && !t.contains(VALUE)
            }),
            "{moved}"
        );
        assert_eq!(f.fake.calls_for("Input.insertText").len(), before);

        // The page goes on to another site by itself while the secret is typed: the tool says
        // where it went instead of `filled`, without the value, and the secret's activity
        // keeps it for its people.
        mcp_tool(
            &f.h,
            TOKEN,
            "browser_navigate",
            json!({"tab_id": tab, "url": "https://docs.example/login"}),
        )
        .await?;
        f.fake.navigate_to_during_nth(
            "Input.insertText",
            before + 1,
            "https://evil.example/collect",
        );
        let astray = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN", "selector": "#password"}),
        )
        .await?;
        assert_eq!(astray["isError"], true, "{astray}");
        assert!(
            tool_text(&astray).as_str().is_some_and(|t| {
                t.contains("went to evil.example while the secret was typed") && !t.contains(VALUE)
            }),
            "{astray}"
        );
        let activity = support::call(
            &f.h.router,
            axum::http::Method::GET,
            "/api/secrets/activity",
            None,
            &f.writer,
        )
        .await?;
        let recorded = activity.body.as_array().is_some_and(|entries| {
            entries.iter().any(|e| {
                e["action"] == "vault.agent.fill"
                    && e["detail"].as_str().is_some_and(|d| {
                        d.contains("docs.example") && d.contains("evil.example") && !d.contains(VALUE)
                    })
            })
        });
        assert!(recorded, "{}", activity.body);
        let before = f.fake.calls_for("Input.insertText").len();
        mcp_tool(
            &f.h,
            TOKEN,
            "browser_navigate",
            json!({"tab_id": tab, "url": "https://evil.example/login"}),
        )
        .await?;
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

/// SPEC-483 E8 and E9: the page rule also covers where the keys can end up besides the tab's
/// own document: a frame of another site with the focus, and the page a navigation under way
/// is bringing. Both answer as a page the secret is not for, naming the site, with no key sent.
/// E10: so for the secret's user name (and its one-time code, typed the same way), which was
/// typed on what Yulius had last heard of the tab.
#[tokio::test]
async fn a_secret_is_typed_neither_into_another_sites_frame_nor_a_tab_that_is_leaving() -> TestResult
{
    const VALUE: &str = "docs-pass-8842-qrs";
    const USER: &str = "alice-docs-8842";
    with_fixture(|f| async move {
        let created = support::call(
            &f.h.router,
            axum::http::Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(
                json!({"name": "DOCS_LOGIN", "hosts": ["docs.example"], "injection": "basic",
                "username": USER, "value": VALUE}),
            ),
            &f.writer,
        )
        .await?;
        assert_eq!(
            created.status,
            axum::http::StatusCode::CREATED,
            "{}",
            created.body
        );
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
        let fill = |field: &'static str| {
            mcp_tool(
                &f.h,
                TOKEN,
                "browser_fill_secret",
                json!({"tab_id": tab, "secret": "DOCS_LOGIN", "field": field}),
            )
        };
        let typed = || f.fake.calls_for("Input.insertText").len();
        let before = typed();
        // Events arrive in order: once a title set after an announcement is listed, Yulius
        // heard the announcement.
        let heard = |title: &'static str| {
            let (f, tab) = (&f, &tab);
            async move {
                f.fake.set_target_title(tab, title).await;
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let tabs = mcp_tool(&f.h, TOKEN, "browser_tabs", json!({})).await?;
                        if tool_text(&tabs).to_string().contains(title) {
                            return Ok::<_, Box<dyn std::error::Error>>(());
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await?
            }
        };

        // The focus is in a frame of a site the secret is not bound to.
        f.fake.set_child_frames(&["https://evil.example/collect"]);
        f.fake
            .set_secret_probe(Some(json!({"shown": ["Fake page text"], "masked": [],
            "over": false, "focus": {"kind": "frame"}})));
        for field in ["value", "username"] {
            let framed = fill(field).await?;
            assert_eq!(framed["isError"], true, "{field}: {framed}");
            assert!(
                tool_text(&framed).as_str().is_some_and(|t| {
                    t.contains("nothing was typed")
                        && t.contains("a frame of evil.example")
                        && !t.contains(VALUE)
                        && !t.contains(USER)
                }),
                "{field}: {framed}"
            );
        }
        assert_eq!(typed(), before);

        // The tab set out for such a site (a link clicked just before) and has not arrived.
        f.fake.set_child_frames(&[]);
        f.fake.set_secret_probe(None);
        f.fake
            .emit(
                &tab,
                "Page.frameStartedNavigating",
                json!({"frameId": "frame-1", "loaderId": "loader-next",
                    "navigationType": "differentDocument", "url": "https://evil.example/next"}),
            )
            .await;
        heard("leaving").await?;
        for field in ["value", "username"] {
            let leaving = fill(field).await?;
            assert_eq!(leaving["isError"], true, "{field}: {leaving}");
            assert!(
                tool_text(&leaving).as_str().is_some_and(|t| {
                    t.contains("nothing was typed")
                        && t.contains("on its way to evil.example")
                        && !t.contains(VALUE)
                }),
                "{field}: {leaving}"
            );
        }
        assert_eq!(typed(), before);

        // Chrome gave the navigation up (the server never answered): the page is the bound
        // one again, and the secret and its user name are typed.
        f.fake
            .emit(
                &tab,
                "Page.frameStoppedLoading",
                json!({"frameId": "frame-1"}),
            )
            .await;
        heard("stayed").await?;
        for field in ["value", "username"] {
            let filled = fill(field).await?;
            assert_ne!(filled["isError"], true, "{field}: {filled}");
        }
        let texts: Vec<String> = f
            .fake
            .calls_for("Input.insertText")
            .iter()
            .filter_map(|c| c.params["text"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(texts[before..], [VALUE.to_owned(), USER.to_owned()]);

        // The focus went into another site's frame while the user name was typed: said, and
        // kept in the secret's activity, as for the secret itself.
        let probe = |kind: &str| {
            json!({"shown": ["Fake page text"], "masked": [], "over": false,
                "focus": {"kind": kind}})
        };
        f.fake.set_child_frames(&["https://evil.example/collect"]);
        // Three looks: where the focus is, what the typing may edit, where the focus ended.
        f.fake
            .queue_secret_probes(vec![probe("field"), probe("field"), probe("frame")]);
        let astray = fill("username").await?;
        assert_eq!(astray["isError"], true, "{astray}");
        assert!(
            tool_text(&astray).as_str().is_some_and(|t| {
                t.contains("after the secret was typed the focus is not in a field of this page")
                    && t.contains("a frame of evil.example")
                    && !t.contains(USER)
            }),
            "{astray}"
        );
        let activity = support::call(
            &f.h.router,
            axum::http::Method::GET,
            "/api/secrets/activity",
            None,
            &f.writer,
        )
        .await?;
        let recorded = activity.body.as_array().is_some_and(|entries| {
            entries.iter().any(|e| {
                e["action"] == "vault.agent.fill"
                    && e["detail"].as_str().is_some_and(|d| {
                        d.contains("username on docs.example")
                            && d.contains("a frame of evil.example")
                    })
            })
        });
        assert!(recorded, "{}", activity.body);
        f.fake.set_child_frames(&[]);

        // The tab went on to another site and Yulius has not heard of it yet (it still lists
        // the bound host's page): the user name is not typed there either.
        let typed_so_far = typed();
        f.fake
            .move_target_unannounced(&tab, "https://evil.example/collect");
        let moved = fill("username").await?;
        assert_eq!(moved["isError"], true, "{moved}");
        assert!(
            tool_text(&moved).as_str().is_some_and(|t| {
                t.contains("nothing was typed") && t.contains("evil.example") && !t.contains(USER)
            }),
            "{moved}"
        );
        assert_eq!(typed(), typed_so_far);
        Ok(())
    })
    .await
}

/// SPEC-450 (#1113): pixels cannot be redacted, so a screenshot of a page that may show a secret
/// typed with `browser_fill_secret` is refused, pointing to `browser_snapshot`: a plain-text
/// field (and from then on every page of its origin), an edited value, a page in another tab
/// echoing the key, a page showing any agent secret, a page that cannot be read. A password field
/// keeps screenshots until the page reveals it, another origin is captured, and a user name is
/// not a secret.
#[tokio::test]
async fn a_screenshot_never_shows_a_secret_typed_into_a_page() -> TestResult {
    const VALUE: &str = "docs-api-key-5521-qrs";
    const STORED: &str = "panel-token-3391-zz";
    with_fixture(|f| async move {
        for secret in [
            json!({"name": "DOCS_KEY", "description": "Docs API key",
                "hosts": ["docs.example", "login.example"], "injection": "basic",
                "username": "alice-docs", "value": VALUE}),
            json!({"name": "PANEL_TOKEN", "value": STORED}),
        ] {
            let created = support::call(
                &f.h.router,
                axum::http::Method::POST,
                "/api/sandboxes/s1/agent-secrets",
                Some(secret),
                &f.writer,
            )
            .await?;
            assert_eq!(
                created.status,
                axum::http::StatusCode::CREATED,
                "{}",
                created.body
            );
        }
        let opened = mcp_tool(
            &f.h,
            TOKEN,
            "browser_open",
            json!({"url": "https://docs.example/settings"}),
        )
        .await?;
        let tab = tool_text(&opened)["tab"]["id"]
            .as_str()
            .ok_or("tab id")?
            .to_owned();
        let screenshot = || mcp_tool(&f.h, TOKEN, "browser_screenshot", json!({"tab_id": tab}));
        let navigate = |url: &'static str| {
            mcp_tool(
                &f.h,
                TOKEN,
                "browser_navigate",
                json!({"tab_id": tab, "url": url}),
            )
        };
        let refused = |shot: &Value| {
            shot["isError"] == true
                && shot["content"][0]["type"] == "text"
                && tool_text(shot)
                    .as_str()
                    .is_some_and(|t| t.contains("browser_snapshot"))
        };
        let captured =
            |shot: &Value| shot["isError"] == false && shot["content"][0]["type"] == "image";
        let fill_field = |selector: &'static str, field: &'static str| {
            mcp_tool(
                &f.h,
                TOKEN,
                "browser_fill_secret",
                json!({"tab_id": tab, "secret": "DOCS_KEY", "selector": selector,
                    "field": field}),
            )
        };
        let fill = |selector: &'static str| fill_field(selector, "value");

        // A plain-text field (an API-key input) shows the value: no pixels.
        let filled = fill("#api-key").await?;
        assert_eq!(filled["isError"], false, "{filled}");
        let shot = screenshot().await?;
        assert!(refused(&shot), "{shot}");
        assert!(!shot.to_string().contains(VALUE), "{shot}");
        // What the agent types itself is its own: browser_type guards nothing, the key stays.
        let typed = mcp_tool(
            &f.h,
            TOKEN,
            "browser_type",
            json!({"tab_id": tab, "text": "visible note", "selector": "#note"}),
        )
        .await?;
        assert_eq!(typed["isError"], false, "{typed}");
        assert!(refused(&screenshot().await?), "the key is still shown");
        // A character typed into the middle of it: no longer the value as typed, still refused.
        f.fake.set_secret_probe(Some(
            json!({"shown": ["docs-api-key- 5521-qrs"], "masked": [], "over": false}),
        ));
        assert!(refused(&screenshot().await?), "an edited key");
        f.fake.set_secret_probe(None);
        // Every page of that origin from now on (a form re-rendered with an edited copy).
        navigate("https://docs.example/login").await?;
        assert!(refused(&screenshot().await?), "the origin of the fill");

        // Another origin is captured; a user name is not a secret.
        navigate("https://login.example/").await?;
        assert!(captured(&screenshot().await?));
        let user = fill_field("#user", "username").await?;
        assert_eq!(user["isError"], false, "{user}");
        assert!(captured(&screenshot().await?), "a user name");

        // Another tab showing the key (a settings page), or any agent secret, is refused.
        let other = tool_text(
            &mcp_tool(
                &f.h,
                TOKEN,
                "browser_open",
                json!({"url": "https://status.example/keys"}),
            )
            .await?,
        )["tab"]["id"]
            .as_str()
            .ok_or("tab id")?
            .to_owned();
        let other_shot = || mcp_tool(&f.h, TOKEN, "browser_screenshot", json!({"tab_id": other}));
        assert!(captured(&other_shot().await?));
        f.fake.set_secret_probe(Some(
            json!({"shown": [format!("Token: {STORED}")], "masked": [], "over": false}),
        ));
        assert!(
            refused(&other_shot().await?),
            "a page showing a stored secret"
        );
        f.fake.set_secret_probe(Some(
            json!({"shown": [format!("Your key: {VALUE}")], "masked": [], "over": false}),
        ));
        assert!(refused(&other_shot().await?), "a page echoing the key");
        f.fake.set_secret_probe(None);

        // A password field is drawn as dots: screenshots keep working, until the page reveals it.
        f.fake.set_password_field("#password", true);
        let filled = fill("#password").await?;
        assert_eq!(filled["isError"], false, "{filled}");
        assert!(captured(&screenshot().await?), "a password field");
        f.fake.set_password_field("#password", false);
        assert!(
            refused(&screenshot().await?),
            "a revealed password is shown"
        );
        f.fake.set_password_field("#password", true);

        // A page that cannot be read is not captured (fail closed).
        navigate("https://other.example/").await?;
        assert!(captured(&screenshot().await?));
        f.fake.set_secret_probe(Some(json!("not an answer")));
        assert!(refused(&screenshot().await?), "an unreadable page");
        f.fake.set_secret_probe(None);
        assert!(captured(&screenshot().await?));
        Ok(())
    })
    .await
}

/// SPEC-450 SSC-004: when the agent secrets cannot be loaded to check the page, no screenshot is
/// taken (as a result that could not be checked is withheld).
#[tokio::test]
async fn a_screenshot_is_refused_when_the_secrets_cannot_be_checked() -> TestResult {
    use yulius_app::ports::vault::VaultStore as _;
    with_fixture(|f| async move {
        let created = support::call(
            &f.h.router,
            axum::http::Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(json!({"name": "DOCS_KEY", "hosts": ["docs.example"],
                "value": "docs-api-key-5521-qrs"})),
            &f.writer,
        )
        .await?;
        assert_eq!(
            created.status,
            axum::http::StatusCode::CREATED,
            "{}",
            created.body
        );
        // Its sealed payload no longer opens (a key-encryption key nobody has).
        let sandbox = SandboxId::parse("s1")?;
        let records = f.h.store.vault_agent_in(&sandbox).await?;
        let seen = records
            .iter()
            .find(|r| r.name == "DOCS_KEY")
            .ok_or("no secret")?
            .clone();
        let mut broken = seen.clone();
        broken.envelope.kek_id = 9_999;
        assert!(f.h.store.vault_update(&broken, &seen).await?);

        mcp_tool(
            &f.h,
            TOKEN,
            "browser_open",
            json!({"url": "https://docs.example/"}),
        )
        .await?;
        let browser =
            f.h.app
                .browser_session(&sandbox)
                .await
                .map_err(|e| e.to_string())?;
        let tab = browser.tabs().last().ok_or("no tab")?.id.clone();
        let shot = mcp_tool(
            &f.h,
            TOKEN,
            "browser_screenshot",
            json!({"tab_id": tab.as_str()}),
        )
        .await?;
        assert_eq!(shot["isError"], true, "{shot}");
        assert!(
            tool_text(&shot)
                .as_str()
                .is_some_and(|t| t.contains("could not check the page for agent secrets")),
            "{shot}"
        );
        Ok(())
    })
    .await
}

/// SPEC-405 VEV-001 (#710): a production secret (`cd_prod`) is never typed into a page: the
/// refusal is for what it is, before any host is looked at (it has none, ADR-0324).
#[tokio::test]
async fn a_production_secret_is_never_typed_into_a_page() -> TestResult {
    const VALUE: &str = "prod-pass-5512-qrs";
    with_fixture(|f| async move {
        let created = support::call(
            &f.h.router,
            axum::http::Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(
                json!({"name": "PROD_LOGIN", "hosts": ["docs.example"], "value": VALUE,
                "auto_allow": false}),
            ),
            &f.writer,
        )
        .await?;
        assert_eq!(
            created.status,
            axum::http::StatusCode::CREATED,
            "{}",
            created.body
        );
        let id = created.body["id"].as_str().ok_or("id")?;
        support::production_secret(&f.h.store, id).await?;

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
        let before = f.fake.calls_for("Input.insertText").len();
        let refused = mcp_tool(
            &f.h,
            TOKEN,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "PROD_LOGIN", "selector": "#password"}),
        )
        .await?;
        assert_eq!(refused["isError"], true, "{refused}");
        assert!(
            tool_text(&refused)
                .as_str()
                .is_some_and(|t| t.contains("production secret")),
            "{refused}"
        );
        assert!(!refused.to_string().contains(VALUE));
        assert_eq!(f.fake.calls_for("Input.insertText").len(), before);
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
        // The task says it too, with the tab: a page opened later hears no event, and reads
        // the tabs only with its Browser panel open (SPEC-472 ATN-004).
        assert_eq!(
            support::attention(&f.h.app, "t1").await?,
            json!({
                "kind": "yulius",
                "problem": "The agent asks you to act in the sandbox's browser: Log in to AWS",
                "action": "Open the browser tab and do what it asks.",
                "link": body["link"],
            })
        );
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

/// SPEC-483 E1 (ADR-0318): on a virtual display a viewer's clicks and keys are real input that
/// reaches Chrome's own windows (its file chooser, DevTools), which run on the host. They are
/// refused (a button press, a key press, typed text), with what an administrator runs, until
/// the browser itself answers that the host's managed policy is in force; moving the pointer
/// and releases pass; the probe's tab is nobody's tab; and once the policy appears input is
/// taken without a restart.
#[tokio::test]
async fn viewer_clicks_and_keys_wait_for_the_hosts_browser_policy() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let display = display_of(&f)?;
        f.fake.set_browser_policy(false);
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let tab = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        let down = json!({"type": "mouse", "tab_id": tab, "event": "down", "x": 10, "y": 20, "button": "left"});
        let key = json!({"type": "key", "tab_id": tab, "event": "down", "key": "a", "code": "KeyA", "modifiers": 0});
        // Typed text (an IME, a paste) is input too.
        let text = json!({"type": "key", "tab_id": tab, "event": "char", "text": "ab"});

        for refused in [&down, &key, &text] {
            send(&mut socket, refused.clone()).await?;
            let error = loop {
                let message = next_json(&mut socket).await?;
                assert!(
                    message["tabs"].as_array().is_none_or(|tabs| tabs.len() == 1),
                    "the probe's tab was listed: {message}"
                );
                if message["type"] == "error" {
                    break message;
                }
            };
            assert_eq!(error["code"], "input_locked", "{error}");
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("sudo yulius host setup")),
                "{error}"
            );
        }
        // Releases pass (nothing may stay held), and so does moving the pointer.
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "up", "x": 10, "y": 20, "button": "left"})).await?;
        send(&mut socket, json!({"type": "key", "tab_id": tab, "event": "up", "key": "a", "code": "KeyA"})).await?;
        send(&mut socket, json!({"type": "mouse", "tab_id": tab, "event": "move", "x": 30, "y": 40})).await?;
        assert!(display.wait_for_input(|i| *i == DisplayInput::Motion { x: 30, y: 40 }, WAIT).await);
        assert!(
            !display.inputs().iter().any(|i| matches!(
                i,
                DisplayInput::Button { press: true, .. } | DisplayInput::Key { press: true, .. }
            )),
            "{:?}",
            display.inputs()
        );
        assert!(
            display
                .inputs()
                .contains(&DisplayInput::Button { button: 1, press: false }),
            "{:?}",
            display.inputs()
        );
        // One probe for the three refusals (the answer stands for a moment), its tab closed.
        assert_eq!(f.fake.calls_for("Target.createTarget").len(), 1);
        assert_eq!(f.fake.target_ids().len(), 1);

        // An administrator ran host setup: the running browser takes input.
        f.fake.set_browser_policy(true);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            send(&mut socket, down.clone()).await?;
            let pressed = |i: &DisplayInput| *i == DisplayInput::Button { button: 1, press: true };
            if display.wait_for_input(pressed, Duration::from_millis(500)).await {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "input stayed off");
        }
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

/// ADR-0318 (review of slice 6): the policy probe's tab is known by the id Chrome gave it, never
/// by its address. A page that opens a window at the probe's address gets an ordinary tab: it
/// is listed and, on the display, becomes the shown tab, so the panel never shows one tab's
/// address over another window's picture.
#[tokio::test]
async fn a_page_cannot_hide_a_window_behind_the_probes_address() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        // A probe ran (a click asked for it); then a page opens its look-alike.
        send(&mut socket, json!({"type": "mouse", "tab_id": first, "event": "down", "x": 10, "y": 20, "button": "left"})).await?;
        assert!(f.fake.wait_for_calls("Target.closeTarget", 1, WAIT).await);
        let popup = f
            .fake
            .open_from_page(&first, "about:blank#yulius-browser-policy")
            .await;
        let shown = wait_json(&mut socket, |m| {
            m["type"] == "tabs"
                && m["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter()
                        .any(|t| t["id"] == popup.as_str() && t["active"] == true)
                })
        })
        .await?;
        assert_eq!(shown["tabs"].as_array().map(Vec::len), Some(2), "{shown}");
        Ok(())
    })
    .await
}

/// ADR-0318 (review of slice 6, round 2): what Chrome announces while a probe waits for the id
/// of its own tab is listed once the id is known, unless it is the probe's; and a tab that
/// goes to a page of Chrome's own stays a tab (Chrome still shows it).
#[tokio::test]
async fn tabs_made_while_the_probe_waits_are_listed_and_a_tab_at_chromes_page_stays() -> TestResult
{
    with_display(FakeDisplay::new(), |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        // The other tab is made and goes on to a second address while the probe waits: it is
        // listed where it went, not where it started.
        f.fake.create_alongside_next_target(
            &["https://beside.example/", "https://beside.example/moved"],
            false,
        );
        // A press asks for the probe, whose tab and the other are announced before its id.
        send(&mut socket, json!({"type": "mouse", "tab_id": first, "event": "down", "x": 10, "y": 20, "button": "left"})).await?;
        let listed = wait_json(&mut socket, |m| {
            m["type"] == "tabs"
                && m["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter()
                        .any(|t| t["url"] == "https://beside.example/moved")
                })
        })
        .await?;
        let urls: Vec<&str> = listed["tabs"]
            .as_array()
            .map(|tabs| tabs.iter().filter_map(|t| t["url"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(urls.len(), 2, "the probe's tab was listed: {urls:?}");
        assert!(f.fake.wait_for_calls("Target.closeTarget", 1, WAIT).await);

        f.fake.set_target_url(&first, "chrome://settings/").await;
        let stayed = wait_json(&mut socket, |m| {
            m["type"] == "tabs"
                && m["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter()
                        .any(|t| t["id"] == first.as_str() && t["url"] == "chrome://settings/")
                })
        })
        .await?;
        assert_eq!(stayed["tabs"].as_array().map(Vec::len), Some(2), "{stayed}");
        Ok(())
    })
    .await
}

/// ADR-0318 (review of slice 6, round 3): a page made and closed again while a probe waits for
/// the id of its own tab is no tab afterwards.
#[tokio::test]
async fn a_tab_made_and_closed_while_the_probe_waits_is_no_tab() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        f.fake
            .create_alongside_next_target(&["https://gone.example/"], true);
        send(&mut socket, json!({"type": "mouse", "tab_id": first, "event": "down", "x": 10, "y": 20, "button": "left"})).await?;
        // The press arrives once the probe is over, its tab closed.
        assert!(display_of(&f)?.wait_for_input(|i| *i == DisplayInput::Button { button: 1, press: true }, WAIT).await);
        let mut again =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let now = next_json(&mut again).await?;
        assert_eq!(now["tabs"].as_array().map(Vec::len), Some(1), "{now}");
        assert_eq!(now["tabs"][0]["id"], first.as_str(), "{now}");
        Ok(())
    })
    .await
}

/// ADR-0318: a page of Chrome itself is no tab (Chrome 157 reports its address bar's popup,
/// `chrome://omnibox-popup.top-chrome/`, as a page): viewers are never shown it, and it never
/// becomes the shown tab; the page opened after it is.
#[tokio::test]
async fn chromes_own_pages_are_no_tabs() -> TestResult {
    with_display(FakeDisplay::new(), |f| async move {
        let served = serve(f.h.router.clone()).await?;
        let mut socket =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.writer, ORIGIN).await?;
        let tabs = next_json(&mut socket).await?;
        let first = tabs["tabs"][0]["id"].as_str().ok_or("no tab")?.to_owned();
        f.fake
            .open_from_page(&first, "chrome://omnibox-popup.top-chrome/")
            .await;
        let popup = f
            .fake
            .open_from_page(&first, "https://accounts.example/oauth")
            .await;
        // Every tab list on the way to the real popup's has no page of Chrome's.
        let shown = loop {
            let message = next_json(&mut socket).await?;
            let Some(tabs) = message["tabs"].as_array() else {
                continue;
            };
            assert!(
                tabs.iter()
                    .all(|t| !t["url"].as_str().unwrap_or_default().starts_with("chrome:")),
                "{message}"
            );
            if tabs.iter().any(|t| t["id"] == popup.as_str()) {
                break message;
            }
        };
        assert_eq!(shown["tabs"].as_array().map(Vec::len), Some(2), "{shown}");
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

/// An `Authorization` header of another scheme is refused, never taken as absent: the request
/// does not fall back to the session cookie it also carries (SPEC-483 slice 29, review).
#[tokio::test]
async fn another_authorization_scheme_beside_a_session_cookie_is_refused() -> TestResult {
    let h = support::harness()?;
    let creds = support::setup_superadmin_creds(&h).await?;
    let (status, _, _) = get_raw(&h.router, "/api/me", &creds, &[]).await?;
    assert_eq!(status, 200);
    let (status, _, _) = get_raw(
        &h.router,
        "/api/me",
        &creds,
        &[("authorization", "Basic eDp5")],
    )
    .await?;
    assert_eq!(status, 401);
    Ok(())
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
    // Its refresh draws the same list (one writer since SPEC-483 slice 29).
    let (_, _, ports) = get_raw(
        &h.internal,
        "/browser/welcome/ports?sandbox=s9",
        &anonymous,
        &[],
    )
    .await?;
    assert!(String::from_utf8(ports)?.starts_with("<p class=\"muted\">No dev server is listening"));
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
