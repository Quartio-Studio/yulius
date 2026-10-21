//! Shared sandbox browser port (SPEC-002 ADR-15; SPEC-001 BRW-001..008, U-10).
//!
//! One Chromium per sandbox, lazily started by [`BrowserPort::ensure`] and shared by the
//! sandbox's users (viewer WebSocket) and agents (MCP tools). With a virtual display users get
//! the browser's video ([`BrowserSession::video`]) and real input; without one, JPEG frames
//! ([`BrowserSession::frames`]). Only contracts and application types cross this boundary; the
//! adapter owns the Chrome DevTools Protocol, the display and the encoder.
//!
//! Trust model: the browser's DevTools protocol has no authentication, so the adapter opens no
//! DevTools listener: Yulius speaks to Chromium over a pipe only it holds (ADR-0191). No sandbox
//! process, isolated or not (each has its own PID namespace), and no host process of another
//! user can drive the browser or its logged-in sessions through DevTools; only Yulius's own user
//! or root could, through `/proc/<pid>/fd`. Agents use it through Yulius's browser tools. No raw
//! CDP endpoint is offered to a sandbox's CDP-capable MCP servers (SPEC-001 U-10).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::watch;
use yulius_contracts::browser::{
    ActionRequest, ClickParams, ClickResult, NavigateParams, NavigationResult, OpenParams,
    ScreenshotParams, SnapshotParams, SnapshotResult, TabId, TabInfo, TabsResult, TypeParams,
    TypeResult, ViewerClientMsg, ViewerErrorCode,
};
use yulius_domain::ids::{SandboxId, TaskId};

use crate::error::AppError;

/// Why a browser operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserFailureKind {
    /// Chromium is not installed or not configured (an external prerequisite).
    NotInstalled,
    /// The browser could not be started or did not answer.
    Upstream,
    /// The URL policy refused an address.
    UrlRefused,
    /// Unknown tab.
    NotFound,
    /// Tab or resource limit reached.
    Capacity,
    /// Malformed or out-of-bounds input (including a missing element).
    Invalid,
    /// A result exceeded its bound.
    TooLarge,
    /// Unexpected local failure.
    Internal,
}

/// A browser failure with a message safe to show to users and agents (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BrowserFailure {
    /// Classification.
    pub kind: BrowserFailureKind,
    /// Human-readable text.
    pub message: String,
}

impl BrowserFailure {
    /// A failure of `kind` with `message`.
    pub fn new(kind: BrowserFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// The viewer WebSocket error code.
    #[must_use]
    pub fn viewer_code(&self) -> ViewerErrorCode {
        match self.kind {
            BrowserFailureKind::UrlRefused => ViewerErrorCode::UrlRefused,
            BrowserFailureKind::NotFound => ViewerErrorCode::NotFound,
            BrowserFailureKind::Capacity => ViewerErrorCode::Capacity,
            BrowserFailureKind::Invalid => ViewerErrorCode::Invalid,
            BrowserFailureKind::NotInstalled
            | BrowserFailureKind::Upstream
            | BrowserFailureKind::TooLarge => ViewerErrorCode::Upstream,
            BrowserFailureKind::Internal => ViewerErrorCode::Internal,
        }
    }
}

impl From<BrowserFailure> for AppError {
    fn from(failure: BrowserFailure) -> Self {
        match failure.kind {
            // Unknown tabs keep their message (an agent needs to know which id was wrong).
            BrowserFailureKind::UrlRefused
            | BrowserFailureKind::Invalid
            | BrowserFailureKind::NotFound => Self::Invalid(failure.message),
            BrowserFailureKind::Capacity => Self::Capacity(failure.message),
            BrowserFailureKind::NotInstalled
            | BrowserFailureKind::Upstream
            | BrowserFailureKind::TooLarge => Self::Upstream(failure.message),
            BrowserFailureKind::Internal => Self::Internal(failure.message),
        }
    }
}

/// Browser notifications for viewers.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserNotice {
    /// An agent asked the user to act in a tab.
    ActionRequested {
        /// The tab.
        tab_id: TabId,
        /// Reason shown to the user.
        reason: String,
        /// Requesting task.
        task_id: Option<String>,
    },
    /// A pending request was cleared.
    ActionCleared {
        /// The tab.
        tab_id: TabId,
    },
    /// The browser exited or crashed; the next [`BrowserPort::ensure`] restarts it.
    Disconnected,
    /// An agent acted in a tab (the agent's cursor, BRW-007).
    AgentActivity {
        /// The tab.
        tab_id: TabId,
        /// Position in CSS pixels, when the action has one.
        x: Option<f64>,
        /// Position in CSS pixels, when the action has one.
        y: Option<f64>,
        /// `click`, `type`, `open` or `navigate`.
        action: String,
        /// Who acted.
        label: String,
    },
    /// The viewport of all tabs changed.
    Viewport {
        /// Width in CSS pixels.
        width: u32,
        /// Height in CSS pixels.
        height: u32,
    },
}

