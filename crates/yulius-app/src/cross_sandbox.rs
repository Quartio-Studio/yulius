//! Tasks the Superagent starts in its person's other sandboxes (SPEC-335, ADR-0227):
//!
//! - `sandboxes_list` names the other live sandboxes the person can work in (XSR-001);
//! - `sandbox_task_create` asks the person: the request waits on the panel, at most
//!   [`MAX_WAITING`] per person and sandbox; the call waits for the answer at most
//!   [`CROSS_REQUEST_WAIT`], and the request stays after it until the person answers it, the
//!   Superagent withdraws it or its client cancels the call (SPEC-487 XRQ-001); requests and
//!   their outcomes are kept in [`SUPERAGENT_REQUESTS_FILE`], so a restart keeps them
//!   (XSR-002..004, XRQ-002); a waiting request is amended in place (XRQ-003);
//! - `person_ask` leaves a decision of the Superagent's own on the same panel (XRQ-005);
//! - only that person, from a browser session, approves or declines it; an approval creates the
//!   task as `task_create` would in the target, exactly as shown (XSR-005, XSR-006);
//! - the same requests start, re-chain, message and stop a task of another sandbox
//!   (`sandbox_task_update`, `sandbox_task_message`, `sandbox_task_stop`, SPEC-442 XST-002..004),
//!   running the same-sandbox tool in the target on approval; never a delete or an archive;
//! - `sandbox_task_get` only reads, without asking (XST-001);
//! - every answer is audited in both sandboxes (XST-005).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
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

/// The file beside the database that keeps the waiting requests and the recent outcomes
/// (SPEC-487 XRQ-002), written whole at every change.
pub const SUPERAGENT_REQUESTS_FILE: &str = "superagent-requests.json";
/// The outcomes kept for the Superagents to read (`sandbox_requests`), the newest.
pub const MAX_ANSWERED: usize = 200;
/// The longest question of `person_ask`, in bytes.
pub const MAX_QUESTION_BYTES: usize = 4096;

/// What `sandbox_task_create` takes: the target and `task_create`'s arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code, reason = "checked here, read by `tool_task_create`")]
struct CrossTaskArgs {
    sandbox: String,
    prompt: String,
    /// The task's agent (SPEC-505, SPEC-520 RHM-004); the default is Claude Code when no model implies one (SPEC-620).
    harness: Option<String>,
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

/// What `person_ask` takes: the question for the person.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    question: String,
}

/// What `sandbox_request_amend` and `sandbox_request_withdraw` name: a waiting request.
#[derive(Deserialize)]
struct RequestRef {
    request: String,
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
    /// `person_ask`: a decision the Superagent leaves for its person, in its own sandbox
    /// (SPEC-487 XRQ-005); approved is yes, declined is no, and nothing runs.
    Ask,
    /// `cd_deploy` of a production-stage environment (SPEC-487 REL-001): approved with the
    /// project's slug typed, it runs the CD page's deploy as the person.
    Deploy,
    /// `cd_rollback` of a production-stage environment, as the CD page's rollback.
    Rollback,
    /// `promote`: the CD page's promotion to the production branch.
    Promote,
    /// `onboarding_start`: the project page's Start onboarding (REL-005), approved by a click.
    Onboard,
    /// `secret_request_value`: a secret's value its person types masked on the panel, stored in
    /// the vault as theirs (SPEC-487 SEC-001); the request never holds it.
    SecretValue,
}

