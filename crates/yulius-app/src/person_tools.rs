//! The person's project tools (SPEC-487 SAR-001, ADR-0330): what a person does on a project's
//! pages (a task's Continue, Pause, Resume and Start now, its delivery, the CI/CD page), as tools
//! their agents call for them.
//!
//! Contract:
//! - one list ([`PERSON_TOOLS`]) served to the person's Superagent in the sandbox (the Cockpit)
//!   and, with a `project` argument, to their own agent through the remote MCP and to their
//!   Global Superagent (SPEC-247, which had these tools first);
//! - each tool runs the function the web UI's route calls, with the person's [`AuthContext`]:
//!   their rights are checked at the call, by the caller ([`Action`] on the project) and by the
//!   function itself (a task is changed by its owner only);
//! - the callers audit a tool that changes something (`superagent.tool`, `remote.tool`).

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::cd::CdDeploy;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_contracts::mcp::ToolDescriptor;
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// Every person's project tool and the action it needs on the project.
pub(crate) const PERSON_TOOLS: &[(&str, Action)] = &[
    ("task_delivery", Action::View),
    ("task_restart", Action::Work),
    ("task_pause", Action::Work),
    ("task_unpause", Action::Work),
    ("task_start_now", Action::Work),
    ("ci_runs", Action::View),
    ("ci_run", Action::View),
    ("ci_rerun", Action::Work),
    ("ci_cancel", Action::Work),
    ("cd_status", Action::View),
    ("cd_deploy", Action::Work),
    ("variables_list", Action::View),
];

/// The action the person's project tool `name` needs, when it is one.
pub(crate) fn action(name: &str) -> Option<Action> {
    PERSON_TOOLS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, action)| *action)
}

/// A tool's input schema: an object of `properties`, `required` ones named, nothing else taken.
fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// A tool's descriptor.
pub(crate) fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema: schema(properties, required),
    }
}

/// The text argument `key`, trimmed.
///
/// # Errors
/// `Invalid` when it is missing, not text or blank.
pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Invalid(format!("missing string argument `{key}`")))
}

/// A view as a tool's record.
///
/// # Errors
/// `Internal` when it cannot be serialized.
pub(crate) fn record<T: serde::Serialize>(value: &T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(AppError::internal)
}

/// The longest id of a task or a run that an audit entry names.
const SUBJECT_ID_MAX: usize = 64;

/// What a call of the person's tool `name` was about, for its audit entry: the task, the run
/// or the environment that tool takes, and only when the argument has the shape of one (a task
/// number or id, a run id, an environment name). The entry is written for refused calls too
/// and before anything checked the arguments, so nothing else of them goes into it: an audit
/// entry never holds text of the caller's choosing (as the remote MCP's never does, RMT-015).
pub(crate) fn subject(name: &str, args: &Value) -> Value {
    // Trimmed, as the call itself reads its text arguments.
    let id = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= SUBJECT_ID_MAX
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
    };
    let mut about = serde_json::Map::new();
    match name {
        "task_restart" | "task_pause" | "task_unpause" | "task_start_now" => {
            if let Some(number) = args.get("number").and_then(Value::as_u64) {
                about.insert("task".into(), json!(number));
            } else if let Some(task) = id("task_id") {
                about.insert("task".into(), json!(task));
            }
        }
        "ci_rerun" | "ci_cancel" => {
            if let Some(run) = id("run_id") {
                about.insert("run".into(), json!(run));
            }
        }
        "cd_deploy" => {
            if let Some(environment) = args
                .get("environment")
                .and_then(Value::as_str)
                .and_then(yulius_domain::cd::env_name)
            {
                about.insert("environment".into(), json!(environment));
            }
        }
        _ => {}
    }
    Value::Object(about)
}

