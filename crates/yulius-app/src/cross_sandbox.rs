//! Tasks the Superagent starts in its person's other sandboxes (SPEC-335, ADR-0227):
//!
//! - `sandboxes_list` names the other live sandboxes the person can work in (XSR-001);
//! - `sandbox_task_create` asks the person: the request waits in memory, at most
//!   [`MAX_WAITING`] per person and sandbox, for at most [`CROSS_REQUEST_WAIT`], and ends with the
//!   call (answered, unanswered, or dropped) (XSR-002..004);
//! - only that person, from a browser session, approves or declines it; an approval creates the
//!   task as `task_create` would in the target, exactly as shown (XSR-005, XSR-006);
//! - the same requests start, re-chain, message and stop a task of another sandbox
//!   (`sandbox_task_update`, `sandbox_task_message`, `sandbox_task_stop`, SPEC-442 XST-002..004),
//!   running the same-sandbox tool in the target on approval; never a delete or an archive;
//! - `sandbox_task_get` only reads, without asking (XST-001);
//! - every answer is audited in both sandboxes (XST-005).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::{Channel, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;
use crate::manage::{TaskChange, tool_detail};
use crate::model::{SandboxRecord, TaskRecord};
use crate::start_change::StartFields;
use crate::task_message::MAX_MESSAGE_BYTES;

/// How long a request waits for its person: below the 300 s within which Node's HTTP client,
/// which MCP clients commonly run on, wants the answer's headers (ADR-0227).
pub const CROSS_REQUEST_WAIT: Duration = Duration::from_secs(240);
/// Requests that wait at once per person and Superagent sandbox (XSR-004).
pub const MAX_WAITING: usize = 3;
/// How long a waiting call still waits once its person answered (the task is being created).
/// Short enough that the wait and the grace stay below those 300 s.
pub const ANSWER_GRACE: Duration = Duration::from_secs(50);

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

/// What `sandbox_task_get` and `sandbox_task_stop` take: the sandbox and the task.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "checked here, read by `agent_task_ref`")]
struct CrossTaskRef {
    sandbox: String,
    task_id: Option<String>,
    number: Option<u64>,
}

/// What `sandbox_task_update` takes: the task and `task_update`'s start arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "checked here, read by `tool_task_update`")]
struct CrossUpdateArgs {
    sandbox: String,
    task_id: Option<String>,
    number: Option<u64>,
    after_task: Option<u64>,
    start_rule: Option<String>,
    supertask: Option<bool>,
    prepared: Option<bool>,
    start_at: Option<String>,
}

/// What `sandbox_task_message` takes: the task and the message.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "checked here, read by `tool_task_message`")]
struct CrossMessageArgs {
    sandbox: String,
    task_id: Option<String>,
    number: Option<u64>,
    text: String,
}

/// What a request asks for in another sandbox (SPEC-442 XST-004): there is no delete, archive
/// or restore, so none can be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAction {
    /// `sandbox_task_create`: a new task.
    Create,
    /// `sandbox_task_update`: how a task that has not started starts (or start it now).
    Update,
    /// `sandbox_task_message`: a message from the person.
    Message,
    /// `sandbox_task_stop`.
    Stop,
}

impl CrossAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Message => "message",
            Self::Stop => "stop",
        }
    }

    /// The tool that does it in the caller's own sandbox.
    fn own_tool(self) -> &'static str {
        match self {
            Self::Create => "task_create",
            Self::Update => "task_update",
            Self::Message => "task_message",
            Self::Stop => "task_stop",
        }
    }

    /// What did not happen when the request was not approved.
    fn nothing(self) -> &'static str {
        match self {
            Self::Create => "nothing was created",
            Self::Update => "the task was not changed",
            Self::Message => "nothing was sent",
            Self::Stop => "the task was not stopped",
        }
    }

    /// The start options its card shows when set (XSR-005).
    fn options(self) -> &'static [&'static str] {
        match self {
            Self::Create => &OPTIONS,
            Self::Update => &UPDATE_OPTIONS,
            Self::Message | Self::Stop => &[],
        }
    }
}

/// The start options `sandbox_task_update` shows when set (XST-006).
const UPDATE_OPTIONS: [&str; 5] = [
    "after_task",
    "start_rule",
    "supertask",
    "prepared",
    "start_at",
];

/// The task a request is about, as its card names it.
#[derive(Clone)]
struct Named {
    id: TaskId,
    number: u32,
    title: String,
    status: &'static str,
}

