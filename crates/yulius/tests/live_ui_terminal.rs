//! Opt-in live measurement of how a task's terminal appears (F 2026-10-09: "when I open Claude
//! the font size changes 3 times and it loads for a while"), and of the Shell panel opening a
//! working terminal without a click (UI-016). Real server, real detached ptyhost, the fake agent
//! drawing full screen like Claude Code (`--tui`: redrawn in full on every resize) and the real UI
//! in headless Chrome over an emulated remote connection (80 ms round trips).
//!
//! A recorder injected before the page loads samples the task terminal on every animation frame:
//! font size, whether the terminal web font is loaded, the grid (rows, screen width and height),
//! the terminal's place on the page, the agent's own drawing width (its top rule) and whether
//! the terminal is visible. It also sums the page's layout shifts. A visible change is a jump the
//! user sees; the test requires none after the terminal first shows the agent, on a cold first
//! open (nothing cached, the PTY at the size the task started with) and on a warm reload.
//!
//! ```sh
//! YULIUS_LIVE_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   cargo test -p yulius --test live_ui_terminal -- --ignored --nocapture
//! ```

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{
    Creds, Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox, setup_superadmin_creds,
};
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::model::{AccountRecord, AccountStatus, WebSessionRecord};
use yulius_app::ports::store::{AccountStore, SessionStore, UserStore};
use yulius_browser::{CdpClient, CdpConfig};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, UserId};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

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

