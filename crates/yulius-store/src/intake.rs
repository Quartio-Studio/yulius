//! Chained tasks, held prompts and Auto choices (SPEC-001 TSK-012, TSK-013, TSK-015; SPEC-002
//! ADR-16).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::auto_choice::AutoChoiceRecord;
use yulius_app::error::AppResult;
use yulius_app::intake::{ChainRecord, HoldRecord, hold_state};
use yulius_app::ports::store_ext::IntakeStore;
use yulius_domain::auto_choice::ChosenBy;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::intake::PromptClass;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_u32};

const CHAIN_COLS: &str = "task_id, after_task_id, after_number, blocked_reason, created_at, rule";
const AUTO_COLS: &str = "task_id, mode_by, workflow_by, workflow_name, created_at";
const HOLD_COLS: &str = "id, task_id, class, prompt, prompt_sha256, percent, typed_by, state, \
    created_at, resolved_by, resolved_at, new_task_id";

fn task_id(column: &str, text: &str) -> rusqlite::Result<TaskId> {
    TaskId::parse(text).map_err(|_| bad_column(column, text))
}

fn chain_from_row(row: &Row<'_>) -> rusqlite::Result<ChainRecord> {
    let task: String = row.get("task_id")?;
    let after: Option<String> = row.get("after_task_id")?;
    Ok(ChainRecord {
        task_id: task_id("task id", &task)?,
        after_task_id: after
            .as_deref()
            .map(|a| task_id("after task id", a))
            .transpose()?,
        after_number: to_u32(row.get("after_number")?),
        blocked_reason: row.get("blocked_reason")?,
        created_at: Timestamp(row.get("created_at")?),
        rule: row.get("rule")?,
    })
}

fn hold_from_row(row: &Row<'_>) -> rusqlite::Result<HoldRecord> {
    let task: String = row.get("task_id")?;
    let class: String = row.get("class")?;
    let typed_by: Option<String> = row.get("typed_by")?;
    let new_task: Option<String> = row.get("new_task_id")?;
    Ok(HoldRecord {
        id: row.get("id")?,
        task_id: task_id("task id", &task)?,
        class: PromptClass::parse(&class).ok_or_else(|| bad_column("prompt class", &class))?,
        prompt: row.get("prompt")?,
        prompt_sha256: row.get("prompt_sha256")?,
        percent: to_u32(row.get("percent")?),
        typed_by: typed_by
            .as_deref()
            .map(|u| UserId::parse(u).map_err(|_| bad_column("user id", u)))
            .transpose()?,
        state: row.get("state")?,
        created_at: Timestamp(row.get("created_at")?),
        resolved_by: row.get("resolved_by")?,
        resolved_at: row.get::<_, Option<i64>>("resolved_at")?.map(Timestamp),
        new_task_id: new_task
            .as_deref()
            .map(|t| task_id("new task id", t))
            .transpose()?,
    })
}

fn chosen_by(column: &str, text: Option<String>) -> rusqlite::Result<Option<ChosenBy>> {
    text.map(|t| ChosenBy::parse(&t).ok_or_else(|| bad_column(column, &t)))
        .transpose()
}

fn auto_from_row(row: &Row<'_>) -> rusqlite::Result<AutoChoiceRecord> {
    let task: String = row.get("task_id")?;
    Ok(AutoChoiceRecord {
        task_id: task_id("task id", &task)?,
        mode_by: chosen_by("auto mode", row.get("mode_by")?)?,
        workflow_by: chosen_by("auto workflow", row.get("workflow_by")?)?,
        workflow_name: row.get("workflow_name")?,
        created_at: Timestamp(row.get("created_at")?),
    })
}

