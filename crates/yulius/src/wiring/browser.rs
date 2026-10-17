//! [`BrowserPort`] over `yulius-browser`: one Chromium per sandbox, headful on its own virtual
//! display when one is configured (SPEC-002 ADR-15).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{broadcast, watch};
use yulius_app::ports::browser::{
    BrowserFailure, BrowserFailureKind, BrowserFrames, BrowserNotice, BrowserNotices, BrowserPort,
    BrowserSession, BrowserVideo, Screenshot, VideoNetworkReport, VideoUpdate,
};
use yulius_browser::{
    BrowserConfig, BrowserError, BrowserEvent, BrowserInstance, BrowserManager, FrameSubscription,
    NetworkReport, VideoPacket, VideoSubscription,
};
use yulius_contracts::browser::{
    ActionRequest, ClickParams, ClickResult, NavigateParams, NavigationResult, OpenParams,
    ScreenshotParams, SnapshotParams, SnapshotResult, TabId, TabInfo, TabsResult, TypeParams,
    TypeResult, ViewerClientMsg,
};
use yulius_domain::ids::{SandboxId, TaskId};

/// Maps adapter errors to the application's classification; messages carry no secrets.
fn failure(error: BrowserError) -> BrowserFailure {
    let kind = match &error {
        BrowserError::ExecutableNotFound { .. } => BrowserFailureKind::NotInstalled,
        BrowserError::UrlRefused(_) => BrowserFailureKind::UrlRefused,
        BrowserError::TabNotFound(_) => BrowserFailureKind::NotFound,
        BrowserError::TooManyTabs(_) | BrowserError::InputBacklog => BrowserFailureKind::Capacity,
        BrowserError::InvalidInput(_)
        | BrowserError::ToolParams(_)
        | BrowserError::InvalidSelector(_)
        | BrowserError::ElementNotFound
        | BrowserError::ElementNotVisible => BrowserFailureKind::Invalid,
        BrowserError::ResultTooLarge { .. } => BrowserFailureKind::TooLarge,
        BrowserError::Profile(_) | BrowserError::NotReaped { .. } => BrowserFailureKind::Internal,
        BrowserError::Spawn(_)
        | BrowserError::StartupTimeout { .. }
        | BrowserError::ExitedEarly { .. }
        | BrowserError::Cdp(_)
        | BrowserError::NavigationFailed(_)
        | BrowserError::Display(_) => BrowserFailureKind::Upstream,
    };
    BrowserFailure::new(kind, error.to_string())
}

/// Chromium browsers per sandbox.
pub struct ChromiumBrowsers {
    manager: Arc<BrowserManager>,
}

impl ChromiumBrowsers {
    /// Browsers started with `config` (profiles are passed per sandbox by the application).
    #[must_use]
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            manager: Arc::new(BrowserManager::new(config)),
        }
    }
}

#[async_trait]
impl BrowserPort for ChromiumBrowsers {
    async fn ensure(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        let instance = self
            .manager
            .ensure(sandbox, profile_dir)
            .await
            .map_err(failure)?;
        Ok(Arc::new(Session(instance)))
    }

    async fn running(&self, sandbox: &SandboxId) -> Option<Arc<dyn BrowserSession>> {
        let instance = self.manager.get(sandbox).await?;
        Some(Arc::new(Session(instance)))
    }

    async fn reap_idle(&self) -> Vec<SandboxId> {
        self.manager.reap_idle(Instant::now()).await
    }

    async fn stop(&self, sandbox: &SandboxId) -> bool {
        self.manager
            .shutdown(sandbox)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%sandbox, %error, "the browser could not be stopped");
                false
            })
    }

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        self.manager.shutdown_all().await
    }

    async fn devtools_ports(&self) -> Vec<u16> {
        self.manager.devtools_ports()
    }
}

struct Session(Arc<BrowserInstance>);

struct Frames(FrameSubscription);

#[async_trait]
impl BrowserFrames for Frames {
    fn tab_id(&self) -> &TabId {
        self.0.tab_id()
    }

    async fn next(&mut self) -> Option<Arc<[u8]>> {
        self.0.recv().await.map(|frame| frame.jpeg)
    }

    fn dropped(&self) -> u64 {
        self.0.frames_dropped()
    }
}

struct Video(VideoSubscription);

fn video_update(packet: VideoPacket) -> VideoUpdate {
    match packet {
        VideoPacket::Config(config) => VideoUpdate::Config {
            codec: config.codec.clone(),
            description: std::sync::Arc::clone(&config.description),
            width: config.width,
            height: config.height,
        },
        VideoPacket::Frame(frame) => VideoUpdate::Frame {
            keyframe: frame.keyframe,
            timestamp_us: frame.timestamp_us,
            data: frame.data,
        },
    }
}

#[async_trait]
impl BrowserVideo for Video {
    fn take_backlog(&mut self) -> Vec<VideoUpdate> {
        self.0
            .take_backlog()
            .into_iter()
            .map(video_update)
            .collect()
    }

    async fn next(&mut self) -> Option<VideoUpdate> {
        self.0.recv().await.map(video_update)
    }

    fn request_keyframe(&mut self) {
        self.0.request_keyframe();
    }

    fn report(&mut self, report: VideoNetworkReport) {
        self.0.report(NetworkReport {
            delivered_bps: report.delivered_bps,
            congested: report.congested,
            limited: report.limited,
        });
    }
}

struct Notices(broadcast::Receiver<BrowserEvent>);