/// What `task_restart` answers for task #`number`, which is not resting but `status`, so that
/// Continue would refuse it: what to do instead, by what the task is doing. A task that runs
/// is stopped first; one that waits in a chain or prepared is started with `task_start_now`
/// (stopping it would cancel it); one that waits for a free session or for capacity goes on
/// by itself.
fn not_resting(number: u32, status: TaskStatus) -> AppError {
    let what = status.as_str();
    AppError::Conflict(match status {
        TaskStatus::Chained | TaskStatus::Prepared => {
            format!("task #{number} waits to start ({what}): task_start_now starts it now")
        }
        TaskStatus::Queued => {
            format!("task #{number} waits for a free session (queued) and starts by itself")
        }
        TaskStatus::WaitingCapacity => {
            format!("task #{number} waits for a subscription with capacity and goes on by itself")
        }
        _ => format!("the task is {what}: stop it first (task_stop), then restart it"),
    })
}

/// What `task_restart` answers when Continue refused the onboarding task of a project that has
/// not started onboarding (SPEC-051 OBD-002: a person starts it, and their Superagent's
/// context is neither their browser nor a token they made for an agent): that, without the
/// page's words about tokens. Every other refusal is the page's own.
fn restart_refusal(err: AppError, number: u32) -> AppError {
    match err {
        AppError::Forbidden(why) if why == crate::onboarding::STARTS_WITH_A_PERSON => {
            AppError::Forbidden(format!(
                "task #{number} is this project's onboarding, which has not started: a person \
                 starts it with Start onboarding on the project's page. Tell them so; nothing \
                 else is needed from you"
            ))
        }
        other => other,
    }
}

/// The descriptors of [`PERSON_TOOLS`], in its order, without a `project` argument (the remote
/// MCP adds one).
pub(crate) fn descriptors() -> Vec<ToolDescriptor> {
    let task_ref = json!({
        "task_id": {"type": "string", "description": "Task id"},
        "number": {"type": "integer", "description": "Task number within the project"}
    });
    let run = json!({"run_id": {"type": "string", "description": "A run id from ci_runs"}});
    vec![
        tool(
            "task_delivery",
            "A task's delivery, as its page shows it: its pull request, the checks at its head, the delivery stage, what comes next (next_actions), what is refused now and why (blocked), and notes, among them where an armed merge stands once Yulius has looked at it. It does not say whether a merge is armed while the checks still run.",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_restart",
            "Continue (restart) a task whose session ended: stopped, crashed, lost with the server, failed or closed, as Continue in the web UI. Its session continues where it was, and it takes messages again. A task that runs (also one the list shows Done or Answered, whose session is idle: it takes a message as it is) must be stopped first (task_stop); one that waits in a chain or prepared is started with task_start_now; one that is queued starts by itself. The task's owner only.",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_pause",
            "Pause a running task, as Pause in the web UI: its process is frozen where it is and nothing is lost, also across a restart of Yulius, until task_unpause. Use it instead of telling a task to wait. The task's owner only.",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_unpause",
            "Resume a task a person paused, as Resume in the web UI; a task that is not paused stays as it is. The task's owner only.",
            task_ref.clone(),
            &[],
        ),
        tool(
            "task_start_now",
            "Start now a task that waits (prepared, scheduled, after another task or on a start rule), as Start now in the web UI, whatever it waited for.",
            task_ref,
            &[],
        ),
        tool(
            "ci_runs",
            "The project's Yulius CI: the jobs in force and where they come from, and the latest runs (branch, pull request or merge train, commit, state, jobs).",
            json!({}),
            &[],
        ),
        tool(
            "ci_run",
            "One CI run: its jobs with state and time, and each job's output (its end) with the failing tests' output kept from all of it.",
            run.clone(),
            &["run_id"],
        ),
        tool(
            "ci_rerun",
            "Run the CI again for the commit of a finished run (after a failure the code did not cause). Refused while a run of that commit is queued or running, and for a merge train's run (Yulius builds a new one).",
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
            "The project's CD (deployment): every environment as yulius.toml defines it, what is deployed, the secrets it needs and the recent runs; with run_id, one run with the end of its output.",
            json!({"run_id": {"type": "string", "description": "A run id, to read its output"}}),
            &[],
        ),
        tool(
            "cd_deploy",
            "Deploy a dev-stage environment (dev, or another [cd.<name>] cd_status lists with stage dev): Yulius runs its deploy and verify commands from the integration branch head; follow the run with cd_status. Production and production-stage environments are deployed only by a person on the CD page.",
            json!({"environment": {"type": "string", "description": "dev or another dev-stage environment's name"}}),
            &["environment"],
        ),
        tool(
            "variables_list",
            "The project's variables (SPEC-275): its non-secret values (regions, environment names, account and project ids, base URLs, contact addresses) with their descriptions, in plain. Every task run of the project has them in its environment.",
            json!({}),
            &[],
        ),
    ]
}