#[async_trait]
impl IntakeStore for SqliteStore {
    async fn insert_chain(&self, chain: &ChainRecord) -> AppResult<()> {
        let c = chain.clone();
        self.call(move |conn| {
            conn.execute(
                &format!("INSERT INTO task_chains ({CHAIN_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"),
                params![
                    c.task_id.as_str(),
                    c.after_task_id.as_ref().map(TaskId::as_str),
                    i64::from(c.after_number),
                    c.blocked_reason,
                    c.created_at.0,
                    c.rule,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn chain(&self, task: &TaskId) -> AppResult<Option<ChainRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |conn| {
            conn.query_row(
                &format!("SELECT {CHAIN_COLS} FROM task_chains WHERE task_id = ?1"),
                [task],
                chain_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn chain_successors(&self, task: &TaskId) -> AppResult<Vec<ChainRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {CHAIN_COLS} FROM task_chains WHERE after_task_id = ?1 ORDER BY created_at, task_id"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([task], chain_from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn set_chain_blocked(&self, task: &TaskId, reason: Option<&str>) -> AppResult<()> {
        let (task, reason) = (task.as_str().to_owned(), reason.map(str::to_owned));
        self.call(move |conn| {
            conn.execute(
                "UPDATE task_chains SET blocked_reason = ?1 WHERE task_id = ?2",
                params![reason, task],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn insert_hold(&self, hold: &HoldRecord) -> AppResult<()> {
        let h = hold.clone();
        self.call(move |conn| {
            let tx = conn.transaction().map_err(db)?;
            tx.execute(
                "UPDATE prompt_holds SET state = ?1, resolved_at = ?2, resolved_by = 'system' \
                 WHERE task_id = ?3 AND state = ?4",
                params![
                    hold_state::SUPERSEDED,
                    h.created_at.0,
                    h.task_id.as_str(),
                    hold_state::PENDING
                ],
            )
            .map_err(db)?;
            tx.execute(
                &format!(
                    "INSERT INTO prompt_holds ({HOLD_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    h.id,
                    h.task_id.as_str(),
                    h.class.as_str(),
                    h.prompt,
                    h.prompt_sha256,
                    i64::from(h.percent),
                    h.typed_by.as_ref().map(UserId::as_str),
                    h.state,
                    h.created_at.0,
                    h.resolved_by,
                    h.resolved_at.map(|t| t.0),
                    h.new_task_id.as_ref().map(TaskId::as_str),
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn hold(&self, id: &str) -> AppResult<Option<HoldRecord>> {
        let id = id.to_owned();
        self.call(move |conn| {
            conn.query_row(
                &format!("SELECT {HOLD_COLS} FROM prompt_holds WHERE id = ?1"),
                [id],
                hold_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn pending_hold(&self, task: &TaskId) -> AppResult<Option<HoldRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |conn| {
            conn.query_row(
                &format!(
                    "SELECT {HOLD_COLS} FROM prompt_holds WHERE task_id = ?1 AND state = ?2 \
                     ORDER BY created_at DESC LIMIT 1"
                ),
                params![task, hold_state::PENDING],
                hold_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn resolve_hold(
        &self,
        id: &str,
        expected: &str,
        state: &str,
        by: &str,
        at: Timestamp,
        new_task: Option<&TaskId>,
    ) -> AppResult<bool> {
        let (id, expected, state, by) = (
            id.to_owned(),
            expected.to_owned(),
            state.to_owned(),
            by.to_owned(),
        );
        let new_task = new_task.map(|t| t.as_str().to_owned());
        self.call(move |conn| {
            let pending = state == hold_state::PENDING;
            let n = conn
                .execute(
                    "UPDATE prompt_holds SET state = ?1, \
                     resolved_by = CASE WHEN ?2 THEN NULL ELSE ?3 END, \
                     resolved_at = CASE WHEN ?2 THEN NULL ELSE ?4 END, \
                     new_task_id = COALESCE(?5, new_task_id) \
                     WHERE id = ?6 AND state = ?7",
                    params![state, pending, by, at.0, new_task, id, expected],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn take_sent_hold(
        &self,
        task: &TaskId,
        sha256: &str,
        since: Timestamp,
    ) -> AppResult<bool> {
        let (task, sha) = (task.as_str().to_owned(), sha256.to_owned());
        self.call(move |conn| {
            let n = conn
                .execute(
                    "UPDATE prompt_holds SET state = ?1 WHERE id = (SELECT id FROM prompt_holds \
                     WHERE task_id = ?2 AND prompt_sha256 = ?3 AND state = ?4 AND resolved_at >= ?5 \
                     ORDER BY resolved_at DESC LIMIT 1)",
                    params![hold_state::CONSUMED, task, sha, hold_state::SENT, since.0],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn insert_auto_choice(&self, choice: &AutoChoiceRecord) -> AppResult<()> {
        let c = choice.clone();
        self.call(move |conn| {
            conn.execute(
                &format!("INSERT INTO task_auto_choices ({AUTO_COLS}) VALUES (?1, ?2, ?3, ?4, ?5)"),
                params![
                    c.task_id.as_str(),
                    c.mode_by.map(ChosenBy::as_str),
                    c.workflow_by.map(ChosenBy::as_str),
                    c.workflow_name,
                    c.created_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn auto_choice(&self, task: &TaskId) -> AppResult<Option<AutoChoiceRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |conn| {
            conn.query_row(
                &format!("SELECT {AUTO_COLS} FROM task_auto_choices WHERE task_id = ?1"),
                [task],
                auto_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }
}
