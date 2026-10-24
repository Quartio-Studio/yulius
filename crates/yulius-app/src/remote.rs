//! The remote MCP (SPEC-247, ADR-0188): a person's own agent on their laptop drives Yulius over
//! `POST /mcp` on the public listener with a personal `read` or `operate` API token.
//!
//! Contract:
//! - only API tokens of the `read` and `operate` scopes are served; every request counts against
//!   [`REMOTE_REQUESTS_PER_MINUTE`] for its token (RMT-010, RMT-014);
//! - every tool has an [`Action`]; a project tool names its project in `project` and runs only
//!   when the token's scope and the person's access to that project allow the action, checked
//!   before it runs (RMT-011, RMT-012);
//! - the Superagent's tools run as a [`ProjectCaller`] with the actor `remote:<login>`; the rest
//!   call the functions the web UI's routes call, with the person's [`AuthContext`] (RMT-013,
//!   RMT-020..027);
//! - every call of a tool that changes something is audited `remote.tool`, without its
//!   arguments; project results are redacted of the project's agent secrets (RMT-015).

use std::sync::Arc;

use serde_json::{Map, Value, json};
use tracing::Instrument as _;
use yulius_contracts::api::cd::CdDeploy;
use yulius_contracts::api::sandboxes::CreateSandbox;
use yulius_contracts::api::secrets::{AgentSecretPatch, AgentSecretView, HostRequestAnswer};
use yulius_contracts::api::tasks::ContinueTask;
use yulius_contracts::api::variables::ProjectVariableInput;
use yulius_contracts::mcp::{
    RpcRequest, RpcResponse, SUPPORTED_PROTOCOL_VERSIONS, ToolDescriptor, text_result,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{Action, TokenScope};

use crate::auth::{AuthContext, count_client_attempt};
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::manage::TaskChange;
use crate::mcp::{STRUCTURED_META, cockpit_tool_descriptors};
use crate::model::SandboxRecord;
use crate::vault::Holder;

/// Largest remote MCP request: a `project_change` of 50 files of 256 KiB, JSON-escaped.
pub const MAX_REMOTE_BODY_BYTES: usize = 32 << 20;

/// Requests one token may make in a minute (RMT-014).
pub const REMOTE_REQUESTS_PER_MINUTE: usize = 120;

/// The remote MCP's instructions for a person's own agent.
pub const REMOTE_INSTRUCTIONS: &str = "Yulius runs coding-agent tasks on the person's GitHub repositories, which it calls projects. Every project tool takes `project`: owner/name as projects_list shows it. To bring a repository in: repositories_list, project_import, project_get (what onboarding will do), onboarding_start; its onboarding task then adapts the repository and its CI; follow it with tasks_list and task_get. CI is Yulius CI, configured by [[ci.jobs]] in the repository's yulius.toml: read files with project_files and project_file_read, change them with project_change (a commit on a new branch and a pull request that Yulius CI tests), follow it with ci_runs and ci_run, and merge it with change_merge once it passed. Work that needs judgment is a task: task_create, then task_get, session_read (its transcript), task_delivery (its pull request and checks), task_message to tell it something, task_restart to continue it, task_stop. To onboard a folder on this computer (\"onboard this into Yulius\"), use the `yulius` CLI here, signed in with `yulius remote login <url>` and this operate token, so nothing waits for the person: 1. `yulius onboard <folder> --yes --sessions all` sends its unpublished branches, uncommitted work and Claude Code sessions (secrets redacted on this computer) and adds the project. 2. Find every key the project uses to build, test and deploy: `secrets.*` in .github/workflows, deploy scripts and docs, .env* files, the ~/.ssh/config hosts and keys its deploys use, cloud CLI credentials; never print or read a value into this conversation. Import them by file: `yulius secrets import --to agent --sandbox owner/name --yes --env-files [dotenv file] --from-file NAME=PATH ...` (a dotenv comment above a key becomes its description; a multi-line value other than a PEM or OpenSSH key must be made one line first), then describe each with secret_update. The import never makes ordinary text a secret (configuration words such as production, host names, e-mail addresses, URLs without credentials), because a secret's value is masked as [secret:NAME] wherever it appears: it sends such values, and plain values shorter than 8 characters, to the project variables instead (marked ~, not a secret), which every task of the project has in its environment in plain; under a credential's name, or with --no-vars, it skips them. Run in the project's checkout, it asks about secret values its tracked files hold (with --yes it skips them). Set other values that are not secrets, such as account or project ids, with `yulius vars set NAME=value` (variables_set) or `yulius vars import FILE`; never make them secrets, which would mask them everywhere. A secret imported by mistake, whose [secret:NAME] now masks ordinary words, is deleted with `yulius secrets delete NAME` (secret_delete). Live state a deploy tool keeps on this computer (a folder with an inventory database, rollout folders, configs, for example ~/.<tool>-operator or ~/.local/share/<tool>) moves as an agent state, without reading it: `yulius secrets import --to agent --sandbox owner/name --yes --state NAME=DIR ...` (one per folder, up to 64 MiB packed; links are refused). Once the project's task restores it, the sandbox's copy is the one written to and this computer is no longer the writer: say so to the person, never run rollouts from that folder again, and resend only on purpose with --replace-state; `yulius secrets state-get NAME --out FILE` downloads the sandbox's copy; a CD environment that names it (states = [\"NAME\"] in its [cd.<name>]) restores it for each deploy and saves it back, so the vault's copy is then the one written to. 3. Write a hand-over without values (which secret is which, which state goes where in the sandbox home, the hosts and commands deploys use, what only ran on this computer, what you could not find) and start onboarding with it: `yulius project onboard --notes <file>` (onboarding_start with notes). Its task adapts the repository, its docs, CI and CD by itself. Act only on what the person asks; delete, skip and merge only what they name.";

/// Where a tool acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Across the instance, for the person.
    Instance,
    /// In the project its `project` argument names.
    Project,
}

