//! Read-only analytics queries over tasks, attempts, metrics, feedback and switch decisions.
//!
//! One database job runs all four queries against the same cohort definition, so the result
//! is consistent (the single writer thread interleaves no writes within a job).

use async_trait::async_trait;
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};
use yulius_app::analytics::{
    AnalyticsData, AnalyticsFeedback, AnalyticsQuery, AnalyticsRun, AnalyticsSwitch, AnalyticsTask,
};
use yulius_app::error::AppResult;
use yulius_app::model::{MetricsRecord, TaskOrigin};
use yulius_app::ports::store_ext::AnalyticsStore;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_u32, to_u64};

fn task_id(text: &str) -> rusqlite::Result<TaskId> {
    TaskId::parse(text).map_err(|_| bad_column("task id", text))
}

fn harness(text: &str) -> rusqlite::Result<Harness> {
    Harness::parse(text).map_err(|_| bad_column("harness", text))
}

fn opt_u64(row: &Row<'_>, column: &str) -> rusqlite::Result<Option<u64>> {
    Ok(row.get::<_, Option<i64>>(column)?.map(to_u64))
}

fn task_row(row: &Row<'_>) -> rusqlite::Result<AnalyticsTask> {
    let id: String = row.get("id")?;
    let sandbox: String = row.get("sandbox_id")?;
    let status: String = row.get("status")?;
    let activity: Option<String> = row.get("activity")?;
    let harness_text: String = row.get("harness")?;
    let origin: String = row.get("origin")?;
    Ok(AnalyticsTask {
        id: task_id(&id)?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        sandbox_slug: row.get("slug")?,
        number: to_u32(row.get("number")?),
        title: row.get("title")?,
        harness: harness(&harness_text)?,
        status: TaskStatus::parse(&status).ok_or_else(|| bad_column("status", &status))?,
        activity: activity.as_deref().and_then(Activity::parse),
        origin: TaskOrigin::parse(&origin),
        created_at: Timestamp(row.get("created_at")?),
        closed_at: row.get::<_, Option<i64>>("closed_at")?.map(Timestamp),
    })
}

fn run_row(row: &Row<'_>) -> rusqlite::Result<AnalyticsRun> {
    let task: String = row.get("task_id")?;
    let harness_text: String = row.get("harness")?;
    let account: Option<String> = row.get("account_id")?;
    let has_metrics: Option<String> = row.get("metrics_attempt")?;
    let metrics = match has_metrics {
        None => None,
        Some(_) => Some(MetricsRecord {
            model: row.get("model")?,
            input_tokens: opt_u64(row, "input_tokens")?,
            output_tokens: opt_u64(row, "output_tokens")?,
            cache_read: opt_u64(row, "cache_read")?,
            cache_write: opt_u64(row, "cache_write")?,
            turns: row.get::<_, Option<i64>>("turns")?.map(to_u32),
            active_ms: opt_u64(row, "active_ms")?,
        }),
    };
    Ok(AnalyticsRun {
        task_id: task_id(&task)?,
        seq: to_u32(row.get("seq")?),
        harness: harness(&harness_text)?,
        account_id: match account {
            Some(a) => Some(AccountId::parse(&a).map_err(|_| bad_column("account id", &a))?),
            None => None,
        },
        account_label: row.get("account_label")?,
        started_at: Timestamp(row.get("started_at")?),
        ended_at: row.get::<_, Option<i64>>("ended_at")?.map(Timestamp),
        metrics,
    })
}

/// `WHERE` clause (over alias `t`) and its parameters for a query.
fn cohort_filter(query: &AnalyticsQuery) -> (String, Vec<Value>) {
    let mut params: Vec<Value> = query
        .sandboxes
        .iter()
        .map(|s| Value::Text(s.as_str().to_owned()))
        .collect();
    let marks = vec!["?"; params.len()].join(", ");
    let mut clause = format!("t.sandbox_id IN ({marks})");
    if let Some(from) = query.from {
        clause.push_str(" AND t.created_at >= ?");
        params.push(Value::Integer(from.0));
    }
    if let Some(to) = query.to {
        clause.push_str(" AND t.created_at < ?");
        params.push(Value::Integer(to.0));
    }
    (clause, params)
}

