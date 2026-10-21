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
//!   cargo test -p yulius --test main live_display:: -- --ignored --nocapture --test-threads 1
//! ```
//! `YULIUS_LIVE_DISPLAY_BASE` (default 90) is the first display number used; `YULIUS_BROWSER_FPS`
//! sets the video frame rate (default 60).

use crate::support;

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
use yulius_browser::{BrowserConfig, CdpClient, DisplayConfig, DisplayMode, UrlDenyList};
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
#open { position: absolute; left: 520px; top: 220px; width: 160px; height: 40px; }
#tall { height: 6000px; }
</style></head><body>
<div id="box"></div><button id="btn">Click me</button><input id="field" autocomplete="off">
<canvas id="canvas" width="200" height="100"></canvas><img id="img" src="/img.svg" width="100" height="100">
<button id="open" onclick="window.open('/popup', 'oauth', 'popup,width=400,height=300,left=900,top=600')">Sign in</button>
<div id="tall"></div>
<script>
window.events = [];
document.getElementById('btn').addEventListener('click', (e) => {
  events.push({ type: 'click', trusted: e.isTrusted, x: e.clientX, y: e.clientY });
  document.getElementById('box').style.background = 'rgb(255, 0, 0)';
});
addEventListener('keydown', (e) => events.push({ type: 'keydown', key: e.key, trusted: e.isTrusted }));
const c = document.getElementById('canvas').getContext('2d');
window.paint = (color) => { c.fillStyle = color; c.fillRect(0, 0, 200, 100); return true; };
paint('rgb(0, 200, 0)');
</script></body></html>"#;