impl CrossAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Message => "message",
            Self::Stop => "stop",
            Self::Ask => "ask",
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
            Self::Promote => "promote",
            Self::Onboard => "onboard",
            Self::SecretValue => "secret",
        }
    }

    /// Asked of the Superagent's own sandbox, on its own panel (not another sandbox's).
    pub(crate) fn own_sandbox(self) -> bool {
        matches!(
            self,
            Self::Ask
                | Self::Deploy
                | Self::Rollback
                | Self::Promote
                | Self::Onboard
                | Self::SecretValue
        )
    }

    /// A production release: approved only with the project's slug typed (REL-001).
    pub(crate) fn typed(self) -> bool {
        matches!(self, Self::Deploy | Self::Rollback | Self::Promote)
    }

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "create" => Self::Create,
            "update" => Self::Update,
            "message" => Self::Message,
            "stop" => Self::Stop,
            "ask" => Self::Ask,
            "deploy" => Self::Deploy,
            "rollback" => Self::Rollback,
            "promote" => Self::Promote,
            "onboard" => Self::Onboard,
            "secret" => Self::SecretValue,
            _ => return None,
        })
    }

    /// The tool that does it in the caller's own sandbox.
    pub(crate) fn own_tool(self) -> &'static str {
        match self {
            Self::Create => "task_create",
            Self::Update => "task_update",
            Self::Message => "task_message",
            Self::Stop => "task_stop",
            Self::Ask => "person_ask",
            Self::Deploy => "cd_deploy",
            Self::Rollback => "cd_rollback",
            Self::Promote => "promote",
            Self::Onboard => "onboarding_start",
            Self::SecretValue => "secret_request_value",
        }
    }

    /// What did not happen when the request was not approved.
    fn nothing(self) -> &'static str {
        match self {
            Self::Create => "nothing was created",
            Self::Update => "the task was not changed",
            Self::Message => "nothing was sent",
            Self::Stop => "the task was not stopped",
            Self::Ask => "the person said no",
            Self::Deploy => "nothing was deployed",
            Self::Rollback => "nothing was rolled back",
            Self::Promote => "nothing was promoted",
            Self::Onboard => "onboarding did not start",
            Self::SecretValue => "no value was stored",
        }
    }

    /// The start options its card shows when set (XSR-005).
    fn options(self) -> &'static [&'static str] {
        match self {
            Self::Create => &OPTIONS,
            Self::Update => &UPDATE_OPTIONS,
            Self::Deploy => &["environment"],
            Self::Rollback => &["environment", "release", "part"],
            Self::Promote => &[],
            Self::SecretValue => &["scope"],
            Self::Message | Self::Stop | Self::Ask | Self::Onboard => &[],
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
#[derive(Clone, Serialize, Deserialize)]
struct Named {
    id: String,
    number: u32,
    title: String,
    status: String,
}

impl Named {
    fn of(task: &TaskRecord) -> Self {
        Self {
            id: task.id.as_str().to_owned(),
            number: task.number,
            title: task.title.clone(),
            status: task.status.as_str().to_owned(),
        }
    }
}

/// The start options a request shows when set (XSR-005).
const OPTIONS: [&str; 10] = [
    "harness",
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
    id: String,
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
    /// When it was asked: the panel lists requests in this order.
    asked_at: Timestamp,
    /// How many times its Superagent amended it: the person's answer names the revision its
    /// card showed (review of slice 8a, F1).
    revision: u32,
    /// The MCP call that waits for it (its Superagent's shell and JSON-RPC id), which a
    /// `notifications/cancelled` names.
    call: Option<String>,
    /// Where its answer goes while that call still waits; none once the call ended (the
    /// request stays, and `sandbox_requests` tells the outcome, SPEC-487 XRQ-001).
    answer: Option<oneshot::Sender<AppResult<Value>>>,
}

impl Waiting {
    /// Its call ended: no answer goes there, and a later `notifications/cancelled` naming that
    /// call (the id reused, a client started again) withdraws nothing (review of slice 8a, F2).
    fn stop_waiting(&mut self) {
        self.answer = None;
        self.call = None;
    }

    /// How it ended, kept for its Superagent (XRQ-004).
    fn ended(&self, outcome: Value, at: Timestamp) -> Answered {
        Answered {
            id: self.id.clone(),
            user: self.user.as_str().to_owned(),
            source: self.source.as_str().to_owned(),
            target: self.target.slug.clone(),
            action: self.action.as_str().to_owned(),
            task: self.task.as_ref().map(|task| task.number),
            outcome,
            at: at.0,
        }
    }
}

/// How a request ended, for its Superagent to read later (SPEC-487 XRQ-004).
#[derive(Clone, Serialize, Deserialize)]
struct Answered {
    id: String,
    user: String,
    source: String,
    target: String,
    action: String,
    task: Option<u32>,
    outcome: Value,
    at: i64,
}

/// A waiting request as [`SUPERAGENT_REQUESTS_FILE`] keeps it (no answer channel: a restart
/// ended every call).
#[derive(Serialize, Deserialize)]
struct SavedRequest {
    id: String,
    user: String,
    source: String,
    target: String,
    action: String,
    task: Option<Named>,
    starts_now: bool,
    args: Value,
    actor: String,
    detail: Value,
    asked_at: i64,
    #[serde(default)]
    revision: u32,
}

/// [`SUPERAGENT_REQUESTS_FILE`]'s content.
#[derive(Default, Serialize, Deserialize)]
struct SavedRequests {
    #[serde(default)]
    waiting: Vec<SavedRequest>,
    #[serde(default)]
    answered: Vec<Answered>,
}

/// The person's answer to one request (`POST …/superagent/requests/{id}`). Its `Debug` never
/// shows a secret's value (review of slice 10a, m3).
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestAnswer {
    /// Approved, or declined.
    pub approve: bool,
    /// The revision of the request the card showed (SPEC-487, review of slice 8a, F1).
    #[serde(default)]
    pub revision: Option<u32>,
    /// A production release: the project's slug, typed by the person (SPEC-487 REL-001).
    #[serde(default)]
    pub confirm: Option<String>,
    /// A secret's value the person typed on the panel (SPEC-487 SEC-002).
    #[serde(default)]
    pub value: Option<String>,
}

impl std::fmt::Debug for RequestAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestAnswer")
            .field("approve", &self.approve)
            .field("revision", &self.revision)
            .field("confirm", &self.confirm)
            .field("value", &self.value.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

impl RequestAnswer {
    /// An approval with nothing typed.
    #[must_use]
    pub fn approve() -> Self {
        Self {
            approve: true,
            ..Self::default()
        }
    }
}

/// The key of a Superagent's MCP call: its shell (from its caller's audit detail) and its
/// JSON-RPC id, as sent (`7` and `"7"` are two calls).
pub(crate) fn call_key(caller: &ProjectCaller, rpc_id: &Value) -> Option<String> {
    let shell = caller.detail.get("shell").and_then(Value::as_str)?;
    Some(format!("{shell}#{rpc_id}"))
}

/// The waiting requests by id (bounded by [`MAX_WAITING`] per person and sandbox) and the
/// recent outcomes, newest last.
#[derive(Default)]
pub struct CrossRequests {
    waiting: Mutex<HashMap<String, Waiting>>,
    answered: Mutex<VecDeque<Answered>>,
    /// Serializes the writes of [`SUPERAGENT_REQUESTS_FILE`].
    file: Mutex<()>,
    /// Set from the start until the saved requests are back: no write may replace the file
    /// before it was read (review of slice 8a, F7).
    restoring: std::sync::atomic::AtomicBool,
}

impl CrossRequests {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Waiting>> {
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn answered(&self) -> std::sync::MutexGuard<'_, VecDeque<Answered>> {
        self.answered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// When the call ends however it ends (a dropped call included), its request stops waiting
/// for it and stays on the panel (SPEC-487 XRQ-001).
struct Ends<'a> {
    app: &'a App,
    id: String,
}

impl Drop for Ends<'_> {
    fn drop(&mut self) {
        if let Some(request) = self.app.cross_requests.lock().get_mut(&self.id) {
            request.stop_waiting();
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
pub(crate) fn check_visible(args: &Value) -> AppResult<()> {
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

/// Waits for the answer, at most `wait`; then `stop_waiting` leaves the request on the panel
/// without the call, unless its person is answering it (it is no longer waiting), whose outcome
/// is awaited for `grace` more rather than said to be unanswered while the task is being
/// created.
async fn settle(
    mut rx: oneshot::Receiver<AppResult<Value>>,
    wait: Duration,
    grace: Duration,
    stop_waiting: impl FnOnce() -> bool,
) -> Settled {
    match tokio::time::timeout(wait, &mut rx).await {
        Ok(Ok(answer)) => return Settled::Answered(answer),
        Ok(Err(_)) => return Settled::Lost,
        Err(_) if stop_waiting() => return Settled::Unanswered,
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
            CrossAction::Ask => {
                let parsed: AskArgs = parse(args)?;
                if parsed.question.trim().is_empty() {
                    return Err(AppError::Invalid("the question is empty".into()));
                }
                if parsed.question.len() > MAX_QUESTION_BYTES {
                    return Err(AppError::Invalid(format!(
                        "the question is longer than {MAX_QUESTION_BYTES} bytes"
                    )));
                }
                String::new()
            }
            CrossAction::Deploy
            | CrossAction::Rollback
            | CrossAction::Promote
            | CrossAction::Onboard
            | CrossAction::SecretValue => String::new(),
        };
        check_visible(args)?;
        let (user, target) = if action.own_sandbox() {
            // A decision of the Superagent's own: on its own panel, about its own sandbox.
            let user = self.manage_authority(caller).await?;
            let own = self
                .deps
                .store
                .sandbox(&caller.sandbox)
                .await?
                .ok_or(AppError::NotFound)?;
            // A release is checked as the CD page checks it, before anyone is asked (REL-002).
            match action {
                CrossAction::Ask => {}
                CrossAction::SecretValue => {
                    self.checked_secret_request(&user, &own, args).await?;
                }
                _ => self.checked_release(&user, &own, action, args).await?,
            }
            (user, own)
        } else {
            self.cross_target(caller, &reference, action.own_tool())
                .await?
        };
        let mut task_args = without_sandbox(args);
        // A secret's card says whose it is: the default scope named (review of slice 10a, M2).
        if action == CrossAction::SecretValue
            && let Some(map) = task_args.as_object_mut()
        {
            map.entry("scope").or_insert_with(|| json!("sandbox"));
        }
        let mut starts_now = false;
        let task = if action == CrossAction::Create || action.own_sandbox() {
            None
        } else {
            let there = pinned(caller, &target);
            let task = self.agent_task_ref(&there, &task_args).await?;
            let named = Named::of(&task);
            if matches!(action, CrossAction::Update | CrossAction::Message)
                && !crate::tasks::owns(&task, &user.id)
                && let Err(error) = self.check_unowned_request(&there, &task, action).await
            {
                self.audit_cross(caller, &target, action, Some(&named), "failed")
                    .await;
                return Err(error);
            }
            // The approval acts on the task the card names, by its id.
            if let Some(map) = task_args.as_object_mut() {
                map.remove("number");
                map.insert("task_id".into(), json!(task.id.as_str()));
            }
            if action == CrossAction::Update {
                starts_now = self.checked_update(&there, &task, &mut task_args).await?;
            }
            Some(named)
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
                    "{MAX_WAITING} requests already wait for the person's answer: wait for \
                     them, amend one (sandbox_request_amend) or withdraw one \
                     (sandbox_request_withdraw); sandbox_requests lists them"
                )));
            }
            waiting.insert(
                id.clone(),
                Waiting {
                    id: id.clone(),
                    user: user.id.clone(),
                    source: caller.sandbox.clone(),
                    target: target.clone(),
                    action,
                    task: task.clone(),
                    starts_now,
                    args: task_args,
                    actor: caller.actor.clone(),
                    detail: caller.detail.clone(),
                    asked_at: self.now(),
                    revision: 0,
                    call,
                    answer: Some(tx),
                },
            );
        }
        self.save_cross_requests();
        let _ends = Ends {
            app: self,
            id: id.clone(),
        };
        self.announce_cross_requests(&user.id, &caller.sandbox);
        // The call stops waiting; the request stays on the panel (XRQ-001).
        let stop_waiting = || match self.cross_requests.lock().get_mut(&id) {
            Some(request) => {
                request.stop_waiting();
                true
            }
            None => false,
        };
        match settle(rx, wait, ANSWER_GRACE, stop_waiting).await {
            Settled::Answered(answer) => answer,
            Settled::Unanswered => Ok(json!({
                "outcome": "waiting",
                "request": id,
                "note": "The person has not answered yet. The request stays on the panel above \
                         this session until they answer it: tell them it waits there, and read \
                         its outcome later with sandbox_requests. To change what it asks, amend \
                         it (sandbox_request_amend) instead of asking again.",
            })),
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
            mine.sort_by_key(|(id, w)| (w.asked_at, (*id).clone()));
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
                    // A message's text, and a question, are read as a prompt is (XST-006).
                    let text = match w.action {
                        CrossAction::Message => w.args.get("text"),
                        CrossAction::Ask => w.args.get("question"),
                        CrossAction::Onboard => w.args.get("notes"),
                        CrossAction::SecretValue => w.args.get("description"),
                        _ => w.args.get("prompt"),
                    };
                    let view = json!({
                        "id": id,
                        "action": w.action.as_str(),
                        "sandbox": {"id": w.target.id.as_str(), "slug": w.target.slug, "repository": label(&w.target)},
                        "task": null,
                        "starts_now": w.starts_now,
                        // A secret asked for is named by its name (SEC-001).
                        "title": if w.action == CrossAction::SecretValue {
                            w.args.get("name")
                        } else {
                            w.args.get("title")
                        },
                        "prompt": text,
                        "options": options,
                        "revision": w.revision,
                        "asked_at": rfc3339(w.asked_at),
                    });
                    (view, w.task.clone())
                })
                .collect()
        };
        let mut out = Vec::with_capacity(listed.len());
        for (mut view, named) in listed {
            // A secret's card says when the value replaces one of the person's (SPEC-487,
            // review of slice 10a, M2).
            if view["action"] == "secret" {
                let mut args = json!({"name": view["title"]});
                if let Some(scope) = view["options"]["scope"].as_str() {
                    args["scope"] = json!(scope);
                }
                view["replaces"] = json!(self.secret_replaced(ctx, sid, &args).await.is_some());
            }
            if let Some(named) = named {
                // Up to 4 minutes may have passed: the card names the task as it is now.
                let now = match TaskId::parse(&named.id) {
                    Ok(id) => self.deps.store.task(&id).await?,
                    Err(_) => None,
                };
                let (title, status) = now
                    .as_ref()
                    .map_or((named.title.as_str(), named.status.as_str()), |t| {
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
        answer: &RequestAnswer,
    ) -> AppResult<Value> {
        let (approve, revision) = (answer.approve, answer.revision);
        let (confirm, value) = (answer.confirm.as_deref(), answer.value.as_deref());
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "only a person answers a Superagent's request, on the Yulius page".into(),
            ));
        }
        let mut stored_value: Option<zeroize::Zeroizing<String>> = None;
        let request = {
            let mut waiting = self.cross_requests.lock();
            match waiting.get(id) {
                Some(w) if w.user == ctx.user.id && w.source.as_str() == sid => {
                    // Amended since the card was drawn: the person has not read what would run.
                    if revision.unwrap_or(0) != w.revision {
                        return Err(AppError::Conflict(
                            "the Superagent changed this request since you read it: read it \
                             again"
                                .into(),
                        ));
                    }
                    // A production release takes the project's slug typed, as the CD page
                    // does; a mistyped one leaves the request waiting (REL-001).
                    if approve
                        && w.action.typed()
                        && confirm.map(str::trim) != Some(w.target.slug.as_str())
                    {
                        return Err(AppError::Invalid(format!(
                            "type the project's slug `{}` to confirm",
                            w.target.slug
                        )));
                    }
                    // A secret's value is checked as the vault will store it before the request
                    // is taken: one it refuses leaves the request waiting (SPEC-487 SEC-002).
                    if approve && w.action == CrossAction::SecretValue {
                        stored_value = Some(crate::release_requests::secret_value_to_store(value)?);
                    }
                    waiting.remove(id)
                }
                _ => None,
            }
        }
        .ok_or(AppError::NotFound)?;
        // Taken: a restart from here on neither shows it again nor loses it unanswered (its
        // outcome is kept when it ends; review of slice 8a, F4).
        self.save_cross_requests();
        // Taken, it is answered to the end even if this request is cancelled (the person's page
        // went away): the waiting call gets the outcome either way.
        let app = Arc::clone(self);
        let person = ctx.clone();
        tokio::spawn(async move {
            app.answer_taken(request, approve, person, stored_value)
                .await
        })
        .await
        .map_err(AppError::internal)?
    }

    async fn answer_taken(
        self: Arc<Self>,
        request: Waiting,
        approve: bool,
        person: AuthContext,
        secret_value: Option<zeroize::Zeroizing<String>>,
    ) -> AppResult<Value> {
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
            let done = if request.action == CrossAction::SecretValue {
                self.store_secret_value(&person, &request.target, &request.args, secret_value)
                    .await
            } else if request.action.own_sandbox() && request.action != CrossAction::Ask {
                self.run_release(&person, &request.target, request.action, &request.args)
                    .await
            } else {
                self.run_approved(&caller, &request).await
            };
            let audited = if done.is_ok() { "approved" } else { "failed" };
            (audited, done)
        } else if request.action == CrossAction::Ask {
            (
                "declined",
                Ok(json!({
                    "outcome": "declined",
                    "answer": "no",
                    "note": "The person answered no.",
                })),
            )
        } else {
            (
                "declined",
                Ok(json!({
                    "outcome": "declined",
                    "note": format!("The person declined: {}. Do not ask again unless they ask.", request.action.nothing()),
                })),
            )
        };
        // Kept for its Superagent to read later (XRQ-004), the call gone or not.
        let kept = match &answer {
            Ok(value) => value.clone(),
            Err(err) => json!({"outcome": "failed", "error": err.public_message()}),
        };
        self.keep_outcome(request.ended(kept, self.now()));
        self.audit_cross(
            &source,
            &request.target,
            request.action,
            request.task.as_ref(),
            outcome,
        )
        .await;
        // The call may have ended meanwhile; the person still gets the answer.
        if let Some(waiting) = request.answer {
            let _ = waiting.send(answer.clone());
        }
        answer
    }

    /// A teammate may answer an eligible open question. Other steering requires control.
    async fn check_unowned_request(
        &self,
        caller: &ProjectCaller,
        task: &TaskRecord,
        action: CrossAction,
    ) -> AppResult<()> {
        let ctx = self.manage_context(caller).await?;
        if action == CrossAction::Message
            && matches!(task.status, TaskStatus::Running | TaskStatus::Parked)
            && let Some(asked) = self.deps.store.task_needs_you(&task.id).await?
        {
            self.require_task(&ctx, task, yulius_domain::task_access::TaskAction::Answer)
                .await?;
            let owner = self.login_of(&task.owner).await;
            self.check_answer_authority(&ctx, task, &asked.named, &owner)
                .await
        } else {
            self.require_control(&ctx, task).await
        }
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
            CrossAction::Ask => {
                return Ok(json!({
                    "outcome": "approved",
                    "answer": "yes",
                    "note": "The person answered yes.",
                }));
            }
            CrossAction::Deploy
            | CrossAction::Rollback
            | CrossAction::Promote
            | CrossAction::Onboard
            | CrossAction::SecretValue => {
                return Err(AppError::internal(
                    "a release runs as the person who answered",
                ));
            }
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
        let outcome = json!({
            "outcome": "cancelled",
            "note": format!("The call was cancelled: {}.", request.action.nothing()),
        });
        self.keep_outcome(request.ended(outcome.clone(), self.now()));
        if let Some(waiting) = request.answer {
            let _ = waiting.send(Ok(outcome));
        }
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
        // A question to the person is asked in its own sandbox: one row (review of slice 8a, F6).
        let sides = if target.id == source.sandbox {
            &sides[1..]
        } else {
            &sides[..]
        };
        for &(sandbox, side, other) in sides {
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

impl App {
    /// Keeps how a request ended for its Superagent (XRQ-004), the newest [`MAX_ANSWERED`].
    fn keep_outcome(&self, answered: Answered) {
        {
            let mut kept = self.cross_requests.answered();
            kept.push_back(answered);
            while kept.len() > MAX_ANSWERED {
                kept.pop_front();
            }
        }
        self.save_cross_requests();
    }

    /// Writes [`SUPERAGENT_REQUESTS_FILE`] whole, then renames it into place (XRQ-002).
    /// Failures are logged: the requests stay in memory.
    fn save_cross_requests(&self) {
        if self
            .cross_requests
            .restoring
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            // The restore writes them all, these with the saved ones.
            return;
        }
        let _guard = self
            .cross_requests
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = SavedRequests {
            waiting: self
                .cross_requests
                .lock()
                .values()
                .map(|w| SavedRequest {
                    id: w.id.clone(),
                    user: w.user.as_str().to_owned(),
                    source: w.source.as_str().to_owned(),
                    target: w.target.id.as_str().to_owned(),
                    action: w.action.as_str().to_owned(),
                    task: w.task.clone(),
                    starts_now: w.starts_now,
                    args: w.args.clone(),
                    actor: w.actor.clone(),
                    detail: w.detail.clone(),
                    asked_at: w.asked_at.0,
                    revision: w.revision,
                })
                .collect(),
            answered: self.cross_requests.answered().iter().cloned().collect(),
        };
        let path = self.config.data_dir.join(SUPERAGENT_REQUESTS_FILE);
        let tmp = path.with_extension("json.tmp");
        let written = serde_json::to_vec(&saved)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(err) = written {
            tracing::warn!(error = %err, "the Superagents' requests were not saved");
        }
    }

    /// At a start: puts back the requests that waited and the recent outcomes (XRQ-002). A
    /// request whose sandbox is gone is dropped. No call waits for them any more.
    pub async fn restore_cross_requests(&self) {
        self.restore_saved_requests().await;
        self.cross_requests
            .restoring
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.save_cross_requests();
    }

    /// At the start, before the server takes calls: until [`App::restore_cross_requests`] has
    /// read the saved requests, none is written, so a request asked meanwhile loses none.
    pub fn begin_cross_restore(&self) {
        self.cross_requests
            .restoring
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    async fn restore_saved_requests(&self) {
        let path = self.config.data_dir.join(SUPERAGENT_REQUESTS_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let saved: SavedRequests = match serde_json::from_slice(&bytes) {
            Ok(saved) => saved,
            Err(err) => {
                // Kept for a person to read: the save that follows the restore would replace it
                // (review of slice 8a, N2).
                let kept = path.with_extension("json.bad");
                tracing::warn!(error = %err, kept = %kept.display(), "the Superagents' requests could not be read");
                if let Err(err) = std::fs::rename(&path, &kept) {
                    tracing::warn!(error = %err, "the unreadable requests file was not kept");
                }
                return;
            }
        };
        let mut restored = Vec::new();
        for entry in saved.waiting {
            let Some(action) = CrossAction::parse(&entry.action) else {
                continue;
            };
            let (Ok(user), Ok(source), Ok(target_id)) = (
                UserId::parse(&entry.user),
                SandboxId::parse(&entry.source),
                SandboxId::parse(&entry.target),
            ) else {
                continue;
            };
            let Ok(Some(target)) = self.deps.store.sandbox(&target_id).await else {
                continue;
            };
            if target.removed_at.is_some() {
                continue;
            }
            restored.push(Waiting {
                id: entry.id,
                user,
                source,
                target,
                action,
                task: entry.task,
                starts_now: entry.starts_now,
                args: entry.args,
                actor: entry.actor,
                detail: entry.detail,
                asked_at: Timestamp(entry.asked_at),
                revision: entry.revision,
                call: None,
                answer: None,
            });
        }
        {
            let mut waiting = self.cross_requests.lock();
            for request in restored {
                waiting.entry(request.id.clone()).or_insert(request);
            }
        }
        {
            // The saved outcomes are older than any kept since the start.
            let mut kept = self.cross_requests.answered();
            let mut merged: VecDeque<Answered> = saved.answered.into();
            merged.extend(kept.drain(..));
            while merged.len() > MAX_ANSWERED {
                merged.pop_front();
            }
            *kept = merged;
        }
    }

    /// `sandbox_requests` (XRQ-004): the Superagent's requests that wait for its person, and
    /// how its recent ones ended, newest first.
    ///
    /// # Errors
    /// `Forbidden` for anyone but a Superagent.
    pub async fn tool_sandbox_requests(&self, caller: &ProjectCaller) -> AppResult<Value> {
        if caller.channel != Channel::Superagent || caller.task.is_some() {
            return Err(AppError::Forbidden(
                "only a Superagent asks for this".into(),
            ));
        }
        let mut waiting: Vec<(i64, Value)> = self
            .cross_requests
            .lock()
            .values()
            .filter(|w| w.user == caller.user && w.source == caller.sandbox)
            .map(|w| {
                (
                    w.asked_at.0,
                    json!({
                        "request": w.id,
                        "action": w.action.as_str(),
                        "sandbox": w.target.slug,
                        "task": w.task.as_ref().map(|task| task.number),
                        "asked_at": rfc3339(w.asked_at),
                    }),
                )
            })
            .collect();
        waiting.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        let answered: Vec<Value> = self
            .cross_requests
            .answered()
            .iter()
            .rev()
            .filter(|a| a.user == caller.user.as_str() && a.source == caller.sandbox.as_str())
            .take(20)
            .map(|a| {
                json!({
                    "request": a.id,
                    "action": a.action,
                    "sandbox": a.target,
                    "task": a.task,
                    "outcome": a.outcome,
                    "answered_at": rfc3339(Timestamp(a.at)),
                })
            })
            .collect();
        Ok(json!({
            "waiting": waiting.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            "answered": answered,
        }))
    }

    /// `sandbox_request_amend` (XRQ-003): replaces what a waiting request asks (a new task's
    /// prompt and options, a message's text, a question), checked as a new request is; it keeps
    /// its place and its id. A change of a task's start, or a stop, is withdrawn and asked again.
    ///
    /// # Errors
    /// `NotFound` for a request that is not the Superagent's or no longer waits; `Invalid` for
    /// arguments a new request would refuse.
    pub async fn tool_sandbox_request_amend(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let id = parse::<RequestRef>(&json!({"request": args.get("request")}))?.request;
        let (action, slug, task_id) = {
            let waiting = self.cross_requests.lock();
            let w = waiting
                .get(&id)
                .filter(|w| w.user == caller.user && w.source == caller.sandbox)
                .ok_or(AppError::NotFound)?;
            (
                w.action,
                w.target.slug.clone(),
                w.args.get("task_id").cloned(),
            )
        };
        let mut new_args = args.clone();
        if let Some(map) = new_args.as_object_mut() {
            map.remove("request");
        }
        match action {
            CrossAction::Create => {
                let mut checked = new_args.clone();
                checked["sandbox"] = json!(slug);
                let parsed: CrossTaskArgs = parse(&checked)?;
                if parsed.prompt.trim().is_empty() || parsed.prompt.len() > MAX_PROMPT_BYTES {
                    return Err(AppError::Invalid(format!(
                        "the prompt must be 1 byte to {MAX_PROMPT_BYTES} bytes"
                    )));
                }
            }
            CrossAction::Message => {
                let text = new_args.get("text").and_then(Value::as_str).unwrap_or("");
                if text.trim().is_empty() || text.len() > MAX_MESSAGE_BYTES {
                    return Err(AppError::Invalid(
                        "the message must be 1 byte to 64 KiB".into(),
                    ));
                }
                new_args = json!({"text": text, "task_id": task_id});
            }
            CrossAction::Ask => {
                let parsed: AskArgs = parse(&new_args)?;
                if parsed.question.trim().is_empty() || parsed.question.len() > MAX_QUESTION_BYTES {
                    return Err(AppError::Invalid(format!(
                        "the question must be 1 byte to {MAX_QUESTION_BYTES} bytes"
                    )));
                }
            }
            CrossAction::Deploy
            | CrossAction::Rollback
            | CrossAction::Promote
            | CrossAction::Onboard
            | CrossAction::SecretValue => {
                return Err(AppError::Invalid(
                    "a release or onboarding is not amended, so the card the person confirms \
                     never changes: withdraw it (sandbox_request_withdraw) and ask again"
                        .into(),
                ));
            }
            CrossAction::Update | CrossAction::Stop => {
                return Err(AppError::Invalid(
                    "a change of a task's start or a stop is not amended: withdraw it \
                     (sandbox_request_withdraw) and ask again"
                        .into(),
                ));
            }
        }
        check_visible(&new_args)?;
        let (user, target, action, task) = {
            let mut waiting = self.cross_requests.lock();
            let w = waiting
                .get_mut(&id)
                .filter(|w| w.user == caller.user && w.source == caller.sandbox)
                .ok_or(AppError::NotFound)?;
            w.args = new_args;
            w.revision += 1;
            (w.user.clone(), w.target.clone(), w.action, w.task.clone())
        };
        self.save_cross_requests();
        self.announce_cross_requests(&user, &caller.sandbox);
        // Audited without its text, as an answer is (review of slice 8a, F6).
        self.audit_cross(caller, &target, action, task.as_ref(), "amended")
            .await;
        Ok(json!({
            "request": id,
            "note": "Amended: the panel shows the new text; it keeps its place.",
        }))
    }

    /// `sandbox_request_withdraw` (XRQ-003): takes back a waiting request; nothing of it runs.
    ///
    /// # Errors
    /// `NotFound` for a request that is not the Superagent's or no longer waits.
    pub async fn tool_sandbox_request_withdraw(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let id = parse::<RequestRef>(&json!({"request": args.get("request")}))?.request;
        let request = {
            let mut waiting = self.cross_requests.lock();
            match waiting.get(&id) {
                Some(w) if w.user == caller.user && w.source == caller.sandbox => {
                    waiting.remove(&id)
                }
                _ => None,
            }
        }
        .ok_or(AppError::NotFound)?;
        self.save_cross_requests();
        self.announce_cross_requests(&request.user, &request.source);
        self.audit_cross(
            caller,
            &request.target,
            request.action,
            request.task.as_ref(),
            "withdrawn",
        )
        .await;
        let outcome = json!({
            "outcome": "withdrawn",
            "note": format!("Withdrawn: {}.", request.action.nothing()),
        });
        self.keep_outcome(request.ended(outcome.clone(), self.now()));
        if let Some(waiting) = request.answer {
            let _ = waiting.send(Ok(outcome));
        }
        Ok(json!({"request": id, "note": format!("Withdrawn: {}.", request.action.nothing())}))
    }

    /// `sandbox_tasks_list` (XRQ-006): `tasks_list` of another sandbox its person can work in.
    /// It only reads: nobody is asked.
    ///
    /// # Errors
    /// `Invalid` arguments or its own sandbox, `NotFound` for a sandbox the person cannot work
    /// in.
    pub async fn tool_sandbox_tasks_list(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let reference = args
            .get("sandbox")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Invalid("missing string argument `sandbox`".into()))?;
        let (_, target) = self.cross_target(caller, reference, "tasks_list").await?;
        let there = pinned(caller, &target);
        self.tool_tasks_list(&there, &without_sandbox(args)).await
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