impl App {
    /// Runs the person's project tool `name` in `sandbox` for the person of `ctx`: what the web
    /// UI's route does. The caller checked the tool's [`action`] on the project and audits.
    ///
    /// # Errors
    /// `Invalid` arguments or an unknown tool, and the errors of the page's function.
    pub(crate) async fn person_project_tool(
        self: &Arc<Self>,
        ctx: &AuthContext,
        project: &ProjectCaller,
        sandbox: &SandboxRecord,
        name: &str,
        args: &Value,
    ) -> AppResult<Value> {
        let sid = sandbox.id.as_str();
        match name {
            "task_delivery" => {
                let task = self.agent_task_ref(project, args).await?;
                record(&self.task_delivery(ctx, task.id.as_str()).await?)
            }
            "task_restart" | "task_pause" | "task_unpause" | "task_start_now" => {
                let task = self.agent_task_ref(project, args).await?;
                let id = task.id.as_str();
                let summary = match name {
                    // Continue takes a resting task; what else the task is doing decides
                    // what its person is told to do instead. Whose task it is comes first, as
                    // it does in Continue: advice is for a task the person may change (review
                    // of slice 2, R3-1). The state is the one that check read.
                    "task_restart" if !task.status.is_resting() => {
                        let task = self.load_task(ctx, id, Action::Work).await?;
                        if !task.status.is_resting() {
                            return Err(not_resting(task.number, task.status));
                        }
                        self.continue_task(ctx, id, &ContinueTask { account_id: None })
                            .await
                            .map_err(|err| restart_refusal(err, task.number))?
                    }
                    "task_restart" => self
                        .continue_task(ctx, id, &ContinueTask { account_id: None })
                        .await
                        .map_err(|err| restart_refusal(err, task.number))?,
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
                // An agent deploys dev-stage environments only (SPEC-257); without a typed
                // confirmation a production-stage one is refused by the deploy anyway.
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
            // Project variables (SPEC-275 VAR-005): plain values, shown as they are.
            "variables_list" => record(&self.project_variables(ctx, sid).await?),
            other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PERSON_TOOLS, descriptors, not_resting, restart_refusal, subject};
    use crate::error::AppError;
    use serde_json::json;
    use yulius_domain::task::TaskStatus;

    /// SPEC-487 SAR-005 (review of slice 2, F9 and R2-3): what `task_restart` says for a task
    /// that is not resting depends on what the task is doing. "Stop it first" for a task that
    /// waits to start would cancel it, and `task_start_now` takes a chained or prepared task
    /// only: a queued one starts by itself.
    #[test]
    fn a_task_that_is_not_resting_is_told_what_to_do_by_its_state() {
        let refused = |status: TaskStatus| not_resting(7, status).public_message();
        for live in [
            TaskStatus::Running,
            TaskStatus::Starting,
            TaskStatus::Switching,
            TaskStatus::Stopping,
        ] {
            let said = refused(live);
            assert!(
                said.starts_with("the task is ")
                    && said.ends_with(": stop it first (task_stop), then restart it"),
                "{said}"
            );
        }
        for waiting in [TaskStatus::Chained, TaskStatus::Prepared] {
            let said = refused(waiting);
            assert!(
                said.contains("task_start_now starts it now") && !said.contains("task_stop"),
                "{said}"
            );
        }
        for by_itself in [TaskStatus::Queued, TaskStatus::WaitingCapacity] {
            let said = refused(by_itself);
            assert!(
                said.contains("by itself")
                    && !said.contains("task_stop")
                    && !said.contains("task_start_now"),
                "{said}"
            );
        }
    }

    /// SPEC-487 SAR-005 (review of slice 2, F12 and R2-4): the onboarding of a project that
    /// has not started it is a person's to start; the Superagent is told so without the words
    /// about tokens that the page's refusal has for a command line. The refusal is recognised
    /// by the one constant the page's check answers with.
    #[test]
    fn a_refused_continue_of_an_onboarding_says_who_starts_it() {
        let onboarding = AppError::Forbidden(crate::onboarding::STARTS_WITH_A_PERSON.into());
        let said = restart_refusal(onboarding, 1).public_message();
        assert!(
            said.contains("a person starts it with Start onboarding on the project's page")
                && !said.contains("token"),
            "{said}"
        );
        // Every other refusal is the page's, as it is.
        let other = restart_refusal(AppError::Forbidden("Task #7 is @carol's".into()), 7);
        assert_eq!(other.public_message(), "Task #7 is @carol's");
    }

    /// SPEC-487 SAR-002 (review of slice 2, R2-1): the audit entry of a call names the task,
    /// run or environment its tool takes, when the argument has the shape of one, and nothing
    /// else: never text of the caller's choosing.
    #[test]
    fn a_calls_subject_is_its_task_run_or_environment() {
        assert_eq!(
            subject("task_pause", &json!({"number": 12})),
            json!({"task": 12})
        );
        assert_eq!(
            subject("task_restart", &json!({"task_id": "t-1"})),
            json!({"task": "t-1"})
        );
        assert_eq!(
            subject("ci_cancel", &json!({"run_id": "r-9"})),
            json!({"run": "r-9"})
        );
        assert_eq!(
            subject("cd_deploy", &json!({"environment": "dev", "confirm": "x"})),
            json!({"environment": "dev"})
        );
        // An argument that is not the tool's, or has not the shape of an id or a name, is
        // left out, however long it is.
        let long = "x".repeat(1 << 20);
        for (tool, args, about) in [
            (
                "task_pause",
                json!({"number": 1, "environment": long.clone()}),
                json!({"task": 1}),
            ),
            (
                "task_pause",
                json!({"number": 1, "run_id": "r-9"}),
                json!({"task": 1}),
            ),
            (
                "ci_cancel",
                json!({"run_id": " r-9 ", "task_id": long.clone()}),
                json!({"run": "r-9"}),
            ),
        ] {
            assert_eq!(subject(tool, &args), about, "{tool}");
        }
        for (tool, args) in [
            ("task_pause", json!({"task_id": long.clone()})),
            ("task_pause", json!({"task_id": "a b"})),
            ("task_pause", json!({"number": "12"})),
            ("ci_rerun", json!({"run_id": long.clone()})),
            ("ci_rerun", json!({"run_id": "r\n9"})),
            ("cd_deploy", json!({"environment": long})),
            ("cd_deploy", json!({"environment": "Dev Prod"})),
            ("cd_status", json!({"run_id": "r-9"})),
            ("task_pause", json!(null)),
        ] {
            assert_eq!(
                subject(tool, &args),
                json!({}),
                "{tool} {}",
                args.to_string().len()
            );
        }
    }

    #[test]
    fn every_persons_tool_is_described_once_in_order() {
        let described: Vec<String> = descriptors().into_iter().map(|tool| tool.name).collect();
        let listed: Vec<&str> = PERSON_TOOLS.iter().map(|(name, _)| *name).collect();
        assert_eq!(described, listed);
    }
}
