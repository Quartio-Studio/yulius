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
use crate::ports::store_ext::{CommandRecord, CommandRunEnd, CommandRunRecord, RunInsert};

/// How long a call waits for a run's end before it answers its state (ADR-0227's bound for MCP
/// clients).
pub const RUN_WAIT: Duration = Duration::from_secs(240);
/// Runs of a project's commands at once (RUN-003, review round 1 of slice 2): each is a process
/// outside the agents' sandbox and two read-backs in the server. More are refused, audited.
pub const MAX_RUNNING: u32 = 4;

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
    /// The run; none: the task's newest run, for a call cut before it told the id.
    #[serde(default)]
    id: Option<String>,
}

/// The states a run no longer leaves.
fn ended(state: &str) -> bool {
    !matches!(state, "waiting" | "approved" | "running")
}

/// What an agent is told of a run.
fn run_view(run: &CommandRunRecord) -> Value {
    // Which run it is, also for a call cut before it told the id (review round 2 of slice 2).
    let mut view = json!({
        "id": run.id,
        "command": run.command,
        "line": run.line,
        "asked_at": crate::auth::rfc3339(run.created_at),
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
            return refuse(
                "no command of that name in this project (commands_list lists them)".into(),
            )
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
        let ask = dom::needs_approval(approve, &def.send_flags, &call.args, call.changes);
        // One run of a task waits for a person at a time: its request is the task's Needs you.
        if ask
            && let Some(waiting) = self
                .deps
                .store
                .waiting_command_run_of_task(&task.id)
                .await?
        {
            return refuse(format!(
                "run {} of this task waits for a person's approval already: wait for its answer \
                 (command_result)",
                waiting.id
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
        let minutes = i64::from(def.approval_minutes);
        let minute = i64::try_from(self.config.command_minute_ms).unwrap_or(60_000);
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
            state: if ask { "waiting" } else { "running" }.into(),
            reason: None,
            deadline: ask.then(|| now.plus_millis(minutes.saturating_mul(minute))),
            answered_by: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            created_at: now,
            started_at: (!ask).then_some(now),
            ended_at: None,
        };
        // Stored and run on a task of its own, so a call cut meanwhile leaves no row without an
        // executor (review round 1 of slice 2).
        let (stored_tx, stored) = tokio::sync::oneshot::channel();
        let (app, id, place) = (Arc::clone(self), run.id.clone(), sandbox.clone());
        tokio::spawn(async move {
            let inserted = app.deps.store.insert_command_run(&run, MAX_RUNNING).await;
            let go = matches!(inserted, Ok(RunInsert::Stored));
            let _ = stored_tx.send(inserted);
            if go {
                crate::race::reached(crate::race::Point::CommandRunStored, &app.config.data_dir);
                if ask {
                    app.ask_command_approval(&place, &run, &def).await;
                } else {
                    app.run_command_to_its_end(place, run).await;
                }
            }
        });
        match stored.await.map_err(AppError::internal)? {
            Ok(RunInsert::Stored) => self.wait_command_run(&id).await,
            Ok(RunInsert::Busy) => {
                refuse(format!(
                    "{MAX_RUNNING} runs of this project's commands run already: wait for one to \
                     end (command_result) and run it again"
                ))
                .await
            }
            Ok(RunInsert::TaskWaits) => {
                refuse(
                    "a run of this task waits for a person's approval already: wait for its \
                     answer (command_result)"
                        .into(),
                )
                .await
            }
            Err(err) => Err(err),
        }
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
        let run = match &call.id {
            Some(id) => self.deps.store.command_run(id).await?,
            None => {
                self.deps
                    .store
                    .latest_command_run_of_task(&ctx.task.id)
                    .await?
            }
        };
        let Some(run) = run.filter(|r| r.task_id == ctx.task.id) else {
            return Err(AppError::NotFound);
        };
        self.wait_command_run(&run.id).await
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

    /// Runs a stored `running` run to its end, then wakes the calls waiting on it and the open
    /// pages (BRK-009..BRK-012).
    pub(crate) async fn run_command_to_its_end(
        &self,
        sandbox: SandboxRecord,
        run: CommandRunRecord,
    ) {
        self.execute_command_run(&sandbox, &run).await;
        self.command_runs_changed.notify_waiters();
        self.announce_commands(&sandbox.id);
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
        let stdout = {
            let shown = self.redact_for_sandbox(&sandbox.id, &outcome.stdout).await;
            // What was not read back counts as left out too.
            let unread = outcome.sizes.0.saturating_sub(outcome.stdout.len() as u64);
            dom::shown_output(&shown, shown.len() as u64 + unread)
        };
        let stderr = {
            let shown = self.redact_for_sandbox(&sandbox.id, &outcome.stderr).await;
            // What was not read back counts as left out too.
            let unread = outcome.sizes.1.saturating_sub(outcome.stderr.len() as u64);
            dom::shown_output(&shown, shown.len() as u64 + unread)
        };
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
            // Redacted, then cut, as the arguments (review round 2 of slice 2).
            "command": cut(self.redact_for_sandbox(&sandbox.id, name).await, 40),
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

    pub(crate) async fn audit_command_run(
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
        // A run that waited past its deadline while Yulius was down is a no (BRK-016).
        self.expire_due_command_runs().await;
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