/// Evaluates `expression` until it returns `true` or `timeout` passes.
async fn until(
    cdp: &CdpClient,
    session: &str,
    what: &str,
    expression: &str,
    timeout: Duration,
) -> TestResult {
    let deadline = Instant::now() + timeout;
    loop {
        if eval(cdp, session, &format!("!!({expression})")).await? == Value::Bool(true) {
            return Ok(());
        }
        if Instant::now() > deadline {
            let text = eval(cdp, session, "document.body.innerText.slice(0, 800)").await?;
            let log = eval(
                cdp,
                session,
                "JSON.stringify((window.__termRec && window.__termRec.log) || [])",
            )
            .await?;
            return Err(format!("timed out waiting for {what}; page: {text}; log: {log}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

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

/// Samples the task terminal on every animation frame (see the module docs); `__termRec.report()`
/// summarises what the user saw.
const RECORDER: &str = r#"
(() => {
  const rec = { samples: [], shifts: [] };
  window.__termRec = rec;
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) rec.shifts.push({ t: Math.round(e.startTime), v: e.value, recent: e.hadRecentInput });
    }).observe({ type: 'layout-shift', buffered: true });
  } catch (e) { /* no layout-instability API */ }
  // API answers and terminal socket events, for diagnosing a failed wait.
  rec.log = [];
  const nativeFetch = window.fetch;
  window.fetch = (input, init) => nativeFetch(input, init).then((response) => {
    const url = String(input && input.url ? input.url : input);
    if (url.includes('/api/')) rec.log.push(`${Math.round(performance.now())} ${(init && init.method) || 'GET'} ${url.replace(/^.*\/api\//, '/api/')} ${response.status}`);
    return response;
  });
  // When terminal sockets open and first answer (relative to navigation start).
  rec.sockets = [];
  const NativeSocket = window.WebSocket;
  window.WebSocket = function (url, protocols) {
    const ws = protocols === undefined ? new NativeSocket(url) : new NativeSocket(url, protocols);
    if (/\/terminal/.test(String(url))) {
      const entry = { url: String(url).replace(/^.*\/api\//, '/api/'), created: Math.round(performance.now()) };
      rec.sockets.push(entry);
      ws.addEventListener('open', () => { entry.open = Math.round(performance.now()); });
      ws.addEventListener('message', (event) => {
        if (entry.first === undefined) entry.first = Math.round(performance.now());
        if (typeof event.data === 'string') rec.log.push(`${Math.round(performance.now())} ws ${entry.url} ${event.data.slice(0, 160)}`);
      });
      ws.addEventListener('close', (event) => rec.log.push(`${Math.round(performance.now())} ws close ${entry.url} ${event.code}`));
      // Resizes the viewer asks for after connecting (each one makes the agent redraw).
      entry.resizes = 0;
      const send = ws.send.bind(ws);
      ws.send = (data) => {
        if (typeof data === 'string' && data.includes('"resize"')) { entry.resizes += 1; rec.log.push(`${Math.round(performance.now())} ws send ${data}`); }
        return send(data);
      };
    }
    return ws;
  };
  window.WebSocket.prototype = NativeSocket.prototype;
  Object.assign(window.WebSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
  const plexLoaded = () => [...document.fonts].some((f) => f.family.replace(/["']/g, '') === 'IBM Plex Mono' && String(f.weight) === '400' && f.status === 'loaded' && /U\+0+-FF/i.test(f.unicodeRange));
  let last = '';
  function sample() {
    const view = document.querySelector('.taskview');
    const rows = view && view.querySelector('.xterm-rows');
    const screen = view && view.querySelector('.xterm-screen');
    let s = { present: false };
    if (rows && screen) {
      const host = screen.closest('.term-host') || screen;
      const box = host.getBoundingClientRect();
      const cover = view.querySelector('.term-skeleton');
      const covered = Boolean(cover && cover.checkVisibility({ opacityProperty: true, visibilityProperty: true }) && getComputedStyle(cover).opacity > 0.5);
      const visible = screen.checkVisibility({ opacityProperty: true, visibilityProperty: true }) && Number(getComputedStyle(host).opacity) > 0.5 && !covered;
      const first = rows.firstElementChild ? rows.firstElementChild.textContent.replace(/\s+$/, '') : '';
      s = {
        present: true,
        visible,
        text: rows.textContent.includes('Fake Claude Code'),
        fs: getComputedStyle(rows).fontSize,
        plex: plexLoaded(),
        nrows: rows.childElementCount,
        w: Math.round(screen.offsetWidth),
        h: Math.round(screen.offsetHeight),
        top: Math.round(box.top),
        hostH: Math.round(box.height),
        rule: first.startsWith('╭') ? first.length : 0,
      };
    }
    const key = JSON.stringify(s);
    if (key !== last) { last = key; rec.samples.push({ t: Math.round(performance.now()), ...s }); }
    requestAnimationFrame(sample);
  }
  requestAnimationFrame(sample);
  rec.report = () => {
    const shown = rec.samples.findIndex((x) => x.present && x.visible && x.text);
    const facts = (x) => JSON.stringify([x.fs, x.plex, x.nrows, x.w, x.h, x.top, x.hostH, x.rule]);
    let changes = 0;
    let stable = shown >= 0 ? rec.samples[shown].t : null;
    const fonts = new Set();
    const seen = [];
    if (shown >= 0) {
      let prev = facts(rec.samples[shown]);
      fonts.add(rec.samples[shown].fs);
      seen.push(rec.samples[shown]);
      for (const x of rec.samples.slice(shown + 1)) {
        if (!x.present || !x.visible) continue;
        const f = facts(x);
        if (f !== prev) { changes += 1; stable = x.t; prev = f; fonts.add(x.fs); seen.push(x); }
      }
    }
    // Font sizes the terminal showed at all, before the agent's text appeared too.
    const everFonts = [...new Set(rec.samples.filter((x) => x.present && x.visible).map((x) => x.fs))];
    return {
      first_visible_ms: shown >= 0 ? rec.samples[shown].t : null,
      stable_ms: stable,
      visible_changes: changes,
      font_sizes_after_first_frame: [...fonts],
      font_sizes_shown: everFonts,
      web_font_at_first_frame: shown >= 0 ? rec.samples[shown].plex : null,
      layout_shift: Number(rec.shifts.filter((s) => !s.recent).reduce((a, s) => a + s.v, 0).toFixed(4)),
      frames: seen,
      sockets: rec.sockets,
      // The slowest requests: [path, start ms, end ms, bytes].
      slowest: performance.getEntriesByType('resource')
        .map((r) => [r.name.replace(/^https?:\/\/[^/]+/, ''), Math.round(r.startTime), Math.round(r.responseEnd), r.transferSize])
        .sort((a, b) => b[2] - a[2]).slice(0, 8),
      timeline: (() => { const n = performance.getEntriesByType('navigation')[0]; return [['document', 0, n ? Math.round(n.responseEnd) : null], ...performance.getEntriesByType('resource').map((r) => [r.name.replace(/^https?:\/\/[^/]+/, '').replace('/assets/', ''), Math.round(r.startTime), Math.round(r.responseEnd)])].map((x) => x.join(' ')).join(' | '); })(),
      requests: performance.getEntriesByType('resource').length,
      transferred: performance.getEntriesByType('resource').reduce((a, r) => a + (r.transferSize || 0), 0),
    };
  };
})();
"#;

struct Fixture {
    h: Harness,
    writer: Creds,
    cookie: &'static str,
    addr: std::net::SocketAddr,
    listener: Option<tokio::net::TcpListener>,
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let dir = tempfile::Builder::new().prefix("ylt").tempdir_in("/tmp")?;
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host: Some(Arc::new(PtySessionHost::new(exe.clone()))),
        harness: Some(Arc::new(FakeAgentHarness::new(
            exe,
            fake,
            FakeAgentOptions {
                tui: true,
                ..FakeAgentOptions::default()
            },
        ))),
        dir: Some(dir),
        configure: Some(Box::new(move |c| {
            c.contained = false;
            c.allowed_origins.push(format!("http://{addr}"));
        })),
        ..HarnessOptions::default()
    })?;
    assert!(h.app.set_shell_program(PathBuf::from("/bin/sh")));
    setup_superadmin_creds(&h).await?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let sandbox = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a1")?,
            provider: Provider::Anthropic,
            label: "Claude".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-live-terminal")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
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
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-ui-terminal-session";
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
    Ok(Fixture {
        h,
        writer,
        cookie,
        addr,
        listener: Some(listener),
    })
}

/// A Chrome tab with the session cookie, an emulated remote connection and the recorder.
async fn open_tab(
    f: &Fixture,
    chrome_exe: &str,
    profile: &Path,
) -> Result<(Chrome, CdpClient, String), Box<dyn std::error::Error>> {
    let (chrome, ws_url) = start_chrome(chrome_exe, profile).await?;
    let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
    let target = cdp
        .call("Target.createTarget", json!({ "url": "about:blank" }))
        .await?;
    let target_id = target["targetId"].as_str().ok_or("target")?;
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await?;
    let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
    cdp.call_session(&session, "Network.enable", json!({}))
        .await?;
    cdp.call_session(&session, "Page.enable", json!({})).await?;
    // A remote server: 80 ms round trips (throughput not throttled; YULIUS_LIVE_THROUGHPUT
    // sets a limit in bytes per second).
    let throughput: f64 = std::env::var("YULIUS_LIVE_THROUGHPUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.0);
    cdp.call_session(
        &session,
        "Network.emulateNetworkConditions",
        json!({ "offline": false, "latency": 80, "downloadThroughput": throughput, "uploadThroughput": throughput }),
    )
    .await?;
    cdp.call_session(
        &session,
        "Network.setCookie",
        json!({ "name": "yulius_session", "value": f.cookie, "url": format!("http://{}/", f.addr) }),
    )
    .await?;
    cdp.call_session(
        &session,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": RECORDER }),
    )
    .await?;
    Ok((chrome, cdp, session))
}

/// Waits for the agent's drawing in the visible terminal, then for `settle` more, and returns
/// the recorder's report.
async fn measure(
    cdp: &CdpClient,
    session: &str,
    label: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    until(
        cdp,
        session,
        &format!("the agent in the terminal ({label})"),
        "window.__termRec && window.__termRec.report().first_visible_ms !== null",
        Duration::from_secs(40),
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let report = eval(cdp, session, "window.__termRec.report()").await?;
    eprintln!(
        "[live_ui_terminal] {label}: {}",
        serde_json::to_string_pretty(&report)?
    );
    Ok(report)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn the_task_terminal_appears_once_at_its_final_size_and_font() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let mut f = fixture().await?;
    let listener = f.listener.take().ok_or("listener")?;
    let router = f.h.router.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let result = async {
        // The task starts before anyone looks: its PTY has the size tasks start with.
        let created = call(
            &f.h.router,
            axum::http::Method::POST,
            "/api/sandboxes/s1/tasks",
            Some(json!({"prompt": "Settle check", "harness": "claude"})),
            &f.writer,
        )
        .await?;
        assert_eq!(created.status, 201, "{:?}", created.body);
        let number = created.body["number"].as_u64().ok_or("number")?;
        let tid = created.body["id"].as_str().ok_or("id")?.to_owned();
        // Measure the view, not the agent's start: wait until the agent has drawn (a watcher that
        // sends no hello, so the PTY keeps the size the task started with).
        let mut watcher = support::ws_connect(
            f.addr,
            &format!("/api/tasks/{tid}/terminal"),
            &f.writer,
            "https://yulius.test",
        )
        .await?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seen = String::new();
        while !seen.contains("Fake Claude Code") {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, futures::StreamExt::next(&mut watcher)).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes)))) => {
                    seen.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(Ok(_))) => {}
                _ => return Err(format!("the agent did not draw: {seen:?}").into()),
            }
        }
        drop(watcher);
        let profile = tempfile::tempdir()?;
        let (_chrome, cdp, session) = open_tab(&f, &chrome_exe, profile.path()).await?;
        let url = format!("http://{}/s/fixture/t/{number}", f.addr);
        cdp.call_session(&session, "Page.navigate", json!({ "url": url }))
            .await?;
        let cold = measure(&cdp, &session, "cold open").await?;
        cdp.call_session(&session, "Page.reload", json!({})).await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let warm = measure(&cdp, &session, "warm reload").await?;
        // Focus lands in the terminal.
        let focused = eval(
            &cdp,
            &session,
            "document.activeElement && document.activeElement.classList.contains('xterm-helper-textarea')",
        )
        .await?;
        for (label, report) in [("cold open", &cold), ("warm reload", &warm)] {
            assert_eq!(
                report["visible_changes"],
                json!(0),
                "{label}: the terminal jumped after it appeared: {report}"
            );
            assert_eq!(
                report["font_sizes_shown"].as_array().map(Vec::len),
                Some(1),
                "{label}: one font size: {report}"
            );
            assert_eq!(report["web_font_at_first_frame"], json!(true), "{label}");
            assert_eq!(
                report["sockets"][0]["resizes"],
                json!(0),
                "{label}: the size travelled with the connection; no resize after it: {report}"
            );
        }
        assert_eq!(focused, json!(true), "focus is in the terminal");
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server.abort();
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn the_shell_panel_opens_a_working_terminal_without_a_click() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let mut f = fixture().await?;
    let listener = f.listener.take().ok_or("listener")?;
    let router = f.h.router.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let result = async {
        let profile = tempfile::tempdir()?;
        let (_chrome, cdp, session) = open_tab(&f, &chrome_exe, profile.path()).await?;
        let url = format!("http://{}/s/fixture", f.addr);
        cdp.call_session(&session, "Page.navigate", json!({ "url": url }))
            .await?;
        let wait = Duration::from_secs(40);
        until(&cdp, &session, "the workspace", "document.querySelector('.tools-toggle')", wait).await?;
        let opened = Instant::now();
        eval(&cdp, &session, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Shell\"]').click(); true").await?;
        // No "Open shell" button: a terminal tab with a live prompt appears by itself.
        until(&cdp, &session, "a shell tab", "document.querySelector('.panel [role=tab][aria-selected=true]')", wait).await?;
        until(&cdp, &session, "the shell prompt", "[...document.querySelectorAll('.panel .xterm-rows')].some((r) => /[$#] *$/m.test(r.textContent.trim()) || r.textContent.includes('$'))", wait).await?;
        eprintln!(
            "[live_ui_terminal] shell panel: prompt after {} ms; {}",
            opened.elapsed().as_millis(),
            eval(&cdp, &session, "JSON.stringify(window.__termRec.log)").await?
        );
        assert_eq!(
            eval(&cdp, &session, "[...document.querySelectorAll('.panel button')].some((b) => b.textContent.trim() === 'Open shell')").await?,
            json!(false),
            "no Open shell button"
        );
        // It works: type a command and read its output.
        eval(&cdp, &session, "document.querySelector('.panel .xterm-helper-textarea').focus(); true").await?;
        cdp.call_session(&session, "Input.insertText", json!({ "text": "echo sum=$((6*7))" }))
            .await?;
        for kind in ["keyDown", "keyUp"] {
            cdp.call_session(
                &session,
                "Input.dispatchKeyEvent",
                json!({ "type": kind, "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": if kind == "keyDown" { "\r" } else { "" } }),
            )
            .await?;
        }
        until(&cdp, &session, "the command's output", "[...document.querySelectorAll('.panel .xterm-rows')].some((r) => r.textContent.includes('sum=42'))", wait).await?;
        // "+" opens a second terminal in its own tab.
        eval(&cdp, &session, "document.querySelector('.panel button[aria-label=\"New terminal\"]').click(); true").await?;
        until(&cdp, &session, "a second tab", "document.querySelectorAll('.panel [role=tab]').length === 2", wait).await?;
        let listed = call(
            &f.h.router,
            axum::http::Method::GET,
            "/api/sandboxes/s1/shells",
            None,
            &f.writer,
        )
        .await?;
        assert_eq!(listed.body.as_array().map(Vec::len), Some(2), "{:?}", listed.body);
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    f.h.app.end_all_shells().await;
    f.h.app.stop_workspace_services().await;
    server.abort();
    result
}
