//! The shared sandbox browser: viewer access for people and MCP tools for agents (SPEC-001
//! BRW-001..008, AC-BRW-1..3; SPEC-002 ADR-15, §7.4).
//!
//! One browser per sandbox ([`App::browser_profile_dir`] keeps its profile, so logins persist
//! per sandbox). Viewers need write access; agents reach the browser of their task's sandbox
//! through their task token. `browser_request_user_action` marks the task `needs_attention`,
//! publishes `browser.action_requested` and returns a link that opens the browser panel on the
//! tab.

use std::path::PathBuf;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use yulius_contracts::browser::{
    ClickParams, NavigateParams, OpenParams, RequestUserActionParams, RequestUserActionResult,
    ScreenshotParams, SnapshotParams, TabId, TabsParams, TypeParams, tool_specs,
};
use yulius_contracts::events::UiEvent;
use yulius_contracts::mcp::{ToolDescriptor, text_result};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::task::Activity;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::mcp::MAX_RESULT_BYTES;
use crate::ports::browser::{BrowserFailure, BrowserFailureKind, BrowserSession};

/// Base64 budget of a screenshot inside one MCP result (the rest is JSON framing and text).
pub const SCREENSHOT_RESULT_BUDGET: usize = MAX_RESULT_BYTES - 4 * 1024;

/// A viewer allowed to use a sandbox's browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserGrant {
    /// The sandbox.
    pub sandbox_id: SandboxId,
}

/// The browser tool descriptors served by `tools/list` (SPEC-002 §7.4).
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    tool_specs()
        .into_iter()
        .map(|spec| ToolDescriptor {
            name: spec.name.to_owned(),
            description: spec.description.to_owned(),
            input_schema: spec.input_schema,
        })
        .collect()
}

