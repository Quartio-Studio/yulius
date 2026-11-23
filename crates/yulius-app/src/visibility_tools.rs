//! What the Superagent sees of its project's work (SPEC-487 slice 10b, VIS-001 to VIS-004; wall
//! 11): a task's screen (live, or the last one its run left) with how its run ended, the
//! Dashboard's tokens per task, and how far the repository's checkout is from its integration
//! branch.
//!
//! Contract:
//! - each is the person's, with their view right on the project, as the page shows it;
//! - a screen is what the page's terminal shows: the live one as viewers get it (redacted of
//!   agent secrets), a run's saved last screen redacted again before it is read;
//! - nothing is changed: the checkout is read as it is (no fetch); the answer says when it was
//!   last fetched.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::person_tools::record;

/// The most task rows `task_usage` answers.
const USAGE_ROWS: usize = 50;

/// Lines of a screen a call answers by default, and at most.
const SCREEN_LINES: usize = 40;
const MAX_SCREEN_LINES: usize = 200;

/// The last `lines` lines of `text` that hold something, in order.
fn last_lines(text: &str, lines: usize) -> String {
    let kept: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .skip_while(|l| l.is_empty())
        .take(lines)
        .collect();
    kept.into_iter().rev().collect::<Vec<_>>().join("\n")
}

impl App {
    /// `task_screen {task_id | number, lines?}` (VIS-001): the task's screen as its page's
    /// terminal shows it, live while it runs, else the last screen its last run left, with how
    /// that run ended (state, exit code, reason, when).
    ///
    /// # Errors
    /// `NotFound` for a task not in the project; `Invalid` arguments.
    pub(crate) async fn task_screen_for_person(
        &self,
        project: &ProjectCaller,
        sandbox: &SandboxRecord,
        args: &Value,
    ) -> AppResult<Value> {
        let lines = args
            .get("lines")
            .and_then(Value::as_u64)
            .map_or(SCREEN_LINES, |n| {
                usize::try_from(n).unwrap_or(MAX_SCREEN_LINES)
            })
            .clamp(1, MAX_SCREEN_LINES);
        let task = self.agent_task_ref(project, args).await?;
        self.task_screen_of(sandbox, &task, lines).await
    }

