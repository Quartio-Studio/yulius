//! CD (deployment) runs (SPEC-134, table `cd_runs`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::ci::cd::{CdRunEnd, CdRunRecord};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::CdStore;
use yulius_domain::cd::CdEnv;
use yulius_domain::ci::RunState;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str = "id, sandbox_id, environment, branch, sha, state, reason, requested_by, \
    task_id, deploy, verify, note, exit_code, started_at, ended_at";

fn run_from_row(row: &Row<'_>, with_output: bool) -> rusqlite::Result<CdRunRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let environment: String = row.get("environment")?;
    let state: String = row.get("state")?;
    let task: Option<String> = row.get("task_id")?;
    Ok(CdRunRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        environment: CdEnv::parse(&environment)
            .ok_or_else(|| bad_column("CD environment", &environment))?,
        branch: row.get("branch")?,
        sha: row.get("sha")?,
        state: RunState::parse(&state).ok_or_else(|| bad_column("CD state", &state))?,
        reason: row.get("reason")?,
        requested_by: row.get("requested_by")?,
        task_id: task
            .map(|id| TaskId::parse(&id).map_err(|_| bad_column("task id", &id)))
            .transpose()?,
        deploy: row.get("deploy")?,
        verify: row.get("verify")?,
        note: row.get("note")?,
        output: if with_output {
            row.get("output")?
        } else {
            String::new()
        },
        exit_code: row.get("exit_code")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
    })
}

fn select(
    c: &rusqlite::Connection,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
) -> AppResult<Vec<CdRunRecord>> {
    let mut stmt = c
        .prepare(&format!("SELECT {COLS} FROM cd_runs WHERE {filter}"))
        .map_err(db)?;
    let rows = stmt
        .query_map(args, |row| run_from_row(row, false))
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    Ok(rows)
}

