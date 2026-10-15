//! Spans: the measured stages of tasks, CI runs and CD runs (SPEC-137, table `spans`).
//!
//! The triggers of migration 28 write the spans that follow a state, in the same transaction as
//! the state; the methods here write the stages Yulius measures without a state row and read
//! them back.

use async_trait::async_trait;
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params, params_from_iter};
use yulius_app::error::AppResult;
use yulius_app::ports::store_ext::SpanStore;
use yulius_app::stages::{NewSpan, SpanFilter, SpanRecord};
use yulius_domain::ids::SandboxId;
use yulius_domain::stages::{SubjectKind, stored_figure_name};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str = "sandbox_id, subject_kind, subject_id, key, parent_key, kind, name, environment, \
    job, started_at, ended_at, outcome, source, detail";

fn span_row(row: &Row<'_>) -> rusqlite::Result<SpanRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let kind: String = row.get("subject_kind")?;
    Ok(SpanRecord {
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        subject_kind: SubjectKind::parse(&kind).ok_or_else(|| bad_column("subject kind", &kind))?,
        subject_id: row.get("subject_id")?,
        key: row.get("key")?,
        parent_key: row.get("parent_key")?,
        kind: row.get("kind")?,
        name: row.get("name")?,
        environment: row.get("environment")?,
        job: row.get("job")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        outcome: row.get("outcome")?,
        source: row.get("source")?,
        detail: row.get("detail")?,
    })
}

