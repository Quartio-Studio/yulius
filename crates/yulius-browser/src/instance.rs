//! One running sandbox browser: tab registry, per-tab sessions, screencast, input and the
//! agent tool functions (SPEC-001 BRW-001..004, SPEC-002 ADR-09).
//!
//! Users (viewer WebSocket) and agents (MCP tools) act on the same tabs. All operations are
//! bounded by the CDP call timeout; input events are serialized per browser so their order is
//! preserved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use yulius_contracts::browser::{
    ACTION_REASON_MAX_BYTES, ActionRequest, ClickParams, ClickResult, ClickTarget, KeyEventKind,
    MouseButton, MouseEventKind, NavigateParams, NavigationResult, OpenParams,
    SCREENSHOT_MAX_BASE64_BYTES, SNAPSHOT_MAX_BYTES, ScreenshotParams, ScreenshotResult,
    SnapshotParams, SnapshotResult, TabId, TabInfo, TabsResult, TypeParams, TypeResult,
    ViewerClientMsg,
};
use yulius_domain::ids::{SandboxId, TaskId};

use crate::cdp::{CdpClient, CdpConfig, CdpEvent, FrameSink, ScreencastFrame, wait_closed};
use crate::error::{BrowserError, CdpError};
use crate::keys;
use crate::mirror::{self, BINDING, DomBatch, MirrorBuffer};
use crate::process::{StopTimeouts, Supervised};
use crate::snapshot;
use crate::url_policy::{UrlDenyList, UrlRefused, normalize_url};

/// Smallest accepted viewport edge in CSS pixels.
pub const VIEWPORT_MIN: u32 = 64;
/// Largest accepted viewport edge in CSS pixels.
pub const VIEWPORT_MAX: u32 = 4096;
/// Largest accepted absolute coordinate or wheel delta.
const COORDINATE_LIMIT: f64 = 1.0e6;
/// Largest accepted `key`/`code` text.
const KEY_FIELD_MAX_BYTES: usize = 64;
/// Largest accepted text of one key event.
const KEY_TEXT_MAX_BYTES: usize = 4096;
/// Presses closer than this in time and space count as a multi-click.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f64 = 5.0;
/// Unsubscribe notifications queued for the event task.
const CONTROL_QUEUE: usize = 64;
/// Browser events buffered per subscriber.
const EVENT_QUEUE: usize = 64;
/// DOM batches queued per mirror subscriber; a lagging viewer resubscribes (full snapshot).
const DOM_QUEUE: usize = 256;
/// Largest page resource fetched for a viewer.
pub const MAX_RESOURCE_BYTES: usize = 20 << 20;
/// Bytes read per `IO.read` call.
const IO_READ_CHUNK: u32 = 1 << 20;

const CENTER_FUNCTION: &str = "function (selector) {
  const element = this.querySelector(selector);
  if (!element) return { found: false };
  element.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  const r = element.getBoundingClientRect();
  return { found: true, x: r.left + r.width / 2, y: r.top + r.height / 2, width: r.width, height: r.height };
}";

const FOCUS_FUNCTION: &str = "function (selector) {
  const element = this.querySelector(selector);
  if (!element) return { found: false };
  element.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
  element.focus();
  return { found: true };
}";

const TEXT_FALLBACK_EXPRESSION: &str =
    "(document.body ? document.body.innerText : '').slice(0, 70000)";

/// Screencast parameters (`Page.startScreencast`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreencastConfig {
    /// JPEG quality 0–100.
    pub quality: u8,
    /// Deliver every n-th frame.
    pub every_nth_frame: u32,
    /// Frames buffered per subscriber before the oldest are dropped.
    pub frame_buffer: usize,
}

impl Default for ScreencastConfig {
    fn default() -> Self {
        Self {
            quality: 60,
            every_nth_frame: 1,
            frame_buffer: 4,
        }
    }
}

/// Settings of one browser instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceConfig {
    /// Initial viewport (width, height) in CSS pixels.
    pub viewport: (u32, u32),
    /// CDP connection limits.
    pub cdp: CdpConfig,
    /// Screencast parameters.
    pub screencast: ScreencastConfig,
    /// Maximum open tabs; `open` beyond it fails with [`BrowserError::TooManyTabs`].
    pub max_tabs: usize,
    /// How long tools wait for `Page.loadEventFired` before returning `loaded: false`.
    pub load_timeout: Duration,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            viewport: (1280, 800),
            cdp: CdpConfig::default(),
            screencast: ScreencastConfig::default(),
            max_tabs: 32,
            load_timeout: Duration::from_secs(15),
        }
    }
}

/// One encoding attempt of [`BrowserInstance::tool_screenshot_within`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenshotStep {
    /// JPEG quality; `None` means PNG.
    pub quality: Option<u8>,
    /// Scale of the viewport (1.0 = full size).
    pub scale: f64,
}

/// Attempts of [`BrowserInstance::tool_screenshot_within`], largest first.
pub const BOUNDED_SCREENSHOT_STEPS: [ScreenshotStep; 7] = [
    ScreenshotStep {
        quality: None,
        scale: 1.0,
    },
    ScreenshotStep {
        quality: Some(80),
        scale: 1.0,
    },
    ScreenshotStep {
        quality: Some(60),
        scale: 1.0,
    },
    ScreenshotStep {
        quality: Some(50),
        scale: 0.75,
    },
    ScreenshotStep {
        quality: Some(45),
        scale: 0.5,
    },
    ScreenshotStep {
        quality: Some(40),
        scale: 0.35,
    },
    ScreenshotStep {
        quality: Some(30),
        scale: 0.25,
    },
];

/// A screenshot that fits a byte budget, with the encoding that made it fit.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundedScreenshot {
    /// The image (`image/png` or `image/jpeg`).
    pub result: ScreenshotResult,
    /// JPEG quality used; `None` for PNG.
    pub quality: Option<u8>,
    /// Scale of the image relative to the viewport (1.0 = CSS pixels).
    pub scale: f64,
    /// Viewport (width, height) in CSS pixels when captured.
    pub viewport: (u32, u32),
}

/// Notifications for the server (viewer messages and the UI event stream).
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserEvent {
    /// An agent asked the user to act in a tab (`browser.action_requested`).
    ActionRequested {
        /// Tab needing the user.
        tab_id: TabId,
        /// Reason shown to the user.
        reason: String,
        /// Requesting task.
        task_id: Option<String>,
    },
    /// A pending user-action request was cleared (explicitly or because the tab closed).
    ActionCleared {
        /// The tab.
        tab_id: TabId,
    },
    /// The DevTools connection closed (browser exited or crashed); the next
    /// [`crate::BrowserManager::ensure`] restarts it.
    Disconnected,
    /// An agent acted in a tab (shown to viewers as the agent's cursor, BRW-007).
    AgentActivity {
        /// The tab.
        tab_id: TabId,
        /// Position in CSS pixels, when the action has one.
        x: Option<f64>,
        /// Position in CSS pixels, when the action has one.
        y: Option<f64>,
        /// `click`, `type`, `open` or `navigate`.
        action: String,
        /// Who acted (e.g. `#12 Claude Code`).
        label: String,
    },
    /// The viewport of all tabs changed (viewers lay their DOM mirror out at this size).
    Viewport {
        /// Width in CSS pixels.
        width: u32,
        /// Height in CSS pixels.
        height: u32,
    },
}

/// A page resource fetched by the browser for a viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// HTTP status, when known.
    pub status: Option<u16>,
    /// `Content-Type`, when given.
    pub content_type: Option<String>,
    /// The bytes.
    pub body: Vec<u8>,
}

/// What a DOM subscriber receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomEvent {
    /// New events (or, with `reset`, a new stream).
    Batch(DomBatch),
    /// The subscriber fell behind; it must resubscribe for a fresh snapshot.
    Lagged,
}

/// The DOM mirror of one tab. Dropping it unsubscribes; the recorder stops when the last
/// subscriber of a tab is gone.
#[derive(Debug)]
pub struct DomSubscription {
    tab: TabId,
    initial: Option<DomBatch>,
    receiver: broadcast::Receiver<DomBatch>,
    control: mpsc::Sender<TabId>,
}

impl DomSubscription {
    /// The tab.
    #[must_use]
    pub fn tab_id(&self) -> &TabId {
        &self.tab
    }

