//! Shared sandbox browser port (SPEC-002 ADR-09; SPEC-001 BRW-001..004, U-10).
//!
//! One headless Chromium per sandbox, lazily started by [`BrowserPort::ensure`] and shared by
//! the sandbox's users (viewer WebSocket) and agents (MCP tools). Only contracts and
//! application types cross this boundary; the adapter owns the Chrome DevTools Protocol.
//!
//! Trust model: the adapter's DevTools endpoint listens on loopback without authentication,
//! so any local process (including other sandboxes' processes, which share loopback) can
//! drive the browser and its logged-in sessions. See [`BrowserSession::cdp_endpoint`].

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The tab list, updated after every change.
    fn watch_tabs(&self) -> watch::Receiver<Vec<TabInfo>>;
    /// User-action and connection notifications.
    fn notices(&self) -> Box<dyn BrowserNotices>;
    /// Applies one viewer message (open, navigate, activate, close, input, viewport).
    async fn apply_viewer(&self, message: ViewerClientMsg) -> Result<(), BrowserFailure>;
    /// Subscribes to a tab's screencast.
    async fn frames(&self, tab: &TabId) -> Result<Box<dyn BrowserFrames>, BrowserFailure>;
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
    /// The unauthenticated loopback DevTools WebSocket URL (for CDP-capable MCP servers of the
    /// same sandbox, U-10).
    fn cdp_endpoint(&self) -> String;
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
    /// Stops every browser (server shutdown); returns sandboxes whose process survived.
    async fn shutdown_all(&self) -> Vec<SandboxId>;
}
