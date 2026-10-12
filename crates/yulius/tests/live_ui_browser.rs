//! Opt-in browser checks of the sandbox browser panel with a real server-side Chrome (the
//! sandbox Chrome agents drive) and a second headless Chrome as the user's browser running the
//! real UI (SPEC-001 BRW-005..008, SPEC-002 ADR-14, ADR-15).
//!
//! - The panel streams the sandbox Chrome: an agent's page shows on the canvas (a canvas in the
//!   page updates in the stream); a click and keys in the panel reach the page; a paste inserts
//!   the user's text; copy brings the page's selection to the user's clipboard; the wheel scrolls;
//!   the agent's click shows its cursor. With `YULIUS_LIVE_DISPLAY=1` on Linux (Xvfb, ffmpeg) the
//!   server Chrome is headful on a virtual display and the panel decodes its video; otherwise it is
//!   headless and the panel shows labelled JPEG frames.
//! - A sandbox dev server opens live in a new window on its preview origin, through the
//!   handshake: root-relative stylesheet and image, a redirect to `localhost`, a WebSocket; a
//!   hostile page on the preview origin cannot use the Yulius session (API and WebSocket refused);
//!   the sandbox Chrome refuses to open Yulius.
//! - Yulius refuses to render framed.
//! - The terminal (F 2026-10-07): its text is HTML and a double-click selects a word even after
//!   the program switched on mouse tracking; links to the VM are told apart from others.
//!
//! ```sh
//! YULIUS_LIVE_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   cargo test -p yulius --test main live_ui_browser:: -- --ignored
//! ```

use crate::support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use serde_json::{Value, json};
use support::{
    HarnessOptions, TestResult, harness, harness_with, perms, seed_github_user, seed_sandbox,
    setup_superadmin_creds,
};
use yulius::wiring::browser::ChromiumBrowsers;
use yulius_app::model::WebSessionRecord;
use yulius_app::ports::store::{SandboxStore, SessionStore, UserStore};
use yulius_browser::{CdpClient, CdpConfig};
use yulius_contracts::browser::OpenParams;
use yulius_domain::ids::{SandboxId, UserId};