/// Percent-encodes everything except RFC 3986 unreserved characters (and `/` when
/// `keep_slash`), for query values built by the server.
pub(crate) fn percent_encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if unreserved || (keep_slash && byte == b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn parse_args<T: DeserializeOwned>(args: &Value) -> AppResult<T> {
    let args = if args.is_null() {
        json!({})
    } else {
        args.clone()
    };
    serde_json::from_value(args)
        .map_err(|err| AppError::Invalid(format!("invalid arguments: {err}")))
}

fn to_value<T: serde::Serialize>(value: &T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(AppError::internal)
}

impl App {
    /// `<data>/sandboxes/<sandbox>/browser`: the sandbox's browser profile (BRW-002).
    #[must_use]
    pub fn browser_profile_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("browser")
    }

    /// Link that opens the browser panel of a sandbox on a tab (BRW-004; the web UI route
    /// `/s/<slug>/browser?tab=<id>`).
    #[must_use]
    pub fn browser_tab_link(&self, slug: &str, tab: &TabId) -> String {
        format!(
            "{}/s/{}/browser?tab={}",
            self.config.public_url,
            percent_encode(slug, false),
            percent_encode(tab.as_str(), false)
        )
    }

    /// Authorizes a browser viewer: write access to the sandbox (BRW-002, ACC-007).
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes, `Forbidden` for read-only users.
    pub async fn browser_grant(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<BrowserGrant> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        Ok(BrowserGrant { sandbox_id })
    }

    /// Whether a viewer still has write access (re-checked periodically, ACC-008).
    pub async fn browser_still_allowed(&self, ctx: &AuthContext, grant: &BrowserGrant) -> bool {
        ctx.user.disabled_at.is_none()
            && self
                .require(&ctx.user, &grant.sandbox_id, Action::Work)
                .await
                .is_ok()
    }

    /// The sandbox's browser, started on first use; never for a removed sandbox (SPEC-206): its
    /// removal is checked again once the browser runs, so a start racing the removal's own stop
    /// is stopped too.
    ///
    /// # Errors
    /// [`BrowserFailure`] when Chromium is missing or fails to start, or the sandbox was removed.
    pub async fn browser_session(
        &self,
        sandbox: &SandboxId,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        let removed =
            || BrowserFailure::new(BrowserFailureKind::NotFound, "the sandbox was removed");
        if self.sandbox_removed(sandbox).await {
            return Err(removed());
        }
        let session = self
            .deps
            .browser
            .ensure(sandbox, &self.browser_profile_dir(sandbox))
            .await?;
        if self.sandbox_removed(sandbox).await {
            self.deps.browser.stop(sandbox).await;
            return Err(removed());
        }
        Ok(session)
    }

    /// Whether `sandbox` reads as removed ([`reads_removed`]).
    pub(crate) async fn sandbox_removed(&self, sandbox: &SandboxId) -> bool {
        reads_removed(&self.deps.store.sandbox(sandbox).await)
    }

    async fn agent_browser(&self, ctx: &AgentContext) -> AppResult<Arc<dyn BrowserSession>> {
        Ok(self.browser_session(&ctx.task.sandbox_id).await?)
    }

    /// The label viewers see on the agent's cursor (BRW-007).
    fn agent_label(ctx: &AgentContext) -> String {
        format!("#{} {}", ctx.task.number, ctx.task.harness.display_name())
    }

    /// The sandbox's listening dev-server ports (its assigned port range, probed on loopback).
    ///
    /// # Errors
    /// `NotFound` for unknown sandboxes.
    pub async fn sandbox_listening_ports(
        &self,
        sandbox: &str,
    ) -> AppResult<(String, u16, u16, Vec<u16>)> {
        let id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let record = self
            .deps
            .store
            .sandbox(&id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        let first = record.port_base;
        let last = first.saturating_add(record.port_count.saturating_sub(1));
        let mut probes = tokio::task::JoinSet::new();
        for port in first..=last {
            probes.spawn(async move {
                let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
                matches!(
                    tokio::time::timeout(std::time::Duration::from_millis(300), connect).await,
                    Ok(Ok(_))
                )
                .then_some(port)
            });
        }
        let mut open: Vec<u16> = probes.join_all().await.into_iter().flatten().collect();
        // Yulius's own listeners and DevTools endpoints are never offered as dev servers.
        let excluded = self.preview_excluded_ports().await;
        open.retain(|port| !excluded.contains(port));
        open.sort_unstable();
        Ok((record.slug, first, last, open))
    }

    /// `browser_tabs`.
    pub(crate) async fn tool_browser_tabs(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let _: TabsParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        to_value(&browser.tool_tabs())
    }

    /// `browser_open`.
    pub(crate) async fn tool_browser_open(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let params: OpenParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        let result = browser.tool_open(&params).await?;
        browser.agent_activity(&result.tab.id, None, "open", &Self::agent_label(ctx));
        to_value(&result)
    }

    /// `browser_navigate`.
    pub(crate) async fn tool_browser_navigate(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let params: NavigateParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        let result = browser.tool_navigate(&params).await?;
        browser.agent_activity(&params.tab_id, None, "navigate", &Self::agent_label(ctx));
        to_value(&result)
    }

    /// `browser_click`.
    pub(crate) async fn tool_browser_click(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let params: ClickParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        let result = browser.tool_click(&params).await?;
        browser.agent_activity(
            &params.tab_id,
            Some((result.x, result.y)),
            "click",
            &Self::agent_label(ctx),
        );
        to_value(&result)
    }

    /// `browser_type`.
    pub(crate) async fn tool_browser_type(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let params: TypeParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        let result = browser.tool_type(&params).await?;
        browser.agent_activity(&params.tab_id, None, "type", &Self::agent_label(ctx));
        to_value(&result)
    }

    /// `browser_snapshot`.
    pub(crate) async fn tool_browser_snapshot(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let params: SnapshotParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        to_value(&browser.tool_snapshot(&params).await?)
    }

    /// `browser_screenshot` as a complete MCP `tools/call` result: an image content item that
    /// fits the 256 KiB result cap (PNG, or a JPEG of lower quality and scale), plus a text item
    /// with the scale. When nothing fits, an error text points to `browser_snapshot`.
    pub(crate) async fn tool_browser_screenshot(&self, ctx: &AgentContext, args: &Value) -> Value {
        match self.browser_screenshot(ctx, args).await {
            Ok(value) => value,
            Err(err) => text_result(err.public_message(), true),
        }
    }

    async fn browser_screenshot(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let params: ScreenshotParams = parse_args(args)?;
        let browser = self.agent_browser(ctx).await?;
        let shot = match browser
            .tool_screenshot(&params, SCREENSHOT_RESULT_BUDGET)
            .await
        {
            Ok(shot) => shot,
            Err(failure) if failure.kind == BrowserFailureKind::TooLarge => {
                return Err(AppError::Upstream(format!(
                    "the screenshot does not fit the {} KiB tool result limit even as a small JPEG; use browser_snapshot to read the page",
                    MAX_RESULT_BYTES / 1024
                )));
            }
            Err(failure) => return Err(failure.into()),
        };
        let encoding = match shot.quality {
            None => "PNG".to_owned(),
            Some(quality) => format!("JPEG quality {quality}"),
        };
        let note = format!(
            "Screenshot of tab {} ({encoding}, scale {} of the {}x{} viewport). Click coordinates are viewport CSS pixels: divide image pixels by the scale.",
            params.tab_id, shot.scale, shot.viewport.0, shot.viewport.1
        );
        Ok(json!({
            "content": [
                {"type": "image", "data": shot.data_base64, "mimeType": shot.mime_type},
                {"type": "text", "text": note},
            ],
            "isError": false,
        }))
    }

    /// `browser_request_user_action`: highlights the tab for viewers, marks the task
    /// `needs_attention`, publishes `browser.action_requested` and returns the link to print
    /// (AC-BRW-3).
    pub(crate) async fn tool_browser_request_user_action(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let params: RequestUserActionParams = parse_args(args)?;
        params
            .validate()
            .map_err(|err| AppError::Invalid(err.to_string()))?;
        let browser = self.agent_browser(ctx).await?;
        let request =
            browser.request_user_action(&params.tab_id, &params.reason, Some(&ctx.task.id))?;
        let store = &self.deps.store;
        let task = store.task(&ctx.task.id).await?.ok_or(AppError::NotFound)?;
        self.set_task_activity(&task, Activity::NeedsAttention)
            .await?;
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.events.publish(
            Some(sandbox.id.clone()),
            false,
            UiEvent::BrowserActionRequested {
                sandbox_id: sandbox.id.to_string(),
                tab_id: params.tab_id.to_string(),
                reason: request.reason.clone(),
                task_id: request.task_id.clone(),
            },
        );
        let result = RequestUserActionResult {
            link: self.browser_tab_link(&sandbox.slug, &params.tab_id),
            tab_id: params.tab_id,
        };
        let mut value = to_value(&result)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "note".into(),
                Value::String(
                    "The user was notified. Print the link in the session so they can open the tab, then wait for them."
                        .into(),
                ),
            );
        }
        Ok(value)
    }
}