impl Named {
    fn of(task: &TaskRecord) -> Self {
        Self {
            id: task.id.clone(),
            number: task.number,
            title: task.title.clone(),
            status: task.status.as_str(),
        }
    }
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
    action: CrossAction,
    /// The task it is about (all but a create), named by its id in `args`.
    task: Option<Named>,
    /// An update that starts the task now (XST-006).
    starts_now: bool,
    /// The same-sandbox tool's arguments, without `sandbox`.
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

/// A request's arguments as its tool takes them.
fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> AppResult<T> {
    serde_json::from_value(args.clone())
        .map_err(|err| AppError::Invalid(format!("invalid arguments: {err}")))
}

/// The same-sandbox tool's arguments: a request's, without `sandbox`.
fn without_sandbox(args: &Value) -> Value {
    let mut args = args.clone();
    if let Some(map) = args.as_object_mut() {
        map.remove("sandbox");
    }
    args
}

/// The Superagent's caller, acting in `target` for the same person.
fn pinned(caller: &ProjectCaller, target: &SandboxRecord) -> ProjectCaller {
    ProjectCaller {
        sandbox: target.id.clone(),
        ..caller.clone()
    }
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
        self.cross_task_request(caller, CrossAction::Create, args, None)
            .await
    }

    /// `sandbox_task_create`, `sandbox_task_update`, `sandbox_task_message` or
    /// `sandbox_task_stop` (SPEC-442 XST-002, XST-003): asks the Superagent's person for `action`
    /// in another of their sandboxes and waits for the answer.
    ///
    /// # Errors
    /// As [`App::tool_sandbox_task_create`]; `NotFound` for a task not in that sandbox, and on
    /// approval the errors of the same-sandbox tool.
    pub async fn tool_sandbox_task_request(
        &self,
        caller: &ProjectCaller,
        action: CrossAction,
        args: &Value,
    ) -> AppResult<Value> {
        self.cross_task_request(caller, action, args, None).await
    }

    /// `sandbox_task_get` (SPEC-442 XST-001): `task_get` of a task in another sandbox its
    /// person can work in. It only reads: nobody is asked.
    ///
    /// # Errors
    /// `Invalid` arguments or its own sandbox, `NotFound` for a sandbox the person cannot work
    /// in or a task not in it.
    pub async fn tool_sandbox_task_get(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let parsed: CrossTaskRef = parse(args)?;
        let (_, target) = self
            .cross_target(caller, &parsed.sandbox, "task_get")
            .await?;
        let there = pinned(caller, &target);
        self.tool_task_get(&there, &without_sandbox(args)).await
    }

    /// The other sandbox `reference` names, once the Superagent's person may work in it, and
    /// that person.
    async fn cross_target(
        &self,
        caller: &ProjectCaller,
        reference: &str,
        own_tool: &str,
    ) -> AppResult<(crate::model::UserRecord, SandboxRecord)> {
        if caller.channel != Channel::Superagent || caller.task.is_some() {
            return Err(AppError::Forbidden(
                "only a Superagent asks for this".into(),
            ));
        }
        let target = self.sandbox_by_reference(reference).await?;
        if target.id == caller.sandbox {
            return Err(AppError::Invalid(format!(
                "that is this sandbox: use {own_tool}"
            )));
        }
        let user = self.manage_authority(caller).await?;
        // A sandbox the person only reads is as unknown as one they cannot see.
        self.require(&user, &target.id, Action::Work)
            .await
            .map_err(|_| AppError::NotFound)?;
        Ok((user, target))
    }

