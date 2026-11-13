//! One running sandbox browser: tab registry, per-tab sessions, video or screencast, input and
//! the agent tool functions (SPEC-001 BRW-001..008, SPEC-002 ADR-15).
//!
//! Users (viewer WebSocket) and agents (MCP tools) act on the same tabs. With a virtual display
//! the browser is headful: users get its video and their input is real (XTest) on the display,
//! which shows one tab at a time (the active tab); agents use CDP. Without one (headless) users
//! get JPEG frames and CDP input. All operations are bounded by the CDP call timeout; input
//! events are serialized per browser so their order is preserved.

use std::collections::{HashMap, HashSet};
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
use zeroize::Zeroizing;

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

/// Distinct secrets a sandbox's browser remembers (SPEC-450); typing another one is refused.
const TYPED_SECRETS_MAX: usize = 32;
/// Lines of multi-line secrets remembered, each matched on its own.
const TYPED_LINES_MAX: usize = 1024;
/// Documents, and hosts, with a typed secret remembered; past the bound every screenshot is
/// refused.
const TYPED_DOCUMENTS_MAX: usize = 256;
/// Name of the isolated world the secret probe runs in, out of reach of the page's scripts.
const SECRET_PROBE_WORLD: &str = "yulius-secret-probe";

/// Reads what a page can show (SPEC-450 SSC-003): the text of its documents (all but `head`),
/// open shadow roots and same-origin frames, objects and embeds, and the values and
/// placeholders of fields drawn as text (`shown`), the values of password fields (`masked`),
/// where typed text goes (`focus`: a password field and its value, a frame it cannot read,
/// other), and how many frame documents at addresses of the page's own origin it read, PDFs
/// apart (`frames`); `over` past 4 000 000 characters. It takes nothing in: Yulius compares the
/// typed values, which never reach the page. The marker comment names it for the fake.
const SECRET_PROBE_EXPRESSION: &str = "(() => {
  // yulius:secret-probe
  // Read through the prototypes: a page's named elements override a document's own properties.
  const getter = (proto, name) => Object.getOwnPropertyDescriptor(proto, name).get;
  const rootOf = getter(Document.prototype, 'documentElement');
  const kindOf = getter(Node.prototype, 'nodeType');
  const inDocument = Document.prototype.querySelectorAll;
  const inFragment = DocumentFragment.prototype.querySelectorAll;
  const textOf = getter(Node.prototype, 'textContent');
  const addressOf = getter(Document.prototype, 'URL');
  const typeOf = getter(Document.prototype, 'contentType');
  const shown = [], masked = [];
  let size = 0, over = false, frames = 0;
  const add = (list, value) => {
    const text = value == null ? '' : String(value);
    if (!text) return;
    size += text.length;
    if (size > 4000000) over = true; else list.push(text);
  };
  const walk = (root) => {
    const all = (kindOf.call(root) === 9 ? inDocument : inFragment).call(root, '*');
    for (const el of all) {
      const name = String(el.localName).toLowerCase();
      if (name === 'input') {
        const type = String(el.type).toLowerCase();
        if (type === 'password') add(masked, el.value); else if (type !== 'hidden') add(shown, el.value);
        add(shown, el.placeholder);
      } else if (name === 'textarea') { add(shown, el.value); add(shown, el.placeholder); }
      if (el.shadowRoot) { add(shown, textOf.call(el.shadowRoot)); walk(el.shadowRoot); }
      if (name === 'iframe' || name === 'frame' || name === 'object' || name === 'embed') {
        let doc = null;
        try { doc = el.contentDocument || (el.getSVGDocument ? el.getSVGDocument() : null); } catch (_) {}
        if (doc) { if (own(doc)) frames += 1; text(doc); walk(doc); }
      }
    }
  };
  // A frame document at an address of the page's own origin, as the frame tree lists it; not
  // one that holds a PDF, which has no page text.
  const own = (doc) => {
    try {
      return typeOf.call(doc) !== 'application/pdf' && new URL(addressOf.call(doc)).origin === location.origin;
    } catch (_) { return false; }
  };
  const text = (doc) => {
    const top = rootOf.call(doc);
    if (!top) return;
    for (const child of top.children) if (String(child.localName).toLowerCase() !== 'head') add(shown, textOf.call(child));
  };
  // Where typed text goes: the focused element, through open shadow roots and same-origin
  // frames. `field` and `password` are text fields of a document read here; `frame` is an
  // element holding a document that cannot be read (a frame, an object or an embed of another
  // site); `sealed` one no frame list names (a fenced frame); anything else is `other` (also
  // the host of a closed shadow root, whatever has the focus inside it).
  const focus = () => {
    let el = document.activeElement;
    for (;;) {
      if (!el) return { kind: 'none' };
      if (el.shadowRoot && el.shadowRoot.activeElement) { el = el.shadowRoot.activeElement; continue; }
      const name = String(el.localName).toLowerCase();
      if (name === 'fencedframe') return { kind: 'sealed' };
      if (name === 'iframe' || name === 'frame' || name === 'object' || name === 'embed') {
        let inner = null;
        try { inner = el.contentDocument || (el.getSVGDocument ? el.getSVGDocument() : null); } catch (_) {}
        if (!inner) return { kind: 'frame' };
        el = inner.activeElement;
        continue;
      }
      if (name === 'input' && String(el.type).toLowerCase() === 'password') return { kind: 'password', value: String(el.value) };
      if (name === 'input' || name === 'textarea' || el.isContentEditable === true) return { kind: 'field' };
      return { kind: 'other' };
    }
  };
  text(document);
  walk(document);
  return { shown, masked, over, focus: focus(), frames };
})()";

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

/// Tells whether a text holds a secret Yulius hides (the redactor's matcher), for
/// [`BrowserInstance::tool_screenshot_guarded`].
pub type SecretCheck<'a> = &'a (dyn Fn(&str) -> bool + Send + Sync + 'a);

/// What a sandbox's browsers remember about secrets typed with
/// [`BrowserInstance::tool_type_secret`] (SPEC-450). The [`crate::BrowserManager`] keeps it per
/// sandbox, so it outlives a browser that stops (idle, crash) while the site keeps what was typed.
#[derive(Clone, Default)]
pub struct SecretMemory(Arc<Mutex<TypedSecrets>>);

impl std::fmt::Debug for SecretMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretMemory").finish_non_exhaustive()
    }
}

impl SecretMemory {
    fn lock(&self) -> MutexGuard<'_, TypedSecrets> {
        lock(&self.0)
    }

    /// For a new browser: documents (loader ids) belong to the old one.
    fn restart(&self) {
        self.lock().documents.clear();
    }
}

#[derive(Clone, Default)]
struct TypedSecrets {
    /// Distinct values, at most [`TYPED_SECRETS_MAX`].
    values: Vec<Zeroizing<String>>,
    /// Lines of multi-line values (typed one by one, each may be echoed alone), at most
    /// [`TYPED_LINES_MAX`].
    lines: Vec<Zeroizing<String>>,
    /// Each document (main-frame loader id) a value was typed into, with that value.
    documents: Vec<(String, Zeroizing<String>)>,
    /// Hosts a value was typed into: a page framing one is refused (its frame is unreadable).
    hosts: Vec<String>,
    /// Hosts where a value went outside a password field or was found shown or edited: every
    /// page of them is refused (the site may show an edited copy anywhere).
    exposed: Vec<String>,
    /// A value went where a browser could not tell the page, or a bound was passed: every
    /// screenshot is refused (until Yulius restarts).
    everywhere: bool,
}