struct Chrome(Child);

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn start_chrome(
    exe: &str,
    profile: &Path,
) -> Result<(Chrome, String), Box<dyn std::error::Error>> {
    let child = Command::new(exe)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--no-first-run",
            "--remote-debugging-port=0",
            "--window-size=1500,950",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let chrome = Chrome(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(profile.join("DevToolsActivePort")) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                return Ok((chrome, format!("ws://127.0.0.1:{port}{path}")));
            }
        }
        if Instant::now() > deadline {
            return Err("Chrome did not publish its DevTools port".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn eval(
    cdp: &CdpClient,
    session: &str,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let value = cdp
        .call_session(
            session,
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    Ok(value["result"]["value"].clone())
}

async fn until(cdp: &CdpClient, session: &str, what: &str, expression: &str) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if eval(cdp, session, &format!("!!({expression})")).await? == Value::Bool(true) {
            return Ok(());
        }
        if Instant::now() > deadline {
            let page = eval(cdp, session, "(() => document.querySelector('.bp')?.innerText.slice(0, 300) + ' | address: ' + document.querySelector('.bp-bar input')?.value + ' | shown: ' + document.querySelector('.bp-tab.active')?.textContent)()").await?;
            return Err(format!("timed out waiting for {what}; {page}").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The page served on the VM's loopback: its stylesheet and image are VM-only resources.
fn fixture_site() -> Router {
    Router::new()
        .route(
            "/",
            get(|| async {
                axum::response::Html(
                    r#"<!doctype html><html><head><link rel="stylesheet" href="/style.css"></head>
<body><h1 id="title">Fixture page</h1><img id="logo" src="/logo.svg" width="40" height="40">
<button id="go" onclick="document.getElementById('title').textContent='Clicked!'" style="position:absolute;left:100px;top:160px;width:140px;height:44px">Click me</button>
<canvas id="paint" width="60" height="30" style="position:absolute;left:300px;top:20px"></canvas>
<input id="name" style="position:absolute;left:100px;top:240px;width:160px">
<div style="height:3000px"></div>
<script>
const g = document.getElementById('paint').getContext('2d');
g.fillStyle = 'rgb(255, 0, 0)'; g.fillRect(0, 0, 60, 30);
setTimeout(() => { g.fillStyle = 'rgb(0, 200, 0)'; g.fillRect(0, 0, 60, 30); }, 1500);
</script></body></html>"#,
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async { ([("content-type", "text/css")], "h1 { color: rgb(1, 2, 3); }") }),
        )
        .route(
            "/logo.svg",
            get(|| async {
                ([("content-type", "image/svg+xml")], r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#539bf5"/></svg>"##)
            }),
        )
}

async fn serve_on(
    router: Router,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((addr, task))
}

/// The panel point (viewer CSS pixels) of page point `(x, y)` (CSS pixels of the sandbox page).
async fn stage_point(
    cdp: &CdpClient,
    session: &str,
    (x, y): (f64, f64),
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let point = eval(
        cdp,
        session,
        &format!("(() => {{ const c = document.querySelector('.bp-canvas'); const r = c.getBoundingClientRect(); const fit = r.width / c.width; return {{ x: r.left + {x} * fit, y: r.top + {y} * fit }}; }})()"),
    )
    .await?;
    Ok((
        point["x"].as_f64().ok_or("x")?,
        point["y"].as_f64().ok_or("y")?,
    ))
}

/// Evaluates `expression` in the server Chrome's page whose URL starts with `url`.
async fn page_eval(
    browser: &CdpClient,
    url: &str,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let targets = browser.call("Target.getTargets", json!({})).await?;
    let target = targets["targetInfos"]
        .as_array()
        .and_then(|infos| {
            infos.iter().find(|t| {
                t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with(url))
            })
        })
        .and_then(|t| t["targetId"].as_str())
        .ok_or_else(|| format!("no page at {url}"))?
        .to_owned();
    let attached = browser
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        )
        .await?;
    let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
    let value = browser
        .call_session(
            &session,
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .await?;
    let _ = browser
        .call("Target.detachFromTarget", json!({ "sessionId": session }))
        .await;
    Ok(value["result"]["value"].clone())
}

async fn until_page(browser: &CdpClient, url: &str, what: &str, expression: &str) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if page_eval(browser, url, &format!("!!({expression})"))
            .await
            .ok()
            == Some(Value::Bool(true))
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            let text = page_eval(
                browser,
                url,
                "document.body ? document.body.innerText.slice(0, 200) : ''",
            )
            .await
            .unwrap_or(Value::Null);
            return Err(format!("timed out waiting for {what} in the sandbox page: {text}").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn the_panel_streams_the_sandbox_chrome_and_takes_input() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    let display = std::env::var("YULIUS_LIVE_DISPLAY").ok().as_deref() == Some("1");
    let internal_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let internal_addr = internal_listener.local_addr()?;
    let ui_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = ui_listener.local_addr()?;
    let dir = tempfile::Builder::new().prefix("ylb").tempdir_in("/tmp")?;
    let mut config =
        yulius_browser::BrowserConfig::new(PathBuf::from(&chrome_exe), dir.path().join("profiles"));
    config.new_tab_url = Some(format!(
        "http://{internal_addr}/browser/welcome?sandbox={{sandbox}}"
    ));
    if display {
        let mut virtual_display = yulius_browser::DisplayConfig::new(dir.path().join("displays"));
        virtual_display.first_display = std::env::var("YULIUS_LIVE_DISPLAY_BASE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(90);
        config.display = yulius_browser::DisplayMode::Virtual(virtual_display);
    } else {
        config.no_sandbox = true;
    }
    let h = harness_with(HarnessOptions {
        browser: Some(Arc::new(ChromiumBrowsers::new(config))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.allowed_origins.push(format!("http://{addr}"));
            config.secure_cookies = false;
        })),
        ..HarnessOptions::default()
    })?;
    setup_superadmin_creds(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-ui-browser-session";
    h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: user.id,
            csrf_token: "csrf".into(),
            created_at: h.app.now(),
            expires_at: h.app.now().plus_millis(3_600_000),
            revoked_at: None,
        })
        .await?;
    let internal = h.internal.clone();
    let internal_task = tokio::spawn(async move {
        let _ = axum::serve(internal_listener, internal).await;
    });
    let router = h.router.clone();
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(ui_listener, router).await;
    });
    let (site, site_task) = serve_on(fixture_site()).await?;
    let page = format!("http://localhost:{}/", site.port());

    let result = async {
        let profile = tempfile::tempdir()?;
        let (_viewer, ws_url) = start_chrome(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" })).await?;
        let attached = cdp
            .call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }))
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let session = session.as_str();
        let mut console = cdp.subscribe_events();
        cdp.call_session(session, "Runtime.enable", json!({})).await?;
        cdp.call_session(session, "Network.enable", json!({})).await?;
        cdp.call_session(session, "Network.setCookie", json!({ "name": "yulius_session", "value": cookie, "url": format!("http://{addr}/") }))
            .await?;
        cdp.call(
            "Browser.grantPermissions",
            json!({ "origin": format!("http://{addr}"), "permissions": ["clipboardReadWrite", "clipboardSanitizedWrite"] }),
        )
        .await?;
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/s/fixture") }))
            .await?;
        until(&cdp, session, "the workspace", "document.querySelector('.tools-toggle')").await?;
        eval(&cdp, session, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Browser\"]').click(); true").await?;
        // The welcome page streams in the first tab.
        until(&cdp, session, "the first picture", "document.querySelector('.bp-overlay').hidden && document.querySelector('.bp-canvas').width > 0").await?;
        until(&cdp, session, "the welcome tab", "[...document.querySelectorAll('.bp-tab')].some((t) => t.textContent.includes('New tab'))").await?;
        let jpeg = eval(&cdp, session, "!document.querySelector('.bp-viewchip').hidden").await?;
        eprintln!("the panel shows {}", if jpeg == json!(true) { "JPEG frames" } else { "video" });
        if !display {
            assert_eq!(jpeg, json!(true), "a headless server browser is labelled JPEG mode");
        }
        // An agent opens the fixture: it comes to the front and shows in the panel.
        let agent = h.app.browser_session(&SandboxId::parse("s1")?).await?;
        let browser = CdpClient::connect(&agent.cdp_endpoint(), CdpConfig::default()).await?;
        let opened = agent.tool_open(&OpenParams { url: page.clone() }).await?;
        until(&cdp, session, "the agent's tab selected", &format!("document.querySelector('.bp-bar input').value === 'localhost:{}/'", site.port())).await?;
        // The page's canvas turns green after 1.5 s: the stream shows it.
        until(&cdp, session, "the updated canvas in the stream", "(() => { const c = document.querySelector('.bp-canvas'); const p = c.getContext('2d').getImageData(330, 35, 1, 1).data; return p[1] > 150 && p[0] < 80; })()").await?;
        // A click in the panel reaches the page.
        let button = stage_point(&cdp, session, (170.0, 182.0)).await?;
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": kind, "x": button.0, "y": button.1, "button": "left", "clickCount": 1 }))
                .await?;
        }
        until_page(&browser, &page, "the click", "document.getElementById('title').textContent === 'Clicked!'").await?;
        // Keys typed in the panel reach the focused field; a paste inserts the user's text.
        let field = stage_point(&cdp, session, (180.0, 251.0)).await?;
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": kind, "x": field.0, "y": field.1, "button": "left", "clickCount": 1 }))
                .await?;
        }
        until_page(&browser, &page, "the field focused", "document.activeElement && document.activeElement.id === 'name'").await?;
        for (key, code) in [("A", "KeyA"), ("d", "KeyD"), ("a", "KeyA")] {
            cdp.call_session(session, "Input.dispatchKeyEvent", json!({ "type": "keyDown", "key": key, "code": code, "text": key })).await?;
            cdp.call_session(session, "Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": key, "code": code })).await?;
        }
        until_page(&browser, &page, "the typed text", "document.getElementById('name').value === 'Ada'").await?;
        eval(&cdp, session, "(() => { const data = new DataTransfer(); data.setData('text/plain', ' Lovelace'); document.querySelector('.bp-keys').dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true })); return true; })()").await?;
        until_page(&browser, &page, "the pasted text", "document.getElementById('name').value === 'Ada Lovelace'").await?;
        // Copy: the page's selection reaches the user's clipboard.
        page_eval(&browser, &page, "(() => { document.activeElement.blur(); getSelection().selectAllChildren(document.getElementById('title')); return true; })()").await?;
        let mac = eval(&cdp, session, "/Mac/.test(navigator.platform)").await? == json!(true);
        let modifier = if mac { 4 } else { 2 };
        cdp.call_session(session, "Input.dispatchKeyEvent", json!({ "type": "rawKeyDown", "key": "c", "code": "KeyC", "modifiers": modifier, "windowsVirtualKeyCode": 67 })).await?;
        cdp.call_session(session, "Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": "c", "code": "KeyC", "modifiers": modifier, "windowsVirtualKeyCode": 67 })).await?;
        until(&cdp, session, "the copied selection", "navigator.clipboard.readText().then((t) => t === 'Clicked!')").await?;
        // The wheel scrolls the page.
        cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": field.0, "y": field.1, "deltaX": 0, "deltaY": 500 })).await?;
        until_page(&browser, &page, "the scrolled page", "window.scrollY > 100").await?;
        // An agent's click shows its labelled cursor over the stream.
        agent.agent_activity(&opened.tab.id, Some((150.0, 180.0)), "click", "#7 Claude Code");
        until(&cdp, session, "the agent cursor", "(() => { const c = document.querySelector('.bp-agent'); return c && !c.hidden && c.textContent.includes('#7 Claude Code'); })()").await?;
        // A new tab opens the welcome page.
        eval(&cdp, session, "document.querySelector('.bp-tabs button[aria-label=\"New tab\"]').click(), true").await?;
        until(&cdp, session, "the new welcome tab", "document.querySelector('.bp-tab.active')?.textContent.includes('New tab') && document.querySelector('.bp-bar input').value === ''").await?;
        // No uncaught exception or console error in the UI.
        let mut errors = Vec::new();
        while let Ok(event) = console.try_recv() {
            let error = event.method == "Runtime.exceptionThrown"
                || (event.method == "Runtime.consoleAPICalled" && event.params["type"] == "error");
            if error && event.session_id.as_deref() == Some(session) {
                errors.push(event.params.to_string());
            }
        }
        assert!(errors.is_empty(), "UI errors: {errors:?}");
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    h.app.stop_workspace_services().await;
    server_task.abort();
    internal_task.abort();
    site_task.abort();
    result
}