/// A page whose whole background flips between black and white on every key press (the input
/// to frame measurement) and that scrolls (the fps measurement).
const FLIP: &str = r#"<!doctype html><html><head><title>flip</title><style>
html, body { margin: 0; background: #fff; font: 16px/1.5 sans-serif; }
.row { height: 120px; border-bottom: 1px solid #ccc; padding: 8px; }
</style></head><body><input id="big" style="position:fixed;left:20px;top:20px;width:900px;font-size:160px;caret-color:transparent;border:0;outline:0;background:#fff" autocomplete="off"><div id="rows"></div><script>
const rows = document.getElementById('rows');
for (let i = 0; i < 400; i += 1) {
  const d = document.createElement('div');
  d.className = 'row';
  d.textContent = 'Row ' + i + ' lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor';
  d.style.background = 'hsl(' + (i * 37 % 360) + ', 60%, 85%)';
  rows.append(d);
}
window.keys = 0;
window.scrollFrames = 0;
let lastY = -1;
(function frame() {
  if (scrollY !== lastY) { lastY = scrollY; window.scrollFrames += 1; }
  requestAnimationFrame(frame);
})();
addEventListener('keydown', (e) => {
  window.keys += 1;
  if (document.activeElement && document.activeElement.id === 'big') return;
  const black = document.body.dataset.black === '1';
  document.body.dataset.black = black ? '0' : '1';
  document.documentElement.style.filter = black ? '' : 'invert(1)';
});
</script></body></html>"#;

/// The "OAuth" popup: green, and it records clicks.
const POPUP: &str = r#"<!doctype html><html><head><title>popup</title><style>
html, body { margin: 0; height: 100%; background: rgb(0, 160, 0); }
</style></head><body><script>
window.clicks = [];
addEventListener('click', (e) => clicks.push({ trusted: e.isTrusted, x: e.clientX, y: e.clientY }));
</script></body></html>"#;

const IMAGE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="rgb(255,200,0)"/></svg>"#;

fn fixture_site() -> Router {
    Router::new()
        .route("/", get(|| async { axum::response::Html(FIXTURE) }))
        .route("/flip", get(|| async { axum::response::Html(FLIP) }))
        .route("/popup", get(|| async { axum::response::Html(POPUP) }))
        .route(
            "/img.svg",
            get(|| async { ([("content-type", "image/svg+xml")], IMAGE) }),
        )
}

struct Live {
    h: Harness,
    browsers: Arc<ChromiumBrowsers>,
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
    if let Some(fps) = std::env::var("YULIUS_BROWSER_FPS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        display.fps = fps;
    }
    let mut config = BrowserConfig::new(chrome.to_path_buf(), data.join("unused"));
    config.display = DisplayMode::Virtual(display);
    config.startup_timeout = Duration::from_secs(60);
    config.deny = UrlDenyList::yulius(ORIGIN, &[], Vec::new());
    let browsers = Arc::new(ChromiumBrowsers::new(config));
    let h = harness_with(HarnessOptions {
        browser: Some(Arc::clone(&browsers) as _),
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
        browsers,
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

    /// Asks for a keyframe (the stream has none on a timer, F 2026-10-10) and reads until one
    /// produced after now arrived, then a little more: the decoded picture is current even if
    /// this viewer lagged meanwhile (a lagging viewer skips to a fresh keyframe, by design).
    async fn settle(&mut self) -> TestResult {
        let start = Instant::now();
        let until = start + WAIT;
        self.send(json!({"type": "keyframe"})).await?;
        while !self.arrivals.iter().any(|a| a.2 && a.0 > start) {
            if self.step(until).await?.is_none() {
                return Err("no keyframe arrived".into());
            }
        }
        self.pump(Duration::from_millis(400)).await
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

    /// The current group as an Annex-B file.
    fn annexb(&self) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
        let (avcc, _, _) = self.config.as_ref().ok_or("no video configuration")?;
        let mut annexb = parameter_sets(avcc)?;
        for frame in &self.gop {
            annexb.extend(avcc_to_annexb(frame)?);
        }
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(file.path(), &annexb)?;
        Ok(file)
    }

    /// Saves the newest frame, decoded by ffmpeg, as a PNG.
    fn save_png(&self, path: &Path) -> TestResult {
        let file = self.annexb()?;
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "h264",
                "-i",
            ])
            .arg(file.path())
            .args(["-update", "1"])
            .arg(path)
            .status()?;
        if !status.success() {
            return Err("ffmpeg could not save the frame".into());
        }
        Ok(())
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
                &format!("format=rgb24,crop=1:1:{x}:{y}"),
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
            let page = eval(
                cdp,
                url,
                "JSON.stringify({events: window.events || null, field: document.activeElement && document.activeElement.value})",
            )
            .await
            .unwrap_or(Value::Null);
            return Err(format!("timed out waiting for {what}; page {page}").into());
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
    let sandbox = SandboxId::parse("s1")?;
    live.h.app.browser_session(&sandbox).await?;
    let cdp = live.browsers.cdp(&sandbox).await.ok_or("no browser")?;
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
    viewer.settle().await?;
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
    // A canvas the page repaints shows in the video each time.
    for (color, magenta) in [
        ("rgb(200, 0, 200)", true),
        ("rgb(0, 200, 0)", false),
        ("rgb(200, 0, 200)", true),
    ] {
        eval(&cdp, &url, &format!("paint('{color}')")).await?;
        viewer.settle().await?;
        let [cr, cg, cb] = viewer.pixel(350, 250)?;
        assert_eq!(
            cr > 150 && cb > 150 && cg < 60,
            magenta,
            "canvas pixel {cr},{cg},{cb} after {color}"
        );
    }

    // A popup (an OAuth window) opened by a click comes to the front, fitted to the view (it
    // asked for a spot outside it), and takes clicks; closing it shows the page again.
    click(&mut viewer, &tab, 600.0, 240.0).await?;
    let popup = viewer
        .wait_json("the popup in front", |m| {
            m["type"] == "tabs"
                && m["tabs"].as_array().is_some_and(|tabs| {
                    tabs.iter().any(|t| {
                        t["active"] == true
                            && t["url"].as_str().is_some_and(|u| u.ends_with("/popup"))
                    })
                })
        })
        .await?;
    let popup_url = format!("http://{}/popup", live.site);
    until(
        &cdp,
        &popup_url,
        "the popup loaded",
        "document.readyState === 'complete'",
    )
    .await?;
    viewer.settle().await?;
    for (x, y) in [(60, 60), (1200, 700)] {
        let [pr, pg, pb] = viewer.pixel(x, y)?;
        assert!(
            pg > 120 && pr < 60 && pb < 60,
            "popup pixel at {x},{y}: {pr},{pg},{pb} (the popup fills the view)"
        );
    }
    click(&mut viewer, &tab, 640.0, 400.0).await?;
    until(
        &cdp,
        &popup_url,
        "a trusted click in the popup",
        "clicks.some((c) => c.trusted)",
    )
    .await?;
    eval(&cdp, &popup_url, "window.close(), true").await?;
    viewer
        .wait_json("the popup closed", |m| {
            m["type"] == "tabs" && !m.to_string().contains("/popup")
        })
        .await?;
    viewer.settle().await?;
    let [r, g, b] = viewer.pixel(50, 50)?;
    assert!(r > 200 && g < 60 && b < 60, "the page is back: {r},{g},{b}");
    drop(popup);
    let tab = viewer.active_tab().ok_or("no active tab")?;

    // Typing: ASCII, a shifted character and a Polish letter (a keysym key press), in order.
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
    // Composed text and a short paste are typed as key presses, in order.
    viewer
        .send(json!({"type": "key", "tab_id": tab, "event": "char", "text": " Lovelace żółć 🙂"}))
        .await?;
    until(
        &cdp,
        &url,
        "the typed text",
        "document.getElementById('field').value === 'Hello ą! Lovelace żółć 🙂'",
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
            join: None,
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
        if viewer.config.as_ref().map(|c| c.1) != Some(width) {
            viewer
                .wait_json("the new size", |m| {
                    m["type"] == "video_config" && m["width"] == width
                })
                .await?;
        }
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
        let painted_before = eval(&cdp, &url, "window.scrollFrames")
            .await?
            .as_f64()
            .unwrap_or(0.0);
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
        let painted = (eval(&cdp, &url, "window.scrollFrames")
            .await?
            .as_f64()
            .unwrap_or(0.0)
            - painted_before)
            / secs;
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
            "{width}x{height}: idle {idle_fps:.0} fps {:.2} Mbit/s encoder {idle_cpu:.0}% | scrolling: video fps p50 {:.0} p5 {:.0}, page frames with a new scroll position {painted:.0}/s, frame gap p50 {:.1} ms p95 {:.1} ms, {:.2} Mbit/s, encoder {scroll_cpu:.0}% of a core",
            idle_bytes as f64 * 8.0 / idle_secs / 1e6,
            percentile(&mut per_second.clone(), 0.5),
            percentile(&mut per_second.clone(), 0.05),
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
    if viewer.config.as_ref().map(|c| c.1) != Some(1280) {
        viewer
            .wait_json("1280x800", |m| {
                m["type"] == "video_config" && m["width"] == 1280
            })
            .await?;
    }
    viewer.pump(Duration::from_secs(2)).await?;
    // Keys go to the page once it has the focus.
    click(&mut viewer, &tab, 600.0, 400.0).await?;
    viewer.pump(Duration::from_millis(300)).await?;
    let keys_before = eval(&cdp, &url, "window.keys")
        .await?
        .as_f64()
        .unwrap_or(0.0);
    let mut delays = Vec::new();
    let mut misses = Vec::new();
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
        } else {
            misses.push(viewer.arrivals.iter().map(|a| a.1).max().unwrap_or(0));
        }
    }
    let keys = eval(&cdp, &url, "window.keys")
        .await?
        .as_f64()
        .unwrap_or(0.0)
        - keys_before;
    eprintln!("the page saw {keys} key presses; misses (largest frame after the key): {misses:?}");
    eprintln!(
        "input to frame (key over the viewer socket → XTest → Chrome → x11grab → x264 → socket): n {} p50 {:.1} ms p95 {:.1} ms",
        delays.len(),
        percentile(&mut delays.clone(), 0.5),
        percentile(&mut delays, 0.95)
    );
    assert!(delays.len() >= 30, "most key presses showed in the video");

    // Typing: a character appearing in a large field (a typical edit, not a full repaint).
    eval(
        &cdp,
        &url,
        "document.documentElement.style.filter = ''; document.body.dataset.black = '0'; true",
    )
    .await?;
    click(&mut viewer, &tab, 400.0, 100.0).await?;
    until(
        &cdp,
        &url,
        "the big field focused",
        "document.activeElement && document.activeElement.id === 'big'",
    )
    .await?;
    let mut typing = Vec::new();
    for n in 0..40 {
        viewer.pump(Duration::from_millis(250)).await?;
        if n % 8 == 7 {
            eval(
                &cdp,
                &url,
                "document.getElementById('big').value = ''; true",
            )
            .await?;
            viewer.pump(Duration::from_millis(250)).await?;
        }
        viewer.arrivals.clear();
        let sent = Instant::now();
        press(&mut viewer, &tab, "m", "KeyM").await?;
        let until_at = sent + Duration::from_secs(2);
        let mut seen = None;
        while seen.is_none() && Instant::now() < until_at {
            viewer.step(until_at).await?;
            seen = viewer
                .arrivals
                .iter()
                .find(|a| a.1 > 1_000 && !a.2)
                .map(|a| a.0);
        }
        if let Some(at) = seen {
            typing.push(at.duration_since(sent).as_secs_f64() * 1000.0);
        }
    }
    eprintln!(
        "typing a character to the frame showing it: n {} p50 {:.1} ms p95 {:.1} ms",
        typing.len(),
        percentile(&mut typing.clone(), 0.5),
        percentile(&mut typing, 0.95)
    );
    Ok(())
}

/// Opens a captcha page in the sandbox Chrome and clicks its checkbox through the viewer (real
/// XTest input), saving the frames before and after under `$YULIUS_LIVE_SHOTS` (default
/// `/tmp/yulius-live-shots`). Needs the internet; prints what happened.
#[tokio::test]
#[ignore = "needs Linux with Xvfb, ffmpeg, Chrome and the internet (YULIUS_LIVE_DISPLAY=1)"]
async fn captchas_take_real_clicks() -> TestResult {
    let Some(chrome) = live_chrome() else {
        return Ok(());
    };
    let live = live(&chrome).await?;
    let result = captchas(&live).await;
    live.h.app.stop_workspace_services().await;
    result
}

/// Top-left corner (CSS pixels) of the first frame whose `src` contains `needle`, searched
/// through shadow roots.
async fn frame_corner(
    cdp: &CdpClient,
    session: &str,
    needle: &str,
) -> Result<Option<(f64, f64)>, Box<dyn std::error::Error>> {
    cdp.call_session(session, "DOM.enable", json!({})).await?;
    cdp.call_session(
        session,
        "DOM.getDocument",
        json!({"depth": -1, "pierce": true}),
    )
    .await?;
    let flat = cdp
        .call_session(
            session,
            "DOM.getFlattenedDocument",
            json!({"depth": -1, "pierce": true}),
        )
        .await?;
    let node = flat["nodes"].as_array().and_then(|nodes| {
        nodes.iter().find(|n| {
            n["nodeName"] == "IFRAME"
                && n["attributes"].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|v| v.as_str().is_some_and(|v| v.contains(needle)))
                })
        })
    });
    let Some(id) = node.and_then(|n| n["nodeId"].as_i64()) else {
        return Ok(None);
    };
    let model = cdp
        .call_session(session, "DOM.getBoxModel", json!({"nodeId": id}))
        .await?;
    let quad = &model["model"]["content"];
    Ok(Some((
        quad[0].as_f64().unwrap_or(0.0),
        quad[1].as_f64().unwrap_or(0.0),
    )))
}

