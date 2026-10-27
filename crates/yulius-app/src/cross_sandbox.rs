//! Tasks the Superagent starts in its person's other sandboxes (SPEC-335, ADR-0227):
//!
//! - `sandboxes_list` names the other live sandboxes the person can work in (XSR-001);
//! - `sandbox_task_create` asks the person: the request waits in memory, at most
//!   [`MAX_WAITING`] per person and sandbox, for at most [`CROSS_REQUEST_WAIT`], and ends with the
//!   call (answered, unanswered, or dropped) (XSR-002..004);
//! - only that person, from a browser session, approves or declines it; an approval creates the
//!   task as `task_create` would in the target, exactly as shown (XSR-005, XSR-006).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::{Channel, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;
use crate::manage::tool_detail;
use crate::model::SandboxRecord;

/// How long a request waits for its person: below the 300 s within which Node's HTTP client,
/// which MCP clients commonly run on, wants the answer's headers (ADR-0227).
pub const CROSS_REQUEST_WAIT: Duration = Duration::from_secs(240);
/// Requests that wait at once per person and Superagent sandbox (XSR-004).
pub const MAX_WAITING: usize = 3;
/// How long a waiting call still waits once its person answered (the task is being created).
/// Short enough that the wait and the grace stay below those 300 s.
const ANSWER_GRACE: Duration = Duration::from_secs(50);

/// What `sandbox_task_create` takes: the target and `task_create`'s arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "checked here, read by `tool_task_create`")]
struct CrossTaskArgs {
    sandbox: String,
    prompt: String,
    title: Option<String>,
    workflow: Option<String>,
    mode: Option<String>,
    after_task: Option<u64>,
    start_rule: Option<String>,
    supertask: Option<bool>,
    prepared: Option<bool>,
    start_at: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// The start options a request shows when set (XSR-005).
const OPTIONS: [&str; 9] = [
    "workflow",
    "mode",
    "after_task",
    "start_rule",
    "supertask",
    "prepared",
    "start_at",
    "model",
    "effort",
];

/// One request waiting for its person.
struct Waiting {
    user: UserId,
    source: SandboxId,
    target: SandboxRecord,
    /// `task_create`'s arguments, without `sandbox`.
    args: Value,
    actor: String,
    detail: Value,
    ends_at: Timestamp,
    /// The MCP call that waits for it (its Superagent's shell and JSON-RPC id), which a
    /// `notifications/cancelled` names.
    call: Option<String>,
    answer: oneshot::Sender<AppResult<Value>>,
}

/// The key of a Superagent's MCP call: its shell (from its caller's audit detail) and its
/// JSON-RPC id, as sent (`7` and `"7"` are two calls).
pub(crate) fn call_key(caller: &ProjectCaller, rpc_id: &Value) -> Option<String> {
    let shell = caller.detail.get("shell").and_then(Value::as_str)?;
    Some(format!("{shell}#{rpc_id}"))
}

/// The waiting requests by id (bounded by [`MAX_WAITING`] per person and sandbox).
#[derive(Default)]
pub struct CrossRequests(Mutex<HashMap<String, Waiting>>);

impl CrossRequests {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Waiting>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Removes its request when the call ends however it ends (a dropped call included, XSR-004).
struct Ends<'a> {
    app: &'a App,
    id: String,
    user: UserId,
    source: SandboxId,
}

impl Drop for Ends<'_> {
    fn drop(&mut self) {
        if self.app.cross_requests.lock().remove(&self.id).is_some() {
            self.app.announce_cross_requests(&self.user, &self.source);
        }
    }
}

