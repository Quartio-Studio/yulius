//! Tasks, attempts, provider-session lineage, metrics, feedback and switch decisions.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{
    AttemptRecord, ControlRequest, FeedbackRecord, LaunchKind, MetricsRecord, PersonPause,
    ProviderSessionRecord, RecountOutcome, SwitchDecisionRecord, TaskMode, TaskOrigin, TaskRecord,
    TaskTimes,
};
use yulius_app::ports::store::{AttemptUpdate, TaskFieldUpdate, TaskStore};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u32, to_u64};

const TASK_COLS: &str = "id, sandbox_id, number, title, prompt, harness, model, effort, workflow_id, \
    status, activity, branch, worktree_path, spec_path, origin, automation_run, created_by, created_at, \
    updated_at, closed_at, archived_at, queued_at, gate_blocks, wake_enabled, mode, supertask, owner_id";

const ATTEMPT_COLS: &str = "id, task_id, seq, harness, account_id, provider_session_id, launch_kind, \
    state, run_dir, token_hash, host_token, ptyhost_pid, child_pid, started_at, ended_at, exit_code, exit_reason";

fn parse_id<T>(
    column: &str,
    text: &str,
    f: fn(&str) -> Result<T, yulius_domain::ids::IdError>,
) -> rusqlite::Result<T> {
    f(text).map_err(|_| bad_column(column, text))
}

fn harness(text: &str) -> rusqlite::Result<Harness> {
    Harness::parse(text).map_err(|_| bad_column("harness", text))
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<TaskRecord> {
    let id: String = row.get("id")?;
    let sandbox: String = row.get("sandbox_id")?;
    let status: String = row.get("status")?;
    let activity: Option<String> = row.get("activity")?;
    let created_by: String = row.get("created_by")?;
    let owner: String = row.get("owner_id")?;
    let origin: String = row.get("origin")?;
    let harness_text: String = row.get("harness")?;
    Ok(TaskRecord {
        id: parse_id("task id", &id, TaskId::parse)?,
        sandbox_id: parse_id("sandbox id", &sandbox, SandboxId::parse)?,
        number: to_u32(row.get("number")?),
        title: row.get("title")?,
        prompt: row.get("prompt")?,
        harness: harness(&harness_text)?,
        model: row.get("model")?,
        effort: row.get("effort")?,
        workflow_id: row.get("workflow_id")?,
        status: TaskStatus::parse(&status).ok_or_else(|| bad_column("status", &status))?,
        activity: activity.as_deref().and_then(Activity::parse),
        branch: row.get("branch")?,
        worktree_path: row.get("worktree_path")?,
        spec_path: row.get("spec_path")?,
        origin: TaskOrigin::parse(&origin),
        automation_run: row.get("automation_run")?,
        created_by: parse_id("user id", &created_by, UserId::parse)?,
        owner: parse_id("user id", &owner, UserId::parse)?,
        created_at: Timestamp(row.get("created_at")?),
        updated_at: Timestamp(row.get("updated_at")?),
        closed_at: row.get::<_, Option<i64>>("closed_at")?.map(Timestamp),
        archived_at: row.get::<_, Option<i64>>("archived_at")?.map(Timestamp),
        queued_at: row.get::<_, Option<i64>>("queued_at")?.map(Timestamp),
        gate_blocks: to_u32(row.get("gate_blocks")?),
        wake_enabled: row.get("wake_enabled")?,
        mode: {
            let mode: String = row.get("mode")?;
            TaskMode::parse(&mode).ok_or_else(|| bad_column("mode", &mode))?
        },
        supertask: row.get("supertask")?,
    })
}

fn attempt_from_row(row: &Row<'_>) -> rusqlite::Result<AttemptRecord> {
    let id: String = row.get("id")?;
    let task: String = row.get("task_id")?;
    let account: Option<String> = row.get("account_id")?;
    let state: String = row.get("state")?;
    let kind: String = row.get("launch_kind")?;
    let harness_text: String = row.get("harness")?;
    let pid = |name: &str| -> rusqlite::Result<Option<u32>> {
        Ok(row.get::<_, Option<i64>>(name)?.map(to_u32))
    };
    Ok(AttemptRecord {
        id: parse_id("attempt id", &id, AttemptId::parse)?,
        task_id: parse_id("task id", &task, TaskId::parse)?,
        seq: to_u32(row.get("seq")?),
        harness: harness(&harness_text)?,
        account_id: account
            .as_deref()
            .map(|a| parse_id("account id", a, AccountId::parse))
            .transpose()?,
        provider_session_id: row.get("provider_session_id")?,
        launch_kind: LaunchKind::parse(&kind),
        state: AttemptState::parse(&state).ok_or_else(|| bad_column("attempt state", &state))?,
        run_dir: row.get("run_dir")?,
        token_hash: row.get("token_hash")?,
        host_token: yulius_app::model::Sealed(row.get("host_token")?),
        ptyhost_pid: pid("ptyhost_pid")?,
        child_pid: pid("child_pid")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        exit_code: row.get("exit_code")?,
        exit_reason: row.get("exit_reason")?,
    })
}

fn opt_u64(v: Option<i64>) -> Option<u64> {
    v.map(to_u64)
}

pub(crate) fn metrics_from_row(row: &Row<'_>) -> rusqlite::Result<MetricsRecord> {
    Ok(MetricsRecord {
        model: row.get("model")?,
        input_tokens: opt_u64(row.get("input_tokens")?),
        output_tokens: opt_u64(row.get("output_tokens")?),
        cache_read: opt_u64(row.get("cache_read")?),
        cache_write: opt_u64(row.get("cache_write")?),
        turns: row.get::<_, Option<i64>>("turns")?.map(to_u32),
        active_ms: opt_u64(row.get("active_ms")?),
    })
}

/// An attempt's stored figures as JSON, `None` without a row (SPEC-091 RCT-004).
fn stored_metrics_json(c: &rusqlite::Connection, attempt: &str) -> AppResult<Option<String>> {
    let row = c
        .query_row(
            "SELECT model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms FROM metrics WHERE attempt_id = ?1",
            [attempt],
            metrics_from_row,
        )
        .optional()
        .map_err(db)?;
    Ok(row.map(|m| {
        serde_json::json!({
            "model": m.model,
            "input_tokens": m.input_tokens,
            "output_tokens": m.output_tokens,
            "cache_read": m.cache_read,
            "cache_write": m.cache_write,
            "turns": m.turns,
            "active_ms": m.active_ms,
        })
        .to_string()
    }))
}

impl SqliteStore {
    async fn tasks_query(&self, sql: String, args: Vec<String>) -> AppResult<Vec<TaskRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&sql).map_err(db)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), task_from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn attempts_query(
        &self,
        sql: String,
        args: Vec<String>,
    ) -> AppResult<Vec<AttemptRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&sql).map_err(db)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), attempt_from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }
}

