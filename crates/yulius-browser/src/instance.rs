//! One running sandbox browser: tab registry, per-tab sessions, video or screencast, input and
//! the agent tool functions (SPEC-001 BRW-001..008, SPEC-002 ADR-15).
//!
//! Users (viewer WebSocket) and agents (MCP tools) act on the same tabs. With a virtual display
//! the browser is headful: users get its video and their input is real (XTest) on the display,
//! which shows one tab at a time (the active tab); agents use CDP. Without one (headless) users
//! get JPEG frames and CDP input. All operations are bounded by the CDP call timeout; input
//! events are serialized per browser so their order is preserved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use yulius_contracts::browser::{
    ACTION_REASON_MAX_BYTES, ActionRequest, CLIPBOARD_MAX_BYTES, ClickParams, ClickResult,
    ClickTarget, KeyEventKind, MODIFIER_ALT, MODIFIER_CTRL, MODIFIER_META, MODIFIER_SHIFT,
    MouseButton, MouseEventKind, NavigateParams, NavigationResult, OpenParams,
    SCREENSHOT_MAX_BASE64_BYTES, SNAPSHOT_MAX_BYTES, ScreenshotParams, ScreenshotResult,
    SnapshotParams, SnapshotResult, TabId, TabInfo, TabsResult, TypeParams, TypeResult,
    ViewerClientMsg,
};
use yulius_domain::ids::{SandboxId, TaskId};

use crate::cdp::{CdpClient, CdpConfig, CdpEvent, FrameSink, ScreencastFrame, wait_closed};
use crate::display::VirtualDisplay;
use crate::error::{BrowserError, CdpError};
use crate::keys::{self, DisplayKey, SHIFT_KEYCODES};
use crate::process::{StopTimeouts, Supervised};
use crate::snapshot;
use crate::url_policy::{UrlDenyList, UrlRefused, normalize_url};
use crate::video::{VideoSubscription, VideoUnavailable};
use crate::wheel::{WheelDispatcher, WheelEvent};

/// Smallest accepted viewport edge in CSS pixels.
pub const VIEWPORT_MIN: u32 = 64;
/// Largest accepted viewport edge in CSS pixels.
pub const VIEWPORT_MAX: u32 = 4096;
/// Largest accepted absolute coordinate or wheel delta.
const COORDINATE_LIMIT: f64 = 1.0e6;
/// Largest accepted `key`/`code` text.
const KEY_FIELD_MAX_BYTES: usize = 64;
/// Largest accepted text of one key event (a paste).
const KEY_TEXT_MAX_BYTES: usize = 32 * 1024;
/// Text up to this many characters is typed as real key presses on a display (in order with
/// other keys); longer text (a paste) is inserted through CDP once the display settled.
const TYPED_TEXT_MAX_CHARS: usize = 64;
/// Presses closer than this in time and space count as a multi-click.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f64 = 5.0;
/// Unsubscribe notifications queued for the event task.
const CONTROL_QUEUE: usize = 64;
/// Browser events buffered per subscriber.
const EVENT_QUEUE: usize = 64;
/// Reads the user's selection in the shown tab (the focused text field's, or the page's).
const SELECTION_EXPRESSION: &str = "(() => {
  let doc = document;
  let active = doc.activeElement;
  try { while (active && active.tagName === 'IFRAME' && active.contentDocument) { doc = active.contentDocument; active = doc.activeElement; } } catch (_) {}
  const field = active && (active.tagName === 'TEXTAREA' || (active.tagName === 'INPUT' && /^(text|search|url|tel|email|number)$/i.test(active.type)));
  if (field && typeof active.selectionStart === 'number') return active.value.slice(active.selectionStart, active.selectionEnd);
  const selection = doc.getSelection ? doc.getSelection() : null;
  return selection ? String(selection) : '';
})()";

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
    /// The viewport of all tabs changed (viewers scale the page to it).
    Viewport {
        /// Width in CSS pixels.
        width: u32,
        /// Height in CSS pixels.
        height: u32,
    },
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

