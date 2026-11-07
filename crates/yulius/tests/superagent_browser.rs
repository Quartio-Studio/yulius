//! The Superagents drive the shared sandbox browser (SPEC-437): the sandbox Superagent its own
//! sandbox's, the Global Superagent the browser of a project it names, over the Cockpit tests'
//! real ptyhost and fake `claude`, and the real Chromium manager with a fake Chromium launcher and
//! an in-process fake DevTools server.

use crate::cockpit::{
    Fixture, ORIGIN, attach, cockpit_id, fixture_with_browser, mcp, printed, read_until,
};
use crate::support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use futures::StreamExt as _;
use serde_json::{Value, json};
use support::{TestResult, call, mcp_tool, perms, seed_sandbox, serve, tool_text, ws_connect};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius::wiring::browser::ChromiumBrowsers;
use yulius_browser::fake::{FakeCdpServer, FakeChromium, write_fake_chromium};
use yulius_browser::{BrowserConfig, StopTimeouts};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, UserId};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(10);

/// The nine tools a task's agent drives the browser with.
const BROWSER_TOOLS: [&str; 9] = [
    "browser_tabs",
    "browser_open",
    "browser_navigate",
    "browser_click",
    "browser_type",
    "browser_snapshot",
    "browser_screenshot",
    "browser_request_user_action",
    "browser_fill_secret",
];

struct Browsed {
    f: Fixture,
    fake: FakeCdpServer,
    _scripts: tempfile::TempDir,
}

/// The Cockpit fixture (alice and carol write, bob reads `s1`, acme/fixture) with browsers.
async fn browsed() -> Result<Browsed, Box<dyn std::error::Error>> {
    let fake = FakeCdpServer::start().await?;
    let scripts = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        scripts.path(),
        "fake-chromium",
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
    )?;
    let config = BrowserConfig {
        startup_timeout: Duration::from_secs(30),
        stop: StopTimeouts {
            graceful: Duration::from_millis(100),
            term: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        },
        ..BrowserConfig::new(exe, scripts.path().join("unused"))
    };
    let f = fixture_with_browser(Arc::new(ChromiumBrowsers::new(config))).await?;
    Ok(Browsed {
        f,
        fake,
        _scripts: scripts,
    })
}

/// Ends the test's shells and browsers, so no fake process outlives it.
async fn finish(b: &Browsed) {
    b.f.h.app.end_all_shells().await;
    b.f.h.app.stop_workspace_services().await;
}

/// The next JSON message `accept` takes, skipping others; fails after [`WAIT`].
async fn wait_json(
    socket: &mut Socket,
    accept: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| "timed out waiting for a browser message")?
            .ok_or("socket closed")??;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(text.as_str())?;
            if accept(&value) {
                return Ok(value);
            }
        }
    }
}

