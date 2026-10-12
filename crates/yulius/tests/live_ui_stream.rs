//! Opt-in client-side measurement of the streamed sandbox browser (SPEC-001 BRW-009,
//! AC-BRW-12/13; SPEC-002 ADR-15, F 2026-10-10): the real UI in a real Chrome watches the
//! sandbox Chrome on a virtual display, and a probe page paints its scroll position, pointer
//! position and an input counter as pixel rows, read back from every frame the panel draws. So
//! input → displayed frame is measured on the viewer's clock, as a user sees it.
//!
//! It scrolls with a trackpad-like stream of wheel events (60 per second, not waiting for each),
//! moves the pointer, drags and types, and checks that the panel keeps showing frames, that the
//! delay does not grow over a scroll (no backlog), and that no input is refused.
//!
//! Needs Linux with Xvfb and ffmpeg (the server Chrome on a display) and a Chrome for the viewer:
//!
//! ```sh
//! YULIUS_LIVE_DISPLAY=1 YULIUS_LIVE_CHROME=/usr/bin/google-chrome \
//!   cargo test -p yulius --test main live_ui_stream:: -- --ignored --nocapture
//! ```
//! `YULIUS_LIVE_DISPLAY_BASE` (default 90) is the first display number.
//! `YULIUS_LIVE_LINK=<kbit/s>,<one-way ms>` puts an emulated link between the viewer and the
//! server: the downlink at that rate behind a 1 MiB buffer (a home router's), both directions
//! delayed; `YULIUS_LIVE_REPORT_ONLY=1` prints the figures without asserting (to compare builds).
//! The same instrumentation measured the real path (a viewer in Poland, the VM in Germany) for the
//! PR of F 2026-10-10.

use crate::support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use serde_json::{Value, json};
use support::{
    HarnessOptions, TestResult, harness_with, perms, seed_github_user, seed_sandbox,
    setup_superadmin_creds,
};
use yulius::wiring::browser::ChromiumBrowsers;
use yulius_app::model::WebSessionRecord;
use yulius_app::ports::store::{SessionStore, UserStore};
use yulius_browser::{CdpClient, CdpConfig};
use yulius_contracts::browser::OpenParams;
use yulius_domain::ids::{SandboxId, UserId};

const PROBE_PAGE: &str = include_str!("support/stream_probe.html");
const PROBE_SCRIPT: &str = include_str!("support/stream_probe.js");