/// The lines of a multi-line `value` worth matching alone: at least 8 characters, and not a
/// PEM armour line (`-----BEGIN …-----`), which documentation shows too.
fn lines_of(value: &str) -> Vec<&str> {
    if !value.contains('\n') {
        return Vec::new();
    }
    value
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| line.chars().count() >= 8 && !line.starts_with("-----"))
        .collect()
}

/// Adds `item` to `list` unless there; `false` when the list is at `max` (not added).
fn remember(list: &mut Vec<String>, item: &str, max: usize) -> bool {
    if list.iter().any(|known| known == item) {
        return true;
    }
    if list.len() >= max {
        return false;
    }
    list.push(item.to_owned());
    true
}

/// The host of an origin or URL (`scheme://host[:port]…`); `None` for opaque ones
/// (`about:blank`, `null`).
fn host_of(origin: &str) -> Option<String> {
    let (_, rest) = origin.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// `scheme://host[:port]` of an origin or URL, lowercased; `None` for opaque ones.
fn origin_of(text: &str) -> Option<String> {
    let (scheme, rest) = text.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!scheme.is_empty() && !authority.is_empty())
        .then(|| format!("{scheme}://{authority}").to_ascii_lowercase())
}

/// The address of Chrome's and Chromium's PDF viewer, a built-in extension: it shows a PDF in a
/// frame of its own at the PDF's address.
const PDF_VIEWER: &str = "chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/";

/// The document a tab's main frame shows, from `Page.getFrameTree`.
struct Document {
    frame: String,
    loader: String,
    /// The main frame's address.
    address: String,
    /// Its host; `None` for an opaque origin.
    host: Option<String>,
    /// Hosts of its frames the probe cannot read: those of another origin in the frame tree,
    /// the `iframe` targets of out-of-process frames ([`Document::add_frame_targets`]), which
    /// the tree may list without their address or not at all, and its own host when the probe
    /// did not read every frame of its own origin ([`Document::add_unread_frames`]).
    framed: Vec<String>,
    /// The addresses of the frames of another origin and of the out-of-process frames (empty
    /// for one whose address is not said): where keys go when the focus is in such a frame.
    framed_addresses: Vec<String>,
    /// Its frames at addresses of its own origin, those holding a PDF apart (no page text). The
    /// frame tree says the origin of a frame's address, sandboxed or not, so only the probe tells
    /// whether it read them.
    own_frames: usize,
    /// Ids of its frames, the main one included.
    frames: Vec<String>,
}

