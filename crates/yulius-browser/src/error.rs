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
    /// Chromium did not answer on its DevTools pipe in time.
    #[error("Chromium did not answer on its DevTools pipe within {timeout:?}")]
    StartupTimeout {
        /// Applied startup timeout.
        timeout: Duration,
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
    /// A viewer's click or key on a virtual display while Chrome's own windows are not locked
    /// by the host's managed policy (ADR-0318): not delivered.
    #[error(
        "clicks and keys are off in this browser: the host's browser policy is not in force, so Chrome's own file dialogs and developer tools would reach the host's files. An administrator runs `sudo yulius host setup` on the host"
    )]
    InputLocked,
    /// The tab's page, read with the input lock held, is not one the secret may be typed into
    /// (the tab went elsewhere since the caller looked): nothing was typed (SPEC-483 E3).
    #[error(
        "nothing was typed: this tab shows {0} now, which is not a page this secret may be typed into"
    )]
    SecretPageRefused(String),
    /// The tab is on its way to another page (a link was clicked, a form sent) that the secret
    /// may not be typed into, and would arrive under the keys: nothing was typed (SPEC-483 E9).
    #[error(
        "nothing was typed: this tab is on its way to {0}, which is not a page this secret may be typed into"
    )]
    SecretPageLeaving(String),
    /// The field with the focus is in a frame of another site inside the tab's page, and that
    /// site is not one the secret may be typed into: nothing was typed (SPEC-483 E8).
    #[error(
        "nothing was typed: the focus is in a frame of {0} inside this page, which is not a site this secret may be typed into"
    )]
    SecretFrameRefused(String),
    /// The tab's document changed while a secret was typed, to a page it may not be typed
    /// into: part of the secret may be in that page.
    #[error(
        "the tab went to {0} while the secret was typed, so part of it may be in that page: tell the secret's person, who may want to replace it"
    )]
    SecretAstray(String),
    /// The page may show a secret (typed with [`crate::BrowserInstance::tool_type_secret`] or
    /// one Yulius hides), or cannot be read to tell, so no screenshot is taken (SPEC-450).
    #[error(
        "no screenshot: this tab's page may show an agent secret (it shows one, it is on a site a secret was typed into outside a password field, it frames a site a secret was typed into, or Yulius cannot read it); read it with browser_snapshot, which shows secrets as [secret:NAME]"
    )]
    SecretOnPage,
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
            Self::InputLocked => ViewerErrorCode::InputLocked,
            Self::InvalidInput(_)
            | Self::ToolParams(_)
            | Self::InvalidSelector(_)
            | Self::ElementNotFound
            | Self::ElementNotVisible
            | Self::SecretPageRefused(_)
            | Self::SecretPageLeaving(_)
            | Self::SecretFrameRefused(_)
            | Self::SecretAstray(_)
            | Self::SecretOnPage => ViewerErrorCode::Invalid,
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
