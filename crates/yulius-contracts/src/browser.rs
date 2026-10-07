//! Browser viewer frames and tool parameters (SPEC-002 ADR-09, §7.4; SPEC-001 BRW-001..004).
//!
//! Three contracts live here:
//!
//! - the viewer WebSocket `WS /api/sandboxes/{sid}/browser` (docs/api.md "Browser, editor,
//!   shell"): JSON text messages [`ViewerClientMsg`] and [`ViewerServerMsg`], plus binary
//!   screencast frames built by [`encode_frame`] and parsed by [`decode_frame`];
//! - the parameter and result payloads of the Yulius MCP browser tools (`browser_tabs`,
//!   `browser_open`, …), with their JSON input schemas in [`tool_specs`];
//! - the shared bounds ([`SNAPSHOT_MAX_BYTES`], [`SCREENSHOT_MAX_BASE64_BYTES`], …) that the
//!   browser adapter enforces.
//!
//! Version: v1 (same as the HTTP API). Additive optional fields are compatible; renaming or
//! removing a field, message type or frame byte is a breaking change.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Binary frame kind byte for a browser screencast frame (terminal frames use 1 and 2).
pub const FRAME_KIND_BROWSER: u8 = 3;
/// Width of the space-padded ASCII tab id inside a binary frame.
pub const FRAME_TAB_ID_LEN: usize = 32;
/// Bytes before the JPEG payload: kind byte plus padded tab id.
pub const FRAME_HEADER_LEN: usize = 1 + FRAME_TAB_ID_LEN;

/// Maximum UTF-8 bytes of a `browser_snapshot` outline; longer outlines are truncated.
pub const SNAPSHOT_MAX_BYTES: usize = 64 * 1024;
/// Maximum length of the base64 text returned by `browser_screenshot`.
pub const SCREENSHOT_MAX_BASE64_BYTES: usize = 2 * 1024 * 1024;
/// Maximum bytes of a URL accepted from the viewer or a tool.
pub const URL_MAX_BYTES: usize = 8 * 1024;
/// Maximum bytes of the text accepted by `browser_type`.
pub const TYPE_TEXT_MAX_BYTES: usize = 64 * 1024;
/// Maximum bytes of a CSS selector accepted by `browser_click` and `browser_type`.
pub const SELECTOR_MAX_BYTES: usize = 4 * 1024;
/// Maximum bytes of a `browser_request_user_action` reason.
pub const ACTION_REASON_MAX_BYTES: usize = 1024;

/// MCP tool name: list tabs.
pub const TOOL_TABS: &str = "browser_tabs";
/// MCP tool name: open a new tab.
pub const TOOL_OPEN: &str = "browser_open";
/// MCP tool name: navigate a tab.
pub const TOOL_NAVIGATE: &str = "browser_navigate";
/// MCP tool name: click in a tab.
pub const TOOL_CLICK: &str = "browser_click";
/// MCP tool name: type text into a tab.
pub const TOOL_TYPE: &str = "browser_type";
/// MCP tool name: accessible text outline of a tab.
pub const TOOL_SNAPSHOT: &str = "browser_snapshot";
/// MCP tool name: screenshot of a tab (PNG, or JPEG when shrunk to fit a result bound).
pub const TOOL_SCREENSHOT: &str = "browser_screenshot";
/// MCP tool name: ask the user to act in a tab (e.g. log in).
pub const TOOL_REQUEST_USER_ACTION: &str = "browser_request_user_action";

/// Rejected tab identifier text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tab id: must be 1-32 printable ASCII characters without spaces")]
pub struct TabIdError;

/// A browser tab (Chrome DevTools Protocol page target id).
///
/// Valid ids are 1–32 printable ASCII characters (`!`..=`~`, no spaces), so every id fits the
/// fixed, space-padded field of a binary frame. Chromium target ids are 32 hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TabId(String);

