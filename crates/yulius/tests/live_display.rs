//! Opt-in checks of the streamed sandbox browser on Linux with Xvfb, ffmpeg and a real Chrome
//! (SPEC-002 ADR-15; SPEC-001 BRW-004..008, AC-BRW-9..12). They run the whole server path — the
//! viewer WebSocket, the virtual display, the encoder, XTest — and read the page back over CDP
//! and the video back through ffmpeg:
//!
//! - the stream starts with a decoder configuration and a keyframe;
//! - a click, typing (ASCII, a Polish letter, a shifted character) and the wheel reach the page as
//!   trusted events, and the change shows in the decoded video;
//! - an image loads and a canvas animates in the video;
//! - the agent's click reaches viewers as its cursor; `navigator.webdriver` is false;
//! - Yulius refuses to open itself (BRW-008);
//! - a contained process (the agents' and users' bubblewrap) reaches no display: no socket, no
//!   cookie, no abstract socket, no TCP; another sandbox's cookie does not open a display.
//!
//! `measures_*` prints the performance figures of the ADR (fps while scrolling, input-to-frame
//! delay, encoder CPU, bandwidth).
//!
//! ```sh
//! YULIUS_LIVE_DISPLAY=1 YULIUS_LIVE_CHROME=/usr/bin/google-chrome \
//!   cargo test -p yulius --test live_display -- --ignored --nocapture --test-threads 1
//! ```
//! `YULIUS_LIVE_DISPLAY_BASE` (default 90) is the first display number used.

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::{
    Harness, HarnessOptions, TestResult, harness_with, mcp_tool, perms, seed_github_user,
    seed_live_attempt, seed_sandbox, seed_task, serve, tool_text, ws_connect,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius::wiring::browser::ChromiumBrowsers;
use yulius_browser::{
    BrowserConfig, CdpClient, CdpConfig, DisplayConfig, DisplayMode, UrlDenyList,
};
use yulius_contracts::browser::decode_video_frame;
use yulius_domain::ids::SandboxId;
use yulius_domain::task::TaskStatus;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const ORIGIN: &str = "https://yulius.test";
const TOKEN: &str = "live-display-task-token-0123456789";
const WAIT: Duration = Duration::from_secs(30);

/// The Chrome to use when the opt-in is set and the prerequisites exist.
fn live_chrome() -> Option<PathBuf> {
    if std::env::var("YULIUS_LIVE_DISPLAY").ok().as_deref() != Some("1") {
        eprintln!("skipped: set YULIUS_LIVE_DISPLAY=1 and YULIUS_LIVE_CHROME");
        return None;
    }
    let chrome = PathBuf::from(std::env::var_os("YULIUS_LIVE_CHROME")?);
    for program in ["Xvfb", "ffmpeg", "xdpyinfo"] {
        if yulius_browser::resolve_executable(Path::new(program)).is_err() {
            eprintln!("skipped: {program} is not installed");
            return None;
        }
    }
    Some(chrome)
}

fn display_base() -> u32 {
    std::env::var("YULIUS_LIVE_DISPLAY_BASE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90)
}

const FIXTURE: &str = r#"<!doctype html><html><head><title>fixture</title><style>
html, body { margin: 0; background: #fff; font: 20px sans-serif; }
#box { position: fixed; left: 0; top: 0; width: 200px; height: 200px; background: rgb(0, 0, 255); }
#btn { position: absolute; left: 300px; top: 40px; width: 200px; height: 80px; font-size: 24px; }
#field { position: absolute; left: 300px; top: 150px; width: 300px; font-size: 24px; }
#canvas { position: absolute; left: 300px; top: 220px; }
#img { position: absolute; left: 620px; top: 40px; }
#tall { height: 6000px; }
</style></head><body>
<div id="box"></div><button id="btn">Click me</button><input id="field" autocomplete="off">
<canvas id="canvas" width="200" height="100"></canvas><img id="img" src="/img.svg" width="100" height="100">
<div id="tall"></div>
<script>
window.events = [];
document.getElementById('btn').addEventListener('click', (e) => {
  events.push({ type: 'click', trusted: e.isTrusted, x: e.clientX, y: e.clientY });
  document.getElementById('box').style.background = 'rgb(255, 0, 0)';
});
addEventListener('keydown', (e) => events.push({ type: 'keydown', key: e.key, trusted: e.isTrusted }));
let n = 0;
const c = document.getElementById('canvas').getContext('2d');
setInterval(() => { c.fillStyle = n++ % 2 ? 'rgb(0, 200, 0)' : 'rgb(200, 0, 200)'; c.fillRect(0, 0, 200, 100); }, 250);
</script></body></html>"#;

/// A page whose whole background flips between black and white on every key press (the input
/// to frame measurement) and that scrolls (the fps measurement).
const FLIP: &str = r#"<!doctype html><html><head><title>flip</title><style>
html, body { margin: 0; background: #fff; font: 16px/1.5 sans-serif; }
.row { height: 120px; border-bottom: 1px solid #ccc; padding: 8px; }
</style></head><body><div id="rows"></div><script>
const rows = document.getElementById('rows');
for (let i = 0; i < 400; i += 1) {
  const d = document.createElement('div');
  d.className = 'row';
  d.textContent = 'Row ' + i + ' lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor';
  d.style.background = 'hsl(' + (i * 37 % 360) + ', 60%, 85%)';
  rows.append(d);
}
addEventListener('keydown', () => {
  const black = document.body.dataset.black === '1';
  document.body.dataset.black = black ? '0' : '1';
  document.documentElement.style.filter = black ? '' : 'invert(1)';
});
</script></body></html>"#;

const IMAGE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="rgb(255,200,0)"/></svg>"#;

fn fixture_site() -> Router {
    Router::new()
        .route("/", get(|| async { axum::response::Html(FIXTURE) }))
        .route("/flip", get(|| async { axum::response::Html(FLIP) }))
        .route(
            "/img.svg",
            get(|| async { ([("content-type", "image/svg+xml")], IMAGE) }),
        )
}

struct Live {
    h: Harness,
    served: support::Served,
    site: SocketAddr,
    writer: support::Creds,
    runtime: PathBuf,
    data: PathBuf,
    _site_task: tokio::task::JoinHandle<()>,
}

async fn live(chrome: &Path) -> Result<Live, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let data = dir.path().canonicalize()?;
    let runtime = data.join("displays");
    let mut display = DisplayConfig::new(runtime.clone());
    display.first_display = display_base();
    let mut config = BrowserConfig::new(chrome.to_path_buf(), data.join("unused"));
    config.display = DisplayMode::Virtual(display);
    config.startup_timeout = Duration::from_secs(60);
    config.deny = UrlDenyList::yulius(ORIGIN, &[], Vec::new());
    let h = harness_with(HarnessOptions {
        browser: Some(Arc::new(ChromiumBrowsers::new(config))),
        dir: Some(dir),
        ..HarnessOptions::default()
    })?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    for repo in ["acme/fixture", "acme/other"] {
        h.github.set_perm("alice", repo, perms("write"));
    }
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    let served = serve(h.router.clone()).await?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let site = listener.local_addr()?;
    let site_task = tokio::spawn(async move {
        let _ = axum::serve(listener, fixture_site()).await;
    });
    Ok(Live {
        h,
        served,
        site,
        writer,
        runtime,
        data,
        _site_task: site_task,
    })
}

/// A viewer: its socket, the video it received and its JSON messages.
struct Viewer {
    socket: Socket,
    /// The decoder configuration (`avcC`) and coded size.
    config: Option<(Vec<u8>, u32, u32)>,
    /// The frames since the last keyframe.
    gop: Vec<Vec<u8>>,
    /// Arrival time and size of every frame.
    arrivals: Vec<(Instant, usize, bool)>,
    tabs: Value,
}

impl Viewer {
    async fn connect(live: &Live, sandbox: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let path = format!("/api/sandboxes/{sandbox}/browser");
        let socket = ws_connect(live.served.addr, &path, &live.writer, ORIGIN).await?;
        Ok(Self {
            socket,
            config: None,
            gop: Vec::new(),
            arrivals: Vec::new(),
            tabs: Value::Null,
        })
    }

    async fn send(&mut self, value: Value) -> TestResult {
        self.socket.send(Message::text(value.to_string())).await?;
        Ok(())
    }

    /// Reads one message (recording video and tabs); `None` on timeout.
    async fn step(&mut self, until: Instant) -> Result<Option<Value>, Box<dyn std::error::Error>> {
        let left = until.saturating_duration_since(Instant::now());
        let Ok(message) = tokio::time::timeout(left, self.socket.next()).await else {
            return Ok(None);
        };
        match message.ok_or("socket closed")?? {
            Message::Binary(bytes) => {
                if let Ok(frame) = decode_video_frame(&bytes) {
                    if frame.keyframe {
                        self.gop.clear();
                    }
                    self.gop.push(frame.data.to_vec());
                    self.arrivals
                        .push((Instant::now(), frame.data.len(), frame.keyframe));
                }
                Ok(Some(json!({"type": "binary"})))
            }
            Message::Text(text) => {
                let value: Value = serde_json::from_str(text.as_str())?;
                if value["type"] == "video_config" {
                    use base64::Engine as _;
                    let description = base64::engine::general_purpose::STANDARD
                        .decode(value["description"].as_str().unwrap_or_default())?;
                    let width = value["width"].as_u64().unwrap_or(0) as u32;
                    let height = value["height"].as_u64().unwrap_or(0) as u32;
                    self.config = Some((description, width, height));
                    self.gop.clear();
                }
                if value["type"] == "tabs" {
                    self.tabs = value.clone();
                }
                Ok(Some(value))
            }
            Message::Close(frame) => Err(format!("closed: {frame:?}").into()),
            _ => Ok(Some(Value::Null)),
        }
    }

    async fn wait_json(
        &mut self,
        what: &str,
        predicate: impl Fn(&Value) -> bool,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let until = Instant::now() + WAIT;
        loop {
            match self.step(until).await? {
                Some(value) if predicate(&value) => return Ok(value),
                Some(_) => {}
                None => return Err(format!("timed out waiting for {what}").into()),
            }
        }
    }

    /// Reads messages for `duration` (keeps the video current).
    async fn pump(&mut self, duration: Duration) -> TestResult {
        let until = Instant::now() + duration;
        while self.step(until).await?.is_some() {}
        Ok(())
    }

    fn active_tab(&self) -> Option<String> {
        self.tabs["tabs"]
            .as_array()?
            .iter()
            .find(|t| t["active"] == true)
            .and_then(|t| t["id"].as_str())
            .map(str::to_owned)
    }

    /// The RGB pixel at `(x, y)` of the newest frame, decoded by ffmpeg from the current group.
    fn pixel(&self, x: u32, y: u32) -> Result<[u8; 3], Box<dyn std::error::Error>> {
        let (avcc, _, _) = self.config.as_ref().ok_or("no video configuration")?;
        let mut annexb = parameter_sets(avcc)?;
        for frame in &self.gop {
            annexb.extend(avcc_to_annexb(frame)?);
        }
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(file.path(), &annexb)?;
        let output = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "h264", "-i"])
            .arg(file.path())
            .args([
                "-vf",
                &format!("crop=1:1:{x}:{y}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ])
            .output()?;
        let bytes = output.stdout;
        if bytes.len() < 3 {
            return Err(format!(
                "ffmpeg decoded nothing: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let last = &bytes[bytes.len() - 3..];
        Ok([last[0], last[1], last[2]])
    }
}

/// SPS and PPS of an `avcC` record with start codes.
fn parameter_sets(avcc: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let mut i = 5;
    let sps = usize::from(*avcc.get(i).ok_or("avcC")? & 0x1F);
    i += 1;
    for _ in 0..sps {
        let len = usize::from(u16::from_be_bytes([avcc[i], avcc[i + 1]]));
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(avcc.get(i + 2..i + 2 + len).ok_or("avcC sps")?);
        i += 2 + len;
    }
    let pps = usize::from(*avcc.get(i).ok_or("avcC pps count")?);
    i += 1;
    for _ in 0..pps {
        let len = usize::from(u16::from_be_bytes([avcc[i], avcc[i + 1]]));
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(avcc.get(i + 2..i + 2 + len).ok_or("avcC pps")?);
        i += 2 + len;
    }
    Ok(out)
}

/// Length-prefixed NAL units to start codes.
fn avcc_to_annexb(frame: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::with_capacity(frame.len() + 16);
    let mut i = 0;
    while i + 4 <= frame.len() {
        let len = u32::from_be_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]) as usize;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(frame.get(i + 4..i + 4 + len).ok_or("truncated NAL")?);
        i += 4 + len;
    }
    Ok(out)
}

/// The display number of the X server using `cookie` (from its `-auth` argument).
fn display_of(cookie: &Path) -> Option<u32> {
    let wanted = cookie.to_string_lossy().into_owned();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<String> = cmdline
            .split(|b| *b == 0)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        if args.first().is_some_and(|a| a.ends_with("Xvfb")) && args.iter().any(|a| a == &wanted) {
            return args.get(1)?.strip_prefix(':')?.parse().ok();
        }
    }
    None
}

/// Pids of processes whose command line contains `needle`.
fn pids_with(needle: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
            String::from_utf8_lossy(&cmdline)
                .replace('\0', " ")
                .contains(needle)
                .then_some(pid)
        })
        .collect()
}

/// CPU seconds (user + system) of a process.
fn cpu_seconds(pid: u32) -> f64 {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return 0.0;
    };
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .split_whitespace()
        .collect();
    let ticks: f64 = fields
        .get(11)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
        + fields
            .get(12)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
    ticks / 100.0
}

/// Evaluates `expression` in the page target whose URL starts with `url`.
async fn eval(
    cdp: &CdpClient,
    url: &str,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let targets = cdp.call("Target.getTargets", json!({})).await?;
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
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
        )
        .await?;
    let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
    let value = cdp
        .call_session(
            &session,
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        )
        .await?;
    let _ = cdp
        .call("Target.detachFromTarget", json!({"sessionId": session}))
        .await;
    Ok(value["result"]["value"].clone())
}

async fn until(cdp: &CdpClient, url: &str, what: &str, expression: &str) -> TestResult {
    let deadline = Instant::now() + WAIT;
    loop {
        if eval(cdp, url, &format!("!!({expression})")).await.ok() == Some(Value::Bool(true)) {
            return Ok(());
        }
        if Instant::now() > deadline {
            let events = eval(cdp, url, "JSON.stringify(window.events || null)")
                .await
                .unwrap_or(Value::Null);
            return Err(format!("timed out waiting for {what}; events {events}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn mouse(viewer: &mut Viewer, tab: &str, kind: &str, x: f64, y: f64) -> TestResult {
    viewer
        .send(json!({"type": "mouse", "tab_id": tab, "event": kind, "x": x, "y": y, "button": if kind == "move" { "none" } else { "left" }}))
        .await
}

async fn click(viewer: &mut Viewer, tab: &str, x: f64, y: f64) -> TestResult {
    mouse(viewer, tab, "move", x, y).await?;
    mouse(viewer, tab, "down", x, y).await?;
    mouse(viewer, tab, "up", x, y).await
}

async fn press(viewer: &mut Viewer, tab: &str, key: &str, code: &str) -> TestResult {
    viewer.send(json!({"type": "key", "tab_id": tab, "event": "down", "key": key, "code": code, "modifiers": 0})).await?;
    viewer.send(json!({"type": "key", "tab_id": tab, "event": "up", "key": key, "code": code, "modifiers": 0})).await
}

/// Opens the viewer, waits for the stream and shows `path` of the fixture site in the active tab.
async fn open_page(
    live: &Live,
    viewer: &mut Viewer,
    path: &str,
) -> Result<(String, CdpClient, String), Box<dyn std::error::Error>> {
    if viewer.config.is_none() {
        viewer
            .wait_json("video config", |m| m["type"] == "video_config")
            .await?;
    }
    viewer
        .send(json!({"type": "viewport", "width": 1280, "height": 800}))
        .await?;
    let tab = viewer.active_tab().ok_or("no active tab")?;
    let url = format!("http://{}{path}", live.site);
    viewer
        .send(json!({"type": "navigate", "tab_id": tab, "url": url}))
        .await?;
    let session = live.h.app.browser_session(&SandboxId::parse("s1")?).await?;
    let cdp = CdpClient::connect(&session.cdp_endpoint(), CdpConfig::default()).await?;
    until(&cdp, &url, "the page", "document.readyState === 'complete'").await?;
    Ok((tab, cdp, url))
}

#[tokio::test]
#[ignore = "needs Linux with Xvfb, ffmpeg and Chrome (YULIUS_LIVE_DISPLAY=1)"]
async fn the_real_chrome_streams_takes_real_input_and_stays_out_of_containment() -> TestResult {
    let Some(chrome) = live_chrome() else {
        return Ok(());
    };
    let live = live(&chrome).await?;
    let result = functional(&live).await;
    live.h.app.stop_workspace_services().await;
    result
}

async fn functional(live: &Live) -> TestResult {
    let started = Instant::now();
    let mut viewer = Viewer::connect(live, "s1").await?;
    let config = viewer
        .wait_json("video config", |m| m["type"] == "video_config")
        .await?;
    assert!(
        config["codec"]
            .as_str()
            .is_some_and(|c| c.starts_with("avc1.")),
        "{config}"
    );
    let first = viewer.wait_json("a frame", |m| m["type"] == "binary").await;
    first?;
    assert!(
        viewer.arrivals.first().is_some_and(|a| a.2),
        "the stream starts with a keyframe"
    );
    eprintln!(
        "first frame after {} ms (browser and display started cold)",
        started.elapsed().as_millis()
    );

    let (tab, cdp, url) = open_page(live, &mut viewer, "/").await?;
    // Real input: a click on the button is a trusted event at its position…
    click(&mut viewer, &tab, 400.0, 80.0).await?;
    until(&cdp, &url, "a trusted click", "events.some(e => e.type === 'click' && e.trusted && Math.abs(e.x - 400) < 2 && Math.abs(e.y - 80) < 2)").await?;
    // …and the video shows its effect (the blue box turns red).
    viewer.pump(Duration::from_millis(600)).await?;
    let [r, g, b] = viewer.pixel(50, 50)?;
    assert!(
        r > 200 && g < 60 && b < 60,
        "box pixel {r},{g},{b} should be red"
    );
    // An image loaded, and the canvas animates in the video.
    assert_eq!(
        eval(&cdp, &url, "document.getElementById('img').naturalWidth").await?,
        json!(100)
    );
    let [ir, ig, ib] = viewer.pixel(670, 90)?;
    assert!(
        ir > 200 && ig > 150 && ib < 80,
        "image pixel {ir},{ig},{ib} should be yellow"
    );
    let mut canvas = std::collections::HashSet::new();
    for _ in 0..4 {
        viewer.pump(Duration::from_millis(300)).await?;
        let [cr, cg, _] = viewer.pixel(350, 250)?;
        canvas.insert(cr > cg);
    }
    assert_eq!(canvas.len(), 2, "the canvas changes in the video");

    // Typing: ASCII, a shifted character and a Polish letter (inserted as text).
    click(&mut viewer, &tab, 450.0, 165.0).await?;
    until(
        &cdp,
        &url,
        "the field focused",
        "document.activeElement && document.activeElement.id === 'field'",
    )
    .await?;
    for (key, code) in [
        ("H", "KeyH"),
        ("e", "KeyE"),
        ("l", "KeyL"),
        ("l", "KeyL"),
        ("o", "KeyO"),
        (" ", "Space"),
        ("ą", "KeyA"),
        ("!", "Digit1"),
    ] {
        press(&mut viewer, &tab, key, code).await?;
    }
    until(
        &cdp,
        &url,
        "the typed text",
        "document.getElementById('field').value === 'Hello ą!'",
    )
    .await?;
    until(
        &cdp,
        &url,
        "trusted keys",
        "events.filter(e => e.type === 'keydown' && e.trusted).length >= 6",
    )
    .await?;
    // The wheel scrolls the page by CSS pixels.
    viewer.send(json!({"type": "mouse", "tab_id": tab, "event": "wheel", "x": 640, "y": 500, "dx": 0, "dy": 900})).await?;
    until(&cdp, &url, "the page scrolled", "window.scrollY > 500").await?;

    // Not an automated browser to the page.
    assert_eq!(eval(&cdp, &url, "navigator.webdriver").await?, json!(false));
    let agent = eval(&cdp, &url, "navigator.userAgent").await?;
    assert!(
        agent.as_str().is_some_and(|ua| !ua.contains("Headless")),
        "{agent}"
    );

    // The agent's click reaches the viewer as its cursor.
    let clicked = mcp_tool(
        &live.h,
        TOKEN,
        "browser_click",
        json!({"tab_id": tab, "x": 120, "y": 140}),
    )
    .await?;
    assert_eq!(clicked["isError"], false, "{clicked}");
    let activity = viewer
        .wait_json("the agent's cursor", |m| m["type"] == "agent_activity")
        .await?;
    assert_eq!(
        (activity["x"].clone(), activity["y"].clone()),
        (json!(120.0), json!(140.0))
    );
    let shot = mcp_tool(&live.h, TOKEN, "browser_screenshot", json!({"tab_id": tab})).await?;
    assert_eq!(
        shot["content"][0]["type"], "image",
        "screenshots work headful"
    );

    // No recursion: Yulius refuses to open itself, directly or from a page.
    let refused = mcp_tool(
        &live.h,
        TOKEN,
        "browser_open",
        json!({"url": format!("{ORIGIN}/")}),
    )
    .await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        tool_text(&refused)
            .as_str()
            .is_some_and(|t| t.contains("Yulius")),
        "{refused}"
    );
    eval(
        &cdp,
        &url,
        &format!("location.href = '{ORIGIN}/api/me'; true"),
    )
    .await?;
    until(
        &cdp,
        ORIGIN,
        "the blocked navigation",
        "document.documentElement.innerText.includes('ERR_BLOCKED_BY_CLIENT')",
    )
    .await?;

    containment(live).await
}

/// A contained process (the agents' and users' bubblewrap) reaches no display; another sandbox's
/// cookie does not open this sandbox's display.
async fn containment(live: &Live) -> TestResult {
    let mut other = Viewer::connect(live, "s2").await?;
    other
        .wait_json("the second display", |m| m["type"] == "video_config")
        .await?;
    let cookie1 = live.runtime.join("s1/Xauthority");
    let cookie2 = live.runtime.join("s2/Xauthority");
    let n1 = display_of(&cookie1).ok_or("display of s1")?;
    let n2 = display_of(&cookie2).ok_or("display of s2")?;
    assert_ne!(n1, n2, "one display per sandbox");
    let xdpyinfo = |display: u32, cookie: &Path| {
        std::process::Command::new("xdpyinfo")
            .args(["-display", &format!(":{display}")])
            .env("XAUTHORITY", cookie)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    // Positive control: with its own cookie the display answers; without, or with another
    // sandbox's cookie, it refuses.
    assert!(
        xdpyinfo(n1, &cookie1),
        "the display answers with its cookie"
    );
    assert!(!xdpyinfo(n1, Path::new("/dev/null")), "no cookie: refused");
    assert!(!xdpyinfo(n1, &cookie2), "another sandbox's cookie: refused");
    assert!(!xdpyinfo(n2, &cookie1), "another sandbox's cookie: refused");
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&cookie1)?.permissions().mode() & 0o777,
        0o600
    );
    // Only the filesystem socket listens: no abstract socket, no TCP.
    let net = std::fs::read_to_string("/proc/net/unix")?;
    assert!(
        net.contains(&format!("/tmp/.X11-unix/X{n1}")),
        "the path socket exists"
    );
    assert!(
        !net.contains(&format!("@/tmp/.X11-unix/X{n1}")),
        "no abstract socket"
    );
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", u16::try_from(6000 + n1)?)).is_err(),
        "no TCP"
    );

    // Inside containment (the same bubblewrap layout as agents and users' shells).
    let sandbox_root = live.data.join("sandboxes/s1");
    std::fs::create_dir_all(sandbox_root.join("home"))?;
    let script = format!(
        r#"echo "sockets:[$(ls /tmp/.X11-unix 2>/dev/null | tr '\n' ' ')]"
for n in {n1} {n2}; do if xdpyinfo -display :$n >/dev/null 2>&1; then echo "display:$n:reachable"; else echo "display:$n:refused"; fi; done
for c in {c1} {c2}; do if [ -r "$c" ]; then echo "cookie:visible"; else echo "cookie:hidden"; fi; done
python3 -c '
import socket
for n in ({n1}, {n2}):
    s = socket.socket(socket.AF_UNIX)
    try:
        s.connect("\0/tmp/.X11-unix/X%d" % n); print("abstract:%d:reachable" % n)
    except OSError:
        print("abstract:%d:refused" % n)
    t = socket.socket()
    try:
        t.connect(("127.0.0.1", 6000 + n)); print("tcp:%d:reachable" % n)
    except OSError:
        print("tcp:%d:refused" % n)
'"#,
        c1 = cookie1.display(),
        c2 = cookie2.display(),
    );
    for identity in [
        None,
        Some(yulius_contracts::ptyhost::SandboxIdentity {
            uid: 0,
            gid: 0,
            hostname: "fixture".into(),
        }),
    ] {
        let spec = yulius_ptyhost::BubblewrapSpec {
            sandbox_root: sandbox_root.to_string_lossy().into_owned(),
            home: sandbox_root.join("home").to_string_lossy().into_owned(),
            writable: Vec::new(),
            readonly: Vec::new(),
            share_net: true,
            binds: Vec::new(),
            identity: identity.clone(),
        };
        let argv = yulius_ptyhost::bwrap_argv(
            &spec,
            &["/bin/sh".into(), "-c".into(), script.clone()],
            &spec.home,
        );
        let output = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        eprintln!(
            "inside containment (identity {:?}):\n{text}{}",
            identity.as_ref().map(|i| i.uid),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "bwrap ran");
        assert!(text.contains("sockets:[]"), "no X socket inside: {text}");
        for n in [n1, n2] {
            for kind in ["display", "abstract", "tcp"] {
                assert!(
                    text.contains(&format!("{kind}:{n}:refused")),
                    "{kind} :{n} refused inside: {text}"
                );
            }
        }
        assert_eq!(
            text.matches("cookie:hidden").count(),
            2,
            "no cookie inside: {text}"
        );
    }
    Ok(())
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * p).round() as usize;
    values[index]
}

#[tokio::test]
#[ignore = "needs Linux with Xvfb, ffmpeg and Chrome (YULIUS_LIVE_DISPLAY=1)"]
async fn measures_fps_latency_cpu_and_bandwidth() -> TestResult {
    let Some(chrome) = live_chrome() else {
        return Ok(());
    };
    let live = live(&chrome).await?;
    let result = measure(&live).await;
    live.h.app.stop_workspace_services().await;
    result
}

async fn measure(live: &Live) -> TestResult {
    let mut viewer = Viewer::connect(live, "s1").await?;
    let (tab, cdp, url) = open_page(live, &mut viewer, "/flip").await?;
    let n = display_of(&live.runtime.join("s1/Xauthority")).ok_or("display")?;
    for (width, height) in [(1280_u32, 800_u32), (1920, 1080)] {
        viewer
            .send(json!({"type": "viewport", "width": width, "height": height}))
            .await?;
        viewer
            .wait_json("the new size", |m| {
                m["type"] == "video_config" && m["width"] == width
            })
            .await?;
        viewer.pump(Duration::from_secs(2)).await?;
        let encoder = pids_with(&format!(":{n}.0+0,0"))
            .first()
            .copied()
            .ok_or("encoder")?;

        // Idle.
        viewer.arrivals.clear();
        let cpu = cpu_seconds(encoder);
        let idle_start = Instant::now();
        viewer.pump(Duration::from_secs(6)).await?;
        let idle_secs = idle_start.elapsed().as_secs_f64();
        let idle_cpu = (cpu_seconds(encoder) - cpu) / idle_secs * 100.0;
        let idle_bytes: usize = viewer.arrivals.iter().map(|a| a.1).sum();
        let idle_fps = viewer.arrivals.len() as f64 / idle_secs;

        // Scrolling: wheel turns at 60 Hz through the viewer, as a trackpad sends them.
        eval(&cdp, &url, "scrollTo(0, 0); true").await?;
        viewer.arrivals.clear();
        let cpu = cpu_seconds(encoder);
        let scroll_start = Instant::now();
        while scroll_start.elapsed() < Duration::from_secs(8) {
            let direction = if (scroll_start.elapsed().as_millis() / 3000).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            viewer.send(json!({"type": "mouse", "tab_id": tab, "event": "wheel", "x": 400, "y": 400, "dx": 0, "dy": 12.0 * direction})).await?;
            viewer.pump(Duration::from_millis(16)).await?;
        }
        let secs = scroll_start.elapsed().as_secs_f64();
        let scroll_cpu = (cpu_seconds(encoder) - cpu) / secs * 100.0;
        let bytes: usize = viewer.arrivals.iter().map(|a| a.1).sum();
        // Frames per second over 1 s windows: all frames, and frames carrying a change.
        let first = viewer.arrivals.first().map_or(scroll_start, |a| a.0);
        let mut per_second = vec![0.0_f64; secs.ceil() as usize + 1];
        let mut changed = per_second.clone();
        for (at, size, _) in &viewer.arrivals {
            let index = at.duration_since(first).as_secs() as usize;
            if let Some(slot) = per_second.get_mut(index) {
                *slot += 1.0
            }
            if *size > 1500
                && let Some(slot) = changed.get_mut(index)
            {
                *slot += 1.0
            }
        }
        per_second.truncate(per_second.len().saturating_sub(2)); // partial windows
        changed.truncate(per_second.len());
        let mut gaps: Vec<f64> = viewer
            .arrivals
            .windows(2)
            .map(|w| w[1].0.duration_since(w[0].0).as_secs_f64() * 1000.0)
            .collect();
        eprintln!(
            "{width}x{height}: idle {idle_fps:.0} fps {:.2} Mbit/s encoder {idle_cpu:.0}% | scrolling fps p50 {:.0} p5 {:.0}, changed frames/s p50 {:.0} p5 {:.0}, frame gap p50 {:.1} ms p95 {:.1} ms, {:.2} Mbit/s, encoder {scroll_cpu:.0}% of a core",
            idle_bytes as f64 * 8.0 / idle_secs / 1e6,
            percentile(&mut per_second.clone(), 0.5),
            percentile(&mut per_second.clone(), 0.05),
            percentile(&mut changed.clone(), 0.5),
            percentile(&mut changed.clone(), 0.05),
            percentile(&mut gaps.clone(), 0.5),
            percentile(&mut gaps, 0.95),
            bytes as f64 * 8.0 / secs / 1e6,
        );
    }

    // Input to frame: a key press flips the whole page (invert); time from sending the key to
    // the first frame carrying the change, over the loopback viewer socket.
    viewer
        .send(json!({"type": "viewport", "width": 1280, "height": 800}))
        .await?;
    viewer
        .wait_json("1280x800", |m| {
            m["type"] == "video_config" && m["width"] == 1280
        })
        .await?;
    viewer.pump(Duration::from_secs(2)).await?;
    let mut delays = Vec::new();
    for _ in 0..40 {
        viewer.pump(Duration::from_millis(300)).await?;
        viewer.arrivals.clear();
        let sent = Instant::now();
        press(&mut viewer, &tab, "x", "KeyX").await?;
        let until_at = sent + Duration::from_secs(2);
        let mut seen = None;
        while seen.is_none() && Instant::now() < until_at {
            viewer.step(until_at).await?;
            seen = viewer
                .arrivals
                .iter()
                .find(|a| a.1 > 20_000 && !a.2)
                .map(|a| a.0);
        }
        if let Some(at) = seen {
            delays.push(at.duration_since(sent).as_secs_f64() * 1000.0);
        }
    }
    eprintln!(
        "input to frame (key over the viewer socket → XTest → Chrome → x11grab → x264 → socket): n {} p50 {:.1} ms p95 {:.1} ms",
        delays.len(),
        percentile(&mut delays.clone(), 0.5),
        percentile(&mut delays, 0.95)
    );
    assert!(delays.len() >= 30, "most key presses showed in the video");
    Ok(())
}
