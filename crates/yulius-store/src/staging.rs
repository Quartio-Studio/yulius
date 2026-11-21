//! Merge-first staging requests (SPEC-575 MFS-005, MFS-008, migration 0078).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::ports::store_ext::{
    BootstrapUpgrade, StagingChange, StagingRequestRecord, StagingStore,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::staging::StagingState;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLUMNS: &str = "id, sandbox_id, environment, task_id, revision, state, ci_run_id, \
                       cd_run_id, attempts, note, requested_by, told, created_at, updated_at";

fn request_row(row: &Row<'_>) -> rusqlite::Result<StagingRequestRecord> {
    let sandbox: String = row.get(1)?;
    let task: Option<String> = row.get(3)?;
    let state: String = row.get(5)?;
    Ok(StagingRequestRecord {
        id: row.get(0)?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        environment: row.get(2)?,
        task_id: task
            .map(|id| TaskId::parse(&id).map_err(|_| bad_column("task id", &id)))
            .transpose()?,
        revision: row.get(4)?,
        state: StagingState::parse(&state).ok_or_else(|| bad_column("staging state", &state))?,
        ci_run_id: row.get(6)?,
        cd_run_id: row.get(7)?,
        attempts: row.get(8)?,
        note: row.get(9)?,
        requested_by: row.get(10)?,
        told: row.get(11)?,
        created_at: Timestamp(row.get(12)?),
        updated_at: Timestamp(row.get(13)?),
    })
}

/// Whether `err` is a unique or check rule refusing the write.
fn refused(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(code, _)
        if code.code == rusqlite::ErrorCode::ConstraintViolation)
}

#[async_trait]
impl StagingStore for SqliteStore {
    async fn insert_staging_request(&self, r: &StagingRequestRecord) -> AppResult<bool> {
        let r = r.clone();
        self.call(move |c| {
            let inserted = c.execute(
                &format!(
                    "INSERT INTO staging_requests ({COLUMNS}) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
                ),
                params![
                    r.id,
                    r.sandbox_id.as_str(),
                    r.environment,
                    r.task_id.as_ref().map(TaskId::as_str),
                    r.revision,
                    r.state.as_str(),
                    r.ci_run_id,
                    r.cd_run_id,
                    r.attempts,
                    r.note,
                    r.requested_by,
                    r.told,
                    r.created_at.0,
                    r.updated_at.0,
                ],
            );
            match inserted {
                Ok(_) => Ok(true),
                Err(err) if refused(&err) => Ok(false),
                Err(err) => Err(db(err)),
            }
        })
        .await
    }

