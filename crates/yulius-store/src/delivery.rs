//! Per-task delivery stages and environment runs (SPEC-002 §6.7).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::delivery::evidence::{EvidenceRun, Landing, LandingSource, MergeEvidence};
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

/// The current delivery binds the task and PR even during legacy recovery. An insert from a
/// stale read (another PR opened meanwhile) matches nothing.
fn record_landing(
    c: &rusqlite::Connection,
    task: &str,
    pr: Option<i64>,
    landed: &Landing,
) -> AppResult<()> {
    c.execute(
        "INSERT INTO task_merges(task_id, sandbox_id, pr_number, branch, head_sha, merged_sha)
         SELECT t.id, t.sandbox_id, d.pr_number, ?3, ?4, ?5 FROM tasks t JOIN delivery d ON d.task_id = t.id
         WHERE t.id = ?1 AND d.pr_number IS ?2 AND d.stage NOT IN ('none', 'pr_open')
           AND (NOT ?6 OR EXISTS (SELECT 1 FROM task_merge_recovery e WHERE e.task_id = t.id
                AND e.pr_number = d.pr_number AND e.branch = ?3
                AND (e.head_sha IS NULL OR e.head_sha = ?4) AND (e.merged_sha IS NULL OR e.merged_sha = ?5)))
         ON CONFLICT(task_id, merged_sha) DO NOTHING",
        params![task, pr, landed.branch, landed.head, landed.sha, landed.source == LandingSource::ClosedPull],
    ).map_err(db)?;
    Ok(())
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

    async fn armed_notice_recipient(
        &self,
        task: &TaskId,
        pr: u64,
        branch: &str,
    ) -> AppResult<Option<yulius_app::pr_watch::NoticeRecipient>> {
        let task = task.to_string();
        let branch = branch.to_owned();
        let row: Option<(bool, Option<String>)> = self.call(move |conn| {
            conn.query_row(
                "SELECT t.activity IS 'working', CASE WHEN t.activity = 'needs_attention' THEN t.attention END
                 FROM tasks t JOIN delivery d ON d.task_id = t.id
                 WHERE t.id = ?1 AND d.stage = 'pr_open' AND d.pr_number = ?2 AND t.branch = ?3
                   AND t.status = 'running' AND t.archived_at IS NULL AND t.needs_you IS NULL
                   AND (t.activity IS NOT 'needs_attention' OR
                        CASE WHEN json_valid(t.attention) THEN json_extract(t.attention, '$.at') END = t.state_at)
                   AND NOT EXISTS (SELECT 1 FROM capacity_pauses WHERE task_id = t.id AND ended_at IS NULL)
                   AND NOT EXISTS (SELECT 1 FROM prompt_holds WHERE task_id = t.id AND state = 'pending')",
                params![task, to_i64(pr), branch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_err(db)
        }).await?;
        Ok(row.and_then(|(working, reason)| {
            let attention = reason
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .ok()?;
            Some(yulius_app::pr_watch::NoticeRecipient { working, attention })
        }))
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

    async fn transition_with_landing(
        &self,
        expected: &DeliveryStage,
        next: &DeliveryRecord,
        opening_base: Option<&str>,
        landing: Option<&Landing>,
    ) -> AppResult<bool> {
        let opening_base = opening_base.map(str::to_owned);
        let landing = landing.cloned();
        let expected = expected.encode();
        let next = next.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let current: Option<(String, Option<i64>)> = tx
                .query_row(
                    "SELECT stage, pr_number FROM delivery WHERE task_id = ?1",
                    [next.task_id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db)?;
            let (stage, pr) = current.as_ref().map_or(("none", None), |(s, n)| (s.as_str(), *n));
            if stage != expected || (stage == "pr_open" && next.stage == DeliveryStage::MergedIntegration
                && pr.is_some() && pr != next.pr_number.map(to_i64)) {
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
            if next.stage == DeliveryStage::PrOpen && let (Some(base), Some(pr)) = (opening_base, next.pr_number) {
                tx.execute(
                    "INSERT INTO task_merge_recovery(task_id, pr_number, branch) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                    params![next.task_id.as_str(), to_i64(pr), base],
                ).map_err(db)?;
            }
            // A merged pull request takes its review rounds along: the task's next one starts
            // at round 1 (SPEC-493 REV-002). Here, so every way a merge is recorded does it.
            if next.stage == DeliveryStage::MergedIntegration {
                if let Some(landing) = &landing {
                    if landing.source != LandingSource::AwaitingConfirmation {
                        record_landing(&tx, next.task_id.as_str(), next.pr_number.map(to_i64), landing)?;
                    } else if let Some(pr) = next.pr_number {
                        tx.execute(
                            "INSERT INTO task_merge_recovery(task_id, pr_number, branch, head_sha, merged_sha)
                             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(task_id, pr_number) DO UPDATE SET
                                branch = excluded.branch, head_sha = excluded.head_sha, merged_sha = excluded.merged_sha
                             WHERE task_merge_recovery.head_sha IS NULL",
                            params![next.task_id.as_str(), to_i64(pr), landing.branch, landing.head, landing.sha],
                        ).map_err(db)?;
                    }
                }
                tx.execute(
                    "UPDATE review_rounds SET pr_number = ?1, merged_at = ?2 \
                     WHERE task_id = ?3 AND merged_at IS NULL",
                    params![
                        next.pr_number.map(to_i64),
                        next.updated_at.0,
                        next.task_id.as_str()
                    ],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn landed_sha(
        &self,
        task: &TaskId,
        pr: Option<u64>,
        known: Option<&Landing>,
    ) -> AppResult<Option<String>> {
        let (task, pr, known) = (task.to_string(), pr.map(to_i64), known.cloned());
        self.call(move |c| {
            let lookup = |c: &rusqlite::Connection| {
                c.query_row(
                    "SELECT merged_sha FROM task_merges WHERE task_id = ?1 AND pr_number IS ?2
                     ORDER BY rowid DESC LIMIT 1",
                    params![task, pr],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)
            };
            let existing = lookup(c)?;
            if existing.is_none()
                && let Some(landed) = known.filter(|l| l.source == LandingSource::ClosedPull)
            {
                record_landing(c, &task, pr, &landed)?;
                return lookup(c);
            }
            Ok(existing)
        })
        .await
    }

    async fn merge_evidence(
        &self,
        task: &TaskId,
        sha: &str,
        run_id: Option<&str>,
        before: Option<&str>,
    ) -> AppResult<Option<MergeEvidence>> {
        const LIMIT: u16 = 50;
        let before = before.map(str::to_owned);
        let (task, sha, run_id) = (task.to_string(), sha.to_owned(), run_id.map(str::to_owned));
        self.call(move |c| {
            let landing = c
                .query_row(
                    "SELECT pr_number, branch, head_sha FROM task_merges
                 WHERE task_id = ?1 AND merged_sha = ?2",
                    params![task, sha],
                    |r| {
                        Ok((
                            r.get::<_, Option<i64>>(0)?.map(to_u64),
                            r.get(1)?,
                            r.get(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(db)?;
            let Some((pr_number, branch, head_sha)) = landing else {
                return Ok(None);
            };
            // The cursor names an attempt. Archiving preserves its original sort key.
            let boundary: Option<(i64, i64)> = if let Some(cursor) = &before {
                let found = c.query_row(
                    "SELECT at, seq FROM task_merge_runs WHERE task_id = ?1 AND merged_sha = ?2 AND run_id = ?3",
                    params![task, sha, cursor], |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional().map_err(db)?;
                if found.is_none() {
                    return Ok(None);
                }
                found
            } else {
                None
            };
            let mut stmt = c
                .prepare(
                    "SELECT result FROM task_merge_runs
                 WHERE task_id = ?1 AND merged_sha = ?2 AND (?3 IS NULL OR run_id = ?3)
                 AND (?4 IS NULL OR (at, seq, run_id) < (?5, ?6, ?4))
                 ORDER BY at DESC, seq DESC, run_id DESC LIMIT ?7",
                )
                .map_err(db)?;
            let (at, seq) = boundary.unzip();
            let rows = stmt
                .query_map(params![task, sha, run_id, before, at, seq, LIMIT + 1], |row| {
                    let result: String = row.get(0)?;
                    serde_json::from_str::<EvidenceRun>(&result)
                        .map_err(|err| bad_column("merge evidence", &err.to_string()))
                })
                .map_err(db)?;
            let mut runs = rows.collect::<Result<Vec<_>, _>>().map_err(db)?;
            if run_id.is_some() && runs.is_empty() {
                return Ok(None);
            }
            let truncated = runs.len() > usize::from(LIMIT);
            runs.truncate(usize::from(LIMIT));
            let next_before = runs.last().filter(|_| truncated).map(|run| run.run_id.clone());
            Ok(Some(MergeEvidence {
                url: format!("/api/tasks/{task}/delivery/evidence/{sha}"),
                task_id: task,
                merged_sha: sha,
                pr_number,
                branch,
                head_sha,
                runs,
                truncated,
                next_before,
            }))
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
                parked_attempt_id: None,
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
                owner: UserId::parse("u1")?,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                closed_at: None,
                archived_at: None,
                queued_at: None,
                gate_blocks: 0,
                wake_enabled: true,
                mode: yulius_app::model::TaskMode::Prompt,
                supertask: false,
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

    #[tokio::test]
    async fn legacy_evidence_binds_only_its_observed_merge_and_survives_a_follow_up() -> TestResult
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v76.db");
        crate::create_at_version(&path, 76)?;
        {
            let c = rusqlite::Connection::open(&path)?;
            c.execute_batch(r#"
                INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                VALUES ('s1','s','o','r','main','{"integration_branch":"changed-after-merge"}',20000,10,'u1','not_needed',1);
                INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, owner_id, created_at, updated_at)
                VALUES ('t1','s1',1,'Guide','Guide','claude','closed','ui','u1','u1',1,5);
                UPDATE tasks SET branch = 'docs/guide' WHERE id = 't1';
                INSERT INTO audit_log(at, actor, action, target, detail) VALUES
                   (1,'yulius','delivery.pr.open','s#1','pull request #7 docs/guide -> dev at aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
                INSERT INTO delivery (task_id, stage, pr_number, updated_at) VALUES ('t1','merged_integration',7,5);"#)?;
        }
        let store = SqliteStore::open(&path)?;
        let task = TaskId::parse("t1")?;
        let old = DeliveryRecord {
            task_id: task.clone(),
            stage: DeliveryStage::MergedIntegration,
            pr_number: Some(7),
            updated_at: Timestamp(5),
        };
        let landing = Landing {
            source: LandingSource::ClosedPull,
            branch: "dev".into(),
            head: "a".repeat(40),
            sha: "b".repeat(40),
        };
        let reconfigured = Landing {
            branch: "changed-after-merge".into(),
            ..landing.clone()
        };
        assert_eq!(
            store
                .landed_sha(&task, Some(7), Some(&reconfigured))
                .await?,
            None
        );
        assert_eq!(store.landed_sha(&task, Some(7), None).await?, None);
        assert_eq!(
            store.landed_sha(&task, Some(8), Some(&landing)).await?,
            None
        );
        assert_eq!(
            store.landed_sha(&task, Some(7), Some(&landing)).await?,
            Some(landing.sha.clone())
        );

        let new = DeliveryRecord {
            stage: DeliveryStage::PrOpen,
            pr_number: Some(8),
            ..old.clone()
        };
        assert!(store.transition_delivery(&old.stage, &new).await?);
        assert!(
            !store
                .transition_with_landing(&new.stage, &old, None, Some(&landing))
                .await?,
            "an old PR's delayed merge cannot bind the new open PR"
        );
        assert_eq!(
            store.landed_sha(&task, Some(8), Some(&landing)).await?,
            None
        );
        assert_eq!(store.delivery(&task).await?, Some(new));
        assert_eq!(
            store
                .merge_evidence(&task, &landing.sha, None, None)
                .await?
                .ok_or("old landing")?
                .pr_number,
            Some(7)
        );
        // A transient inability to confirm the next merge can recover only its original
        // branch, accepted head and merge SHA, even if the configured branch later changes.
        let pending = Landing {
            source: LandingSource::AwaitingConfirmation,
            sha: "c".repeat(40),
            ..landing.clone()
        };
        let next = DeliveryRecord {
            pr_number: Some(8),
            ..old
        };
        assert!(
            store
                .transition_with_landing(&DeliveryStage::PrOpen, &next, None, Some(&pending))
                .await?
        );
        assert_eq!(store.landed_sha(&task, Some(8), None).await?, None);
        let confirmed = Landing {
            source: LandingSource::ClosedPull,
            ..pending
        };
        for other in [
            Landing {
                branch: "new-base".into(),
                ..confirmed.clone()
            },
            Landing {
                head: "d".repeat(40),
                ..confirmed.clone()
            },
            Landing {
                sha: "e".repeat(40),
                ..confirmed.clone()
            },
        ] {
            assert_eq!(store.landed_sha(&task, Some(8), Some(&other)).await?, None);
        }
        assert_eq!(
            store.landed_sha(&task, Some(8), Some(&confirmed)).await?,
            Some(confirmed.sha.clone())
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