/// Every remote tool: name, reach and the action it needs (its token scope and the person's
/// access to the project must allow it).
const REMOTE_TOOLS: &[(&str, Reach, Action)] = &[
    ("projects_list", Reach::Instance, Action::View),
    ("repositories_list", Reach::Instance, Action::View),
    ("project_import", Reach::Instance, Action::Work),
    ("project_get", Reach::Project, Action::View),
    ("onboarding_start", Reach::Project, Action::Work),
    ("onboarding_skip", Reach::Project, Action::Administer),
    ("tasks_list", Reach::Project, Action::View),
    ("task_get", Reach::Project, Action::View),
    ("task_delivery", Reach::Project, Action::View),
    ("task_create", Reach::Project, Action::Work),
    ("task_message", Reach::Project, Action::Work),
    ("task_stop", Reach::Project, Action::Work),
    ("task_restart", Reach::Project, Action::Work),
    ("task_pause", Reach::Project, Action::Work),
    ("task_unpause", Reach::Project, Action::Work),
    ("task_start_now", Reach::Project, Action::Work),
    ("task_archive", Reach::Project, Action::Work),
    ("task_delete", Reach::Project, Action::Work),
    ("sessions_search", Reach::Project, Action::View),
    ("session_read", Reach::Project, Action::View),
    ("ci_runs", Reach::Project, Action::View),
    ("ci_run", Reach::Project, Action::View),
    ("ci_rerun", Reach::Project, Action::Work),
    ("ci_cancel", Reach::Project, Action::Work),
    ("cd_status", Reach::Project, Action::View),
    ("cd_deploy", Reach::Project, Action::Work),
    ("project_files", Reach::Project, Action::View),
    ("project_file_read", Reach::Project, Action::View),
    ("project_change", Reach::Project, Action::Work),
    ("change_merge", Reach::Project, Action::Work),
    ("automations_list", Reach::Project, Action::View),
    ("automation_create", Reach::Project, Action::Configure),
    ("automation_update", Reach::Project, Action::Configure),
    ("automation_delete", Reach::Project, Action::Configure),
    ("automation_run_now", Reach::Project, Action::Configure),
    (
        "automation_template_enable",
        Reach::Project,
        Action::Configure,
    ),
    ("workflows_list", Reach::Project, Action::View),
    ("secrets_list", Reach::Project, Action::View),
    ("secret_update", Reach::Project, Action::Configure),
    ("secret_hosts_answer", Reach::Project, Action::Configure),
    ("secret_delete", Reach::Project, Action::Configure),
    ("variables_list", Reach::Project, Action::View),
    ("variables_set", Reach::Project, Action::Configure),
    ("variable_delete", Reach::Project, Action::Configure),
];

/// Tools that run in a task of their own, to their end and their audit, even when the caller goes
/// away.
const SPAWNED_TOOLS: &[&str] = &["project_change", "change_merge"];

/// The Superagent's tools the remote MCP serves as they are, with `project` added.
const SHARED_TOOLS: &[&str] = &[
    "tasks_list",
    "task_get",
    "task_create",
    "task_message",
    "task_stop",
    "task_archive",
    "task_delete",
    "sessions_search",
    "session_read",
    "automations_list",
    "automation_create",
    "automation_update",
    "automation_delete",
    "automation_run_now",
    "automation_template_enable",
    "workflows_list",
    "secrets_list",
];

fn remote_tool(name: &str) -> Option<(Reach, Action)> {
    REMOTE_TOOLS
        .iter()
        .find(|(tool, _, _)| *tool == name)
        .map(|(_, reach, action)| (*reach, *action))
}

/// A person's agent calling through one of their API tokens.
#[derive(Debug, Clone)]
pub struct RemoteCaller {
    /// The person, with the token's grant.
    pub ctx: AuthContext,
    /// The token's id.
    pub token_id: String,
    /// The token's scope (`read` or `operate`).
    pub scope: TokenScope,
    /// The person's login (audits).
    pub login: String,
}