    /// The next batch (the buffered snapshot first, when one existed); `None` once the tab or
    /// browser closed.
    pub async fn recv(&mut self) -> Option<DomEvent> {
        if let Some(batch) = self.initial.take() {
            return Some(DomEvent::Batch(batch));
        }
        match self.receiver.recv().await {
            Ok(batch) => Some(DomEvent::Batch(batch)),
            Err(broadcast::error::RecvError::Lagged(_)) => Some(DomEvent::Lagged),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

impl Drop for DomSubscription {
    fn drop(&mut self) {
        let _ = self.control.try_send(self.tab.clone());
    }
}

/// Pointer input for [`BrowserInstance::dispatch_mouse`], in CSS pixels of the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseInput {
    /// Event kind.
    pub event: MouseEventKind,
    /// Horizontal position.
    pub x: f64,
    /// Vertical position.
    pub y: f64,
    /// Button (none for moves and wheel).
    pub button: MouseButton,
    /// Horizontal wheel delta.
    pub dx: f64,
    /// Vertical wheel delta.
    pub dy: f64,
    /// Modifier bit field (`MODIFIER_*`).
    pub modifiers: u8,
}

/// Keyboard input for [`BrowserInstance::dispatch_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    /// Event kind.
    pub event: KeyEventKind,
    /// DOM key.
    pub key: Option<String>,
    /// DOM code.
    pub code: Option<String>,
    /// Produced text.
    pub text: Option<String>,
    /// Modifier bit field (`MODIFIER_*`).
    pub modifiers: u8,
}

/// Screencast frames of one tab. Dropping it unsubscribes; the screencast stops when the last
/// subscriber of a tab is gone.
#[derive(Debug)]
pub struct FrameSubscription {
    tab: TabId,
    first: Option<ScreencastFrame>,
    receiver: Option<broadcast::Receiver<ScreencastFrame>>,
    dropped: u64,
    control: mpsc::Sender<TabId>,
}

impl FrameSubscription {
    /// The subscribed tab.
    #[must_use]
    pub fn tab_id(&self) -> &TabId {
        &self.tab
    }