#[async_trait]
impl TaskStore for SqliteStore {
    async fn insert_task(&self, task: &TaskRecord) -> AppResult<TaskRecord> {
        let t = task.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let next: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(number), 0) + 1 FROM tasks WHERE sandbox_id = ?1",
                    [t.sandbox_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(db)?;
            tx.execute(
                &format!("INSERT INTO tasks ({TASK_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)"),
                params![
                    t.id.as_str(),
                    t.sandbox_id.as_str(),
                    next,
                    t.title,
                    t.prompt,
                    t.harness.as_str(),
                    t.model,
                    t.effort,
                    t.workflow_id,
                    t.status.as_str(),
                    t.activity.map(Activity::as_str),
                    t.branch,
                    t.worktree_path,
                    t.spec_path,
                    t.origin.as_str(),
                    t.automation_run,
                    t.created_by.as_str(),
                    t.created_at.0,
                    t.updated_at.0,
                    t.closed_at.map(|x| x.0),
                    t.archived_at.map(|x| x.0),
                    t.queued_at.map(|x| x.0),
                    i64::from(t.gate_blocks),
                    t.wake_enabled,
                    t.mode.as_str(),
                    t.supertask,
                    t.owner.as_str(),
                ],
            )
            .map_err(db)?;
            // A new task is in its first state since it was created (SPEC-087 TIM-001).
            tx.execute(
                "UPDATE tasks SET state_at = created_at, working_since = CASE \
                 WHEN status IN ('starting', 'running', 'switching', 'waiting_capacity') \
                 AND (activity IS NULL OR activity = 'working') THEN created_at END WHERE id = ?1",
                [t.id.as_str()],
            )
            .map_err(db)?;
            let stored = tx
                .query_row(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [t.id.as_str()], task_from_row)
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(stored)
        })
        .await
    }

    async fn task(&self, id: &TaskId) -> AppResult<Option<TaskRecord>> {
        let id = id.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"),
                [id],
                task_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn task_by_number(
        &self,
        sandbox: &SandboxId,
        number: u32,
    ) -> AppResult<Option<TaskRecord>> {
        let s = sandbox.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {TASK_COLS} FROM tasks WHERE sandbox_id = ?1 AND number = ?2"),
                params![s, i64::from(number)],
                task_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn tasks(&self, sandbox: &SandboxId, archived: bool) -> AppResult<Vec<TaskRecord>> {
        let clause = if archived {
            "archived_at IS NOT NULL"
        } else {
            "archived_at IS NULL"
        };
        self.tasks_query(
            format!("SELECT {TASK_COLS} FROM tasks WHERE sandbox_id = ?1 AND {clause} ORDER BY number DESC"),
            vec![sandbox.as_str().to_owned()],
        )
        .await
    }

    async fn tasks_with_status(&self, statuses: &[TaskStatus]) -> AppResult<Vec<TaskRecord>> {
        if statuses.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (1..=statuses.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        self.tasks_query(
            format!("SELECT {TASK_COLS} FROM tasks WHERE status IN ({marks}) ORDER BY COALESCE(queued_at, created_at)"),
            statuses.iter().map(|s| s.as_str().to_owned()).collect(),
        )
        .await
    }

    async fn transition_task(
        &self,
        id: &TaskId,
        expected: Option<TaskStatus>,
        next: TaskStatus,
        activity: Option<Activity>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let id = id.as_str().to_owned();
        // `release_scheduled` (intake.rs) repeats this bookkeeping for prepared → queued.
        self.call(move |c| {
            let closed = matches!(next, TaskStatus::Closed).then_some(at.0);
            let queued = matches!(next, TaskStatus::Queued).then_some(at.0);
            let n = c
                .execute(
                    "UPDATE tasks SET status = ?1, activity = ?2, updated_at = ?3, \
                     state_at = CASE WHEN status IS NOT ?1 OR activity IS NOT ?2 THEN ?3 ELSE state_at END, \
                     working_since = CASE WHEN ?1 IN ('starting', 'switching', 'waiting_capacity') \
                       OR (?1 = 'running' AND (?2 IS NULL OR ?2 = 'working')) THEN COALESCE(working_since, ?3) \
                       WHEN ?1 = 'running' AND ?2 != 'needs_attention' THEN working_since ELSE NULL END, \
                     closed_at = CASE WHEN ?4 IS NOT NULL THEN ?4 ELSE closed_at END, \
                     queued_at = CASE WHEN ?5 IS NOT NULL THEN ?5 WHEN ?1 = 'queued' THEN queued_at ELSE NULL END, \
                     gate_blocks = CASE WHEN ?1 = 'running' THEN gate_blocks ELSE 0 END \
                     WHERE id = ?6 AND (?7 IS NULL OR status = ?7)",
                    params![
                        next.as_str(),
                        activity.map(Activity::as_str),
                        at.0,
                        closed,
                        queued,
                        id,
                        expected.map(TaskStatus::as_str)
                    ],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn set_activity(
        &self,
        id: &TaskId,
        activity: Option<Activity>,
        at: Timestamp,
    ) -> AppResult<()> {
        let id = id.as_str().to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE tasks SET activity = ?1, updated_at = ?2, \
                 state_at = CASE WHEN activity IS NOT ?1 THEN ?2 ELSE state_at END, \
                 working_since = CASE WHEN ?1 = 'needs_attention' THEN NULL \
                   WHEN ?1 = 'working' OR ?1 IS NULL THEN COALESCE(working_since, ?2) \
                   ELSE working_since END WHERE id = ?3",
                params![activity.map(Activity::as_str), at.0, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_activity_from(
        &self,
        id: &TaskId,
        from: (Option<Activity>, Timestamp),
        activity: Option<Activity>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let id = id.as_str().to_owned();
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE tasks SET activity = ?1, updated_at = ?2, \
                     state_at = CASE WHEN activity IS NOT ?1 THEN ?2 ELSE state_at END, \
                     working_since = CASE WHEN ?1 = 'needs_attention' THEN NULL \
                       WHEN ?1 = 'working' OR ?1 IS NULL THEN COALESCE(working_since, ?2) \
                       ELSE working_since END \
                     WHERE id = ?3 AND status = 'running' AND activity IS ?4 AND state_at = ?5",
                    params![
                        activity.map(Activity::as_str),
                        at.0,
                        id,
                        from.0.map(Activity::as_str),
                        from.1.0
                    ],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn delete_task(&self, id: &TaskId) -> AppResult<bool> {
        let id = id.as_str().to_owned();
        self.call_access(move |c| {
            let tx = c.transaction().map_err(db)?;
            let statements = [
                "DELETE FROM metrics WHERE task_id = ?1",
                "DELETE FROM metrics_recounts WHERE task_id = ?1",
                "DELETE FROM message_receipts WHERE task_id = ?1",
                "DELETE FROM message_receipts WHERE message_id IN (SELECT id FROM messages WHERE from_task = ?1 OR to_task = ?1)",
                "DELETE FROM messages WHERE from_task = ?1 OR to_task = ?1",
                "DELETE FROM step_events WHERE task_id = ?1",
                "DELETE FROM steps WHERE task_id = ?1",
                "DELETE FROM switch_decisions WHERE task_id = ?1",
                "DELETE FROM capacity_pauses WHERE task_id = ?1",
                "DELETE FROM spans WHERE subject_kind = 'task' AND subject_id = ?1",
                "DELETE FROM scope_claims WHERE task_id = ?1",
                "DELETE FROM record_numbers WHERE task_id = ?1",
                "DELETE FROM feedback WHERE task_id = ?1",
                "DELETE FROM delivery WHERE task_id = ?1",
                "DELETE FROM provider_sessions WHERE task_id = ?1",
                "DELETE FROM attempts WHERE task_id = ?1",
                "DELETE FROM task_chains WHERE task_id = ?1",
                "DELETE FROM task_schedules WHERE task_id = ?1",
                "UPDATE task_chains SET after_task_id = NULL WHERE after_task_id = ?1",
                "DELETE FROM prompt_holds WHERE task_id = ?1",
                "UPDATE prompt_holds SET new_task_id = NULL WHERE new_task_id = ?1",
                "DELETE FROM task_auto_choices WHERE task_id = ?1",
                "UPDATE automation_runs SET task_id = NULL WHERE task_id = ?1",
                "UPDATE sandboxes SET onboarding_task_id = NULL WHERE onboarding_task_id = ?1",
            ];
            for sql in statements {
                tx.execute(sql, [id.as_str()]).map_err(db)?;
            }
            let deleted = tx
                .execute("DELETE FROM tasks WHERE id = ?1", [id.as_str()])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(deleted > 0)
        })
        .await
    }

    async fn task_times(&self, task: &TaskId) -> AppResult<Option<TaskTimes>> {
        let id = task.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT state_at, working_since FROM tasks WHERE id = ?1",
                [id.as_str()],
                |r| {
                    Ok(TaskTimes {
                        state_at: Timestamp(r.get(0)?),
                        working_since: r.get::<_, Option<i64>>(1)?.map(Timestamp),
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn set_working_since(&self, task: &TaskId, since: Option<Timestamp>) -> AppResult<()> {
        let id = task.as_str().to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE tasks SET working_since = ?1 WHERE id = ?2",
                params![since.map(|t| t.0), id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn person_pause(&self, task: &TaskId) -> AppResult<Option<PersonPause>> {
        let id = task.as_str().to_owned();
        self.call(move |c| {
            let row: Option<PauseRow> = c
                .query_row(
                    "SELECT paused_by, paused_at, paused_attempt FROM tasks WHERE id = ?1",
                    [id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(db)?;
            row.map_or(Ok(None), person_pause_of)
        })
        .await
    }

    async fn set_person_pause(
        &self,
        task: &TaskId,
        by: &UserId,
        attempt: &AttemptId,
        at: Timestamp,
    ) -> AppResult<PersonPause> {
        let (id, by, attempt) = (
            task.as_str().to_owned(),
            by.as_str().to_owned(),
            attempt.as_str().to_owned(),
        );
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "UPDATE tasks SET paused_by = ?2, paused_at = ?3, paused_attempt = ?4, \
                 updated_at = ?3 WHERE id = ?1 AND paused_by IS NULL",
                params![id, by, at.0, attempt],
            )
            .map_err(db)?;
            let row: PauseRow = tx
                .query_row(
                    "SELECT paused_by, paused_at, paused_attempt FROM tasks WHERE id = ?1",
                    [id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(db)?;
            tx.commit().map_err(db)?;
            person_pause_of(row)?.ok_or_else(|| AppError::internal("the pause was not recorded"))
        })
        .await
    }

    async fn clear_person_pause(&self, task: &TaskId, at: Timestamp) -> AppResult<bool> {
        let id = task.as_str().to_owned();
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE tasks SET paused_by = NULL, paused_at = NULL, paused_attempt = NULL, \
                     updated_at = ?2 WHERE id = ?1 AND paused_by IS NOT NULL",
                    params![id, at.0],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn person_paused_tasks(&self) -> AppResult<Vec<(TaskId, PersonPause)>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, paused_by, paused_at, paused_attempt FROM tasks \
                     WHERE paused_by IS NOT NULL ORDER BY paused_at LIMIT 1000",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
                })
                .map_err(db)?;
            let mut paused = Vec::new();
            for row in rows {
                let (id, pause) = row.map_err(db)?;
                if let Some(pause) = person_pause_of(pause)? {
                    let task = TaskId::parse(&id).map_err(|_| db(bad_column("tasks.id", &id)))?;
                    paused.push((task, pause));
                }
            }
            Ok(paused)
        })
        .await
    }

    async fn control_request(&self, task: &TaskId) -> AppResult<Option<ControlRequest>> {
        let id = task.as_str().to_owned();
        self.call(move |c| {
            let row: Option<(Option<String>, Option<i64>)> = c
                .query_row(
                    "SELECT control_requested_by, control_requested_at FROM tasks WHERE id = ?1",
                    [id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db)?;
            match row {
                Some((Some(by), Some(at))) => Ok(Some(ControlRequest {
                    by: UserId::parse(&by).map_err(|_| AppError::internal("bad user id"))?,
                    at: Timestamp(at),
                })),
                _ => Ok(None),
            }
        })
        .await
    }

    async fn request_control(&self, task: &TaskId, by: &UserId, at: Timestamp) -> AppResult<bool> {
        let (id, by) = (task.as_str().to_owned(), by.as_str().to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "UPDATE tasks SET control_requested_by = ?2, control_requested_at = ?3, \
                 updated_at = ?3 WHERE id = ?1 AND control_requested_by IS NULL",
                params![id, by, at.0],
            )
            .map_err(db)?;
            let waits = tx
                .query_row(
                    "SELECT 1 FROM tasks WHERE id = ?1 AND control_requested_by = ?2",
                    params![id, by],
                    |_| Ok(()),
                )
                .optional()
                .map_err(db)?
                .is_some();
            tx.commit().map_err(db)?;
            Ok(waits)
        })
        .await
    }

    async fn drop_control_request(
        &self,
        task: &TaskId,
        by: &UserId,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, by) = (task.as_str().to_owned(), by.as_str().to_owned());
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE tasks SET control_requested_by = NULL, control_requested_at = NULL, \
                     updated_at = ?3 WHERE id = ?1 AND control_requested_by = ?2",
                    params![id, by, at.0],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn pass_control(
        &self,
        task: &TaskId,
        from: &UserId,
        to: &UserId,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, from, to) = (
            task.as_str().to_owned(),
            from.as_str().to_owned(),
            to.as_str().to_owned(),
        );
        self.call_access(move |c| {
            let n = c
                .execute(
                    "UPDATE tasks SET owner_id = ?3, control_requested_by = NULL, \
                     control_requested_at = NULL, updated_at = ?4 \
                     WHERE id = ?1 AND owner_id = ?2 AND control_requested_by = ?3",
                    params![id, from, to, at.0],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn take_control(
        &self,
        task: &TaskId,
        from: &UserId,
        to: &UserId,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, from, to) = (
            task.as_str().to_owned(),
            from.as_str().to_owned(),
            to.as_str().to_owned(),
        );
        self.call_access(move |c| {
            let n = c
                .execute(
                    "UPDATE tasks SET owner_id = ?3, control_requested_by = NULL, \
                     control_requested_at = NULL, updated_at = ?4 \
                     WHERE id = ?1 AND owner_id = ?2",
                    params![id, from, to, at.0],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn append_unstarted_prompt(
        &self,
        id: &TaskId,
        addition: &str,
        max_bytes: usize,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, addition) = (id.as_str().to_owned(), addition.to_owned());
        let max = i64::try_from(max_bytes).unwrap_or(i64::MAX);
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE tasks SET prompt = prompt || ?1, updated_at = ?2 \
                     WHERE id = ?3 AND status IN (?4, ?5, ?6, ?8) \
                     AND NOT EXISTS (SELECT 1 FROM attempts WHERE task_id = ?3) \
                     AND NOT EXISTS (SELECT 1 FROM provider_sessions WHERE task_id = ?3) \
                     AND length(CAST(prompt AS BLOB)) + length(CAST(?1 AS BLOB)) <= ?7",
                    params![
                        addition,
                        at.0,
                        id,
                        TaskStatus::Queued.as_str(),
                        TaskStatus::Chained.as_str(),
                        TaskStatus::WaitingCapacity.as_str(),
                        max,
                        TaskStatus::Prepared.as_str()
                    ],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn update_task(&self, id: &TaskId, u: &TaskFieldUpdate, at: Timestamp) -> AppResult<()> {
        let (id, u) = (id.as_str().to_owned(), u.clone());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let set = |col: &str, value: rusqlite::types::Value| -> AppResult<()> {
                tx.execute(
                    &format!("UPDATE tasks SET {col} = ?1, updated_at = ?2 WHERE id = ?3"),
                    params![value, at.0, id],
                )
                .map_err(db)?;
                Ok(())
            };
            use rusqlite::types::Value as V;
            let text = |s: Option<String>| s.map_or(V::Null, V::Text);
            if let Some(v) = u.title {
                set("title", V::Text(v))?;
            }
            if let Some(v) = u.harness {
                set("harness", V::Text(v.as_str().to_owned()))?;
            }
            if let Some(v) = u.branch {
                set("branch", V::Text(v))?;
            }
            if let Some(v) = u.worktree_path {
                set("worktree_path", V::Text(v))?;
            }
            if let Some(v) = u.spec_path {
                set("spec_path", V::Text(v))?;
            }
            if let Some(v) = u.model {
                set("model", text(v))?;
            }
            if let Some(v) = u.effort {
                set("effort", text(v))?;
            }
            if let Some(v) = u.archived_at {
                set("archived_at", v.map_or(V::Null, |t| V::Integer(t.0)))?;
            }
            if let Some(v) = u.gate_blocks {
                set("gate_blocks", V::Integer(i64::from(v)))?;
            }
            if let Some(v) = u.wake_enabled {
                set("wake_enabled", V::Integer(i64::from(v)))?;
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn insert_attempt(&self, a: &AttemptRecord) -> AppResult<()> {
        let a = a.clone();
        self.call(move |c| {
            c.execute(
                &format!("INSERT INTO attempts ({ATTEMPT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"),
                params![
                    a.id.as_str(),
                    a.task_id.as_str(),
                    i64::from(a.seq),
                    a.harness.as_str(),
                    a.account_id.as_ref().map(AccountId::as_str),
                    a.provider_session_id,
                    a.launch_kind.as_str(),
                    a.state.as_str(),
                    a.run_dir,
                    a.token_hash,
                    a.host_token.0,
                    a.ptyhost_pid.map(i64::from),
                    a.child_pid.map(i64::from),
                    a.started_at.0,
                    a.ended_at.map(|t| t.0),
                    a.exit_code,
                    a.exit_reason,
                ],
            )
            .map_err(|e| match db(e) {
                AppError::Conflict(_) => AppError::Conflict("task already has a live run".into()),
                other => other,
            })?;
            Ok(())
        })
        .await
    }

    async fn attempt(&self, id: &AttemptId) -> AppResult<Option<AttemptRecord>> {
        Ok(self
            .attempts_query(
                format!("SELECT {ATTEMPT_COLS} FROM attempts WHERE id = ?1"),
                vec![id.as_str().to_owned()],
            )
            .await?
            .into_iter()
            .next())
    }

    async fn attempt_by_token(&self, token_hash: &str) -> AppResult<Option<AttemptRecord>> {
        Ok(self
            .attempts_query(
                format!("SELECT {ATTEMPT_COLS} FROM attempts WHERE token_hash = ?1"),
                vec![token_hash.to_owned()],
            )
            .await?
            .into_iter()
            .next())
    }

    async fn has_attempts(&self, task: &TaskId) -> AppResult<bool> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM attempts WHERE task_id = ?1)",
                params![task],
                |row| row.get::<_, bool>(0),
            )
            .map_err(db)
        })
        .await
    }

    async fn attempts(&self, task: &TaskId) -> AppResult<Vec<AttemptRecord>> {
        self.attempts_query(
            format!("SELECT {ATTEMPT_COLS} FROM attempts WHERE task_id = ?1 ORDER BY seq"),
            vec![task.as_str().to_owned()],
        )
        .await
    }

    async fn live_attempt(&self, task: &TaskId) -> AppResult<Option<AttemptRecord>> {
        Ok(self
            .attempts_query(
                format!("SELECT {ATTEMPT_COLS} FROM attempts WHERE task_id = ?1 AND state IN ('spawning', 'live')"),
                vec![task.as_str().to_owned()],
            )
            .await?
            .into_iter()
            .next())
    }

    async fn live_attempts(&self) -> AppResult<Vec<AttemptRecord>> {
        self.attempts_query(
            format!("SELECT {ATTEMPT_COLS} FROM attempts WHERE state IN ('spawning', 'live') ORDER BY started_at"),
            Vec::new(),
        )
        .await
    }

    async fn transition_attempt(
        &self,
        id: &AttemptId,
        next: AttemptState,
        u: &AttemptUpdate,
    ) -> AppResult<bool> {
        let (id, u) = (id.as_str().to_owned(), u.clone());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let current: Option<String> = tx
                .query_row("SELECT state FROM attempts WHERE id = ?1", [&id], |r| r.get(0))
                .optional()
                .map_err(db)?;
            let Some(current) = current.as_deref().and_then(AttemptState::parse) else {
                return Err(AppError::NotFound);
            };
            // One durable outcome: a terminal attempt is not transitioned again, not even to the
            // same state (its second observer is not first, SPEC-065).
            if (current != next || current.is_terminal()) && !current.can_become(next) {
                return Ok(false);
            }
            tx.execute(
                "UPDATE attempts SET state = ?1, ptyhost_pid = COALESCE(?2, ptyhost_pid), child_pid = COALESCE(?3, child_pid), \
                 ended_at = COALESCE(?4, ended_at), exit_code = COALESCE(?5, exit_code), exit_reason = COALESCE(?6, exit_reason) \
                 WHERE id = ?7",
                params![
                    next.as_str(),
                    u.ptyhost_pid.map(i64::from),
                    u.child_pid.map(i64::from),
                    u.ended_at.map(|t| t.0),
                    u.exit_code,
                    u.exit_reason,
                    id
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn set_attempt_session(&self, id: &AttemptId, session_id: &str) -> AppResult<()> {
        let (id, sid) = (id.as_str().to_owned(), session_id.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE attempts SET provider_session_id = ?1 WHERE id = ?2",
                params![sid, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn upsert_provider_session(&self, r: &ProviderSessionRecord) -> AppResult<()> {
        let r = r.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO provider_sessions (id, sandbox_id, task_id, harness, session_id, transcript_ref, origin, first_seen, last_seen) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(sandbox_id, harness, session_id) DO UPDATE SET \
                 last_seen = excluded.last_seen, transcript_ref = COALESCE(excluded.transcript_ref, transcript_ref)",
                params![
                    r.id,
                    r.sandbox_id.as_str(),
                    r.task_id.as_str(),
                    r.harness.as_str(),
                    r.session_id,
                    r.transcript_ref,
                    r.origin,
                    r.first_seen.0,
                    r.last_seen.0
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn provider_sessions(&self, task: &TaskId) -> AppResult<Vec<ProviderSessionRecord>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, sandbox_id, task_id, harness, session_id, transcript_ref, origin, first_seen, last_seen \
                     FROM provider_sessions WHERE task_id = ?1 ORDER BY first_seen",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([t], |r| {
                    let sb: String = r.get(1)?;
                    let tk: String = r.get(2)?;
                    let h: String = r.get(3)?;
                    Ok(ProviderSessionRecord {
                        id: r.get(0)?,
                        sandbox_id: parse_id("sandbox id", &sb, SandboxId::parse)?,
                        task_id: parse_id("task id", &tk, TaskId::parse)?,
                        harness: harness(&h)?,
                        session_id: r.get(4)?,
                        transcript_ref: r.get(5)?,
                        origin: r.get(6)?,
                        first_seen: Timestamp(r.get(7)?),
                        last_seen: Timestamp(r.get(8)?),
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn provider_session_exists(
        &self,
        sandbox: &SandboxId,
        harness: &str,
        session_id: &str,
    ) -> AppResult<bool> {
        let (s, h, id) = (
            sandbox.as_str().to_owned(),
            harness.to_owned(),
            session_id.to_owned(),
        );
        self.call(move |c| {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM provider_sessions WHERE sandbox_id = ?1 AND harness = ?2 AND session_id = ?3",
                    params![s, h, id],
                    |r| r.get(0),
                )
                .map_err(db)?;
            Ok(n > 0)
        })
        .await
    }

    async fn put_metrics(
        &self,
        attempt: &AttemptId,
        task: &TaskId,
        m: &MetricsRecord,
        at: Timestamp,
    ) -> AppResult<()> {
        let (a, t, m) = (
            attempt.as_str().to_owned(),
            task.as_str().to_owned(),
            m.clone(),
        );
        self.call(move |c| {
            c.execute(
                "INSERT INTO metrics (attempt_id, task_id, model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(attempt_id) DO UPDATE SET model = excluded.model, \
                 input_tokens = excluded.input_tokens, output_tokens = excluded.output_tokens, cache_read = excluded.cache_read, \
                 cache_write = excluded.cache_write, turns = excluded.turns, active_ms = excluded.active_ms, updated_at = excluded.updated_at",
                params![
                    a,
                    t,
                    m.model,
                    m.input_tokens.map(to_i64),
                    m.output_tokens.map(to_i64),
                    m.cache_read.map(to_i64),
                    m.cache_write.map(to_i64),
                    m.turns.map(i64::from),
                    m.active_ms.map(to_i64),
                    at.0
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn task_metrics(&self, task: &TaskId) -> AppResult<Vec<MetricsRecord>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare("SELECT model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms FROM metrics WHERE task_id = ?1")
                .map_err(db)?;
            let rows = stmt.query_map([t], metrics_from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn queued_metrics_recounts(&self) -> AppResult<Vec<AttemptId>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT r.attempt_id FROM metrics_recounts r LEFT JOIN attempts a ON a.id = r.attempt_id \
                     WHERE r.outcome IS NULL ORDER BY COALESCE(a.started_at, 0), r.attempt_id",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(db)?;
            rows.map(|id| {
                let id = id.map_err(db)?;
                AttemptId::parse(&id).map_err(|_| db(bad_column("attempt_id", &id)))
            })
            .collect()
        })
        .await
    }

    async fn finish_metrics_recount(
        &self,
        attempt: &AttemptId,
        outcome: RecountOutcome,
        metrics: Option<&MetricsRecord>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (a, m) = (attempt.as_str().to_owned(), metrics.cloned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let task: Option<String> = tx
                .query_row(
                    "SELECT task_id FROM metrics_recounts WHERE attempt_id = ?1 AND outcome IS NULL",
                    [&a],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            let Some(task) = task else {
                return Ok(false);
            };
            if let Some(m) = m {
                tx.execute(
                    "INSERT INTO metrics (attempt_id, task_id, model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(attempt_id) DO UPDATE SET model = COALESCE(excluded.model, metrics.model), \
                     input_tokens = excluded.input_tokens, output_tokens = excluded.output_tokens, cache_read = excluded.cache_read, \
                     cache_write = excluded.cache_write, turns = excluded.turns, active_ms = excluded.active_ms, updated_at = excluded.updated_at",
                    params![
                        a,
                        task,
                        m.model,
                        m.input_tokens.map(to_i64),
                        m.output_tokens.map(to_i64),
                        m.cache_read.map(to_i64),
                        m.cache_write.map(to_i64),
                        m.turns.map(i64::from),
                        m.active_ms.map(to_i64),
                        at.0
                    ],
                )
                .map_err(db)?;
            }
            let after = stored_metrics_json(&tx, &a)?;
            tx.execute(
                "UPDATE metrics_recounts SET outcome = ?2, after = ?3, done_at = ?4 WHERE attempt_id = ?1",
                params![a, outcome.as_str(), after, at.0],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn put_feedback(&self, f: &FeedbackRecord) -> AppResult<()> {
        let f = f.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO feedback (task_id, user_id, verdict, comment, at) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(task_id, user_id) DO UPDATE SET verdict = excluded.verdict, comment = excluded.comment, at = excluded.at",
                params![f.task_id.as_str(), f.user_id.as_str(), f.verdict, f.comment, f.at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn feedback(&self, task: &TaskId) -> AppResult<Vec<FeedbackRecord>> {
        let t = task.as_str().to_owned();
        let task = task.clone();
        self.call(move |c| {
            let mut stmt = c.prepare("SELECT user_id, verdict, comment, at FROM feedback WHERE task_id = ?1 ORDER BY at").map_err(db)?;
            let rows = stmt
                .query_map([t], |r| {
                    let u: String = r.get(0)?;
                    Ok(FeedbackRecord {
                        task_id: task.clone(),
                        user_id: parse_id("user id", &u, UserId::parse)?,
                        verdict: r.get(1)?,
                        comment: r.get(2)?,
                        at: Timestamp(r.get(3)?),
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn insert_switch(&self, d: &SwitchDecisionRecord) -> AppResult<()> {
        let d = d.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO switch_decisions (id, task_id, at, reason, inputs, from_desc, to_desc, outcome) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![d.id, d.task_id.as_str(), d.at.0, d.reason, d.inputs, d.from_desc, d.to_desc, d.outcome],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_switch_outcome(&self, id: &str, outcome: &str) -> AppResult<()> {
        let (id, o) = (id.to_owned(), outcome.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE switch_decisions SET outcome = ?1 WHERE id = ?2",
                params![o, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn switches(&self, task: &TaskId) -> AppResult<Vec<SwitchDecisionRecord>> {
        let t = task.as_str().to_owned();
        let task = task.clone();
        self.call(move |c| {
            let mut stmt = c
                .prepare("SELECT id, at, reason, inputs, from_desc, to_desc, outcome FROM switch_decisions WHERE task_id = ?1 ORDER BY at")
                .map_err(db)?;
            let rows = stmt
                .query_map([t], |r| {
                    Ok(SwitchDecisionRecord {
                        id: r.get(0)?,
                        task_id: task.clone(),
                        at: Timestamp(r.get(1)?),
                        reason: r.get(2)?,
                        inputs: r.get(3)?,
                        from_desc: r.get(4)?,
                        to_desc: r.get(5)?,
                        outcome: r.get(6)?,
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }
}

/// `paused_by`, `paused_at`, `paused_attempt` of a task row.
type PauseRow = (Option<String>, Option<i64>, Option<String>);

/// A person's pause from its columns (SPEC-191); none unless all three are set.
fn person_pause_of(row: PauseRow) -> AppResult<Option<PersonPause>> {
    let (Some(by), Some(at), Some(attempt)) = row else {
        return Ok(None);
    };
    Ok(Some(PersonPause {
        by: UserId::parse(&by).map_err(|_| db(bad_column("tasks.paused_by", &by)))?,
        at: Timestamp(at),
        attempt: AttemptId::parse(&attempt)
            .map_err(|_| db(bad_column("tasks.paused_attempt", &attempt)))?,
    }))
}
