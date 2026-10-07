//! Steps, messages and scope claims.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{InboxEntry, MessageRecord, ScopeClaimRecord, StepRecord};
use yulius_app::ports::store::CoordinationStore;
use yulius_domain::ids::{MessageId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_u32};

fn step_from_row(row: &Row<'_>) -> rusqlite::Result<StepRecord> {
    let id: String = row.get("id")?;
    let task: String = row.get("task_id")?;
    Ok(StepRecord {
        id: StepId::parse(&id).map_err(|_| bad_column("step id", &id))?,
        task_id: TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?,
        ordinal: to_u32(row.get("ordinal")?),
        title: row.get("title")?,
        status: row.get("status")?,
        note: row.get("note")?,
        updated_by: row.get("updated_by")?,
        updated_at: row.get::<_, Option<i64>>("updated_at")?.map(Timestamp),
    })
}

const STEP_COLS: &str = "id, task_id, ordinal, title, status, note, updated_by, updated_at";

fn opt_task(text: Option<String>) -> rusqlite::Result<Option<TaskId>> {
    text.as_deref()
        .map(|t| TaskId::parse(t).map_err(|_| bad_column("task id", t)))
        .transpose()
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<MessageRecord> {
    let id: String = row.get("id")?;
    let sandbox: String = row.get("sandbox_id")?;
    let from_user: Option<String> = row.get("from_user")?;
    Ok(MessageRecord {
        id: MessageId::parse(&id).map_err(|_| bad_column("message id", &id))?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        from_task: opt_task(row.get("from_task")?)?,
        from_user: from_user
            .as_deref()
            .map(|u| UserId::parse(u).map_err(|_| bad_column("user id", u)))
            .transpose()?,
        to_task: opt_task(row.get("to_task")?)?,
        kind: row.get("kind")?,
        subject: row.get("subject")?,
        body: row.get("body")?,
        created_at: Timestamp(row.get("created_at")?),
    })
}

fn inbox_from_row(row: &Row<'_>) -> rusqlite::Result<InboxEntry> {
    Ok(InboxEntry {
        message: message_from_row(row)?,
        delivered: row.get::<_, Option<i64>>("delivered_at")?.is_some(),
        read: row.get::<_, Option<i64>>("read_at")?.is_some(),
    })
}

const MSG_COLS: &str = "m.id, m.sandbox_id, m.from_task, m.from_user, m.to_task, m.kind, m.subject, m.body, m.created_at";

#[async_trait]
impl CoordinationStore for SqliteStore {
    async fn replace_steps(
        &self,
        task: &TaskId,
        steps: &[StepRecord],
        actor: &str,
        at: Timestamp,
    ) -> AppResult<()> {
        let (task, steps, actor) = (task.as_str().to_owned(), steps.to_vec(), actor.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let mut old = std::collections::BTreeMap::new();
            {
                let mut stmt = tx.prepare("SELECT id, status FROM steps WHERE task_id = ?1").map_err(db)?;
                let rows = stmt.query_map([&task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(db)?;
                for row in rows {
                    let (id, status) = row.map_err(db)?;
                    old.insert(id, status);
                }
            }
            tx.execute("DELETE FROM steps WHERE task_id = ?1", [&task]).map_err(db)?;
            for s in &steps {
                tx.execute(
                    &format!("INSERT INTO steps ({STEP_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                    params![s.id.as_str(), task, i64::from(s.ordinal), s.title, s.status, s.note, s.updated_by, s.updated_at.map(|t| t.0)],
                )
                .map_err(db)?;
                let previous = old.get(s.id.as_str());
                if previous.map(String::as_str) != Some(s.status.as_str()) {
                    tx.execute(
                        "INSERT INTO step_events (step_id, task_id, at, actor, from_status, to_status, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![s.id.as_str(), task, at.0, actor, previous, s.status, s.note],
                    )
                    .map_err(db)?;
                }
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn steps(&self, task: &TaskId) -> AppResult<Vec<StepRecord>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {STEP_COLS} FROM steps WHERE task_id = ?1 ORDER BY ordinal"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([t], step_from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn update_step(
        &self,
        step: &StepId,
        status: &str,
        note: Option<&str>,
        actor: &str,
        at: Timestamp,
    ) -> AppResult<StepRecord> {
        let (id, status, note, actor) = (
            step.as_str().to_owned(),
            status.to_owned(),
            note.map(str::to_owned),
            actor.to_owned(),
        );
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let current = tx
                .query_row(&format!("SELECT {STEP_COLS} FROM steps WHERE id = ?1"), [&id], step_from_row)
                .optional()
                .map_err(db)?
                .ok_or(AppError::NotFound)?;
            tx.execute(
                "UPDATE steps SET status = ?1, note = COALESCE(?2, note), updated_by = ?3, updated_at = ?4 WHERE id = ?5",
                params![status, note, actor, at.0, id],
            )
            .map_err(db)?;
            tx.execute(
                "INSERT INTO step_events (step_id, task_id, at, actor, from_status, to_status, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, current.task_id.as_str(), at.0, actor, current.status, status, note],
            )
            .map_err(db)?;
            let updated = tx.query_row(&format!("SELECT {STEP_COLS} FROM steps WHERE id = ?1"), [&id], step_from_row).map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(updated)
        })
        .await
    }

    async fn insert_message(&self, m: &MessageRecord, recipients: &[TaskId]) -> AppResult<()> {
        let (m, recipients) = (m.clone(), recipients.to_vec());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "INSERT INTO messages (id, sandbox_id, from_task, from_user, to_task, kind, subject, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    m.id.as_str(),
                    m.sandbox_id.as_str(),
                    m.from_task.as_ref().map(TaskId::as_str),
                    m.from_user.as_ref().map(UserId::as_str),
                    m.to_task.as_ref().map(TaskId::as_str),
                    m.kind,
                    m.subject,
                    m.body,
                    m.created_at.0
                ],
            )
            .map_err(db)?;
            for r in &recipients {
                tx.execute("INSERT OR IGNORE INTO message_receipts (message_id, task_id) VALUES (?1, ?2)", params![m.id.as_str(), r.as_str()])
                    .map_err(db)?;
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn inbox(
        &self,
        task: &TaskId,
        unread_only: bool,
        limit: u32,
    ) -> AppResult<Vec<InboxEntry>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let filter = if unread_only { "AND r.read_at IS NULL" } else { "" };
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {MSG_COLS}, r.delivered_at, r.read_at FROM message_receipts r JOIN messages m ON m.id = r.message_id \
                     WHERE r.task_id = ?1 {filter} ORDER BY m.created_at DESC LIMIT ?2"
                ))
                .map_err(db)?;
            let rows = stmt.query_map(params![t, i64::from(limit)], inbox_from_row).map_err(db)?;
            let mut out = rows.collect::<Result<Vec<_>, _>>().map_err(db)?;
            out.reverse();
            Ok(out)
        })
        .await
    }

    async fn mark_delivered(&self, task: &TaskId, at: Timestamp) -> AppResult<u32> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let n = c
                .execute("UPDATE message_receipts SET delivered_at = ?1 WHERE task_id = ?2 AND delivered_at IS NULL", params![at.0, t])
                .map_err(db)?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn mark_read(&self, task: &TaskId, at: Timestamp) -> AppResult<u32> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE message_receipts SET read_at = ?1, delivered_at = COALESCE(delivered_at, ?1) WHERE task_id = ?2 AND read_at IS NULL",
                    params![at.0, t],
                )
                .map_err(db)?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn unread_count(&self, task: &TaskId) -> AppResult<u32> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM message_receipts WHERE task_id = ?1 AND read_at IS NULL",
                    [t],
                    |r| r.get(0),
                )
                .map_err(db)?;
            Ok(to_u32(n))
        })
        .await
    }

    async fn task_messages(&self, task: &TaskId, limit: u32) -> AppResult<Vec<InboxEntry>> {
        let t = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {MSG_COLS}, r.delivered_at, r.read_at FROM messages m \
                     LEFT JOIN message_receipts r ON r.message_id = m.id AND r.task_id = ?1 \
                     WHERE r.task_id IS NOT NULL OR m.from_task = ?1 ORDER BY m.created_at DESC LIMIT ?2"
                ))
                .map_err(db)?;
            let rows = stmt.query_map(params![t, i64::from(limit)], inbox_from_row).map_err(db)?;
            let mut out = rows.collect::<Result<Vec<_>, _>>().map_err(db)?;
            out.reverse();
            Ok(out)
        })
        .await
    }

    async fn insert_claim(&self, claim: &ScopeClaimRecord) -> AppResult<()> {
        let cl = claim.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO scope_claims (id, task_id, sandbox_id, pattern, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cl.id, cl.task_id.as_str(), cl.sandbox_id.as_str(), cl.pattern, cl.note, cl.created_at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn active_claims(&self, sandbox: &SandboxId) -> AppResult<Vec<ScopeClaimRecord>> {
        let s = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare("SELECT id, task_id, sandbox_id, pattern, note, created_at FROM scope_claims WHERE sandbox_id = ?1 AND released_at IS NULL ORDER BY created_at")
                .map_err(db)?;
            let rows = stmt
                .query_map([s], |r| {
                    let t: String = r.get(1)?;
                    let sb: String = r.get(2)?;
                    Ok(ScopeClaimRecord {
                        id: r.get(0)?,
                        task_id: TaskId::parse(&t).map_err(|_| bad_column("task id", &t))?,
                        sandbox_id: SandboxId::parse(&sb).map_err(|_| bad_column("sandbox id", &sb))?,
                        pattern: r.get(3)?,
                        note: r.get(4)?,
                        created_at: Timestamp(r.get(5)?),
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn release_claims(
        &self,
        task: &TaskId,
        pattern: Option<&str>,
        at: Timestamp,
    ) -> AppResult<u32> {
        let (t, p) = (task.as_str().to_owned(), pattern.map(str::to_owned));
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE scope_claims SET released_at = ?1 WHERE task_id = ?2 AND released_at IS NULL AND (?3 IS NULL OR pattern = ?3)",
                    params![at.0, t, p],
                )
                .map_err(db)?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }
}
