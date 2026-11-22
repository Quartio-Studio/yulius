//! Running brokered commands (SPEC-626 BRK-005..BRK-012): `command_run` and `command_result`
//! for task agents. A run is a row of `command_runs`; it goes on whatever happens to the call
//! that started it, and its end wakes every call waiting on it.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::commands::CommandDefinition;
use yulius_domain::commands as dom;
use zeroize::Zeroizing;

use crate::ci::brokered::BrokeredRun;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::store_ext::{CommandRecord, CommandRunEnd, CommandRunRecord};

/// How long a call waits for a run's end before it answers its state (ADR-0227's bound for MCP
/// clients).
pub const RUN_WAIT: Duration = Duration::from_secs(240);

/// `command_run`'s arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    name: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    changes: bool,
}

/// `command_result`'s arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultArgs {
    id: String,
}

/// The states a run no longer leaves.
fn ended(state: &str) -> bool {
    !matches!(state, "waiting" | "approved" | "running")
}

/// What an agent is told of a run.
fn run_view(run: &CommandRunRecord) -> Value {
    let mut view = json!({
        "id": run.id,
        "command": run.command,
        "state": run.state,
    });
    if let Some(reason) = &run.reason {
        view["reason"] = json!(reason);
    }
    if let Some(deadline) = run.deadline.filter(|_| run.state == "waiting") {
        view["approval_until"] = json!(crate::auth::rfc3339(deadline));
    }
    if ended(&run.state) {
        view["exit_code"] = json!(run.exit_code);
        view["stdout"] = json!(run.stdout);
        view["stderr"] = json!(run.stderr);
    } else {
        view["note"] = json!(
            "Not ended yet: call command_result with this id to wait for it (yulius run does)."
        );
    }
    view
}

impl App {
    /// `command_run` (BRK-005..BRK-013): checks the call, stores the run and starts it (or, when
    /// it needs a person, waits for one), then waits up to [`RUN_WAIT`] for its end.
    ///
    /// # Errors
    /// A refusal with its reason (audited `command.refused`), store failures.
    pub(crate) async fn tool_command_run(
        self: &Arc<Self>,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let task = &ctx.task;
        let call: RunArgs = serde_json::from_value(args.clone()).map_err(|e| {
            AppError::Invalid(format!(
                "command_run takes name, args (a list of strings) and changes: {e}"
            ))
        })?;
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let refuse = |reason: String| {
            self.refuse_command_run(&sandbox, task.number, &call.name, &call.args, reason)
        };
        // Another project's command answers as an unknown one (BRK-005).
        let Some(record) = self
            .deps
            .store
            .command_named(&sandbox.id, &call.name)
            .await?
        else {
            let shown: String = call.name.chars().take(40).collect();
            return refuse(format!(
                "no command {shown:?} in this project (commands_list lists them)"
            ))
            .await;
        };
        let def: CommandDefinition =
            serde_json::from_str(&record.definition).map_err(AppError::internal)?;
        if !def.tasks.is_empty() && !def.tasks.contains(&task.number) {
            return refuse(format!("task #{} may not run {}", task.number, def.name)).await;
        }
        // What a person approves is all that runs: nothing they cannot see (BRK-006).
        let hidden = call
            .args
            .iter()
            .flat_map(|a| a.chars())
            .find(|c| crate::cross_sandbox::unseen(*c));
        if let Some(c) = hidden {
            return refuse(format!(
                "an argument holds U+{:04X}, a control or invisible character",
                u32::from(c)
            ))
            .await;
        }
        if let Some(reason) =
            dom::args_refusal(&call.args, &def.refuse, &def.subcommands, def.slashes)
        {
            return refuse(reason).await;
        }
        if self.command_secret(&record, &def.secret).await?.is_none() {
            return refuse(format!(
                "{} is not set up: this project has no agent secret {} it may use (not a personal or production one)",
                def.name, def.secret
            ))
            .await;
        }
        let approve = dom::Approve::parse(&def.approve).unwrap_or(dom::Approve::Always);
        if dom::needs_approval(approve, &def.send_flags, &call.args, call.changes) {
            // Slice 2: approvals arrive with slice 3 (SPEC-626 BRK-014).
            return refuse(format!(
                "this run of {} needs a person's approval, which Yulius does not ask for yet",
                def.name
            ))
            .await;
        }
        let line = self
            .redact_for_sandbox(
                &sandbox.id,
                &dom::command_line(&def.binary, &def.fixed, &call.args),
            )
            .await;
        let now = self.now();
        let run = CommandRunRecord {
            id: self.new_command_run_id(),
            sandbox_id: sandbox.id.clone(),
            command_id: record.id.clone(),
            command: def.name.clone(),
            command_updated_at: record.updated_at,
            task_id: task.id.clone(),
            task_number: task.number,
            args: call.args.clone(),
            line,
            changes: call.changes,
            state: "running".into(),
            reason: None,
            deadline: None,
            answered_by: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            created_at: now,
            started_at: Some(now),
            ended_at: None,
        };
        self.deps.store.insert_command_run(&run).await?;
        self.spawn_command_run(sandbox, run.clone());
        self.wait_command_run(&run.id).await
    }

