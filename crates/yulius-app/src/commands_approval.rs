//! Approving brokered runs (SPEC-626 BRK-013..BRK-016): a run that needs a person waits, with its
//! task in Needs you showing the full command line; a person who manages the project's secrets
//! approves or rejects it in a browser session or by replying to its Telegram notice; no answer
//! by its deadline is a no. Every answer is one conditional write that holds only before the
//! deadline, so two answers never both act and a late one does nothing.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use yulius_contracts::api::commands::{CommandDefinition, CommandRunList, CommandRunView};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;

use crate::auth::{AuthContext, rfc3339};
use crate::commands_run::MAX_RUNNING;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, TaskRecord, UserRecord};
use crate::needs_you::{Named, NeedsYouRecord};
use crate::ports::store_ext::CommandRunRecord;

/// Ended runs a project's list shows besides those that wait or run.
const LISTED: u32 = 50;
/// The longest command line a Needs you request carries; the whole line is on the Secrets page.
const SHOWN_LINE: usize = 380;

/// The command line as a request shows it: cut at a character, saying so.
fn shown_line(line: &str) -> String {
    if line.chars().count() <= SHOWN_LINE {
        return line.to_owned();
    }
    let cut: String = line.chars().take(SHOWN_LINE).collect();
    format!("{cut} … (the whole line is on the Secrets page)")
}

impl App {
    /// The Secrets page's commands section of `sandbox`, at run `id`.
    fn command_runs_link(&self, sandbox: &SandboxRecord, id: &str) -> String {
        format!(
            "{}/s/{}/secrets?section=commands&run={id}",
            self.config.public_url.trim_end_matches('/'),
            sandbox.slug
        )
    }