fn tool_names(listed: &Value) -> Vec<String> {
    listed["result"]["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `DOCS_LOGIN`, bound to docs.example, in `s1`.
async fn docs_login(f: &Fixture, value: &str) -> TestResult {
    let created = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"name": "DOCS_LOGIN", "description": "Docs admin login",
            "hosts": ["docs.example"], "value": value}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(())
}

/// AC-SAB-1, AC-SAB-2 (SAB-001, SAB-003..005): the sandbox Superagent is served the nine browser
/// tools on its sandbox's browser, the reading ones run without asking; it signs in with an agent
/// secret it never sees, only on the secret's hosts, audited as its own; viewers see its cursor
/// as its person's Superagent; its hand-over flags no task.
#[tokio::test]
async fn the_sandbox_superagent_drives_its_sandboxs_browser() -> TestResult {
    const VALUE: &str = "docs-pass-7731-xyz";
    let b = browsed().await?;
    let result = async {
        let f = &b.f;
        let served = serve(f.h.router.clone()).await?;
        let id = cockpit_id(f, &f.alice).await?;
        let mut socket = attach(served.addr, &id, &f.alice).await?;
        let mut screen = String::new();
        read_until(&mut socket, &mut screen, "ready").await?;
        let config = PathBuf::from(printed(&screen, "config")?);
        let allowed = std::fs::read_to_string(config.join("launch-allowed-tools"))?;
        let allowed: Vec<&str> = allowed.split(',').collect();
        for tool in BROWSER_TOOLS {
            let reads = ["browser_tabs", "browser_snapshot", "browser_screenshot"].contains(&tool);
            assert_eq!(
                allowed.contains(&format!("mcp__yulius__{tool}").as_str()),
                reads,
                "{tool} runs without asking: {allowed:?}"
            );
        }
        let token =
            f.h.app
                .cockpit_mcp_token(&UserId::parse("u-alice")?, &SandboxId::parse("s1")?)
                .ok_or("alice's cockpit token")?;
        let names = tool_names(&mcp(f, &token, "tools/list", json!({})).await?);
        for tool in BROWSER_TOOLS {
            assert!(names.iter().any(|n| n == tool), "{tool} in {names:?}");
        }
        let init = mcp(f, &token, "initialize", json!({})).await?;
        let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
        assert!(
            instructions.contains("browser_fill_secret"),
            "{instructions}"
        );

        docs_login(f, VALUE).await?;
        let mut viewer =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.alice, ORIGIN).await?;
        let opened = mcp_tool(
            &f.h,
            &token,
            "browser_open",
            json!({"url": "https://docs.example/login"}),
        )
        .await?;
        assert_eq!(opened["isError"], false, "{opened}");
        let tab = tool_text(&opened)["tab"]["id"]
            .as_str()
            .ok_or("tab id")?
            .to_owned();
        let tabs = tool_text(&mcp_tool(&f.h, &token, "browser_tabs", json!({})).await?);
        assert!(
            tabs["tabs"]
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t["id"] == tab.as_str())),
            "{tabs}"
        );
        let clicked = mcp_tool(
            &f.h,
            &token,
            "browser_click",
            json!({"tab_id": tab, "selector": "#login"}),
        )
        .await?;
        assert_eq!(clicked["isError"], false, "{clicked}");
        let cursor = wait_json(&mut viewer, |m| {
            m["type"] == "agent_activity" && m["action"] == "click"
        })
        .await?;
        assert_eq!(cursor["label"], "alice's Superagent", "{cursor}");
        let typed = mcp_tool(
            &f.h,
            &token,
            "browser_type",
            json!({"tab_id": tab, "text": "alice"}),
        )
        .await?;
        assert_eq!(typed["isError"], false, "{typed}");
        let snapshot = mcp_tool(&f.h, &token, "browser_snapshot", json!({"tab_id": tab})).await?;
        assert_eq!(snapshot["isError"], false, "{snapshot}");
        let shot = mcp_tool(&f.h, &token, "browser_screenshot", json!({"tab_id": tab})).await?;
        assert_eq!(shot["content"][0]["type"], "image", "{shot}");

        // It signs in without seeing the value, on the secret's hosts only.
        let filled = mcp_tool(
            &f.h,
            &token,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN", "selector": "#password"}),
        )
        .await?;
        assert_eq!(filled["isError"], false, "{filled}");
        assert!(!filled.to_string().contains(VALUE), "{filled}");
        let typed: Vec<String> = b
            .fake
            .calls_for("Input.insertText")
            .iter()
            .filter_map(|c| c.params["text"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(typed.last().map(String::as_str), Some(VALUE));
        let fills = audited(f, "vault.agent.fill").await?;
        assert_eq!(fills.len(), 1, "{fills:?}");
        assert_eq!(fills[0]["actor"], "cockpit:alice", "{fills:?}");
        mcp_tool(
            &f.h,
            &token,
            "browser_navigate",
            json!({"tab_id": tab, "url": "https://evil.example/login"}),
        )
        .await?;
        let before = b.fake.calls_for("Input.insertText").len();
        let refused = mcp_tool(
            &f.h,
            &token,
            "browser_fill_secret",
            json!({"tab_id": tab, "secret": "DOCS_LOGIN"}),
        )
        .await?;
        assert_eq!(refused["isError"], true, "{refused}");
        assert_eq!(b.fake.calls_for("Input.insertText").len(), before);

        // Its hand-over gives the link and notifies viewers, flagging no task.
        let mut events = f.h.app.events.subscribe();
        let handed = mcp_tool(
            &f.h,
            &token,
            "browser_request_user_action",
            json!({"tab_id": tab, "reason": "Approve the sign-in"}),
        )
        .await?;
        assert_eq!(handed["isError"], false, "{handed}");
        assert_eq!(
            tool_text(&handed)["link"],
            format!("https://yulius.test/s/fixture/browser?tab={tab}")
        );
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let envelope = tokio::time::timeout_at(deadline, events.recv())
                .await
                .map_err(|_| "no browser.action_requested")??;
            if let UiEvent::BrowserActionRequested {
                tab_id, task_id, ..
            } = envelope.event
            {
                assert_eq!(tab_id, tab);
                assert_eq!(task_id, None);
                break;
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    finish(&b).await;
    result
}

/// The audited agent-secret uses `action` the Secrets page lists to alice.
async fn audited(f: &Fixture, action: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let activity = call(
        &f.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &f.alice,
    )
    .await?;
    Ok(activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == action)
        .cloned()
        .collect())
}

/// AC-SAB-3 (SAB-002..004): the Global Superagent is served the nine browser tools with a required
/// `project` and drives that project's browser for a person who works there; a person who only
/// reads it, and a project they cannot see, are refused.
#[tokio::test]
async fn the_global_superagent_drives_a_named_projects_browser() -> TestResult {
    const VALUE: &str = "docs-pass-8842-uvw";
    let b = browsed().await?;
    let result = async {
        let f = &b.f;
        // alice works in acme/website too; bob reads acme/fixture and works in acme/website,
        // carol cannot see acme/website.
        seed_sandbox(&f.h, "s2", "website", "u-alice").await?;
        f.h.github.set_perm("alice", "acme/website", perms("write"));
        f.h.github.set_perm("bob", "acme/website", perms("write"));
        std::fs::create_dir_all(f.data.join("sandboxes/s2/repo"))?;
        for creds in [&f.alice, &f.bob] {
            let opened = call(&f.h.router, Method::POST, "/api/superagent", None, creds).await?;
            assert_eq!(opened.status, StatusCode::OK, "{:?}", opened.body);
        }
        let token =
            f.h.app
                .global_superagent_mcp_token(&UserId::parse("u-alice")?)
                .ok_or("alice's Global Superagent")?;
        let listed = mcp(f, &token, "tools/list", json!({})).await?;
        let tools = listed["result"]["tools"].as_array().ok_or("tools")?;
        for name in BROWSER_TOOLS {
            let tool = tools
                .iter()
                .find(|t| t["name"] == name)
                .ok_or_else(|| format!("{name} not served"))?;
            assert_eq!(tool["inputSchema"]["required"][0], "project", "{tool}");
        }
        let init = mcp(f, &token, "initialize", json!({})).await?;
        let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
        assert!(instructions.contains("browser_open"), "{instructions}");

        docs_login(f, VALUE).await?;
        let served = serve(f.h.router.clone()).await?;
        let mut viewer =
            ws_connect(served.addr, "/api/sandboxes/s1/browser", &f.alice, ORIGIN).await?;
        let opened = mcp_tool(
            &f.h,
            &token,
            "browser_open",
            json!({"project": "acme/fixture", "url": "https://docs.example/login"}),
        )
        .await?;
        assert_eq!(opened["isError"], false, "{opened}");
        let tab = tool_text(&opened)["tab"]["id"]
            .as_str()
            .ok_or("tab id")?
            .to_owned();
        let cursor = wait_json(&mut viewer, |m| {
            m["type"] == "agent_activity" && m["action"] == "open"
        })
        .await?;
        assert_eq!(cursor["label"], "alice's Global Superagent", "{cursor}");
        let browser =
            f.h.app
                .browser_session(&SandboxId::parse("s1")?)
                .await
                .map_err(|e| e.to_string())?;
        assert!(browser.tabs().iter().any(|t| t.id.as_str() == tab));
        let filled = mcp_tool(
            &f.h,
            &token,
            "browser_fill_secret",
            json!({"project": "acme/fixture", "tab_id": tab, "secret": "DOCS_LOGIN"}),
        )
        .await?;
        assert_eq!(filled["isError"], false, "{filled}");
        assert!(!filled.to_string().contains(VALUE), "{filled}");
        let fills = audited(f, "vault.agent.fill").await?;
        assert_eq!(fills.len(), 1, "{fills:?}");
        assert_eq!(fills[0]["actor"], "superagent:alice", "{fills:?}");
        let shot = mcp_tool(
            &f.h,
            &token,
            "browser_screenshot",
            json!({"project": "acme/fixture", "tab_id": tab}),
        )
        .await?;
        assert_eq!(shot["content"][0]["type"], "image", "{shot}");

        // bob only reads acme/fixture: its browser is not his to drive.
        let bobs =
            f.h.app
                .global_superagent_mcp_token(&UserId::parse("u-bob")?)
                .ok_or("bob's Global Superagent")?;
        let refused = mcp_tool(
            &f.h,
            &bobs,
            "browser_tabs",
            json!({"project": "acme/fixture"}),
        )
        .await?;
        assert_eq!(refused["isError"], true, "{refused}");
        let unknown = mcp_tool(
            &f.h,
            &token,
            "browser_tabs",
            json!({"project": "acme/nowhere"}),
        )
        .await?;
        assert_eq!(unknown["isError"], true, "{unknown}");
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    finish(&b).await;
    result
}
