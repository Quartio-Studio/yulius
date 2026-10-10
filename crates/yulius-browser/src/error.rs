//! Typed failures of the CDP client and the browser adapter.

use std::time::Duration;

use yulius_contracts::browser::{TabId, ToolParamError, ViewerErrorCode};

use crate::url_policy::UrlRefused;

/// Chrome DevTools Protocol transport or method failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CdpError {
    /// The WebSocket handshake failed or timed out.
    #[error("could not connect to the DevTools endpoint: {0}")]
    Connect(String),
    /// No response within the call timeout; the pending entry was removed.
    #[error("DevTools method {method} timed out after {timeout:?}")]
    Timeout {
        /// Method name.
        method: String,
        /// Applied timeout.
        timeout: Duration,
    },
    /// The connection is closed (browser exited, crashed or was shut down).
    #[error("DevTools connection closed")]
    Closed,
    /// The bounded pending-request map is full.
    #[error("too many DevTools requests in flight (limit {0})")]
    Busy(usize),
    /// The browser answered with a JSON-RPC error.
    #[error("DevTools method {method} failed ({code}): {message}")]
    Protocol {
        /// Method name.
        method: String,
        /// JSON-RPC error code.
        code: i64,
        /// Error message from the browser.
        message: String,
    },
    /// The browser's answer lacks an expected field.
    #[error("unexpected DevTools response to {method}: {detail}")]
    Malformed {
        /// Method name.
        method: String,
        /// What was missing or wrong.
        detail: String,
    },
}

/// Failure of a browser lifecycle operation, viewer request or tool call.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// No Chromium executable was found; Chromium is an external prerequisite.
    #[error(
        "Chromium executable not found (looked for {searched}); install Chromium or Google Chrome or configure the executable path"
    )]
    ExecutableNotFound {
        /// Names and paths that were tried.
        searched: String,
    },
    /// The process could not be spawned.
    #[error("failed to start Chromium: {0}")]
    Spawn(#[source] std::io::Error),
    /// The profile directory could not be prepared.
    #[error("failed to prepare the browser profile directory: {0}")]
    Profile(#[source] std::io::Error),
    /// Chromium did not write a valid `DevToolsActivePort` in time.
    #[error("Chromium did not report a DevTools port within {timeout:?}{detail}")]
    StartupTimeout {
        /// Applied startup timeout.
        timeout: Duration,
        /// Last observed problem, if any (prefixed with `: `).
        detail: String,
    },
    /// Chromium exited before it was ready (e.g. profile locked, missing libraries).
    #[error("Chromium exited during startup ({status}); stderr tail: {stderr}")]
    ExitedEarly {
        /// Exit status text.
        status: String,
        /// Last bytes of stderr.
        stderr: String,
    },
    /// CDP failure.
    #[error(transparent)]
    Cdp(#[from] CdpError),
    /// The URL policy refused an address.
    #[error(transparent)]
    UrlRefused(#[from] UrlRefused),
    /// The tab does not exist (closed or never opened).
    #[error("unknown tab {0}")]
    TabNotFound(TabId),
    /// The per-browser tab bound was reached.
    #[error("tab limit of {0} reached; close a tab first")]
    TooManyTabs(usize),
    /// No element matches the selector.
    #[error("no element matches the selector")]
    ElementNotFound,
    /// The element exists but has an empty box (hidden).
    #[error("the element has no visible box")]
    ElementNotVisible,
    /// The selector is not valid CSS.
    #[error("invalid selector: {0}")]
    InvalidSelector(String),
    /// The browser reported a navigation error (e.g. `net::ERR_NAME_NOT_RESOLVED`).
    #[error("navigation failed: {0}")]
    NavigationFailed(String),
    /// A tool result exceeded its bound.
    #[error("result exceeds the {limit}-byte bound")]
    ResultTooLarge {
        /// Bound in bytes.
        limit: usize,
    },
    /// A viewer or tool input is out of bounds.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Invalid tool parameters.
    #[error(transparent)]
    ToolParams(#[from] ToolParamError),
    /// The virtual display could not start or stopped (SPEC-002 ADR-15).
    #[error("virtual display: {0}")]
    Display(String),
    /// Discrete input (a click, a key, text) backed up because the display's X server stopped
    /// taking input; it was not delivered. Moves never do (they are coalesced).
    #[error("the sandbox browser is not taking input right now; that input was not delivered")]
    InputBacklog,
    /// The process could not be reaped after SIGKILL; it may still be running.
    #[error("Chromium (pid {pid}) did not exit after SIGKILL")]
    NotReaped {
        /// Process id.
        pid: u32,
    },
}

impl BrowserError {
    /// The viewer WebSocket error code for this failure.
    #[must_use]
    pub fn viewer_code(&self) -> ViewerErrorCode {
        match self {
            Self::UrlRefused(_) => ViewerErrorCode::UrlRefused,
            Self::TabNotFound(_) => ViewerErrorCode::NotFound,
            Self::TooManyTabs(_) | Self::InputBacklog => ViewerErrorCode::Capacity,
            Self::InvalidInput(_)
            | Self::ToolParams(_)
            | Self::InvalidSelector(_)
            | Self::ElementNotFound
            | Self::ElementNotVisible => ViewerErrorCode::Invalid,
            Self::Cdp(_)
            | Self::NavigationFailed(_)
            | Self::ResultTooLarge { .. }
            | Self::ExecutableNotFound { .. }
            | Self::Spawn(_)
            | Self::StartupTimeout { .. }
            | Self::ExitedEarly { .. }
            | Self::Display(_) => ViewerErrorCode::Upstream,
            Self::Profile(_) | Self::NotReaped { .. } => ViewerErrorCode::Internal,
        }
    }
}