    /// `command_result` (BRK-014): the state of a run this task started, waiting up to
    /// [`RUN_WAIT`] for its end.
    ///
    /// # Errors
    /// `NotFound` for another task's run.
    pub(crate) async fn tool_command_result(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let call: ResultArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("command_result takes id: {e}")))?;
        let run = self.deps.store.command_run(&call.id).await?;
        if !run.is_some_and(|r| r.task_id == ctx.task.id) {
            return Err(AppError::NotFound);
        }
        self.wait_command_run(&call.id).await
    }

    /// Waits up to [`RUN_WAIT`] (the configuration's `command_wait_ms`) for run `id` to end, then
    /// answers its view.
    async fn wait_command_run(&self, id: &str) -> AppResult<Value> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.config.command_wait_ms);
        loop {
            // Asked before reading, so an end between the read and the wait still wakes it.
            let woken = self.command_runs_changed.notified();
            let run = self
                .deps
                .store
                .command_run(id)
                .await?
                .ok_or(AppError::NotFound)?;
            if ended(&run.state) || tokio::time::Instant::now() >= deadline {
                return Ok(run_view(&run));
            }
            let _ = tokio::time::timeout_at(deadline, woken).await;
        }
    }

    fn new_command_run_id(&self) -> String {
        let hex: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("run-{hex}")
    }

    /// Runs `run` on its own task, so a cut call never leaves it half done (BRK-009..BRK-012).
    pub(crate) fn spawn_command_run(
        self: &Arc<Self>,
        sandbox: SandboxRecord,
        run: CommandRunRecord,
    ) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            app.execute_command_run(&sandbox, &run).await;
            app.command_runs_changed.notify_waiters();
            app.announce_commands(&sandbox.id);
        });
    }

    /// The command as it is now, when it is still what the run was asked against: a command
    /// removed or changed since runs nothing (BRK-003).
    async fn command_as_asked(
        &self,
        sandbox: &SandboxRecord,
        run: &CommandRunRecord,
    ) -> Result<(CommandRecord, CommandDefinition), String> {
        let record = match self.deps.store.command(&sandbox.id, &run.command_id).await {
            Ok(Some(record)) => record,
            Ok(None) => return Err(format!("nothing ran: {} was removed", run.command)),
            Err(err) => return Err(format!("nothing ran: {}", err.public_message())),
        };
        if record.updated_at != run.command_updated_at {
            return Err(format!(
                "nothing ran: {} changed since this run was asked; run it again",
                run.command
            ));
        }
        let def = serde_json::from_str(&record.definition)
            .map_err(|_| format!("nothing ran: {} could not be read", run.command))?;
        Ok((record, def))
    }

    async fn execute_command_run(&self, sandbox: &SandboxRecord, run: &CommandRunRecord) {
        let started = self.now();
        let asked = match self.command_as_asked(sandbox, run).await {
            Ok((record, def)) => self.command_key(&record, &def).await.map(|key| (def, key)),
            Err(reason) => Err(reason),
        };
        let outcome = match asked {
            Err(reason) => crate::ci::brokered::BrokeredOutcome::failed(reason),
            Ok((def, (key, secret))) => {
                self.run_brokered(
                    sandbox,
                    BrokeredRun {
                        id: &run.id,
                        binary: &def.binary,
                        fixed: &def.fixed,
                        args: &run.args,
                        env: &def.env,
                        key,
                        secret,
                        timeout: Duration::from_secs(u64::from(def.timeout_seconds)),
                    },
                )
                .await
            }
        };
        // Redacted whole, then cut: no part of a value survives at the cut (BRK-011).
        let stdout =
            dom::shown_output(&self.redact_for_sandbox(&sandbox.id, &outcome.stdout).await);
        let stderr =
            dom::shown_output(&self.redact_for_sandbox(&sandbox.id, &outcome.stderr).await);
        let end = CommandRunEnd {
            state: outcome.state.to_owned(),
            reason: outcome.reason.clone(),
            exit_code: outcome.exit_code,
            stdout,
            stderr,
            at: self.now(),
        };
        if let Err(err) = self
            .deps
            .store
            .end_command_run(&run.id, "running", &end)
            .await
        {
            tracing::error!(run = %run.id, error = %err, "a brokered run's end was not recorded");
        }
        let took = end.at.millis_since(started);
        let detail = json!({
            "task": format!("#{}", run.task_number),
            "command": run.command,
            "line": run.line,
            "state": end.state,
            "exit_code": end.exit_code,
            "reason": end.reason,
            "ms": took,
            "approved_by": run.answered_by,
        });
        self.audit_command_run(
            sandbox,
            &format!("task:{}", run.task_number),
            "command.run",
            &detail,
        )
        .await;
    }

    /// The key file's text and the value for `{secret}`, read now: a secret removed or made a
    /// production one since the run was asked is refused (BRK-003).
    async fn command_key(
        &self,
        command: &CommandRecord,
        def: &CommandDefinition,
    ) -> Result<(Zeroizing<String>, Zeroizing<String>), String> {
        let not_set_up = || {
            format!(
                "nothing ran: {} has no agent secret {} it may use",
                def.name, def.secret
            )
        };
        let record = match self.command_secret(command, &def.secret).await {
            Ok(Some(record)) => record,
            Ok(None) => return Err(not_set_up()),
            Err(err) => return Err(format!("nothing ran: {}", err.public_message())),
        };
        match self.agent_payload(&record) {
            Ok(Some(payload)) => {
                let value = Zeroizing::new(payload.value.clone());
                let key = Zeroizing::new(
                    yulius_domain::vault::dotenv::key_block(&value)
                        .unwrap_or_else(|| value.to_string()),
                );
                Ok((key, value))
            }
            Ok(None) => Err(not_set_up()),
            Err(err) => Err(format!(
                "nothing ran: the secret could not be read ({})",
                err.public_message()
            )),
        }
    }

    /// Records and audits a refused run, and returns the refusal (BRK-005).
    async fn refuse_command_run(
        &self,
        sandbox: &SandboxRecord,
        task_number: u32,
        name: &str,
        args: &[String],
        reason: String,
    ) -> AppResult<Value> {
        let cut = |text: String, max: usize| text.chars().take(max).collect::<String>();
        let shown = self.redact_for_sandbox(&sandbox.id, &args.join(" ")).await;
        let detail = json!({
            "task": format!("#{task_number}"),
            "command": cut(name.to_owned(), 40),
            "args": cut(shown, 2000),
            "reason": reason,
        });
        self.audit_command_run(
            sandbox,
            &format!("task:{task_number}"),
            "command.refused",
            &detail,
        )
        .await;
        Err(AppError::Forbidden(format!("command refused: {reason}")))
    }

    async fn audit_command_run(
        &self,
        sandbox: &SandboxRecord,
        actor: &str,
        action: &str,
        detail: &Value,
    ) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                actor,
                action,
                Some(sandbox.id.as_str()),
                Some(&detail.to_string()),
            )
            .await
        {
            tracing::warn!(error = %err, action, "a brokered run's audit entry was not written");
        }
    }

    /// At start, before serving: runs recorded `running` have no executor; their process hosts
    /// are ended, their files (the key's included) removed and they are `lost` (BRK-009).
    ///
    /// # Errors
    /// Store failures.
    pub async fn reconcile_command_runs(&self) -> AppResult<u32> {
        let mut lost = 0;
        for run in self.deps.store.command_runs_in_state("running").await? {
            self.remove_command_run_files(&run.sandbox_id, &run.id)
                .await;
            let end = CommandRunEnd {
                state: "lost".into(),
                reason: Some(
                    "Yulius restarted while it ran: it was stopped, its outcome is unknown".into(),
                ),
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                at: self.now(),
            };
            if self
                .deps
                .store
                .end_command_run(&run.id, "running", &end)
                .await?
            {
                lost += 1;
            }
        }
        Ok(lost)
    }
}