    /// [`App::tool_sandbox_task_request`] for the MCP call `call` ([`call_key`]), which its
    /// client's `notifications/cancelled` ends ([`App::cancel_cross_request`]).
    pub(crate) async fn cross_task_request(
        &self,
        caller: &ProjectCaller,
        action: CrossAction,
        args: &Value,
        call: Option<String>,
    ) -> AppResult<Value> {
        if caller.channel != Channel::Superagent || caller.task.is_some() {
            return Err(AppError::Forbidden(
                "only a Superagent asks for this".into(),
            ));
        }
        let reference = match action {
            CrossAction::Create => {
                let parsed: CrossTaskArgs = parse(args)?;
                if parsed.prompt.trim().is_empty() {
                    return Err(AppError::Invalid("the prompt is empty".into()));
                }
                if parsed.prompt.len() > MAX_PROMPT_BYTES {
                    return Err(AppError::Invalid(format!(
                        "the prompt is longer than {MAX_PROMPT_BYTES} bytes"
                    )));
                }
                parsed.sandbox
            }
            CrossAction::Update => parse::<CrossUpdateArgs>(args)?.sandbox,
            CrossAction::Message => {
                let parsed: CrossMessageArgs = parse(args)?;
                if parsed.text.trim().is_empty() {
                    return Err(AppError::Invalid("the message is empty".into()));
                }
                if parsed.text.len() > MAX_MESSAGE_BYTES {
                    return Err(AppError::Invalid(format!(
                        "the message is longer than {} KiB",
                        MAX_MESSAGE_BYTES / 1024
                    )));
                }
                parsed.sandbox
            }
            CrossAction::Stop => parse::<CrossTaskRef>(args)?.sandbox,
        };
        check_visible(args)?;
        let (user, target) = self
            .cross_target(caller, &reference, action.own_tool())
            .await?;
        let mut task_args = without_sandbox(args);
        let mut starts_now = false;
        let task = if action == CrossAction::Create {
            None
        } else {
            let there = pinned(caller, &target);
            let task = self.agent_task_ref(&there, &task_args).await?;
            // The approval acts on the task the card names, by its id.
            if let Some(map) = task_args.as_object_mut() {
                map.remove("number");
                map.insert("task_id".into(), json!(task.id.as_str()));
            }
            if action == CrossAction::Update {
                starts_now = self.checked_update(&there, &task, &mut task_args).await?;
            }
            Some(Named::of(&task))
        };
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
                    action,
                    task: task.clone(),
                    starts_now,
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
                self.audit_cross(caller, &target, action, task.as_ref(), "expired")
                    .await;
                Ok(json!({
                    "outcome": "unanswered",
                    "note": format!("Nobody answered in time: {}. Ask again when the person is here.", action.nothing()),
                }))
            }
            Settled::Lost => Err(unreported(&target)),
        }
    }

    /// `GET /api/sandboxes/{sid}/superagent/requests` (XSR-005): the caller's own requests of
    /// their Superagent in `sid`, oldest first; a request's task as it is now (XST-006).
    ///
    /// # Errors
    /// `Forbidden` for a read-only token (a Superagent is its person's work, SPEC-247 RMT-001);
    /// store failures.
    pub async fn superagent_requests(&self, ctx: &AuthContext, sid: &str) -> AppResult<Vec<Value>> {
        if ctx.read_only() {
            return Err(AppError::Forbidden(
                "this API token is read-only; make an operate token on Settings to do this".into(),
            ));
        }
        let listed: Vec<(Value, Option<Named>)> = {
            let waiting = self.cross_requests.lock();
            let mut mine: Vec<(&String, &Waiting)> = waiting
                .iter()
                .filter(|(_, w)| w.user == ctx.user.id && w.source.as_str() == sid)
                .collect();
            mine.sort_by_key(|(id, w)| (w.ends_at, (*id).clone()));
            mine.into_iter()
                .map(|(id, w)| {
                    let options: serde_json::Map<String, Value> = w
                        .action
                        .options()
                        .iter()
                        .filter_map(|key| {
                            w.args
                                .get(*key)
                                .filter(|v| !v.is_null())
                                .map(|v| ((*key).to_owned(), v.clone()))
                        })
                        .collect();
                    // A message's text is read as a prompt is (XST-006).
                    let text = match w.action {
                        CrossAction::Message => w.args.get("text"),
                        _ => w.args.get("prompt"),
                    };
                    let view = json!({
                        "id": id,
                        "action": w.action.as_str(),
                        "sandbox": {"id": w.target.id.as_str(), "slug": w.target.slug, "repository": label(&w.target)},
                        "task": null,
                        "starts_now": w.starts_now,
                        "title": w.args.get("title"),
                        "prompt": text,
                        "options": options,
                        "ends_at": rfc3339(w.ends_at),
                    });
                    (view, w.task.clone())
                })
                .collect()
        };
        let mut out = Vec::with_capacity(listed.len());
        for (mut view, named) in listed {
            if let Some(named) = named {
                // Up to 4 minutes may have passed: the card names the task as it is now.
                let now = self.deps.store.task(&named.id).await?;
                let (title, status) = now
                    .as_ref()
                    .map_or((named.title.as_str(), named.status), |t| {
                        (t.title.as_str(), t.status.as_str())
                    });
                view["task"] = json!({"number": named.number, "title": title, "status": status});
            }
            out.push(view);
        }
        Ok(out)
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
            let done = self.run_approved(&caller, &request).await;
            let audited = if done.is_ok() { "approved" } else { "failed" };
            (audited, done)
        } else {
            (
                "declined",
                Ok(json!({
                    "outcome": "declined",
                    "note": format!("The person declined: {}. Do not ask again unless they ask.", request.action.nothing()),
                })),
            )
        };
        self.audit_cross(
            &source,
            &request.target,
            request.action,
            request.task.as_ref(),
            outcome,
        )
        .await;
        // The call may have ended meanwhile; the person still gets the answer.
        let _ = request.answer.send(answer.clone());
        answer
    }

    /// An update's start arguments as `task_update` will read them, checked before anyone is
    /// asked (review round 1 of SPEC-442): a task that started is refused; `false` flags, nulls
    /// and blank texts are dropped and texts trimmed, so the card shows exactly what runs; New
    /// task's start checks run in the target. Answers whether it starts the task now.
    async fn checked_update(
        &self,
        there: &ProjectCaller,
        task: &TaskRecord,
        args: &mut Value,
    ) -> AppResult<bool> {
        if !matches!(task.status, TaskStatus::Chained | TaskStatus::Prepared) {
            return Err(AppError::Conflict(format!(
                "task #{} has started (it is {}): only a task waiting to start changes how it starts",
                task.number,
                task.status.as_str()
            )));
        }
        let Some(map) = args.as_object_mut() else {
            return Ok(true);
        };
        map.retain(|key, value| match (key.as_str(), &*value) {
            (_, Value::Null) | ("supertask" | "prepared", Value::Bool(false)) => false,
            ("start_rule" | "start_at", Value::String(text)) => !text.trim().is_empty(),
            _ => true,
        });
        for key in ["start_rule", "start_at"] {
            if let Some(Value::String(text)) = map.get_mut(key) {
                *text = text.trim().to_owned();
            }
        }
        let after = match map.get("after_task") {
            Some(number) => Some(self.after_task_arg(there, number).await?.id),
            None => None,
        };
        let text = |key: &str| map.get(key).and_then(Value::as_str);
        let flag = |key: &str| map.get(key) == Some(&Value::Bool(true));
        let checked = self
            .check_start_choice(
                &there.sandbox,
                StartFields {
                    after_task_id: after.as_ref().map(TaskId::as_str),
                    start_rule: text("start_rule"),
                    supertask: flag("supertask"),
                    prepared: flag("prepared"),
                    start_at: text("start_at"),
                },
            )
            .await?;
        Ok(checked.after.is_none() && checked.rule.is_none() && checked.prepared.is_none())
    }

    /// Runs an approved request's same-sandbox tool in its target as `caller` (pinned there),
    /// with exactly the arguments its card showed (XSR-006, XST-003).
    async fn run_approved(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        request: &Waiting,
    ) -> AppResult<Value> {
        let slug = &request.target.slug;
        let args = &request.args;
        let (done, note) = match request.action {
            CrossAction::Create => (
                self.tool_task_create(caller, args).await?,
                format!("The person approved: the task is on the Tasks page of {slug}."),
            ),
            CrossAction::Update => (
                self.tool_task_update(caller, args).await?,
                format!("The person approved: the task's start was changed in {slug}."),
            ),
            CrossAction::Message => {
                let sent = self.tool_task_message(caller, args).await?;
                let note = format!(
                    "The person approved: {}",
                    sent["note"].as_str().unwrap_or("the message went out.")
                );
                (sent, note)
            }
            CrossAction::Stop => (
                self.tool_task_change(caller, args, TaskChange::Stop)
                    .await?,
                format!("The person approved: the task was stopped in {slug}."),
            ),
        };
        let mut outcome = json!({
            "outcome": "approved",
            "sandbox": slug,
            "task": done["task"],
            "note": note,
        });
        if let Some(delivered) = done.get("delivered") {
            outcome["delivered"] = delivered.clone();
        }
        Ok(outcome)
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
        self.audit_cross(
            caller,
            &request.target,
            request.action,
            request.task.as_ref(),
            "cancelled",
        )
        .await;
        let _ = request.answer.send(Ok(json!({
            "outcome": "cancelled",
            "note": format!("The call was cancelled: {}.", request.action.nothing()),
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

    /// Audits an answer in both sandboxes (XSR-009, XST-005): on the target with the source,
    /// on the source with the target; never the prompt or the message.
    async fn audit_cross(
        &self,
        source: &ProjectCaller,
        target: &SandboxRecord,
        action: CrossAction,
        task: Option<&Named>,
        outcome: &str,
    ) {
        let mut about = json!({"action": action.as_str(), "outcome": outcome});
        if let Some(task) = task {
            about["task"] = json!(task.number);
        }
        let sides = [
            (target.id.as_str(), "from_sandbox", source.sandbox.as_str()),
            (source.sandbox.as_str(), "to_sandbox", target.id.as_str()),
        ];
        for (sandbox, side, other) in sides {
            let mut extra = about.clone();
            extra[side] = json!(other);
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    &source.actor,
                    "superagent.cross_task",
                    Some(sandbox),
                    Some(&tool_detail(source, &extra)),
                )
                .await
            {
                tracing::warn!(error = %err, "cross-sandbox request audit failed");
            }
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