    async fn staging_request(&self, id: &str) -> AppResult<Option<StagingRequestRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLUMNS} FROM staging_requests WHERE id = ?1"),
                [id],
                request_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn staging_requests_of(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        task: Option<&TaskId>,
        revision: &str,
    ) -> AppResult<Vec<StagingRequestRecord>> {
        let (sandbox, environment) = (sandbox.as_str().to_owned(), environment.to_owned());
        let (task, revision) = (task.map(|t| t.as_str().to_owned()), revision.to_owned());
        self.call(move |c| {
            let mut statement = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM staging_requests \
                     WHERE sandbox_id = ?1 AND environment = ?2 \
                       AND (task_id = ?3 OR (?3 IS NULL AND task_id IS NULL AND revision = ?4)) \
                     ORDER BY created_at, rowid"
                ))
                .map_err(db)?;
            let rows = statement
                .query_map(params![sandbox, environment, task, revision], request_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn pending_staging_requests(
        &self,
        sandbox: &SandboxId,
        environment: &str,
    ) -> AppResult<Vec<StagingRequestRecord>> {
        let (sandbox, environment) = (sandbox.as_str().to_owned(), environment.to_owned());
        self.call(move |c| {
            let mut statement = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM staging_requests \
                     WHERE sandbox_id = ?1 AND environment = ?2 \
                       AND state IN ('testing', 'ready', 'deploying') \
                     ORDER BY created_at, rowid"
                ))
                .map_err(db)?;
            let rows = statement
                .query_map(params![sandbox, environment], request_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn staging_queues(&self, failed_since: Timestamp) -> AppResult<Vec<(SandboxId, String)>> {
        self.call(move |c| {
            let mut statement = c
                .prepare(
                    "SELECT DISTINCT sandbox_id, environment FROM staging_requests \
                     WHERE state IN ('testing', 'ready', 'deploying') \
                        OR (state = 'deploy_failed' AND updated_at >= ?1) ORDER BY sandbox_id",
                )
                .map_err(db)?;
            let rows = statement
                .query_map([failed_since.0], |row| {
                    let sandbox: String = row.get(0)?;
                    Ok((
                        SandboxId::parse(&sandbox)
                            .map_err(|_| bad_column("sandbox id", &sandbox))?,
                        row.get::<_, String>(1)?,
                    ))
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn staging_verified(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        revision: &str,
    ) -> AppResult<bool> {
        let (sandbox, env, sha) = (
            sandbox.to_string(),
            environment.to_owned(),
            revision.to_owned(),
        );
        self.call(move |c| c.query_row(
            "SELECT EXISTS(SELECT 1 FROM staging_requests WHERE sandbox_id = ?1 AND environment = ?2 AND revision = ?3 AND state = 'verified')",
            params![sandbox, env, sha], |row| row.get(0)).map_err(db)).await
    }

    async fn recent_staging_requests(
        &self,
        sandbox: &SandboxId,
        environment: &str,
        limit: u32,
    ) -> AppResult<Vec<StagingRequestRecord>> {
        let (sandbox, environment) = (sandbox.as_str().to_owned(), environment.to_owned());
        self.call(move |c| {
            let mut statement = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM staging_requests \
                     WHERE sandbox_id = ?1 AND environment = ?2 \
                     ORDER BY created_at DESC, rowid DESC LIMIT ?3"
                ))
                .map_err(db)?;
            let rows = statement
                .query_map(params![sandbox, environment, limit], request_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn move_staging_request(
        &self,
        id: &str,
        from: StagingState,
        to: StagingState,
        change: &StagingChange,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, change) = (id.to_owned(), change.clone());
        self.call(move |c| {
            let moved = c.execute(
                "UPDATE staging_requests SET state = ?3, \
                     ci_run_id = COALESCE(?4, ci_run_id), \
                     cd_run_id = CASE WHEN ?9 THEN NULL ELSE COALESCE(?5, cd_run_id) END, \
                     attempts = COALESCE(?6, attempts), note = COALESCE(?7, note), \
                     told = 0, updated_at = ?8 \
                 WHERE id = ?1 AND state = ?2",
                params![
                    id,
                    from.as_str(),
                    to.as_str(),
                    change.ci_run_id,
                    change.cd_run_id,
                    change.attempts,
                    change.note,
                    at.0,
                    change.clear_cd_run,
                ],
            );
            match moved {
                Ok(rows) => Ok(rows == 1),
                Err(err) if refused(&err) => Ok(false),
                Err(err) => Err(db(err)),
            }
        })
        .await
    }

    async fn untold_staging_requests(&self) -> AppResult<Vec<StagingRequestRecord>> {
        self.call(|c| {
            let mut statement = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM staging_requests \
                     WHERE told = 0 AND task_id IS NOT NULL \
                       AND state IN ('verified', 'included', 'gate_failed', 'deploy_failed', \
                           'superseded') \
                     ORDER BY updated_at, rowid"
                ))
                .map_err(db)?;
            let rows = statement.query_map([], request_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn task_landings(&self, task: &TaskId, branch: &str) -> AppResult<Vec<String>> {
        let (task, branch) = (task.as_str().to_owned(), branch.to_owned());
        self.call(move |c| {
            let mut statement = c
                .prepare(
                    "SELECT merged_sha FROM task_merges WHERE task_id = ?1 AND branch = ?2 \
                     ORDER BY rowid",
                )
                .map_err(db)?;
            let rows = statement
                .query_map(params![task, branch], |row| row.get::<_, String>(0))
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn claim_bootstrap_upgrade(
        &self,
        sandbox: &SandboxId,
        revision: u32,
        at: Timestamp,
    ) -> AppResult<bool> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let rows = c
                .execute(
                    "INSERT INTO bootstrap_upgrades (sandbox_id, revision, state, task_id, updated_at) \
                     VALUES (?1, ?2, 'claimed', NULL, ?3) \
                     ON CONFLICT(sandbox_id) DO UPDATE SET revision = excluded.revision, \
                         state = 'claimed', task_id = NULL, updated_at = excluded.updated_at \
                     WHERE bootstrap_upgrades.revision < excluded.revision",
                    params![sandbox, revision, at.0],
                )
                .map_err(db)?;
            Ok(rows == 1)
        })
        .await
    }

    async fn lay_bootstrap_revision(
        &self,
        sandbox: &SandboxId,
        revision: u32,
        at: Timestamp,
    ) -> AppResult<()> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            c.execute(
                "INSERT INTO bootstrap_upgrades (sandbox_id, revision, state, task_id, updated_at) \
                 VALUES (?1, ?2, 'laid', NULL, ?3) \
                 ON CONFLICT(sandbox_id) DO UPDATE SET revision = excluded.revision, \
                     state = 'laid', task_id = NULL, updated_at = excluded.updated_at \
                 WHERE bootstrap_upgrades.revision < excluded.revision",
                params![sandbox, revision, at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn link_bootstrap_task(
        &self,
        sandbox: &SandboxId,
        revision: u32,
        task: &TaskId,
    ) -> AppResult<bool> {
        let (sandbox, task) = (sandbox.as_str().to_owned(), task.as_str().to_owned());
        self.call(move |c| {
            let rows = c
                .execute(
                    "UPDATE bootstrap_upgrades SET state = 'linked', task_id = ?3 \
                     WHERE sandbox_id = ?1 AND revision = ?2 AND state = 'claimed'",
                    params![sandbox, revision, task],
                )
                .map_err(db)?;
            Ok(rows == 1)
        })
        .await
    }

    async fn bootstrap_upgrade(&self, sandbox: &SandboxId) -> AppResult<Option<BootstrapUpgrade>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT revision, state, task_id, updated_at FROM bootstrap_upgrades \
                 WHERE sandbox_id = ?1",
                [sandbox],
                |row| {
                    let task: Option<String> = row.get(2)?;
                    Ok(BootstrapUpgrade {
                        revision: row.get(0)?,
                        state: row.get(1)?,
                        task_id: task
                            .map(|id| TaskId::parse(&id).map_err(|_| bad_column("task id", &id)))
                            .transpose()?,
                        claimed_at: Timestamp(row.get(3)?),
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn mark_staging_told(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let rows = c
                .execute(
                    "UPDATE staging_requests SET told = 1 WHERE id = ?1 AND told = 0",
                    [id],
                )
                .map_err(db)?;
            Ok(rows == 1)
        })
        .await
    }
}