/// The live fixture app: root-relative stylesheet and image, a redirect to `localhost`, a
/// WebSocket echo, a field, long text to scroll, and a hostile script that tries to use the
/// user's Yulius session from the preview origin and reports what happened.
fn live_app(port: u16, yulius: String) -> Router {
    use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
    use axum::response::IntoResponse as _;
    let home = yulius.clone();
    let page = move || {
        let yulius = yulius.clone();
        async move {
            let html = format!(
                r#"<!doctype html><html><head><title>Live fixture</title><link rel="stylesheet" href="/app.css"></head>
<body><h1 id="title">Live fixture</h1><img id="logo" src="/logo.svg" width="40" height="40">
<p><a id="go" href="/go">Go somewhere</a></p><input id="name" style="width:200px">
<p id="ws">ws: waiting</p><p id="hostile">hostile: pending</p>
<div style="height:4000px">long page</div>
<script>
const ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws');
ws.onopen = () => ws.send('hello');
ws.onmessage = (e) => {{ document.getElementById('ws').textContent = 'ws: ' + e.data; }};
const found = {{}};
const report = () => {{ document.getElementById('hostile').textContent = 'hostile: fetch=' + found.fetch + ' ws=' + found.ws; }};
fetch('{yulius}/api/me', {{ credentials: 'include' }}).then((r) => {{ found.fetch = 'read ' + r.status; }}, () => {{ found.fetch = 'refused'; }}).finally(report);
const spy = new WebSocket('{ws}/api/events');
spy.onopen = () => {{ found.ws = 'open'; report(); }};
spy.onclose = () => {{ if (!found.ws) found.ws = 'refused'; report(); }};
</script></body></html>"#,
                ws = yulius.replacen("http://", "ws://", 1),
            );
            (
                [
                    (axum::http::header::X_FRAME_OPTIONS, "DENY"),
                    (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                ],
                html,
            )
        }
    };
    Router::new()
        .route("/", get(page.clone()))
        .route(
            "/after",
            get(|| async { axum::response::Html("<!doctype html><title>After</title><h1>After the redirect</h1>") }),
        )
        .route(
            "/go",
            get(move || async move {
                (axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, format!("http://localhost:{port}/after"))])
            }),
        )
        .route(
            "/go-yulius",
            get(move || {
                let location = format!("{home}/");
                async move { (axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, location)]) }
            }),
        )
        .route("/app.css", get(|| async { ([("content-type", "text/css")], "h1 { color: rgb(1, 2, 3); }") }))
        .route(
            "/logo.svg",
            get(|| async {
                ([("content-type", "image/svg+xml")], r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#539bf5"/></svg>"##)
            }),
        )
        .route(
            "/ws",
            get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if let WsMessage::Text(text) = message {
                            let reply = format!("echo: {}", text.as_str());
                            if socket.send(WsMessage::Text(reply.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                })
                .into_response()
            }),
        )
}

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn a_dev_server_opens_live_in_a_new_window() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    // Yulius at http://yulius.localhost:<port>: its previews are
    // http://<port>-<key>.yulius.localhost:<port>, which browsers resolve to loopback by
    // themselves (no DNS in the test) and which are same-site with Yulius, as in production
    // (a bare `localhost` would not be: `*.localhost` hosts are sites of their own).
    let internal_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let internal_addr = internal_listener.local_addr()?;
    let ui_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let ui_port = ui_listener.local_addr()?.port();
    let yulius = format!("http://yulius.localhost:{ui_port}");
    let dir = tempfile::Builder::new().prefix("ylv").tempdir_in("/tmp")?;
    let mut config =
        yulius_browser::BrowserConfig::new(PathBuf::from(&chrome_exe), dir.path().join("profiles"));
    config.no_sandbox = true;
    let welcome = format!("http://{internal_addr}/browser/welcome");
    config.new_tab_url = Some(format!("{welcome}?sandbox={{sandbox}}"));
    let ui_addr: std::net::SocketAddr = ([127, 0, 0, 1], ui_port).into();
    config.deny = yulius_browser::UrlDenyList::yulius(
        &yulius,
        &[ui_addr, internal_addr],
        vec![format!("{welcome}?"), format!("{welcome}/ports?")],
    );
    let public = yulius.clone();
    let h = harness_with(HarnessOptions {
        browser: Some(Arc::new(ChromiumBrowsers::new(config))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.public_url.clone_from(&public);
            config.secure_cookies = false;
            config.preview = yulius_app::preview::PreviewConfig::from_public_url(&public);
            config.own_ports = vec![ui_port, internal_addr.port()];
        })),
        ..HarnessOptions::default()
    })?;
    setup_superadmin_creds(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    // The fixture app listens on an OS-assigned port, the first of the sandbox's range (tests
    // run in parallel; a fixed range would collide).
    let site_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = site_listener.local_addr()?.port();
    let mut record = seed_sandbox(&h, "s0", "unused", "u-alice").await?;
    record.id = SandboxId::parse("s1")?;
    record.slug = "fixture".into();
    record.repo_name = "fixture".into();
    record.port_base = port;
    record.port_count = 3;
    h.store.insert_sandbox(&record).await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-ui-preview-session";
    h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: user.id,
            csrf_token: "csrf".into(),
            created_at: h.app.now(),
            expires_at: h.app.now().plus_millis(3_600_000),
            revoked_at: None,
        })
        .await?;
    let internal = h.internal.clone();
    let internal_task = tokio::spawn(async move {
        let _ = axum::serve(internal_listener, internal).await;
    });
    let router = h.router.clone();
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(ui_listener, router).await;
    });
    let site = live_app(port, yulius.clone());
    let site_task = tokio::spawn(async move {
        let _ = axum::serve(site_listener, site).await;
    });
    let key = h.app.preview_key(&SandboxId::parse("s1")?);
    let preview = format!("http://{port}-{key}.yulius.localhost:{ui_port}");

    let result = async {
        let profile = tempfile::tempdir()?;
        let (_viewer, ws_url) = start_chrome(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" })).await?;
        let attached = cdp
            .call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }))
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let session = session.as_str();
        cdp.call_session(session, "Page.enable", json!({})).await?;
        cdp.call_session(session, "Network.enable", json!({})).await?;
        cdp.call_session(session, "Network.setCookie", json!({ "name": "yulius_session", "value": cookie, "url": format!("{yulius}/") }))
            .await?;
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("{yulius}/s/fixture") })).await?;
        until(&cdp, session, "the workspace", "document.querySelector('.tools-toggle')").await?;
        eval(&cdp, session, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Browser\"]').click(); true").await?;
        // The dev server is open in the sandbox Chrome (an agent opened it): "Open live in a new
        // window" opens it on its preview origin in the user's own browser, after the handshake.
        let agent = h.app.browser_session(&SandboxId::parse("s1")?).await?;
        let opened = agent.tool_open(&OpenParams { url: format!("http://localhost:{port}/") }).await?;
        until(&cdp, session, "the app's tab", &format!("document.querySelector('.bp-bar input').value === 'localhost:{port}/'")).await?;
        let live = "document.querySelector('.bp-bar button[aria-label=\"Open live in a new window\"]')";
        until(&cdp, session, "Open live", &format!("{live} && !{live}.hidden")).await?;
        // A real click (a user gesture: the new window is not a blocked popup).
        let button = eval(&cdp, session, &format!("(() => {{ const r = {live}.getBoundingClientRect(); return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }}; }})()")).await?;
        let (bx, by) = (button["x"].as_f64().ok_or("x")?, button["y"].as_f64().ok_or("y")?);
        for kind in ["mousePressed", "mouseReleased"] {
            cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": kind, "x": bx, "y": by, "button": "left", "clickCount": 1 }))
                .await?;
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let window = loop {
            let targets = cdp.call("Target.getTargets", json!({})).await?;
            let found = targets["targetInfos"].as_array().and_then(|infos| {
                infos.iter().find(|t| t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with(&preview)))
            }).and_then(|t| t["targetId"].as_str()).map(str::to_owned);
            if let Some(found) = found {
                break found;
            }
            if Instant::now() > deadline {
                return Err(format!("no window on {preview}: {targets}").into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        let attached = cdp.call("Target.attachToTarget", json!({ "targetId": window, "flatten": true })).await?;
        let live_session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let live_session = live_session.as_str();
        until(&cdp, live_session, "the live page", "document.getElementById('title')?.textContent === 'Live fixture'").await?;
        // Root-relative stylesheet and image load from the preview origin; the WebSocket works.
        until(&cdp, live_session, "the page's styles", "getComputedStyle(document.getElementById('title')).color === 'rgb(1, 2, 3)'").await?;
        until(&cdp, live_session, "the image", "document.getElementById('logo').complete && document.getElementById('logo').naturalWidth > 0").await?;
        until(&cdp, live_session, "the WebSocket echo", "document.getElementById('ws').textContent === 'ws: echo: hello'").await?;
        // The page on the preview origin cannot use the Yulius session: the API refuses it and
        // its WebSockets do not open.
        until(&cdp, live_session, "the hostile report", "/fetch=(?!undefined)\\w+ ws=(?!undefined)\\w+/.test(document.getElementById('hostile').textContent)").await?;
        let hostile = eval(&cdp, live_session, "document.getElementById('hostile').textContent").await?;
        assert_eq!(hostile, json!("hostile: fetch=refused ws=refused"), "a preview page used the Yulius session");
        // A link that redirects to localhost stays on the preview origin.
        eval(&cdp, live_session, "document.getElementById('go').click(), true").await?;
        until(&cdp, live_session, "the page after the redirect", &format!("location.href.startsWith('{preview}/after') && document.body.innerText.includes('After the redirect')")).await?;
        // The sandbox Chrome refuses Yulius itself, asked directly or through a page.
        let refused = agent.tool_open(&OpenParams { url: format!("{yulius}/") }).await;
        assert!(refused.is_err(), "the sandbox Chrome opened Yulius: {refused:?}");
        let refused = agent.tool_open(&OpenParams { url: format!("{preview}/") }).await;
        assert!(refused.is_err(), "the sandbox Chrome opened a preview origin: {refused:?}");
        // A page of the app that redirects to Yulius: the request is intercepted and failed, so
        // the tab never shows the Yulius UI (whose title is "Yulius").
        let navigated = agent
            .tool_navigate(&yulius_contracts::browser::NavigateParams { tab_id: opened.tab.id.clone(), url: format!("http://localhost:{port}/go-yulius") })
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let tab = agent.tabs().into_iter().find(|t| t.id == opened.tab.id).ok_or("the agent's tab")?;
        assert_ne!(tab.title, "Yulius", "the sandbox Chrome showed Yulius: {tab:?} ({navigated:?})");
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    h.app.stop_workspace_services().await;
    server_task.abort();
    internal_task.abort();
    site_task.abort();
    result
}

/// Yulius refuses to render inside a frame even when a proxy strips its frame headers.
#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn the_ui_refuses_to_render_framed() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let h = harness()?;
    let router = h.router.clone();
    // A server that serves the UI without its frame headers, and a page that frames it.
    let stripped = Router::new()
        .route("/frame-test", get(|| async { axum::response::Html("<!doctype html><title>outer</title><iframe id=\"inner\" src=\"/\" width=\"800\" height=\"600\"></iframe>") }))
        .fallback(move |request: axum::extract::Request| {
            let router = router.clone();
            async move {
                use tower::ServiceExt as _;
                match router.oneshot(request).await {
                    Ok(mut response) => {
                        response.headers_mut().remove("x-frame-options");
                        response.headers_mut().remove("content-security-policy");
                        response
                    }
                    Err(never) => match never {},
                }
            }
        });
    let (addr, task) = serve_on(stripped).await?;
    let result = async {
        let profile = tempfile::tempdir()?;
        let (_viewer, ws_url) = start_chrome(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" })).await?;
        let attached = cdp.call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true })).await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let session = session.as_str();
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/frame-test") })).await?;
        until(&cdp, session, "the refusal inside the frame", "document.getElementById('inner')?.contentDocument?.body?.innerText.includes('cannot run inside a frame')").await?;
        let app = eval(&cdp, session, "Boolean(document.getElementById('inner').contentDocument.querySelector('.app'))").await?;
        assert_eq!(app, json!(false), "the framed UI must not start");
        // Top level, the same server's UI starts normally.
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/") })).await?;
        until(&cdp, session, "the UI at top level", "document.querySelector('#app')?.children.length > 0 && !document.body.innerText.includes('cannot run inside a frame')").await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    task.abort();
    result
}