/// Whether a read of a sandbox says it was removed (or is unknown). A failed read says nothing:
/// a store error never stops a live sandbox's browser or ends its shell (SPEC-206).
fn reads_removed(read: &AppResult<Option<crate::model::SandboxRecord>>) -> bool {
    match read {
        Ok(Some(record)) => record.removed_at.is_some(),
        Ok(None) => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-206 (review of #193, round 2): only a positive read means removed; a store error
    /// never stops a live sandbox's browser or ends its shell.
    #[test]
    fn only_a_read_that_says_so_means_removed() {
        assert!(!reads_removed(&Err(AppError::Internal(
            "database is locked".into()
        ))));
        assert!(reads_removed(&Ok(None)), "unknown: nothing runs for it");
    }

    #[test]
    fn percent_encoding_keeps_only_unreserved_characters() {
        assert_eq!(percent_encode("AB-12_x.~", false), "AB-12_x.~");
        assert_eq!(percent_encode("a b&c/d", false), "a%20b%26c%2Fd");
        assert_eq!(percent_encode("/srv/a b", true), "/srv/a%20b");
        assert_eq!(percent_encode("é", false), "%C3%A9");
    }

    #[test]
    fn browser_tools_are_the_eight_spec_tools() {
        let names: Vec<String> = tool_descriptors().into_iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            [
                "browser_tabs",
                "browser_open",
                "browser_navigate",
                "browser_click",
                "browser_type",
                "browser_snapshot",
                "browser_screenshot",
                "browser_request_user_action"
            ]
        );
    }
}
