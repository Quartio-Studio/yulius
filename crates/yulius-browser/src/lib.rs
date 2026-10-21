//! Per-sandbox Chromium lifecycle, its virtual display and the Chrome DevTools Protocol client
//! (SPEC-002 ADR-09, ADR-15; SPEC-001 BRW-001..008, U-10).
//!
//! - [`BrowserManager`] lazily starts one Chromium per sandbox ([`BrowserManager::ensure`]) with
//!   `--remote-debugging-pipe` (DevTools on its fds 3 and 4) and a per-sandbox profile, waits
//!   for its first answer, restarts a crashed browser on next use, stops idle ones
//!   ([`BrowserManager::reap_idle`]) and kills and reaps the process group on
//!   [`BrowserManager::shutdown`]. With [`DisplayMode::Virtual`] each browser is headful on its
//!   own `Xvfb` ([`VirtualDisplay`]): users watch H.264 video ([`VideoHub`]) and their input is
//!   real (XTest); otherwise it runs `--headless=new` and users get JPEG screencast frames.
//! - [`BrowserInstance`] is the shared browser: tab list and events for viewers, video and
//!   screencast subscriptions, input dispatch, the MCP tool functions (`tool_*`), the
//!   user-action registry ([`BrowserInstance::request_user_action`]) and, in process only, its
//!   DevTools client ([`BrowserInstance::cdp`]).
//! - [`CdpClient`] is the bounded JSON-RPC client underneath, over the pipe or a WebSocket.
//! - [`normalize_url`] is the URL policy (http, https, about:blank only).
//!
//! Trust model: DevTools has no authentication, so the browser opens no DevTools socket at all
//! (ADR-0191). Only Yulius holds the pipe's other ends (close-on-exec), so no other process, of
//! this sandbox, another one or the host, can drive the browser or its logged-in sessions
//! through DevTools, whatever the containment. Agents use it through Yulius's browser tools.
//! The virtual display is not reachable from containment at all
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

pub use cdp::{
    CdpClient, CdpConfig, CdpEvent, FrameMetadata, FrameSink, PIPE_URL, ScreencastFrame,
};
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
pub use manager::{BrowserConfig, BrowserManager, IdleReaper};
pub use process::StopTimeouts;
pub use url_policy::{UrlDenyList, UrlRefused, normalize_url};
pub use video::{
    GOP_MAX_BYTES, GOP_MAX_FRAMES, IDLE_GRACE, JOIN_REPLAY_BYTES, KEYFRAME_SPACING, NetworkReport,
    RateControl, START_TIER, SUBSCRIBER_QUEUE, TIERS, Tier, VideoConfig, VideoFrame, VideoHub,
    VideoPacket, VideoSubscription, VideoUnavailable,
};