impl Document {
    fn parse(tree: &Value) -> Option<Self> {
        let main = tree.pointer("/frameTree/frame")?;
        let host = |frame: &Value| {
            ["securityOrigin", "url"]
                .iter()
                .find_map(|field| frame.get(*field).and_then(Value::as_str).and_then(host_of))
        };
        // A frame of another origin (another host, port or scheme) is one the probe cannot read,
        // and so is one the tree calls opaque (sandboxed), whatever its address and whatever the
        // page's own origin: two opaque origins are never the same one.
        let origin = |frame: &Value| match frame.get("securityOrigin").and_then(Value::as_str) {
            Some(own) => origin_of(own),
            None => frame.get("url").and_then(Value::as_str).and_then(origin_of),
        };
        let own = host(main);
        let own_origin = origin(main);
        let mut framed = Vec::new();
        let mut framed_addresses = Vec::new();
        let mut own_frames = 0;
        let mut frames = Vec::new();
        let mut stack: Vec<&Value> = tree
            .pointer("/frameTree/childFrames")
            .and_then(Value::as_array)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        while let Some(child) = stack.pop() {
            if let Some(id) = child.pointer("/frame/id").and_then(Value::as_str) {
                frames.push(id.to_owned());
            }
            let child_origin = child.get("frame").and_then(origin);
            if child_origin.is_some() && child_origin == own_origin {
                let pdf = child.pointer("/frame/mimeType").and_then(Value::as_str)
                    == Some("application/pdf");
                own_frames += usize::from(!pdf);
            } else {
                framed_addresses.push(
                    child
                        .pointer("/frame/url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                );
                if let Some(child_host) = child.get("frame").and_then(host)
                    && !framed.contains(&child_host)
                {
                    framed.push(child_host);
                }
            }
            if let Some(children) = child.get("childFrames").and_then(Value::as_array) {
                stack.extend(children);
            }
        }
        let frame = main.get("id")?.as_str()?.to_owned();
        frames.push(frame.clone());
        Some(Self {
            frame,
            loader: main.get("loaderId")?.as_str()?.to_owned(),
            address: main
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            host: own,
            framed,
            framed_addresses,
            own_frames,
            frames,
        })
    }

    /// Adds the hosts of the out-of-process frames among `targets` (`Target.getTargets`) that
    /// belong to the tab `tab`: `iframe` targets whose parent frame is one of its frames, nested
    /// ones too, and, failing closed, any whose parent is not said or is no target listed (a
    /// frame inside another out-of-process frame). Also on the page's own host: the probe reads
    /// no frame in another process (Chrome runs a sandboxed frame in one). Not the PDF viewer's
    /// frames: a PDF is pixels, as an image is, with no page text.
    fn add_frame_targets(&mut self, tab: &TabId, targets: &Value) {
        let iframes: Vec<&Value> = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .map(|infos| {
                infos
                    .iter()
                    .filter(|info| info.get("type").and_then(Value::as_str) == Some("iframe"))
                    .collect()
            })
            .unwrap_or_default();
        let listed: Vec<&str> = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .map(|infos| {
                infos
                    .iter()
                    .filter_map(|info| info.get("targetId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let mut ours: Vec<String> = self.frames.clone();
        ours.push(tab.as_str().to_owned());
        let mut viewers: Vec<&str> = Vec::new();
        let mut taken = vec![false; iframes.len()];
        let mut grew = true;
        while grew {
            grew = false;
            for (index, info) in iframes.iter().enumerate() {
                if taken[index] {
                    continue;
                }
                let parent = ["parentFrameId", "parentId"]
                    .iter()
                    .find_map(|field| info.get(*field).and_then(Value::as_str));
                if parent.is_some_and(|parent| {
                    !ours.iter().any(|id| id == parent) && listed.contains(&parent)
                }) {
                    continue;
                }
                taken[index] = true;
                grew = true;
                let id = info.get("targetId").and_then(Value::as_str);
                if let Some(id) = id {
                    ours.push(id.to_owned());
                }
                let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
                if url.starts_with(PDF_VIEWER) {
                    viewers.extend(id);
                    continue;
                }
                // The viewer's frame at the PDF's address.
                if parent.is_some_and(|parent| viewers.contains(&parent)) {
                    continue;
                }
                self.framed_addresses.push(url.to_owned());
                if let Some(host) = host_of(url)
                    && !self.framed.contains(&host)
                {
                    self.framed.push(host);
                }
            }
        }
    }

    /// Adds the page's own host when `page`'s probe read fewer frames of the page's origin than
    /// the frame tree lists: one of them is out of its reach (sandboxed, or the page is).
    fn add_unread_frames(&mut self, page: &PageText) {
        if page.frames < self.own_frames
            && let Some(host) = &self.host
            && !self.framed.contains(host)
        {
            self.framed.push(host.clone());
        }
    }
}

impl TypedSecrets {
    fn any(&self) -> bool {
        !self.values.is_empty() || !self.exposed.is_empty() || self.everywhere
    }

    /// Room for `value`, before it is typed.
    fn room_for(&self, value: &str) -> bool {
        let known =
            |list: &[Zeroizing<String>], text: &str| list.iter().any(|v| v.as_str() == text);
        let lines = lines_of(value)
            .into_iter()
            .filter(|line| !known(&self.lines, line))
            .count();
        (known(&self.values, value) || self.values.len() < TYPED_SECRETS_MAX)
            && self.lines.len() + lines <= TYPED_LINES_MAX
    }

    /// Records `value` as typed into `document` (`None`: a document that could not be told).
    fn record(&mut self, document: Option<&Document>, value: &str) {
        if !self.values.iter().any(|v| v.as_str() == value) {
            self.values.push(Zeroizing::new(value.to_owned()));
        }
        for line in lines_of(value) {
            if !self.lines.iter().any(|v| v.as_str() == line) {
                self.lines.push(Zeroizing::new(line.to_owned()));
            }
        }
        let Some(document) = document else {
            self.everywhere = true;
            return;
        };
        if !self
            .documents
            .iter()
            .any(|(d, v)| *d == document.loader && v.as_str() == value)
        {
            if self.documents.len() < TYPED_DOCUMENTS_MAX {
                self.documents
                    .push((document.loader.clone(), Zeroizing::new(value.to_owned())));
            } else {
                self.everywhere = true;
            }
        }
        if let Some(host) = &document.host
            && !remember(&mut self.hosts, host, TYPED_DOCUMENTS_MAX)
        {
            self.everywhere = true;
        }
    }

    /// Refuses every page of `host` from now on (`None`: every page of this browser).
    fn expose(&mut self, host: Option<&str>) {
        match host {
            Some(host) if remember(&mut self.exposed, host, TYPED_DOCUMENTS_MAX) => {}
            _ => self.everywhere = true,
        }
    }

    /// The values typed into `document`.
    fn typed_into(&self, document: &Document) -> Vec<Zeroizing<String>> {
        self.documents
            .iter()
            .filter(|(d, _)| *d == document.loader)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// Whether a screenshot of `page`, read from `document`, may show a secret, and whether its
    /// host is to be refused from now on. It may: on an exposed host; framing a host a value was
    /// typed into; when its text or text fields hold a typed value or line (exposing it) or, by
    /// `vault`, any secret Yulius hides; when the document or the page is unknown. A value gone
    /// from a page (a single-page sign-in) is no edit: edits are caught as they are typed
    /// ([`BrowserInstance::type_text`]).
    fn judge(
        &self,
        document: Option<&Document>,
        page: Option<&PageText>,
        vault: bool,
    ) -> (bool, bool) {
        let (Some(document), Some(page)) = (document, page) else {
            return (true, false);
        };
        let exposed = |host: &String| self.exposed.contains(host);
        if self.everywhere
            || document.host.as_ref().is_some_and(exposed)
            || document
                .framed
                .iter()
                .any(|host| exposed(host) || self.hosts.contains(host))
        {
            return (true, false);
        }
        let typed = self
            .values
            .iter()
            .chain(&self.lines)
            .any(|value| page.shows(value));
        (typed || vault, typed)
    }
}

/// What a page can show, as [`SECRET_PROBE_EXPRESSION`] read it.
struct PageText {
    /// Text and the values of fields drawn as text.
    shown: Vec<Zeroizing<String>>,
    /// The values of password fields (drawn as dots).
    masked: Vec<Zeroizing<String>>,
    /// The value of the focused element when it is a password field (`None`: it is not, or it
    /// is a frame the probe cannot read).
    focused_password: Option<Zeroizing<String>>,
    /// The focused element holds a document the probe cannot read (a frame, an object or an
    /// embed of another site, a fenced frame).
    focused_frame: bool,
    /// The focused element is one no list of the page's frames names (a fenced frame).
    focus_sealed: bool,
    /// The focus is in a text field of a document the probe read: the page's own, or a frame's
    /// of its origin. Anything else (a frame, an element that takes no text, the host of a
    /// closed shadow root, nothing, a kind this code does not know) is not.
    focus_in_page: bool,
    /// The frame documents at addresses of the page's own origin it read; none when not said.
    frames: usize,
}

impl PageText {
    fn parse(value: &Value) -> Option<Self> {
        let strings = |name: &str| -> Option<Vec<Zeroizing<String>>> {
            value
                .get(name)?
                .as_array()?
                .iter()
                .map(|item| item.as_str().map(|text| Zeroizing::new(text.to_owned())))
                .collect()
        };
        if value.get("over").and_then(Value::as_bool) != Some(false) {
            return None;
        }
        let focus = value.get("focus");
        let focused_password = focus
            .filter(|focus| focus.get("kind").and_then(Value::as_str) == Some("password"))
            .and_then(|focus| focus.get("value").and_then(Value::as_str))
            .map(|text| Zeroizing::new(text.to_owned()));
        let kind = focus
            .and_then(|focus| focus.get("kind"))
            .and_then(Value::as_str);
        let focused_frame = matches!(kind, Some("frame" | "sealed"));
        let focus_sealed = kind == Some("sealed");
        let focus_in_page = matches!(kind, Some("field" | "password"));
        let frames = value
            .get("frames")
            .and_then(Value::as_u64)
            .map_or(0, |frames| usize::try_from(frames).unwrap_or(usize::MAX));
        Some(Self {
            shown: strings("shown")?,
            masked: strings("masked")?,
            focused_password,
            focused_frame,
            focus_sealed,
            focus_in_page,
            frames,
        })
    }

    fn shows(&self, value: &str) -> bool {
        self.shown.iter().any(|text| text.contains(value))
    }

    /// Whether `value` is whole in a password field and shown nowhere.
    fn confines(&self, value: &str) -> bool {
        self.masked.iter().any(|text| text.contains(value)) && !self.shows(value)
    }

    /// Whether `value`, just typed, went into the focused password field whole and is shown
    /// nowhere: another password field holding it says nothing of where this typing went.
    fn received(&self, value: &str) -> bool {
        self.focused_password
            .as_ref()
            .is_some_and(|text| text.contains(value))
            && !self.shows(value)
    }
}

/// Navigations to another document that Chrome announced and has not finished (SPEC-483 E9):
/// a secret typed meanwhile would land in the page that arrives.
#[derive(Default)]
struct Leaving {
    /// By session and frame: where the frame is going, and since when. At most
    /// [`LEAVING_MAX`]; one older than [`LEAVING_STANDS`] no longer counts.
    going: HashMap<(String, String), (String, Instant)>,
    /// Announcements were lost (events skipped, or more frames on their way than are kept):
    /// for [`LEAVING_STANDS`] from then, where a frame is going is not known.
    lost: Option<Instant>,
}

/// Where the frames of a page are going ([`Leaving::going`]).
#[derive(Debug, PartialEq, Eq)]
enum Going {
    /// To these addresses (none: nowhere).
    To(Vec<String>),
    /// Not known: announcements were lost.
    Unknown,
}

impl Leaving {
    /// Chrome announced that `frame` of `session` set out for `url`.
    fn announce(&mut self, session: &str, frame: &str, url: &str, now: Instant) {
        let key = (session.to_owned(), frame.to_owned());
        if !self.going.contains_key(&key) && self.going.len() >= LEAVING_MAX {
            self.going
                .retain(|_, (_, since)| now.duration_since(*since) < LEAVING_STANDS);
        }
        if !self.going.contains_key(&key) && self.going.len() >= LEAVING_MAX {
            self.lose(now);
            return;
        }
        // What is asked of the address is its site: its beginning says it.
        let mut end = url.len().min(LEAVING_ADDRESS_MAX);
        while !url.is_char_boundary(end) {
            end -= 1;
        }
        self.going.insert(key, (url[..end].to_owned(), now));
    }

    /// `frame` of `session` arrived, stopped loading or is gone.
    fn settled(&mut self, session: &str, frame: &str) {
        self.going.remove(&(session.to_owned(), frame.to_owned()));
    }

    fn session_ended(&mut self, session: &str) {
        self.going.retain(|(of, _), _| of != session);
    }

    /// Announcements were lost: nothing kept says where a frame is going now.
    fn lose(&mut self, now: Instant) {
        self.going.clear();
        self.lost = Some(now);
    }

    /// Where `frames` of `session` are going.
    fn going(&self, session: &str, frames: &[String], now: Instant) -> Going {
        if self
            .lost
            .is_some_and(|at| now.duration_since(at) < LEAVING_STANDS)
        {
            return Going::Unknown;
        }
        Going::To(
            frames
                .iter()
                .filter_map(|frame| self.going.get(&(session.to_owned(), frame.clone())))
                .filter(|(_, since)| now.duration_since(*since) < LEAVING_STANDS)
                .map(|(url, _)| url.clone())
                .collect(),
        )
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
    /// Where frames are going (SPEC-483 E9).
    leaving: Leaving,
    /// The policy probe's own tabs (ADR-0318), by the ids Chrome gave them: never listed.
    probe_tabs: HashSet<TabId>,
    /// A policy probe asked Chrome for its tab and has no id for it yet: what is announced
    /// meanwhile of a page no page opened (the probe's own tab is one, however Chrome first
    /// names it) waits here until the id says whether it is the probe's.
    probing: bool,
    probe_held: Vec<CdpEvent>,
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
    /// Secrets typed into this sandbox's browsers (SPEC-450).
    secrets: SecretMemory,
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
    /// Whether the host's managed policy is in force in this browser, as it last answered
    /// (ADR-0318).
    policy: Mutex<PolicyCheck>,
    /// One probe at a time.
    policy_lock: tokio::sync::Mutex<()>,
}

/// A policy probe's wait for the id of its tab. Dropping it ends the wait: the id (when Chrome
/// gave one) marks the probe's tab, which is taken off the tab list should it be on it, and
/// what was announced meanwhile is listed, unless it is the probe's own or listed already by a
/// newer event.
struct ProbeWait<'a> {
    browser: &'a BrowserInstance,
    target: Option<TabId>,
}

impl Drop for ProbeWait<'_> {
    fn drop(&mut self) {
        // What a newer event listed while the others waited: a held announcement of it is
        // older than what is shown. What the replay itself lists takes its later ones too.
        let (held, listed) = {
            let mut state = lock(&self.browser.state);
            state.probing = false;
            if let Some(target) = &self.target {
                state.probe_tabs.insert(target.clone());
            }
            let listed: HashSet<TabId> = state.tabs.iter().map(|tab| tab.id.clone()).collect();
            (std::mem::take(&mut state.probe_held), listed)
        };
        if let Some(target) = &self.target {
            self.browser.forget_tab(target);
        }
        for event in held {
            let id = event
                .params
                .get("targetInfo")
                .and_then(page_entry)
                .map(|(id, _, _)| id);
            if !id.is_some_and(|id| listed.contains(&id)) {
                self.browser.on_event(&event);
            }
        }
    }
}

/// What the browser last answered about the host's managed policy, and when.
#[derive(Debug, Default, Clone, Copy)]
struct PolicyCheck {
    in_force: bool,
    asked: Option<Instant>,
}

/// How long a navigation Chrome announced and never finished keeps a secret from being typed
/// into its frame.
const LEAVING_STANDS: Duration = Duration::from_secs(30);
/// Frames on their way that are kept at once, in all tabs of a browser; past it none is known
/// for [`LEAVING_STANDS`].
const LEAVING_MAX: usize = 256;
/// Bytes of an announced address that are kept.
const LEAVING_ADDRESS_MAX: usize = 2048;
/// The site named when an address has no host (`about:srcdoc`, `data:`).
const UNKNOWN_SITE: &str = "an unknown site";

/// How long a "not in force" answer stands before a refused input asks again (the policy
/// appears while the browser runs when an administrator runs host setup: Chrome reloads it).
const POLICY_RECHECK: Duration = Duration::from_secs(5);
/// How long a "in force" answer stands before the next press asks again: the files can be
/// taken away under a running browser.
const POLICY_CONFIRM: Duration = Duration::from_secs(300);
/// The longest the probe's navigation is waited for: a viewer's connection waits with it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Whether a page at an address may get a secret typed into it
/// ([`BrowserInstance::tool_type_secret`]).
pub type PageRule<'r> = dyn for<'a> Fn(&'a str) -> bool + Send + Sync + 'r;

/// A document as an error names it: its host, or its address when it has none.
fn shown_as(document: &Document) -> String {
    document
        .host
        .clone()
        .unwrap_or_else(|| document.address.clone())
}

/// Whether an address is a page of Chrome itself: a target created at one is no tab for
/// viewers and tools (Chrome 157 reports its address bar's popup,
/// `chrome://omnibox-popup.top-chrome/`, as a page). Nobody navigates to such an address
/// through Yulius either ([`normalize_url`]).
fn chromes_own(url: &str) -> bool {
    url.starts_with("chrome://") || url.starts_with("chrome-untrusted://")
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
        Self::attach_on(sandbox, cdp, config, None, SecretMemory::default()).await
    }

    /// Attaches over a connected client to a browser running on `display` (the instance owns
    /// the display and stops it after the browser, or at once when attaching fails), with what
    /// the sandbox's earlier browsers remember about typed secrets.
    pub(crate) async fn attach_on(
        sandbox: SandboxId,
        cdp: CdpClient,
        config: InstanceConfig,
        display: Option<VirtualDisplay>,
        secrets: SecretMemory,
    ) -> Result<Arc<Self>, BrowserError> {
        secrets.restart();
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
                leaving: Leaving::default(),
                probe_tabs: HashSet::new(),
                probing: false,
                probe_held: Vec::new(),
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
            secrets,
            new_tab_url: Mutex::new(None),
            deny: Mutex::new(UrlDenyList::default()),
            policy: Mutex::new(PolicyCheck::default()),
            policy_lock: tokio::sync::Mutex::new(()),
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
            // The probe's own tabs and pages of Chrome's that are no tabs yet stay unlisted.
            let pages: Vec<_> = pages
                .into_iter()
                .filter(|(id, _, url)| {
                    !state.probe_tabs.contains(id) && (state.has(id) || !chromes_own(url))
                })
                .collect();
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
                if let Some((id, _, url)) = info.and_then(page_entry) {
                    let opened_by_a_page = info
                        .and_then(|info| info.get("openerId"))
                        .and_then(Value::as_str)
                        .is_some_and(|opener| !opener.is_empty());
                    let mut state = lock(&self.state);
                    // The policy probe's own tab, known by the id Chrome gave it (never by its
                    // address, which a page can copy), is nobody's tab.
                    if state.probe_tabs.contains(&id) {
                        return;
                    }
                    let known = state.has(&id);
                    // The probe's tab may be announced before Chrome answers with its id, under
                    // whatever address: a new page no page opened waits for the id.
                    if !known && state.probing && !opened_by_a_page {
                        state.probe_held.push(event.clone());
                        return;
                    }
                    // A page of Chrome's own never becomes a tab; a tab that went to one stays
                    // a tab (Chrome still shows it).
                    if !known && chromes_own(&url) {
                        return;
                    }
                }
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
                    {
                        let mut state = lock(&self.state);
                        state.probe_tabs.remove(&tab);
                        // A page made and closed while a probe waits is no tab afterwards.
                        state.probe_held.retain(|held| {
                            held.params
                                .get("targetInfo")
                                .and_then(page_entry)
                                .is_none_or(|(id, _, _)| id != tab)
                        });
                    }
                    self.forget_tab(&tab);
                }
            }
            "Fetch.requestPaused" if event.session_id.is_none() => self.on_request_paused(event),
            // A frame set out for another document (a clicked link, a form, a script): until
            // Chrome says it arrived or gave up, a secret's typing may land in either.
            "Page.frameStartedNavigating" | "Page.frameRequestedNavigation" => {
                let same_document = event
                    .params
                    .get("navigationType")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.ends_with("ameDocument"));
                let elsewhere = event
                    .params
                    .get("disposition")
                    .and_then(Value::as_str)
                    .is_some_and(|tab| tab != "currentTab");
                if let (Some(session), Some(frame), Some(url), false) = (
                    event.session_id.as_deref(),
                    event.params.get("frameId").and_then(Value::as_str),
                    event.params.get("url").and_then(Value::as_str),
                    same_document || elsewhere,
                ) {
                    lock(&self.state)
                        .leaving
                        .announce(session, frame, url, Instant::now());
                }
            }
            "Page.frameNavigated" | "Page.frameStoppedLoading" | "Page.frameDetached" => {
                let frame = event
                    .params
                    .pointer("/frame/id")
                    .or_else(|| event.params.get("frameId"))
                    .and_then(Value::as_str);
                if let (Some(session), Some(frame)) = (event.session_id.as_deref(), frame) {
                    lock(&self.state).leaving.settled(session, frame);
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(session) = event.params.get("sessionId").and_then(Value::as_str) {
                    {
                        let mut state = lock(&self.state);
                        state.sessions.retain(|_, s| s != session);
                        state.screencasts.retain(|_, cast| cast.session != session);
                        state.leaving.session_ended(session);
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
        // Never under a secret being typed, or between a screenshot's look and its capture:
        // those hold this lock from their look at the page to their end (SPEC-483 E3).
        let _input = self.input_lock.lock().await;
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
        // As every navigation of Yulius's: never under a secret being typed.
        let _input = self.input_lock.lock().await;
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
        // As every navigation of Yulius's: never under a secret being typed.
        let _input = self.input_lock.lock().await;
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

    /// Whether a viewer's clicks and keys may reach this browser: always without a virtual
    /// display (a headless browser shows no window of its own), else only while the host's
    /// managed policy is in force (ADR-0318). A "no" is asked again after `POLICY_RECHECK`, a
    /// "yes" after `POLICY_CONFIRM` (Chrome reloads its policy files while it runs).
    pub async fn viewer_input_allowed(&self) -> bool {
        if self.display.is_none() {
            return true;
        }
        let fresh = |check: PolicyCheck| {
            let stands = if check.in_force {
                POLICY_CONFIRM
            } else {
                POLICY_RECHECK
            };
            check.asked.is_some_and(|at| at.elapsed() < stands)
        };
        let known = *lock(&self.policy);
        if fresh(known) {
            return known.in_force;
        }
        let _one = self.policy_lock.lock().await;
        let known = *lock(&self.policy);
        if fresh(known) {
            return known.in_force;
        }
        let in_force = match self.probe_policy().await {
            Ok(in_force) => in_force,
            Err(error) => {
                warn!(sandbox = %self.sandbox, %error, "the browser's policy could not be asked");
                false
            }
        };
        if !in_force {
            warn!(sandbox = %self.sandbox, "the host's browser policy is not in force: viewers' clicks and keys are refused (run `yulius host setup`)");
        }
        *lock(&self.policy) = PolicyCheck {
            in_force,
            asked: Some(Instant::now()),
        };
        in_force
    }

    /// Asks the browser whether it loaded the host's managed policy: a hidden tab is sent to
    /// the address the policy blocks, and the navigation fails as blocked, or otherwise.
    async fn probe_policy(&self) -> Result<bool, BrowserError> {
        lock(&self.state).probing = true;
        // Ends the wait for the tab's id however this future ends, a drop included (it runs in
        // a viewer's connection).
        let mut wait = ProbeWait {
            browser: self,
            target: None,
        };
        // The call lives in its own task, which closes the tab Chrome made unless this probe
        // takes its answer: after the probe gave up on it, or was dropped (it runs in a
        // viewer's connection), nobody else would.
        let (answer, answered) = tokio::sync::oneshot::channel();
        let cdp = self.cdp.clone();
        tokio::spawn(async move {
            let created = cdp
                .call(
                    "Target.createTarget",
                    json!({"url": crate::policy::PROBE_BLANK, "background": true}),
                )
                .await;
            if let Err(Ok(unclaimed)) = answer.send(created)
                && let Some(target) = unclaimed.get("targetId").and_then(Value::as_str)
            {
                let _ = cdp
                    .call("Target.closeTarget", json!({"targetId": target}))
                    .await;
            }
        });
        let created = match tokio::time::timeout(PROBE_TIMEOUT, answered).await {
            Ok(Ok(created)) => created.map_err(BrowserError::from),
            Ok(Err(_)) => Err(BrowserError::NavigationFailed(
                "the policy probe's tab could not be asked for".into(),
            )),
            Err(_) => Err(BrowserError::NavigationFailed(
                "the policy probe got no tab in time".into(),
            )),
        }
        .and_then(|created| {
            created
                .get("targetId")
                .and_then(Value::as_str)
                .and_then(|id| TabId::parse(id).ok())
                .ok_or_else(|| malformed("Target.createTarget", "missing targetId"))
        });
        wait.target = created.as_ref().ok().cloned();
        drop(wait);
        let target = created?;
        let asked = async {
            let attached = self
                .cdp
                .call(
                    "Target.attachToTarget",
                    json!({"targetId": target.as_str(), "flatten": true}),
                )
                .await?;
            let session = attached
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("Target.attachToTarget", "missing sessionId"))?;
            let answer = self
                .cdp
                .call_session(
                    session,
                    "Page.navigate",
                    json!({"url": crate::policy::probe_url()}),
                )
                .await?;
            Ok::<_, BrowserError>(
                answer
                    .get("errorText")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains(crate::policy::BLOCKED)),
            )
        };
        // No answer in time is a "no": the tab is closed either way.
        let asked = tokio::time::timeout(PROBE_TIMEOUT, asked)
            .await
            .unwrap_or(Ok(false));
        if let Err(error) = self
            .cdp
            .call("Target.closeTarget", json!({"targetId": target.as_str()}))
            .await
        {
            debug!(%error, "the policy probe's tab could not be closed");
        }
        asked
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
            // A press is real X input wherever the pointer is, Chrome's own windows included
            // (ADR-0318); moving and scrolling open none, and a release always passes, as a
            // key's does.
            if input.event == MouseEventKind::Down && !self.viewer_input_allowed().await {
                return Err(BrowserError::InputLocked);
            }
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
            // A release always passes: a key held when the answer changed must not stay down.
            if input.event != KeyEventKind::Up && !self.viewer_input_allowed().await {
                return Err(BrowserError::InputLocked);
            }
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
        self.type_text(params, None).await
    }

    /// `browser_fill_secret`'s typing of the secret's value: as [`Self::tool_type`], only into
    /// what `allowed` says the secret may be typed into (the tab's page as read with the input
    /// lock held, the frames it holds when the focus is not in a field of the page, and where
    /// its frames are going), and the browser remembers the text and the document it went into,
    /// so that a screenshot that may show it is refused (SPEC-450).
    ///
    /// # Errors
    /// As [`Self::tool_type`]; [`BrowserError::SecretPageRefused`],
    /// [`BrowserError::SecretFrameRefused`] and [`BrowserError::SecretPageLeaving`] when
    /// `allowed` refuses, and [`BrowserError::InvalidInput`] when the tab's page cannot be told
    /// or read or the browser remembers too many secrets or documents already (nothing is
    /// typed in any of these); [`BrowserError::SecretAstray`] and
    /// [`BrowserError::SecretFrameAstray`] when the page or the focus moved to where it may not
    /// go while it was typed.
    pub async fn tool_type_secret(
        &self,
        params: &TypeParams,
        allowed: &PageRule<'_>,
    ) -> Result<TypeResult, BrowserError> {
        self.type_text(params, Some((allowed, true))).await
    }

    /// `browser_fill_secret`'s typing of what belongs to a secret and is not hidden (its user
    /// name, a one-time code): under the page rule of [`Self::tool_type_secret`], and not
    /// remembered for screenshots.
    ///
    /// # Errors
    /// As [`Self::tool_type_secret`], without the bound on remembered secrets.
    pub async fn tool_type_beside_secret(
        &self,
        params: &TypeParams,
        allowed: &PageRule<'_>,
    ) -> Result<TypeResult, BrowserError> {
        self.type_text(params, Some((allowed, false))).await
    }

    /// Types `params`. With a `rule` (the page rule and whether the text is remembered as a
    /// typed secret), only where the rule allows.
    async fn type_text(
        &self,
        params: &TypeParams,
        rule: Option<(&PageRule<'_>, bool)>,
    ) -> Result<TypeResult, BrowserError> {
        self.touch();
        params.validate()?;
        let session = self.session(&params.tab_id).await?;
        let focus = || async {
            if let Some(selector) = &params.selector {
                let value = self
                    .call_on_document(&session, FOCUS_FUNCTION, selector)
                    .await?;
                if value.get("found").and_then(Value::as_bool) != Some(true) {
                    return Err(BrowserError::ElementNotFound);
                }
            }
            Ok(())
        };
        let Some((allowed, remember)) = rule else {
            focus().await?;
            // Screenshots hold the same lock from their look at the page to the capture.
            let _input = self.input_lock.lock().await;
            let watched = self.watched_values(&session).await;
            return self.type_lines(&session, params, watched.as_ref()).await;
        };
        let unknown = || UNKNOWN_SITE.to_owned();
        // A secret's field is focused with the lock held too: nothing of Yulius's moves the
        // focus between that and the keys.
        let _input = self.input_lock.lock().await;
        let Some(before) = self.document(&session).await else {
            return Err(BrowserError::InvalidInput(
                "the tab's page cannot be identified, so the secret was not typed; try again"
                    .into(),
            ));
        };
        // The page as it is now, with the lock held: what the caller saw of the tab may be
        // from before a navigation, and Yulius's own navigations wait for this lock
        // (SPEC-483 E3).
        if !allowed(&before.address) {
            return Err(BrowserError::SecretPageRefused(shown_as(&before)));
        }
        focus().await?;
        // The keys go to the element with the focus, which need not be in this page: where
        // the focus is, is read after the field was focused (SPEC-483 E8).
        let Some(look) = self.read_page(&session, &before.frame).await else {
            return Err(BrowserError::InvalidInput(
                "the tab's page cannot be read right now, so the secret was not typed; try again"
                    .into(),
            ));
        };
        // The document once more: its frames as they are now, and whether it still is the one
        // that was asked (a navigation may have arrived while the field was focused).
        let Some(mut framing) = self.document(&session).await else {
            return Err(BrowserError::InvalidInput(
                "the tab's frames cannot be identified, so the secret was not typed; try again"
                    .into(),
            ));
        };
        if framing.loader != before.loader || framing.frame != before.frame {
            return Err(if allowed(&framing.address) {
                BrowserError::InvalidInput(
                    "the tab's page changed while its field was focused, so the secret was not typed; try again"
                        .into(),
                )
            } else {
                BrowserError::SecretPageRefused(shown_as(&framing))
            });
        }
        if look.focus_sealed {
            return Err(BrowserError::SecretFrameRefused(unknown()));
        }
        // Only a text field of a document the probe read is the page's own. Anything else can
        // be a document of another site (a frame, an object, an embed, a frame inside a closed
        // shadow root), and which one the page cannot say: every frame of another site the
        // page holds must then be one the secret may go to as well.
        if !look.focus_in_page {
            let targets = self.cdp.call("Target.getTargets", json!({})).await?;
            framing.add_frame_targets(&params.tab_id, &targets);
            if let Some(other) = framing
                .framed_addresses
                .iter()
                .find(|address| !allowed(address))
            {
                return Err(BrowserError::SecretFrameRefused(
                    host_of(other).unwrap_or_else(unknown),
                ));
            }
        }
        // As other typing, what is not remembered may edit a secret typed before.
        let watched = if remember {
            None
        } else {
            self.watched_values(&session).await
        };
        // A navigation under way would put its page under the keys: where the page's frames
        // are going is asked too, last, as the announcement can come at any time (SPEC-483
        // E9). One Chrome never finished is forgotten after a while.
        let going = lock(&self.state)
            .leaving
            .going(&session, &framing.frames, Instant::now());
        match going {
            Going::Unknown => {
                return Err(BrowserError::InvalidInput(
                    "the browser lost track of where its tabs are going, so the secret was not typed; try again in half a minute"
                        .into(),
                ));
            }
            Going::To(addresses) => {
                if let Some(url) = addresses.iter().find(|url| !allowed(url)) {
                    return Err(BrowserError::SecretPageLeaving(
                        host_of(url).unwrap_or_else(unknown),
                    ));
                }
            }
        }
        if remember {
            let mut typed = self.secrets.lock();
            if !typed.room_for(&params.text) {
                return Err(BrowserError::InvalidInput(format!(
                    "this browser holds {TYPED_SECRETS_MAX} secrets typed already; it forgets them when Yulius restarts"
                )));
            }
            typed.record(Some(&before), &params.text);
        }
        let typed = self.type_lines(&session, params, watched.as_ref()).await;
        let mut after = self.document(&session).await;
        let page = match &after {
            Some(document) => self.read_page(&session, &document.frame).await,
            None => None,
        };
        // Typed into a frame the probe cannot read: the frames' hosts are where it went.
        let into_frame = page.as_ref().is_some_and(|page| page.focused_frame);
        let mut frames_known = true;
        if into_frame && let Some(document) = after.as_mut() {
            match self.cdp.call("Target.getTargets", json!({})).await {
                Ok(targets) => document.add_frame_targets(&params.tab_id, &targets),
                Err(_) => frames_known = false,
            }
        }
        // A navigation meanwhile (the page's own) may have put it into the next document.
        let moved = after.as_ref().map(|d| d.loader.as_str()) != Some(before.loader.as_str());
        // The page it moved to is asked too. One that cannot be identified is not called a
        // page it went astray to: every screenshot is refused from here on instead (below).
        let astray = after
            .as_ref()
            .filter(|document| moved && !allowed(&document.address))
            .map(|document| BrowserError::SecretAstray(shown_as(document)));
        // So is a frame that has the focus now: the look before the keys found it in a field
        // of the page, or every frame allowed, so one of another site took the focus (its
        // script, as the page's field got it) or came since.
        let astray = astray.or_else(|| {
            let document = after.as_ref().filter(|_| into_frame)?;
            let sealed = page.as_ref().is_some_and(|page| page.focus_sealed);
            let frame = document
                .framed_addresses
                .iter()
                .find(|address| !allowed(address));
            match (frame, sealed) {
                (Some(address), _) => Some(host_of(address).unwrap_or_else(unknown)),
                (None, true) => Some(unknown()),
                (None, false) => None,
            }
            .map(BrowserError::SecretFrameAstray)
        });
        if remember {
            let mut secrets = self.secrets.lock();
            if moved {
                secrets.record(after.as_ref(), &params.text);
            }
            // Only a password field keeps it out of the pixels: anywhere else exposes the
            // site, the one it was typed into and the one shown since.
            if !page
                .as_ref()
                .is_some_and(|page| page.received(&params.text))
            {
                let after_host = after.as_ref().and_then(|d| d.host.as_ref());
                let framed: &[String] = match (&after, into_frame) {
                    (Some(document), true) => &document.framed,
                    _ => &[],
                };
                let mut exposed = false;
                for host in before.host.iter().chain(after_host).chain(framed) {
                    secrets.expose(Some(host));
                    exposed = true;
                }
                if page.is_none() || !exposed || !frames_known {
                    secrets.expose(None);
                }
            }
        }
        // Where it went is said whatever else the typing ran into.
        match astray {
            Some(astray) => Err(astray),
            None => typed,
        }
    }

    /// Before other typing (`browser_type`, a user name): the secrets typed into this document
    /// that are whole in a password field now, which the typing may edit (a character into the
    /// middle of one); `None` when there are none. A document that cannot be told, with secrets
    /// typed in this browser, refuses every screenshot; a page that cannot be read, or that shows
    /// one of them (a password revealed), exposes its host, since an edit would go unseen.
    async fn watched_values(&self, session: &str) -> Option<(Document, Vec<Zeroizing<String>>)> {
        if self.secrets.lock().documents.is_empty() {
            return None;
        }
        let Some(document) = self.document(session).await else {
            self.secrets.lock().expose(None);
            return None;
        };
        let values = self.secrets.lock().typed_into(&document);
        if values.is_empty() {
            return None;
        }
        let Some(page) = self.read_page(session, &document.frame).await else {
            // A navigation committing meanwhile (its context gone): the typing goes into the
            // next document, which holds none of them, unless it is one a value went into (a
            // page restored from the back/forward cache keeps its loader id).
            match self.document(session).await {
                Some(now) if now.loader != document.loader => {
                    let mut secrets = self.secrets.lock();
                    if !secrets.typed_into(&now).is_empty() {
                        secrets.expose(now.host.as_deref());
                    }
                }
                Some(_) => self.secrets.lock().expose(document.host.as_deref()),
                None => self.secrets.lock().expose(None),
            }
            return None;
        };
        if values.iter().any(|value| page.shows(value)) {
            self.secrets.lock().expose(document.host.as_deref());
            return None;
        }
        let values: Vec<_> = values
            .into_iter()
            .filter(|value| page.confines(value))
            .collect();
        (!values.is_empty()).then_some((document, values))
    }

    /// After a line typed into `watched`'s document: a value no longer whole in a password field
    /// was edited, so its host is exposed.
    async fn check_edits(&self, session: &str, watched: &(Document, Vec<Zeroizing<String>>)) {
        let (document, values) = watched;
        let page = self.read_page(session, &document.frame).await;
        if !page
            .as_ref()
            .is_some_and(|page| values.iter().all(|value| page.confines(value)))
        {
            self.secrets.lock().expose(document.host.as_deref());
        }
    }

    /// The document the tab's main frame shows; `None` when unknown.
    async fn document(&self, session: &str) -> Option<Document> {
        let tree = self
            .cdp
            .call_session(session, "Page.getFrameTree", json!({}))
            .await
            .ok()?;
        Document::parse(&tree)
    }

    /// What the page in `frame` can show ([`SECRET_PROBE_EXPRESSION`], in an isolated world);
    /// `None` when it cannot be read.
    async fn read_page(&self, session: &str, frame: &str) -> Option<PageText> {
        let read = async {
            let world = self
                .cdp
                .call_session(
                    session,
                    "Page.createIsolatedWorld",
                    json!({"frameId": frame, "worldName": SECRET_PROBE_WORLD}),
                )
                .await?;
            let context = world
                .get("executionContextId")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    malformed("Page.createIsolatedWorld", "missing executionContextId")
                })?;
            let result = self
                .cdp
                .call_session(
                    session,
                    "Runtime.evaluate",
                    json!({"expression": SECRET_PROBE_EXPRESSION, "contextId": context,
                           "returnByValue": true}),
                )
                .await?;
            if result.get("exceptionDetails").is_some() {
                return Err(malformed("Runtime.evaluate", "the secret probe threw"));
            }
            Ok::<_, BrowserError>(result.pointer("/result/value").and_then(PageText::parse))
        };
        match read.await {
            Ok(page) => page,
            Err(error) => {
                debug!(%error, "reading the page for typed secrets failed");
                None
            }
        }
    }

    /// Refuses a screenshot while the tab's page may show a secret (SPEC-450,
    /// [`TypedSecrets::judge`]); the caller holds `input_lock` until it has captured. `Ok(None)`:
    /// nothing to check; else the loader id of the document checked.
    async fn check_page(
        &self,
        tab: &TabId,
        session: &str,
        vault: Option<SecretCheck<'_>>,
    ) -> Result<Option<String>, BrowserError> {
        if vault.is_none() && !self.secrets.lock().any() {
            return Ok(None);
        }
        let mut document = self.document(session).await;
        // Out-of-process frames are targets of their own; unknown, the document is unknown.
        if let Some(found) = document.as_mut() {
            match self.cdp.call("Target.getTargets", json!({})).await {
                Ok(targets) => found.add_frame_targets(tab, &targets),
                Err(error) => {
                    debug!(%error, "listing frame targets failed");
                    document = None;
                }
            }
        }
        let page = match &document {
            Some(document) => self.read_page(session, &document.frame).await,
            None => None,
        };
        if let (Some(document), Some(page)) = (document.as_mut(), page.as_ref()) {
            document.add_unread_frames(page);
        }
        // Searched outside the memory's lock: a page holds up to 4 000 000 characters.
        let shows_vault = page
            .as_ref()
            .zip(vault)
            .is_some_and(|(page, vault)| page.shown.iter().any(|text| vault(text)));
        let snapshot = self.secrets.lock().clone();
        let (refuse, expose) = snapshot.judge(document.as_ref(), page.as_ref(), shows_vault);
        if expose {
            self.secrets
                .lock()
                .expose(document.as_ref().and_then(|d| d.host.as_deref()));
        }
        if refuse {
            return Err(BrowserError::SecretOnPage);
        }
        Ok(document.map(|document| document.loader))
    }

    /// After a capture of a page [`Self::check_page`] passed: the same document must still pass
    /// (a navigation or a page script may have changed it meanwhile), else the image is dropped.
    async fn check_again(
        &self,
        tab: &TabId,
        session: &str,
        vault: Option<SecretCheck<'_>>,
        checked: Option<String>,
    ) -> Result<(), BrowserError> {
        if let Some(loader) = checked
            && self.check_page(tab, session, vault).await? != Some(loader)
        {
            return Err(BrowserError::SecretOnPage);
        }
        Ok(())
    }

    /// Inserts `params.text`, pressing Enter for each `\n`; with `watched`, checks for edits
    /// after each line, before its Enter may submit the form ([`Self::check_edits`]).
    async fn type_lines(
        &self,
        session: &str,
        params: &TypeParams,
        watched: Option<&(Document, Vec<Zeroizing<String>>)>,
    ) -> Result<TypeResult, BrowserError> {
        let mut chars = 0;
        for (index, line) in params.text.split('\n').enumerate() {
            if index > 0 {
                self.press_enter(session).await?;
                chars += 1;
            }
            let line = line.strip_suffix('\r').unwrap_or(line);
            if !line.is_empty() {
                let inserted = self
                    .cdp
                    .call_session(session, "Input.insertText", json!({"text": line}))
                    .await;
                if let Some(watched) = watched {
                    self.check_edits(session, watched).await;
                }
                inserted?;
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
    /// [`BrowserError::SecretOnPage`] while the page may show a secret typed with
    /// [`Self::tool_type_secret`]; [`BrowserError::ResultTooLarge`], tab or CDP failures.
    pub async fn tool_screenshot(
        &self,
        params: &ScreenshotParams,
    ) -> Result<ScreenshotResult, BrowserError> {
        self.touch();
        let session = self.session(&params.tab_id).await?;
        let _input = self.input_lock.lock().await;
        let checked = self.check_page(&params.tab_id, &session, None).await?;
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
        let data = data.to_owned();
        self.check_again(&params.tab_id, &session, None, checked)
            .await?;
        Ok(ScreenshotResult {
            mime_type: "image/png".to_owned(),
            data_base64: data,
        })
    }

    /// `browser_screenshot` within a byte budget (e.g. an MCP result cap): a PNG of the
    /// viewport when it fits, otherwise JPEGs of decreasing quality and scale
    /// ([`BOUNDED_SCREENSHOT_STEPS`]) until one fits. The result says which step was used so
    /// callers can map coordinates of a downscaled image back to the viewport.
    ///
    /// # Errors
    /// [`BrowserError::SecretOnPage`] while the page may show a secret typed with
    /// [`Self::tool_type_secret`]; [`BrowserError::ResultTooLarge`] when even the smallest step
    /// exceeds the budget; tab or CDP failures.
    pub async fn tool_screenshot_within(
        &self,
        params: &ScreenshotParams,
        max_base64_bytes: usize,
    ) -> Result<BoundedScreenshot, BrowserError> {
        self.tool_screenshot_guarded(params, max_base64_bytes, None)
            .await
    }

    /// [`Self::tool_screenshot_within`], refused also while the page shows a text `vault` says
    /// holds a secret Yulius hides (every agent secret of the sandbox, typed or not, SPEC-450).
    ///
    /// # Errors
    /// As [`Self::tool_screenshot_within`].
    pub async fn tool_screenshot_guarded(
        &self,
        params: &ScreenshotParams,
        max_base64_bytes: usize,
        vault: Option<SecretCheck<'_>>,
    ) -> Result<BoundedScreenshot, BrowserError> {
        self.touch();
        let session = self.session(&params.tab_id).await?;
        // Held to the last capture: no agent input reveals a secret after the look at the page.
        let _input = self.input_lock.lock().await;
        let checked = self.check_page(&params.tab_id, &session, vault).await?;
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
                let data = data.to_owned();
                self.check_again(&params.tab_id, &session, vault, checked)
                    .await?;
                return Ok(BoundedScreenshot {
                    result: ScreenshotResult {
                        mime_type: mime_type.to_owned(),
                        data_base64: data,
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
                        // An announced navigation may be among them (SPEC-483 E9).
                        lock(&instance.state).leaving.lose(Instant::now());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    /// SPEC-483 E9: what Chrome announced stands until the frame settled, its session ended
    /// or [`LEAVING_STANDS`] passed, and only for the frames asked about.
    #[test]
    fn an_announced_navigation_stands_until_it_settled_or_is_old() {
        let now = Instant::now();
        let mut leaving = Leaving::default();
        leaving.announce("s1", "main", "https://evil.example/a", now);
        leaving.announce("s1", "child", "https://bank.example/b", now);
        leaving.announce("s2", "main", "https://other.example/c", now);
        let both = Going::To(vec![
            "https://evil.example/a".to_owned(),
            "https://bank.example/b".to_owned(),
        ]);
        assert_eq!(leaving.going("s1", &frames(&["main", "child"]), now), both);
        assert_eq!(
            leaving.going("s1", &frames(&["child"]), now),
            Going::To(vec!["https://bank.example/b".to_owned()])
        );
        // Just before the time is up it stands; at it, it does not.
        let late = now + LEAVING_STANDS;
        assert_eq!(
            leaving.going(
                "s1",
                &frames(&["main", "child"]),
                late - Duration::from_millis(1)
            ),
            both
        );
        assert_eq!(
            leaving.going("s1", &frames(&["main", "child"]), late),
            Going::To(Vec::new())
        );
        leaving.settled("s1", "main");
        assert_eq!(
            leaving.going("s1", &frames(&["main"]), now),
            Going::To(Vec::new())
        );
        leaving.session_ended("s2");
        assert_eq!(
            leaving.going("s2", &frames(&["main"]), now),
            Going::To(Vec::new())
        );
        assert_eq!(leaving.going.len(), 1, "only s1's child is kept");
    }

    /// Bounded: past [`LEAVING_MAX`] frames on their way, old ones go first; when none is old,
    /// nothing is known for [`LEAVING_STANDS`] (a fill is refused, not typed blind). So when
    /// events were skipped. A long address is kept by its beginning.
    #[test]
    fn announcements_are_bounded_and_losing_them_is_not_knowing() {
        let start = Instant::now();
        let mut leaving = Leaving::default();
        for n in 0..LEAVING_MAX {
            leaving.announce("s1", &format!("f{n}"), "https://a.example/", start);
        }
        // An announcement for a frame already kept replaces it at the bound.
        leaving.announce("s1", "f0", "https://b.example/", start);
        assert_eq!(
            leaving.going("s1", &frames(&["f0"]), start),
            Going::To(vec!["https://b.example/".to_owned()])
        );
        // The old ones make room.
        let later = start + LEAVING_STANDS;
        leaving.announce("s1", "new", "https://c.example/", later);
        assert_eq!(leaving.going.len(), 1);
        assert_eq!(
            leaving.going("s1", &frames(&["new"]), later),
            Going::To(vec!["https://c.example/".to_owned()])
        );
        // None is old: one more than are kept loses them all.
        for n in 0..LEAVING_MAX {
            leaving.announce("s1", &format!("g{n}"), "https://a.example/", later);
        }
        assert!(leaving.going.is_empty());
        assert_eq!(
            leaving.going("s1", &frames(&["new"]), later),
            Going::Unknown
        );
        assert_eq!(
            leaving.going(
                "s9",
                &frames(&["x"]),
                later + LEAVING_STANDS - Duration::from_millis(1)
            ),
            Going::Unknown
        );
        assert_eq!(
            leaving.going("s1", &frames(&["new"]), later + LEAVING_STANDS),
            Going::To(Vec::new())
        );

        let mut leaving = Leaving::default();
        leaving.lose(start);
        assert_eq!(
            leaving.going("s1", &frames(&["main"]), start),
            Going::Unknown
        );

        let long = format!("https://evil.example/{}", "é".repeat(LEAVING_ADDRESS_MAX));
        let mut leaving = Leaving::default();
        leaving.announce("s1", "main", &long, start);
        let Going::To(kept) = leaving.going("s1", &frames(&["main"]), start) else {
            panic!("known");
        };
        assert!(kept[0].len() <= LEAVING_ADDRESS_MAX && long.starts_with(&kept[0]));
        assert!(kept[0].starts_with("https://evil.example/"));
    }
}