    /// The screen of `task` as [`App::task_screen_for_person`] answers it, its last `lines`
    /// lines (also `task_get`'s, SPEC-625 PSV-001).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn task_screen_of(
        &self,
        sandbox: &SandboxRecord,
        task: &crate::model::TaskRecord,
        lines: usize,
    ) -> AppResult<Value> {
        let attempts = self.deps.store.attempts(&task.id).await?;
        let last = attempts.last();
        let runtime = self.runtime(&task.id);
        let live = self.deps.store.live_attempt(&task.id).await?;
        // A run still starting shows nothing of its own yet: not the screen the run before
        // left in the hub (review of slice 10b, F6).
        let starting = live
            .as_ref()
            .is_some_and(|run| run.state != yulius_domain::task::AttemptState::Live);
        let (screen, from) = if live.is_some() && runtime.hub.grid_state().1 {
            (runtime.hub.screen_text(), "last run")
        } else if !starting && live.is_some() && runtime.hub.has_output() {
            (runtime.hub.screen_text(), "live")
        } else if (live.is_none() || !runtime.hub.initialized())
            && let Some(saved) = self.visible_ended_screen(task, &attempts).await
        {
            let (cols, rows, bytes) = saved.grid;
            // Redacted again: the saved screen is read outside the terminal's own redactor.
            let scrub = crate::vault::TerminalScrub::default();
            let shown = self.terminal_snapshot(&scrub, &sandbox.id, &bytes).await;
            let hub = crate::hub::Hub::new(cols, rows);
            hub.reset(1, &shown, cols, rows);
            (hub.screen_text(), "last run")
        } else if starting {
            (String::new(), "starting")
        } else {
            (String::new(), "none")
        };
        Ok(json!({
            "task": task.number,
            "status": task.status.as_str(),
            "screen_from": from,
            "screen": last_lines(&screen, lines),
            "last_run": last.map(|run| json!({
                "state": run.state.as_str(),
                "exit_code": run.exit_code,
                "exit_reason": run.exit_reason,
                "started_at": rfc3339(run.started_at),
                "ended_at": run.ended_at.map(rfc3339),
            })),
        }))
    }

    /// `task_usage {number?, from?, to?}` (VIS-002): the Dashboard's tokens per task of the
    /// project for the tasks created in the period (default: the last 7 days), the most first
    /// and at most [`USAGE_ROWS`], and the period's totals by model.
    ///
    /// # Errors
    /// `Invalid` dates, the Dashboard's refusals.
    pub(crate) async fn task_usage_for_person(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxRecord,
        args: &Value,
    ) -> AppResult<Value> {
        let text = |key: &str| args.get(key).and_then(Value::as_str);
        let sid = Some(sandbox.id.as_str());
        // The last 7 days when no period is named (review of slice 10b, F1), as the
        // description says; the Dashboard's own default is all time.
        let week =
            crate::analytics::day_string(self.now().0.div_euclid(crate::analytics::DAY_MS) - 6);
        let from = text("from").or(if text("to").is_none() {
            Some(week.as_str())
        } else {
            None
        });
        let to = text("to");
        let summary = self.analytics_summary(ctx, sid, from, to).await?;
        let mut rows = self.analytics_tasks(ctx, sid, from, to).await?;
        if let Some(number) = args.get("number").and_then(Value::as_u64) {
            rows.retain(|row| u64::from(row.number) == number);
        }
        let used = |row: &yulius_contracts::api::analytics::TaskRow| {
            [
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_write_tokens,
            ]
            .iter()
            .flatten()
            .sum::<u64>()
        };
        rows.sort_by_key(|row| std::cmp::Reverse(used(row)));
        // How many tasks the period has, when the answer shows fewer (review of slice 10b, n4).
        let total = rows.len();
        rows.truncate(USAGE_ROWS);
        Ok(json!({
            "period": record(&summary.range)?,
            "tokens": record(&summary.tokens)?,
            "tokens_by_model": record(&summary.tokens_by_model)?,
            "tasks": record(&rows)?,
            "tasks_in_period": total,
        }))
    }

    /// `checkout` (VIS-003): the repository checkout the project's Superagent works in: its
    /// branch, how many commits its head is behind and ahead of its integration branch on the
    /// remote as last fetched, and when that branch was last fetched.
    ///
    /// # Errors
    /// `Conflict` when the repository is not cloned; Git failures.
    pub async fn checkout_for_person(&self, sandbox: &SandboxRecord) -> AppResult<Value> {
        let repo = self.sandbox_repo(&sandbox.id);
        if !repo.join(".git").exists() {
            return Err(AppError::Conflict(
                "the project's repository is not cloned yet".into(),
            ));
        }
        let integration = crate::delivery::delivery_profile(sandbox).integration_branch;
        let upstream = format!("origin/{integration}");
        // A remote branch never fetched is a state, not a failure (review of slice 10b, F4).
        let (ahead, behind) = self
            .deps
            .git
            .ahead_behind(&repo, &upstream, "HEAD")
            .await
            .map_err(|err| {
                tracing::info!(error = %err, "a checkout's distance could not be read");
                AppError::Conflict(format!(
                    "{upstream} is not in this checkout yet: run git fetch, then look again"
                ))
            })?;
        let git = repo.join(".git");
        let branch = std::fs::read_to_string(git.join("HEAD"))
            .ok()
            .and_then(|head| {
                head.trim()
                    .strip_prefix("ref: refs/heads/")
                    .map(str::to_owned)
            });
        // When that remote branch was fetched (its own ref), else the last fetch of anything
        // (F5).
        let remote_ref = git.join("refs/remotes/origin").join(&integration);
        let stamp = if remote_ref.exists() {
            remote_ref
        } else {
            git.join("FETCH_HEAD")
        };
        let fetched_at = std::fs::metadata(stamp)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .map(|ms| rfc3339(yulius_domain::time::Timestamp(ms)));
        Ok(json!({
            "branch": branch,
            "integration_branch": integration,
            "behind": behind,
            "ahead": ahead,
            "fetched_at": fetched_at,
            "note": "As of the last fetch: run git fetch in this checkout before you answer from it when it is old.",
        }))
    }
}

/// The tool names this module serves through the person's project tools.
pub(crate) const VISIBILITY_TOOLS: [&str; 3] = ["task_screen", "task_usage", "checkout"];

/// Runs one of [`VISIBILITY_TOOLS`].
pub(crate) async fn run(
    app: &Arc<App>,
    ctx: &AuthContext,
    project: &ProjectCaller,
    sandbox: &SandboxRecord,
    name: &str,
    args: &Value,
) -> AppResult<Value> {
    match name {
        "task_screen" => app.task_screen_for_person(project, sandbox, args).await,
        "task_usage" => app.task_usage_for_person(ctx, sandbox, args).await,
        "checkout" => app.checkout_for_person(sandbox).await,
        other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::last_lines;

    /// SPEC-487 VIS-001: a screen is answered by its last lines that hold something.
    #[test]
    fn a_screen_is_cut_to_its_last_lines() {
        assert_eq!(last_lines("a\nb\nc\n\n  \n", 2), "b\nc");
        assert_eq!(last_lines("", 3), "");
        assert_eq!(last_lines("x  \ny", 5), "x\ny");
    }
}
