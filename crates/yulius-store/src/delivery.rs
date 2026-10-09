//! Per-task delivery stages and environment runs (SPEC-002 §6.7).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::delivery::{
    DeliveryRecord, EnvironmentRunKind, EnvironmentRunRecord, EnvironmentRunState,
};
use yulius_app::error::AppResult;
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u64};

const RUN_COLS: &str = "id, sandbox_id, environment, kind, requested_by, command, commit_sha, \
    state, output, exit_code, started_at, ended_at";

fn delivery_from_row(row: &Row<'_>) -> rusqlite::Result<DeliveryRecord> {
    let task: String = row.get("task_id")?;
    let stage: String = row.get("stage")?;
    Ok(DeliveryRecord {
        task_id: TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?,
        stage: DeliveryStage::decode(&stage).ok_or_else(|| bad_column("delivery stage", &stage))?,
        pr_number: row.get::<_, Option<i64>>("pr_number")?.map(to_u64),
        updated_at: Timestamp(row.get("updated_at")?),
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<EnvironmentRunRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let kind: String = row.get("kind")?;
    let user: String = row.get("requested_by")?;
    let state: String = row.get("state")?;
    Ok(EnvironmentRunRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        environment: row.get("environment")?,
        kind: EnvironmentRunKind::parse(&kind).ok_or_else(|| bad_column("run kind", &kind))?,
        requested_by: UserId::parse(&user).map_err(|_| bad_column("user id", &user))?,
        command: row.get("command")?,
        commit: row.get("commit_sha")?,
        state: EnvironmentRunState::parse(&state).ok_or_else(|| bad_column("run state", &state))?,
        output: row.get("output")?,
        exit_code: row.get("exit_code")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
    })
}

#[async_trait]
impl DeliveryStore for SqliteStore {
    async fn delivery(&self, task: &TaskId) -> AppResult<Option<DeliveryRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT task_id, stage, pr_number, updated_at FROM delivery WHERE task_id = ?1",
                [task],
                delivery_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn deliveries(&self, sandbox: &SandboxId) -> AppResult<Vec<DeliveryRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT d.task_id, d.stage, d.pr_number, d.updated_at FROM delivery d \
                     JOIN tasks t ON t.id = d.task_id WHERE t.sandbox_id = ?1 ORDER BY t.number",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([sandbox], delivery_from_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn transition_delivery(
        &self,
        expected: &DeliveryStage,
        next: &DeliveryRecord,
    ) -> AppResult<bool> {
        let expected = expected.encode();
        let next = next.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let current: Option<String> = tx
                .query_row(
                    "SELECT stage FROM delivery WHERE task_id = ?1",
                    [next.task_id.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            if current.as_deref().unwrap_or("none") != expected {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO delivery (task_id, stage, pr_number, updated_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(task_id) DO UPDATE SET stage = excluded.stage, \
                 pr_number = excluded.pr_number, updated_at = excluded.updated_at",
                params![
                    next.task_id.as_str(),
                    next.stage.encode(),
                    next.pr_number.map(to_i64),
                    next.updated_at.0
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn insert_environment_run(&self, run: &EnvironmentRunRecord) -> AppResult<()> {
        let run = run.clone();
        self.call(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO environment_runs ({RUN_COLS}) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    run.id,
                    run.sandbox_id.as_str(),
                    run.environment,
                    run.kind.as_str(),
                    run.requested_by.as_str(),
                    run.command,
                    run.commit,
                    run.state.as_str(),
                    run.output,
                    run.exit_code,
                    run.started_at.0,
                    run.ended_at.map(|t| t.0)
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn environment_run(&self, id: &str) -> AppResult<Option<EnvironmentRunRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {RUN_COLS} FROM environment_runs WHERE id = ?1"),
                [id],
                run_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn environment_runs(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<EnvironmentRunRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {RUN_COLS} FROM environment_runs WHERE sandbox_id = ?1 \
                     ORDER BY started_at DESC, rowid DESC LIMIT ?2"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map(params![sandbox, i64::from(limit)], run_from_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn running_environment_runs(&self) -> AppResult<Vec<EnvironmentRunRecord>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {RUN_COLS} FROM environment_runs WHERE state = 'running'"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map([], run_from_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn update_environment_run(
        &self,
        id: &str,
        output: &str,
        commit: Option<&str>,
    ) -> AppResult<()> {
        let (id, output, commit) = (id.to_owned(), output.to_owned(), commit.map(str::to_owned));
        self.call(move |c| {
            c.execute(
                "UPDATE environment_runs SET output = ?2, commit_sha = COALESCE(?3, commit_sha) \
                 WHERE id = ?1 AND state = 'running'",
                params![id, output, commit],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn finish_environment_run(
        &self,
        id: &str,
        state: EnvironmentRunState,
        output: &str,
        exit_code: Option<i32>,
        ended_at: Timestamp,
    ) -> AppResult<bool> {
        let (id, output) = (id.to_owned(), output.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE environment_runs SET state = ?2, output = ?3, exit_code = ?4, ended_at = ?5 \
                     WHERE id = ?1 AND state = 'running'",
                    params![id, state.as_str(), output, exit_code, ended_at.0],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_app::model::{SandboxRecord, TaskOrigin, TaskRecord};
    use yulius_app::ports::store::{SandboxStore, TaskStore};
    use yulius_domain::harness::Harness;
    use yulius_domain::task::TaskStatus;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn seed(store: &SqliteStore) -> Result<TaskId, Box<dyn std::error::Error>> {
        store
            .insert_sandbox(&SandboxRecord {
                id: SandboxId::parse("s1")?,
                slug: "s".into(),
                repo_owner: "o".into(),
                repo_name: "r".into(),
                github_repo_id: None,
                default_branch: "main".into(),
                profile_json: "{}".into(),
                port_base: 20000,
                port_count: 20,
                onboarding_state: "not_needed".into(),
                onboarding_task_id: None,
                created_by: UserId::parse("u1")?,
                created_at: Timestamp(1),
                removed_at: None,
            })
            .await?;
        let task = store
            .insert_task(&TaskRecord {
                id: TaskId::parse("t1")?,
                sandbox_id: SandboxId::parse("s1")?,
                number: 0,
                title: "t".into(),
                prompt: "p".into(),
                harness: Harness::ClaudeCode,
                model: None,
                effort: None,
                workflow_id: None,
                status: TaskStatus::Closed,
                activity: None,
                branch: None,
                worktree_path: None,
                spec_path: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                created_by: UserId::parse("u1")?,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                closed_at: None,
                archived_at: None,
                queued_at: None,
                gate_blocks: 0,
                wake_enabled: true,
                mode: yulius_app::model::TaskMode::Prompt,
            })
            .await?;
        Ok(task.id)
    }

    #[tokio::test]
    async fn delivery_stage_is_compare_and_set() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        let task = seed(&store).await?;
        assert_eq!(store.delivery(&task).await?, None);
        let open = DeliveryRecord {
            task_id: task.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(7),
            updated_at: Timestamp(5),
        };
        assert!(
            store
                .transition_delivery(&DeliveryStage::None, &open)
                .await?
        );
        assert!(
            !store
                .transition_delivery(&DeliveryStage::None, &open)
                .await?,
            "a stale expectation loses"
        );
        let done = DeliveryRecord {
            stage: DeliveryStage::EnvDone("dev-vm".into()),
            ..open.clone()
        };
        assert!(
            store
                .transition_delivery(&DeliveryStage::PrOpen, &done)
                .await?
        );
        assert_eq!(store.delivery(&task).await?, Some(done.clone()));
        assert_eq!(
            store.deliveries(&SandboxId::parse("s1")?).await?,
            vec![done]
        );
        Ok(())
    }

    fn run(
        id: &str,
        state: EnvironmentRunState,
        at: i64,
    ) -> Result<EnvironmentRunRecord, Box<dyn std::error::Error>> {
        Ok(EnvironmentRunRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: "dev-vm".into(),
            kind: EnvironmentRunKind::Command,
            requested_by: UserId::parse("u1")?,
            command: Some("make deploy".into()),
            commit: None,
            state,
            output: String::new(),
            exit_code: None,
            started_at: Timestamp(at),
            ended_at: None,
        })
    }

    #[tokio::test]
    async fn one_running_environment_run_per_sandbox() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        seed(&store).await?;
        store
            .insert_environment_run(&run("r1", EnvironmentRunState::Running, 1)?)
            .await?;
        let second = store
            .insert_environment_run(&run("r2", EnvironmentRunState::Running, 2)?)
            .await;
        assert!(matches!(
            second,
            Err(yulius_app::error::AppError::Conflict(_))
        ));
        store
            .insert_environment_run(&run("m1", EnvironmentRunState::Succeeded, 3)?)
            .await?;
        store
            .update_environment_run("r1", "partial", Some("abc"))
            .await?;
        assert_eq!(store.running_environment_runs().await?.len(), 1);
        assert!(
            store
                .finish_environment_run(
                    "r1",
                    EnvironmentRunState::Failed,
                    "all",
                    Some(2),
                    Timestamp(9)
                )
                .await?
        );
        assert!(
            !store
                .finish_environment_run(
                    "r1",
                    EnvironmentRunState::Succeeded,
                    "x",
                    Some(0),
                    Timestamp(10)
                )
                .await?,
            "a finished run keeps its outcome"
        );
        let stored = store.environment_run("r1").await?.ok_or("run")?;
        assert_eq!(stored.state, EnvironmentRunState::Failed);
        assert_eq!(stored.output, "all");
        assert_eq!(stored.commit.as_deref(), Some("abc"));
        assert_eq!(stored.exit_code, Some(2));
        store.update_environment_run("r1", "late", None).await?;
        assert_eq!(
            store.environment_run("r1").await?.ok_or("run")?.output,
            "all"
        );
        let listed = store.environment_runs(&SandboxId::parse("s1")?, 10).await?;
        assert_eq!(
            listed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["m1", "r1"]
        );
        store
            .insert_environment_run(&run("r3", EnvironmentRunState::Running, 4)?)
            .await?;
        Ok(())
    }
}
