//! Opt-in browser check of the DOM-mirrored sandbox browser (SPEC-001 BRW-005..007): a real
//! server-side Chromium (the sandbox browser) and a second headless Chrome as the user's
//! browser running the real UI. The new tab shows the welcome page as DOM with a link to the
//! sandbox port that listens; following it mirrors the page served on the VM's loopback with its
//! stylesheet and image (loaded through Yulius) and a canvas that keeps updating; a click in the
//! mirror reaches the real page; text in the mirror selects like a page (double-click and drag,
//! copied at once); typing into a field reaches the real page and streams back; an
//! agent's click shows its cursor; the pixel view can be chosen
//! and left; and a hostile page that feeds crafted events to the recorder's binding cannot run
//! script in the UI (the mirror iframe stays script-free). A second test checks
//! the terminal (F 2026-10-07): its text is HTML and a double-click selects a word even after
//! the program switched on mouse tracking; and that links to the VM are told apart from others.
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
async fn the_browser_panel_mirrors_pages_as_dom() -> TestResult {
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
        // A fresh browser opens the welcome page, mirrored as DOM.
        let mirror = "document.querySelector('.bp-mirror iframe')?.contentDocument";
        until(&cdp, session, "the welcome page in the mirror", &format!("{mirror}?.body?.innerText.includes('Browser of fixture')")).await?;
        let sandbox = eval(&cdp, session, "document.querySelector('.bp-mirror iframe').getAttribute('sandbox')").await?;
        assert_eq!(sandbox, json!("allow-same-origin"), "the mirror iframe never runs scripts");
        // The welcome page links the listening port; following the link in the mirror opens
        // the page served on the VM's loopback, with its stylesheet and image.
        let link = format!("a.port[href='http://localhost:{port}/']");
        until(&cdp, session, "the port link on the welcome page", &format!("{mirror}?.querySelector(\"{link}\")")).await?;
        click_in_mirror(&cdp, session, &link).await?;
        until(&cdp, session, "the fixture page in the mirror", &format!("{mirror}?.body?.innerText.includes('Fixture page')")).await?;
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