async fn captchas(live: &Live) -> TestResult {
    let shots = PathBuf::from(
        std::env::var("YULIUS_LIVE_SHOTS").unwrap_or_else(|_| "/tmp/yulius-live-shots".into()),
    );
    std::fs::create_dir_all(&shots)?;
    let mut viewer = Viewer::connect(live, "s1").await?;
    viewer
        .wait_json("video config", |m| m["type"] == "video_config")
        .await?;
    viewer
        .send(json!({"type": "viewport", "width": 1280, "height": 800}))
        .await?;
    let tab = viewer.active_tab().ok_or("no active tab")?;
    let sandbox = SandboxId::parse("s1")?;
    live.h.app.browser_session(&sandbox).await?;
    let cdp = live.browsers.cdp(&sandbox).await.ok_or("no browser")?;
    let pages = [
        (
            "recaptcha",
            "https://www.google.com/recaptcha/api2/demo",
            "google.com/recaptcha/api2/anchor",
            (27.0, 38.0),
        ),
        (
            "turnstile",
            "https://nopecha.com/demo/cloudflare",
            "challenges.cloudflare.com",
            (22.0, 33.0),
        ),
    ];
    for (name, url, frame, offset) in pages {
        viewer
            .send(json!({"type": "navigate", "tab_id": tab, "url": url}))
            .await?;
        viewer.pump(Duration::from_secs(8)).await?;
        viewer.save_png(&shots.join(format!("{name}-before.png")))?;
        let targets = cdp.call("Target.getTargets", json!({})).await?;
        let target = targets["targetInfos"]
            .as_array()
            .and_then(|infos| infos.iter().find(|t| t["targetId"] == tab.as_str()))
            .ok_or("tab target")?
            .clone();
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId": target["targetId"], "flatten": true}),
            )
            .await?;
        let page = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let webdriver = cdp
            .call_session(
                &page,
                "Runtime.evaluate",
                json!({"expression": "navigator.webdriver", "returnByValue": true}),
            )
            .await?;
        let corner = frame_corner(&cdp, &page, frame).await?;
        eprintln!(
            "{name}: {url} navigator.webdriver = {}, widget frame at {corner:?}",
            webdriver["result"]["value"]
        );
        let Some((left, top)) = corner else {
            eprintln!("{name}: no widget frame found (see {name}-before.png)");
            continue;
        };
        let (x, y) = (left + offset.0, top + offset.1);
        // A human-like approach, then the click: all real pointer input on the display.
        for step in 0..8 {
            let t = f64::from(step) / 7.0;
            mouse(
                &mut viewer,
                &tab,
                "move",
                700.0 + (x - 700.0) * t,
                500.0 + (y - 500.0) * t,
            )
            .await?;
            viewer.pump(Duration::from_millis(40)).await?;
        }
        mouse(&mut viewer, &tab, "down", x, y).await?;
        viewer.pump(Duration::from_millis(90)).await?;
        mouse(&mut viewer, &tab, "up", x, y).await?;
        viewer.pump(Duration::from_secs(8)).await?;
        viewer.save_png(&shots.join(format!("{name}-after.png")))?;
        let title = cdp
            .call_session(
                &page,
                "Runtime.evaluate",
                json!({"expression": "document.title", "returnByValue": true}),
            )
            .await?;
        eprintln!(
            "{name}: clicked at ({x:.0}, {y:.0}); page title now {}; frames saved in {}",
            title["result"]["value"],
            shots.display()
        );
        // An image challenge: its first tile takes a real click too (it gets selected).
        if name == "recaptcha"
            && let Some((left, top)) = frame_corner(&cdp, &page, "recaptcha/api2/bframe").await?
        {
            let (tx, ty) = (left + 70.0, top + 190.0);
            mouse(&mut viewer, &tab, "move", tx, ty).await?;
            viewer.pump(Duration::from_millis(120)).await?;
            mouse(&mut viewer, &tab, "down", tx, ty).await?;
            viewer.pump(Duration::from_millis(90)).await?;
            mouse(&mut viewer, &tab, "up", tx, ty).await?;
            viewer.pump(Duration::from_secs(3)).await?;
            viewer.save_png(&shots.join("recaptcha-tile.png"))?;
            eprintln!(
                "recaptcha: image challenge at ({left:.0}, {top:.0}); clicked its first tile at ({tx:.0}, {ty:.0})"
            );
        }
        let _ = cdp
            .call("Target.detachFromTarget", json!({"sessionId": page}))
            .await;
    }
    Ok(())
}
