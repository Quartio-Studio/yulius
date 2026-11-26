//! Tasks hand work to another sandbox (SPEC-512 part 5, migration 0076): the instance's setting
//! and every handoff.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{HandoffRecord, HandoffStore};
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, text_parsed};

const COLUMNS: &str = "id, source_task, source_sandbox, target_sandbox, owner, args, chain, depth, \
                       state, reason, target_task, created_at";

fn handoff_row(row: &Row<'_>) -> rusqlite::Result<HandoffRecord> {
    let parse = |i: usize, what: &str| -> rusqlite::Result<String> {
        let text: String = row.get(i)?;
        if text.is_empty() {
            return Err(bad_column(what, &text));
        }
        Ok(text)
    };
    let source_task = parse(1, "source task")?;
    let source_sandbox = parse(2, "source sandbox")?;
    let target_sandbox = parse(3, "target sandbox")?;
    let owner = parse(4, "owner")?;
    let target: Option<String> = row.get(10)?;
    Ok(HandoffRecord {
        id: row.get(0)?,
        source_task: TaskId::parse(&source_task)
            .map_err(|_| bad_column("task id", &source_task))?,
        source_sandbox: SandboxId::parse(&source_sandbox)
            .map_err(|_| bad_column("sandbox id", &source_sandbox))?,
        target_sandbox: SandboxId::parse(&target_sandbox)
            .map_err(|_| bad_column("sandbox id", &target_sandbox))?,
        owner: UserId::parse(&owner).map_err(|_| bad_column("user id", &owner))?,
        args: row.get(5)?,
        chain: text_parsed(row, 6, "chain", |text| serde_json::from_str(text))?,
        depth: row.get(7)?,
        state: row.get(8)?,
        reason: row.get(9)?,
        target_task: target
            .map(|id| TaskId::parse(&id).map_err(|_| bad_column("task id", &id)))
            .transpose()?,
        created_at: Timestamp(row.get(11)?),
    })
}