impl RemoteCaller {
    fn actor(&self) -> String {
        format!("remote:{}", self.login)
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// The `project` argument's schema.
fn project_property() -> Value {
    json!({"type": "string", "description": "The project: owner/name as projects_list shows it (or its slug or id)"})
}

/// `descriptor` with a required `project` argument.
fn with_project(mut descriptor: ToolDescriptor) -> ToolDescriptor {
    let schema = &mut descriptor.input_schema;
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert("project".into(), project_property());
    }
    match schema.get_mut("required").and_then(Value::as_array_mut) {
        Some(required) => required.insert(0, json!("project")),
        None => schema["required"] = json!(["project"]),
    }
    descriptor
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema: schema(properties, required),
    }
}

/// Descriptors of the tools only the remote MCP serves.
fn own_descriptors() -> Vec<ToolDescriptor> {
    let task_ref = json!({
        "task_id": {"type": "string", "description": "Task id"},
        "number": {"type": "integer", "description": "Task number within the project"}
    });
    let run = json!({"run_id": {"type": "string", "description": "A run id from ci_runs"}});
    let instance = vec![
        tool(
            "projects_list",
            "The projects (Yulius sandboxes of GitHub repositories, and playgrounds) the person can see: owner/name, access (read, write, admin), onboarding state and default branch.",
            json!({}),
            &[],
        ),
        tool(
            "repositories_list",
            "The GitHub repositories the person may add to Yulius (they administer or maintain them, owned by the instance's owner), and whether each is already a project.",
            json!({}),
            &[],
        ),
        tool(
            "project_import",
            "Add a GitHub repository to Yulius as a project, as Import in the web UI. It is cloned and stays pending: onboarding_start starts its onboarding task, which adapts the repository to Yulius and sets up its CI. Needs admin or maintain on GitHub.",
            json!({"repo": {"type": "string", "description": "owner/name"}}),
            &["repo"],
        ),
    ];
    let project = vec![
        tool(
            "project_get",
            "One project: repository, access, delivery profile, onboarding state with what is still pending, and what onboarding does (its task's steps).",
            json!({}),
            &[],
        ),
        tool(
            "onboarding_start",
            "Start a pending project's onboarding, as Start onboarding on its page: its onboarding task starts and adapts the repository (docs, gates, Yulius CI, CD); other tasks wait until it is done. Only when the person asks. Pass `notes` to hand over what you learned on the person's computer: which agent secret is which, the hosts and commands deploys use, what runs only there. Never put a secret's value in them.",
            json!({"notes": {"type": "string", "description": "Hand-over for the onboarding task (at most 24 KiB), added to its request; refused once onboarding started"}}),
            &[],
        ),
        tool(
            "onboarding_skip",
            "Skip onboarding, as Skip on the project page (admin): waiting tasks start at once. Only when the person asks.",
            json!({}),
            &[],
        ),
        tool(
            "task_delivery",
            "A task's delivery: its pull request, the checks at its head, review and merge state and what comes next.",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_restart",
            "Restart a task that is not running (stopped, failed, done or waiting for the person), as Continue in the web UI: its session continues where it was. A running task must be stopped first (task_stop).",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_pause",
            "Pause a running task, as Pause in the web UI (its process is suspended, nothing is lost).",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_unpause",
            "Resume a paused task, as Resume in the web UI.",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_start_now",
            "Start now a task that waits (prepared, scheduled, after another task or a start rule), as Start now in the web UI.",
            task_ref,
            &[],
        ),
        tool(
            "ci_runs",
            "The project's Yulius CI: the jobs in force and where they come from, and the latest runs (branch or pull request, commit, state, jobs).",
            json!({}),
            &[],
        ),
        tool(
            "ci_run",
            "One CI run: its jobs with state and time, and for each job that failed its failing output.",
            run.clone(),
            &["run_id"],
        ),
        tool(
            "ci_rerun",
            "Run the CI again for the commit of a finished run.",
            run.clone(),
            &["run_id"],
        ),
        tool(
            "ci_cancel",
            "Cancel a queued or running CI run.",
            run,
            &["run_id"],
        ),
        tool(
            "cd_status",
            "The project's CD (deployment): dev and production as yulius.toml defines them, what is deployed, the secrets they need and the recent runs; with run_id, one run with its output.",
            json!({"run_id": {"type": "string", "description": "A run id, to read its output"}}),
            &[],
        ),
        tool(
            "cd_deploy",
            "Deploy a dev-stage environment (dev, or another [cd.<name>] cd_status lists with stage dev): Yulius runs its deploy and verify commands from the integration branch head. Production and production-stage environments are deployed only by a person on the CD page.",
            json!({"environment": {"type": "string", "description": "dev or another dev-stage environment's name"}}),
            &["environment"],
        ),
        tool(
            "project_files",
            "List a directory of the project's repository at the integration branch head (or `ref`: a branch or commit), after fetching it.",
            json!({
                "path": {"type": "string", "description": "Directory, relative to the repository root (default: the root)"},
                "ref": {"type": "string", "description": "Branch or commit (default: the integration branch)"}
            }),
            &[],
        ),
        tool(
            "project_file_read",
            "Read one file of the project's repository at the integration branch head (or `ref`), at most 256 KiB of text.",
            json!({
                "path": {"type": "string", "description": "File path relative to the repository root, e.g. yulius.toml"},
                "ref": {"type": "string", "description": "Branch or commit (default: the integration branch)"}
            }),
            &["path"],
        ),
        tool(
            "project_change",
            "Change files of the project the safe way: the given whole files (or deletions) become one commit, authored by the person, on a new branch yulius/change-… from the integration branch head, pushed with a pull request into it. Yulius CI tests it like any pull request; follow it with ci_runs and merge it with change_merge once it passed. For exact edits such as yulius.toml's [[ci.jobs]]; work that needs judgment is a task (task_create). Never touches a running task's files.",
            json!({
                "title": {"type": "string", "description": "The commit and pull request title, e.g. \"ci: run cargo test in Yulius CI\""},
                "body": {"type": "string", "description": "Why, for the pull request"},
                "files": {"type": "array", "maxItems": 50, "items": {"type": "object", "properties": {
                    "path": {"type": "string", "description": "Repository-relative path"},
                    "content": {"type": "string", "description": "The file's whole new content (at most 256 KiB)"},
                    "delete": {"type": "boolean", "description": "true deletes the file (no content)"}
                }, "required": ["path"]}}
            }),
            &["title", "files"],
        ),
        tool(
            "change_merge",
            "Merge a pull request project_change opened, once its checks (Yulius CI) passed at its exact head, with the project's merge method, and delete its branch. Refused while they run or failed, and for any other pull request.",
            json!({"number": {"type": "integer", "description": "The pull request's number"}}),
            &["number"],
        ),
        tool(
            "secret_update",
            "Change an agent secret's bound hosts or description (never its value: values are set on the Secrets page or with `yulius secrets import`). Changing hosts of a secret agents set up needs expect_hosts, the hosts secrets_list showed.",
            json!({
                "secret": {"type": "string", "description": "Secret NAME from secrets_list"},
                "hosts": {"type": "array", "items": {"type": "string"}, "description": "The hosts it is sent to, e.g. api.example.com, *.example.com"},
                "expect_hosts": {"type": "array", "items": {"type": "string"}, "description": "The bound hosts secrets_list showed"},
                "description": {"type": "string"}
            }),
            &["secret"],
        ),
        tool(
            "secret_hosts_answer",
            "Allow or dismiss the hosts an agent asked a secret to be bound to (requested_hosts in secrets_list), as on the Secrets page.",
            json!({
                "secret": {"type": "string", "description": "Secret NAME from secrets_list"},
                "allow": {"type": "boolean"},
                "hosts": {"type": "array", "items": {"type": "string"}, "description": "The requested hosts secrets_list showed"}
            }),
            &["secret", "allow", "hosts"],
        ),
        tool(
            "secret_delete",
            "Delete one of the project's own agent secrets by name, for example one imported by mistake whose value is ordinary text (a word every [secret:NAME] in files stands for). Global and personal secrets are removed on the Secrets page. Audited; the value is never shown.",
            json!({"secret": {"type": "string", "description": "Secret NAME from secrets_list"}}),
            &["secret"],
        ),
        tool(
            "variables_list",
            "The project's variables (SPEC-275): its non-secret values (regions, environment names, account and project ids, base URLs, contact addresses) with their descriptions, in plain. Every task run of the project has them in its environment.",
            json!({}),
            &[],
        ),
        tool(
            "variables_set",
            "Create or change project variables, in plain: non-secret values only, never a key, token or password (those are agent secrets: `yulius secrets import`). A name that is an agent secret of the project is refused. An absent description keeps the stored one. Audited without values.",
            json!({
                "variables": {
                    "type": "array",
                    "description": "At most 200",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "NAME, like an environment variable"},
                            "value": {"type": "string", "description": "One line, at most 4 KiB"},
                            "description": {"type": "string", "description": "What it is"}
                        },
                        "required": ["name", "value"]
                    }
                }
            }),
            &["variables"],
        ),
        tool(
            "variable_delete",
            "Delete a project variable by name. Audited.",
            json!({"name": {"type": "string", "description": "Variable NAME from variables_list"}}),
            &["name"],
        ),
    ];
    instance
        .into_iter()
        .chain(project.into_iter().map(with_project))
        .collect()
}