impl TabId {
    /// Validates and wraps a tab id.
    ///
    /// # Errors
    /// Returns [`TabIdError`] when the text is empty, longer than 32 bytes, or contains a byte
    /// outside printable ASCII (space and control characters included).
    pub fn parse(text: &str) -> Result<Self, TabIdError> {
        let valid = !text.is_empty()
            && text.len() <= FRAME_TAB_ID_LEN
            && text.bytes().all(|b| b.is_ascii_graphic());
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(TabIdError)
        }
    }

    /// The tab id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TabId {
    type Error = TabIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TabId> for String {
    fn from(value: TabId) -> Self {
        value.0
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A pending request for the user to act in a tab (BRW-004), e.g. an AWS login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRequest {
    /// Why the agent needs the user, shown in the UI.
    pub reason: String,
    /// The requesting task, when the request came from a task's MCP token.
    pub task_id: Option<String>,
}

/// One tab as shown to viewers and returned by tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    /// Tab id.
    pub id: TabId,
    /// Page title (may be empty while loading).
    pub title: String,
    /// Current URL.
    pub url: String,
    /// Whether this is the tab the viewer shows (last opened or activated).
    pub active: bool,
    /// Pending user-action request, serialized as `null` when absent.
    pub action_requested: Option<ActionRequest>,
}

/// Mouse event kind sent by the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEventKind {
    /// Button pressed.
    Down,
    /// Button released.
    Up,
    /// Pointer moved.
    Move,
    /// Wheel scrolled by `dx`/`dy`.
    Wheel,
}

/// Mouse button (Chrome DevTools Protocol names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// No button (moves and wheel events).
    #[default]
    None,
    /// Primary button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary button.
    Right,
    /// Browser back button.
    Back,
    /// Browser forward button.
    Forward,
}

impl MouseButton {
    /// The Chrome DevTools Protocol `button` value.
    #[must_use]
    pub fn as_cdp(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
            Self::Back => "back",
            Self::Forward => "forward",
        }
    }
}

/// Keyboard event kind sent by the viewer.
///
/// Send `down` (with `text` for printable keys) and `up` for physical keys; use `char` only
/// for text that has no key press (IME composition, paste), otherwise the text is inserted
/// twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyEventKind {
    /// Key pressed.
    Down,
    /// Key released.
    Up,
    /// Text input without a key press.
    Char,
}

/// Modifier bit for Alt in [`ViewerClientMsg::Key::modifiers`] (CDP bit field).
pub const MODIFIER_ALT: u8 = 1;
/// Modifier bit for Control.
pub const MODIFIER_CTRL: u8 = 2;
/// Modifier bit for Meta/Command.
pub const MODIFIER_META: u8 = 4;
/// Modifier bit for Shift.
pub const MODIFIER_SHIFT: u8 = 8;

/// Viewer → server text message on the browser WebSocket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewerClientMsg {
    /// Open a new tab at an address (becomes active).
    Open {
        /// Address typed by the user; see the adapter's URL policy.
        url: String,
    },
    /// Navigate an existing tab.
    Navigate {
        /// Target tab.
        tab_id: TabId,
        /// Address typed by the user.
        url: String,
    },
    /// Make a tab the active (shown) tab.
    Activate {
        /// Target tab.
        tab_id: TabId,
    },
    /// Close a tab.
    Close {
        /// Target tab.
        tab_id: TabId,
    },
    /// Pointer input in CSS pixels of the tab viewport.
    Mouse {
        /// Target tab.
        tab_id: TabId,
        /// Event kind.
        event: MouseEventKind,
        /// Horizontal position.
        x: f64,
        /// Vertical position.
        y: f64,
        /// Button; absent or `null` means none.
        #[serde(default)]
        button: Option<MouseButton>,
        /// Horizontal wheel delta.
        #[serde(default)]
        dx: Option<f64>,
        /// Vertical wheel delta.
        #[serde(default)]
        dy: Option<f64>,
    },
    /// Keyboard input.
    Key {
        /// Target tab.
        tab_id: TabId,
        /// Event kind.
        event: KeyEventKind,
        /// DOM `KeyboardEvent.key` (e.g. `Enter`, `a`).
        #[serde(default)]
        key: Option<String>,
        /// DOM `KeyboardEvent.code` (e.g. `KeyA`).
        #[serde(default)]
        code: Option<String>,
        /// Text produced by the key, if any.
        #[serde(default)]
        text: Option<String>,
        /// Bit field of `MODIFIER_*` constants.
        #[serde(default)]
        modifiers: Option<u8>,
    },
    /// The viewer's panel size in CSS pixels; applies to all tabs of the sandbox browser.
    Viewport {
        /// Width.
        width: u32,
        /// Height.
        height: u32,
    },
}