/// Whether a person reading a request could not see `c`, or would see the text around it
/// reordered: control characters but line breaks and tabs, format characters (bidirectional
/// marks and overrides, zero-width ones, the soft hyphen, the Arabic, Syriac, Kaithi and
/// Egyptian format characters), Unicode tag characters (which carry
/// hidden ASCII), variation selectors, fillers that draw as blanks, private-use and noncharacters.
fn unseen(c: char) -> bool {
    if matches!(c, '\n' | '\t' | '\r') {
        return false;
    }
    matches!(u32::from(c),
        0x0000..=0x001F | 0x007F..=0x009F | 0x00AD | 0x034F | 0x0600..=0x0605 | 0x061C
        | 0x06DD | 0x070F | 0x0890 | 0x0891 | 0x08E2 | 0x115F | 0x1160 | 0x110BD | 0x110CD
        | 0x13430..=0x1343F
        | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x206F | 0x3164 | 0xE000..=0xF8FF | 0xFDD0..=0xFDEF | 0xFE00..=0xFE0F
        | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
        | 0xE0000..=0xE0FFF | 0xF0000..=0x10FFFF)
        || (u32::from(c) & 0xFFFE) == 0xFFFE
}

/// Refuses a request whose texts hold characters the person who approves it cannot see
/// (SPEC-335 XSR-008): what they read is all that reaches the task.
fn check_visible(args: &Value) -> AppResult<()> {
    let Some(map) = args.as_object() else {
        return Ok(());
    };
    for (key, value) in map {
        let Some(text) = value.as_str() else { continue };
        if let Some(c) = text.chars().find(|c| unseen(*c)) {
            return Err(AppError::Invalid(format!(
                "`{key}` holds U+{:04X}, a character the person cannot see: write it without invisible or direction characters",
                u32::from(c)
            )));
        }
    }
    Ok(())
}

/// How a waiting call ends.
#[derive(Debug)]
enum Settled {
    /// Its person answered (or the call was cancelled).
    Answered(AppResult<Value>),
    /// The wait ended and the request was withdrawn before anyone answered.
    Unanswered,
    /// It was being answered when the wait ended, and no outcome came within the grace.
    Lost,
}

/// Waits for the answer, at most `wait`; then `withdraw` takes the request back, unless its
/// person is answering it (it is no longer waiting), whose outcome is awaited for `grace` more
/// rather than said to be unanswered while the task is being created.
async fn settle(
    mut rx: oneshot::Receiver<AppResult<Value>>,
    wait: Duration,
    grace: Duration,
    withdraw: impl FnOnce() -> bool,
) -> Settled {
    match tokio::time::timeout(wait, &mut rx).await {
        Ok(Ok(answer)) => return Settled::Answered(answer),
        Ok(Err(_)) => return Settled::Lost,
        Err(_) if withdraw() => return Settled::Unanswered,
        Err(_) => {}
    }
    match tokio::time::timeout(grace, rx).await {
        Ok(Ok(answer)) => Settled::Answered(answer),
        _ => Settled::Lost,
    }
}

/// The person answered, but the outcome never came back to the call.
fn unreported(target: &SandboxRecord) -> AppError {
    AppError::Conflict(format!(
        "the person answered but the outcome did not come back: look at the Tasks page of {}",
        target.slug
    ))
}

fn label(sandbox: &SandboxRecord) -> String {
    if sandbox.is_playground() {
        "playground".to_owned()
    } else {
        format!("{}/{}", sandbox.repo_owner, sandbox.repo_name)
    }
}

impl App {
    /// `sandboxes_list` (XSR-001): the other live sandboxes the Superagent's person can work in.
    ///
    /// # Errors
    /// `Forbidden` for a disabled person; store failures.
    pub async fn tool_sandboxes_list(&self, caller: &ProjectCaller) -> AppResult<Value> {
        let user = self.manage_authority(caller).await?;
        let mut out = Vec::new();
        for (sandbox, level) in self.visible_sandboxes(&user).await? {
            if sandbox.id == caller.sandbox
                || sandbox.removed_at.is_some()
                || !level.allows(Action::Work)
            {
                continue;
            }
            out.push(json!({
                "sandbox": sandbox.slug,
                "repository": label(&sandbox),
                "onboarding": sandbox.onboarding_state,
            }));
        }
        Ok(json!({
            "sandboxes": out,
            "note": "Start a task in one with sandbox_task_create: its person approves each request.",
        }))
    }

