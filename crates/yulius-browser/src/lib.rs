//! Per-sandbox Chromium lifecycle, its virtual display and the Chrome DevTools Protocol client
//! (SPEC-002 ADR-09, ADR-15; SPEC-001 BRW-001..008, U-10).
//!
//! - [`BrowserManager`] lazily starts one Chromium per sandbox ([`BrowserManager::ensure`]) with
//!   `--remote-debugging-port=0` on loopback and a per-sandbox profile, learns the port from
//!   `DevToolsActivePort`, restarts a crashed browser on next use, stops idle ones
//!   ([`BrowserManager::reap_idle`]) and kills and reaps the process group on
//!   [`BrowserManager::shutdown`]. With [`DisplayMode::Virtual`] each browser is headful on its
//!   own `Xvfb` ([`VirtualDisplay`]): users watch H.264 video ([`VideoHub`]) and their input is
//!   real (XTest); otherwise it runs `--headless=new` and users get JPEG screencast frames.
//! - [`BrowserInstance`] is the shared browser: tab list and events for viewers, video and
//!   screencast subscriptions, input dispatch, the MCP tool functions (`tool_*`), the
//!   user-action registry ([`BrowserInstance::request_user_action`]) and the raw DevTools
//!   endpoint ([`BrowserInstance::cdp_ws_url`]).
//! - [`CdpClient`] is the bounded JSON-RPC client underneath.
//! - [`normalize_url`] is the URL policy (http, https, about:blank only).
//!
//! Trust model: the DevTools endpoint has no authentication. It listens on 127.0.0.1, so any
//! local process that finds the port can control the browser, including its logged-in
//! sessions. Agents of the same sandbox are meant to (U-10); processes of other sandboxes
//! share the loopback network under ADR-03 containment and could too. Treat the endpoint as
//! sandbox-shared, not per-user. The virtual display is not reachable from containment at all
//! (no TCP or abstract socket, a private `/tmp`, a cookie outside the sandbox directories).
//!
//! The `fake` feature adds [`fake`]: an in-process fake CDP server, a fake Chromium launcher
//! and a fake display for tests.

mod cdp;
mod discovery;
mod display;
mod encoder;
mod error;
pub mod flv;
mod instance;
mod keys;
mod manager;
mod process;
mod snapshot;
mod url_policy;
mod video;
mod wheel;
mod xserver;

#[cfg(feature = "fake")]
pub mod fake;

pub use cdp::{CdpClient, CdpConfig, CdpEvent, FrameMetadata, FrameSink, ScreencastFrame};
pub use discovery::{
    CHROMIUM_FIXED_PATHS, CHROMIUM_NAMES, discover_chromium, discover_in, resolve_executable,
};
pub use display::{
    DisplayConfig, DisplayInput, DisplayMode, VirtualDisplay, XAUTHORITY_FILE,
    stop_orphaned_displays, xauthority_entry,
};
pub use error::{BrowserError, CdpError};
pub use instance::{
    BOUNDED_SCREENSHOT_STEPS, BoundedScreenshot, BrowserEvent, BrowserInstance, FrameSubscription,
    InstanceConfig, KeyInput, MouseInput, ScreencastConfig, ScreenshotStep, VIEWPORT_MAX,
    VIEWPORT_MIN,
};
pub use manager::{BrowserConfig, BrowserManager, DEVTOOLS_PORT_FILE, IdleReaper};
pub use process::StopTimeouts;
pub use url_policy::{UrlDenyList, UrlRefused, normalize_url};
pub use video::{
    GOP_MAX_BYTES, GOP_MAX_FRAMES, IDLE_GRACE, JOIN_REPLAY_BYTES, KEYFRAME_SPACING, NetworkReport,
    RateControl, START_TIER, SUBSCRIBER_QUEUE, TIERS, Tier, VideoConfig, VideoFrame, VideoHub,
    VideoPacket, VideoSubscription, VideoUnavailable,
};