/// Error codes sent to viewers in [`ViewerServerMsg::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerErrorCode {
    /// Malformed or out-of-bounds message.
    Invalid,
    /// The URL policy refused the address.
    UrlRefused,
    /// Unknown tab.
    NotFound,
    /// Tab or resource limit reached.
    Capacity,
    /// The browser failed or did not answer.
    Upstream,
    /// Unexpected server failure.
    Internal,
}

/// Server → viewer text message on the browser WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewerServerMsg {
    /// Full tab list; sent on connect and after every change.
    Tabs {
        /// Tabs in creation order.
        tabs: Vec<TabInfo>,
    },
    /// An agent asked the user to act in a tab (BRW-004).
    ActionRequested {
        /// Tab needing the user.
        tab_id: TabId,
        /// Reason shown to the user.
        reason: String,
        /// Requesting task, if any.
        task_id: Option<String>,
    },
    /// A request failed.
    Error {
        /// Machine-readable code.
        code: ViewerErrorCode,
        /// Human-readable text (never contains secrets).
        message: String,
    },
}

/// Malformed binary browser frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Shorter than the 33-byte header.
    #[error("browser frame shorter than its header")]
    Truncated,
    /// First byte is not [`FRAME_KIND_BROWSER`].
    #[error("unexpected frame kind {0}")]
    WrongKind(u8),
    /// The padded tab id field is not a valid [`TabId`].
    #[error("invalid tab id in browser frame")]
    InvalidTabId,
}

/// Builds a binary screencast frame: `3`, the tab id space-padded to 32 bytes, the JPEG bytes.
///
/// ```
/// use yulius_contracts::browser::{TabId, decode_frame, encode_frame};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let tab = TabId::parse("E3C8B2A1")?;
/// let frame = encode_frame(&tab, b"\xFF\xD8jpeg");
/// assert_eq!(frame.len(), 33 + 6);
/// let (decoded, jpeg) = decode_frame(&frame)?;
/// assert_eq!((decoded, jpeg), (tab, &b"\xFF\xD8jpeg"[..]));
/// # Ok(())
/// # }
/// ```
#[must_use]
pub fn encode_frame(tab_id: &TabId, jpeg: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + jpeg.len());
    frame.push(FRAME_KIND_BROWSER);
    frame.extend_from_slice(tab_id.as_str().as_bytes());
    frame.resize(FRAME_HEADER_LEN, b' ');
    frame.extend_from_slice(jpeg);
    frame
}

/// Parses a binary screencast frame into its tab id and JPEG bytes.
///
/// # Errors
/// [`FrameError`] when the frame is shorter than the header, has another kind byte, or its
/// tab id field (trailing spaces removed) is not a valid [`TabId`].
pub fn decode_frame(frame: &[u8]) -> Result<(TabId, &[u8]), FrameError> {
    let (header, jpeg) = frame
        .split_at_checked(FRAME_HEADER_LEN)
        .ok_or(FrameError::Truncated)?;
    let (&kind, id_field) = header.split_first().ok_or(FrameError::Truncated)?;
    if kind != FRAME_KIND_BROWSER {
        return Err(FrameError::WrongKind(kind));
    }
    let id_text = std::str::from_utf8(id_field).map_err(|_| FrameError::InvalidTabId)?;
    let tab = TabId::parse(id_text.trim_end_matches(' ')).map_err(|_| FrameError::InvalidTabId)?;
    Ok((tab, jpeg))
}

