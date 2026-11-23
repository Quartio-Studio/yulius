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

/// The `cause` of a run's Needs you request: `command-run:<id>`. An agent's `needs_you` names
/// only a cause of `needs_you::CAUSES`, so it never writes one (SPEC-626 BRK-015).
const RUN_CAUSE: &str = "command-run:";

/// The run a request's `cause` names, if it is a run's.
#[must_use]
pub fn run_of_cause(cause: &str) -> Option<&str> {
    cause.strip_prefix(RUN_CAUSE).filter(|id| !id.is_empty())
}

/// Whether a request shows `line` whole.
fn shown_whole(line: &str) -> bool {
    line.chars().count() <= SHOWN_LINE
}

/// The command line as a request shows it: cut at a character, saying so.
fn shown_line(line: &str) -> String {
    if shown_whole(line) {
        return line.to_owned();
    }
    let cut: String = line.chars().take(SHOWN_LINE).collect();
    format!("{cut} … (cut here: the whole line is on the Secrets page)")
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
            // A line this notice cannot show whole is approved where it is read whole.
            problem: Some(if shown_whole(&run.line) {
                format!(
                    "{} — it waits {} min; no answer is a no. Reply approve or reject to this \
                     notice, or answer on the Secrets page.",
                    run.line, def.approval_minutes
                )
            } else {
                format!(
                    "{} — it waits {} min; no answer is a no. Approve it on the Secrets page, \
                     where the whole line is; reply reject to this notice to refuse it.",
                    shown_line(&run.line),
                    def.approval_minutes
                )
            }),
            decision: true,
            link: Some(self.command_runs_link(sandbox, &run.id)),
            at: run.created_at.0,
            cause: Some(format!("{RUN_CAUSE}{}", run.id)),
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
            self.end_run_request(run).await;
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
        self.end_run_request(&run).await;
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
        // As the write left it, without reading it again (review round 1 of slice 3).
        let answered = CommandRunRecord {
            state: if approve { "running" } else { "rejected" }.into(),
            answered_by: Some(login.clone()),
            started_at: approve.then_some(now),
            ended_at: (!approve).then_some(now),
            reason: (!approve).then(|| "a person rejected it".to_owned()),
            ..run
        };
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

    /// Ends `run`'s Needs you request while it is still that run's own (its `cause` names the
    /// run): a request the agent opened since stays (review round 1 of slice 3).
    async fn end_run_request(&self, run: &CommandRunRecord) {
        let own = format!("{RUN_CAUSE}{}", run.id);
        if let Ok(Some(request)) = self.deps.store.task_needs_you(&run.task_id).await
            && request.cause.as_deref() == Some(own.as_str())
        {
            self.end_needs_you_asked_at(&run.task_id, request.at, None)
                .await;
        }
    }

    /// BRK-015: a Telegram reply to the notice of run `run_id`'s request answers that run, as
    /// `person`, who must manage the project's secrets: `approve` or `reject`; anything else is
    /// no answer and goes nowhere. A run whose line the notice could not show whole is approved
    /// on the Secrets page only.
    pub(crate) async fn telegram_command_answer(
        self: &Arc<Self>,
        person: &UserRecord,
        task: &TaskRecord,
        run_id: &str,
        text: &str,
    ) -> String {
        let word = text.trim().to_lowercase();
        let approve = match word.as_str() {
            "approve" => true,
            "reject" => false,
            _ => {
                return "This notice asks to approve a command run: reply approve or reject, or \
                        answer on the Secrets page."
                    .into();
            }
        };
        let refused = |why: &str| format!("Not answered: {why}");
        let run = match self.deps.store.command_run(run_id).await {
            Ok(Some(run)) if run.task_id == task.id => run,
            Ok(_) => return refused("that run is gone"),
            Err(err) => return refused(&err.public_message()),
        };
        let sandbox = match self
            .require(person, &task.sandbox_id, Action::Configure)
            .await
        {
            Ok((sandbox, _)) => sandbox,
            Err(AppError::Forbidden(_) | AppError::NotFound) => {
                return refused(
                    "only people who manage this project's secrets answer its command runs",
                );
            }
            Err(err) => return refused(&err.public_message()),
        };
        if approve && run.state == "waiting" && !shown_whole(&run.line) {
            return refused(
                "this notice could not show the whole command line: approve it on the Secrets \
                 page, where it is whole",
            );
        }
        match self
            .answer_command_run_as(&sandbox, person, &run.id, approve, "telegram")
            .await
        {
            Ok(_) if approve => format!(
                "Approved: task #{}'s run of {} starts now.",
                run.task_number, run.command
            ),
            Ok(_) => format!(
                "Rejected: task #{}'s run of {} runs nothing.",
                run.task_number, run.command
            ),
            Err(err) => refused(&err.public_message()),
        }
    }
}