/// The tools a token of `scope` is served by `tools/list` (RMT-011).
#[must_use]
pub fn remote_tool_descriptors(scope: TokenScope) -> Vec<ToolDescriptor> {
    let shared = cockpit_tool_descriptors()
        .into_iter()
        .filter(|t| SHARED_TOOLS.contains(&t.name.as_str()))
        .map(|mut t| {
            t.description = t
                .description
                .replace("this project", "the project")
                .replace("this sandbox", "the project");
            with_project(t)
        });
    let mut tools: Vec<ToolDescriptor> = own_descriptors().into_iter().chain(shared).collect();
    tools.retain(|t| remote_tool(&t.name).is_some_and(|(_, action)| scope.allows(action)));
    tools.sort_by_key(|t| {
        REMOTE_TOOLS
            .iter()
            .position(|(name, _, _)| *name == t.name)
            .unwrap_or(usize::MAX)
    });
    tools
}

/// `args` without `key`.
fn without(args: &Value, key: &str) -> Value {
    let mut map: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
    map.remove(key);
    Value::Object(map)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Invalid(format!("missing string argument `{key}`")))
}

fn strings_arg(args: &Value, key: &str) -> AppResult<Option<Vec<String>>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| AppError::Invalid(format!("`{key}` must be a list of strings")))
            })
            .collect::<AppResult<Vec<_>>>()
            .map(Some),
        Some(_) => Err(AppError::Invalid(format!(
            "`{key}` must be a list of strings"
        ))),
    }
}