fn collect<T>(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    f: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> AppResult<Vec<T>> {
    let mut stmt = conn.prepare(sql).map_err(db)?;
    let rows = stmt
        .query_map(params_from_iter(params.iter()), f)
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

fn read(conn: &Connection, query: &AnalyticsQuery) -> AppResult<AnalyticsData> {
    if query.sandboxes.is_empty() || query.limit == 0 {
        return Ok(AnalyticsData::default());
    }
    let (filter, params) = cohort_filter(query);
    let limit = query.limit;
    let cohort = format!(
        "WITH cohort AS (SELECT t.id FROM tasks t WHERE {filter} \
         ORDER BY t.created_at DESC, t.id DESC LIMIT {limit})"
    );
    let mut tasks = collect(
        conn,
        &format!(
            "SELECT t.id, t.sandbox_id, s.slug, t.number, t.title, t.harness, t.status, t.activity, \
             t.origin, t.created_at, t.closed_at FROM tasks t JOIN sandboxes s ON s.id = t.sandbox_id \
             WHERE {filter} ORDER BY t.created_at DESC, t.id DESC LIMIT {}",
            u64::from(limit) + 1
        ),
        &params,
        task_row,
    )?;
    let truncated = tasks.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    tasks.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    tasks.reverse();
    let runs = collect(
        conn,
        &format!(
            "{cohort} SELECT a.task_id, a.seq, a.harness, a.account_id, ac.label AS account_label, \
             a.started_at, a.ended_at, m.attempt_id AS metrics_attempt, m.model, m.input_tokens, \
             m.output_tokens, m.cache_read, m.cache_write, m.turns, m.active_ms \
             FROM attempts a JOIN cohort c ON c.id = a.task_id \
             LEFT JOIN metrics m ON m.attempt_id = a.id \
             LEFT JOIN accounts ac ON ac.id = a.account_id \
             ORDER BY a.task_id, a.seq"
        ),
        &params,
        run_row,
    )?;
    let switches = collect(
        conn,
        &format!(
            "{cohort} SELECT d.task_id, d.at, d.inputs FROM switch_decisions d \
             JOIN cohort c ON c.id = d.task_id ORDER BY d.at, d.id"
        ),
        &params,
        |row| {
            let task: String = row.get("task_id")?;
            Ok(AnalyticsSwitch {
                task_id: task_id(&task)?,
                at: Timestamp(row.get("at")?),
                inputs: row.get("inputs")?,
            })
        },
    )?;
    let feedback = collect(
        conn,
        &format!(
            "{cohort} SELECT f.task_id, f.verdict, f.at FROM feedback f \
             JOIN cohort c ON c.id = f.task_id ORDER BY f.at"
        ),
        &params,
        |row| {
            let task: String = row.get("task_id")?;
            Ok(AnalyticsFeedback {
                task_id: task_id(&task)?,
                verdict: row.get("verdict")?,
                at: Timestamp(row.get("at")?),
            })
        },
    )?;
    Ok(AnalyticsData {
        tasks,
        runs,
        switches,
        feedback,
        truncated,
    })
}

#[async_trait]
impl AnalyticsStore for SqliteStore {
    async fn analytics_data(&self, query: &AnalyticsQuery) -> AppResult<AnalyticsData> {
        let query = query.clone();
        self.call(move |conn| read(conn, &query)).await
    }
}

#[cfg(test)]
mod tests {
    use yulius_app::model::{
        AttemptRecord, FeedbackRecord, LaunchKind, SandboxRecord, Sealed, SwitchDecisionRecord,
        TaskRecord,
    };
    use yulius_app::ports::store::{SandboxStore, TaskStore};
    use yulius_domain::ids::{AttemptId, UserId};
    use yulius_domain::task::AttemptState;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn seed(store: &SqliteStore) -> TestResult {
        for (id, port) in [("s1", 20000u16), ("s2", 20020)] {
            store
                .insert_sandbox(&SandboxRecord {
                    id: SandboxId::parse(id)?,
                    slug: format!("slug-{id}"),
                    repo_owner: "o".into(),
                    repo_name: id.into(),
                    github_repo_id: None,
                    default_branch: "main".into(),
                    profile_json: "{}".into(),
                    port_base: port,
                    port_count: 20,
                    onboarding_state: "not_needed".into(),
                    onboarding_task_id: None,
                    created_by: UserId::parse("u")?,
                    created_at: Timestamp(0),
                    removed_at: None,
                })
                .await?;
        }
        for (task, sandbox, created) in [("t1", "s1", 100), ("t2", "s1", 200), ("t3", "s2", 150)] {
            store
                .insert_task(&TaskRecord {
                    id: TaskId::parse(task)?,
                    sandbox_id: SandboxId::parse(sandbox)?,
                    number: 0,
                    title: task.into(),
                    prompt: String::new(),
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
                    created_by: UserId::parse("u")?,
                    owner: UserId::parse("u")?,
                    created_at: Timestamp(created),
                    updated_at: Timestamp(created),
                    closed_at: None,
                    archived_at: None,
                    queued_at: None,
                    gate_blocks: 0,
                    wake_enabled: true,
                    mode: yulius_app::model::TaskMode::Prompt,
                    supertask: false,
                })
                .await?;
            let attempt = AttemptId::parse(&format!("a-{task}"))?;
            store
                .insert_attempt(&AttemptRecord {
                    id: attempt.clone(),
                    task_id: TaskId::parse(task)?,
                    seq: 1,
                    harness: Harness::ClaudeCode,
                    account_id: Some(AccountId::parse("gone")?),
                    provider_session_id: None,
                    launch_kind: LaunchKind::New,
                    state: AttemptState::Exited,
                    run_dir: String::new(),
                    token_hash: format!("hash-{task}"),
                    host_token: Sealed(vec![1]),
                    ptyhost_pid: None,
                    child_pid: None,
                    started_at: Timestamp(created),
                    ended_at: Some(Timestamp(created + 50)),
                    exit_code: Some(0),
                    exit_reason: None,
                })
                .await?;
            if task != "t2" {
                store
                    .put_metrics(
                        &attempt,
                        &TaskId::parse(task)?,
                        &MetricsRecord {
                            input_tokens: Some(10),
                            ..MetricsRecord::default()
                        },
                        Timestamp(created),
                    )
                    .await?;
            }
            store
                .put_feedback(&FeedbackRecord {
                    task_id: TaskId::parse(task)?,
                    user_id: UserId::parse("u")?,
                    verdict: "correct".into(),
                    comment: String::new(),
                    at: Timestamp(created),
                })
                .await?;
            store
                .insert_switch(&SwitchDecisionRecord {
                    id: format!("d-{task}"),
                    task_id: TaskId::parse(task)?,
                    at: Timestamp(created + 10),
                    reason: "manual".into(),
                    inputs: "{}".into(),
                    from_desc: String::new(),
                    to_desc: String::new(),
                    outcome: "completed".into(),
                })
                .await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn filters_by_sandbox_and_range_and_bounds_rows() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        seed(&store).await?;
        let all = store
            .analytics_data(&AnalyticsQuery {
                sandboxes: vec![SandboxId::parse("s1")?],
                from: None,
                to: None,
                limit: 100,
            })
            .await?;
        let ids: Vec<&str> = all.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t1", "t2"], "other sandboxes excluded, oldest first");
        assert_eq!(all.tasks[0].sandbox_slug, "slug-s1");
        assert_eq!(all.runs.len(), 2);
        let t2 = all.runs.iter().find(|r| r.task_id.as_str() == "t2");
        assert_eq!(
            t2.map(|r| r.metrics.is_none()),
            Some(true),
            "missing metrics stay unknown"
        );
        assert_eq!(
            all.runs[0].account_label, None,
            "deleted account has no label"
        );
        assert_eq!((all.switches.len(), all.feedback.len()), (2, 2));
        assert!(!all.truncated);

        let ranged = store
            .analytics_data(&AnalyticsQuery {
                sandboxes: vec![SandboxId::parse("s1")?, SandboxId::parse("s2")?],
                from: Some(Timestamp(150)),
                to: Some(Timestamp(200)),
                limit: 100,
            })
            .await?;
        assert_eq!(ranged.tasks.len(), 1);
        assert_eq!(ranged.tasks[0].id.as_str(), "t3");

        let bounded = store
            .analytics_data(&AnalyticsQuery {
                sandboxes: vec![SandboxId::parse("s1")?, SandboxId::parse("s2")?],
                from: None,
                to: None,
                limit: 2,
            })
            .await?;
        let ids: Vec<&str> = bounded.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t3", "t2"], "newest kept");
        assert!(bounded.truncated);
        assert!(bounded.runs.iter().all(|r| r.task_id.as_str() != "t1"));

        let none = store
            .analytics_data(&AnalyticsQuery {
                sandboxes: Vec::new(),
                from: None,
                to: None,
                limit: 10,
            })
            .await?;
        assert!(none.tasks.is_empty());
        Ok(())
    }
}