    /// The next frame (the tab's latest frame first); `None` once the tab or browser closed.
    /// Frames a slow consumer missed are skipped and counted in [`Self::frames_dropped`].
    pub async fn recv(&mut self) -> Option<ScreencastFrame> {
        if let Some(frame) = self.first.take() {
            return Some(frame);
        }
        let receiver = self.receiver.as_mut()?;
        loop {
            match receiver.recv().await {
                Ok(frame) => return Some(frame),
                Err(broadcast::error::RecvError::Lagged(skipped)) => self.dropped += skipped,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// A buffered frame without waiting, if any.
    pub fn try_recv(&mut self) -> Option<ScreencastFrame> {
        if let Some(frame) = self.first.take() {
            return Some(frame);
        }
        let receiver = self.receiver.as_mut()?;
        loop {
            match receiver.try_recv() {
                Ok(frame) => return Some(frame),
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => self.dropped += skipped,
                Err(_) => return None,
            }
        }
    }

    /// Frames skipped because this subscriber lagged.
    #[must_use]
    pub fn frames_dropped(&self) -> u64 {
        self.dropped
    }
}

impl Drop for FrameSubscription {
    fn drop(&mut self) {
        drop(self.receiver.take());
        // Best effort: a full queue only delays stopping until the next notification.
        let _ = self.control.try_send(self.tab.clone());
    }
}

struct TabEntry {
    id: TabId,
    title: String,
    url: String,
}

struct Screencast {
    session: String,
    sink: Arc<FrameSink>,
}

struct Press {
    at: Instant,
    button: MouseButton,
    x: f64,
    y: f64,
    count: u32,
}

#[derive(Default)]
struct Pointer {
    buttons: u32,
    last_press: Option<Press>,
}

impl Pointer {
    fn bit(button: MouseButton) -> u32 {
        match button {
            MouseButton::None => 0,
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
            MouseButton::Back => 8,
            MouseButton::Forward => 16,
        }
    }

    /// CDP event type, click count and pressed-buttons mask for an input.
    fn apply(&mut self, input: &MouseInput, now: Instant) -> (&'static str, u32, u32) {
        match input.event {
            MouseEventKind::Down => {
                let count = match &self.last_press {
                    Some(press)
                        if press.button == input.button
                            && now.duration_since(press.at) <= MULTI_CLICK_WINDOW
                            && (press.x - input.x).hypot(press.y - input.y)
                                <= MULTI_CLICK_DISTANCE =>
                    {
                        (press.count + 1).min(3)
                    }
                    _ => 1,
                };
                self.last_press = Some(Press {
                    at: now,
                    button: input.button,
                    x: input.x,
                    y: input.y,
                    count,
                });
                self.buttons |= Self::bit(input.button);
                ("mousePressed", count, self.buttons)
            }
            MouseEventKind::Up => {
                let count = match &self.last_press {
                    Some(press) if press.button == input.button => press.count,
                    _ => 1,
                };
                self.buttons &= !Self::bit(input.button);
                ("mouseReleased", count, self.buttons)
            }
            MouseEventKind::Move => ("mouseMoved", 0, self.buttons),
            MouseEventKind::Wheel => ("mouseWheel", 0, self.buttons),
        }
    }
}

/// A tab whose DOM is mirrored to viewers (SPEC-001 BRW-005).
struct MirrorTab {
    session: String,
    script: Option<String>,
    buffer: MirrorBuffer,
    tx: broadcast::Sender<DomBatch>,
    checkout_pending: bool,
}

struct State {
    tabs: Vec<TabEntry>,
    active: Option<TabId>,
    sessions: HashMap<TabId, String>,
    actions: HashMap<TabId, ActionRequest>,
    screencasts: HashMap<TabId, Screencast>,
    mirrors: HashMap<TabId, MirrorTab>,
    viewport: (u32, u32),
    pointer: Pointer,
}

impl State {
    fn has(&self, tab: &TabId) -> bool {
        self.tabs.iter().any(|entry| &entry.id == tab)
    }

    fn upsert(&mut self, id: TabId, title: String, url: String) {
        if let Some(entry) = self.tabs.iter_mut().find(|entry| entry.id == id) {
            entry.title = title;
            entry.url = url;
        } else {
            if self.active.is_none() {
                self.active = Some(id.clone());
            }
            self.tabs.push(TabEntry { id, title, url });
        }
    }

    /// Removes a tab and everything attached to it; returns its session and whether a
    /// user-action request was pending.
    fn remove(&mut self, tab: &TabId) -> (Option<String>, bool) {
        self.tabs.retain(|entry| &entry.id != tab);
        let had_action = self.actions.remove(tab).is_some();
        self.screencasts.remove(tab);
        self.mirrors.remove(tab);
        let session = self.sessions.remove(tab);
        if self.active.as_ref() == Some(tab) {
            self.active = self.tabs.last().map(|entry| entry.id.clone());
        }
        (session, had_action)
    }

    fn info(&self, entry: &TabEntry) -> TabInfo {
        TabInfo {
            id: entry.id.clone(),
            title: entry.title.clone(),
            url: entry.url.clone(),
            active: self.active.as_ref() == Some(&entry.id),
            action_requested: self.actions.get(&entry.id).cloned(),
        }
    }

    fn infos(&self) -> Vec<TabInfo> {
        self.tabs.iter().map(|entry| self.info(entry)).collect()
    }
}

/// A running (or attached) Chromium shared by a sandbox's users and agents.
pub struct BrowserInstance {
    sandbox: SandboxId,
    cdp: CdpClient,
    http_url: Option<String>,
    config: InstanceConfig,
    state: Mutex<State>,
    tabs_tx: watch::Sender<Vec<TabInfo>>,
    events_tx: broadcast::Sender<BrowserEvent>,
    control_tx: mpsc::Sender<TabId>,
    session_lock: tokio::sync::Mutex<()>,
    screencast_lock: tokio::sync::Mutex<()>,
    open_lock: tokio::sync::Mutex<()>,
    input_lock: tokio::sync::Mutex<()>,
    last_used: Mutex<Instant>,
    process: OnceLock<Supervised>,
    event_task: Mutex<Option<JoinHandle<()>>>,
    mirror_lock: tokio::sync::Mutex<()>,
    /// Address of new and blank tabs (the Yulius welcome page, BRW-006).
    new_tab_url: Mutex<Option<String>>,
    /// Addresses this browser never loads: Yulius itself (BRW-008).
    deny: Mutex<UrlDenyList>,
}

impl std::fmt::Debug for BrowserInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserInstance")
            .field("sandbox", &self.sandbox)
            .field("cdp", &self.cdp)
            .field("pid", &self.pid())
            .finish_non_exhaustive()
    }
}

impl Drop for BrowserInstance {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.event_task).take() {
            task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn malformed(method: &str, detail: &str) -> BrowserError {
    BrowserError::Cdp(CdpError::Malformed {
        method: method.to_owned(),
        detail: detail.to_owned(),
    })
}

fn page_entry(info: &Value) -> Option<(TabId, String, String)> {
    if info.get("type").and_then(Value::as_str) != Some("page") {
        return None;
    }
    let id = TabId::parse(info.get("targetId").and_then(Value::as_str)?).ok()?;
    let text = |name: &str| {
        info.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Some((id, text("title"), text("url")))
}

fn metrics(width: u32, height: u32) -> Value {
    json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": false})
}

fn check_coordinate(name: &str, value: f64) -> Result<(), BrowserError> {
    if value.is_finite() && value.abs() <= COORDINATE_LIMIT {
        Ok(())
    } else {
        Err(BrowserError::InvalidInput(format!(
            "{name} must be a finite number within ±{COORDINATE_LIMIT}"
        )))
    }
}

fn check_len(name: &str, value: Option<&str>, max: usize) -> Result<(), BrowserError> {
    match value {
        Some(text) if text.len() > max => Err(BrowserError::InvalidInput(format!(
            "{name} exceeds {max} bytes"
        ))),
        _ => Ok(()),
    }
}

fn exception_text(details: &Value) -> String {
    let text = details
        .get("exception")
        .and_then(|e| e.get("description"))
        .and_then(Value::as_str)
        .or_else(|| details.get("text").and_then(Value::as_str))
        .unwrap_or("exception");
    text.chars().take(300).collect()
}

async fn wait_for_load(
    events: &mut broadcast::Receiver<CdpEvent>,
    session: &str,
    timeout: Duration,
) -> bool {
    let waiting = async {
        loop {
            match events.recv().await {
                Ok(event)
                    if event.method == "Page.loadEventFired"
                        && event.session_id.as_deref() == Some(session) =>
                {
                    return true;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return false,
            }
        }
    };
    tokio::time::timeout(timeout, waiting)
        .await
        .unwrap_or(false)
}

impl BrowserInstance {
    /// Attaches to a DevTools endpoint without owning a process (tests, externally managed
    /// browsers). [`crate::BrowserManager::ensure`] uses this after launching Chromium.
    ///
    /// # Errors
    /// CDP connection or target discovery failures.
    pub async fn attach(
        sandbox: SandboxId,
        ws_url: &str,
        config: InstanceConfig,
    ) -> Result<Arc<Self>, BrowserError> {
        let cdp = CdpClient::connect(ws_url, config.cdp).await?;
        let events = cdp.subscribe_events();
        cdp.call("Target.setDiscoverTargets", json!({"discover": true}))
            .await?;
        let http_url = ws_url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .map(|authority| format!("http://{authority}"));
        let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
        let (events_tx, _) = broadcast::channel(EVENT_QUEUE);
        let closed = cdp.close_signal();
        let instance = Arc::new(Self {
            sandbox,
            cdp,
            http_url,
            config,
            state: Mutex::new(State {
                tabs: Vec::new(),
                active: None,
                sessions: HashMap::new(),
                actions: HashMap::new(),
                screencasts: HashMap::new(),
                mirrors: HashMap::new(),
                viewport: config.viewport,
                pointer: Pointer::default(),
            }),
            tabs_tx: watch::channel(Vec::new()).0,
            events_tx,
            control_tx,
            session_lock: tokio::sync::Mutex::new(()),
            screencast_lock: tokio::sync::Mutex::new(()),
            open_lock: tokio::sync::Mutex::new(()),
            input_lock: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            process: OnceLock::new(),
            event_task: Mutex::new(None),
            mirror_lock: tokio::sync::Mutex::new(()),
            new_tab_url: Mutex::new(None),
            deny: Mutex::new(UrlDenyList::default()),
        });
        instance.resync().await?;
        let task = tokio::spawn(event_loop(
            Arc::downgrade(&instance),
            events,
            control_rx,
            closed,
        ));
        *lock(&instance.event_task) = Some(task);
        Ok(instance)
    }

    pub(crate) fn set_process(&self, process: Supervised) {
        if let Err(process) = self.process.set(process) {
            warn!(
                pid = process.pid(),
                "browser instance already owns a process"
            );
        }
    }

    /// The owning sandbox.
    #[must_use]
    pub fn sandbox(&self) -> &SandboxId {
        &self.sandbox
    }

    /// Browser-level DevTools WebSocket URL for CDP-capable MCP servers (SPEC-001 U-10).
    /// Loopback only; see the crate documentation for its trust implications.
    #[must_use]
    pub fn cdp_ws_url(&self) -> &str {
        self.cdp.url()
    }

    /// `http://127.0.0.1:<port>` form of the DevTools endpoint (for tools taking a browser URL).
    #[must_use]
    pub fn cdp_http_url(&self) -> Option<&str> {
        self.http_url.as_deref()
    }

    /// The loopback port of the DevTools endpoint.
    #[must_use]
    pub fn devtools_port(&self) -> Option<u16> {
        self.cdp
            .url()
            .strip_prefix("ws://")?
            .split('/')
            .next()?
            .rsplit_once(':')?
            .1
            .parse()
            .ok()
    }

    /// The underlying CDP client, for callers needing methods this adapter does not wrap.
    #[must_use]
    pub fn cdp(&self) -> &CdpClient {
        &self.cdp
    }

    /// Chromium's process id when this instance owns the process.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.process.get().map(Supervised::pid)
    }

    /// Whether the DevTools connection is open and the owned process (if any) runs.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.cdp.is_closed() && self.process.get().is_none_or(|p| !p.has_exited())
    }

    /// Resolves when the connection closes or the owned process exits (crash detection).
    pub async fn wait_disconnected(&self) {
        let process_exit = async {
            match self.process.get() {
                Some(process) => process.exited().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = self.cdp.closed() => {}
            () = process_exit => {}
        }
    }

    /// Records activity (resets the idle timer).
    pub fn touch(&self) {
        *lock(&self.last_used) = Instant::now();
    }

    /// Last recorded activity.
    #[must_use]
    pub fn last_used(&self) -> Instant {
        *lock(&self.last_used)
    }

    /// Live screencast subscribers across tabs (viewers watching keep the browser busy).
    #[must_use]
    pub fn frame_subscribers(&self) -> usize {
        lock(&self.state)
            .screencasts
            .values()
            .map(|cast| cast.sink.receiver_count())
            .sum()
    }

    /// Current tabs in creation order.
    #[must_use]
    pub fn tabs(&self) -> Vec<TabInfo> {
        self.touch();
        lock(&self.state).infos()
    }

    /// The tab list, updated after every change (viewers send it as `{"type":"tabs"}`).
    #[must_use]
    pub fn subscribe_tabs(&self) -> watch::Receiver<Vec<TabInfo>> {
        self.tabs_tx.subscribe()
    }

    /// User-action and connection events.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<BrowserEvent> {
        self.events_tx.subscribe()
    }

    fn publish(&self) {
        let infos = lock(&self.state).infos();
        self.tabs_tx.send_replace(infos);
    }

    fn tab_info(&self, tab: &TabId) -> Result<TabInfo, BrowserError> {
        let state = lock(&self.state);
        state
            .tabs
            .iter()
            .find(|entry| &entry.id == tab)
            .map(|entry| state.info(entry))
            .ok_or_else(|| BrowserError::TabNotFound(tab.clone()))
    }

    fn require_tab(&self, tab: &TabId) -> Result<(), BrowserError> {
        if lock(&self.state).has(tab) {
            Ok(())
        } else {
            Err(BrowserError::TabNotFound(tab.clone()))
        }
    }

    async fn resync(&self) -> Result<(), BrowserError> {
        let result = self.cdp.call("Target.getTargets", json!({})).await?;
        let pages: Vec<_> = result
            .get("targetInfos")
            .and_then(Value::as_array)
            .map(|infos| infos.iter().filter_map(page_entry).collect())
            .unwrap_or_default();
        let mut gone_sessions = Vec::new();
        {
            let mut state = lock(&self.state);
            let gone: Vec<TabId> = state
                .tabs
                .iter()
                .filter(|entry| !pages.iter().any(|(id, _, _)| id == &entry.id))
                .map(|entry| entry.id.clone())
                .collect();
            for tab in &gone {
                gone_sessions.extend(state.remove(tab).0);
            }
            for (id, title, url) in pages {
                state.upsert(id, title, url);
            }
        }
        for session in gone_sessions {
            self.cdp.remove_frame_sink(&session);
        }
        self.publish();
        Ok(())
    }

    fn on_event(&self, event: &CdpEvent) {
        match event.method.as_str() {
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                if let Some((id, title, url)) = event.params.get("targetInfo").and_then(page_entry)
                {
                    lock(&self.state).upsert(id, title, url);
                    self.publish();
                }
            }
            "Target.targetDestroyed" => {
                let tab = event
                    .params
                    .get("targetId")
                    .and_then(Value::as_str)
                    .and_then(|id| TabId::parse(id).ok());
                if let Some(tab) = tab {
                    self.forget_tab(&tab);
                }
            }
            "Runtime.bindingCalled" => self.on_mirror_batch(event),
            "Fetch.requestPaused" if event.session_id.is_none() => self.on_request_paused(event),
            "Target.detachedFromTarget" => {
                if let Some(session) = event.params.get("sessionId").and_then(Value::as_str) {
                    {
                        let mut state = lock(&self.state);
                        state.sessions.retain(|_, s| s != session);
                        state.screencasts.retain(|_, cast| cast.session != session);
                        state.mirrors.retain(|_, mirror| mirror.session != session);
                    }
                    self.cdp.remove_frame_sink(session);
                }
            }
            _ => {}
        }
    }