#[async_trait]
impl HandoffStore for SqliteStore {
    async fn handoff_setting(&self) -> AppResult<Option<String>> {
        self.call(|c| {
            let doc = c
                .query_row(
                    "SELECT handoffs FROM instance WHERE singleton = 1",
                    [],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?;
            Ok(doc.flatten())
        })
        .await
    }

    async fn set_handoff_setting(&self, doc: &str) -> AppResult<()> {
        let doc = doc.to_owned();
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE instance SET handoffs = ?1 WHERE singleton = 1",
                [doc],
            )?;
            if changed == 0 {
                return Err(AppError::Conflict("the instance is not set up yet".into()).into());
            }
            Ok(())
        })
        .await
    }

    async fn insert_handoff(&self, h: &HandoffRecord) -> AppResult<()> {
        let h = h.clone();
        self.call(move |c| {
            let chain = serde_json::to_string(&h.chain).map_err(AppError::internal)?;
            c.execute(
                &format!(
                    "INSERT INTO task_handoffs ({COLUMNS}) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    h.id,
                    h.source_task.as_str(),
                    h.source_sandbox.as_str(),
                    h.target_sandbox.as_str(),
                    h.owner.as_str(),
                    h.args,
                    chain,
                    h.depth,
                    h.state,
                    h.reason,
                    h.target_task.as_ref().map(TaskId::as_str),
                    h.created_at.0,
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn handoff(&self, id: &str) -> AppResult<Option<HandoffRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLUMNS} FROM task_handoffs WHERE id = ?1"),
                [id],
                handoff_row,
            )
            .optional()?)
        })
        .await
    }

    async fn handoffs_of_source(&self, task: &TaskId) -> AppResult<Vec<HandoffRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM task_handoffs WHERE source_task = ?1 \
                     ORDER BY created_at DESC, rowid DESC LIMIT 50"
            ))?;
            let rows = stmt
                .query_map([task], handoff_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn handoff_of_target(&self, task: &TaskId) -> AppResult<Option<HandoffRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLUMNS} FROM task_handoffs WHERE target_task = ?1"),
                [task],
                handoff_row,
            )
            .optional()?)
        })
        .await
    }

    async fn decide_handoff(
        &self,
        id: &str,
        from: &str,
        to: &str,
        reason: Option<&str>,
        target_task: Option<&TaskId>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, from, to) = (id.to_owned(), from.to_owned(), to.to_owned());
        let reason = reason.map(str::to_owned);
        let target = target_task.map(|t| t.as_str().to_owned());
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE task_handoffs SET state = ?3, reason = ?4, \
                     target_task = COALESCE(?5, target_task), decided_at = ?6 \
                     WHERE id = ?1 AND state = ?2",
                params![id, from, to, reason, target, at.0],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn handoffs_since(&self, owner: &UserId, since: Timestamp) -> AppResult<u32> {
        let owner = owner.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM task_handoffs \
                 WHERE owner = ?1 AND created_at >= ?2 AND state <> 'refused'",
                params![owner, since.0],
                |r| r.get(0),
            )?)
        })
        .await
    }

    async fn untold_handoffs(&self) -> AppResult<Vec<HandoffRecord>> {
        self.call(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM task_handoffs \
                     WHERE state = 'created' AND outcome_told = 0 AND target_task IS NOT NULL"
            ))?;
            let rows = stmt
                .query_map([], handoff_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn settle_creating_handoffs(&self, at: Timestamp) -> AppResult<u64> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let made = tx.execute(
                "UPDATE task_handoffs SET state = 'created', decided_at = ?1 \
                     WHERE state = 'creating' AND target_task IS NOT NULL",
                [at.0],
            )?;
            let lost = tx.execute(
                "UPDATE task_handoffs SET state = 'refused', decided_at = ?1, \
                     reason = 'interrupted before its task was made: ask again' \
                     WHERE state = 'creating' AND target_task IS NULL",
                [at.0],
            )?;
            tx.commit()?;
            Ok(u64::try_from(made + lost).unwrap_or(u64::MAX))
        })
        .await
    }

    async fn mark_handoff_told(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE task_handoffs SET outcome_told = 1 WHERE id = ?1 AND outcome_told = 0",
                [id],
            )?;
            Ok(changed == 1)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn record(id: &str, state: &str, at: i64) -> Result<HandoffRecord, Box<dyn std::error::Error>> {
        Ok(HandoffRecord {
            id: id.into(),
            source_task: TaskId::parse("t1")?,
            source_sandbox: SandboxId::parse("s1")?,
            target_sandbox: SandboxId::parse("s2")?,
            owner: UserId::parse("u1")?,
            args: r#"{"title":"T","prompt":"P"}"#.into(),
            chain: vec!["s1".into()],
            depth: 1,
            state: state.into(),
            reason: None,
            target_task: None,
            created_at: Timestamp(at),
        })
    }

    /// SPEC-512 HOF-001, HOF-004, HOF-005: the setting is kept; a handoff is decided once; the
    /// day's count leaves refused ones out; a created one's outcome is told once.
    #[tokio::test]
    async fn a_handoff_is_decided_once_and_told_once() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("y.db");
        let store = SqliteStore::open(&path)?;
        Connection::open(&path)?.execute_batch(
            "INSERT INTO instance (singleton, public_url) VALUES (1, 'https://yulius.test')",
        )?;
        assert_eq!(store.handoff_setting().await?, None);
        store.set_handoff_setting(r#"{"mode":"ask"}"#).await?;
        assert_eq!(
            store.handoff_setting().await?.as_deref(),
            Some(r#"{"mode":"ask"}"#)
        );

        store
            .insert_handoff(&record("h1", "requested", 10)?)
            .await?;
        store.insert_handoff(&record("h2", "refused", 11)?).await?;
        let owner = UserId::parse("u1")?;
        assert_eq!(
            store.handoffs_since(&owner, Timestamp(0)).await?,
            1,
            "refused left out"
        );
        let made = TaskId::parse("t9")?;
        assert!(
            store
                .decide_handoff(
                    "h1",
                    "requested",
                    "created",
                    None,
                    Some(&made),
                    Timestamp(12)
                )
                .await?
        );
        assert!(
            !store
                .decide_handoff(
                    "h1",
                    "requested",
                    "declined",
                    Some("no"),
                    None,
                    Timestamp(13)
                )
                .await?,
            "decided once"
        );
        let h1 = store.handoff("h1").await?.ok_or("h1")?;
        assert_eq!(
            (h1.state.as_str(), h1.target_task.as_ref()),
            ("created", Some(&made))
        );
        assert_eq!(
            store.handoff_of_target(&made).await?.map(|h| h.id),
            Some("h1".into())
        );
        assert_eq!(
            store.handoffs_of_source(&TaskId::parse("t1")?).await?.len(),
            2
        );
        assert_eq!(store.untold_handoffs().await?.len(), 1);
        // A claim a stopped server left: refused when no task was made (review round 2).
        store.insert_handoff(&record("h3", "creating", 14)?).await?;
        assert_eq!(store.settle_creating_handoffs(Timestamp(15)).await?, 1);
        let h3 = store.handoff("h3").await?.ok_or("h3")?;
        assert_eq!(h3.state, "refused");
        assert!(h3.reason.is_some_and(|r| r.contains("ask again")));
        assert!(store.mark_handoff_told("h1").await?);
        assert!(!store.mark_handoff_told("h1").await?);
        assert!(store.untold_handoffs().await?.is_empty());
        Ok(())
    }
}