struct State {
    tabs: Vec<TabEntry>,
    active: Option<TabId>,
    sessions: HashMap<TabId, String>,
    actions: HashMap<TabId, ActionRequest>,
    screencasts: HashMap<TabId, Screencast>,
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

/// Real keyboard state of a virtual display: which keycode each viewer key pressed (so its
/// release matches even if the character changed meanwhile) and which Shift keys are down.
#[derive(Debug, Default)]
struct Keyboard {
    pressed: HashMap<String, u8>,
    shift: Vec<u8>,
    /// Headless: modifier bits of the modifier keys a viewer holds (CDP needs them on every key
    /// event; a viewer mapping ⌘C to Ctrl+C sends the Control key and then a plain C).
    modifiers: u8,
}

impl Keyboard {
    /// Whether Ctrl, Alt or Meta is down: a shortcut, whose Shift state is the user's own
    /// (Ctrl+Shift+Z stays Ctrl+Shift+Z).
    fn shortcut(&self) -> bool {
        self.pressed
            .values()
            .any(|keycode| keys::SHORTCUT_KEYCODES.contains(keycode))
    }
}

/// A running (or attached) Chromium shared by a sandbox's users and agents.
pub struct BrowserInstance {
    sandbox: SandboxId,
    cdp: CdpClient,
    /// Viewers' wheel input, coalesced so it never queues up behind Chrome (F 2026-10-10).
    wheel: WheelDispatcher,
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
    /// The virtual display the browser runs on (headful), when it has one.
    display: Option<VirtualDisplay>,
    keyboard: Mutex<Keyboard>,
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
    /// Attaches to a DevTools WebSocket without owning a process (tests, externally managed
    /// browsers). [`crate::BrowserManager::ensure`] attaches over the browser's pipe instead.
    ///
    /// # Errors
    /// CDP connection or target discovery failures.
    pub async fn attach(
        sandbox: SandboxId,
        ws_url: &str,
        config: InstanceConfig,
    ) -> Result<Arc<Self>, BrowserError> {
        let cdp = CdpClient::connect(ws_url, config.cdp).await?;
        Self::attach_on(sandbox, cdp, config, None).await
    }