    /// BRK-014: a stored `waiting` run asks its task's people. Its task is in Needs you with the
    /// full command line, the minutes it waits and how to answer, as a request asked at the
    /// run's own time, so a Telegram reply to its notice answers this run and no other. The
    /// request takes the place of one the agent opened; the run ends it once it is answered or
    /// expired.
    pub(crate) async fn ask_command_approval(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        run: &CommandRunRecord,
        def: &CommandDefinition,
    ) {
        let says = if run.changes {
            ", saying it changes something"
        } else {
            ""
        };
        let record = NeedsYouRecord {
            action: format!(
                "Approve or reject task #{}'s run of the command {}",
                run.task_number, run.command
            ),
            happened: Some(format!(
                "Task #{} asked to run the brokered command {}{says}.",
                run.task_number, run.command
            )),
            problem: Some(format!(
                "{} — it waits {} min; no answer is a no. Reply approve or reject to this \
                 notice, or answer on the Secrets page.",
                shown_line(&run.line),
                def.approval_minutes
            )),
            decision: true,
            link: Some(self.command_runs_link(sandbox, &run.id)),
            at: run.created_at.0,
            cause: None,
            named: Named::default(),
        };
        match self
            .deps
            .store
            .set_task_needs_you(&run.task_id, Some(&record), run.created_at)
            .await
        {
            Ok(_) => self.publish_task(&run.task_id).await,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "a brokered run's approval request was not opened");
            }
        }
        let detail = json!({
            "task": format!("#{}", run.task_number),
            "command": run.command,
            "line": run.line,
            "run": run.id,
            "minutes": def.approval_minutes,
        });
        self.audit_command_run(
            sandbox,
            &format!("task:{}", run.task_number),
            "command.ask",
            &detail,
        )
        .await;
        self.announce_commands(&sandbox.id);
        // Its deadline ends it; the background tick does after a restart (BRK-016). Checked again
        // until it no longer waits: the deadline is the clock's, which a sleep does not measure.
        if let Some(deadline) = run.deadline {
            let (app, id) = (Arc::clone(self), run.id.clone());
            tokio::spawn(async move {
                loop {
                    let left = u64::try_from(deadline.0.saturating_sub(app.now().0)).unwrap_or(0);
                    tokio::time::sleep(Duration::from_millis(left.max(100))).await;
                    app.expire_due_command_runs().await;
                    match app.deps.store.command_run(&id).await {
                        Ok(Some(run)) if run.state == "waiting" => {}
                        _ => break,
                    }
                }
            });
        }
    }

    /// BRK-016: every waiting run past its deadline ends `expired`, never runs, and ends its
    /// task's request. Called at each run's deadline, by the background tick and at start.
    pub async fn expire_due_command_runs(&self) {
        let due = match self.deps.store.expire_command_runs(self.now()).await {
            Ok(due) => due,
            Err(err) => {
                tracing::warn!(error = %err, "brokered runs past their deadline could not be expired");
                return;
            }
        };
        for run in &due {
            self.end_needs_you_asked_at(&run.task_id, run.created_at.0, None)
                .await;
            if let Ok(Some(sandbox)) = self.deps.store.sandbox(&run.sandbox_id).await {
                let detail = json!({
                    "task": format!("#{}", run.task_number),
                    "command": run.command,
                    "line": run.line,
                    "run": run.id,
                });
                self.audit_command_run(&sandbox, "yulius", "command.expired", &detail)
                    .await;
                self.announce_commands(&sandbox.id);
            }
        }
        if !due.is_empty() {
            self.command_runs_changed.notify_waiters();
        }
    }

    /// `GET /api/sandboxes/{sid}/command-runs` (BRK-015): for those who may view the project.
    ///
    /// # Errors
    /// Access errors.
    pub async fn command_runs(&self, ctx: &AuthContext, sid: &str) -> AppResult<CommandRunList> {
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require_for(ctx, &sid, Action::View).await?;
        let answers = ctx.is_browser() && level.allows(Action::Configure);
        let mut items = Vec::new();
        for run in self.deps.store.command_runs_in(&sandbox.id, LISTED).await? {
            items.push(self.command_run_view(&run, answers));
        }
        Ok(CommandRunList { items })
    }

    fn command_run_view(&self, run: &CommandRunRecord, answers: bool) -> CommandRunView {
        let waits = run.state == "waiting" && run.deadline.is_some_and(|d| d > self.now());
        CommandRunView {
            id: run.id.clone(),
            command: run.command.clone(),
            task_number: run.task_number,
            line: run.line.clone(),
            changes: run.changes,
            state: run.state.clone(),
            reason: run.reason.clone(),
            deadline: run.deadline.map(rfc3339),
            answered_by: run.answered_by.clone(),
            exit_code: run.exit_code,
            created_at: rfc3339(run.created_at),
            ended_at: run.ended_at.map(rfc3339),
            can_answer: answers && waits,
        }
    }

    /// `POST /api/sandboxes/{sid}/command-runs/{id}` (BRK-015): a person who manages the
    /// project's secrets approves or rejects a waiting run, in a browser session.
    ///
    /// # Errors
    /// `Forbidden` for an API token or below write access; `NotFound` for another project's
    /// run; `Conflict` when it no longer waits, its time ran out, or approved it would pass the
    /// project's runs at once.
    pub async fn answer_command_run(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
        id: &str,
        approve: bool,
    ) -> AppResult<CommandRunView> {
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "a run is answered in Yulius in a browser, or by a reply to its Telegram \
                 notice, not with an API token"
                    .into(),
            ));
        }
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (sandbox, _) = self.require_for(ctx, &sid, Action::Configure).await?;
        let run = self
            .answer_command_run_as(&sandbox, &ctx.user, id, approve, "browser")
            .await?;
        Ok(self.command_run_view(&run, true))
    }

    /// Answers waiting run `id` of `sandbox` as `person`, who may manage its secrets (checked by
    /// the caller), through `via` (BRK-015): one conditional write before the deadline; an
    /// approved run runs on its own task.
    pub(crate) async fn answer_command_run_as(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        person: &UserRecord,
        id: &str,
        approve: bool,
        via: &str,
    ) -> AppResult<CommandRunRecord> {
        let store = &self.deps.store;
        let run = store
            .command_run(id)
            .await?
            .filter(|r| r.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)?;
        let login = person.label();
        let now = self.now();
        if !store
            .answer_command_run(id, approve, &login, now, MAX_RUNNING)
            .await?
        {
            let current = store.command_run(id).await?.unwrap_or(run);
            return Err(AppError::Conflict(match current.state.as_str() {
                "waiting" if current.deadline.is_some_and(|d| d <= now) => {
                    self.expire_due_command_runs().await;
                    "its time ran out, which is a no: the agent asks again".to_owned()
                }
                "waiting" => format!(
                    "{MAX_RUNNING} runs of this project's commands run now: approve it again \
                     once one ended"
                ),
                state => format!("this run is {state} already"),
            }));
        }
        self.end_needs_you_asked_at(&run.task_id, run.created_at.0, None)
            .await;
        let detail = json!({
            "task": format!("#{}", run.task_number),
            "command": run.command,
            "line": run.line,
            "run": run.id,
            "via": via,
        });
        let action = if approve {
            "command.approve"
        } else {
            "command.reject"
        };
        self.audit_command_run(sandbox, &login, action, &detail)
            .await;
        let answered = store.command_run(id).await?.ok_or(AppError::NotFound)?;
        if approve {
            let (app, place, running) = (Arc::clone(self), sandbox.clone(), answered.clone());
            tokio::spawn(async move {
                app.run_command_to_its_end(place, running).await;
            });
        } else {
            self.command_runs_changed.notify_waiters();
        }
        self.announce_commands(&sandbox.id);
        Ok(answered)
    }

    /// BRK-015: a Telegram reply `approve` or `reject` to the notice of a run's request answers
    /// that run, the one asked at the notice's time, as `person`, who must manage the project's
    /// secrets. `None` when the notice is no run's: the reply goes to the task as before.
    pub(crate) async fn telegram_command_answer(
        self: &Arc<Self>,
        person: &UserRecord,
        task: &TaskRecord,
        asked_at: Option<i64>,
        approve: bool,
    ) -> Option<String> {
        let asked_at = asked_at?;
        let runs = self
            .deps
            .store
            .command_runs_in(&task.sandbox_id, 200)
            .await
            .ok()?;
        let run = runs
            .into_iter()
            .find(|r| r.task_id == task.id && r.created_at.0 == asked_at)?;
        let refused = |why: String| Some(format!("Not answered: {why}"));
        let sandbox = match self
            .require(person, &task.sandbox_id, Action::Configure)
            .await
        {
            Ok((sandbox, _)) => sandbox,
            Err(_) => {
                return refused(
                    "only people who manage this project's secrets answer its command runs".into(),
                );
            }
        };
        match self
            .answer_command_run_as(&sandbox, person, &run.id, approve, "telegram")
            .await
        {
            Ok(_) if approve => Some(format!(
                "Approved: task #{}'s run of {} starts now.",
                run.task_number, run.command
            )),
            Ok(_) => Some(format!(
                "Rejected: task #{}'s run of {} runs nothing.",
                run.task_number, run.command
            )),
            Err(err) => refused(err.public_message()),
        }
    }
}
