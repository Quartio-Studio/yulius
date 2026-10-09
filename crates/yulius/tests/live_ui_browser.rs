//! Opt-in browser checks of the sandbox browser panel with a real server-side Chromium (the
//! sandbox Chrome agents drive) and a second headless Chrome as the user's browser running the
//! real UI (SPEC-001 BRW-005..008, SPEC-002 ADR-14).
//!
//! - Live tabs: the start page lists the listening sandbox port; opening it shows the app live
//!   in an iframe on its preview origin (`<port>-<key>.yulius.localhost`), through the handshake:
//!   root-relative stylesheet and image, a redirect to `localhost`, a WebSocket, native typing,
//!   double-click selection and wheel scrolling inside the page, the address bar and Back from
//!   the bridge script; an agent acting on the same app is announced over the live tab and its
//!   cursor shows in its own tab; a hostile page on the preview origin cannot use the Yulius
//!   session (API and WebSocket refused); Yulius refuses to render framed; the sandbox Chrome
//!   refuses to open Yulius.
//! - The sandbox Chrome's tab (an agent's) is mirrored as DOM: its stylesheet and image load
//!   through Yulius, a canvas keeps updating, clicks and typing reach the real page, text
//!   selects and copies, the agent's click shows its cursor, the pixel view can be chosen and
//!   left, and a hostile page that feeds crafted events to the recorder cannot run script in the
//!   UI (the mirror iframe stays script-free).
//! - The terminal (F 2026-10-07): its text is HTML and a double-click selects a word even after
//!   the program switched on mouse tracking; links to the VM are told apart from others.
//!
//! ```sh
//! YULIUS_LIVE_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   cargo test -p yulius --test live_ui_browser -- --ignored
//! ```