    /// Attaches over a connected client to a browser running on `display` (the instance owns
    /// the display and stops it after the browser, or at once when attaching fails).
    pub(crate) async fn attach_on(
        sandbox: SandboxId,
        cdp: CdpClient,
        config: InstanceConfig,
        display: Option<VirtualDisplay>,
    ) -> Result<Arc<Self>, BrowserError> {
        let viewport = match &display {
            Some(display) => display.config().css_size(display.size()),
            None => config.viewport,
        };
        let connected = async {
            let events = cdp.subscribe_events();
            cdp.call("Target.setDiscoverTargets", json!({"discover": true}))
                .await?;
            Ok::<_, BrowserError>((cdp, events))
        };
        let (cdp, events) = match connected.await {
            Ok(connected) => connected,
            Err(error) => {
                if let Some(display) = &display {
                    let _ = display.stop(StopTimeouts::default()).await;
                }
                return Err(error);
            }
        };
        let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
        let (events_tx, _) = broadcast::channel(EVENT_QUEUE);
        let closed = cdp.close_signal();
        let wheel = WheelDispatcher::new(cdp.clone());
        let instance = Arc::new(Self {
            sandbox,
            cdp,
            wheel,
            config,
            state: Mutex::new(State {
                tabs: Vec::new(),
                active: None,
                sessions: HashMap::new(),
                actions: HashMap::new(),
                screencasts: HashMap::new(),
                viewport,
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
            display,
            keyboard: Mutex::new(Keyboard::default()),
            new_tab_url: Mutex::new(None),
            deny: Mutex::new(UrlDenyList::default()),
        });
        if let Err(error) = instance.resync().await {
            if let Some(display) = &instance.display {
                let _ = display.stop(StopTimeouts::default()).await;
            }
            return Err(error);
        }
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

    /// The underlying CDP client, for in-process callers needing methods this adapter does not
    /// wrap. Nothing outside Yulius can reach the browser's DevTools (ADR-0191).
    #[must_use]
    pub fn cdp(&self) -> &CdpClient {
        &self.cdp
    }

    /// Chromium's process id when this instance owns the process.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.process.get().map(Supervised::pid)
    }

    /// Whether the DevTools connection is open and the owned process (and display, if any)
    /// runs.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.cdp.is_closed()
            && self.process.get().is_none_or(|p| !p.has_exited())
            && self.display.as_ref().is_none_or(VirtualDisplay::is_alive)
    }

    /// The virtual display, when the browser runs on one.
    #[must_use]
    pub fn display(&self) -> Option<&VirtualDisplay> {
        self.display.as_ref()
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

    /// Live viewers' streams: screencast subscribers across tabs plus video subscribers
    /// (viewers watching keep the browser busy).
    #[must_use]
    pub fn frame_subscribers(&self) -> usize {
        let video = self
            .display
            .as_ref()
            .map_or(0, |display| display.video().subscribers());
        lock(&self.state)
            .screencasts
            .values()
            .map(|cast| cast.sink.receiver_count())
            .sum::<usize>()
            + video
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
                let info = event.params.get("targetInfo");
                if let Some((id, title, url)) = info.and_then(page_entry) {
                    // A page a page opened (a link to a new tab, a popup) comes to the front on
                    // the display: it is the shown tab now.
                    let opened = event.method == "Target.targetCreated"
                        && self.display.is_some()
                        && info
                            .and_then(|info| info.get("openerId"))
                            .and_then(Value::as_str)
                            .is_some_and(|opener| !opener.is_empty());
                    let mut state = lock(&self.state);
                    state.upsert(id.clone(), title, url);
                    if opened {
                        state.active = Some(id);
                    }
                    drop(state);
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
            "Fetch.requestPaused" if event.session_id.is_none() => self.on_request_paused(event),
            "Target.detachedFromTarget" => {
                if let Some(session) = event.params.get("sessionId").and_then(Value::as_str) {
                    {
                        let mut state = lock(&self.state);
                        state.sessions.retain(|_, s| s != session);
                        state.screencasts.retain(|_, cast| cast.session != session);
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
        let (session, had_action, existed, front) = {
            let mut state = lock(&self.state);
            let existed = state.has(tab);
            let was_active = state.active.as_ref() == Some(tab);
            let (session, had_action) = state.remove(tab);
            let front = was_active.then(|| state.active.clone()).flatten();
            (session, had_action, existed, front)
        };
        // The display shows the active tab: bring the new one to the front so what Chrome shows
        // and what the panel says agree.
        if let (Some(front), true) = (front, self.display.is_some()) {
            let cdp = self.cdp.clone();
            tokio::spawn(async move {
                if let Err(error) = cdp
                    .call("Target.activateTarget", json!({"targetId": front.as_str()}))
                    .await
                {
                    debug!(%error, "showing the next tab failed");
                }
            });
        }
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
        // On a display the window has the viewport's size: no emulation (a page could tell).
        if self.display.is_none() {
            self.cdp
                .call_session(
                    &session,
                    "Emulation.setDeviceMetricsOverride",
                    metrics(width, height),
                )
                .await?;
        }
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
        if latest != (width, height) && self.display.is_none() {
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

    /// Moves `delta` entries through a tab's history (back is -1); beyond its ends nothing
    /// happens. An entry that is Yulius itself is refused like any navigation (BRW-008).
    ///
    /// # Errors
    /// [`BrowserError::UrlRefused`], [`BrowserError::TabNotFound`] or CDP failures.
    pub async fn history(&self, tab: &TabId, delta: i32) -> Result<(), BrowserError> {
        self.touch();
        let session = self.session(tab).await?;
        let history = self
            .cdp
            .call_session(&session, "Page.getNavigationHistory", json!({}))
            .await?;
        let current = history
            .get("currentIndex")
            .and_then(Value::as_i64)
            .ok_or_else(|| malformed("Page.getNavigationHistory", "missing currentIndex"))?;
        let entries = history
            .get("entries")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let target = current + i64::from(delta.clamp(-100, 100));
        let Some(entry) = usize::try_from(target)
            .ok()
            .and_then(|index| entries.get(index))
        else {
            return Ok(());
        };
        if let Some(url) = entry.get("url").and_then(Value::as_str) {
            self.check_allowed(url)?;
        }
        let id = entry
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| malformed("Page.getNavigationHistory", "entry without id"))?;
        self.cdp
            .call_session(
                &session,
                "Page.navigateToHistoryEntry",
                json!({"entryId": id}),
            )
            .await?;
        Ok(())
    }

    /// Reloads a tab.
    ///
    /// # Errors
    /// [`BrowserError::TabNotFound`] or CDP failures.
    pub async fn reload(&self, tab: &TabId) -> Result<(), BrowserError> {
        self.touch();
        let session = self.session(tab).await?;
        self.cdp
            .call_session(&session, "Page.reload", json!({}))
            .await?;
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
        // On a display the screen and the windows take the size (clamped to its maximum).
        let (width, height) = match &self.display {
            Some(display) => display.resize((width, height))?,
            None => (width, height),
        };
        let (sessions, casts) = {
            let mut state = lock(&self.state);
            state.viewport = (width, height);
            let sessions: Vec<String> = if self.display.is_some() {
                Vec::new()
            } else {
                state.sessions.values().cloned().collect()
            };
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
        if let Some(display) = &self.display {
            return self.display_mouse(display, input).await;
        }
        let session = self.session(tab).await?;
        if input.event == MouseEventKind::Wheel {
            let buttons = lock(&self.state).pointer.apply(&input, Instant::now()).2;
            self.push_wheel(session, &input, buttons);
            return Ok(());
        }
        let _input = self.input_lock.lock().await;
        let (kind, click_count, buttons) = lock(&self.state).pointer.apply(&input, Instant::now());
        let params = json!({
            "type": kind,
            "x": input.x,
            "y": input.y,
            "button": input.button.as_cdp(),
            "buttons": buttons,
            "clickCount": click_count,
            "modifiers": input.modifiers & 0x0F,
        });
        self.cdp
            .call_session(&session, "Input.dispatchMouseEvent", params)
            .await?;
        Ok(())
    }

    /// Real pointer input on the display: moves, presses and releases go through XTest wherever
    /// the pointer is (any window, any frame); the wheel scrolls the shown tab by CSS pixels
    /// (smooth for trackpads, which X wheel buttons are not).
    async fn display_mouse(
        &self,
        display: &VirtualDisplay,
        input: MouseInput,
    ) -> Result<(), BrowserError> {
        match input.event {
            MouseEventKind::Move => display.pointer(input.x, input.y),
            MouseEventKind::Down | MouseEventKind::Up => {
                display.pointer(input.x, input.y)?;
                display.button(input.button, input.event == MouseEventKind::Down)
            }
            MouseEventKind::Wheel => {
                let Some(tab) = lock(&self.state).active.clone() else {
                    return Ok(());
                };
                let session = self.session(&tab).await?;
                self.push_wheel(session, &input, 0);
                Ok(())
            }
        }
    }

    /// Queues a wheel event without waiting for Chrome ([`WheelDispatcher`]): a fast trackpad
    /// is summed while dispatches are outstanding instead of queueing up.
    fn push_wheel(&self, session: String, input: &MouseInput, buttons: u32) {
        self.wheel.push(WheelEvent {
            session,
            x: input.x,
            y: input.y,
            dx: input.dx,
            dy: input.dy,
            modifiers: input.modifiers & 0x0F,
            buttons,
        });
    }

    /// Wheel events summed into others because Chrome had not answered yet.
    #[must_use]
    pub fn wheel_coalesced(&self) -> u64 {
        self.wheel.coalesced()
    }

    /// Inserts text in the shown tab's focused field (IME, emoji, non-ASCII letters, paste).
    async fn insert_text(&self, text: &str) -> Result<(), BrowserError> {
        let Some(tab) = lock(&self.state).active.clone() else {
            return Ok(());
        };
        let session = self.session(&tab).await?;
        let _input = self.input_lock.lock().await;
        self.cdp
            .call_session(&session, "Input.insertText", json!({"text": text}))
            .await?;
        Ok(())
    }

    /// Real keyboard input on the display (see [`crate::keys`]): printable ASCII by character,
    /// pressing or lifting Shift around the key when the character needs another Shift state
    /// than the user's; other keys by code; text no key types through `Input.insertText`.
    async fn display_key(
        &self,
        display: &VirtualDisplay,
        input: KeyInput,
    ) -> Result<(), BrowserError> {
        let id = input
            .code
            .clone()
            .filter(|code| !code.is_empty())
            .or_else(|| input.key.clone())
            .unwrap_or_default();
        match input.event {
            KeyEventKind::Char => match input.text.as_deref().filter(|t| !t.is_empty()) {
                Some(text) if text.chars().count() <= TYPED_TEXT_MAX_CHARS => text
                    .chars()
                    .filter(|c| *c != '\r')
                    .try_for_each(|c| display.type_char(c)),
                Some(text) => {
                    display.settle().await;
                    self.insert_text(text).await
                }
                None => Err(BrowserError::InvalidInput("a char event needs text".into())),
            },
            KeyEventKind::Down => {
                match keys::display_key(input.key.as_deref(), input.code.as_deref()) {
                    DisplayKey::Char { keycode, shift } => {
                        let (held, shortcut) = {
                            let mut keyboard = lock(&self.keyboard);
                            let shortcut = keyboard.shortcut();
                            keyboard.pressed.insert(id, keycode);
                            (keyboard.shift.clone(), shortcut)
                        };
                        if shortcut {
                            return display.key(keycode, true);
                        }
                        match (shift, held.is_empty()) {
                            (true, true) => {
                                display.key(SHIFT_KEYCODES[0], true)?;
                                display.key(keycode, true)?;
                                display.key(SHIFT_KEYCODES[0], false)
                            }
                            (false, false) => {
                                for shift in &held {
                                    display.key(*shift, false)?;
                                }
                                display.key(keycode, true)?;
                                for shift in &held {
                                    display.key(*shift, true)?;
                                }
                                Ok(())
                            }
                            _ => display.key(keycode, true),
                        }
                    }
                    DisplayKey::Key(keycode) => {
                        {
                            let mut keyboard = lock(&self.keyboard);
                            if SHIFT_KEYCODES.contains(&keycode)
                                && !keyboard.shift.contains(&keycode)
                            {
                                keyboard.shift.push(keycode);
                            }
                            keyboard.pressed.insert(id, keycode);
                        }
                        display.key(keycode, true)
                    }
                    DisplayKey::Text(text) => text.chars().try_for_each(|c| display.type_char(c)),
                    DisplayKey::Ignore => Ok(()),
                }
            }
            KeyEventKind::Up => {
                let keycode = {
                    let mut keyboard = lock(&self.keyboard);
                    let keycode =
                        keyboard.pressed.remove(&id).or_else(|| {
                            match keys::display_key(input.key.as_deref(), input.code.as_deref()) {
                                DisplayKey::Key(keycode) | DisplayKey::Char { keycode, .. } => {
                                    Some(keycode)
                                }
                                DisplayKey::Text(_) | DisplayKey::Ignore => None,
                            }
                        });
                    if let Some(keycode) = keycode {
                        keyboard.shift.retain(|shift| *shift != keycode);
                    }
                    keycode
                };
                match keycode {
                    Some(keycode) => display.key(keycode, false),
                    None => Ok(()),
                }
            }
        }
    }

    /// Releases every key and button a viewer held (it left or reconnected).
    pub fn release_input(&self) {
        *lock(&self.keyboard) = Keyboard::default();
        if let Some(display) = &self.display {
            display.release_all();
        }
    }

    /// Subscribes to the display's video.
    ///
    /// # Errors
    /// [`VideoUnavailable`] without a display or when its encoder failed.
    pub fn subscribe_video(&self) -> Result<VideoSubscription, VideoUnavailable> {
        match &self.display {
            Some(display) => display.subscribe_video(),
            None => Err(VideoUnavailable(
                "the sandbox browser runs headless (no virtual display)".into(),
            )),
        }
    }

    /// The text selected in a tab (the focused field's selection or the page's), at most
    /// [`CLIPBOARD_MAX_BYTES`]: what the user copied.
    ///
    /// # Errors
    /// Tab or CDP failures.
    pub async fn copy_selection(&self, tab: &TabId) -> Result<String, BrowserError> {
        self.touch();
        let session = self.session(tab).await?;
        let result = self
            .cdp
            .call_session(
                &session,
                "Runtime.evaluate",
                json!({"expression": SELECTION_EXPRESSION, "returnByValue": true}),
            )
            .await?;
        let mut text = result
            .pointer("/result/value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        snapshot::truncate_utf8(&mut text, CLIPBOARD_MAX_BYTES);
        Ok(text)
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
        if let Some(display) = &self.display {
            return self.display_key(display, input).await;
        }
        let bit = match input.key.as_deref() {
            Some("Control") => MODIFIER_CTRL,
            Some("Shift") => MODIFIER_SHIFT,
            Some("Alt") => MODIFIER_ALT,
            Some("Meta") => MODIFIER_META,
            _ => 0,
        };
        let held = {
            let mut keyboard = lock(&self.keyboard);
            match input.event {
                KeyEventKind::Down => keyboard.modifiers |= bit,
                KeyEventKind::Up => keyboard.modifiers &= !bit,
                KeyEventKind::Char => {}
            }
            keyboard.modifiers
        };
        let input = KeyInput {
            modifiers: input.modifiers | held | bit,
            ..input
        };
        // Text without a key (IME, paste): a key event carries at most four characters.
        if input.event == KeyEventKind::Char
            && let Some(text) = input.text.as_deref().filter(|t| t.chars().count() > 1)
        {
            let session = self.session(tab).await?;
            let _input = self.input_lock.lock().await;
            self.cdp
                .call_session(&session, "Input.insertText", json!({"text": text}))
                .await?;
            return Ok(());
        }
        let text = match input.event {
            KeyEventKind::Up => None,
            KeyEventKind::Down | KeyEventKind::Char => input
                .text
                .clone()
                .filter(|text| !text.is_empty())
                .or_else(|| keys::implied_text(input.key.as_deref()).map(str::to_owned))
                // A printable key without Ctrl/Meta types its character (viewers send no text).
                .or_else(|| {
                    let printable = input.key.as_deref().filter(|key| {
                        key.chars().count() == 1 && !key.chars().any(char::is_control)
                    });
                    let shortcut = input.modifiers & (MODIFIER_CTRL | MODIFIER_META) != 0;
                    printable.filter(|_| !shortcut).map(str::to_owned)
                }),
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
            ViewerClientMsg::History { tab_id, delta } => {
                self.history(&tab_id, delta).await.map(|()| None)
            }
            ViewerClientMsg::Reload { tab_id } => self.reload(&tab_id).await.map(|()| None),
            ViewerClientMsg::Mouse {
                tab_id,
                event,
                x,
                y,
                button,
                dx,
                dy,
                modifiers,
            } => {
                let input = MouseInput {
                    event,
                    x,
                    y,
                    button: button.unwrap_or_default(),
                    dx: dx.unwrap_or(0.0),
                    dy: dy.unwrap_or(0.0),
                    modifiers: modifiers.unwrap_or(0) & 0x0F,
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
            // Per-connection choices and answers, handled by the viewer connection.
            ViewerClientMsg::View { .. }
            | ViewerClientMsg::Copy { .. }
            | ViewerClientMsg::Watch { .. }
            | ViewerClientMsg::Ack { .. }
            | ViewerClientMsg::Keyframe => Ok(None),
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
        let browser = match self.process.get() {
            Some(process) => {
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
            None => Ok(()),
        };
        // The display goes after the browser that draws on it (encoder and X server reaped).
        let display = match &self.display {
            Some(display) => display.stop(timeouts).await,
            None => Ok(()),
        };
        browser.and(display)
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
