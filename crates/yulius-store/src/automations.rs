//! Automations and their deduplicated runs (SPEC-002 §6.4). The run table's primary key
//! (automation_id, scheduled_for) and the unique `tasks.automation_run` index make run handling
//! idempotent across restarts.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::automations::{
    AuditEntry, AutomationRecord, AutomationRunRecord, AutomationStart, CreatedVia, RunState,
};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::AutomationStore;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str = "id, sandbox_id, name, cron, utc_offset_minutes, prompt, workflow_id, harness, \
    enabled, created_by, created_via, created_at, last_scheduled, supertask, after_task_id, start_rule, prepared";

const RUN_COLS: &str = "automation_id, scheduled_for, state, task_id, error, created_at";

/// The start choice as its four columns: `supertask`, `after_task_id`, `start_rule` and
/// `prepared` (SPEC-155).
type StartColumns = (bool, Option<String>, Option<String>, bool);

fn start_columns(start: &AutomationStart) -> StartColumns {
    match start {
        AutomationStart::Now => (false, None, None, false),
        AutomationStart::Supertask => (true, None, None, false),
        AutomationStart::After(task) => (false, Some(task.as_str().to_owned()), None, false),
        AutomationStart::Rule(rule) => (false, None, Some(rule.clone()), false),
        AutomationStart::Prepared => (false, None, None, true),
    }
}

fn start_from_row(row: &Row<'_>) -> rusqlite::Result<AutomationStart> {
    let after: Option<String> = row.get("after_task_id")?;
    let rule: Option<String> = row.get("start_rule")?;
    Ok(if row.get("prepared")? {
        AutomationStart::Prepared
    } else if row.get("supertask")? {
        AutomationStart::Supertask
    } else if let Some(task) = after {
        AutomationStart::After(TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?)
    } else if let Some(rule) = rule {
        AutomationStart::Rule(rule)
    } else {
        AutomationStart::Now
    })
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<AutomationRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let harness: String = row.get("harness")?;
    let created_by: String = row.get("created_by")?;
    let created_via: String = row.get("created_via")?;
    let offset: i64 = row.get("utc_offset_minutes")?;
    Ok(AutomationRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        name: row.get("name")?,
        cron: row.get("cron")?,
        utc_offset_minutes: i32::try_from(offset)
            .map_err(|_| bad_column("utc offset", &offset.to_string()))?,
        prompt: row.get("prompt")?,
        workflow_id: row.get("workflow_id")?,
        harness: Harness::parse(&harness).map_err(|_| bad_column("harness", &harness))?,
        enabled: row.get("enabled")?,
        start: start_from_row(row)?,
        created_by: UserId::parse(&created_by).map_err(|_| bad_column("user id", &created_by))?,
        created_via: CreatedVia::parse(&created_via),
        created_at: Timestamp(row.get("created_at")?),
        last_scheduled: row.get::<_, Option<i64>>("last_scheduled")?.map(Timestamp),
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<AutomationRunRecord> {
    let state: String = row.get("state")?;
    let task: Option<String> = row.get("task_id")?;
    Ok(AutomationRunRecord {
        automation_id: row.get("automation_id")?,
        scheduled_for: Timestamp(row.get("scheduled_for")?),
        state: RunState::parse(&state).ok_or_else(|| bad_column("run state", &state))?,
        task_id: task
            .as_deref()
            .map(|t| TaskId::parse(t).map_err(|_| bad_column("task id", t)))
            .transpose()?,
        error: row.get("error")?,
        created_at: Timestamp(row.get("created_at")?),
    })
}

impl SqliteStore {
    async fn automations_query(
        &self,
        sql: String,
        args: Vec<String>,
    ) -> AppResult<Vec<AutomationRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&sql).map_err(db)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn runs_query(
        &self,
        sql: String,
        args: Vec<String>,
        limit: u32,
    ) -> AppResult<Vec<AutomationRunRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&sql).map_err(db)?;
            let mut values: Vec<rusqlite::types::Value> =
                args.into_iter().map(rusqlite::types::Value::Text).collect();
            values.push(rusqlite::types::Value::Integer(i64::from(limit)));
            let rows = stmt
                .query_map(rusqlite::params_from_iter(values.iter()), run_from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }
}

