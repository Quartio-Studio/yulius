//! Instance updates and the saved Jev key (SPEC-002 ADR-18, SPEC-001 ADM-002): `update_runs`
//! (at most one open run) and the sealed key in the `instance` row.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::jev_key::StoredJevKey;
use yulius_app::model::Sealed;
use yulius_app::ports::store_ext::UpdateStore;
use yulius_app::updates::UpdateRunRecord;
use yulius_domain::time::Timestamp;
use yulius_domain::update::RunState;

use crate::{SqliteStore, bad_column, db};

const RUN_COLS: &str = "id, target_commit, from_commit, summary, state, outcome, error, \
    requested_by, created_at, state_since, finished_at";

fn state(column: &str, text: &str) -> rusqlite::Result<RunState> {
    RunState::parse(text).ok_or_else(|| bad_column(column, text))
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<UpdateRunRecord> {
    let current: String = row.get("state")?;
    let outcome: Option<String> = row.get("outcome")?;
    Ok(UpdateRunRecord {
        id: row.get("id")?,
        target_commit: row.get("target_commit")?,
        from_commit: row.get("from_commit")?,
        summary: row.get("summary")?,
        state: state("update state", &current)?,
        outcome: outcome.map(|o| state("update outcome", &o)).transpose()?,
        error: row.get("error")?,
        requested_by: row.get("requested_by")?,
        created_at: Timestamp(row.get("created_at")?),
        state_since: Timestamp(row.get("state_since")?),
        finished_at: row.get::<_, Option<i64>>("finished_at")?.map(Timestamp),
    })
}

#[async_trait]
impl UpdateStore for SqliteStore {
    async fn insert_update_run(&self, run: &UpdateRunRecord) -> AppResult<bool> {
        let r = run.clone();
        self.call(move |c| {
            let inserted = c
                .execute(
                    &format!(
                        "INSERT OR IGNORE INTO update_runs ({RUN_COLS}) \
                         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?7, NULL)"
                    ),
                    params![
                        r.id,
                        r.target_commit,
                        r.from_commit,
                        r.summary,
                        r.state.as_str(),
                        r.requested_by,
                        r.created_at.0
                    ],
                )
                .map_err(db)?;
            Ok(inserted > 0)
        })
        .await
    }

    async fn open_update_run(&self) -> AppResult<Option<UpdateRunRecord>> {
        self.call(|c| {
            c.query_row(
                &format!("SELECT {RUN_COLS} FROM update_runs WHERE finished_at IS NULL"),
                [],
                run_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn last_update_run(&self) -> AppResult<Option<UpdateRunRecord>> {
        self.call(|c| {
            c.query_row(
                &format!(
                    "SELECT {RUN_COLS} FROM update_runs ORDER BY created_at DESC, rowid DESC LIMIT 1"
                ),
                [],
                run_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn move_update_run(
        &self,
        id: &str,
        from: RunState,
        to: RunState,
        outcome: Option<RunState>,
        error: Option<&str>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, error) = (id.to_owned(), error.map(str::to_owned));
        self.call(move |c| {
            let finished = to.is_terminal().then_some(at.0);
            let changed = c
                .execute(
                    "UPDATE update_runs SET state = ?3, outcome = COALESCE(?4, outcome), \
                     error = COALESCE(?5, error), state_since = ?6, finished_at = ?7 \
                     WHERE id = ?1 AND state = ?2 AND finished_at IS NULL",
                    params![
                        id,
                        from.as_str(),
                        to.as_str(),
                        outcome.map(RunState::as_str),
                        error,
                        at.0,
                        finished
                    ],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn jev_key(&self) -> AppResult<Option<StoredJevKey>> {
        self.call(|c| {
            let row = c
                .query_row(
                    "SELECT jev_key, jev_key_set_at, jev_key_set_by FROM instance WHERE singleton = 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, Option<Vec<u8>>>(0)?,
                            r.get::<_, Option<i64>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(db)?;
            Ok(row.and_then(|(key, at, by)| {
                key.map(|bytes| StoredJevKey {
                    sealed: Sealed(bytes),
                    set_at: Timestamp(at.unwrap_or(0)),
                    set_by: by,
                })
            }))
        })
        .await
    }

    async fn set_jev_key(&self, key: Option<(&Sealed, &str)>, at: Timestamp) -> AppResult<()> {
        let key = key.map(|(sealed, by)| (sealed.0.clone(), by.to_owned()));
        self.call(move |c| {
            match key {
                Some((bytes, by)) => c.execute(
                    "UPDATE instance SET jev_key = ?1, jev_key_set_at = ?2, jev_key_set_by = ?3 \
                     WHERE singleton = 1",
                    params![bytes, at.0, by],
                ),
                None => c.execute(
                    "UPDATE instance SET jev_key = NULL, jev_key_set_at = NULL, \
                     jev_key_set_by = NULL WHERE singleton = 1",
                    [],
                ),
            }
            .map_err(db)?;
            Ok(())
        })
        .await
    }
}