/// What a video subscriber receives, in order (SPEC-002 ADR-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoUpdate {
    /// Decoder configuration for the frames that follow.
    Config {
        /// Codec string, e.g. `avc1.42c028`.
        codec: String,
        /// The `avcC` record.
        description: Arc<[u8]>,
        /// Coded width in device pixels.
        width: u32,
        /// Coded height in device pixels.
        height: u32,
    },
    /// One access unit.
    Frame {
        /// A decoder can start here.
        keyframe: bool,
        /// Presentation timestamp in microseconds.
        timestamp_us: u64,
        /// Length-prefixed NAL units.
        data: Arc<[u8]>,
    },
}

impl VideoUpdate {
    /// Whether a decoder can start at this update (a configuration or a keyframe).
    #[must_use]
    pub fn starts_decoding(&self) -> bool {
        match self {
            Self::Config { .. } => true,
            Self::Frame { keyframe, .. } => *keyframe,
        }
    }
}

/// What a viewer connection measured about its network over the last period (about a second;
/// SPEC-002 ADR-15, F 2026-10-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoNetworkReport {
    /// Video the viewer acknowledged, in bits per second.
    pub delivered_bps: u64,
    /// Frames queued up on the way to the viewer (its round trip grew well beyond the base, or
    /// frames were dropped because it fell behind).
    pub congested: bool,
    /// The path, not the stream, limited what was delivered (video waited for it), so
    /// `delivered_bps` measures what the path carries.
    pub limited: bool,
}

/// The video of the sandbox browser's display; dropping it unsubscribes. A subscriber that
/// falls behind misses frames until the next keyframe; the producer never waits for it.
#[async_trait]
pub trait BrowserVideo: Send {
    /// The configuration and cached frames to send before live ones (taken once).
    fn take_backlog(&mut self) -> Vec<VideoUpdate>;
    /// The next live update; `None` once the stream ended (the browser stopped or the encoder
    /// failed: the viewer falls back to JPEG frames).
    async fn next(&mut self) -> Option<VideoUpdate>;
    /// Asks for a keyframe (this viewer fell behind or its decoder failed); the producer
    /// rate-limits it. This subscription resumes at the next keyframe.
    fn request_keyframe(&mut self) {}
    /// Reports this viewer's network: the stream's bitrate and frame rate follow the slowest
    /// viewer.
    fn report(&mut self, _report: VideoNetworkReport) {}
}

/// A screenshot that fits the caller's byte budget.
#[derive(Debug, Clone, PartialEq)]
pub struct Screenshot {
    /// `image/png` or `image/jpeg`.
    pub mime_type: String,
    /// Base64 image.
    pub data_base64: String,
    /// JPEG quality when the image was re-encoded; `None` for PNG.
    pub quality: Option<u8>,
    /// Image scale relative to the viewport (1.0 = CSS pixels).
    pub scale: f64,
    /// Viewport (width, height) in CSS pixels.
    pub viewport: (u32, u32),
}

/// Screencast frames (JPEG) of one tab; dropping it unsubscribes. A slow consumer loses the
/// oldest frames; the producer never blocks.
#[async_trait]
pub trait BrowserFrames: Send {
    /// The tab.
    fn tab_id(&self) -> &TabId;
    /// The next JPEG frame; `None` once the tab or browser closed.
    async fn next(&mut self) -> Option<Arc<[u8]>>;
    /// Frames skipped because this consumer lagged.
    fn dropped(&self) -> u64;
}