mod support;

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
            let page = eval(cdp, session, "(() => { const d = document.querySelector('.bp-mirror iframe')?.contentDocument; return document.querySelector('.bp')?.innerText.slice(0, 300) + ' | mirror: ' + (d ? d.body?.innerText.slice(0, 300) : 'none'); })()").await?;
            return Err(format!("timed out waiting for {what}; {page}").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// A page that calls the recorder's binding itself with a crafted snapshot whose inline handler
/// and script would run in the UI if the mirror iframe allowed scripts.
const HOSTILE_PAGE: &str = r"<!doctype html><html><body><p>hostile page</p><script>
setTimeout(() => {
  const t = Date.now();
  const node = { type: 0, id: 1, childNodes: [{ type: 2, tagName: 'html', attributes: {}, id: 2, childNodes: [
    { type: 2, tagName: 'head', attributes: {}, id: 3, childNodes: [] },
    { type: 2, tagName: 'body', attributes: { onload: 'parent.__pwned = 3' }, id: 4, childNodes: [
      { type: 3, textContent: 'crafted mirror', id: 5 },
      { type: 2, tagName: 'img', attributes: { src: 'data:,', onerror: 'parent.__pwned = 1' }, id: 6, childNodes: [] },
      { type: 2, tagName: 'script', attributes: {}, id: 7, childNodes: [{ type: 3, textContent: 'parent.__pwned = 2', id: 8 }] },
    ] },
  ] }] };
  window.__yuliusMirrorSend(JSON.stringify([
    { type: 4, data: { href: location.href, width: 800, height: 600 }, timestamp: t },
    { type: 2, data: { node, initialOffset: { left: 0, top: 0 } }, timestamp: t },
  ]));
}, 1500);
</script></body></html>";

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
<script>
const g = document.getElementById('paint').getContext('2d');
g.fillStyle = 'rgb(255, 0, 0)'; g.fillRect(0, 0, 60, 30);
setTimeout(() => { g.fillStyle = 'rgb(0, 200, 0)'; g.fillRect(0, 0, 60, 30); }, 1500);
</script></body></html>"#,
                )
            }),
        )
        .route(
            "/hostile",
            get(|| async { axum::response::Html(HOSTILE_PAGE) }),
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

/// Clicks the centre of `selector` in the mirror with real input events in the viewer.
async fn click_in_mirror(cdp: &CdpClient, session: &str, selector: &str) -> TestResult {
    let target = eval(
        cdp,
        session,
        &format!(
            "(() => {{ const frame = document.querySelector('.bp-mirror iframe'); const b = frame.contentDocument.querySelector(\"{selector}\").getBoundingClientRect(); const f = frame.getBoundingClientRect(); const scale = f.width / frame.offsetWidth; return {{ x: f.left + (b.left + b.width / 2) * scale, y: f.top + (b.top + b.height / 2) * scale }}; }})()"
        ),
    )
    .await?;
    let (x, y) = (
        target["x"].as_f64().ok_or("x")?,
        target["y"].as_f64().ok_or("y")?,
    );
    for kind in ["mousePressed", "mouseReleased"] {
        cdp.call_session(
            session,
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
        )
        .await?;
    }
    Ok(())
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

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn the_sandbox_chrome_tab_is_mirrored_as_dom() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    // The internal listener serves the welcome page; its address is fixed before the app.
    let internal_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let internal_addr = internal_listener.local_addr()?;
    // The UI listener too: its origin must be allowed for the viewer WebSocket.
    let ui_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = ui_listener.local_addr()?;
    let dir = tempfile::Builder::new().prefix("ylb").tempdir_in("/tmp")?;
    let mut config =
        yulius_browser::BrowserConfig::new(PathBuf::from(&chrome_exe), dir.path().join("profiles"));
    config.no_sandbox = true;
    config.new_tab_url = Some(format!(
        "http://{internal_addr}/browser/welcome?sandbox={{sandbox}}"
    ));
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
    // The fixture site listens on the sandbox's first port, so the welcome page links it.
    let port = h
        .store
        .sandbox(&SandboxId::parse("s1")?)
        .await?
        .ok_or("sandbox")?
        .port_base;
    let site_listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let site_task = tokio::spawn(async move {
        let _ = axum::serve(site_listener, fixture_site()).await;
    });

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
        cdp.call_session(session, "Network.enable", json!({})).await?;
        cdp.call_session(session, "Network.setCookie", json!({ "name": "yulius_session", "value": cookie, "url": format!("http://{addr}/") }))
            .await?;
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/s/fixture") }))
            .await?;
        until(&cdp, session, "the workspace", "document.querySelector('.tools-toggle')").await?;
        let rail_hidden = eval(&cdp, session, "document.querySelector('.app').classList.contains('rail-hidden')").await?;
        assert_eq!(rail_hidden, json!(true), "the tools rail is hidden by default");
        eval(&cdp, session, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Browser\"]').click(); true").await?;
        // The start page lists the listening port. An agent opens the page in the sandbox Chrome:
        // its tab joins the strip, marked as the sandbox Chrome's; watching it mirrors the page
        // served on the VM's loopback, with its stylesheet and image.
        until(&cdp, session, "the start page", &format!("[...document.querySelectorAll('.bp-port')].some((b) => b.textContent.includes('localhost:{port}'))")).await?;
        let agent = h.app.browser_session(&SandboxId::parse("s1")?).await?;
        let opened = agent.tool_open(&OpenParams { url: format!("http://localhost:{port}/") }).await?;
        agent.agent_activity(&opened.tab.id, None, "open", "#7 Claude Code");
        until(&cdp, session, "the agent's tab in the strip", "document.querySelector('.bp-tab.remote')").await?;
        eval(&cdp, session, "document.querySelector('.bp-tab.remote').click(), true").await?;
        let mirror = "document.querySelector('.bp-mirror iframe')?.contentDocument";
        until(&cdp, session, "the fixture page in the mirror", &format!("{mirror}?.body?.innerText.includes('Fixture page')")).await?;
        let sandbox = eval(&cdp, session, "document.querySelector('.bp-mirror iframe').getAttribute('sandbox')").await?;
        assert_eq!(sandbox, json!("allow-same-origin"), "the mirror iframe never runs scripts");
        until(&cdp, session, "the proxied image", &format!("(() => {{ const i = {mirror}?.querySelector('#logo'); return i && i.complete && i.naturalWidth > 0 && i.src.includes('/browser/resource'); }})()")).await?;
        until(&cdp, session, "the page's styles", &format!("(() => {{ const d = {mirror}; const t = d?.querySelector('#title'); return t && d.defaultView.getComputedStyle(t).color === 'rgb(1, 2, 3)'; }})()")).await?;
        // The canvas keeps updating (sampled frames painted by the UI, not by the iframe).
        until(&cdp, session, "the updated canvas", &format!("(() => {{ const c = {mirror}?.querySelector('#paint'); if (!c) return false; const p = c.getContext('2d').getImageData(5, 5, 1, 1).data; return p[0] < 50 && p[1] > 150; }})()")).await?;
        // The mirror is a real page: a double-click selects a word and copies it at once.
        cdp.call(
            "Browser.grantPermissions",
            json!({ "origin": format!("http://{addr}"), "permissions": ["clipboardReadWrite", "clipboardSanitizedWrite"] }),
        )
        .await?;
        let word = eval(&cdp, session, &format!("(() => {{ const frame = document.querySelector('.bp-mirror iframe'); const d = {mirror}; const text = d.querySelector('#title').firstChild; const range = d.createRange(); range.setStart(text, 1); range.setEnd(text, 4); const b = range.getBoundingClientRect(); const f = frame.getBoundingClientRect(); const scale = f.width / frame.offsetWidth; return {{ x: f.left + (b.left + b.width / 2) * scale, y: f.top + (b.top + b.height / 2) * scale }}; }})()")).await?;
        let (x, y) = (word["x"].as_f64().ok_or("x")?, word["y"].as_f64().ok_or("y")?);
        for count in [1, 2] {
            for kind in ["mousePressed", "mouseReleased"] {
                cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": count }))
                    .await?;
            }
        }
        until(&cdp, session, "the selected word", &format!("{mirror}.getSelection().toString().trim() === 'Fixture'")).await?;
        until(&cdp, session, "the copy notice", "!document.querySelector('.bp-copied').hidden").await?;
        // Dragging selects across words (starting outside the current selection, as on any page).
        eval(&cdp, session, &format!("{mirror}.getSelection().removeAllRanges(), true")).await?;
        let span = eval(&cdp, session, &format!("(() => {{ const frame = document.querySelector('.bp-mirror iframe'); const d = {mirror}; const text = d.querySelector('#title').firstChild; const point = (offset) => {{ const r = d.createRange(); r.setStart(text, offset); r.setEnd(text, offset + 1); const b = r.getBoundingClientRect(); const f = frame.getBoundingClientRect(); const scale = f.width / frame.offsetWidth; return [f.left + (b.left + 1) * scale, f.top + (b.top + b.height / 2) * scale]; }}; return {{ from: point(0), to: point(text.data.length - 1) }}; }})()")).await?;
        let (fx, fy) = (span["from"][0].as_f64().ok_or("from")?, span["from"][1].as_f64().ok_or("from")?);
        let (tx, ty) = (span["to"][0].as_f64().ok_or("to")?, span["to"][1].as_f64().ok_or("to")?);
        cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": "mousePressed", "x": fx, "y": fy, "button": "left", "clickCount": 1 })).await?;
        for step in 1..=5 {
            let x = fx + (tx + 8.0 - fx) * f64::from(step) / 5.0;
            cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": ty, "button": "left", "buttons": 1 })).await?;
        }
        cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": "mouseReleased", "x": tx + 8.0, "y": ty, "button": "left", "clickCount": 1 })).await?;
        until(&cdp, session, "the dragged selection", &format!("{mirror}.getSelection().toString().trim() === 'Fixture page'")).await?;
        // A click in the mirror reaches the real page; the mirror shows the change.
        click_in_mirror(&cdp, session, "#go").await?;
        until(&cdp, session, "the click result in the mirror", &format!("{mirror}?.querySelector('#title')?.textContent === 'Clicked!'")).await?;
        // Typing into a field of the mirror reaches the real page; its value streams back.
        click_in_mirror(&cdp, session, "#name").await?;
        for (key, code) in [("A", "KeyA"), ("d", "KeyD"), ("a", "KeyA")] {
            for kind in ["keyDown", "keyUp"] {
                let text = if kind == "keyDown" { key } else { "" };
                cdp.call_session(session, "Input.dispatchKeyEvent", json!({ "type": kind, "key": key, "code": code, "text": text })).await?;
            }
        }
        until(&cdp, session, "the typed value in the mirror", &format!("{mirror}?.querySelector('#name')?.value === 'Ada'")).await?;
        // An agent's click shows its cursor.
        let browser = h.app.browser_session(&SandboxId::parse("s1")?).await?;
        let tab = browser.tabs().into_iter().find(|t| t.active).ok_or("active tab")?;
        browser.agent_activity(&tab.id, Some((150.0, 180.0)), "click", "#7 Claude Code");
        until(&cdp, session, "the agent cursor", "(() => { const c = document.querySelector('.bp-agent'); return c && !c.hidden && c.textContent.includes('#7 Claude Code'); })()").await?;
        // The pixel view on request, and back.
        let toggle = "[...document.querySelectorAll('button')].find((b) => b.textContent === 'Pixels')";
        eval(&cdp, session, &format!("{toggle}.click(), true")).await?;
        until(&cdp, session, "the pixel view", "(() => { const c = document.querySelector('.bp-canvas'); return c && !c.hidden && c.width > 0 && document.querySelector('.bp-mirror').hidden; })()").await?;
        eval(&cdp, session, &format!("{toggle}.click(), true")).await?;
        until(&cdp, session, "the DOM view again", &format!("document.querySelector('.bp-canvas').hidden && {mirror}?.body?.innerText.includes('Clicked!')")).await?;
        // A hostile page feeding crafted events: shown, but nothing runs in the UI.
        eval(&cdp, session, &format!("(() => {{ const a = document.querySelector('.bp-bar input'); a.value = 'http://127.0.0.1:{port}/hostile'; a.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'Enter', bubbles: true }})); return true; }})()")).await?;
        until(&cdp, session, "the crafted mirror", &format!("{mirror}?.body?.innerText.includes('crafted mirror')")).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let pwned = eval(&cdp, session, "String(window.__pwned)").await?;
        assert_eq!(pwned, json!("undefined"), "crafted mirror content ran script in the UI");
        let sandbox = eval(&cdp, session, "document.querySelector('.bp-mirror iframe').getAttribute('sandbox')").await?;
        assert_eq!(sandbox, json!("allow-same-origin"));
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

/// Evaluates `expression` in the first frame whose URL starts with `prefix` (an isolated world:
/// the page's DOM, none of its scripts).
async fn frame_eval(
    cdp: &CdpClient,
    session: &str,
    prefix: &str,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    fn find(node: &Value, prefix: &str) -> Option<String> {
        let frame = &node["frame"];
        if frame["url"]
            .as_str()
            .is_some_and(|url| url.starts_with(prefix))
        {
            return frame["id"].as_str().map(str::to_owned);
        }
        node["childFrames"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, prefix))
    }
    let tree = cdp
        .call_session(session, "Page.getFrameTree", json!({}))
        .await?;
    let Some(frame) = find(&tree["frameTree"], prefix) else {
        return Ok(Value::Null);
    };
    let world = cdp
        .call_session(
            session,
            "Page.createIsolatedWorld",
            json!({ "frameId": frame, "worldName": "live-test" }),
        )
        .await?;
    let value = cdp
        .call_session(
            session,
            "Runtime.evaluate",
            json!({ "expression": expression, "contextId": world["executionContextId"], "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    Ok(value["result"]["value"].clone())
}

async fn until_frame(
    cdp: &CdpClient,
    session: &str,
    prefix: &str,
    what: &str,
    expression: &str,
) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if frame_eval(cdp, session, prefix, &format!("!!({expression})")).await?
            == Value::Bool(true)
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            let text = frame_eval(
                cdp,
                session,
                prefix,
                "document.body ? document.body.innerText.slice(0, 300) : location.href",
            )
            .await?;
            let ui = eval(
                cdp,
                session,
                "document.querySelector('.bp')?.innerText.slice(0, 300)",
            )
            .await?;
            return Err(format!("timed out waiting for {what}; frame: {text}; panel: {ui}").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Page coordinates of `selector`'s centre inside the live iframe (with `text_chars`, the centre
/// of its first characters: a word to double-click).
async fn live_point(
    cdp: &CdpClient,
    session: &str,
    prefix: &str,
    selector: &str,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    live_point_of(cdp, session, prefix, selector, 0).await
}

async fn live_point_of(
    cdp: &CdpClient,
    session: &str,
    prefix: &str,
    selector: &str,
    text_chars: u32,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let frame = eval(cdp, session, "(() => { const r = [...document.querySelectorAll('.bp-live')].find((f) => f.offsetParent).getBoundingClientRect(); return { x: r.left, y: r.top }; })()").await?;
    let inner = frame_eval(cdp, session, prefix, &format!("(() => {{ const el = document.querySelector(\"{selector}\"); let r = el.getBoundingClientRect(); if ({text_chars} > 0) {{ const range = document.createRange(); range.setStart(el.firstChild, 0); range.setEnd(el.firstChild, {text_chars}); r = range.getBoundingClientRect(); }} return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }}; }})()")).await?;
    Ok((
        frame["x"].as_f64().ok_or("frame x")? + inner["x"].as_f64().ok_or("x")?,
        frame["y"].as_f64().ok_or("frame y")? + inner["y"].as_f64().ok_or("y")?,
    ))
}

async fn click_at(cdp: &CdpClient, session: &str, (x, y): (f64, f64), count: u32) -> TestResult {
    for click in 1..=count {
        for kind in ["mousePressed", "mouseReleased"] {
            cdp.call_session(
                session,
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": click }),
            )
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn sandbox_apps_open_live_in_the_users_browser() -> TestResult {
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
        // The start page lists the listening port; opening it shows the app live, in an iframe
        // on its own preview origin, after the handshake through Yulius.
        let button = format!("[...document.querySelectorAll('.bp-port')].find((b) => b.textContent.includes('localhost:{port}'))");
        until(&cdp, session, "the port on the start page", &button).await?;
        eval(&cdp, session, &format!("{button}.click(), true")).await?;
        until(&cdp, session, "the live iframe", &format!("[...document.querySelectorAll('.bp-live')].some((f) => f.src.startsWith('{preview}/'))")).await?;
        until_frame(&cdp, session, &preview, "the live page", "document.getElementById('title')?.textContent === 'Live fixture'").await?;
        // Root-relative stylesheet and image load from the preview origin; the WebSocket works.
        until_frame(&cdp, session, &preview, "the page's styles", "getComputedStyle(document.getElementById('title')).color === 'rgb(1, 2, 3)'").await?;
        until_frame(&cdp, session, &preview, "the image", "document.getElementById('logo').complete && document.getElementById('logo').naturalWidth > 0").await?;
        until_frame(&cdp, session, &preview, "the WebSocket echo", "document.getElementById('ws').textContent === 'ws: echo: hello'").await?;
        // The page on the preview origin cannot use the Yulius session: the API refuses it and
        // its WebSockets do not open.
        until_frame(&cdp, session, &preview, "the hostile report", "/fetch=\\w+ ws=\\w+/.test(document.getElementById('hostile').textContent)").await?;
        let hostile = frame_eval(&cdp, session, &preview, "document.getElementById('hostile').textContent").await?;
        assert_eq!(hostile, json!("hostile: fetch=refused ws=refused"), "a preview page used the Yulius session");
        // The bridge reports the address; the panel shows it as localhost.
        until(&cdp, session, "the address bar", &format!("document.querySelector('.bp-bar input').value === 'localhost:{port}/'")).await?;
        // Native typing, selection and scrolling: real events in the user's browser.
        let field = live_point(&cdp, session, &preview, "#name").await?;
        click_at(&cdp, session, field, 1).await?;
        cdp.call_session(session, "Input.insertText", json!({ "text": "Ada" })).await?;
        until_frame(&cdp, session, &preview, "the typed text", "document.getElementById('name').value === 'Ada'").await?;
        let word = live_point_of(&cdp, session, &preview, "#title", 4).await?;
        click_at(&cdp, session, word, 2).await?;
        until_frame(&cdp, session, &preview, "the double-click selection", "getSelection().toString().trim() === 'Live'").await?;
        cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": word.0, "y": word.1 + 60.0, "deltaX": 0, "deltaY": 600 }))
            .await?;
        until_frame(&cdp, session, &preview, "the scrolled page", "window.scrollY > 0").await?;
        // A link that redirects to localhost stays on the preview; Back returns.
        cdp.call_session(session, "Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": word.0, "y": word.1, "deltaX": 0, "deltaY": -2000 }))
            .await?;
        until_frame(&cdp, session, &preview, "the top of the page", "window.scrollY === 0").await?;
        let link = live_point(&cdp, session, &preview, "#go").await?;
        click_at(&cdp, session, link, 1).await?;
        until(&cdp, session, "the redirected address", &format!("document.querySelector('.bp-bar input').value === 'localhost:{port}/after'")).await?;
        until_frame(&cdp, session, &preview, "the page after the redirect", "document.body.innerText.includes('After the redirect')").await?;
        until(&cdp, session, "Back enabled", "!document.querySelector('.bp-bar button[aria-label=\"Back\"]').disabled").await?;
        eval(&cdp, session, "document.querySelector('.bp-bar button[aria-label=\"Back\"]').click(), true").await?;
        until(&cdp, session, "the address after Back", &format!("document.querySelector('.bp-bar input').value === 'localhost:{port}/'")).await?;
        // An agent works on the same app in the sandbox Chrome: announced over the live tab,
        // and its cursor shows in its own tab.
        let agent = h.app.browser_session(&SandboxId::parse("s1")?).await?;
        let opened = agent.tool_open(&OpenParams { url: format!("http://localhost:{port}/") }).await?;
        until(&cdp, session, "the agent's tab", "document.querySelector('.bp-tab.remote')").await?;
        agent.agent_activity(&opened.tab.id, Some((120.0, 40.0)), "click", "#7 Claude Code");
        until(&cdp, session, "the agent notice over the live tab", "(() => { const n = document.querySelector('.bp-notice'); return n && !n.hidden && n.textContent.includes('#7 Claude Code'); })()").await?;
        eval(&cdp, session, "[...document.querySelectorAll('.bp-notice button')].find((b) => b.textContent === 'Watch').click(), true").await?;
        until(&cdp, session, "the agent's tab watched", "document.querySelector('.bp-tab.remote.active')").await?;
        agent.agent_activity(&opened.tab.id, Some((120.0, 40.0)), "click", "#7 Claude Code");
        until(&cdp, session, "the agent cursor", "(() => { const c = document.querySelector('.bp-agent'); return c && !c.hidden && c.textContent.includes('#7 Claude Code'); })()").await?;
        // One click opens the agent's page live.
        until(&cdp, session, "Open live", "!document.querySelector('.bp-bar button.btn-sm:not([hidden])')?.hidden").await?;
        eval(&cdp, session, "[...document.querySelectorAll('.bp-bar button')].find((b) => b.textContent === 'Open live').click(), true").await?;
        until(&cdp, session, "a live tab again", "document.querySelector('.bp-tab.active:not(.remote)')").await?;
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
