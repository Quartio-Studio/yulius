//! The shared sandbox browser: viewer access for people and MCP tools for agents (SPEC-001
//! BRW-001..008, AC-BRW-1..3; SPEC-002 ADR-15, §7.4).
//!
//! One browser per sandbox ([`App::browser_profile_dir`] keeps its profile, so logins persist
//! per sandbox). Viewers need write access; agents reach the browser of their task's sandbox
//! through their task token, a person's Superagent its sandbox's and a Global Superagent the
//! named project's (SPEC-437). `browser_request_user_action` marks a task `needs_attention`,
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

use crate::attention::AttentionRecord;
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::mcp::{MAX_RESULT_BYTES, TaskToolResult};
use crate::ports::browser::{BrowserFailure, BrowserFailureKind, BrowserSession};
use crate::vault::SecretUser;

/// Base64 budget of a screenshot inside one MCP result (the rest is JSON framing and text).
pub const SCREENSHOT_RESULT_BUDGET: usize = MAX_RESULT_BYTES - 4 * 1024;

/// A viewer allowed to use a sandbox's browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserGrant {
    /// The sandbox.
    pub sandbox_id: SandboxId,
}

/// The browser tools that only read: a Global Superagent's calls of them are not audited
/// (SPEC-437); the harness's `COCKPIT_READ_ONLY_TOOLS` lets the Superagent run the same ones
/// without asking.
pub(crate) const READING_TOOLS: [&str; 3] =
    ["browser_tabs", "browser_snapshot", "browser_screenshot"];

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