    /// Re-reads a tab's title and URL. Chromium does not report every title change through
    /// `Target.targetInfoChanged`, so this runs after loads and same-document navigations.
    async fn refresh_tab(&self, tab: &TabId) {
        match self
            .cdp
            .call("Target.getTargetInfo", json!({"targetId": tab.as_str()}))
            .await
        {
            Ok(result) => {
                if let Some((id, title, url)) = result.get("targetInfo").and_then(page_entry) {
                    let mut state = lock(&self.state);
                    if state.has(&id) {
                        state.upsert(id, title, url);
                        drop(state);
                        self.publish();
                    }
                }
            }
            Err(error) => debug!(%error, "refreshing tab info failed"),
        }
    }

    async fn refresh_session_tab(&self, session: &str) {
        let tab = lock(&self.state)
            .sessions
            .iter()
            .find(|(_, s)| s.as_str() == session)
            .map(|(tab, _)| tab.clone());
        if let Some(tab) = tab {
            self.refresh_tab(&tab).await;
        }
    }

    fn forget_tab(&self, tab: &TabId) {
        let (session, had_action, existed) = {
            let mut state = lock(&self.state);
            let existed = state.has(tab);
            let (session, had_action) = state.remove(tab);
            (session, had_action, existed)
        };
        if let Some(session) = session {
            self.cdp.remove_frame_sink(&session);
        }
        if had_action {
            let _ = self.events_tx.send(BrowserEvent::ActionCleared {
                tab_id: tab.clone(),
            });
        }
        if existed {
            self.publish();
        }
    }

    fn on_disconnected(&self) {
        {
            let mut state = lock(&self.state);
            state.screencasts.clear();
            state.sessions.clear();
            state.mirrors.clear();
        }
        let _ = self.events_tx.send(BrowserEvent::Disconnected);
    }

    async fn session(&self, tab: &TabId) -> Result<String, BrowserError> {
        if let Some(session) = self.cached_session(tab)? {
            return Ok(session);
        }
        let _guard = self.session_lock.lock().await;
        if let Some(session) = self.cached_session(tab)? {
            return Ok(session);
        }
        let attached = self
            .cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId": tab.as_str(), "flatten": true}),
            )
            .await?;
        let session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("Target.attachToTarget", "missing sessionId"))?
            .to_owned();
        self.cdp
            .call_session(&session, "Page.enable", json!({}))
            .await?;
        let (width, height) = lock(&self.state).viewport;
        self.cdp
            .call_session(
                &session,
                "Emulation.setDeviceMetricsOverride",
                metrics(width, height),
            )
            .await?;
        let latest = {
            let mut state = lock(&self.state);
            if !state.has(tab) {
                return Err(BrowserError::TabNotFound(tab.clone()));
            }
            state.sessions.insert(tab.clone(), session.clone());
            state.viewport
        };
        // A viewport set while this tab was attaching did not reach it (it was not in the
        // sessions yet): apply the newest one.
        if latest != (width, height) {
            self.cdp
                .call_session(
                    &session,
                    "Emulation.setDeviceMetricsOverride",
                    metrics(latest.0, latest.1),
                )
                .await?;
        }
        Ok(session)
    }

    fn cached_session(&self, tab: &TabId) -> Result<Option<String>, BrowserError> {
        let state = lock(&self.state);
        if state.has(tab) {
            Ok(state.sessions.get(tab).cloned())
        } else {
            Err(BrowserError::TabNotFound(tab.clone()))
        }
    }