/// The terminal as the UI builds it: the real xterm and mouse filter, fed output that turns on
/// mouse tracking (as Claude Code does).
const TERMINAL_FIXTURE: &str = r"(async () => {
  const { Terminal } = await import('/assets/vendor/xterm/xterm.mjs');
  const { createMouseFilter } = await import('/assets/mousefilter.js');
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = '/assets/vendor/xterm/xterm.css';
  const loaded = new Promise((resolve) => { link.onload = resolve; });
  document.head.append(link);
  await loaded;
  const host = document.createElement('div');
  host.style.cssText = 'position:fixed;left:0;top:0;width:900px;height:300px;z-index:2147483647;background:#000';
  document.body.append(host);
  const term = new Terminal({ fontSize: 16, macOptionClickForcesSelection: true });
  term.open(host);
  const filter = createMouseFilter();
  const output = new TextEncoder().encode('\x1b[?1000h\x1b[?1002h\x1b[?1006hhello yulius world\r\n');
  await new Promise((resolve) => term.write(filter.feed(output), resolve));
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  window.__term = term;
  const walker = document.createTreeWalker(host.querySelector('.xterm-rows'), NodeFilter.SHOW_TEXT);
  let node;
  while ((node = walker.nextNode()) && !node.data.includes('yulius')) {}
  if (!node) return { error: 'no text node with the word' };
  const range = document.createRange();
  const at = node.data.indexOf('yulius');
  range.setStart(node, at + 1);
  range.setEnd(node, at + 3);
  const box = range.getBoundingClientRect();
  return {
    canvases: host.querySelectorAll('canvas').length,
    row: host.querySelector('.xterm-rows').textContent,
    tracking: term.modes.mouseTrackingMode,
    x: box.left + box.width / 2,
    y: box.top + box.height / 2,
  };
})()";

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn terminal_text_is_html_a_double_click_selects_a_word_and_links_route() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let h = harness()?;
    let (addr, server_task) = serve_on(h.router.clone()).await?;
    let result = async {
        let profile = tempfile::tempdir()?;
        let (_viewer, ws_url) = start_chrome(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await?;
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target["targetId"], "flatten": true }),
            )
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let session = session.as_str();
        cdp.call_session(
            session,
            "Page.navigate",
            json!({ "url": format!("http://{addr}/") }),
        )
        .await?;
        until(
            &cdp,
            session,
            "the page",
            "document.readyState === 'complete'",
        )
        .await?;
        let fixture = eval(&cdp, session, TERMINAL_FIXTURE).await?;
        assert_eq!(fixture["error"], Value::Null, "{fixture}");
        assert_eq!(
            fixture["canvases"],
            json!(0),
            "rows are HTML, not a canvas: {fixture}"
        );
        assert!(
            fixture["row"]
                .as_str()
                .is_some_and(|row| row.contains("hello yulius world")),
            "{fixture}"
        );
        assert_eq!(
            fixture["tracking"],
            json!("none"),
            "the filter kept the mouse for selection: {fixture}"
        );
        let (x, y) = (
            fixture["x"].as_f64().ok_or("x")?,
            fixture["y"].as_f64().ok_or("y")?,
        );
        for count in [1, 2] {
            for kind in ["mousePressed", "mouseReleased"] {
                cdp.call_session(
                    session,
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": count }),
                )
                .await?;
            }
        }
        until(
            &cdp,
            session,
            "the double-click selection",
            "window.__term.getSelection() === 'yulius'",
        )
        .await?;
        // Links to the VM open in the sandbox browser, others in a new tab (UI-018).
        let links = eval(&cdp, session, "(async () => { const { isLocalUrl } = await import('/assets/dom.js'); return ['http://localhost:3000/', 'http://127.0.0.1:8080/x', 'http://[::1]:5173/', 'http://app.localhost/', 'https://github.com/acme', 'http://example.com:3000/', 'not a url'].map(isLocalUrl); })()").await?;
        assert_eq!(links, json!([true, true, true, true, false, false, false]));
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server_task.abort();
    result
}