/// Extracts the browser tab id from a Yulius browser tab link (`…/browser?tab=<id>`, SPEC-578).
pub(crate) fn browser_tab_from_link(link: &str) -> Option<String> {
    let (_, query) = link.split_once("/browser?")?;
    for pair in query.split('&') {
        if let Some(raw) = pair.strip_prefix("tab=") {
            let mut decoded = Vec::with_capacity(raw.len());
            let bytes = raw.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%'
                    && i + 2 < bytes.len()
                    && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
                    && let Ok(val) = u8::from_str_radix(hex, 16)
                {
                    decoded.push(val);
                    i += 3;
                } else {
                    decoded.push(bytes[i]);
                    i += 1;
                }
            }
            let text = String::from_utf8(decoded).ok()?;
            return TabId::parse(&text).ok().map(|id| id.to_string());
        }
    }
    None
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
        self.require_for(ctx, &sandbox_id, Action::Work).await?;
        Ok(BrowserGrant { sandbox_id })
    }

    /// Whether a viewer still has write access (re-checked periodically, ACC-008).
    pub async fn browser_still_allowed(&self, ctx: &AuthContext, grant: &BrowserGrant) -> bool {
        ctx.user.disabled_at.is_none()
            && self
                .require_for(ctx, &grant.sandbox_id, Action::Work)
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
        self.start_browser_session(sandbox, false).await
    }

    /// Restarts a viewer's sandbox Chrome using its existing write grant and profile.
    ///
    /// # Errors
    /// Access, removal, process shutdown and startup failures.
    pub async fn restart_browser(&self, ctx: &AuthContext, sid: &str) -> AppResult<()> {
        let grant = self.browser_grant(ctx, sid).await?;
        self.start_browser_session(&grant.sandbox_id, true).await?;
        Ok(())
    }

    async fn start_browser_session(
        &self,
        sandbox: &SandboxId,
        restart: bool,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        let removed =
            || BrowserFailure::new(BrowserFailureKind::NotFound, "the sandbox was removed");
        if self.sandbox_removed(sandbox).await {
            return Err(removed());
        }
        let profile = self.browser_profile_dir(sandbox);
        let session = if restart {
            self.deps.browser.restart(sandbox, &profile).await?
        } else {
            self.deps.browser.ensure(sandbox, &profile).await?
        };
        if self.sandbox_removed(sandbox).await {
            self.deps.browser.stop(sandbox).await;
            return Err(removed());
        }
        Ok(session)
    }

    /// The dev-server ports of `sandbox`, which the host relays from its loopback into the
    /// sandbox and which its browser therefore reaches there (ADR-0321); `None` when there is
    /// no such sandbox.
    ///
    /// # Errors
    /// The store could not be read.
    pub async fn sandbox_ports(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Option<std::ops::RangeInclusive<u16>>> {
        Ok(self
            .deps
            .store
            .sandbox(sandbox)
            .await?
            .filter(|record| record.removed_at.is_none())
            .map(|record| {
                let of = self.sandbox_ref(&record);
                of.port_first..=of.port_last
            }))
    }

    /// The live sandbox's range and the identity a browser proxy holds while it may use it.
    /// Register the identity before reading the row so removal either sees this lease or the
    /// reader sees the removal. Connections accepted by that proxy inherit the same lease.
    ///
    /// # Errors
    /// The store could not be read.
    pub async fn lease_sandbox_ports(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Option<(Arc<str>, std::ops::RangeInclusive<u16>)>> {
        let lease = {
            let mut leases = self
                .browser_port_leases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            leases.retain(|_, lease| lease.strong_count() > 0);
            match leases.get(sandbox).and_then(std::sync::Weak::upgrade) {
                Some(lease) => lease,
                None => {
                    let lease: Arc<str> = Arc::from(sandbox.as_str());
                    leases.insert(sandbox.clone(), Arc::downgrade(&lease));
                    lease
                }
            }
        };
        Ok(self
            .sandbox_ports(sandbox)
            .await?
            .map(|ports| (lease, ports)))
    }

    /// Number of sandbox identities retained by the browser proxy lease registry.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn browser_port_lease_entries(&self) -> usize {
        self.browser_port_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub(crate) fn browser_holds_ports(&self, sandbox: &SandboxId) -> bool {
        self.browser_port_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(sandbox)
            .is_some_and(|lease| lease.strong_count() > 0)
    }

    /// Whether `sandbox` reads as removed ([`reads_removed`]).
    pub(crate) async fn sandbox_removed(&self, sandbox: &SandboxId) -> bool {
        reads_removed(&self.deps.store.sandbox(sandbox).await)
    }

    async fn agent_browser(&self, user: SecretUser<'_>) -> AppResult<Arc<dyn BrowserSession>> {
        Ok(self.browser_session(user.sandbox()).await?)
    }

    /// The browser tool `name` for `user`, a task's agent or a Superagent; `None` when `name` is
    /// not one.
    pub(crate) async fn browser_tool(
        &self,
        user: SecretUser<'_>,
        name: &str,
        args: &Value,
    ) -> Option<TaskToolResult> {
        Some(TaskToolResult::Value(match name {
            "browser_tabs" => self.tool_browser_tabs(user, args).await,
            "browser_open" => self.tool_browser_open(user, args).await,
            "browser_navigate" => self.tool_browser_navigate(user, args).await,
            "browser_click" => self.tool_browser_click(user, args).await,
            "browser_type" => self.tool_browser_type(user, args).await,
            "browser_snapshot" => self.tool_browser_snapshot(user, args).await,
            // Returns a complete result with an image item (size-bounded by the tool itself).
            "browser_screenshot" => {
                return Some(TaskToolResult::Complete(
                    self.tool_browser_screenshot(user, args).await,
                ));
            }
            "browser_request_user_action" => {
                self.tool_browser_request_user_action(user, args).await
            }
            "browser_fill_secret" => self.tool_browser_fill_secret(user, args).await,
            _ => return None,
        }))
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
        // Yulius's own listeners are never offered as dev servers.
        let excluded = self.preview_excluded_ports();
        open.retain(|port| !excluded.contains(port));
        open.sort_unstable();
        Ok((record.slug, first, last, open))
    }

    /// `browser_tabs`.
    pub(crate) async fn tool_browser_tabs(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let _: TabsParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        to_value(&browser.tool_tabs())
    }

    /// `browser_open`.
    pub(crate) async fn tool_browser_open(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let params: OpenParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        let result = browser.tool_open(&params).await?;
        browser.agent_activity(&result.tab.id, None, "open", &user.label());
        to_value(&result)
    }

    /// `browser_navigate`.
    pub(crate) async fn tool_browser_navigate(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let params: NavigateParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        let result = browser.tool_navigate(&params).await?;
        browser.agent_activity(&params.tab_id, None, "navigate", &user.label());
        to_value(&result)
    }

    /// `browser_click`.
    pub(crate) async fn tool_browser_click(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let params: ClickParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        let result = browser.tool_click(&params).await?;
        browser.agent_activity(
            &params.tab_id,
            Some((result.x, result.y)),
            "click",
            &user.label(),
        );
        to_value(&result)
    }

    /// `browser_type`.
    pub(crate) async fn tool_browser_type(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let params: TypeParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        let result = browser.tool_type(&params).await?;
        browser.agent_activity(&params.tab_id, None, "type", &user.label());
        to_value(&result)
    }

    /// `browser_snapshot`.
    pub(crate) async fn tool_browser_snapshot(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let params: SnapshotParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        to_value(&browser.tool_snapshot(&params).await?)
    }

    /// `browser_screenshot` as a complete MCP `tools/call` result: an image content item that
    /// fits the 256 KiB result cap (PNG, or a JPEG of lower quality and scale), plus a text item
    /// with the scale. When nothing fits, an error text points to `browser_snapshot`.
    pub(crate) async fn tool_browser_screenshot(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> Value {
        match self.browser_screenshot(user, args).await {
            Ok(value) => value,
            Err(err) => text_result(err.public_message(), true),
        }
    }

    async fn browser_screenshot(&self, user: SecretUser<'_>, args: &Value) -> AppResult<Value> {
        let params: ScreenshotParams = parse_args(args)?;
        let browser = self.agent_browser(user).await?;
        // Pixels cannot be redacted: a page showing any agent secret is not captured (SPEC-450).
        let matcher = self
            .agent_secret_matcher(user.sandbox())
            .await
            .map_err(|err| {
                tracing::error!(error = %err, "agent secrets could not be loaded for a screenshot");
                AppError::Upstream(
                    "Yulius could not check the page for agent secrets, so no screenshot was taken; use browser_snapshot".into(),
                )
            })?;
        let shows_secret = |text: &str| {
            !yulius_domain::vault::redact::Matcher::find(matcher.as_ref(), text.as_bytes())
                .is_empty()
        };
        let vault: Option<&(dyn Fn(&str) -> bool + Send + Sync)> =
            matcher.needles().next().is_some().then_some(&shows_secret);
        let shot = match browser
            .tool_screenshot(&params, SCREENSHOT_RESULT_BUDGET, vault)
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

    /// `browser_request_user_action`: highlights the tab for viewers, marks a task
    /// `needs_attention` and persists the handoff on `tasks.needs_you` (SPEC-578) (a Superagent's
    /// person is in front of it: nothing is flagged, SPEC-437 SAB-005), publishes
    /// `browser.action_requested` and returns the link to print (AC-BRW-3).
    pub(crate) async fn tool_browser_request_user_action(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let params: RequestUserActionParams = parse_args(args)?;
        params
            .validate()
            .map_err(|err| AppError::Invalid(err.to_string()))?;
        let browser = self.agent_browser(user).await?;
        let task = user.task().map(|task| task.id.clone());
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(user.sandbox())
            .await?
            .ok_or(AppError::NotFound)?;
        let redacted_reason = self.redact_notice_text(&sandbox.id, &params.reason).await;
        let request =
            browser.request_user_action(&params.tab_id, &redacted_reason, task.as_ref())?;
        let link = self.browser_tab_link(&sandbox.slug, &params.tab_id);
        if let Some(task_id) = &task {
            // Supersede any earlier browser handoff this task left on another tab (SPEC-578).
            self.clear_task_browser_actions(&sandbox.id, task_id, Some(&params.tab_id))
                .await;
            let now = self.now();
            let happened = crate::attention::shown_line(
                &format!(
                    "The agent asked you to act in the sandbox browser: {}",
                    request.reason
                ),
                crate::needs_you::MAX_HAPPENED_CHARS,
            );
            let action = crate::attention::shown_line(
                &format!("Open the browser tab: {}", request.reason),
                crate::needs_you::MAX_ACTION_CHARS,
            )
            .unwrap_or_else(|| {
                "Open the browser tab and complete the requested action.".to_owned()
            });
            let record = crate::needs_you::NeedsYouRecord {
                action,
                happened,
                problem: None,
                decision: false,
                link: Some(link.clone()),
                at: now.0,
                cause: None,
                named: crate::needs_you::Named::default(),
            };
            store
                .set_task_needs_you(task_id, Some(&record), now)
                .await?;
            // The task says it too, with the tab (SPEC-472): keeps `activity = needs_attention`
            // while live; `tasks.needs_you` preserves it across activity changes, refresh, park,
            // resume, and server restart (SPEC-578).
            self.flag_attention(
                task_id,
                AttentionRecord::yulius(
                    &format!(
                        "The agent asks you to act in the sandbox's browser: {}",
                        request.reason
                    ),
                    "Open the browser tab and do what it asks.",
                )
                .on_page(&link),
            )
            .await?;
        }
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
            link,
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

    /// Active in-memory browser tab handoff for `task`, if the sandbox's browser is running and
    /// still marks a tab for it: `(tab_id, redacted_reason)` (SPEC-578).
    pub(crate) async fn browser_tab_request_for(
        &self,
        sandbox: &SandboxId,
        task: &yulius_domain::ids::TaskId,
    ) -> Option<(String, String)> {
        let session = self.deps.browser.running(sandbox).await?;
        for tab in session.tabs() {
            if let Some(req) = tab.action_requested
                && req.task_id.as_deref() == Some(task.as_str())
            {
                let reason = self.redact_notice_text(sandbox, &req.reason).await;
                return Some((tab.id.to_string(), reason));
            }
        }
        None
    }

    /// Clears every in-memory browser action request belonging to `task` in `sandbox` (except
    /// `except_tab`, when superseding with a newer tab) and broadcasts `browser.action_cleared`
    /// (SPEC-578). Returns how many tab requests were cleared.
    pub(crate) async fn clear_task_browser_actions(
        &self,
        sandbox: &SandboxId,
        task: &yulius_domain::ids::TaskId,
        except_tab: Option<&TabId>,
    ) -> usize {
        let mut cleared = 0;
        if let Some(session) = self.deps.browser.running(sandbox).await {
            for tab in session.tabs() {
                if except_tab.is_some_and(|keep| keep == &tab.id) {
                    continue;
                }
                if tab
                    .action_requested
                    .as_ref()
                    .and_then(|req| req.task_id.as_deref())
                    == Some(task.as_str())
                    && session.clear_action(&tab.id)
                {
                    cleared += 1;
                    self.events.publish(
                        Some(sandbox.clone()),
                        false,
                        UiEvent::BrowserActionCleared {
                            sandbox_id: sandbox.to_string(),
                            tab_id: tab.id.to_string(),
                            task_id: Some(task.to_string()),
                        },
                    );
                }
            }
        }
        if except_tab.is_none()
            && let Ok(Some((attn, _))) = self.deps.store.stored_task_attention(task).await
            && attn
                .link
                .as_deref()
                .and_then(browser_tab_from_link)
                .is_some()
        {
            if self
                .deps
                .store
                .clear_stored_task_attention(task)
                .await
                .unwrap_or(false)
            {
                cleared += 1;
            }
            if let Ok(Some(record)) = self.deps.store.task(task).await
                && record.status == yulius_domain::task::TaskStatus::Running
                && record.activity == Some(yulius_domain::task::Activity::NeedsAttention)
            {
                let _ = self
                    .deps
                    .store
                    .set_activity(
                        &record.id,
                        Some(yulius_domain::task::Activity::Waiting),
                        self.now(),
                    )
                    .await;
            }
        }
        cleared
    }

    /// Clears a tab's browser user-action request on behalf of a viewer with write access
    /// (SPEC-578) and reconciles any task pointing at that tab.
    pub async fn clear_browser_action_for_sandbox(&self, sandbox: &SandboxId, tab: &TabId) {
        let mut task_id = None;
        if let Some(session) = self.deps.browser.running(sandbox).await {
            for info in session.tabs() {
                if &info.id == tab {
                    task_id = info.action_requested.and_then(|req| req.task_id);
                    break;
                }
            }
            let _ = session.clear_action(tab);
        }
        self.on_browser_action_cleared_for(sandbox, tab, task_id.as_deref())
            .await;
    }

    /// Handles a tab's browser user-action request being cleared (tab closed or viewer cleared):
    /// resolves matching persisted `needs_you` and legacy browser `attention` rows and publishes
    /// `browser.action_cleared` + `task.updated` (SPEC-578).
    pub async fn on_browser_action_cleared(&self, sandbox: &SandboxId, tab: &TabId) {
        self.on_browser_action_cleared_for(sandbox, tab, None).await;
    }

    async fn on_browser_action_cleared_for(
        &self,
        sandbox: &SandboxId,
        tab: &TabId,
        known_task: Option<&str>,
    ) {
        let store = &self.deps.store;
        let now = self.now();
        let mut affected_task: Option<String> = known_task.map(str::to_owned);
        if let Ok(tasks) = store.needs_you_tasks(sandbox).await {
            for task_id in tasks {
                if let Ok(Some(req)) = store.task_needs_you(&task_id).await
                    && req
                        .link
                        .as_deref()
                        .and_then(browser_tab_from_link)
                        .as_deref()
                        == Some(tab.as_str())
                    && store
                        .end_task_needs_you_at(&task_id, req.at, now)
                        .await
                        .unwrap_or(false)
                {
                    affected_task = Some(task_id.to_string());
                    if let Ok(Some((attn, _))) = store.stored_task_attention(&task_id).await
                        && attn
                            .link
                            .as_deref()
                            .and_then(browser_tab_from_link)
                            .as_deref()
                            == Some(tab.as_str())
                    {
                        let _ = store.clear_stored_task_attention(&task_id).await;
                        if let Ok(Some(record)) = store.task(&task_id).await
                            && record.status == yulius_domain::task::TaskStatus::Running
                            && record.activity
                                == Some(yulius_domain::task::Activity::NeedsAttention)
                        {
                            let _ = self
                                .set_task_activity(&record, yulius_domain::task::Activity::Waiting)
                                .await;
                        }
                    }
                    self.publish_task(&task_id).await;
                }
            }
        }
        // Legacy rows where `tasks.needs_you` was never written (only `tasks.attention`).
        if let Ok(tasks) = store.tasks(sandbox, false).await {
            for record in tasks {
                if record.status.is_resting() {
                    continue;
                }
                if let Ok(Some((attn, _))) = store.stored_task_attention(&record.id).await
                    && attn
                        .link
                        .as_deref()
                        .and_then(browser_tab_from_link)
                        .as_deref()
                        == Some(tab.as_str())
                {
                    affected_task.get_or_insert_with(|| record.id.to_string());
                    let _ = store.clear_stored_task_attention(&record.id).await;
                    if record.status == yulius_domain::task::TaskStatus::Running
                        && record.activity == Some(yulius_domain::task::Activity::NeedsAttention)
                    {
                        let _ = self
                            .set_task_activity(&record, yulius_domain::task::Activity::Waiting)
                            .await;
                    } else {
                        self.publish_task(&record.id).await;
                    }
                }
            }
        }
        self.events.publish(
            Some(sandbox.clone()),
            false,
            UiEvent::BrowserActionCleared {
                sandbox_id: sandbox.to_string(),
                tab_id: tab.to_string(),
                task_id: affected_task,
            },
        );
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
