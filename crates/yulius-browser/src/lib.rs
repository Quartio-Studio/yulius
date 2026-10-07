//! Per-sandbox Chromium lifecycle and Chrome DevTools Protocol client (SPEC-002 ADR-09;
//! SPEC-001 BRW-001..004, U-10).
//!
//! - [`BrowserManager`] lazily starts one headless Chromium per sandbox
//!   ([`BrowserManager::ensure`]) with `--remote-debugging-port=0` on loopback and a
//!   per-sandbox profile, learns the port from `DevToolsActivePort`, restarts a crashed browser
//!   on next use, stops idle ones ([`BrowserManager::reap_idle`]) and kills and reaps the
//!   process group on [`BrowserManager::shutdown`].
//! - [`BrowserInstance`] is the shared browser: tab list and events for viewers, screencast
//!   frames ([`BrowserInstance::subscribe_frames`]), input dispatch, the MCP tool functions
//!   (`tool_*`), the user-action registry ([`BrowserInstance::request_user_action`]) and the
//!   raw DevTools endpoint ([`BrowserInstance::cdp_ws_url`]).
//! - [`CdpClient`] is the bounded JSON-RPC client underneath.
//! - [`normalize_url`] is the URL policy (http, https, about:blank only).
//!
//! Trust model: the DevTools endpoint has no authentication. It listens on 127.0.0.1, so any
//! local process that finds the port can control the browser, including its logged-in
//! sessions. Agents of the same sandbox are meant to (U-10); processes of other sandboxes
//! share the loopback network under ADR-03 containment and could too. Treat the endpoint as
//! sandbox-shared, not per-user.
//!
//! The `fake` feature adds [`fake`]: an in-process fake CDP server and a fake Chromium
//! launcher for tests.

mod cdp;
mod discovery;
mod error;
mod instance;
mod keys;
mod manager;
pub mod mirror;
mod process;
mod snapshot;
mod url_policy;

#[cfg(feature = "fake")]
pub mod fake;

pub use cdp::{CdpClient, CdpConfig, CdpEvent, FrameMetadata, FrameSink, ScreencastFrame};
pub use discovery::{
    CHROMIUM_FIXED_PATHS, CHROMIUM_NAMES, discover_chromium, discover_in, resolve_executable,
};
pub use error::{BrowserError, CdpError};
pub use instance::{
    BOUNDED_SCREENSHOT_STEPS, BoundedScreenshot, BrowserEvent, BrowserInstance, DomEvent,
    DomSubscription, FrameSubscription, InstanceConfig, KeyInput, MAX_RESOURCE_BYTES, MouseInput,
    Resource, ScreencastConfig, ScreenshotStep, VIEWPORT_MAX, VIEWPORT_MIN,
};
pub use manager::{BrowserConfig, BrowserManager, DEVTOOLS_PORT_FILE, IdleReaper};
pub use mirror::DomBatch;
pub use process::StopTimeouts;
pub use url_policy::{UrlRefused, normalize_url};