/// Invalid combination of tool parameters.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolParamError {
    /// `browser_click` needs either `selector` or both `x` and `y`, not both forms.
    #[error("give either selector or both x and y")]
    ClickTarget,
    /// A coordinate is not finite.
    #[error("coordinates must be finite numbers")]
    Coordinates,
    /// A text field exceeds its byte bound.
    #[error("{field} exceeds {max} bytes")]
    TooLong {
        /// Parameter name.
        field: &'static str,
        /// Bound in bytes.
        max: usize,
    },
    /// A required text field is empty.
    #[error("{0} must not be empty")]
    Empty(&'static str),
}

/// Parameters of `browser_tabs` (none).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabsParams {}

/// Result of `browser_tabs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabsResult {
    /// Tabs in creation order.
    pub tabs: Vec<TabInfo>,
}

/// Parameters of `browser_open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// Address to open (http, https or about:blank).
    pub url: String,
}

/// Result of `browser_open` and `browser_navigate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationResult {
    /// The tab after navigation started.
    pub tab: TabInfo,
    /// Whether the page's load event fired within the adapter's load timeout.
    pub loaded: bool,
}

/// Parameters of `browser_navigate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigateParams {
    /// Target tab.
    pub tab_id: TabId,
    /// Address (http, https or about:blank).
    pub url: String,
}

/// What `browser_click` clicks.
#[derive(Debug, Clone, PartialEq)]
pub enum ClickTarget {
    /// Centre of the first element matching a CSS selector (scrolled into view).
    Selector(String),
    /// A point in CSS pixels of the viewport.
    Point {
        /// Horizontal position.
        x: f64,
        /// Vertical position.
        y: f64,
    },
}

/// Parameters of `browser_click`: `selector`, or `x` and `y`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickParams {
    /// Target tab.
    pub tab_id: TabId,
    /// CSS selector of the element to click.
    #[serde(default)]
    pub selector: Option<String>,
    /// Horizontal position in CSS pixels.
    #[serde(default)]
    pub x: Option<f64>,
    /// Vertical position in CSS pixels.
    #[serde(default)]
    pub y: Option<f64>,
}

impl ClickParams {
    /// The validated click target.
    ///
    /// # Errors
    /// [`ToolParamError`] when neither or both forms are given, a coordinate is not finite,
    /// or the selector is empty or longer than [`SELECTOR_MAX_BYTES`].
    pub fn target(&self) -> Result<ClickTarget, ToolParamError> {
        match (&self.selector, self.x, self.y) {
            (Some(selector), None, None) => {
                check_text("selector", selector, SELECTOR_MAX_BYTES)?;
                Ok(ClickTarget::Selector(selector.clone()))
            }
            (None, Some(x), Some(y)) => {
                if x.is_finite() && y.is_finite() {
                    Ok(ClickTarget::Point { x, y })
                } else {
                    Err(ToolParamError::Coordinates)
                }
            }
            _ => Err(ToolParamError::ClickTarget),
        }
    }
}

/// Result of `browser_click`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickResult {
    /// Clicked horizontal position.
    pub x: f64,
    /// Clicked vertical position.
    pub y: f64,
}

/// Parameters of `browser_type`. Each `\n` in `text` presses Enter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeParams {
    /// Target tab.
    pub tab_id: TabId,
    /// Text to insert at the focused element.
    pub text: String,
    /// Element to focus first; the currently focused element otherwise.
    #[serde(default)]
    pub selector: Option<String>,
}

impl TypeParams {
    /// Checks the byte bounds of `text` and `selector`.
    ///
    /// # Errors
    /// [`ToolParamError::TooLong`] or [`ToolParamError::Empty`] for out-of-bounds fields.
    pub fn validate(&self) -> Result<(), ToolParamError> {
        if self.text.len() > TYPE_TEXT_MAX_BYTES {
            return Err(ToolParamError::TooLong {
                field: "text",
                max: TYPE_TEXT_MAX_BYTES,
            });
        }
        if let Some(selector) = &self.selector {
            check_text("selector", selector, SELECTOR_MAX_BYTES)?;
        }
        Ok(())
    }
}

