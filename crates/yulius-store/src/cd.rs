//! CD (deployment) runs (SPEC-134, SPEC-257, table `cd_runs`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::ci::cd::{CdRunEnd, CdRunRecord};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::CdStore;
use yulius_domain::cd::CdStage;
use yulius_domain::ci::RunState;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str = "id, sandbox_id, environment, stage, branch, sha, state, reason, requested_by, \
    task_id, deploy, verify, note, exit_code, started_at, ended_at";

fn run_from_row(row: &Row<'_>, with_output: bool) -> rusqlite::Result<CdRunRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let stage: String = row.get("stage")?;
    let state: String = row.get("state")?;
    let task: Option<String> = row.get("task_id")?;
    Ok(CdRunRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        environment: row.get("environment")?,
        stage: CdStage::parse(&stage).ok_or_else(|| bad_column("CD stage", &stage))?,
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
                     (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
                ),
                params![
                    run.id,
                    run.sandbox_id.as_str(),
                    run.environment,
                    run.stage.as_str(),
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
        environment: &str,
        stage: CdStage,
    ) -> AppResult<Option<CdRunRecord>> {
        let (sandbox, environment) = (sandbox.as_str().to_owned(), environment.to_owned());
        let stage = stage.as_str();
        self.call(move |c| {
            Ok(select(
                c,
                "sandbox_id = ?1 AND environment = ?2 AND stage = ?3 AND state = 'passed' \
                 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                &[&sandbox, &environment, &stage],
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
                     ended_at = ?6, cpu_ms = ?7, memory_peak_mb = ?8, disk_written_mb = ?9 \
                     WHERE id = ?1 AND state = 'running'",
                    params![
                        id,
                        end.state.as_str(),
                        end.reason,
                        end.exit_code,
                        end.output,
                        end.ended_at.0,
                        end.usage.map(|u| u.cpu_ms),
                        end.usage.map(|u| u.memory_peak_mb),
                        end.usage.and_then(|u| u.disk_written_mb)
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

    fn run(id: &str, env: &str, at: i64) -> Result<CdRunRecord, Box<dyn std::error::Error>> {
        Ok(CdRunRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: env.into(),
            stage: yulius_domain::cd::fixed_stage(env).unwrap_or(CdStage::Prod),
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
            usage: None,
        }
    }

    /// SPEC-134 CDR-004: one running run per environment; it ends once.
    #[tokio::test]
    async fn one_running_run_per_environment_ends_once() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store.insert_cd_run(&run("d1", "dev", 1)?).await?;
        assert!(matches!(
            store.insert_cd_run(&run("d2", "dev", 2)?).await,
            Err(AppError::Conflict(_))
        ));
        store.insert_cd_run(&run("p1", "prod", 3)?).await?;
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
        store.insert_cd_run(&run("d2", "dev", 7)?).await?;
        let listed = store.cd_runs(&SandboxId::parse("s1")?, 10).await?;
        let ids: Vec<&str> = listed.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["d2", "p1", "d1"], "newest first");
        Ok(())
    }

    /// SPEC-257 CDN-006: a named environment keeps its name and stage, and runs one at a time
    /// on its own, beside the default of its stage.
    #[tokio::test]
    async fn named_environments_keep_their_name_and_stage() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store.insert_cd_run(&run("x1", "platform", 1)?).await?;
        assert!(matches!(
            store.insert_cd_run(&run("x2", "platform", 2)?).await,
            Err(AppError::Conflict(_))
        ));
        store.insert_cd_run(&run("p1", "prod", 3)?).await?;
        let platform = store.cd_run("x1", false).await?.ok_or("x1")?;
        assert_eq!(
            (platform.environment.as_str(), platform.stage),
            ("platform", CdStage::Prod)
        );
        store.finish_cd_run("x1", &end(RunState::Passed, 4)).await?;
        let sandbox = SandboxId::parse("s1")?;
        let deployed = store
            .last_passed_cd_run(&sandbox, "platform", CdStage::Prod)
            .await?;
        assert_eq!(deployed.map(|r| r.id), Some("x1".to_owned()));
        assert!(
            store
                .last_passed_cd_run(&sandbox, "platform", CdStage::Dev)
                .await?
                .is_none(),
            "a run of the other stage is not what is deployed there"
        );
        assert!(
            store
                .last_passed_cd_run(&sandbox, "prod", CdStage::Prod)
                .await?
                .is_none()
        );
        Ok(())
    }

    /// SPEC-257 CDN-006: runs written before stages existed read back with their name's stage.
    #[tokio::test]
    async fn runs_from_before_stages_take_their_names_stage() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let migration = include_str!("migrations/0041_cd_run_stage.sql");
        store
            .call(move |c| {
                c.execute_batch(
                    "DROP TRIGGER spans_cd_run_insert; ALTER TABLE cd_runs DROP COLUMN stage;\
                     INSERT INTO cd_runs (id, sandbox_id, environment, branch, sha, state, requested_by, started_at) \
                     VALUES ('d', 's1', 'dev', 'dev', 'a', 'passed', 'x', 1), \
                            ('p', 's1', 'prod', 'main', 'b', 'passed', 'x', 2); \
                     CREATE TRIGGER spans_cd_run_insert AFTER INSERT ON cd_runs BEGIN SELECT 1; END;",
                )
                .map_err(db)?;
                c.execute_batch(migration).map_err(db)
            })
            .await?;
        let dev = store.cd_run("d", false).await?.ok_or("d")?;
        let prod = store.cd_run("p", false).await?.ok_or("p")?;
        assert_eq!((dev.stage, prod.stage), (CdStage::Dev, CdStage::Prod));
        // A run's span carries its stage (review L1: delivery figures per stage).
        store.insert_cd_run(&run("x1", "platform", 3)?).await?;
        let stage: String = store
            .call(|c| {
                c.query_row(
                    "SELECT json_extract(detail, '$.stage') FROM spans \
                     WHERE subject_kind = 'cd_run' AND subject_id = 'x1' AND key = 'run'",
                    [],
                    |row| row.get(0),
                )
                .map_err(db)
            })
            .await?;
        assert_eq!(stage, "prod");
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
            store.insert_cd_run(&run(id, "dev", at)?).await?;
            store.finish_cd_run(id, &end(state, at)).await?;
        }
        store.insert_cd_run(&run("d4", "dev", 4)?).await?;
        // Keep one, delete everything started before 100: d1 (deployed) and d4 (running) stay.
        assert_eq!(store.prune_cd_runs(1, Timestamp(100)).await?, 2);
        assert!(store.cd_run("d1", false).await?.is_some());
        assert!(store.cd_run("d4", false).await?.is_some());
        assert!(store.cd_run("d2", false).await?.is_none());
        Ok(())
    }
}