/// Notifications of one browser; lagging consumers skip missed notices.
#[async_trait]
pub trait BrowserNotices: Send {
    /// The next notice; `None` once the browser is gone.
    async fn next(&mut self) -> Option<BrowserNotice>;
}

/// One running sandbox browser.
#[async_trait]
pub trait BrowserSession: Send + Sync {
    /// Whether the browser still runs.
    fn is_alive(&self) -> bool;
    /// Records activity (resets the idle timer).
    fn touch(&self);
    /// Current tabs in creation order.
    fn tabs(&self) -> Vec<TabInfo>;
    /// The viewport of all tabs (width, height) in CSS pixels.
    fn viewport(&self) -> (u32, u32);
    /// The tab list, updated after every change.
    fn watch_tabs(&self) -> watch::Receiver<Vec<TabInfo>>;
    /// User-action and connection notifications.
    fn notices(&self) -> Box<dyn BrowserNotices>;
    /// Applies one viewer message (open, navigate, activate, close, input, viewport).
    async fn apply_viewer(&self, message: ViewerClientMsg) -> Result<(), BrowserFailure>;
    /// Subscribes to a tab's JPEG screencast (the fallback view).
    async fn frames(&self, tab: &TabId) -> Result<Box<dyn BrowserFrames>, BrowserFailure>;
    /// Subscribes to the display's H.264 video; an error says why there is none (no display,
    /// the encoder failed).
    fn video(&self) -> Result<Box<dyn BrowserVideo>, String>;
    /// The text the user selected in a tab (what they copied).
    async fn copy_selection(&self, tab: &TabId) -> Result<String, BrowserFailure>;
    /// Releases every key and button a viewer held on the display (it left).
    fn release_input(&self);
    /// Tells viewers that an agent acted in a tab (BRW-007).
    fn agent_activity(&self, tab: &TabId, position: Option<(f64, f64)>, action: &str, label: &str);
    /// `browser_tabs`.
    fn tool_tabs(&self) -> TabsResult;
    /// `browser_open`.
    async fn tool_open(&self, params: &OpenParams) -> Result<NavigationResult, BrowserFailure>;
    /// `browser_navigate`.
    async fn tool_navigate(
        &self,
        params: &NavigateParams,
    ) -> Result<NavigationResult, BrowserFailure>;
    /// `browser_click`.
    async fn tool_click(&self, params: &ClickParams) -> Result<ClickResult, BrowserFailure>;
    /// `browser_type`.
    async fn tool_type(&self, params: &TypeParams) -> Result<TypeResult, BrowserFailure>;
    /// `browser_snapshot`.
    async fn tool_snapshot(
        &self,
        params: &SnapshotParams,
    ) -> Result<SnapshotResult, BrowserFailure>;
    /// `browser_screenshot` within `max_base64_bytes`.
    async fn tool_screenshot(
        &self,
        params: &ScreenshotParams,
        max_base64_bytes: usize,
    ) -> Result<Screenshot, BrowserFailure>;
    /// `browser_request_user_action`: marks the tab and notifies viewers.
    fn request_user_action(
        &self,
        tab: &TabId,
        reason: &str,
        task: Option<&TaskId>,
    ) -> Result<ActionRequest, BrowserFailure>;
}

/// Owns at most one browser per sandbox.
#[async_trait]
pub trait BrowserPort: Send + Sync {
    /// The sandbox's running browser, started (or restarted after a crash) with
    /// `profile_dir` as its profile.
    async fn ensure(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure>;
    /// The sandbox's browser if it runs; never starts one.
    async fn running(&self, sandbox: &SandboxId) -> Option<Arc<dyn BrowserSession>>;
    /// Stops idle browsers (no viewers, no recent activity) and reaps crashed ones; returns
    /// the stopped sandboxes.
    async fn reap_idle(&self) -> Vec<SandboxId>;
    /// Stops `sandbox`'s browser (its sandbox was removed, SPEC-206); `false` when none ran or
    /// its process survived.
    async fn stop(&self, sandbox: &SandboxId) -> bool;
    /// Stops every browser (server shutdown); returns sandboxes whose process survived.
    async fn shutdown_all(&self) -> Vec<SandboxId>;
}