fn record<T: serde::Serialize>(value: &T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(AppError::internal)
}

impl App {
    /// Handles one JSON-RPC message of the remote MCP (`POST /mcp` on the public listener) from
    /// `caller` ([`App::remote_caller`]); `None` for a notification.
    pub async fn remote_mcp_handle(
        self: &Arc<Self>,
        caller: &RemoteCaller,
        request: RpcRequest,
    ) -> Option<RpcResponse> {
        let id = request.id.clone()?;
        let result = match request.method.as_str() {
            "initialize" => {
                let requested = request
                    .params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = SUPPORTED_PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == requested)
                    .copied()
                    .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "yulius", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": REMOTE_INSTRUCTIONS,
                    // Who the token acts for, for `yulius remote status`.
                    "_meta": {"yulius/person": caller.login, "yulius/scope": caller.scope.as_str()},
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": remote_tool_descriptors(caller.scope)}),
            "tools/call" => self.remote_call(caller, &request.params).await,
            other => {
                return Some(RpcResponse::err(
                    id,
                    -32601,
                    format!("method not found: {other}"),
                ));
            }
        };
        Some(RpcResponse::ok(id, result))
    }

    /// The caller of a remote request with bearer `token`, counted against its token's minute.
    ///
    /// # Errors
    /// `Unauthenticated` for anything but a live `read` or `operate` token of an enabled person,
    /// `Forbidden` for a `cli` token, `Capacity` past [`REMOTE_REQUESTS_PER_MINUTE`].
    pub async fn remote_caller(&self, token: &str) -> AppResult<RemoteCaller> {
        if !token.starts_with("yul_") {
            return Err(AppError::Unauthenticated);
        }
        let ctx = self
            .authenticate_bearer(token)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        let grant = ctx.token.clone().ok_or(AppError::Unauthenticated)?;
        if !grant.scope.remote() {
            return Err(AppError::Forbidden(
                "this token is for `yulius login` only; make a token for your agent on Settings → Connect your local agent".into(),
            ));
        }
        count_client_attempt(
            &self.remote_calls,
            &grant.id,
            self.now(),
            REMOTE_REQUESTS_PER_MINUTE,
            &format!(
                "at most {REMOTE_REQUESTS_PER_MINUTE} requests a minute per token; wait a minute"
            ),
        )?;
        let login = self.login_of(&ctx.user.id).await;
        Ok(RemoteCaller {
            ctx,
            token_id: grant.id,
            scope: grant.scope,
            login,
        })
    }

    /// One remote `tools/call`.
    async fn remote_call(self: &Arc<Self>, caller: &RemoteCaller, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let structured = params
            .get("_meta")
            .and_then(|meta| meta.get(STRUCTURED_META))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some((reach, action)) = remote_tool(name) else {
            return text_result(format!("unknown tool: {name}"), true);
        };
        if !caller.scope.allows(action) {
            self.audit_remote(caller, name, None, false).await;
            return text_result(
                format!(
                    "{name} changes things, and this token is read-only; make an operate token on Settings to use it"
                ),
                true,
            );
        }
        match reach {
            Reach::Instance => {
                let outcome = self.remote_instance_tool(caller, name, &args).await;
                if action != Action::View {
                    self.audit_remote(caller, name, None, outcome.is_ok()).await;
                }
                match outcome {
                    Ok(value) => {
                        let mut result =
                            text_result(crate::mcp_text::render(name, &args, &value), false);
                        if structured {
                            result["structuredContent"] = value;
                        }
                        result
                    }
                    Err(err) => text_result(err.public_message(), true),
                }
            }
            Reach::Project => {
                let found = match self.remote_project(&args).await {
                    Ok(found) => found,
                    Err(err) => {
                        if action != Action::View {
                            self.audit_remote(caller, name, None, false).await;
                        }
                        return text_result(err.public_message(), true);
                    }
                };
                let sandbox = match self.require(&caller.ctx.user, &found.id, action).await {
                    Ok((sandbox, _)) => sandbox,
                    Err(err) => {
                        // A refused change is audited too; a project the person cannot see is
                        // not found, and its entry names the token, not the project.
                        if action != Action::View {
                            let visible = !matches!(err, AppError::NotFound);
                            self.audit_remote(caller, name, visible.then_some(&found.id), false)
                                .await;
                        }
                        return text_result(err.public_message(), true);
                    }
                };
                let args = without(&args, "project");
                let project = ProjectCaller {
                    sandbox: sandbox.id.clone(),
                    user: caller.ctx.user.id.clone(),
                    task: None,
                    actor: caller.actor(),
                    detail: json!({"via": "remote", "token": caller.token_id}),
                };
                let outcome = self
                    .remote_project_tool(caller, &project, &sandbox, name, &args)
                    .await;
                // The spawned change tools audit themselves (they outlive their caller).
                if action != Action::View && !SPAWNED_TOOLS.contains(&name) {
                    self.audit_remote(caller, name, Some(&sandbox.id), outcome.is_ok())
                        .await;
                }
                self.tool_result(&sandbox.id, name, &args, outcome, structured)
                    .await
            }
        }
    }