/// Result of `browser_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeResult {
    /// Number of characters typed (Enter presses included).
    pub chars: usize,
}

/// Parameters of `browser_snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotParams {
    /// Target tab.
    pub tab_id: TabId,
}

/// Result of `browser_snapshot`: an indented accessibility outline of the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotResult {
    /// The tab.
    pub tab: TabInfo,
    /// One line per accessible node, `role "name"`, indented by depth; at most
    /// [`SNAPSHOT_MAX_BYTES`].
    pub outline: String,
    /// Whether the outline was cut at the bound.
    pub truncated: bool,
}

/// Parameters of `browser_screenshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotParams {
    /// Target tab.
    pub tab_id: TabId,
}

/// Result of `browser_screenshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    /// `image/png`, or `image/jpeg` when the image was shrunk to fit a result bound.
    pub mime_type: String,
    /// Base64 image of the viewport, at most [`SCREENSHOT_MAX_BASE64_BYTES`].
    pub data_base64: String,
}

/// Parameters of `browser_request_user_action`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestUserActionParams {
    /// Tab the user must act in.
    pub tab_id: TabId,
    /// What the user should do, shown in the UI.
    pub reason: String,
}

impl RequestUserActionParams {
    /// Checks that `reason` is non-empty and within [`ACTION_REASON_MAX_BYTES`].
    ///
    /// # Errors
    /// [`ToolParamError`] for an empty or oversized reason.
    pub fn validate(&self) -> Result<(), ToolParamError> {
        check_text("reason", &self.reason, ACTION_REASON_MAX_BYTES)
    }
}

/// Result of `browser_request_user_action`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestUserActionResult {
    /// The tab.
    pub tab_id: TabId,
    /// Server-composed URL that opens the browser panel on this tab; the agent should print
    /// it in the session so the user can click it (BRW-004, UI-011).
    pub link: String,
}

fn check_text(field: &'static str, text: &str, max: usize) -> Result<(), ToolParamError> {
    if text.trim().is_empty() {
        Err(ToolParamError::Empty(field))
    } else if text.len() > max {
        Err(ToolParamError::TooLong { field, max })
    } else {
        Ok(())
    }
}

/// Name, description and JSON input schema of one MCP browser tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// Tool name (`browser_*`).
    pub name: &'static str,
    /// Description shown to the agent.
    pub description: &'static str,
    /// JSON Schema of the tool's `arguments` object.
    pub input_schema: Value,
}

