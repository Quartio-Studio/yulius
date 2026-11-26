//! Subscription pool persistence (SPEC-002 ADR-17): pauses for lack of capacity
//! (`capacity_pauses`, at most one open per task) and provider usage readings
//! (`usage_samples`, kept 8 days).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::{PauseKind, PauseRecord, TokenRunRecord, UsageSampleRecord};
use yulius_app::ports::store_ext::CapacityStore;
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column};

/// The pauses that were open since `?1`, with the columns `cols` (`pauses_since`): the open
/// ones and those that ended at or after it, as two branches that each read the `ended_at`
/// index. Unordered: an `ORDER BY` (or one `OR`) made SQLite scan the whole table in
/// `started_at` order, so the caller sorts the few rows.
pub(crate) fn pauses_since_sql(cols: &str) -> String {
    format!(
        "SELECT {cols} FROM capacity_pauses WHERE ended_at IS NULL \
         UNION ALL SELECT {cols} FROM capacity_pauses WHERE ended_at >= ?1"
    )
}

/// The runs that count towards the capacity view since `?1`: the live ones and those that ended
/// at or after it, with their metrics (`token_runs`).
pub(crate) const TOKEN_RUNS_SQL: &str = "SELECT a.account_id, a.started_at, a.ended_at, m.attempt_id AS metrics_attempt, \
     m.model, m.input_tokens, m.output_tokens, m.cache_read, m.cache_write, \
     m.turns, m.active_ms, m.model_parts FROM attempts a LEFT JOIN metrics m ON m.attempt_id = a.id \
     WHERE a.ended_at IS NULL AND a.state IN ('spawning', 'live') OR a.ended_at >= ?1";

pub(crate) const PAUSE_COLS: &str =
    "id, task_id, kind, account_id, continuation, started_at, ended_at, outcome, during_turn";

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
        during_turn: row.get("during_turn")?,
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
            let inserted = c.execute(
                &format!(
                    "INSERT OR IGNORE INTO capacity_pauses ({PAUSE_COLS}) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7)"
                ),
                params![
                    p.id,
                    p.task_id.as_str(),
                    p.kind.as_str(),
                    p.account_id.as_ref().map(AccountId::as_str),
                    p.continuation,
                    p.started_at.0,
                    p.during_turn
                ],
            )?;
            Ok(inserted > 0)
        })
        .await
    }

    async fn open_pause(&self, task: &TaskId) -> AppResult<Option<PauseRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {PAUSE_COLS} FROM capacity_pauses WHERE task_id = ?1 AND ended_at IS NULL"
                ),
                [task],
                pause_from_row,
            )
            .optional()?)
        })
        .await
    }

    async fn open_pause_with_process(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<(PauseRecord, bool)>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {PAUSE_COLS}, EXISTS (SELECT 1 FROM attempts WHERE task_id = ?1 \
                     AND state IN ('spawning', 'live')) AS has_process FROM capacity_pauses \
                     WHERE task_id = ?1 AND ended_at IS NULL"
                ),
                [task],
                |row| Ok((pause_from_row(row)?, row.get("has_process")?)),
            )
            .optional()?)
        })
        .await
    }

    async fn open_pauses(&self) -> AppResult<Vec<PauseRecord>> {
        self.call(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {PAUSE_COLS} FROM capacity_pauses WHERE ended_at IS NULL \
                     ORDER BY started_at, id"
            ))?;
            let rows = stmt.query_map([], pause_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
            let changed = c.execute(
                "UPDATE capacity_pauses SET kind = ?1, continuation = ?2, during_turn = 0 \
                     WHERE task_id = ?3 AND ended_at IS NULL",
                params![kind.as_str(), continuation, task],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn end_pause_if(
        &self,
        task: &TaskId,
        observed: Option<&str>,
        at: Timestamp,
        outcome: &str,
    ) -> AppResult<bool> {
        let (task, outcome) = (task.as_str().to_owned(), outcome.to_owned());
        let observed = observed.map(str::to_owned);
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE capacity_pauses SET ended_at = ?1, outcome = ?2, continuation = NULL \
                     WHERE task_id = ?3 AND ended_at IS NULL AND (?4 IS NULL OR id = ?4)",
                params![at.0, outcome, task, observed],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn pauses_since(&self, since: Timestamp) -> AppResult<Vec<PauseRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&pauses_since_sql(PAUSE_COLS))?;
            let rows = stmt.query_map([since.0], pause_from_row)?;
            let mut pauses = rows.collect::<Result<Vec<_>, _>>()?;
            pauses.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
            Ok(pauses)
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
            let tx = c.transaction()?;
            for s in &samples {
                tx.execute(
                    "INSERT INTO usage_samples \
                     (account_id, at, window_name, used_percent, resets_at, source) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        s.account_id.as_str(),
                        s.at.0,
                        s.window,
                        s.used_percent,
                        s.resets_at.map(|t| t.0),
                        s.source
                    ],
                )?;
            }
            tx.execute("DELETE FROM usage_samples WHERE at < ?1", [keep_after.0])?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn usage_samples(&self, since: Timestamp) -> AppResult<Vec<UsageSampleRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT account_id, at, window_name, used_percent, resets_at, source \
                     FROM usage_samples WHERE at >= ?1 ORDER BY at, account_id",
            )?;
            let rows = stmt.query_map([since.0], |row| {
                let account: String = row.get(0)?;
                Ok(UsageSampleRecord {
                    account_id: AccountId::parse(&account)
                        .map_err(|_| bad_column("account id", &account))?,
                    at: Timestamp(row.get(1)?),
                    window: row.get(2)?,
                    used_percent: row.get(3)?,
                    resets_at: row.get::<_, Option<i64>>(4)?.map(Timestamp),
                    source: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn token_runs(&self, since: Timestamp) -> AppResult<Vec<TokenRunRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(TOKEN_RUNS_SQL)?;
            let rows = stmt.query_map([since.0], |row| {
                let account: Option<String> = row.get("account_id")?;
                let metrics: Option<String> = row.get("metrics_attempt")?;
                Ok(TokenRunRecord {
                    account_id: account
                        .map(|a| AccountId::parse(&a).map_err(|_| bad_column("account id", &a)))
                        .transpose()?,
                    started_at: Timestamp(row.get("started_at")?),
                    ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
                    metrics: metrics
                        .map(|_| crate::tasks::metrics_from_row(row))
                        .transpose()?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrating_a_prompt_already_held_waits_for_its_turn_record() -> rusqlite::Result<()> {
        let db = rusqlite::Connection::open_in_memory()?;
        db.execute_batch(
            "CREATE TABLE capacity_pauses (id TEXT, kind TEXT, ended_at INTEGER);
             INSERT INTO capacity_pauses VALUES ('held', 'prompt', NULL),
                ('done', 'prompt', 99), ('limit', 'limit', NULL);",
        )?;
        db.execute_batch(include_str!("migrations/0075_prompt_held_during_turn.sql"))?;
        let mut rows = db.prepare("SELECT id, during_turn FROM capacity_pauses ORDER BY id")?;
        let values = rows
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(
            values,
            [
                ("done".into(), false),
                ("held".into(), true),
                ("limit".into(), false)
            ]
        );
        Ok(())
    }
}