    /// The live project `args.project` names (owner/name, slug or id), before any access check:
    /// callers check the person's access with [`App::require`], which hides a project they cannot
    /// see as not found (RMT-012).
    ///
    /// # Errors
    /// `Invalid` without `project`, `NotFound`.
    async fn remote_project(&self, args: &Value) -> AppResult<SandboxRecord> {
        let reference = str_arg(args, "project")?;
        let store = &self.deps.store;
        let found = match reference.split_once('/') {
            Some((owner, name)) => store.sandbox_by_repo(owner.trim(), name.trim()).await?,
            None => match store.sandbox_by_slug(reference).await? {
                Some(record) => Some(record),
                None => match SandboxId::parse(reference) {
                    Ok(id) => store.sandbox(&id).await?,
                    Err(_) => None,
                },
            },
        };
        found
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)
    }

    /// Audits a remote call of a tool that changes something, never its arguments (RMT-015):
    /// about its project, else about the token (`token:<id>`).
    async fn audit_remote(
        &self,
        caller: &RemoteCaller,
        tool: &str,
        sandbox: Option<&SandboxId>,
        ok: bool,
    ) {
        let detail = json!({"tool": tool, "token": caller.token_id, "ok": ok}).to_string();
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &caller.actor(),
                "remote.tool",
                Some(&sandbox.map_or_else(
                    || format!("token:{}", caller.token_id),
                    |id| id.as_str().to_owned(),
                )),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, tool, "remote tool audit failed");
        }
    }

    /// The instance tools.
    async fn remote_instance_tool(
        self: &Arc<Self>,
        caller: &RemoteCaller,
        name: &str,
        args: &Value,
    ) -> AppResult<Value> {
        let ctx = &caller.ctx;
        match name {
            "projects_list" => {
                let projects = self.list_sandboxes(ctx).await?;
                Ok(json!({"projects": projects.iter().map(|p| json!({
                    "project": format!("{}/{}", p.repo.owner, p.repo.name),
                    "slug": p.slug,
                    "id": p.id,
                    "access": p.access,
                    "onboarding": p.onboarding,
                    "default_branch": p.default_branch,
                    "playground": p.playground,
                })).collect::<Vec<_>>()}))
            }
            "repositories_list" => {
                Ok(json!({"repositories": self.github_repositories(ctx).await?}))
            }
            "project_import" => {
                let repo = str_arg(args, "repo")?;
                let created = self
                    .create_sandbox(
                        ctx,
                        &CreateSandbox::Import {
                            repo: repo.to_owned(),
                        },
                    )
                    .await?;
                Ok(json!({
                    "project": format!("{}/{}", created.repo.owner, created.repo.name),
                    "slug": created.slug,
                    "id": created.id,
                    "onboarding": created.onboarding,
                    "next": "project_get shows what onboarding will do; onboarding_start starts it",
                }))
            }
            other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
        }
    }

    /// The project tools.
    async fn remote_project_tool(
        self: &Arc<Self>,
        caller: &RemoteCaller,
        project: &ProjectCaller,
        sandbox: &SandboxRecord,
        name: &str,
        args: &Value,
    ) -> AppResult<Value> {
        let ctx = &caller.ctx;
        let sid = sandbox.id.as_str();
        match name {
            // The Superagent's own tools (RMT-013).
            "tasks_list" => self.tool_tasks_list(project, args).await,
            "task_get" => self.tool_task_get(project, args).await,
            "task_create" => self.tool_task_create(project, args).await,
            "task_message" => self.tool_task_message(project, args).await,
            "task_stop" => self.tool_task_change(project, args, TaskChange::Stop).await,
            "task_archive" => {
                let archived = match args.get("archived") {
                    None | Some(Value::Null) => true,
                    Some(Value::Bool(archived)) => *archived,
                    Some(_) => {
                        return Err(AppError::Invalid("`archived` must be true or false".into()));
                    }
                };
                self.tool_task_change(project, args, TaskChange::Archive(archived))
                    .await
            }
            "task_delete" => {
                self.tool_task_change(project, args, TaskChange::Delete)
                    .await
            }
            "sessions_search" => self.tool_sessions_search(project, args).await,
            "session_read" => self.tool_session_read(project, args).await,
            "automations_list" => self.tool_automations_list(project).await,
            "automation_create" => self.tool_automation_create(project, args).await,
            "automation_update" => self.tool_automation_update(project, args).await,
            "automation_delete" => self.tool_automation_delete(project, args).await,
            "automation_run_now" => self.tool_automation_run_now(project, args).await,
            "automation_template_enable" => {
                self.tool_automation_template_enable(project, args).await
            }
            "workflows_list" => self.tool_workflows_list(project).await,
            "secrets_list" => self.tool_secrets_list(project).await,
            // What the web UI's routes do, for the person (RMT-020..027).
            "project_get" => {
                let view = self.get_sandbox(ctx, sid).await?;
                let status = self.onboarding_status(sandbox).await?;
                let plan = self.onboarding_plan(ctx, sid).await?;
                let mut out = json!({
                    "project": record(&view)?,
                    "onboarding": record(&status)?,
                    "onboarding_plan": record(&plan)?,
                });
                if status.state == "pending" {
                    out["next"] = json!("onboarding_start starts the onboarding task");
                }
                Ok(out)
            }
            "onboarding_start" => {
                let notes = match args.get("notes") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(notes)) => Some(notes.as_str()),
                    Some(_) => return Err(AppError::Invalid("`notes` must be text".into())),
                };
                record(&self.approve_onboarding(ctx, sid, notes).await?)
            }
            "onboarding_skip" => record(&self.skip_onboarding(ctx, sid).await?),
            "task_delivery" => {
                let task = self.agent_task_ref(project, args).await?;
                record(&self.task_delivery(ctx, task.id.as_str()).await?)
            }
            "task_restart" | "task_pause" | "task_unpause" | "task_start_now" => {
                let task = self.agent_task_ref(project, args).await?;
                let id = task.id.as_str();
                let summary = match name {
                    "task_restart" => self
                        .continue_task(ctx, id, &ContinueTask { account_id: None })
                        .await
                        .map_err(|err| match err {
                            AppError::Conflict(why) if why.starts_with("the task is") => {
                                AppError::Conflict(format!(
                                    "{why}: stop it first (task_stop), then restart it"
                                ))
                            }
                            other => other,
                        })?,
                    "task_pause" => self.pause_task_for_person(ctx, id).await?,
                    "task_unpause" => self.resume_task_for_person(ctx, id).await?,
                    _ => self.start_chained_now(ctx, id).await?,
                };
                Ok(json!({"task": record(&summary)?}))
            }
            "ci_runs" => record(&self.ci_overview(ctx, sid).await?),
            "ci_run" => record(
                &self
                    .ci_run_detail(ctx, sid, str_arg(args, "run_id")?)
                    .await?,
            ),
            "ci_rerun" => record(
                &self
                    .ci_rerun_run(ctx, sid, str_arg(args, "run_id")?)
                    .await?,
            ),
            "ci_cancel" => record(
                &self
                    .ci_cancel_run(ctx, sid, str_arg(args, "run_id")?)
                    .await?,
            ),
            "cd_status" => match args.get("run_id").and_then(Value::as_str) {
                Some(run) => record(&self.cd_run_detail(ctx, sid, run).await?),
                None => record(&self.cd_overview(ctx, sid).await?),
            },
            "cd_deploy" => {
                let env = yulius_domain::cd::env_name(str_arg(args, "environment")?).ok_or_else(
                    || AppError::Invalid("environment is dev or another [cd.<name>]".into()),
                )?;
                // A laptop's agent deploys dev-stage environments only (SPEC-257); without a
                // typed confirmation a production-stage one is refused by the deploy anyway.
                let production = env == "prod"
                    || self.cd_overview(ctx, sid).await.is_ok_and(|cd| {
                        cd.environments
                            .iter()
                            .any(|e| e.name == env && e.stage == "prod")
                    });
                if production {
                    return Err(AppError::Forbidden(
                        "only a person deploys production, on the project's CD page".into(),
                    ));
                }
                let req = CdDeploy {
                    confirm: None,
                    expect: None,
                    confirm_ms: None,
                };
                record(&self.cd_deploy(ctx, sid, &env, &req).await?)
            }
            "project_files" => self.remote_files(sandbox, args).await,
            "project_file_read" => self.remote_file_read(sandbox, args).await,
            // Run to their end even when the caller goes away (a client timeout): a change
            // removes its scratch clone and records its pull request, a merge records itself, and
            // either is audited with its token (RMT-015) in the same task.
            "project_change" | "change_merge" => {
                let (app, owner, who, place, input, tool) = (
                    Arc::clone(self),
                    caller.clone(),
                    ctx.clone(),
                    sandbox.clone(),
                    args.clone(),
                    name.to_owned(),
                );
                let task = tokio::spawn(
                    async move {
                        let actor = owner.actor();
                        let outcome = if tool == "project_change" {
                            app.remote_change(&who, &actor, &place, &input).await
                        } else {
                            app.remote_change_merge(&who, &actor, &place, &input).await
                        };
                        app.audit_remote(&owner, &tool, Some(&place.id), outcome.is_ok())
                            .await;
                        outcome
                    }
                    .in_current_span(),
                );
                match task.await {
                    Ok(outcome) => outcome,
                    // It panicked or was cancelled before its own audit: audit the failure here.
                    Err(err) => {
                        self.audit_remote(caller, name, Some(&sandbox.id), false)
                            .await;
                        Err(AppError::internal(err))
                    }
                }
            }
            "secret_update" | "secret_hosts_answer" => {
                if args.get("value").is_some() {
                    return Err(AppError::Invalid(
                        "a secret's value never goes through an agent: set it on the Secrets page or with `yulius secrets import`".into(),
                    ));
                }
                let secret = str_arg(args, "secret")?;
                // The secret of the name that wins for this person, as `secrets_list` shows it
                // (SPEC-287): never a shadowed one or a global one switched off.
                let id = self
                    .resolved_agent_secrets(&sandbox.id, Holder::Person(&project.user))
                    .await?
                    .into_iter()
                    .find(|s| s.name == secret)
                    .map(|s| s.id)
                    .ok_or_else(|| {
                        AppError::Invalid(format!("the project has no agent secret {secret}"))
                    })?;
                let view = if name == "secret_update" {
                    let patch = AgentSecretPatch {
                        description: args
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        hosts: strings_arg(args, "hosts")?,
                        expect_hosts: strings_arg(args, "expect_hosts")?,
                        ..AgentSecretPatch::default()
                    };
                    self.update_agent_secret(ctx, &id, &patch).await?
                } else {
                    let allow = args
                        .get("allow")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| AppError::Invalid("`allow` must be true or false".into()))?;
                    let hosts = strings_arg(args, "hosts")?.unwrap_or_default();
                    self.answer_host_request(ctx, &id, &HostRequestAnswer { allow, hosts })
                        .await?
                };
                Ok(json!({"secret": {
                    "name": view.name,
                    "description": view.description,
                    "hosts": view.hosts,
                    "requested_hosts": view.requested_hosts,
                }}))
            }
            // SPEC-274 PLW-004: the project's own secret only; the value is never read.
            "secret_delete" => {
                let secret = str_arg(args, "secret")?;
                let named: Vec<AgentSecretView> = self
                    .agent_secrets(ctx, sid)
                    .await?
                    .into_iter()
                    .filter(|s| s.name == secret)
                    .collect();
                let Some(own) = named.iter().find(|s| s.scope == "sandbox") else {
                    return Err(match named.first() {
                        Some(other) => AppError::Forbidden(format!(
                            "{secret} is a {} secret, not the project's: remove it on the Secrets page",
                            if other.scope == "instance" {
                                "global"
                            } else {
                                "personal"
                            }
                        )),
                        None => {
                            AppError::Invalid(format!("the project has no agent secret {secret}"))
                        }
                    });
                };
                self.delete_agent_secret(ctx, &own.id).await?;
                Ok(json!({"deleted": own.name}))
            }
            // Project variables (SPEC-275 VAR-005): plain values, shown as they are.
            "variables_list" => record(&self.project_variables(ctx, sid).await?),
            "variables_set" => {
                let inputs: Vec<ProjectVariableInput> =
                    serde_json::from_value(args.get("variables").cloned().unwrap_or(Value::Null))
                        .map_err(|e| {
                        AppError::Invalid(format!(
                            "`variables` must be a list of {{name, value, description?}}: {e}"
                        ))
                    })?;
                record(&self.set_project_variables(ctx, sid, &inputs, false).await?)
            }
            "variable_delete" => {
                let variable = str_arg(args, "name")?;
                self.delete_project_variable(ctx, sid, variable).await?;
                Ok(json!({"deleted": variable}))
            }
            other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_remote_tool_is_described_once_and_project_tools_take_a_project() {
        let tools = remote_tool_descriptors(TokenScope::Operate);
        assert_eq!(tools.len(), REMOTE_TOOLS.len());
        for (name, reach, _) in REMOTE_TOOLS {
            let found: Vec<&ToolDescriptor> = tools.iter().filter(|t| t.name == *name).collect();
            assert_eq!(found.len(), 1, "{name}");
            let required = found[0].input_schema["required"].clone();
            let takes_project = required
                .as_array()
                .is_some_and(|r| r.iter().any(|p| p == "project"));
            assert_eq!(takes_project, *reach == Reach::Project, "{name}");
        }
    }

    /// SPEC-256 LOB-007: the person's agent learns how to onboard a folder on their computer
    /// with nothing left for them: upload, keys by file, hand-over.
    #[test]
    fn the_instructions_teach_onboarding_a_local_folder() {
        for step in [
            "yulius onboard",
            "--sessions all",
            "yulius secrets import --to agent",
            "--yes --env-files",
            "--from-file NAME=PATH",
            "--state NAME=DIR",
            "no longer the writer",
            "never print",
            "yulius project onboard --notes",
            // SPEC-275 VAR-008: plain values go to the project's variables, never secrets.
            "project variables",
            "yulius vars set NAME=value",
            // SPEC-274 PLW-006.
            "ordinary text",
            "yulius secrets delete NAME",
        ] {
            assert!(REMOTE_INSTRUCTIONS.contains(step), "{step}");
        }
    }

    #[test]
    fn a_read_token_is_served_only_tools_that_read() {
        let tools = remote_tool_descriptors(TokenScope::Read);
        assert!(tools.iter().any(|t| t.name == "tasks_list"));
        for t in &tools {
            assert_eq!(
                remote_tool(&t.name).map(|(_, a)| a),
                Some(Action::View),
                "{}",
                t.name
            );
        }
        assert!(remote_tool_descriptors(TokenScope::Cli).len() == REMOTE_TOOLS.len());
    }
}
