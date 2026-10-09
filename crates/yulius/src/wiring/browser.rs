//! [`BrowserPort`] over `yulius-browser`: one headless Chromium per sandbox (SPEC-002 ADR-09).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{broadcast, watch};
use yulius_app::ports::browser::{
    BrowserDom, BrowserFailure, BrowserFailureKind, BrowserFrames, BrowserNotice, BrowserNotices,
    BrowserPort, BrowserSession, DomUpdate, FetchedResource, Screenshot,
};
use yulius_browser::{
    BrowserConfig, BrowserError, BrowserEvent, BrowserInstance, BrowserManager, DomEvent,
    DomSubscription, FrameSubscription,
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
        BrowserError::TooManyTabs(_) => BrowserFailureKind::Capacity,
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
        | BrowserError::NavigationFailed(_) => BrowserFailureKind::Upstream,
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

struct Dom(DomSubscription);

#[async_trait]
impl BrowserDom for Dom {
    fn tab_id(&self) -> &TabId {
        self.0.tab_id()
    }

    async fn next(&mut self) -> Option<DomUpdate> {
        Some(match self.0.recv().await? {
            DomEvent::Batch(batch) => DomUpdate::Batch {
                reset: batch.reset,
                events: batch.events,
            },
            DomEvent::Lagged => DomUpdate::Lagged,
        })
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

    async fn dom(&self, tab: &TabId) -> Result<Box<dyn BrowserDom>, BrowserFailure> {
        let subscription = self.0.subscribe_dom(tab).await.map_err(failure)?;
        Ok(Box::new(Dom(subscription)))
    }

    async fn fetch_resource(
        &self,
        tab: &TabId,
        url: &str,
    ) -> Result<FetchedResource, BrowserFailure> {
        let resource = self.0.fetch_resource(tab, url).await.map_err(failure)?;
        Ok(FetchedResource {
            status: resource.status,
            content_type: resource.content_type,
            body: resource.body,
        })
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

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        Vec::new()
    }
}