#[async_trait]
impl BrowserNotices for Notices {
    async fn next(&mut self) -> Option<BrowserNotice> {
        loop {
            match self.0.recv().await {
                Ok(BrowserEvent::ActionRequested {
                    tab_id,
                    reason,
                    task_id,
                }) => {
                    return Some(BrowserNotice::ActionRequested {
                        tab_id,
                        reason,
                        task_id,
                    });
                }
                Ok(BrowserEvent::ActionCleared { tab_id }) => {
                    return Some(BrowserNotice::ActionCleared { tab_id });
                }
                Ok(BrowserEvent::Disconnected) => return Some(BrowserNotice::Disconnected),
                Ok(BrowserEvent::AgentActivity {
                    tab_id,
                    x,
                    y,
                    action,
                    label,
                }) => {
                    return Some(BrowserNotice::AgentActivity {
                        tab_id,
                        x,
                        y,
                        action,
                        label,
                    });
                }
                Ok(BrowserEvent::Viewport { width, height }) => {
                    return Some(BrowserNotice::Viewport { width, height });
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[async_trait]
impl BrowserSession for Session {
    fn is_alive(&self) -> bool {
        self.0.is_alive()
    }

    fn touch(&self) {
        self.0.touch();
    }

    fn tabs(&self) -> Vec<TabInfo> {
        self.0.tabs()
    }

    fn viewport(&self) -> (u32, u32) {
        self.0.viewport()
    }

    fn watch_tabs(&self) -> watch::Receiver<Vec<TabInfo>> {
        self.0.subscribe_tabs()
    }

    fn notices(&self) -> Box<dyn BrowserNotices> {
        Box::new(Notices(self.0.subscribe_events()))
    }

    async fn apply_viewer(&self, message: ViewerClientMsg) -> Result<(), BrowserFailure> {
        self.0
            .apply_viewer_msg(message)
            .await
            .map(|_| ())
            .map_err(failure)
    }

    async fn frames(&self, tab: &TabId) -> Result<Box<dyn BrowserFrames>, BrowserFailure> {
        let subscription = self.0.subscribe_frames(tab).await.map_err(failure)?;
        Ok(Box::new(Frames(subscription)))
    }

    fn video(&self) -> Result<Box<dyn BrowserVideo>, String> {
        self.0
            .subscribe_video()
            .map(|subscription| Box::new(Video(subscription)) as Box<dyn BrowserVideo>)
            .map_err(|unavailable| unavailable.0)
    }

    async fn copy_selection(&self, tab: &TabId) -> Result<String, BrowserFailure> {
        self.0.copy_selection(tab).await.map_err(failure)
    }

    fn release_input(&self) {
        self.0.release_input();
    }

    fn agent_activity(&self, tab: &TabId, position: Option<(f64, f64)>, action: &str, label: &str) {
        self.0.agent_activity(tab, position, action, label);
    }

    fn tool_tabs(&self) -> TabsResult {
        self.0.tool_tabs()
    }

    async fn tool_open(&self, params: &OpenParams) -> Result<NavigationResult, BrowserFailure> {
        self.0.tool_open(params).await.map_err(failure)
    }

    async fn tool_navigate(
        &self,
        params: &NavigateParams,
    ) -> Result<NavigationResult, BrowserFailure> {
        self.0.tool_navigate(params).await.map_err(failure)
    }

    async fn tool_click(&self, params: &ClickParams) -> Result<ClickResult, BrowserFailure> {
        self.0.tool_click(params).await.map_err(failure)
    }

    async fn tool_type(&self, params: &TypeParams) -> Result<TypeResult, BrowserFailure> {
        self.0.tool_type(params).await.map_err(failure)
    }

    async fn tool_snapshot(
        &self,
        params: &SnapshotParams,
    ) -> Result<SnapshotResult, BrowserFailure> {
        self.0.tool_snapshot(params).await.map_err(failure)
    }

    async fn tool_screenshot(
        &self,
        params: &ScreenshotParams,
        max_base64_bytes: usize,
    ) -> Result<Screenshot, BrowserFailure> {
        let shot = self
            .0
            .tool_screenshot_within(params, max_base64_bytes)
            .await
            .map_err(failure)?;
        Ok(Screenshot {
            mime_type: shot.result.mime_type,
            data_base64: shot.result.data_base64,
            quality: shot.quality,
            scale: shot.scale,
            viewport: shot.viewport,
        })
    }

    fn request_user_action(
        &self,
        tab: &TabId,
        reason: &str,
        task: Option<&TaskId>,
    ) -> Result<ActionRequest, BrowserFailure> {
        self.0
            .request_user_action(tab, reason, task)
            .map_err(failure)
    }

    fn cdp_endpoint(&self) -> String {
        self.0.cdp_ws_url().to_owned()
    }
}

/// Placeholder when no browser is configured: every call fails with a clear message.
pub struct UnavailableBrowser;

fn unavailable() -> BrowserFailure {
    BrowserFailure::new(
        BrowserFailureKind::NotInstalled,
        "the sandbox browser is not available in this build",
    )
}

#[async_trait]
impl BrowserPort for UnavailableBrowser {
    async fn ensure(
        &self,
        _: &SandboxId,
        _: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        Err(unavailable())
    }

    async fn running(&self, _: &SandboxId) -> Option<Arc<dyn BrowserSession>> {
        None
    }

    async fn reap_idle(&self) -> Vec<SandboxId> {
        Vec::new()
    }

    async fn stop(&self, _: &SandboxId) -> bool {
        false
    }

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        Vec::new()
    }
}