    async fn create_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        let _guard = self.open_lock.lock().await;
        let open = lock(&self.state).tabs.len();
        if open >= self.config.max_tabs {
            return Err(BrowserError::TooManyTabs(self.config.max_tabs));
        }
        let created = self
            .cdp
            .call("Target.createTarget", json!({"url": url}))
            .await?;
        let tab = created
            .get("targetId")
            .and_then(Value::as_str)
            .and_then(|id| TabId::parse(id).ok())
            .ok_or_else(|| malformed("Target.createTarget", "missing or invalid targetId"))?;
        {
            let mut state = lock(&self.state);
            if !state.has(&tab) {
                state.upsert(tab.clone(), String::new(), url.to_owned());
            }
            state.active = Some(tab.clone());
        }
        self.publish();
        Ok(tab)
    }

    /// Opens a new tab at an address; it becomes the active tab. Returns without waiting for
    /// the page to load.
    ///
    /// # Errors
    /// [`BrowserError::UrlRefused`], [`BrowserError::TooManyTabs`] or CDP failures.
    pub async fn open(&self, url: &str) -> Result<TabId, BrowserError> {
        self.touch();
        // An empty address opens the welcome page (BRW-006), when one is configured.
        let welcome = lock(&self.new_tab_url).clone();
        let url = match welcome {
            Some(welcome) if url.trim().is_empty() => welcome,
            _ => normalize_url(url)?,
        };
        self.check_allowed(&url)?;
        self.create_tab(&url).await
    }

    async fn navigate_session(
        &self,
        tab: &TabId,
        session: &str,
        url: &str,
    ) -> Result<(), BrowserError> {
        let result = self
            .cdp
            .call_session(session, "Page.navigate", json!({"url": url}))
            .await?;
        if let Some(error) = result
            .get("errorText")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
        {
            return Err(BrowserError::NavigationFailed(error.to_owned()));
        }
        let mut state = lock(&self.state);
        if let Some(entry) = state.tabs.iter_mut().find(|entry| &entry.id == tab) {
            url.clone_into(&mut entry.url);
        }
        drop(state);
        self.publish();
        Ok(())
    }

    /// Navigates a tab without waiting for the load event.
    ///
    /// # Errors
    /// [`BrowserError::UrlRefused`], [`BrowserError::TabNotFound`],
    /// [`BrowserError::NavigationFailed`] or CDP failures.
    pub async fn navigate(&self, tab: &TabId, url: &str) -> Result<(), BrowserError> {
        self.touch();
        let url = normalize_url(url)?;
        self.check_allowed(&url)?;
        let session = self.session(tab).await?;
        self.navigate_session(tab, &session, &url).await
    }

    /// Makes a tab the active tab and brings it to the front.
    ///
    /// # Errors
    /// [`BrowserError::TabNotFound`] or CDP failures.
    pub async fn activate(&self, tab: &TabId) -> Result<(), BrowserError> {
        self.touch();
        self.require_tab(tab)?;
        self.cdp
            .call("Target.activateTarget", json!({"targetId": tab.as_str()}))
            .await?;
        lock(&self.state).active = Some(tab.clone());
        self.publish();
        Ok(())
    }

    /// Closes a tab (its pending user-action request and screencast end with it).
    ///
    /// # Errors
    /// [`BrowserError::TabNotFound`] or CDP failures.
    pub async fn close(&self, tab: &TabId) -> Result<(), BrowserError> {
        self.touch();
        self.require_tab(tab)?;
        self.cdp
            .call("Target.closeTarget", json!({"targetId": tab.as_str()}))
            .await?;
        self.forget_tab(tab);
        Ok(())
    }

    fn screencast_params(&self, width: u32, height: u32) -> Value {
        let cast = self.config.screencast;
        json!({
            "format": "jpeg",
            "quality": cast.quality.min(100),
            "maxWidth": width,
            "maxHeight": height,
            "everyNthFrame": cast.every_nth_frame.max(1),
        })
    }

    /// Subscribes to a tab's screencast, starting it if needed. The subscription yields the
    /// tab's latest frame first, then new frames; a slow subscriber loses the oldest frames.
    ///
    /// # Errors
    /// [`BrowserError::TabNotFound`] or CDP failures.
    pub async fn subscribe_frames(&self, tab: &TabId) -> Result<FrameSubscription, BrowserError> {
        self.touch();
        let session = self.session(tab).await?;
        let _guard = self.screencast_lock.lock().await;
        let existing = lock(&self.state)
            .screencasts
            .get(tab)
            .map(|cast| Arc::clone(&cast.sink));
        let (receiver, first) = match existing {
            Some(sink) => {
                let receiver = sink.subscribe();
                (receiver, sink.latest())
            }
            None => {
                let sink = Arc::new(FrameSink::new(self.config.screencast.frame_buffer));
                // Subscribe before starting so the first frames are not missed.
                let receiver = sink.subscribe();
                self.cdp.register_frame_sink(&session, Arc::clone(&sink));
                let (width, height) = lock(&self.state).viewport;
                let started = self
                    .cdp
                    .call_session(
                        &session,
                        "Page.startScreencast",
                        self.screencast_params(width, height),
                    )
                    .await;
                if let Err(error) = started {
                    self.cdp.remove_frame_sink(&session);
                    return Err(error.into());
                }
                let mut state = lock(&self.state);
                if !state.has(tab) {
                    drop(state);
                    self.cdp.remove_frame_sink(&session);
                    return Err(BrowserError::TabNotFound(tab.clone()));
                }
                state.screencasts.insert(
                    tab.clone(),
                    Screencast {
                        session: session.clone(),
                        sink,
                    },
                );
                (receiver, None)
            }
        };
        Ok(FrameSubscription {
            tab: tab.clone(),
            first,
            receiver: Some(receiver),
            dropped: 0,
            control: self.control_tx.clone(),
        })
    }

    /// Sets the address of new and blank tabs (the welcome page).
    pub fn set_new_tab_url(&self, url: Option<String>) {
        *lock(&self.new_tab_url) = url;
    }

    /// Refuses `list`'s addresses from now on (BRW-008): tool and viewer navigation fail with
    /// [`UrlRefused::Yulius`], and Chromium intercepts every matching request a page makes
    /// (links, redirects, frames, scripts) through a browser-wide `Fetch` interception, which
    /// this instance fails as blocked by the client unless an allowed prefix matches.
    ///
    /// # Errors
    /// CDP failures while enabling the interception.
    pub async fn deny_urls(&self, list: UrlDenyList) -> Result<(), BrowserError> {
        let patterns: Vec<Value> = list
            .patterns()
            .iter()
            .map(|pattern| json!({"urlPattern": pattern, "requestStage": "Request"}))
            .collect();
        *lock(&self.deny) = list;
        if !patterns.is_empty() {
            self.cdp
                .call("Fetch.enable", json!({"patterns": patterns}))
                .await?;
        }
        Ok(())
    }

    /// Refuses an address of the deny list.
    fn check_allowed(&self, url: &str) -> Result<(), BrowserError> {
        if lock(&self.deny).denies(url) {
            Err(BrowserError::UrlRefused(UrlRefused::Yulius))
        } else {
            Ok(())
        }
    }

    /// A request a page made matched the deny list's patterns: continue it only when an allowed
    /// prefix matches (the welcome page), otherwise fail it as blocked.
    fn on_request_paused(&self, event: &CdpEvent) {
        let Some(request_id) = event
            .params
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let url = event
            .params
            .pointer("/request/url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let denied = lock(&self.deny).denies(url);
        if denied {
            debug!(url, "the sandbox browser refused to load Yulius");
        }
        let cdp = self.cdp.clone();
        tokio::spawn(async move {
            let outcome = if denied {
                cdp.call(
                    "Fetch.failRequest",
                    json!({"requestId": request_id, "errorReason": "BlockedByClient"}),
                )
                .await
            } else {
                cdp.call("Fetch.continueRequest", json!({"requestId": request_id}))
                    .await
            };
            if let Err(error) = outcome {
                debug!(%error, "answering an intercepted request failed");
            }
        });
    }

    /// Shows the welcome page in tabs that are still `about:blank` (a fresh browser).
    pub async fn show_welcome_in_blank_tabs(&self) {
        let Some(welcome) = lock(&self.new_tab_url).clone() else {
            return;
        };
        let blank: Vec<TabId> = lock(&self.state)
            .tabs
            .iter()
            .filter(|entry| entry.url.is_empty() || entry.url == "about:blank")
            .map(|entry| entry.id.clone())
            .collect();
        for tab in blank {
            match self.session(&tab).await {
                Ok(session) => {
                    if let Err(error) = self.navigate_session(&tab, &session, &welcome).await {
                        debug!(%error, "showing the welcome page failed");
                    }
                }
                Err(error) => debug!(%error, "showing the welcome page failed"),
            }
        }
    }

    /// Subscribes to a tab's DOM mirror, starting the recorder on first use (BRW-005).
    ///
    /// # Errors
    /// [`BrowserError::TabNotFound`] or CDP failures while installing the recorder.
    pub async fn subscribe_dom(&self, tab: &TabId) -> Result<DomSubscription, BrowserError> {
        self.touch();
        let session = self.session(tab).await?;
        let _guard = self.mirror_lock.lock().await;
        let installed = lock(&self.state)
            .mirrors
            .get(tab)
            .is_some_and(|mirror| mirror.session == session);
        if !installed {
            self.install_mirror(tab, &session).await?;
        }
        let (initial, receiver) = {
            let state = lock(&self.state);
            let mirror = state
                .mirrors
                .get(tab)
                .ok_or_else(|| BrowserError::TabNotFound(tab.clone()))?;
            (mirror.buffer.snapshot(), mirror.tx.subscribe())
        };
        if initial.is_none() && installed {
            // A running recorder without a buffered snapshot sends one on request; a fresh one
            // snapshots by itself when it starts.
            if let Err(error) = self
                .cdp
                .call_session(
                    &session,
                    "Runtime.evaluate",
                    json!({"expression": mirror::SNAPSHOT_EXPRESSION}),
                )
                .await
            {
                debug!(%error, "requesting a DOM snapshot failed");
            }
        }
        Ok(DomSubscription {
            tab: tab.clone(),
            initial,
            receiver,
            control: self.control_tx.clone(),
        })
    }

    async fn install_mirror(&self, tab: &TabId, session: &str) -> Result<(), BrowserError> {
        let script = mirror::injection_script();
        self.cdp
            .call_session(session, "Runtime.enable", json!({}))
            .await?;
        self.cdp
            .call_session(session, "Runtime.addBinding", json!({"name": BINDING}))
            .await?;
        let added = self
            .cdp
            .call_session(
                session,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": script}),
            )
            .await?;
        {
            let mut state = lock(&self.state);
            if !state.has(tab) {
                return Err(BrowserError::TabNotFound(tab.clone()));
            }
            state.mirrors.insert(
                tab.clone(),
                MirrorTab {
                    session: session.to_owned(),
                    script: added
                        .get("identifier")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    buffer: MirrorBuffer::default(),
                    tx: broadcast::channel(DOM_QUEUE).0,
                    checkout_pending: false,
                },
            );
        }
        // The current document gets the recorder now; later documents through the script.
        if let Err(error) = self
            .cdp
            .call_session(session, "Runtime.evaluate", json!({"expression": script}))
            .await
        {
            debug!(%error, "starting the DOM recorder in the current document failed");
        }
        Ok(())
    }

    fn on_mirror_batch(&self, event: &CdpEvent) {
        if event.params.get("name").and_then(Value::as_str) != Some(BINDING) {
            return;
        }
        let (Some(session), Some(payload)) = (
            event.session_id.as_deref(),
            event.params.get("payload").and_then(Value::as_str),
        ) else {
            return;
        };
        let mut checkout = false;
        {
            let mut state = lock(&self.state);
            let Some(mirror) = state
                .mirrors
                .values_mut()
                .find(|mirror| mirror.session == session)
            else {
                return;
            };
            match mirror.buffer.push(payload) {
                Ok((batch, outgrown)) => {
                    if batch.reset {
                        mirror.checkout_pending = false;
                    }
                    let _ = mirror.tx.send(batch);
                    if outgrown && !mirror.checkout_pending {
                        mirror.checkout_pending = true;
                        checkout = true;
                    }
                }
                Err(error) => debug!(%error, "dropping a DOM mirror batch"),
            }
        }
        if checkout {
            self.request_snapshot(session);
        }
    }

    fn request_snapshot(&self, session: &str) {
        let cdp = self.cdp.clone();
        let session = session.to_owned();
        tokio::spawn(async move {
            let _ = cdp
                .call_session(
                    &session,
                    "Runtime.evaluate",
                    json!({"expression": mirror::SNAPSHOT_EXPRESSION}),
                )
                .await;
        });
    }

    /// Asks every running recorder for a fresh full snapshot (after lost events).
    fn checkout_mirrors(&self) {
        let sessions: Vec<String> = {
            let mut state = lock(&self.state);
            state
                .mirrors
                .values_mut()
                .map(|mirror| {
                    mirror.checkout_pending = true;
                    mirror.session.clone()
                })
                .collect()
        };
        for session in sessions {
            self.request_snapshot(&session);
        }
    }

    /// Stops recorders nobody watches (CPU and memory of agent-driven pages).
    async fn stop_unwatched_mirrors(&self) {
        let _guard = self.mirror_lock.lock().await;
        let idle: Vec<MirrorTab> = {
            let mut state = lock(&self.state);
            let tabs: Vec<TabId> = state
                .mirrors
                .iter()
                .filter(|(_, mirror)| mirror.tx.receiver_count() == 0)
                .map(|(tab, _)| tab.clone())
                .collect();
            tabs.iter()
                .filter_map(|tab| state.mirrors.remove(tab))
                .collect()
        };
        for mirror in idle {
            let session = mirror.session.as_str();
            let _ = self
                .cdp
                .call_session(
                    session,
                    "Runtime.evaluate",
                    json!({"expression": mirror::STOP_EXPRESSION}),
                )
                .await;
            if let Some(identifier) = &mirror.script {
                let _ = self
                    .cdp
                    .call_session(
                        session,
                        "Page.removeScriptToEvaluateOnNewDocument",
                        json!({"identifier": identifier}),
                    )
                    .await;
            }
            let _ = self
                .cdp
                .call_session(session, "Runtime.removeBinding", json!({"name": BINDING}))
                .await;
        }
    }

    /// Fetches a page resource with the browser's own network stack (its cookies and the
    /// VM's loopback), for a viewer's mirror (BRW-005).
    ///
    /// # Errors
    /// [`BrowserError::UrlRefused`] outside http(s), [`BrowserError::TabNotFound`],
    /// [`BrowserError::NavigationFailed`] when the fetch fails,
    /// [`BrowserError::ResultTooLarge`] beyond [`MAX_RESOURCE_BYTES`], CDP failures.
    pub async fn fetch_resource(&self, tab: &TabId, url: &str) -> Result<Resource, BrowserError> {
        use base64::Engine as _;
        let url = normalize_url(url)?;
        if url == "about:blank" {
            return Err(BrowserError::InvalidInput("not a resource address".into()));
        }
        self.check_allowed(&url)?;
        let session = self.session(tab).await?;
        let tree = self
            .cdp
            .call_session(&session, "Page.getFrameTree", json!({}))
            .await?;
        let frame = tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("Page.getFrameTree", "missing frame id"))?;
        let loaded = self
            .cdp
            .call_session(
                &session,
                "Network.loadNetworkResource",
                json!({"frameId": frame, "url": url, "options": {"disableCache": false, "includeCredentials": true}}),
            )
            .await?;
        let resource = loaded.get("resource").cloned().unwrap_or_default();
        if resource.get("success").and_then(Value::as_bool) != Some(true) {
            let reason = resource
                .get("netErrorName")
                .and_then(Value::as_str)
                .unwrap_or("the resource could not be loaded");
            return Err(BrowserError::NavigationFailed(reason.to_owned()));
        }
        let status = resource
            .get("httpStatusCode")
            .and_then(Value::as_u64)
            .and_then(|s| u16::try_from(s).ok());
        let content_type = resource
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                    .and_then(|(_, value)| value.as_str())
                    .map(str::to_owned)
            });
        let Some(stream) = resource.get("stream").and_then(Value::as_str) else {
            return Ok(Resource {
                status,
                content_type,
                body: Vec::new(),
            });
        };
        let mut body = Vec::new();
        let outcome = loop {
            let chunk = match self
                .cdp
                .call_session(
                    &session,
                    "IO.read",
                    json!({"handle": stream, "size": IO_READ_CHUNK}),
                )
                .await
            {
                Ok(chunk) => chunk,
                Err(error) => break Err(BrowserError::Cdp(error)),
            };
            let data = chunk
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if chunk.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => body.extend_from_slice(&bytes),
                    Err(_) => break Err(malformed("IO.read", "invalid base64")),
                }
            } else {
                body.extend_from_slice(data.as_bytes());
            }
            if body.len() > MAX_RESOURCE_BYTES {
                break Err(BrowserError::ResultTooLarge {
                    limit: MAX_RESOURCE_BYTES,
                });
            }
            if chunk.get("eof").and_then(Value::as_bool) == Some(true) {
                break Ok(());
            }
        };
        let _ = self
            .cdp
            .call_session(&session, "IO.close", json!({"handle": stream}))
            .await;
        outcome.map(|()| Resource {
            status,
            content_type,
            body,
        })
    }

    /// Tells viewers that an agent acted in a tab (its cursor and action, BRW-007).
    pub fn agent_activity(
        &self,
        tab: &TabId,
        position: Option<(f64, f64)>,
        action: &str,
        label: &str,
    ) {
        let _ = self.events_tx.send(BrowserEvent::AgentActivity {
            tab_id: tab.clone(),
            x: position.map(|(x, _)| x),
            y: position.map(|(_, y)| y),
            action: action.to_owned(),
            label: label.to_owned(),
        });
    }

    async fn stop_unwatched_screencasts(&self) {
        let _guard = self.screencast_lock.lock().await;
        let idle: Vec<String> = {
            let mut state = lock(&self.state);
            let tabs: Vec<TabId> = state
                .screencasts
                .iter()
                .filter(|(_, cast)| cast.sink.receiver_count() == 0)
                .map(|(tab, _)| tab.clone())
                .collect();
            tabs.iter()
                .filter_map(|tab| state.screencasts.remove(tab))
                .map(|cast| cast.session)
                .collect()
        };
        for session in idle {
            self.cdp.remove_frame_sink(&session);
            if let Err(error) = self
                .cdp
                .call_session(&session, "Page.stopScreencast", json!({}))
                .await
            {
                debug!(%error, "stopping screencast failed");
            }
        }
    }

    /// The viewport of all tabs in CSS pixels.
    #[must_use]
    pub fn viewport(&self) -> (u32, u32) {
        lock(&self.state).viewport
    }

    /// Sets the viewport of all tabs and screencasts (the viewer panel size).
    ///
    /// # Errors
    /// [`BrowserError::InvalidInput`] outside [`VIEWPORT_MIN`]..=[`VIEWPORT_MAX`]; CDP transport
    /// failures (per-tab protocol errors are logged and skipped).
    pub async fn set_viewport(&self, width: u32, height: u32) -> Result<(), BrowserError> {
        self.touch();
        let range = VIEWPORT_MIN..=VIEWPORT_MAX;
        if !range.contains(&width) || !range.contains(&height) {
            return Err(BrowserError::InvalidInput(format!(
                "viewport must be within {VIEWPORT_MIN}..={VIEWPORT_MAX} pixels per edge"
            )));
        }
        let (sessions, casts) = {
            let mut state = lock(&self.state);
            state.viewport = (width, height);
            let sessions: Vec<String> = state.sessions.values().cloned().collect();
            let casts: Vec<String> = state
                .screencasts
                .values()
                .map(|cast| cast.session.clone())
                .collect();
            (sessions, casts)
        };
        let _ = self
            .events_tx
            .send(BrowserEvent::Viewport { width, height });
        let calls = sessions
            .iter()
            .map(|s| {
                (
                    s,
                    "Emulation.setDeviceMetricsOverride",
                    metrics(width, height),
                )
            })
            .chain(casts.iter().map(|s| {
                (
                    s,
                    "Page.startScreencast",
                    self.screencast_params(width, height),
                )
            }));
        for (session, method, params) in calls {
            match self.cdp.call_session(session, method, params).await {
                Ok(_) => {}
                Err(CdpError::Protocol { message, .. }) => {
                    debug!(%message, method, "viewport update skipped for a stale session");
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    /// Dispatches pointer input to a tab. Presses at the same spot within 500 ms count as
    /// double/triple clicks.
    ///
    /// # Errors
    /// [`BrowserError::InvalidInput`] for non-finite or huge values; tab or CDP failures.
    pub async fn dispatch_mouse(&self, tab: &TabId, input: MouseInput) -> Result<(), BrowserError> {
        self.touch();
        for (name, value) in [
            ("x", input.x),
            ("y", input.y),
            ("dx", input.dx),
            ("dy", input.dy),
        ] {
            check_coordinate(name, value)?;
        }
        let session = self.session(tab).await?;
        let _input = self.input_lock.lock().await;
        let (kind, click_count, buttons) = lock(&self.state).pointer.apply(&input, Instant::now());
        let mut params = json!({
            "type": kind,
            "x": input.x,
            "y": input.y,
            "button": input.button.as_cdp(),
            "buttons": buttons,
            "clickCount": click_count,
            "modifiers": input.modifiers & 0x0F,
        });
        if input.event == MouseEventKind::Wheel {
            params["button"] = json!("none");
            params["deltaX"] = json!(input.dx);
            params["deltaY"] = json!(input.dy);
        }
        self.cdp
            .call_session(&session, "Input.dispatchMouseEvent", params)
            .await?;
        Ok(())
    }

    /// Dispatches keyboard input to a tab, adding the virtual key code Chromium needs for
    /// non-text keys and `\r` text for Enter.
    ///
    /// # Errors
    /// [`BrowserError::InvalidInput`] for oversized fields, invalid modifiers or a `char`
    /// event without text; tab or CDP failures.
    pub async fn dispatch_key(&self, tab: &TabId, input: KeyInput) -> Result<(), BrowserError> {
        self.touch();
        check_len("key", input.key.as_deref(), KEY_FIELD_MAX_BYTES)?;
        check_len("code", input.code.as_deref(), KEY_FIELD_MAX_BYTES)?;
        check_len("text", input.text.as_deref(), KEY_TEXT_MAX_BYTES)?;
        if input.modifiers > 0x0F {
            return Err(BrowserError::InvalidInput(
                "modifiers must be a 4-bit field".into(),
            ));
        }
        let text = match input.event {
            KeyEventKind::Up => None,
            KeyEventKind::Down | KeyEventKind::Char => input
                .text
                .clone()
                .filter(|text| !text.is_empty())
                .or_else(|| keys::implied_text(input.key.as_deref()).map(str::to_owned)),
        };
        let kind = match (input.event, &text) {
            (KeyEventKind::Down, Some(_)) => "keyDown",
            (KeyEventKind::Down, None) => "rawKeyDown",
            (KeyEventKind::Up, _) => "keyUp",
            (KeyEventKind::Char, Some(_)) => "char",
            (KeyEventKind::Char, None) => {
                return Err(BrowserError::InvalidInput("a char event needs text".into()));
            }
        };
        let mut params = json!({"type": kind, "modifiers": input.modifiers});
        if let Some(key) = &input.key {
            params["key"] = json!(key);
        }
        if let Some(code) = &input.code {
            params["code"] = json!(code);
        }
        if let Some(text) = &text {
            params["text"] = json!(text);
            params["unmodifiedText"] = json!(text);
        }
        if let Some(code) = keys::virtual_key_code(input.key.as_deref(), input.code.as_deref()) {
            params["windowsVirtualKeyCode"] = json!(code);
            params["nativeVirtualKeyCode"] = json!(code);
        }
        let session = self.session(tab).await?;
        let _input = self.input_lock.lock().await;
        self.cdp
            .call_session(&session, "Input.dispatchKeyEvent", params)
            .await?;
        Ok(())
    }

    /// Applies one viewer WebSocket message; returns the new tab for `open`.
    ///
    /// # Errors
    /// The failure of the underlying operation; map it with [`BrowserError::viewer_code`].
    pub async fn apply_viewer_msg(
        &self,
        message: ViewerClientMsg,
    ) -> Result<Option<TabId>, BrowserError> {
        match message {
            ViewerClientMsg::Open { url } => self.open(&url).await.map(Some),
            ViewerClientMsg::Navigate { tab_id, url } => {
                self.navigate(&tab_id, &url).await.map(|()| None)
            }
            ViewerClientMsg::Activate { tab_id } => self.activate(&tab_id).await.map(|()| None),
            ViewerClientMsg::Close { tab_id } => self.close(&tab_id).await.map(|()| None),
            ViewerClientMsg::Mouse {
                tab_id,
                event,
                x,
                y,
                button,
                dx,
                dy,
            } => {
                let input = MouseInput {
                    event,
                    x,
                    y,
                    button: button.unwrap_or_default(),
                    dx: dx.unwrap_or(0.0),
                    dy: dy.unwrap_or(0.0),
                    modifiers: 0,
                };
                self.dispatch_mouse(&tab_id, input).await.map(|()| None)
            }
            ViewerClientMsg::Key {
                tab_id,
                event,
                key,
                code,
                text,
                modifiers,
            } => {
                let input = KeyInput {
                    event,
                    key,
                    code,
                    text,
                    modifiers: modifiers.unwrap_or(0),
                };
                self.dispatch_key(&tab_id, input).await.map(|()| None)
            }
            ViewerClientMsg::Viewport { width, height } => {
                self.set_viewport(width, height).await.map(|()| None)
            }
            // Per-connection choices, handled by the viewer connection.
            ViewerClientMsg::View { .. }
            | ViewerClientMsg::DomResync
            | ViewerClientMsg::Watch { .. } => Ok(None),
        }
    }

    async fn navigate_and_wait(
        &self,
        tab: &TabId,
        url: &str,
    ) -> Result<NavigationResult, BrowserError> {
        let session = self.session(tab).await?;
        let mut events = self.cdp.subscribe_events();
        self.navigate_session(tab, &session, url).await?;
        let loaded = wait_for_load(&mut events, &session, self.config.load_timeout).await;
        self.refresh_tab(tab).await;
        Ok(NavigationResult {
            tab: self.tab_info(tab)?,
            loaded,
        })
    }

    /// `browser_tabs`.
    #[must_use]
    pub fn tool_tabs(&self) -> TabsResult {
        TabsResult { tabs: self.tabs() }
    }

    /// `browser_open`: opens a tab, navigates it and waits (bounded) for the load event.
    ///
    /// # Errors
    /// As [`Self::open`] plus [`BrowserError::NavigationFailed`].
    pub async fn tool_open(&self, params: &OpenParams) -> Result<NavigationResult, BrowserError> {
        self.touch();
        let url = normalize_url(&params.url)?;
        self.check_allowed(&url)?;
        let tab = self.create_tab("about:blank").await?;
        self.navigate_and_wait(&tab, &url).await
    }

    /// `browser_navigate`: navigates and waits (bounded) for the load event.
    ///
    /// # Errors
    /// As [`Self::navigate`].
    pub async fn tool_navigate(
        &self,
        params: &NavigateParams,
    ) -> Result<NavigationResult, BrowserError> {
        self.touch();
        let url = normalize_url(&params.url)?;
        self.check_allowed(&url)?;
        self.navigate_and_wait(&params.tab_id, &url).await
    }

    async fn call_on_document(
        &self,
        session: &str,
        function: &str,
        argument: &str,
    ) -> Result<Value, BrowserError> {
        let document = self
            .cdp
            .call_session(
                session,
                "Runtime.evaluate",
                json!({"expression": "document", "returnByValue": false}),
            )
            .await?;
        let object_id = document
            .get("result")
            .and_then(|r| r.get("objectId"))
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("Runtime.evaluate", "missing document objectId"))?
            .to_owned();
        // The selector travels as a call argument, never spliced into code.
        let outcome = self
            .cdp
            .call_session(
                session,
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": function,
                    "arguments": [{"value": argument}],
                    "returnByValue": true,
                }),
            )
            .await;
        if let Err(error) = self
            .cdp
            .call_session(
                session,
                "Runtime.releaseObject",
                json!({"objectId": object_id}),
            )
            .await
        {
            debug!(%error, "releasing document handle failed");
        }
        let outcome = outcome?;
        if let Some(details) = outcome.get("exceptionDetails") {
            return Err(BrowserError::InvalidSelector(exception_text(details)));
        }
        Ok(outcome
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn element_center(
        &self,
        session: &str,
        selector: &str,
    ) -> Result<(f64, f64), BrowserError> {
        let value = self
            .call_on_document(session, CENTER_FUNCTION, selector)
            .await?;
        if value.get("found").and_then(Value::as_bool) != Some(true) {
            return Err(BrowserError::ElementNotFound);
        }
        let number = |name: &str| value.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        if number("width") <= 0.0 || number("height") <= 0.0 {
            return Err(BrowserError::ElementNotVisible);
        }
        Ok((number("x"), number("y")))
    }

    /// `browser_click`: left-clicks an element's centre (scrolled into view) or a point.
    ///
    /// # Errors
    /// Parameter errors, [`BrowserError::ElementNotFound`], [`BrowserError::ElementNotVisible`],
    /// [`BrowserError::InvalidSelector`], tab or CDP failures.
    pub async fn tool_click(&self, params: &ClickParams) -> Result<ClickResult, BrowserError> {
        self.touch();
        let target = params.target()?;
        let session = self.session(&params.tab_id).await?;
        let (x, y) = match &target {
            ClickTarget::Point { x, y } => {
                check_coordinate("x", *x)?;
                check_coordinate("y", *y)?;
                (*x, *y)
            }
            ClickTarget::Selector(selector) => self.element_center(&session, selector).await?,
        };
        let _input = self.input_lock.lock().await;
        for (kind, button, buttons, count) in [
            ("mouseMoved", "none", 0, 0),
            ("mousePressed", "left", 1, 1),
            ("mouseReleased", "left", 0, 1),
        ] {
            self.cdp
                .call_session(
                    &session,
                    "Input.dispatchMouseEvent",
                    json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons,
                           "clickCount": count, "modifiers": 0}),
                )
                .await?;
        }
        Ok(ClickResult { x, y })
    }

    async fn press_enter(&self, session: &str) -> Result<(), BrowserError> {
        for kind in ["keyDown", "keyUp"] {
            let mut params = json!({"type": kind, "key": "Enter", "code": "Enter",
                "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13, "modifiers": 0});
            if kind == "keyDown" {
                params["text"] = json!("\r");
                params["unmodifiedText"] = json!("\r");
            }
            self.cdp
                .call_session(session, "Input.dispatchKeyEvent", params)
                .await?;
        }
        Ok(())
    }

    /// `browser_type`: focuses `selector` (if given) and inserts the text; each `\n` presses
    /// Enter.
    ///
    /// # Errors
    /// Parameter errors, [`BrowserError::ElementNotFound`], tab or CDP failures.
    pub async fn tool_type(&self, params: &TypeParams) -> Result<TypeResult, BrowserError> {
        self.touch();
        params.validate()?;
        let session = self.session(&params.tab_id).await?;
        if let Some(selector) = &params.selector {
            let value = self
                .call_on_document(&session, FOCUS_FUNCTION, selector)
                .await?;
            if value.get("found").and_then(Value::as_bool) != Some(true) {
                return Err(BrowserError::ElementNotFound);
            }
        }
        let _input = self.input_lock.lock().await;
        let mut chars = 0;
        for (index, line) in params.text.split('\n').enumerate() {
            if index > 0 {
                self.press_enter(&session).await?;
                chars += 1;
            }
            let line = line.strip_suffix('\r').unwrap_or(line);
            if !line.is_empty() {
                self.cdp
                    .call_session(&session, "Input.insertText", json!({"text": line}))
                    .await?;
                chars += line.chars().count();
            }
        }
        Ok(TypeResult { chars })
    }

    async fn text_fallback(&self, session: &str) -> Result<(String, bool), BrowserError> {
        let result = self
            .cdp
            .call_session(
                session,
                "Runtime.evaluate",
                json!({"expression": TEXT_FALLBACK_EXPRESSION, "returnByValue": true}),
            )
            .await?;
        let mut text = result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let truncated = snapshot::truncate_utf8(&mut text, SNAPSHOT_MAX_BYTES);
        Ok((text, truncated))
    }

    /// `browser_snapshot`: accessibility outline (falls back to the page's visible text when
    /// the accessibility tree is unavailable), at most [`SNAPSHOT_MAX_BYTES`].
    ///
    /// # Errors
    /// Tab or CDP transport failures.
    pub async fn tool_snapshot(
        &self,
        params: &SnapshotParams,
    ) -> Result<SnapshotResult, BrowserError> {
        self.touch();
        let session = self.session(&params.tab_id).await?;
        let tree = self
            .cdp
            .call_session(&session, "Accessibility.getFullAXTree", json!({}))
            .await;
        let (outline, truncated) = match tree {
            Ok(result) => {
                let nodes = result
                    .get("nodes")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let (outline, truncated) = snapshot::outline(nodes, SNAPSHOT_MAX_BYTES);
                if outline.is_empty() {
                    self.text_fallback(&session).await?
                } else {
                    (outline, truncated)
                }
            }
            Err(CdpError::Protocol { message, .. }) => {
                debug!(%message, "accessibility tree unavailable; using page text");
                self.text_fallback(&session).await?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(SnapshotResult {
            tab: self.tab_info(&params.tab_id)?,
            outline,
            truncated,
        })
    }

    /// `browser_screenshot`: PNG of the viewport as base64, at most
    /// [`SCREENSHOT_MAX_BASE64_BYTES`].
    ///
    /// # Errors
    /// [`BrowserError::ResultTooLarge`], tab or CDP failures.
    pub async fn tool_screenshot(
        &self,
        params: &ScreenshotParams,
    ) -> Result<ScreenshotResult, BrowserError> {
        self.touch();
        let session = self.session(&params.tab_id).await?;
        let shot = self
            .cdp
            .call_session(&session, "Page.captureScreenshot", json!({"format": "png"}))
            .await?;
        let data = shot
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("Page.captureScreenshot", "missing data"))?;
        if data.len() > SCREENSHOT_MAX_BASE64_BYTES {
            return Err(BrowserError::ResultTooLarge {
                limit: SCREENSHOT_MAX_BASE64_BYTES,
            });
        }
        Ok(ScreenshotResult {
            mime_type: "image/png".to_owned(),
            data_base64: data.to_owned(),
        })
    }

    /// `browser_screenshot` within a byte budget (e.g. an MCP result cap): a PNG of the
    /// viewport when it fits, otherwise JPEGs of decreasing quality and scale
    /// ([`BOUNDED_SCREENSHOT_STEPS`]) until one fits. The result says which step was used so
    /// callers can map coordinates of a downscaled image back to the viewport.
    ///
    /// # Errors
    /// [`BrowserError::ResultTooLarge`] when even the smallest step exceeds the budget; tab or
    /// CDP failures.
    pub async fn tool_screenshot_within(
        &self,
        params: &ScreenshotParams,
        max_base64_bytes: usize,
    ) -> Result<BoundedScreenshot, BrowserError> {
        self.touch();
        let session = self.session(&params.tab_id).await?;
        let viewport = lock(&self.state).viewport;
        let limit = max_base64_bytes.min(SCREENSHOT_MAX_BASE64_BYTES);
        for step in BOUNDED_SCREENSHOT_STEPS {
            let mut request = match step.quality {
                None => json!({"format": "png"}),
                Some(quality) => json!({"format": "jpeg", "quality": quality}),
            };
            if step.scale < 1.0 {
                request["clip"] = json!({"x": 0, "y": 0, "width": viewport.0,
                    "height": viewport.1, "scale": step.scale});
            }
            let shot = self
                .cdp
                .call_session(&session, "Page.captureScreenshot", request)
                .await?;
            let data = shot
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("Page.captureScreenshot", "missing data"))?;
            if data.len() <= limit {
                let mime_type = if step.quality.is_some() {
                    "image/jpeg"
                } else {
                    "image/png"
                };
                return Ok(BoundedScreenshot {
                    result: ScreenshotResult {
                        mime_type: mime_type.to_owned(),
                        data_base64: data.to_owned(),
                    },
                    quality: step.quality,
                    scale: step.scale,
                    viewport,
                });
            }
        }
        Err(BrowserError::ResultTooLarge { limit })
    }

    /// `browser_request_user_action`: records a pending request on the tab (shown in tab
    /// lists) and emits [`BrowserEvent::ActionRequested`]. A newer request replaces an older
    /// one on the same tab.
    ///
    /// # Errors
    /// [`BrowserError::InvalidInput`] for an empty or oversized reason;
    /// [`BrowserError::TabNotFound`].
    pub fn request_user_action(
        &self,
        tab: &TabId,
        reason: &str,
        task_id: Option<&TaskId>,
    ) -> Result<ActionRequest, BrowserError> {
        self.touch();
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > ACTION_REASON_MAX_BYTES {
            return Err(BrowserError::InvalidInput(format!(
                "reason must be 1..={ACTION_REASON_MAX_BYTES} bytes"
            )));
        }
        let request = ActionRequest {
            reason: reason.to_owned(),
            task_id: task_id.map(|task| task.as_str().to_owned()),
        };
        {
            let mut state = lock(&self.state);
            if !state.has(tab) {
                return Err(BrowserError::TabNotFound(tab.clone()));
            }
            state.actions.insert(tab.clone(), request.clone());
        }
        self.publish();
        let _ = self.events_tx.send(BrowserEvent::ActionRequested {
            tab_id: tab.clone(),
            reason: request.reason.clone(),
            task_id: request.task_id.clone(),
        });
        Ok(request)
    }

    /// Clears a tab's pending user-action request; returns whether one existed.
    pub fn clear_action(&self, tab: &TabId) -> bool {
        let removed = lock(&self.state).actions.remove(tab).is_some();
        if removed {
            self.publish();
            let _ = self.events_tx.send(BrowserEvent::ActionCleared {
                tab_id: tab.clone(),
            });
        }
        removed
    }

    /// Stops the owned process: `Browser.close` (flushes the profile), then the escalation in
    /// `timeouts`; the process is reaped before this returns `Ok`.
    pub(crate) async fn terminate(&self, timeouts: StopTimeouts) -> Result<(), BrowserError> {
        let Some(process) = self.process.get() else {
            return Ok(());
        };
        if !process.has_exited() && !self.cdp.is_closed() && !timeouts.graceful.is_zero() {
            let closing = self.cdp.call("Browser.close", json!({}));
            // The browser may drop the socket before answering; either outcome is fine.
            let _ = tokio::time::timeout(timeouts.graceful, closing).await;
        }
        process
            .terminate(timeouts)
            .await
            .map(|_| ())
            .map_err(|_| BrowserError::NotReaped { pid: process.pid() })
    }
}

async fn event_loop(
    instance: Weak<BrowserInstance>,
    mut events: broadcast::Receiver<CdpEvent>,
    mut control: mpsc::Receiver<TabId>,
    mut closed: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(instance) = instance.upgrade() else { return };
                match event {
                    Ok(event) => {
                        instance.on_event(&event);
                        let refresh = matches!(
                            event.method.as_str(),
                            "Page.loadEventFired" | "Page.domContentEventFired" | "Page.navigatedWithinDocument"
                        );
                        if let (true, Some(session)) = (refresh, event.session_id.as_deref()) {
                            instance.refresh_session_tab(session).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "browser events lagged; resynchronizing targets");
                        if let Err(error) = instance.resync().await {
                            warn!(%error, "target resynchronization failed");
                        }
                        // Lost recorder batches leave mirrors inconsistent: start over.
                        instance.checkout_mirrors();
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        instance.on_disconnected();
                        return;
                    }
                }
            }
            tab = control.recv() => {
                let (Some(_), Some(instance)) = (tab, instance.upgrade()) else { return };
                instance.stop_unwatched_screencasts().await;
                instance.stop_unwatched_mirrors().await;
            }
            () = wait_closed(&mut closed) => {
                if let Some(instance) = instance.upgrade() {
                    instance.on_disconnected();
                }
                return;
            }
        }
    }
}