struct Chrome(Child);

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn start_viewer(
    exe: &str,
    profile: &Path,
) -> Result<(Chrome, String), Box<dyn std::error::Error>> {
    let child = Command::new(exe)
        .args([
            "--headless=new",
            "--no-sandbox",
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
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if eval(cdp, session, &format!("!!({expression})")).await? == Value::Bool(true) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// What the viewer recorded during one scenario.
struct Recorded {
    seconds: f64,
    /// (time, scroll, pointer x, pointer y, input counter) of each drawn frame whose probe read.
    draws: Vec<(f64, f64, f64, f64, f64)>,
    /// (time, kind, a, b) of the user's input.
    inputs: Vec<(f64, String, f64, f64)>,
    errors: Vec<String>,
}

impl Recorded {
    fn parse(data: &Value, seconds: f64) -> Self {
        let num = |v: &Value| v.as_f64().unwrap_or(f64::NAN);
        let draws = data["draws"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d[2] == json!(1))
            .map(|d| (num(&d[0]), num(&d[3]), num(&d[4]), num(&d[5]), num(&d[6])))
            .collect();
        let inputs = data["inputs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| {
                (
                    num(&i[0]),
                    i[1].as_str().unwrap_or_default().to_owned(),
                    num(&i[2]),
                    num(&i[3]),
                )
            })
            .collect();
        let errors = data["text"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t[2].as_str())
            .filter(|t| t.contains("\"error\""))
            .map(str::to_owned)
            .collect();
        Self {
            seconds,
            draws,
            inputs,
            errors,
        }
    }

    fn displayed_fps(&self) -> f64 {
        self.draws.len() as f64 / self.seconds
    }

    /// Input → first drawn frame showing it, in ms; and how many never showed.
    fn latencies(&self, kind: &str) -> (Vec<f64>, usize) {
        let mut out = Vec::new();
        let mut missed = 0;
        let events: Vec<_> = self.inputs.iter().filter(|i| i.1 == kind).collect();
        let Some(first) = events.first() else {
            return (out, 0);
        };
        let before = self.draws.iter().rev().find(|d| d.0 < first.0).copied();
        if before.is_none() && kind != "move" {
            // Without a frame before the first input the starting value is unknown.
            return (out, events.len());
        }
        let mut cumulative = 0.0;
        for (n, (t, _, a, b)) in events.iter().enumerate() {
            let hit = match kind {
                "wheel" => {
                    cumulative += a;
                    let target = before.map_or(0.0, |d| d.1) + cumulative;
                    self.draws.iter().find(|d| d.0 >= *t && d.1 >= target - 0.5)
                }
                "move" => self
                    .draws
                    .iter()
                    .find(|d| d.0 >= *t && d.2 >= *a && (d.3 - b).abs() <= 1.0),
                _ => {
                    let target = before.map_or(0.0, |d| d.4) + (n + 1) as f64;
                    self.draws.iter().find(|d| d.0 >= *t && d.4 >= target)
                }
            };
            match hit {
                Some(draw) => out.push(draw.0 - t),
                None => missed += 1,
            }
        }
        (out, missed)
    }
}

/// Checks a scenario's figures: frames shown, delay bounded and not growing, nothing refused.
/// An emulated link (`YULIUS_LIVE_LINK`): accepts on a loopback port and forwards to `target`,
/// delaying both directions by `delay` and limiting server → viewer to `rate` bytes per second
/// behind a bounded buffer (beyond it, TCP pushes back on the server, as a router's full queue
/// does). Ends with the test.
async fn emulated_link(
    target: std::net::SocketAddr,
    rate: f64,
    delay: Duration,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    const BUFFER: usize = 1 << 20;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    // One direction: chunks leave `delay` after they arrived, at most `rate` bytes per second.
    async fn pipe(
        mut from: tokio::net::tcp::OwnedReadHalf,
        mut to: tokio::net::tcp::OwnedWriteHalf,
        rate: f64,
        delay: Duration,
    ) {
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<(tokio::time::Instant, Vec<u8>)>(BUFFER / 4096);
        let reader = tokio::spawn(async move {
            let mut buffer = vec![0_u8; 4096];
            while let Ok(n) = from.read(&mut buffer).await {
                if n == 0
                    || tx
                        .send((tokio::time::Instant::now(), buffer[..n].to_vec()))
                        .await
                        .is_err()
                {
                    break;
                }
            }
        });
        let mut free_at = tokio::time::Instant::now();
        while let Some((arrived, chunk)) = rx.recv().await {
            let send_at = (arrived + delay).max(free_at);
            tokio::time::sleep_until(send_at).await;
            if rate.is_finite() {
                free_at = send_at + Duration::from_secs_f64(chunk.len() as f64 / rate);
            }
            if to.write_all(&chunk).await.is_err() {
                break;
            }
        }
        reader.abort();
    }
    let task = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let Ok(server) = tokio::net::TcpStream::connect(target).await else {
                continue;
            };
            let _ = (client.set_nodelay(true), server.set_nodelay(true));
            let (client_read, client_write) = client.into_split();
            let (server_read, server_write) = server.into_split();
            tokio::spawn(pipe(client_read, server_write, f64::INFINITY, delay));
            tokio::spawn(pipe(server_read, client_write, rate, delay));
        }
    });
    Ok((addr, task))
}