#[async_trait]
impl AutomationStore for SqliteStore {
    async fn insert_automation(
        &self,
        automation: &AutomationRecord,
        max_per_sandbox: u32,
    ) -> AppResult<()> {
        let a = automation.clone();
        let (supertask, after_task_id, start_rule, prepared) = start_columns(&a.start);
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM automations WHERE sandbox_id = ?1",
                    [a.sandbox_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(db)?;
            if count >= i64::from(max_per_sandbox) {
                return Err(AppError::Capacity(format!(
                    "at most {max_per_sandbox} automations per project"
                )));
            }
            tx.execute(
                &format!(
                    "INSERT INTO automations ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
                ),
                params![
                    a.id,
                    a.sandbox_id.as_str(),
                    a.name,
                    a.cron,
                    i64::from(a.utc_offset_minutes),
                    a.prompt,
                    a.workflow_id,
                    a.harness.as_str(),
                    a.enabled,
                    a.created_by.as_str(),
                    a.created_via.as_str(),
                    a.created_at.0,
                    a.last_scheduled.map(|t| t.0),
                    supertask,
                    after_task_id,
                    start_rule,
                    prepared,
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn automation(&self, id: &str) -> AppResult<Option<AutomationRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM automations WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn automations(&self, sandbox: &SandboxId) -> AppResult<Vec<AutomationRecord>> {
        self.automations_query(
            format!(
                "SELECT {COLS} FROM automations WHERE sandbox_id = ?1 ORDER BY name COLLATE NOCASE, created_at"
            ),
            vec![sandbox.as_str().to_owned()],
        )
        .await
    }

    async fn enabled_automations(&self) -> AppResult<Vec<AutomationRecord>> {
        let cols = COLS
            .split(", ")
            .map(|c| format!("a.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        self.automations_query(
            format!(
                "SELECT {cols} FROM automations a JOIN sandboxes s ON s.id = a.sandbox_id \
                 WHERE a.enabled = 1 AND s.removed_at IS NULL ORDER BY a.created_at, a.id"
            ),
            Vec::new(),
        )
        .await
    }

    async fn update_automation(&self, automation: &AutomationRecord) -> AppResult<bool> {
        let a = automation.clone();
        let (supertask, after_task_id, start_rule, prepared) = start_columns(&a.start);
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE automations SET name = ?2, cron = ?3, utc_offset_minutes = ?4, \
                     prompt = ?5, workflow_id = ?6, harness = ?7, enabled = ?8, supertask = ?9, \
                     after_task_id = ?10, start_rule = ?11, prepared = ?12 WHERE id = ?1",
                    params![
                        a.id,
                        a.name,
                        a.cron,
                        i64::from(a.utc_offset_minutes),
                        a.prompt,
                        a.workflow_id,
                        a.harness.as_str(),
                        a.enabled,
                        supertask,
                        after_task_id,
                        start_rule,
                        prepared,
                    ],
                )
                .map_err(db)?;
            Ok(n > 0)
        })
        .await
    }

    async fn advance_automation_anchor(&self, id: &str, at: Timestamp) -> AppResult<()> {
        let id = id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE automations SET last_scheduled = MAX(COALESCE(last_scheduled, created_at), ?2) \
                 WHERE id = ?1",
                params![id, at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn delete_automation(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM automation_runs WHERE automation_id = ?1",
                [&id],
            )
            .map_err(db)?;
            let n = tx
                .execute("DELETE FROM automations WHERE id = ?1", [&id])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(n > 0)
        })
        .await
    }

    async fn insert_run(&self, run: &AutomationRunRecord) -> AppResult<bool> {
        let r = run.clone();
        self.call(move |c| {
            let n = c
                .execute(
                    &format!(
                        "INSERT OR IGNORE INTO automation_runs ({RUN_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
                    ),
                    params![
                        r.automation_id,
                        r.scheduled_for.0,
                        r.state.as_str(),
                        r.task_id.as_ref().map(TaskId::as_str),
                        r.error,
                        r.created_at.0,
                    ],
                )
                .map_err(db)?;
            Ok(n > 0)
        })
        .await
    }

    async fn automation_run(
        &self,
        automation: &str,
        scheduled_for: Timestamp,
    ) -> AppResult<Option<AutomationRunRecord>> {
        let automation = automation.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!(
                    "SELECT {RUN_COLS} FROM automation_runs WHERE automation_id = ?1 AND scheduled_for = ?2"
                ),
                params![automation, scheduled_for.0],
                run_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn finish_run(
        &self,
        automation: &str,
        scheduled_for: Timestamp,
        state: RunState,
        task: Option<&TaskId>,
        error: Option<&str>,
    ) -> AppResult<bool> {
        let (automation, task, error) = (
            automation.to_owned(),
            task.map(|t| t.as_str().to_owned()),
            error.map(str::to_owned),
        );
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE automation_runs SET state = ?3, task_id = ?4, error = ?5 \
                     WHERE automation_id = ?1 AND scheduled_for = ?2 AND state = 'scheduled'",
                    params![automation, scheduled_for.0, state.as_str(), task, error],
                )
                .map_err(db)?;
            Ok(n > 0)
        })
        .await
    }

    async fn runs(&self, automation: &str, limit: u32) -> AppResult<Vec<AutomationRunRecord>> {
        self.runs_query(
            format!(
                "SELECT {RUN_COLS} FROM automation_runs WHERE automation_id = ?1 \
                 ORDER BY created_at DESC, scheduled_for DESC LIMIT ?2"
            ),
            vec![automation.to_owned()],
            limit,
        )
        .await
    }

    async fn pending_runs(&self, limit: u32) -> AppResult<Vec<AutomationRunRecord>> {
        self.runs_query(
            format!(
                "SELECT {RUN_COLS} FROM automation_runs WHERE state = 'scheduled' \
                 ORDER BY created_at, scheduled_for LIMIT ?1"
            ),
            Vec::new(),
            limit,
        )
        .await
    }

    async fn last_task_run(&self, automation: &str) -> AppResult<Option<AutomationRunRecord>> {
        Ok(self
            .runs_query(
                format!(
                    "SELECT {RUN_COLS} FROM automation_runs WHERE automation_id = ?1 \
                     AND task_id IS NOT NULL ORDER BY created_at DESC, scheduled_for DESC LIMIT ?2"
                ),
                vec![automation.to_owned()],
                1,
            )
            .await?
            .pop())
    }

    async fn task_by_automation_run(&self, key: &str) -> AppResult<Option<TaskId>> {
        let key = key.to_owned();
        self.call(move |c| {
            let id: Option<String> = c
                .query_row(
                    "SELECT id FROM tasks WHERE automation_run = ?1",
                    [key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            id.map(|t| TaskId::parse(&t).map_err(|_| db(bad_column("task id", &t))))
                .transpose()
        })
        .await
    }

    async fn automation_audit(&self, automation: &str, limit: u32) -> AppResult<Vec<AuditEntry>> {
        let automation = automation.to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT at, actor, action, detail FROM audit_log WHERE target = ?1 \
                     ORDER BY id DESC LIMIT ?2",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map(params![automation, i64::from(limit)], |r| {
                    Ok(AuditEntry {
                        at: Timestamp(r.get(0)?),
                        actor: r.get(1)?,
                        action: r.get(2)?,
                        detail: r.get(3)?,
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }
}