/// The MCP browser tools (SPEC-002 §7.4) with input schemas matching the `*Params` types.
#[must_use]
pub fn tool_specs() -> Vec<ToolSpec> {
    let tab = json!({"type": "string", "description": "Tab id from browser_tabs or browser_open."});
    let url = json!({"type": "string", "maxLength": URL_MAX_BYTES,
        "description": "http(s) address or about:blank; a bare host gets https://."});
    let selector = json!({"type": "string", "maxLength": SELECTOR_MAX_BYTES,
        "description": "CSS selector; the first match is used."});
    vec![
        ToolSpec {
            name: TOOL_TABS,
            description: "List the shared sandbox browser's tabs (id, title, url, active, pending user action).",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_OPEN,
            description: "Open a new tab in the shared sandbox browser (the user sees the same tab) and wait for it to load.",
            input_schema: json!({"type": "object", "properties": {"url": url},
                "required": ["url"], "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_NAVIGATE,
            description: "Navigate a tab to an address and wait for it to load.",
            input_schema: json!({"type": "object", "properties": {"tab_id": tab, "url": url},
                "required": ["tab_id", "url"], "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_CLICK,
            description: "Left-click the centre of the element matching a CSS selector, or a viewport point (x, y in CSS pixels).",
            input_schema: json!({"type": "object", "properties": {"tab_id": tab, "selector": selector,
                "x": {"type": "number"}, "y": {"type": "number"}},
                "required": ["tab_id"], "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_TYPE,
            description: "Type text into the focused element, or into the element matching selector after focusing it. Each newline presses Enter.",
            input_schema: json!({"type": "object", "properties": {"tab_id": tab,
                "text": {"type": "string", "maxLength": TYPE_TEXT_MAX_BYTES}, "selector": selector},
                "required": ["tab_id", "text"], "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_SNAPSHOT,
            description: "Accessible text outline of the page (roles and names, indented), at most 64 KiB.",
            input_schema: json!({"type": "object", "properties": {"tab_id": tab},
                "required": ["tab_id"], "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_SCREENSHOT,
            description: "Screenshot of the tab's viewport as an image: PNG, or a smaller JPEG (lower quality, downscaled) when needed to fit the tool result limit; the accompanying text gives the scale.",
            input_schema: json!({"type": "object", "properties": {"tab_id": tab},
                "required": ["tab_id"], "additionalProperties": false}),
        },
        ToolSpec {
            name: TOOL_REQUEST_USER_ACTION,
            description: "Ask the user to act in a tab (e.g. log in). Highlights the tab, notifies the user and returns a link to print in the session.",
            input_schema: json!({"type": "object", "properties": {"tab_id": tab,
                "reason": {"type": "string", "maxLength": ACTION_REASON_MAX_BYTES}},
                "required": ["tab_id", "reason"], "additionalProperties": false}),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn tab_id_validation_matches_frame_field() {
        assert!(TabId::parse("0123456789ABCDEF0123456789ABCDEF").is_ok());
        assert_eq!(TabId::parse(""), Err(TabIdError));
        assert_eq!(TabId::parse(&"A".repeat(33)), Err(TabIdError));
        assert_eq!(TabId::parse("a b"), Err(TabIdError));
        assert_eq!(TabId::parse("tab\u{e9}"), Err(TabIdError));
        assert_eq!(TabId::parse("tab\n"), Err(TabIdError));
    }

    #[test]
    fn viewer_client_messages_parse_as_documented() -> TestResult {
        let cases = [
            (
                r#"{"type":"open","url":"https://example.com"}"#,
                ViewerClientMsg::Open {
                    url: "https://example.com".into(),
                },
            ),
            (
                r#"{"type":"navigate","tab_id":"T1","url":"about:blank"}"#,
                ViewerClientMsg::Navigate {
                    tab_id: TabId::parse("T1")?,
                    url: "about:blank".into(),
                },
            ),
            (
                r#"{"type":"activate","tab_id":"T1"}"#,
                ViewerClientMsg::Activate {
                    tab_id: TabId::parse("T1")?,
                },
            ),
            (
                r#"{"type":"close","tab_id":"T1"}"#,
                ViewerClientMsg::Close {
                    tab_id: TabId::parse("T1")?,
                },
            ),
            (
                r#"{"type":"mouse","tab_id":"T1","event":"down","x":10.5,"y":20,"button":"left","dx":null,"dy":0}"#,
                ViewerClientMsg::Mouse {
                    tab_id: TabId::parse("T1")?,
                    event: MouseEventKind::Down,
                    x: 10.5,
                    y: 20.0,
                    button: Some(MouseButton::Left),
                    dx: None,
                    dy: Some(0.0),
                },
            ),
            (
                r#"{"type":"mouse","tab_id":"T1","event":"wheel","x":1,"y":2,"dy":-120}"#,
                ViewerClientMsg::Mouse {
                    tab_id: TabId::parse("T1")?,
                    event: MouseEventKind::Wheel,
                    x: 1.0,
                    y: 2.0,
                    button: None,
                    dx: None,
                    dy: Some(-120.0),
                },
            ),
            (
                r#"{"type":"key","tab_id":"T1","event":"down","key":"a","code":"KeyA","text":"a","modifiers":8}"#,
                ViewerClientMsg::Key {
                    tab_id: TabId::parse("T1")?,
                    event: KeyEventKind::Down,
                    key: Some("a".into()),
                    code: Some("KeyA".into()),
                    text: Some("a".into()),
                    modifiers: Some(MODIFIER_SHIFT),
                },
            ),
            (
                r#"{"type":"key","tab_id":"T1","event":"char","text":"ż"}"#,
                ViewerClientMsg::Key {
                    tab_id: TabId::parse("T1")?,
                    event: KeyEventKind::Char,
                    key: None,
                    code: None,
                    text: Some("ż".into()),
                    modifiers: None,
                },
            ),
            (
                r#"{"type":"viewport","width":1280,"height":720}"#,
                ViewerClientMsg::Viewport {
                    width: 1280,
                    height: 720,
                },
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                serde_json::from_str::<ViewerClientMsg>(text)?,
                expected,
                "{text}"
            );
        }
        Ok(())
    }

    #[test]
    fn viewer_client_rejects_unknown_types_and_bad_tab_ids() {
        assert!(serde_json::from_str::<ViewerClientMsg>(r#"{"type":"eval","code":"1"}"#).is_err());
        assert!(
            serde_json::from_str::<ViewerClientMsg>(r#"{"type":"activate","tab_id":"a b"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<ViewerClientMsg>(
                r#"{"type":"mouse","tab_id":"T","event":"drag","x":1,"y":1}"#
            )
            .is_err()
        );
    }

    #[test]
    fn viewer_server_messages_serialize_as_documented() -> TestResult {
        let tabs = ViewerServerMsg::Tabs {
            tabs: vec![
                TabInfo {
                    id: TabId::parse("T1")?,
                    title: "Example".into(),
                    url: "https://example.com/".into(),
                    active: true,
                    action_requested: None,
                },
                TabInfo {
                    id: TabId::parse("T2")?,
                    title: String::new(),
                    url: "about:blank".into(),
                    active: false,
                    action_requested: Some(ActionRequest {
                        reason: "Log in to AWS".into(),
                        task_id: Some("task_1".into()),
                    }),
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(&tabs)?,
            json!({"type": "tabs", "tabs": [
            {"id": "T1", "title": "Example", "url": "https://example.com/", "active": true, "action_requested": null},
            {"id": "T2", "title": "", "url": "about:blank", "active": false,
             "action_requested": {"reason": "Log in to AWS", "task_id": "task_1"}}]})
        );
        let action = ViewerServerMsg::ActionRequested {
            tab_id: TabId::parse("T2")?,
            reason: "r".into(),
            task_id: None,
        };
        assert_eq!(
            serde_json::to_value(&action)?,
            json!({"type": "action_requested", "tab_id": "T2", "reason": "r", "task_id": null})
        );
        let error = ViewerServerMsg::Error {
            code: ViewerErrorCode::UrlRefused,
            message: "m".into(),
        };
        assert_eq!(
            serde_json::to_value(&error)?,
            json!({"type": "error", "code": "url_refused", "message": "m"})
        );
        Ok(())
    }

    #[test]
    fn frames_round_trip_with_padding() -> TestResult {
        let tab = TabId::parse("ABC")?;
        let frame = encode_frame(&tab, &[0xFF, 0xD8, 0x01]);
        assert_eq!(frame[0], FRAME_KIND_BROWSER);
        assert_eq!(&frame[1..4], b"ABC");
        assert!(frame[4..FRAME_HEADER_LEN].iter().all(|&b| b == b' '));
        assert_eq!(decode_frame(&frame)?, (tab, &[0xFF, 0xD8, 0x01][..]));
        let full = TabId::parse(&"F".repeat(32))?;
        let frame = encode_frame(&full, b"");
        assert_eq!(frame.len(), FRAME_HEADER_LEN);
        assert_eq!(decode_frame(&frame)?, (full, &b""[..]));
        Ok(())
    }

    #[test]
    fn malformed_frames_are_rejected() {
        assert_eq!(decode_frame(&[3; 10]), Err(FrameError::Truncated));
        let mut frame = vec![1_u8];
        frame.extend_from_slice(&[b'A'; 32]);
        assert_eq!(decode_frame(&frame), Err(FrameError::WrongKind(1)));
        let mut blank = vec![3_u8];
        blank.extend_from_slice(&[b' '; 32]);
        assert_eq!(decode_frame(&blank), Err(FrameError::InvalidTabId));
        let mut inner_space = vec![3_u8];
        inner_space.extend_from_slice(b"A B");
        inner_space.resize(FRAME_HEADER_LEN, b' ');
        assert_eq!(decode_frame(&inner_space), Err(FrameError::InvalidTabId));
    }

    #[test]
    fn click_params_need_exactly_one_target() -> TestResult {
        let tab = TabId::parse("T")?;
        let params = |selector: Option<&str>, x: Option<f64>, y: Option<f64>| ClickParams {
            tab_id: tab.clone(),
            selector: selector.map(str::to_owned),
            x,
            y,
        };
        assert_eq!(
            params(Some("#go"), None, None).target(),
            Ok(ClickTarget::Selector("#go".into()))
        );
        assert_eq!(
            params(None, Some(1.0), Some(2.0)).target(),
            Ok(ClickTarget::Point { x: 1.0, y: 2.0 })
        );
        assert_eq!(
            params(None, None, None).target(),
            Err(ToolParamError::ClickTarget)
        );
        assert_eq!(
            params(Some("#go"), Some(1.0), Some(2.0)).target(),
            Err(ToolParamError::ClickTarget)
        );
        assert_eq!(
            params(None, Some(1.0), None).target(),
            Err(ToolParamError::ClickTarget)
        );
        assert_eq!(
            params(None, Some(f64::NAN), Some(1.0)).target(),
            Err(ToolParamError::Coordinates)
        );
        assert_eq!(
            params(Some(" "), None, None).target(),
            Err(ToolParamError::Empty("selector"))
        );
        let long = "a".repeat(SELECTOR_MAX_BYTES + 1);
        assert_eq!(
            params(Some(&long), None, None).target(),
            Err(ToolParamError::TooLong {
                field: "selector",
                max: SELECTOR_MAX_BYTES
            })
        );
        let parsed: ClickParams = serde_json::from_str(r##"{"tab_id":"T","selector":"#go"}"##)?;
        assert_eq!(parsed.target(), Ok(ClickTarget::Selector("#go".into())));
        assert!(serde_json::from_str::<ClickParams>(r#"{"tab_id":"T","sel":"x"}"#).is_err());
        Ok(())
    }

    #[test]
    fn type_and_action_params_are_bounded() -> TestResult {
        let tab = TabId::parse("T")?;
        let typed = TypeParams {
            tab_id: tab.clone(),
            text: "x".repeat(TYPE_TEXT_MAX_BYTES + 1),
            selector: None,
        };
        assert_eq!(
            typed.validate(),
            Err(ToolParamError::TooLong {
                field: "text",
                max: TYPE_TEXT_MAX_BYTES
            })
        );
        assert_eq!(
            TypeParams {
                tab_id: tab.clone(),
                text: String::new(),
                selector: None
            }
            .validate(),
            Ok(())
        );
        let action = RequestUserActionParams {
            tab_id: tab.clone(),
            reason: String::new(),
        };
        assert_eq!(action.validate(), Err(ToolParamError::Empty("reason")));
        let action = RequestUserActionParams {
            tab_id: tab,
            reason: "r".repeat(ACTION_REASON_MAX_BYTES + 1),
        };
        assert_eq!(
            action.validate(),
            Err(ToolParamError::TooLong {
                field: "reason",
                max: ACTION_REASON_MAX_BYTES
            })
        );
        Ok(())
    }

    #[test]
    fn tool_specs_cover_the_spec_tool_set() {
        let names: Vec<_> = tool_specs().iter().map(|spec| spec.name).collect();
        assert_eq!(
            names,
            [
                TOOL_TABS,
                TOOL_OPEN,
                TOOL_NAVIGATE,
                TOOL_CLICK,
                TOOL_TYPE,
                TOOL_SNAPSHOT,
                TOOL_SCREENSHOT,
                TOOL_REQUEST_USER_ACTION
            ]
        );
        for spec in tool_specs() {
            assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
            assert_eq!(
                spec.input_schema["additionalProperties"], false,
                "{}",
                spec.name
            );
        }
    }
}