    /// `sandbox_task_create` (XSR-002..004): asks the Superagent's person to start a task in
    /// another of their sandboxes and waits for the answer.
    ///
    /// # Errors
    /// `Invalid` arguments or its own sandbox, `NotFound` for a sandbox the person cannot work
    /// in, `Capacity` beyond [`MAX_WAITING`], and on approval the errors of `task_create`.
    pub async fn tool_sandbox_task_create(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        self.cross_task_request(caller, args, None).await
    }

    /// [`App::tool_sandbox_task_create`] for the MCP call `call` ([`call_key`]), which its
    /// client's `notifications/cancelled` ends ([`App::cancel_cross_request`]).
    pub(crate) async fn cross_task_request(
        &self,
        caller: &ProjectCaller,
        args: &Value,
        call: Option<String>,
    ) -> AppResult<Value> {
        if caller.channel != Channel::Superagent || caller.task.is_some() {
            return Err(AppError::Forbidden(
                "only a Superagent asks for this".into(),
            ));
        }
        let parsed: CrossTaskArgs = serde_json::from_value(args.clone())
            .map_err(|err| AppError::Invalid(format!("invalid arguments: {err}")))?;
        if parsed.prompt.trim().is_empty() {
            return Err(AppError::Invalid("the prompt is empty".into()));
        }
        check_visible(args)?;
        if parsed.prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::Invalid(format!(
                "the prompt is longer than {MAX_PROMPT_BYTES} bytes"
            )));
        }
        let target = self.sandbox_by_reference(&parsed.sandbox).await?;
        if target.id == caller.sandbox {
            return Err(AppError::Invalid(
                "that is this sandbox: create the task with task_create".into(),
            ));
        }
        let user = self.manage_authority(caller).await?;
        // A sandbox the person only reads is as unknown as one they cannot see.
        self.require(&user, &target.id, Action::Work)
            .await
            .map_err(|_| AppError::NotFound)?;
        let mut task_args = args.clone();
        if let Some(map) = task_args.as_object_mut() {
            map.remove("sandbox");
        }
        let id = self.deps.crypto.new_id();
        let wait = Duration::from_millis(self.config.cross_request_wait_ms);
        let (tx, rx) = oneshot::channel();
        {
            let mut waiting = self.cross_requests.lock();
            let mine = waiting
                .values()
                .filter(|w| w.user == user.id && w.source == caller.sandbox)
                .count();
            if mine >= MAX_WAITING {
                return Err(AppError::Capacity(format!(
                    "{MAX_WAITING} requests already wait for the person's answer: wait for them"
                )));
            }
            waiting.insert(
                id.clone(),
                Waiting {
                    user: user.id.clone(),
                    source: caller.sandbox.clone(),
                    target: target.clone(),
                    args: task_args,
                    actor: caller.actor.clone(),
                    detail: caller.detail.clone(),
                    ends_at: self
                        .now()
                        .plus_millis(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)),
                    call,
                    answer: tx,
                },
            );
        }
        let _ends = Ends {
            app: self,
            id: id.clone(),
            user: user.id.clone(),
            source: caller.sandbox.clone(),
        };
        self.announce_cross_requests(&user.id, &caller.sandbox);
        let withdraw = || self.cross_requests.lock().remove(&id).is_some();
        match settle(rx, wait, ANSWER_GRACE, withdraw).await {
            Settled::Answered(answer) => answer,
            Settled::Unanswered => {
                self.announce_cross_requests(&user.id, &caller.sandbox);
                self.audit_cross(caller, &target, "expired").await;
                Ok(json!({
                    "outcome": "unanswered",
                    "note": "Nobody answered in time: nothing was created. Ask again when the person is here.",
                }))
            }
            Settled::Lost => Err(unreported(&target)),
        }
    }

    /// `GET /api/sandboxes/{sid}/superagent/requests` (XSR-005): the caller's own requests of
    /// their Superagent in `sid`, oldest first.
    #[must_use]
    pub fn superagent_requests(&self, ctx: &AuthContext, sid: &str) -> Vec<Value> {
        let waiting = self.cross_requests.lock();
        let mut mine: Vec<(&String, &Waiting)> = waiting
            .iter()
            .filter(|(_, w)| w.user == ctx.user.id && w.source.as_str() == sid)
            .collect();
        mine.sort_by_key(|(id, w)| (w.ends_at, (*id).clone()));
        mine.into_iter()
            .map(|(id, w)| {
                let options: serde_json::Map<String, Value> = OPTIONS
                    .iter()
                    .filter_map(|key| {
                        w.args
                            .get(*key)
                            .filter(|v| !v.is_null())
                            .map(|v| ((*key).to_owned(), v.clone()))
                    })
                    .collect();
                json!({
                    "id": id,
                    "sandbox": {"id": w.target.id.as_str(), "slug": w.target.slug, "repository": label(&w.target)},
                    "title": w.args.get("title"),
                    "prompt": w.args.get("prompt"),
                    "options": options,
                    "ends_at": rfc3339(w.ends_at),
                })
            })
            .collect()
    }

    /// `POST /api/sandboxes/{sid}/superagent/requests/{id}` (XSR-006): the person approves
    /// (the task is created in the target, as `task_create` would) or declines one request.
    ///
    /// # Errors
    /// `Forbidden` without a browser session, `NotFound` for a request that is not the
    /// caller's or no longer waits, and on approval the errors of `task_create`.
    pub async fn superagent_request_answer(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
        id: &str,
        approve: bool,
    ) -> AppResult<Value> {
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "only a person answers a Superagent's request, on the Yulius page".into(),
            ));
        }
        let request = {
            let mut waiting = self.cross_requests.lock();
            match waiting.get(id) {
                Some(w) if w.user == ctx.user.id && w.source.as_str() == sid => waiting.remove(id),
                _ => None,
            }
        }
        .ok_or(AppError::NotFound)?;
        // Taken, it is answered to the end even if this request is cancelled (the person's page
        // went away): the waiting call gets the outcome either way.
        let app = Arc::clone(self);
        tokio::spawn(async move { app.answer_taken(request, approve).await })
            .await
            .map_err(AppError::internal)?
    }

    async fn answer_taken(self: Arc<Self>, request: Waiting, approve: bool) -> AppResult<Value> {
        self.announce_cross_requests(&request.user, &request.source);
        let caller = ProjectCaller {
            sandbox: request.target.id.clone(),
            user: request.user.clone(),
            task: None,
            channel: Channel::Superagent,
            actor: request.actor.clone(),
            detail: request.detail.clone(),
        };
        let source = ProjectCaller {
            sandbox: request.source.clone(),
            ..caller.clone()
        };
        let (outcome, answer) = if approve {
            let created = self.tool_task_create(&caller, &request.args).await;
            let audited = if created.is_ok() {
                "approved"
            } else {
                "failed"
            };
            let outcome = created.map(|created| {
                json!({
                    "outcome": "approved",
                    "sandbox": request.target.slug,
                    "task": created["task"],
                    "note": format!("The person approved: the task is on the Tasks page of {}.", request.target.slug),
                })
            });
            (audited, outcome)
        } else {
            (
                "declined",
                Ok(json!({
                    "outcome": "declined",
                    "note": "The person declined: nothing was created. Do not ask again unless they ask.",
                })),
            )
        };
        self.audit_cross(&source, &request.target, outcome).await;
        // The call may have ended meanwhile; the person still gets the answer.
        let _ = request.answer.send(answer.clone());
        answer
    }

    /// A Superagent's client cancelled the MCP call `rpc_id` (Esc in Claude Code, a client
    /// timeout): its request ends at once and the call answers `cancelled` (XSR-004).
    pub(crate) async fn cancel_cross_request(&self, caller: &ProjectCaller, rpc_id: &Value) {
        let Some(key) = call_key(caller, rpc_id) else {
            return;
        };
        let request = {
            let mut waiting = self.cross_requests.lock();
            let id = waiting
                .iter()
                .find(|(_, w)| {
                    w.call.as_deref() == Some(key.as_str())
                        && w.user == caller.user
                        && w.source == caller.sandbox
                })
                .map(|(id, _)| id.clone());
            id.and_then(|id| waiting.remove(&id))
        };
        let Some(request) = request else {
            return;
        };
        self.announce_cross_requests(&request.user, &request.source);
        self.audit_cross(caller, &request.target, "cancelled").await;
        let _ = request.answer.send(Ok(json!({
            "outcome": "cancelled",
            "note": "The call was cancelled: nothing was created.",
        })));
    }

    fn announce_cross_requests(&self, user: &UserId, source: &SandboxId) {
        self.events.publish_to_user(
            user.clone(),
            UiEvent::SuperagentRequests {
                sandbox_id: source.as_str().to_owned(),
            },
        );
    }

    /// Audits an answer (XSR-009): never the prompt.
    async fn audit_cross(&self, caller: &ProjectCaller, target: &SandboxRecord, outcome: &str) {
        let detail = tool_detail(
            caller,
            &json!({"from_sandbox": caller.sandbox.as_str(), "outcome": outcome}),
        );
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &caller.actor,
                "superagent.cross_task",
                Some(target.id.as_str()),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, "cross-sandbox request audit failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tokio::sync::oneshot;

    use super::{Settled, settle};

    const LONG: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn an_answer_in_time_is_the_outcome() {
        let (tx, rx) = oneshot::channel();
        assert!(tx.send(Ok(json!({"outcome": "declined"}))).is_ok());
        let settled = settle(rx, LONG, LONG, || unreachable!("nothing to withdraw")).await;
        assert!(matches!(settled, Settled::Answered(Ok(v)) if v["outcome"] == "declined"));
    }

    #[tokio::test]
    async fn a_request_still_waiting_at_the_end_is_unanswered() {
        let (_tx, rx) = oneshot::channel();
        let settled = settle(rx, Duration::from_millis(1), LONG, || true).await;
        assert!(matches!(settled, Settled::Unanswered), "{settled:?}");
    }

    /// The wait ends while the person's approval creates the task: the call reports the
    /// approval, not "unanswered" (nothing created), which would be false.
    #[tokio::test]
    async fn a_wait_that_ends_during_an_approval_reports_the_approval() {
        let (tx, rx) = oneshot::channel();
        let (ended, wait_ended) = oneshot::channel::<()>();
        let approving = tokio::spawn(async move {
            // Answers only once the wait is over.
            let _ = wait_ended.await;
            let _ = tx.send(Ok(json!({"outcome": "approved"})));
        });
        let settled = settle(rx, Duration::from_millis(1), LONG, move || {
            let _ = ended.send(());
            false
        })
        .await;
        assert!(
            matches!(settled, Settled::Answered(Ok(ref v)) if v["outcome"] == "approved"),
            "{settled:?}"
        );
        assert!(approving.await.is_ok());
    }

    #[tokio::test]
    async fn an_answer_that_never_comes_back_is_lost() {
        let (tx, rx) = oneshot::channel::<crate::error::AppResult<serde_json::Value>>();
        drop(tx);
        let settled = settle(rx, LONG, LONG, || unreachable!()).await;
        assert!(matches!(settled, Settled::Lost), "{settled:?}");
        let (_tx, rx) = oneshot::channel();
        let settled = settle(
            rx,
            Duration::from_millis(1),
            Duration::from_millis(1),
            || false,
        )
        .await;
        assert!(matches!(settled, Settled::Lost), "{settled:?}");
    }
}