#[async_trait]
impl CdStore for SqliteStore {
    async fn insert_cd_run(&self, run: &CdRunRecord) -> AppResult<()> {
        let run = run.clone();
        self.call(move |c| {
            let inserted = c.execute(
                &format!(
                    "INSERT INTO cd_runs ({COLS}, output) VALUES \
                     (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
                ),
                params![
                    run.id,
                    run.sandbox_id.as_str(),
                    run.environment.as_str(),
                    run.branch,
                    run.sha,
                    run.state.as_str(),
                    run.reason,
                    run.requested_by,
                    run.task_id.as_ref().map(TaskId::as_str),
                    run.deploy,
                    run.verify,
                    run.note,
                    run.exit_code,
                    run.started_at.0,
                    run.ended_at.map(|t| t.0),
                    run.output,
                ],
            );
            match inserted {
                Ok(_) => Ok(()),
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Err(AppError::Conflict(
                        "another run of this environment is running".into(),
                    ))
                }
                Err(err) => Err(db(err)),
            }
        })
        .await
    }

    async fn cd_run(&self, id: &str, with_output: bool) -> AppResult<Option<CdRunRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS}, output FROM cd_runs WHERE id = ?1"),
                [id],
                |row| run_from_row(row, with_output),
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn cd_runs(&self, sandbox: &SandboxId, limit: u32) -> AppResult<Vec<CdRunRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            select(
                c,
                "sandbox_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT ?2",
                &[&sandbox, &i64::from(limit)],
            )
        })
        .await
    }

    async fn last_passed_cd_run(
        &self,
        sandbox: &SandboxId,
        environment: CdEnv,
    ) -> AppResult<Option<CdRunRecord>> {
        let (sandbox, environment) = (sandbox.as_str().to_owned(), environment.as_str());
        self.call(move |c| {
            Ok(select(
                c,
                "sandbox_id = ?1 AND environment = ?2 AND state = 'passed' \
                 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                &[&sandbox, &environment],
            )?
            .into_iter()
            .next())
        })
        .await
    }

    async fn latest_cd_run_of_task(&self, task: &TaskId) -> AppResult<Option<CdRunRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            Ok(select(
                c,
                "task_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                &[&task],
            )?
            .into_iter()
            .next())
        })
        .await
    }

    async fn running_cd_runs(&self) -> AppResult<Vec<CdRunRecord>> {
        self.call(move |c| select(c, "state = 'running' ORDER BY started_at, rowid", &[]))
            .await
    }

    async fn set_cd_run_output(&self, id: &str, output: &str) -> AppResult<()> {
        let (id, output) = (id.to_owned(), output.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE cd_runs SET output = ?2 WHERE id = ?1 AND state = 'running'",
                params![id, output],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn mark_cd_run_reported(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE cd_runs SET reported = 1 \
                     WHERE id = ?1 AND reported = 0 AND state <> 'running'",
                    [id],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn finish_cd_run(&self, id: &str, end: &CdRunEnd) -> AppResult<bool> {
        let (id, end) = (id.to_owned(), end.clone());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE cd_runs SET state = ?2, reason = ?3, exit_code = ?4, output = ?5, \
                     ended_at = ?6 WHERE id = ?1 AND state = 'running'",
                    params![
                        id,
                        end.state.as_str(),
                        end.reason,
                        end.exit_code,
                        end.output,
                        end.ended_at.0
                    ],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn prune_cd_runs(&self, keep: u32, before: Timestamp) -> AppResult<u64> {
        self.call(move |c| {
            let changed = c
                .execute(
                    // Each environment's newest passed run is what is deployed: it stays.
                    "DELETE FROM cd_runs WHERE state <> 'running' AND (\
                       started_at < ?1 OR id IN (\
                         SELECT id FROM (\
                           SELECT id, ROW_NUMBER() OVER (\
                             PARTITION BY sandbox_id, environment \
                             ORDER BY started_at DESC, rowid DESC) AS n \
                           FROM cd_runs) WHERE n > ?2)) \
                     AND id NOT IN (\
                       SELECT id FROM (\
                         SELECT id, ROW_NUMBER() OVER (\
                           PARTITION BY sandbox_id, environment \
                           ORDER BY started_at DESC, rowid DESC) AS v \
                         FROM cd_runs WHERE state = 'passed') WHERE v = 1)",
                    params![before.0, i64::from(keep)],
                )
                .map_err(db)?;
            Ok(u64::try_from(changed).unwrap_or(0))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn run(id: &str, env: CdEnv, at: i64) -> Result<CdRunRecord, Box<dyn std::error::Error>> {
        Ok(CdRunRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: env,
            branch: "dev".into(),
            sha: "a".repeat(40),
            state: RunState::Running,
            reason: None,
            requested_by: "task:7".into(),
            task_id: Some(TaskId::parse("t1")?),
            deploy: Some("scripts/cd/deploy.sh dev".into()),
            verify: None,
            note: None,
            output: String::new(),
            exit_code: None,
            started_at: Timestamp(at),
            ended_at: None,
        })
    }

    fn end(state: RunState, at: i64) -> CdRunEnd {
        CdRunEnd {
            state,
            reason: None,
            exit_code: Some(0),
            output: "[yulius] done\n".into(),
            ended_at: Timestamp(at),
        }
    }

    /// SPEC-134 CDR-004: one running run per environment; it ends once.
    #[tokio::test]
    async fn one_running_run_per_environment_ends_once() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store.insert_cd_run(&run("d1", CdEnv::Dev, 1)?).await?;
        assert!(matches!(
            store.insert_cd_run(&run("d2", CdEnv::Dev, 2)?).await,
            Err(AppError::Conflict(_))
        ));
        store.insert_cd_run(&run("p1", CdEnv::Prod, 3)?).await?;
        assert_eq!(store.running_cd_runs().await?.len(), 2);
        store.set_cd_run_output("d1", "partial").await?;
        assert_eq!(
            store.cd_run("d1", true).await?.ok_or("d1")?.output,
            "partial"
        );
        assert_eq!(store.cd_run("d1", false).await?.ok_or("d1")?.output, "");
        assert!(store.finish_cd_run("d1", &end(RunState::Passed, 5)).await?);
        assert!(!store.finish_cd_run("d1", &end(RunState::Failed, 6)).await?);
        let done = store.cd_run("d1", true).await?.ok_or("d1")?;
        assert_eq!(
            (done.state, done.ended_at),
            (RunState::Passed, Some(Timestamp(5)))
        );
        assert_eq!(done.task_id.as_ref().map(TaskId::as_str), Some("t1"));
        // The environment is free again.
        store.insert_cd_run(&run("d2", CdEnv::Dev, 7)?).await?;
        let listed = store.cd_runs(&SandboxId::parse("s1")?, 10).await?;
        let ids: Vec<&str> = listed.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["d2", "p1", "d1"], "newest first");
        Ok(())
    }

    /// Retention keeps what is deployed: each environment's newest passed run.
    #[tokio::test]
    async fn retention_keeps_each_environments_deployed_run() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        for (id, at, state) in [
            ("d1", 1, RunState::Passed),
            ("d2", 2, RunState::Failed),
            ("d3", 3, RunState::Failed),
        ] {
            store.insert_cd_run(&run(id, CdEnv::Dev, at)?).await?;
            store.finish_cd_run(id, &end(state, at)).await?;
        }
        store.insert_cd_run(&run("d4", CdEnv::Dev, 4)?).await?;
        // Keep one, delete everything started before 100: d1 (deployed) and d4 (running) stay.
        assert_eq!(store.prune_cd_runs(1, Timestamp(100)).await?, 2);
        assert!(store.cd_run("d1", false).await?.is_some());
        assert!(store.cd_run("d4", false).await?.is_some());
        assert!(store.cd_run("d2", false).await?.is_none());
        Ok(())
    }
}