fn check(name: &str, recorded: &Recorded, kind: &str, min_fps: f64, max_p95: f64) -> TestResult {
    let (latencies, missed) = recorded.latencies(kind);
    let eighth = (latencies.len() / 8).max(1);
    let first = percentile(&latencies[..eighth.min(latencies.len())], 50.0);
    let last = percentile(&latencies[latencies.len().saturating_sub(eighth)..], 50.0);
    let (p50, p95) = (percentile(&latencies, 50.0), percentile(&latencies, 95.0));
    eprintln!(
        "{name}: displayed {:.1} fps; input → frame p50 {p50:.0} ms, p95 {p95:.0} ms (first eighth {first:.0}, last {last:.0}); {missed} of {} not shown",
        recorded.displayed_fps(),
        latencies.len() + missed
    );
    if std::env::var("YULIUS_LIVE_REPORT_ONLY").ok().as_deref() == Some("1") {
        return Ok(());
    }
    assert!(
        recorded.errors.is_empty(),
        "{name}: errors {:?}",
        recorded.errors
    );
    assert!(
        recorded.displayed_fps() >= min_fps,
        "{name}: {:.1} fps",
        recorded.displayed_fps()
    );
    assert!(
        missed * 10 <= latencies.len() + missed,
        "{name}: {missed} inputs never shown"
    );
    assert!(p95 <= max_p95, "{name}: p95 {p95:.0} ms");
    assert!(
        last <= first * 2.0 + 150.0,
        "{name}: the delay grew from {first:.0} to {last:.0} ms"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs Linux with Xvfb, ffmpeg and Chrome (YULIUS_LIVE_DISPLAY=1, YULIUS_LIVE_CHROME)"]
async fn the_panel_keeps_up_with_scrolling_moving_and_typing() -> TestResult {
    let (Ok(chrome_exe), Some("1")) = (
        std::env::var("YULIUS_LIVE_CHROME"),
        std::env::var("YULIUS_LIVE_DISPLAY").ok().as_deref(),
    ) else {
        eprintln!("skipped: set YULIUS_LIVE_DISPLAY=1 and YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_default())
        .with_test_writer()
        .try_init();
    let ui_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let server_addr = ui_listener.local_addr()?;
    // The viewer reaches the server directly or through an emulated link.
    let link = std::env::var("YULIUS_LIVE_LINK").ok().and_then(|spec| {
        let (kbps, ms) = spec.split_once(',')?;
        Some((
            kbps.trim().parse::<f64>().ok()? * 1000.0 / 8.0,
            Duration::from_millis(ms.trim().parse().ok()?),
        ))
    });
    let (addr, link_task) = match link {
        Some((rate, delay)) => {
            eprintln!(
                "emulated link: {:.0} kbit/s down, {} ms each way",
                rate * 8.0 / 1000.0,
                delay.as_millis()
            );
            let (addr, task) = emulated_link(server_addr, rate, delay).await?;
            (addr, Some(task))
        }
        None => (server_addr, None),
    };
    let dir = tempfile::Builder::new().prefix("yls").tempdir()?;
    let mut config =
        yulius_browser::BrowserConfig::new(PathBuf::from(&chrome_exe), dir.path().join("profiles"));
    let mut display = yulius_browser::DisplayConfig::new(dir.path().join("displays"));
    display.first_display = std::env::var("YULIUS_LIVE_DISPLAY_BASE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90);
    config.display = yulius_browser::DisplayMode::Virtual(display);
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
    let cookie = "live-ui-stream-session";
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
    let router = h.router.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(ui_listener, router).await;
    });
    let site_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let site_port = site_listener.local_addr()?.port();
    let site = tokio::spawn(async move {
        let app = Router::new().route("/", get(|| async { axum::response::Html(PROBE_PAGE) }));
        let _ = axum::serve(site_listener, app).await;
    });

    let result = async {
        let profile = tempfile::tempdir()?;
        let (_viewer, ws_url) = start_viewer(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" })).await?;
        let attached = cdp
            .call("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }))
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let s = session.as_str();
        cdp.call_session(s, "Page.enable", json!({})).await?;
        cdp.call_session(s, "Page.addScriptToEvaluateOnNewDocument", json!({ "source": PROBE_SCRIPT })).await?;
        cdp.call_session(s, "Network.enable", json!({})).await?;
        cdp.call_session(s, "Network.setCookie", json!({ "name": "yulius_session", "value": cookie, "url": format!("http://{addr}/") }))
            .await?;
        cdp.call_session(s, "Page.navigate", json!({ "url": format!("http://{addr}/s/fixture") })).await?;
        until(&cdp, s, "the workspace", "document.querySelector('.tools-toggle')").await?;
        eval(&cdp, s, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Browser\"]').click(); true").await?;
        until(&cdp, s, "the panel", "document.querySelector('button[aria-label=\"Maximize panel\"]')").await?;
        eval(&cdp, s, "document.querySelector('button[aria-label=\"Maximize panel\"]').click(), true").await?;
        until(&cdp, s, "the first picture", "document.querySelector('.bp-overlay').hidden && document.querySelector('.bp-canvas').width > 0").await?;
        assert_eq!(
            eval(&cdp, s, "document.querySelector('.bp-viewchip').hidden").await?,
            json!(true),
            "the panel decodes video"
        );
        let agent = h.app.browser_session(&SandboxId::parse("s1")?).await?;
        agent.tool_open(&OpenParams { url: format!("http://localhost:{site_port}/") }).await?;
        until(&cdp, s, "video at the panel size", "(() => { const c = document.querySelector('.bp-canvas'); const v = document.querySelector('.bp-view').getBoundingClientRect(); return Math.abs(c.width - Math.round(v.width)) <= 1; })()").await?;
        eval(&cdp, s, "window.__perf.take(), true").await?;
        until(&cdp, s, "the probe in the video", "window.__perf.draws.some((d) => d[2] === 1)").await?;
        let stage = eval(&cdp, s, "(() => { const c = document.querySelector('.bp-canvas'); const r = c.getBoundingClientRect(); return { left: r.left, top: r.top, fit: r.width / c.width, vw: c.width, vh: c.height }; })()").await?;
        let num = |key: &str| stage[key].as_f64().unwrap_or(0.0);
        let (left, top, fit, vw, vh) = (num("left"), num("top"), num("fit"), num("vw"), num("vh"));
        eprintln!("panel {vw}×{vh} at fit {fit:.2}");
        let at = move |x: f64, y: f64| (left + x * fit, top + y * fit);

        // Runs `count` input events every 16 ms without waiting for each (as a trackpad does).
        let paced = |events: Vec<Value>| {
            let cdp = cdp.clone();
            let session = session.clone();
            async move {
                let started = tokio::time::Instant::now();
                let mut calls = Vec::new();
                for (n, params) in events.into_iter().enumerate() {
                    let cdp = cdp.clone();
                    let session = session.clone();
                    calls.push(tokio::spawn(async move {
                        cdp.call_session(&session, "Input.dispatchMouseEvent", params).await
                    }));
                    tokio::time::sleep_until(started + Duration::from_millis(16 * (n as u64 + 1))).await;
                }
                for call in calls {
                    let _ = call.await;
                }
            }
        };
        let take = |seconds: f64| {
            let cdp = cdp.clone();
            let session = session.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                let data = eval(&cdp, &session, "window.__perf.take()").await?;
                Ok::<_, Box<dyn std::error::Error>>(Recorded::parse(&data, seconds))
            }
        };

        // Scrolling: 6 s of 50 px wheel turns.
        let (x, y) = at(vw * 0.45, vh * 0.6);
        let wheel: Vec<Value> = (0..375).map(|_| json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": 50 })).collect();
        let started = Instant::now();
        paced(wheel).await;
        let scroll = take(started.elapsed().as_secs_f64()).await?;
        check("scroll", &scroll, "wheel", 30.0, 800.0)?;

        // Pointer moves along a row.
        let step = ((vw - 120.0) / 300.0).clamp(1.0, 3.0).floor();
        let row = (vh - 40.0).min(420.0);
        let moves: Vec<Value> = (0..300)
            .map(|n| {
                let (x, y) = at(60.0 + f64::from(n) * step, row);
                json!({ "type": "mouseMoved", "x": x, "y": y })
            })
            .collect();
        let started = Instant::now();
        paced(moves).await;
        let moved = take(started.elapsed().as_secs_f64()).await?;
        check("move", &moved, "move", 20.0, 600.0)?;

        // Typing into the field.
        let (fx, fy) = at(950.0_f64.min(vw - 40.0), 132.0);
        for kind in ["mousePressed", "mouseReleased"] {
            cdp.call_session(s, "Input.dispatchMouseEvent", json!({ "type": kind, "x": fx, "y": fy, "button": "left", "clickCount": 1 })).await?;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        eval(&cdp, s, "window.__perf.take(), true").await?;
        // Frames before the first key give the counter's starting value.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let started = Instant::now();
        for n in 0..20 {
            let key = ["a", "b", "c", "d"][n % 4];
            let code = format!("Key{}", key.to_uppercase());
            cdp.call_session(s, "Input.dispatchKeyEvent", json!({ "type": "keyDown", "key": key, "code": code, "text": key })).await?;
            cdp.call_session(s, "Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": key, "code": code })).await?;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let typed = take(started.elapsed().as_secs_f64()).await?;
        check("keys", &typed, "key", 5.0, 500.0)?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    h.app.stop_workspace_services().await;
    server.abort();
    site.abort();
    if let Some(task) = link_task {
        task.abort();
    }
    result
}