fn select(conn: &Connection, filter: &str, args: &[Value]) -> AppResult<Vec<SpanRecord>> {
    let mut stmt = conn
        .prepare(&format!("SELECT {COLS} FROM spans WHERE {filter}"))
        .map_err(db)?;
    let rows = stmt
        .query_map(params_from_iter(args.iter()), span_row)
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

#[async_trait]
impl SpanStore for SqliteStore {
    async fn insert_span(&self, span: &NewSpan) -> AppResult<bool> {
        let span = span.clone();
        self.call(move |c| {
            let inserted = c
                .execute(
                    "INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, parent_key, \
                     kind, name, environment, job, started_at, ended_at, outcome, detail) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        span.sandbox_id.as_str(),
                        span.subject_kind.as_str(),
                        span.subject_id,
                        span.key,
                        span.parent_key,
                        span.kind,
                        span.name,
                        span.environment,
                        span.job,
                        span.started_at.0,
                        span.ended_at.map(|t| t.0.max(span.started_at.0)),
                        span.outcome,
                        span.detail,
                    ],
                )
                .map_err(db)?;
            Ok(inserted == 1)
        })
        .await
    }

    async fn end_span(
        &self,
        sandbox: &SandboxId,
        kind: SubjectKind,
        subject: &str,
        key: &str,
        at: Timestamp,
        outcome: Option<&str>,
    ) -> AppResult<bool> {
        let (sandbox, subject, key, outcome) = (
            sandbox.as_str().to_owned(),
            subject.to_owned(),
            key.to_owned(),
            outcome.map(str::to_owned),
        );
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE spans SET ended_at = MAX(started_at, ?5), outcome = COALESCE(?6, outcome) \
                     WHERE sandbox_id = ?1 AND subject_kind = ?2 AND subject_id = ?3 AND key = ?4 \
                     AND ended_at IS NULL",
                    params![sandbox, kind.as_str(), subject, key, at.0, outcome],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn span_figures(
        &self,
        sandbox: &SandboxId,
        kind: SubjectKind,
        subject: &str,
        key: &str,
        figures: &[(String, f64)],
        add: bool,
    ) -> AppResult<()> {
        let (sandbox, subject, key) = (
            sandbox.as_str().to_owned(),
            subject.to_owned(),
            key.to_owned(),
        );
        // Only figure names reach a JSON path (they are checked again here).
        let figures: Vec<(String, f64)> = figures
            .iter()
            .filter(|(name, value)| stored_figure_name(name) && value.is_finite())
            .cloned()
            .collect();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            for (name, value) in figures {
                let path = format!("$.{name}");
                tx.execute(
                    "UPDATE spans SET detail = json_set(COALESCE(detail, '{}'), ?5, \
                     CASE WHEN ?7 THEN COALESCE(json_extract(detail, ?5), 0) + ?6 ELSE ?6 END) \
                     WHERE sandbox_id = ?1 AND subject_kind = ?2 AND subject_id = ?3 AND key = ?4",
                    params![sandbox, kind.as_str(), subject, key, path, value, add],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn subject_spans(
        &self,
        sandbox: &SandboxId,
        kind: SubjectKind,
        subject: &str,
        limit: u32,
    ) -> AppResult<Vec<SpanRecord>> {
        let args = vec![
            Value::Text(sandbox.as_str().to_owned()),
            Value::Text(kind.as_str().to_owned()),
            Value::Text(subject.to_owned()),
        ];
        self.call(move |c| {
            let mut rows = select(
                c,
                &format!(
                    "sandbox_id = ?1 AND subject_kind = ?2 AND subject_id = ?3 \
                     ORDER BY started_at DESC, id DESC LIMIT {limit}"
                ),
                &args,
            )?;
            rows.reverse();
            Ok(rows)
        })
        .await
    }

    async fn ci_reference_spans(
        &self,
        sandbox: &SandboxId,
        number: u64,
    ) -> AppResult<Vec<SpanRecord>> {
        let args = vec![
            Value::Text(sandbox.as_str().to_owned()),
            Value::Text(format!("pr/{number}")),
            Value::Text(format!("%+{number}+%")),
        ];
        self.call(move |c| {
            select(
                c,
                "sandbox_id = ?1 AND subject_kind = 'ci_run' AND kind IN ('queue', 'run') AND \
                 (json_extract(detail, '$.reference') = ?2 OR \
                  (json_extract(detail, '$.reference') LIKE 'train/%' AND \
                   '+' || substr(json_extract(detail, '$.reference'), 7) || '+' LIKE ?3)) \
                 ORDER BY started_at, id LIMIT 500",
                &args,
            )
        })
        .await
    }

    async fn spans_between(&self, filter: &SpanFilter) -> AppResult<Vec<SpanRecord>> {
        let mut clause = String::from("sandbox_id = ? AND started_at >= ? AND started_at < ?");
        let mut args = vec![
            Value::Text(filter.sandbox.as_str().to_owned()),
            Value::Integer(filter.from.0),
            Value::Integer(filter.to.0),
        ];
        let optional = [
            (
                "subject_kind",
                filter.subject_kind.map(|k| k.as_str().to_owned()),
            ),
            ("kind", filter.kind.clone()),
            ("name", filter.name.clone()),
            ("environment", filter.environment.clone()),
            ("job", filter.job.clone()),
        ];
        for (column, value) in optional {
            if let Some(value) = value {
                clause.push_str(&format!(" AND {column} = ?"));
                args.push(Value::Text(value));
            }
        }
        let limit = filter.limit;
        self.call(move |c| {
            let mut rows = select(
                c,
                &format!("{clause} ORDER BY started_at DESC, id DESC LIMIT {limit}"),
                &args,
            )?;
            rows.reverse();
            Ok(rows)
        })
        .await
    }

    async fn prune_spans(&self, before: Timestamp) -> AppResult<u64> {
        self.call(move |c| {
            let deleted = c
                .execute(
                    "DELETE FROM spans WHERE started_at < ?1 AND (ended_at IS NOT NULL OR started_at < ?2)",
                    params![before.0, before.0 - yulius_domain::stages::RETAIN_MS],
                )
                .map_err(db)?;
            Ok(u64::try_from(deleted).unwrap_or(0))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value as Json;
    use yulius_app::model::{MetricsRecord, PauseKind, PauseRecord};
    use yulius_app::needs_you::NeedsYouRecord;
    use yulius_app::ports::store::{CoordinationStore as _, TaskStore as _};
    use yulius_app::ports::store_ext::{CapacityStore as _, CiStore as _, NeedsYouStore as _};
    use yulius_domain::ids::{AttemptId, StepId, TaskId};
    use yulius_domain::task::{Activity, TaskStatus};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn seeded() -> Result<SqliteStore, Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    async fn spans(store: &SqliteStore, kind: SubjectKind, id: &str) -> AppResult<Vec<SpanRecord>> {
        store
            .subject_spans(
                &SandboxId::parse("s1").map_err(|_| db(bad_column("s", "s1")))?,
                kind,
                id,
                1_000,
            )
            .await
    }

    fn detail(span: &SpanRecord) -> Json {
        serde_json::from_str(span.detail.as_deref().unwrap_or("{}")).unwrap_or_default()
    }

    /// A span as kind, name, start, end and outcome.
    type Shape = (String, String, i64, Option<i64>, Option<String>);

    fn summary(spans: &[SpanRecord]) -> Vec<Shape> {
        spans
            .iter()
            .map(|s| {
                (
                    s.kind.clone(),
                    s.name.clone(),
                    s.started_at.0,
                    s.ended_at.map(|t| t.0),
                    s.outcome.clone(),
                )
            })
            .collect()
    }

    /// STG-101..103: a task's states, needs-you request, step (with the tokens its runs recorded
    /// meanwhile), run and pause are spans written with the state's own time.
    #[tokio::test]
    async fn a_task_s_states_steps_runs_pauses_and_requests_become_spans() -> TestResult {
        let store = seeded().await?;
        let task = TaskId::parse("t1")?;
        store
            .transition_task(
                &task,
                Some(TaskStatus::Queued),
                TaskStatus::Starting,
                None,
                Timestamp(200),
            )
            .await?;
        store
            .transition_task(
                &task,
                Some(TaskStatus::Starting),
                TaskStatus::Running,
                Some(Activity::Working),
                Timestamp(300),
            )
            .await?;
        store
            .set_activity(&task, Some(Activity::NeedsAttention), Timestamp(400))
            .await?;
        store
            .set_activity(&task, Some(Activity::Working), Timestamp(500))
            .await?;
        // Setting the same value again is no new state.
        store
            .set_activity(&task, Some(Activity::Working), Timestamp(550))
            .await?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO steps (id, task_id, ordinal, title, status, effort) VALUES ('st1', 't1', 1, 'Spec written', 'pending', 2);
                     INSERT INTO attempts (id, task_id, seq, harness, launch_kind, state, run_dir, token_hash, host_token, started_at)
                       VALUES ('a1', 't1', 1, 'claude', 'new', 'live', '/r', 'h', x'00', 300);",
                )
                .map_err(db)
            })
            .await?;
        let attempt = AttemptId::parse("a1")?;
        let metrics = |output: u64, active: u64| MetricsRecord {
            model: Some("claude-opus".into()),
            input_tokens: Some(output * 2),
            output_tokens: Some(output),
            cache_read: None,
            cache_write: None,
            turns: Some(1),
            active_ms: Some(active),
        };
        store
            .put_metrics(&attempt, &task, &metrics(100, 1_000), Timestamp(590))
            .await?;
        let step = StepId::parse("st1")?;
        store
            .update_step(&step, "in_progress", None, "task:#1", Timestamp(600))
            .await?;
        store
            .put_metrics(&attempt, &task, &metrics(350, 4_000), Timestamp(650))
            .await?;
        store
            .update_step(&step, "done", None, "task:#1", Timestamp(700))
            .await?;
        let request = NeedsYouRecord {
            action: "Add DEV_HOST".into(),
            why: Some("deploy".into()),
            link: None,
            at: 800,
        };
        store
            .set_task_needs_you(&task, Some(&request), Timestamp(800))
            .await?;
        store
            .set_task_needs_you(&task, None, Timestamp(950))
            .await?;
        store
            .insert_pause(&PauseRecord {
                id: "p1".into(),
                task_id: task.clone(),
                kind: PauseKind::Limit,
                account_id: None,
                continuation: None,
                started_at: Timestamp(960),
                ended_at: None,
                outcome: None,
            })
            .await?;
        store.end_pause(&task, Timestamp(990), "switched").await?;
        store
            .transition_task(&task, None, TaskStatus::Closed, None, Timestamp(1_000))
            .await?;

        let all = spans(&store, SubjectKind::Task, "t1").await?;
        let states: Vec<_> = summary(&all)
            .into_iter()
            .filter(|s| s.0 == "state")
            .collect();
        assert_eq!(
            states,
            vec![
                ("state".into(), "queued".into(), 100, Some(200), None),
                ("state".into(), "starting".into(), 200, Some(300), None),
                ("state".into(), "working".into(), 300, Some(400), None),
                ("state".into(), "needs_you".into(), 400, Some(500), None),
                (
                    "state".into(),
                    "working".into(),
                    500,
                    Some(1_000),
                    Some("closed".into())
                ),
            ]
        );
        let step_span = all.iter().find(|s| s.kind == "step").ok_or("step")?;
        assert_eq!(
            (
                step_span.name.as_str(),
                step_span.started_at.0,
                step_span.ended_at.map(|t| t.0)
            ),
            ("Spec written", 600, Some(700))
        );
        assert_eq!(step_span.outcome.as_deref(), Some("done"));
        let d = detail(step_span);
        assert_eq!(
            (
                d["output_tokens"].as_i64(),
                d["input_tokens"].as_i64(),
                d["active_ms"].as_i64(),
                d["effort"].as_i64()
            ),
            (Some(250), Some(500), Some(3_000), Some(2))
        );
        let run = all.iter().find(|s| s.kind == "run").ok_or("run")?;
        assert_eq!((run.key.as_str(), run.ended_at), ("run:a1", None));
        assert_eq!(detail(run)["output_tokens"].as_i64(), Some(350));
        let asked = all.iter().find(|s| s.kind == "wait").ok_or("needs you")?;
        assert_eq!(
            (
                asked.name.as_str(),
                asked.started_at.0,
                asked.ended_at.map(|t| t.0),
                asked.outcome.as_deref()
            ),
            ("needs_you", 800, Some(950), Some("answered"))
        );
        assert_eq!(detail(asked)["action"], "Add DEV_HOST");
        let pause = all.iter().find(|s| s.kind == "pause").ok_or("pause")?;
        assert_eq!(
            (
                pause.name.as_str(),
                pause.ended_at.map(|t| t.0),
                pause.outcome.as_deref()
            ),
            ("limit", Some(990), Some("switched"))
        );
        assert!(all.iter().all(|s| s.source == "recorded"));
        Ok(())
    }

    /// STG-201: a CI run's queue says why it waited, its run and jobs follow their rows, a stage
    /// Yulius started ends with its job, a requeue ends what ran and queues again, and a result
    /// reused from an identical tree is marked.
    #[tokio::test]
    async fn ci_runs_queue_run_jobs_requeue_and_reuse_become_spans() -> TestResult {
        let store = seeded().await?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO ci_runs (id, sandbox_id, reference, sha, source, state, requested_by, queued_at) VALUES ('r1', 's1', 'pr/7', 'abc', 'yulius_toml', 'queued', 'yulius', 10);
                     INSERT INTO ci_jobs (id, run_id, ordinal, name, job_key, command, state) VALUES ('j1', 'r1', 0, 'test (1/2)', 'test', 'make', 'queued');
                     UPDATE ci_runs SET reason = 'waiting for disk space: 9 of 8 GiB used' WHERE id = 'r1';
                     UPDATE ci_runs SET state = 'running', reason = NULL, started_at = 20 WHERE id = 'r1';
                     UPDATE ci_jobs SET state = 'running', started_at = 25 WHERE id = 'j1';",
                )
                .map_err(db)
            })
            .await?;
        let sid = SandboxId::parse("s1")?;
        let stage = NewSpan {
            sandbox_id: sid.clone(),
            subject_kind: SubjectKind::CiRun,
            subject_id: "r1".into(),
            key: "job:j1@25/stage:1".into(),
            parent_key: Some("job:j1@25".into()),
            kind: "stage".into(),
            name: "lint".into(),
            environment: None,
            job: Some("test".into()),
            started_at: Timestamp(26),
            ended_at: None,
            outcome: None,
            detail: None,
        };
        assert!(store.insert_span(&stage).await?);
        assert!(!store.insert_span(&stage).await?, "a key is written once");
        store
            .span_figures(
                &sid,
                SubjectKind::CiRun,
                "r1",
                "job:j1@25/stage:1",
                &[("tests_passed".into(), 3.0)],
                true,
            )
            .await?;
        store
            .span_figures(
                &sid,
                SubjectKind::CiRun,
                "r1",
                "job:j1@25/stage:1",
                &[("tests_passed".into(), 4.0), ("$.bad".into(), 1.0)],
                true,
            )
            .await?;
        store
            .call(|c| c.execute_batch("UPDATE ci_jobs SET state = 'failed', exit_code = 2, ended_at = 40 WHERE id = 'j1';").map_err(db))
            .await?;
        assert!(store.requeue_ci_run("r1", "again", Timestamp(50)).await?);
        let all = spans(&store, SubjectKind::CiRun, "r1").await?;
        assert_eq!(
            summary(&all),
            vec![
                ("queue".into(), "queue".into(), 10, Some(20), None),
                (
                    "run".into(),
                    "run".into(),
                    20,
                    Some(50),
                    Some("requeued".into())
                ),
                (
                    "job".into(),
                    "test (1/2)".into(),
                    25,
                    Some(40),
                    Some("failed".into())
                ),
                (
                    "stage".into(),
                    "lint".into(),
                    26,
                    Some(40),
                    Some("failed".into())
                ),
                ("queue".into(), "queue".into(), 50, None, None),
            ]
        );
        assert_eq!(
            detail(&all[0])["reason"],
            "waiting for disk space: 9 of 8 GiB used"
        );
        assert_eq!(detail(&all[0])["reference"], "pr/7");
        // A timeline reads the newest spans, in start order (review M7).
        let newest = store
            .subject_spans(&sid, SubjectKind::CiRun, "r1", 2)
            .await?;
        assert_eq!(summary(&newest), summary(&all[3..]));
        assert_eq!(all[2].job.as_deref(), Some("test"));
        assert_eq!(detail(&all[2])["exit_code"].as_i64(), Some(2));
        assert_eq!(detail(&all[3])["tests_passed"].as_f64(), Some(7.0));
        assert!(detail(&all[3]).get("bad").is_none());
        assert_eq!(store.ci_reference_spans(&sid, 7).await?.len(), 3);

        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO ci_runs (id, sandbox_id, reference, sha, source, state, reason, requested_by, queued_at, started_at, ended_at)
                       VALUES ('r2', 's1', 'branch/dev', 'abc', 'yulius_toml', 'passed', 'not run again: dev has the same tree as pr/7, which passed', 'yulius', 60, 60, 60);
                     INSERT INTO ci_runs (id, sandbox_id, reference, sha, source, state, requested_by, queued_at) VALUES ('r3', 's1', 'train/6+7', 'def', 'yulius_toml', 'queued', 'yulius', 70);",
                )
                .map_err(db)
            })
            .await?;
        let reused = spans(&store, SubjectKind::CiRun, "r2").await?;
        assert_eq!(reused.len(), 2);
        assert!(
            reused
                .iter()
                .all(|s| detail(s)["reused"] == Json::Bool(true))
        );
        assert_eq!(
            store.ci_reference_spans(&sid, 7).await?.len(),
            4,
            "the train with #7"
        );
        assert_eq!(store.ci_reference_spans(&sid, 17).await?.len(), 0);
        Ok(())
    }

    /// STG-301: a CD run is a span with its trigger and commit; the phases Yulius started end with
    /// it (a failed run fails what was still open).
    #[tokio::test]
    async fn a_cd_run_s_spans_end_with_it() -> TestResult {
        let store = seeded().await?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO cd_runs (id, sandbox_id, environment, branch, sha, state, requested_by, started_at) VALUES ('cd1', 's1', 'dev', 'dev', 'abc', 'running', 'task:#3', 100);",
                )
                .map_err(db)
            })
            .await?;
        let sid = SandboxId::parse("s1")?;
        let phase = |key: &str, at: i64| NewSpan {
            sandbox_id: sid.clone(),
            subject_kind: SubjectKind::CdRun,
            subject_id: "cd1".into(),
            key: key.into(),
            parent_key: Some("run".into()),
            kind: key.trim_start_matches("run/").into(),
            name: key.trim_start_matches("run/").into(),
            environment: Some("dev".into()),
            job: None,
            started_at: Timestamp(at),
            ended_at: None,
            outcome: None,
            detail: None,
        };
        store.insert_span(&phase("run/deploy", 110)).await?;
        assert!(
            store
                .end_span(
                    &sid,
                    SubjectKind::CdRun,
                    "cd1",
                    "run/deploy",
                    Timestamp(150),
                    Some("passed")
                )
                .await?
        );
        assert!(
            !store
                .end_span(
                    &sid,
                    SubjectKind::CdRun,
                    "cd1",
                    "run/deploy",
                    Timestamp(160),
                    Some("x")
                )
                .await?,
            "ended once"
        );
        store.insert_span(&phase("run/verify", 150)).await?;
        store
            .call(|c| c.execute_batch("UPDATE cd_runs SET state = 'failed', exit_code = 3, ended_at = 170 WHERE id = 'cd1';").map_err(db))
            .await?;
        let all = spans(&store, SubjectKind::CdRun, "cd1").await?;
        assert_eq!(
            summary(&all),
            vec![
                (
                    "run".into(),
                    "run".into(),
                    100,
                    Some(170),
                    Some("failed".into())
                ),
                (
                    "deploy".into(),
                    "deploy".into(),
                    110,
                    Some(150),
                    Some("passed".into())
                ),
                (
                    "verify".into(),
                    "verify".into(),
                    150,
                    Some(170),
                    Some("failed".into())
                ),
            ]
        );
        assert_eq!(all[0].environment.as_deref(), Some("dev"));
        assert_eq!(
            (
                detail(&all[0])["requested_by"].as_str(),
                detail(&all[0])["sha"].as_str()
            ),
            (Some("task:#3"), Some("abc"))
        );
        Ok(())
    }

    /// STG-003: a span that cannot be written never fails the state write it follows, also under
    /// an upsert (whose conflict policy SQLite applies to a trigger's own inserts).
    #[tokio::test]
    async fn a_span_already_there_never_fails_the_state_write() -> TestResult {
        use yulius_app::delivery::DeliveryRecord;
        use yulius_app::ports::store_ext::DeliveryStore as _;
        use yulius_domain::delivery::DeliveryStage;
        let store = seeded().await?;
        store
            .call(|c| {
                c.execute_batch(
                    // The seeded task's queued span is id 1; these are ids 2 and 3, so the next
                    // span's key would be `…#4`: every later insert collides.
                    "INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at) VALUES
                       ('s1', 'task', 't1', 'state#4', 'state', 'planted', 1),
                       ('s1', 'task', 't1', 'delivery#4', 'delivery', 'planted', 1);",
                )
                .map_err(db)
            })
            .await?;
        let task = TaskId::parse("t1")?;
        assert!(
            store
                .transition_task(
                    &task,
                    Some(TaskStatus::Queued),
                    TaskStatus::Starting,
                    None,
                    Timestamp(200)
                )
                .await?
        );
        assert_eq!(
            store.task(&task).await?.ok_or("task")?.status,
            TaskStatus::Starting
        );
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
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            ..open.clone()
        };
        assert!(
            store
                .transition_delivery(&DeliveryStage::PrOpen, &merged)
                .await?,
            "a second stage in the same millisecond"
        );
        assert_eq!(
            store.delivery(&task).await?.map(|d| d.stage),
            Some(DeliveryStage::MergedIntegration)
        );
        Ok(())
    }

    /// Review L8: changes in the same millisecond each keep their span, and the last one stays
    /// open.
    #[tokio::test]
    async fn same_millisecond_changes_keep_every_state_span() -> TestResult {
        let store = seeded().await?;
        let task = TaskId::parse("t1")?;
        store
            .transition_task(
                &task,
                Some(TaskStatus::Queued),
                TaskStatus::Running,
                Some(Activity::Working),
                Timestamp(300),
            )
            .await?;
        store
            .set_activity(&task, Some(Activity::NeedsAttention), Timestamp(300))
            .await?;
        store
            .set_activity(&task, Some(Activity::Working), Timestamp(300))
            .await?;
        let states: Vec<_> = summary(&spans(&store, SubjectKind::Task, "t1").await?)
            .into_iter()
            .filter(|s| s.0 == "state")
            .collect();
        assert_eq!(
            states
                .iter()
                .map(|s| (s.1.as_str(), s.3))
                .collect::<Vec<_>>(),
            [
                ("queued", Some(300)),
                ("working", Some(300)),
                ("needs_you", Some(300)),
                ("working", None)
            ]
        );
        Ok(())
    }

    /// STG-005, STG-401: filters, newest-first limit in start order, and pruning by age.
    #[tokio::test]
    async fn spans_are_filtered_limited_and_pruned() -> TestResult {
        let store = seeded().await?;
        let sid = SandboxId::parse("s1")?;
        for (i, job) in [(1, "quality"), (2, "e2e"), (3, "quality")] {
            store
                .insert_span(&NewSpan {
                    sandbox_id: sid.clone(),
                    subject_kind: SubjectKind::CiRun,
                    subject_id: format!("r{i}"),
                    key: "job:x".into(),
                    parent_key: None,
                    kind: "job".into(),
                    name: job.into(),
                    environment: None,
                    job: Some(job.into()),
                    started_at: Timestamp(i * 1_000),
                    ended_at: Some(Timestamp(i * 1_000 + 10)),
                    outcome: Some("passed".into()),
                    detail: None,
                })
                .await?;
        }
        let filter = SpanFilter {
            sandbox: sid.clone(),
            from: Timestamp(0),
            to: Timestamp(10_000),
            subject_kind: Some(SubjectKind::CiRun),
            kind: Some("job".into()),
            name: None,
            environment: None,
            job: Some("quality".into()),
            limit: 10,
        };
        let found = store.spans_between(&filter).await?;
        assert_eq!(
            found
                .iter()
                .map(|s| s.subject_id.as_str())
                .collect::<Vec<_>>(),
            ["r1", "r3"]
        );
        let newest = store
            .spans_between(&SpanFilter {
                job: None,
                limit: 2,
                ..filter.clone()
            })
            .await?;
        assert_eq!(
            newest
                .iter()
                .map(|s| s.subject_id.as_str())
                .collect::<Vec<_>>(),
            ["r2", "r3"]
        );
        let other = store
            .spans_between(&SpanFilter {
                sandbox: SandboxId::parse("s2")?,
                ..filter.clone()
            })
            .await?;
        assert!(other.is_empty());
        assert_eq!(
            store.prune_spans(Timestamp(2_500)).await?,
            2,
            "the task's open state span stays"
        );
        assert_eq!(
            store
                .spans_between(&SpanFilter {
                    job: None,
                    ..filter
                })
                .await?
                .len(),
            1
        );
        Ok(())
    }
}
