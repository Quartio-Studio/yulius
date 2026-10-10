//! Subscription pool persistence (SPEC-002 ADR-16): pauses for lack of capacity
//! (`capacity_pauses`, at most one open per task) and provider usage readings
//! (`usage_samples`, kept 8 days).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::{PauseKind, PauseRecord, UsageSampleRecord};
use yulius_app::ports::store_ext::CapacityStore;
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const PAUSE_COLS: &str =
    "id, task_id, kind, account_id, continuation, started_at, ended_at, outcome";

fn pause_from_row(row: &Row<'_>) -> rusqlite::Result<PauseRecord> {
    let task: String = row.get("task_id")?;
    let account: Option<String> = row.get("account_id")?;
    let kind: String = row.get("kind")?;
    Ok(PauseRecord {
        id: row.get("id")?,
        task_id: TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?,
        kind: PauseKind::parse(&kind),
        account_id: account
            .map(|a| AccountId::parse(&a).map_err(|_| bad_column("account id", &a)))
            .transpose()?,
        continuation: row.get("continuation")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        outcome: row.get("outcome")?,
    })
}

#[async_trait]
impl CapacityStore for SqliteStore {
    async fn insert_pause(&self, pause: &PauseRecord) -> AppResult<bool> {
        let p = pause.clone();
        self.call(move |c| {
            let inserted = c
                .execute(
                    &format!(
                        "INSERT OR IGNORE INTO capacity_pauses ({PAUSE_COLS}) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)"
                    ),
                    params![
                        p.id,
                        p.task_id.as_str(),
                        p.kind.as_str(),
                        p.account_id.as_ref().map(AccountId::as_str),
                        p.continuation,
                        p.started_at.0
                    ],
                )
                .map_err(db)?;
            Ok(inserted > 0)
        })
        .await
    }

    async fn open_pause(&self, task: &TaskId) -> AppResult<Option<PauseRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                &format!(
                    "SELECT {PAUSE_COLS} FROM capacity_pauses WHERE task_id = ?1 AND ended_at IS NULL"
                ),
                [task],
                pause_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn open_pauses(&self) -> AppResult<Vec<PauseRecord>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {PAUSE_COLS} FROM capacity_pauses WHERE ended_at IS NULL \
                     ORDER BY started_at, id"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([], pause_from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn update_open_pause(
        &self,
        task: &TaskId,
        kind: PauseKind,
        continuation: Option<&str>,
    ) -> AppResult<bool> {
        let (task, continuation) = (task.as_str().to_owned(), continuation.map(str::to_owned));
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE capacity_pauses SET kind = ?1, continuation = ?2 \
                     WHERE task_id = ?3 AND ended_at IS NULL",
                    params![kind.as_str(), continuation, task],
                )
                .map_err(db)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn end_pause(&self, task: &TaskId, at: Timestamp, outcome: &str) -> AppResult<bool> {
        let (task, outcome) = (task.as_str().to_owned(), outcome.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE capacity_pauses SET ended_at = ?1, outcome = ?2, continuation = NULL \
                     WHERE task_id = ?3 AND ended_at IS NULL",
                    params![at.0, outcome, task],
                )
                .map_err(db)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn pauses_since(&self, since: Timestamp) -> AppResult<Vec<PauseRecord>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {PAUSE_COLS} FROM capacity_pauses \
                     WHERE ended_at IS NULL OR ended_at >= ?1 ORDER BY started_at, id"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([since.0], pause_from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn record_usage_samples(
        &self,
        samples: &[UsageSampleRecord],
        keep_after: Timestamp,
    ) -> AppResult<()> {
        let samples = samples.to_vec();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            for s in &samples {
                tx.execute(
                    "INSERT INTO usage_samples (account_id, at, window_name, used_percent, resets_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        s.account_id.as_str(),
                        s.at.0,
                        s.window,
                        s.used_percent,
                        s.resets_at.map(|t| t.0)
                    ],
                )
                .map_err(db)?;
            }
            tx.execute("DELETE FROM usage_samples WHERE at < ?1", [keep_after.0])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn usage_samples(&self, since: Timestamp) -> AppResult<Vec<UsageSampleRecord>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT account_id, at, window_name, used_percent, resets_at FROM usage_samples \
                     WHERE at >= ?1 ORDER BY at, account_id",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([since.0], |row| {
                    let account: String = row.get(0)?;
                    Ok(UsageSampleRecord {
                        account_id: AccountId::parse(&account)
                            .map_err(|_| bad_column("account id", &account))?,
                        at: Timestamp(row.get(1)?),
                        window: row.get(2)?,
                        used_percent: row.get(3)?,
                        resets_at: row.get::<_, Option<i64>>(4)?.map(Timestamp),
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn attempt_spans(
        &self,
        since: Timestamp,
    ) -> AppResult<Vec<(Timestamp, Option<Timestamp>)>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT started_at, ended_at FROM attempts \
                     WHERE ended_at IS NULL AND state IN ('spawning', 'live') OR ended_at >= ?1",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([since.0], |row| {
                    Ok((
                        Timestamp(row.get(0)?),
                        row.get::<_, Option<i64>>(1)?.map(Timestamp),
                    ))
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }
}
