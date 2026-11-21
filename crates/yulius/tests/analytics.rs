//! Analytics summary/export over visible sandboxes and the status panel (AC-ANL-1/3,
//! AC-STA-1; ANL-001..003, UI-007).

use crate::support;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use support::{
    Creds, TestResult, admit_and_sign_in, call, configure_github_app, harness, insert_sandbox,
    setup_superadmin_creds,
};
use tower::ServiceExt;
use yulius_app::model::{
    AccountRecord, AccountStatus, AttemptRecord, FeedbackRecord, LaunchKind, MetricsRecord, Sealed,
    SwitchDecisionRecord, TaskOrigin, TaskRecord,
};
use yulius_app::ports::store::{AccountStore, TaskStore};
use yulius_contracts::api::analytics::TASK_ROW_COLUMNS;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::task::{AttemptState, StopReason, TaskStatus};
use yulius_domain::time::Timestamp;

/// 2026-10-01T00:00:00Z.
const OCT1: i64 = 1_790_812_800_000;
const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
const MIN: i64 = 60_000;

struct TaskSpec<'a> {
    id: &'a str,
    sandbox: &'a str,
    title: &'a str,
    harness: Harness,
    status: TaskStatus,
    created: i64,
    closed: Option<i64>,
}

async fn task(h: &support::Harness, t: TaskSpec<'_>) -> TestResult {
    h.store
        .insert_task(&TaskRecord {
            parked_attempt_id: None,
            id: TaskId::parse(t.id)?,
            sandbox_id: SandboxId::parse(t.sandbox)?,
            number: 0,
            title: t.title.into(),
            prompt: String::new(),
            harness: t.harness,
            model: None,
            effort: None,
            workflow_id: None,
            status: t.status,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: UserId::parse("u")?,
            owner: UserId::parse("u")?,
            created_at: Timestamp(t.created),
            updated_at: Timestamp(t.created),
            closed_at: t.closed.map(Timestamp),
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: yulius_app::model::TaskMode::Prompt,
            supertask: false,
        })
        .await?;
    Ok(())
}

/// One attempt to insert: `(task, seq, harness, account, started, ended)`.
type RunSpec<'a> = (&'a str, u32, Harness, &'a str, i64, Option<i64>);

async fn attempt(
    h: &support::Harness,
    (task, seq, harness, account, started, ended): RunSpec<'_>,
    metrics: Option<MetricsRecord>,
) -> TestResult {
    let id = AttemptId::parse(&format!("{task}-a{seq}"))?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: id.clone(),
            task_id: TaskId::parse(task)?,
            seq,
            harness,
            account_id: Some(AccountId::parse(account)?),
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            model: None,
            state: if ended.is_some() {
                AttemptState::Exited
            } else {
                AttemptState::Live
            },
            run_dir: "/nonexistent".into(),
            token_hash: format!("hash-{task}-{seq}"),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(started),
            ended_at: ended.map(Timestamp),
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    if let Some(m) = metrics {
        h.store
            .put_metrics(&id, &TaskId::parse(task)?, &m, Timestamp(started))
            .await?;
    }
    Ok(())
}

async fn account(
    h: &support::Harness,
    id: &str,
    provider: Provider,
    label: &str,
    usage: Option<&str>,
    last_error: Option<&str>,
) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider,
            label: label.into(),
            credential: Some(Sealed(vec![1, 2, 3])),
            status: AccountStatus::Ok,
            usage_json: usage.map(str::to_owned),
            last_error: last_error.map(str::to_owned),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
            email: (label == "Claude A").then(|| "a@example.com".to_owned()),
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    Ok(())
}

async fn switch(h: &support::Harness, id: &str, task: &str, at: i64, inputs: Value) -> TestResult {
    h.store
        .insert_switch(&SwitchDecisionRecord {
            id: id.into(),
            task_id: TaskId::parse(task)?,
            at: Timestamp(at),
            reason: "usage_limit".into(),
            inputs: inputs.to_string(),
            from_desc: String::new(),
            to_desc: String::new(),
            outcome: "completed".into(),
        })
        .await?;
    Ok(())
}

async fn feedback(h: &support::Harness, task: &str, user: &str, verdict: &str) -> TestResult {
    h.store
        .put_feedback(&FeedbackRecord {
            task_id: TaskId::parse(task)?,
            user_id: UserId::parse(user)?,
            verdict: verdict.into(),
            comment: "synthetic".into(),
            at: Timestamp(OCT1 + 5 * DAY),
        })
        .await?;
    Ok(())
}

/// Superadmin, alice (read on `studio` only), two sandboxes, two accounts.
async fn base(h: &support::Harness) -> Result<(Creds, Creds), Box<dyn std::error::Error>> {
    let admin = setup_superadmin_creds(h).await?;
    configure_github_app(h).await?;
    let alice = admit_and_sign_in(h, &admin, "alice", 11, false).await?;
    insert_sandbox(h, "s1", "studio", 20000).await?;
    insert_sandbox(h, "s2", "product", 20020).await?;
    h.github.set_perm(
        "alice",
        "Quartio-Studio/studio",
        GitHubPermissions {
            pull: true,
            ..Default::default()
        },
    );
    Ok((admin, alice))
}

/// The cohort used by the analytics test (see assertions for expected figures).
async fn seed_cohort(h: &support::Harness) -> TestResult {
    account(h, "acc-a", Provider::Anthropic, "Claude A", None, None).await?;
    account(h, "acc-b", Provider::Anthropic, "Claude B", None, None).await?;
    let t1 = OCT1 + 10 * HOUR;
    task(
        h,
        TaskSpec {
            id: "t1",
            sandbox: "s1",
            title: "Fix \"checkout\", totals",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Closed,
            created: t1,
            closed: Some(t1 + 2 * HOUR),
        },
    )
    .await?;
    let known = MetricsRecord {
        model: Some("claude-opus".into()),
        input_tokens: Some(100),
        output_tokens: Some(50),
        cache_read: Some(10),
        cache_write: None,
        turns: Some(3),
        active_ms: Some(600_000),
    };
    attempt(
        h,
        (
            "t1",
            1,
            Harness::ClaudeCode,
            "acc-a",
            t1,
            Some(t1 + 30 * MIN),
        ),
        Some(known),
    )
    .await?;
    let model_only = MetricsRecord {
        model: Some("claude-sonnet".into()),
        ..MetricsRecord::default()
    };
    attempt(
        h,
        (
            "t1",
            2,
            Harness::ClaudeCode,
            "acc-b",
            t1 + 31 * MIN,
            Some(t1 + 41 * MIN),
        ),
        Some(model_only),
    )
    .await?;
    switch(
        h,
        "d1",
        "t1",
        t1 + 30 * MIN + 30_000,
        json!({"account": "acc-b"}),
    )
    .await?;
    feedback(h, "t1", "u-alice", "correct").await?;

    let t2 = OCT1 + 2 * DAY + 9 * HOUR;
    task(
        h,
        TaskSpec {
            id: "t2",
            sandbox: "s1",
            title: "=SUM(A1)\nsecond line",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Stopped(StopReason::Cancelled),
            created: t2,
            closed: None,
        },
    )
    .await?;
    attempt(
        h,
        (
            "t2",
            1,
            Harness::ClaudeCode,
            "acc-a",
            t2,
            Some(t2 + 20 * MIN),
        ),
        None,
    )
    .await?;
    switch(
        h,
        "d2",
        "t2",
        t2 + 10 * MIN,
        json!({"harness": "claude", "account": "acc-a"}),
    )
    .await?;
    // A restart on a newer Claude Code keeps the subscription: it is no switch (SPEC-461).
    h.store
        .insert_switch(&SwitchDecisionRecord {
            id: "d3".into(),
            task_id: TaskId::parse("t2")?,
            at: Timestamp(t2 + 12 * MIN),
            reason: "claude_code".into(),
            inputs: json!({"account": "acc-a"}).to_string(),
            from_desc: String::new(),
            to_desc: String::new(),
            outcome: "completed".into(),
        })
        .await?;
    feedback(h, "t2", "u-alice", "incorrect").await?;

    task(
        h,
        TaskSpec {
            id: "t3",
            sandbox: "s1",
            title: "Queued idea",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Queued,
            created: OCT1 + 2 * DAY + 15 * HOUR,
            closed: None,
        },
    )
    .await?;

    // Invisible to alice.
    task(
        h,
        TaskSpec {
            id: "t4",
            sandbox: "s2",
            title: "secret product work",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Closed,
            created: OCT1 + DAY,
            closed: None,
        },
    )
    .await?;
    let big = MetricsRecord {
        input_tokens: Some(999),
        ..MetricsRecord::default()
    };
    attempt(
        h,
        (
            "t4",
            1,
            Harness::ClaudeCode,
            "acc-a",
            OCT1 + DAY,
            Some(OCT1 + DAY + MIN),
        ),
        Some(big),
    )
    .await?;

    // Visible but before the range.
    task(
        h,
        TaskSpec {
            id: "t5",
            sandbox: "s1",
            title: "older",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Closed,
            created: OCT1 - 11 * DAY,
            closed: None,
        },
    )
    .await?;
    let small = MetricsRecord {
        input_tokens: Some(7),
        ..MetricsRecord::default()
    };
    attempt(
        h,
        (
            "t5",
            1,
            Harness::ClaudeCode,
            "acc-a",
            OCT1 - 11 * DAY,
            Some(OCT1 - 11 * DAY + MIN),
        ),
        Some(small),
    )
    .await?;
    Ok(())
}

fn by<'a>(
    list: &'a Value,
    key: &str,
    value: &str,
) -> Result<&'a Value, Box<dyn std::error::Error>> {
    list.as_array()
        .and_then(|a| a.iter().find(|v| v[key] == value))
        .ok_or_else(|| format!("no {key}={value} in {list}").into())
}

#[tokio::test]
async fn summary_sums_known_usage_counts_unknown_runs_and_hides_invisible_sandboxes() -> TestResult
{
    let h = harness()?;
    let (admin, alice) = base(&h).await?;
    seed_cohort(&h).await?;

    let r = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary?from=2026-10-01&to=2026-10-07",
        None,
        &alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let s = &r.body;
    assert_eq!(
        s["range"],
        json!({"sandbox": null, "from": "2026-10-01", "to": "2026-10-07"})
    );
    assert_eq!(
        s["tasks"]["total"], 3,
        "t4 (invisible) and t5 (before range) excluded"
    );
    assert_eq!(s["tasks"]["closed"], 1);
    assert_eq!(
        s["tasks"]["by_status"],
        json!({"closed": 1, "stopped_cancelled": 1, "queued": 1})
    );
    assert_eq!(s["tasks"]["by_harness"], json!({"claude": 3}));
    let days: Vec<(String, u64)> = s["tasks_per_day"]
        .as_array()
        .ok_or("days")?
        .iter()
        .map(|d| {
            (
                d["date"].as_str().unwrap_or("").to_owned(),
                d["count"].as_u64().unwrap_or(99),
            )
        })
        .collect();
    assert_eq!(days.len(), 7, "every day of the range is present");
    assert_eq!(days[0], ("2026-10-01".to_owned(), 1));
    assert_eq!(days[1], ("2026-10-02".to_owned(), 0));
    assert_eq!(days[2], ("2026-10-03".to_owned(), 2));

    // Known values are summed; runs without usage are counted, never zero-filled.
    assert_eq!(
        s["tokens"],
        json!({"runs": 3, "tasks": 2, "input": 100, "output": 50, "cache_read": 10,
               "cache_write": null, "unknown_usage_runs": 2})
    );
    let opus = by(&s["tokens_by_model"], "model", "claude-opus")?;
    assert_eq!(
        (&opus["input"], &opus["unknown_usage_runs"]),
        (&json!(100), &json!(0))
    );
    let sonnet = by(&s["tokens_by_model"], "model", "claude-sonnet")?;
    assert_eq!(
        (&sonnet["input"], &sonnet["unknown_usage_runs"]),
        (&Value::Null, &json!(1))
    );
    let unknown = by(&s["tokens_by_model"], "model", "unknown")?;
    assert_eq!(unknown["runs"], 1);
    let claude = by(&s["tokens_by_harness"], "harness", "claude")?;
    assert_eq!(
        (
            &claude["runs"],
            &claude["input"],
            &claude["unknown_usage_runs"]
        ),
        (&json!(3), &json!(100), &json!(2))
    );
    let a = by(&s["tokens_by_account"], "account_label", "Claude A")?;
    assert_eq!(
        (&a["account_id"], &a["runs"], &a["input"]),
        (&json!("acc-a"), &json!(2), &json!(100))
    );
    assert!(
        a["account_email"].is_null(),
        "subscription emails are for administrators (UI-022, SPEC-244)"
    );
    let b = by(&s["tokens_by_account"], "account_label", "Claude B")?;
    assert!(b["output"].is_null());
    let as_admin = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary?from=2026-10-01&to=2026-10-07",
        None,
        &admin,
    )
    .await?;
    let a = by(
        &as_admin.body["tokens_by_account"],
        "account_label",
        "Claude A",
    )?;
    assert_eq!(a["account_email"], "a@example.com", "SPEC-244 SUB-005");
    let b = by(
        &as_admin.body["tokens_by_account"],
        "account_label",
        "Claude B",
    )?;
    assert!(b["account_email"].is_null(), "unknown stays unknown");

    // Durations from attempts: t1 = 30 + 10 min, t2 = 20 min; active time from metrics: only
    // t1's first run reported it (SPEC-087 MET-005).
    assert_eq!(
        s["duration_ms"],
        json!({"median": 1_800_000, "p90": 2_400_000, "total_wall": 3_600_000, "total_active": 600_000,
            "median_active": 600_000, "p90_active": 600_000})
    );
    assert_eq!(s["switches"], json!({"total": 2}));
    assert_eq!(s["feedback"], json!({"correct": 1, "incorrect": 1}));
    assert_eq!(s["truncated"], false);

    // Without a range: the older visible task counts too, and only days with tasks appear.
    let all = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary",
        None,
        &alice,
    )
    .await?;
    assert_eq!(all.body["tasks"]["total"], 4);
    assert_eq!(all.body["tokens"]["input"], 107);
    assert_eq!(all.body["tasks_per_day"].as_array().map(Vec::len), Some(3));

    // One sandbox: visible → same cohort; invisible → not found.
    let one = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary?sandbox=s1&from=2026-10-01&to=2026-10-07",
        None,
        &alice,
    )
    .await?;
    assert_eq!(one.body["tasks"], s["tasks"]);
    let hidden = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary?sandbox=s2",
        None,
        &alice,
    )
    .await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);

    for bad in [
        "from=2026-13-01",
        "from=2026-10-07&to=2026-10-01",
        "to=yesterday",
    ] {
        let r = call(
            &h.router,
            Method::GET,
            &format!("/api/analytics/summary?{bad}"),
            None,
            &alice,
        )
        .await?;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    let anonymous = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // The superadmin owns the instance: every sandbox counts without a GitHub identity
    // (ACC-012, F 2026-10-09), the product task t4 included.
    let every = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary",
        None,
        &admin,
    )
    .await?;
    assert_eq!(every.status, StatusCode::OK, "{}", every.body);
    assert_eq!(every.body["tasks"]["total"], 5);
    assert_eq!(every.body["tokens"]["input"], 100 + 999 + 7);
    let product = call(
        &h.router,
        Method::GET,
        "/api/analytics/summary?sandbox=s2",
        None,
        &admin,
    )
    .await?;
    assert_eq!(product.status, StatusCode::OK, "{}", product.body);
    assert_eq!(product.body["tasks"]["total"], 1);
    Ok(())
}

async fn raw_get(
    h: &support::Harness,
    path: &str,
    creds: &Creds,
) -> Result<(StatusCode, axum::http::HeaderMap, String), Box<dyn std::error::Error>> {
    let cookie = support::cookie_header(creds.cookie.as_deref().ok_or("cookie")?);
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())?;
    let resp = h.router.clone().oneshot(req).await?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await?.to_bytes();
    Ok((status, headers, String::from_utf8(bytes.to_vec())?))
}

#[tokio::test]
async fn task_export_as_json_and_escaped_csv() -> TestResult {
    let h = harness()?;
    let (_, alice) = base(&h).await?;
    seed_cohort(&h).await?;
    let range = "from=2026-10-01&to=2026-10-07";

    let json_rows = call(
        &h.router,
        Method::GET,
        &format!("/api/analytics/tasks?format=json&{range}"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(json_rows.status, StatusCode::OK);
    let rows = json_rows.body.as_array().ok_or("rows")?;
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["task_id"].as_str()).collect();
    assert_eq!(
        ids,
        ["t1", "t2", "t3"],
        "oldest first, visible sandboxes only"
    );
    let t1 = &rows[0];
    assert_eq!(t1["sandbox"], "studio");
    assert_eq!(t1["harness"], "claude", "harness of the first run");
    assert_eq!(t1["harnesses"], "claude");
    assert_eq!(t1["models"], "claude-opus;claude-sonnet");
    assert_eq!(t1["accounts"], "Claude A;Claude B");
    assert_eq!(
        (&t1["attempts"], &t1["wall_ms"], &t1["active_ms"]),
        (&json!(2), &json!(2_400_000), &json!(600_000))
    );
    assert_eq!(
        (&t1["input_tokens"], &t1["cache_write_tokens"]),
        (&json!(100), &Value::Null)
    );
    assert_eq!(
        (&t1["unknown_usage_runs"], &t1["switches"]),
        (&json!(1), &json!(1))
    );
    assert_eq!(
        (&t1["feedback"], &t1["feedback_correct"]),
        (&json!("correct"), &json!(1))
    );
    assert_eq!(t1["closed_at"], "2026-10-01T12:00:00Z");
    let t3 = &rows[2];
    assert_eq!(
        (&t3["attempts"], &t3["wall_ms"], &t3["input_tokens"]),
        (&json!(0), &json!(0), &Value::Null)
    );

    let (status, headers, body) = raw_get(
        &h,
        &format!("/api/analytics/tasks?format=csv&{range}"),
        &alice,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/csv; charset=utf-8")
    );
    assert_eq!(
        headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok()),
        Some("attachment; filename=\"yulius-tasks-2026-10-01-2026-10-07.csv\"")
    );
    let records: Vec<&str> = body.split("\r\n").collect();
    assert_eq!(
        records.len(),
        5,
        "header, three rows, trailing terminator: {body:?}"
    );
    assert_eq!(records[0], TASK_ROW_COLUMNS.join(","));
    assert!(records[1].starts_with("t1,studio,"), "{}", records[1]);
    assert!(
        records[1].contains(",\"Fix \"\"checkout\"\", totals\",closed,"),
        "{}",
        records[1]
    );
    assert!(
        records[2].contains(",\"'=SUM(A1)\nsecond line\",stopped_cancelled,"),
        "{}",
        records[2]
    );
    // Unknown values are empty cells, not zeros: t3 has unknown active/token figures.
    assert!(records[3].contains(",0,0,,,,,,0,0,,0,0"), "{}", records[3]);

    let bad = call(
        &h.router,
        Method::GET,
        "/api/analytics/tasks?format=xml",
        None,
        &alice,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}

#[tokio::test]
async fn status_shows_vm_capacity_visible_sandboxes_and_role_scoped_accounts() -> TestResult {
    let h = harness()?;
    let (admin, alice) = base(&h).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 12, true).await?;
    account(
        &h,
        "acc-a",
        Provider::Anthropic,
        "Claude A",
        Some(r#"{"primary":{"used_percent":55.5,"window_minutes":300,"resets_at":1791334800000},"secondary":null,"observed_at":1791331200000}"#),
        Some("synthetic failure"),
    )
    .await?;
    account(&h, "acc-b", Provider::Anthropic, "Claude B", None, None).await?;
    for (id, sandbox, harness_kind, account_id) in [
        ("live1", "s1", Harness::ClaudeCode, "acc-a"),
        ("live2", "s2", Harness::ClaudeCode, "acc-b"),
    ] {
        task(
            &h,
            TaskSpec {
                id,
                sandbox,
                title: id,
                harness: harness_kind,
                status: TaskStatus::Running,
                created: OCT1,
                closed: None,
            },
        )
        .await?;
        attempt(&h, (id, 1, harness_kind, account_id, OCT1, None), None).await?;
    }
    task(
        &h,
        TaskSpec {
            id: "waiting",
            sandbox: "s1",
            title: "waiting",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Queued,
            created: OCT1,
            closed: None,
        },
    )
    .await?;

    let r = call(&h.router, Method::GET, "/api/status", None, &alice).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let s = &r.body;
    assert_eq!(
        s["vm"],
        json!({"cpu_percent": 12.5, "cpu_count": 4, "memory_used_bytes": 1000,
               "memory_total_bytes": 4000, "disk_used_bytes": null, "disk_total_bytes": null,
               "load": [0.5, 0.25, 0.125], "load_1m": 0.5, "uptime_s": 60}),
        "unknown disk stays null"
    );
    assert_eq!(s["sessions"], json!({"live": 2, "limit": 8, "queued": 1}));
    assert_eq!(
        s["sandboxes"],
        json!([{"id": "s1", "slug": "studio", "name": "studio", "live_sessions": 1}]),
        "only visible sandboxes"
    );
    assert_eq!(s["contained"], false);
    let accounts = s["accounts"].as_array().ok_or("accounts")?;
    assert_eq!(accounts.len(), 2);
    let a = by(&s["accounts"], "label", "Claude A")?;
    assert_eq!(a["provider"], "anthropic");
    assert_eq!(a["status"], "ok");
    assert_eq!(a["usage"]["primary"]["used_percent"], 55.5);
    assert!(a["usage"]["secondary"].is_null());
    for hidden in ["last_error", "in_use_by", "priority", "enabled"] {
        assert!(
            a.get(hidden).is_none(),
            "{hidden} is for administrators only"
        );
    }
    // Members name a subscription when they move their task to it (SPEC-447 MOV-004).
    assert!(a["id"].as_str().is_some_and(|id| !id.is_empty()), "{a}");
    let b = by(&s["accounts"], "label", "Claude B")?;
    assert!(b["usage"]["primary"].is_null(), "unknown usage stays null");

    // The superadmin sees every sandbox (ACC-012); an instance administrator only what GitHub
    // grants.
    let owner_view = call(&h.router, Method::GET, "/api/status", None, &admin).await?;
    let mut every = owner_view.body["sandboxes"]
        .as_array()
        .ok_or("sandboxes")?
        .clone();
    every.sort_by_key(|s| s["id"].as_str().unwrap_or_default().to_owned());
    assert_eq!(
        every,
        [
            json!({"id": "s1", "slug": "studio", "name": "studio", "live_sessions": 1}),
            json!({"id": "s2", "slug": "product", "name": "product", "live_sessions": 1})
        ],
        "{}",
        owner_view.body
    );
    let admin_view = call(&h.router, Method::GET, "/api/status", None, &bob).await?;
    let s = &admin_view.body;
    assert_eq!(
        s["sandboxes"],
        json!([]),
        "instance administration grants no repository access"
    );
    let a = by(&s["accounts"], "label", "Claude A")?;
    assert_eq!(
        (&a["id"], &a["in_use_by"], &a["last_error"]),
        (&json!("acc-a"), &json!(1), &json!("synthetic failure"))
    );
    assert!(
        a.get("priority").is_none() && a.get("enabled").is_none(),
        "the pool has no priorities or switches (F 2026-10-10)"
    );

    let anonymous = call(
        &h.router,
        Method::GET,
        "/api/status",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// SPEC-493 REV-006 (review round 1 of fix 1c): the reviews read counts only the sandboxes the
/// caller sees: a member of one of two counts its pull requests alone, and asking for the other
/// by id is not found.
#[tokio::test]
async fn the_reviews_read_counts_only_visible_sandboxes() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::model::ReviewRoundRecord;
    use yulius_app::ports::store::CoordinationStore as _;
    use yulius_app::ports::store_ext::DeliveryStore as _;
    use yulius_domain::delivery::DeliveryStage;
    let h = harness()?;
    let (admin, alice) = base(&h).await?;
    let merged_at = OCT1 + 10 * HOUR;
    for (id, sandbox) in [("t-seen", "s1"), ("t-hidden", "s2")] {
        task(
            &h,
            TaskSpec {
                id,
                sandbox,
                title: id,
                harness: Harness::ClaudeCode,
                status: TaskStatus::Running,
                created: OCT1,
                closed: None,
            },
        )
        .await?;
        h.store
            .insert_review_round(&ReviewRoundRecord {
                id: format!("round-{id}"),
                task_id: TaskId::parse(id)?,
                round: 1,
                head: None,
                agent: "yulius-reviewer".into(),
                model: "claude-fable-5-2".into(),
                reason: "round".into(),
                subagent_id: None,
                model_answered: None,
                started_at: Timestamp(OCT1),
                ended_at: Some(Timestamp(OCT1 + HOUR)),
                verdict: Some("approve".into()),
                cut_off: false,
                pr_number: None,
                merged_at: None,
            })
            .await?;
        // SPEC-506 FST-007: the seen pull request's second round, after the approval, asked
        // for changes.
        if id == "t-seen" {
            h.store
                .insert_review_round(&ReviewRoundRecord {
                    id: format!("round-2-{id}"),
                    task_id: TaskId::parse(id)?,
                    round: 2,
                    head: None,
                    agent: "yulius-reviewer".into(),
                    model: "claude-fable-5-2".into(),
                    reason: "round".into(),
                    subagent_id: None,
                    model_answered: None,
                    started_at: Timestamp(OCT1 + 2 * HOUR),
                    ended_at: Some(Timestamp(OCT1 + 3 * HOUR)),
                    verdict: Some("request_changes".into()),
                    cut_off: false,
                    pr_number: None,
                    merged_at: None,
                })
                .await?;
        }
        h.store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: TaskId::parse(id)?,
                    stage: DeliveryStage::MergedIntegration,
                    pr_number: Some(1),
                    updated_at: Timestamp(merged_at),
                },
            )
            .await?;
    }
    let read = |who: &Creds, query: &str| {
        let (router, who, uri) = (
            h.router.clone(),
            who.clone(),
            format!("/api/analytics/reviews?from=2026-10-01&to=2026-10-07{query}"),
        );
        async move { call(&router, Method::GET, &uri, None, &who).await }
    };
    let seen = read(&alice, "").await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(seen.body["pull_requests"], 1, "{}", seen.body);
    assert_eq!(seen.body["rounds_after_approval"], 1, "{}", seen.body);
    assert_eq!(
        seen.body["rounds_after_approval_asking"], 1,
        "{}",
        seen.body
    );
    assert_eq!(seen.body["re_reviews_after_changes"], 0, "{}", seen.body);
    assert_eq!(read(&admin, "").await?.body["pull_requests"], 2);
    let hidden = read(&alice, "&sandbox=s2").await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{}", hidden.body);
    assert_eq!(read(&alice, "&sandbox=s1").await?.body["pull_requests"], 1);
    Ok(())
}

/// SPEC-493 MQC-005 (AC-15): the merge-queue read counts the candidates formed, merged and
/// dissolved by why, per day, from the audit entries of the sandboxes the caller sees, and the
/// minutes from arming a merge to it; a hidden sandbox is not found.
#[tokio::test]
async fn the_merge_queue_read_counts_candidates_from_the_audit_log() -> TestResult {
    use yulius_app::ports::store::{AuditStore as _, SandboxStore as _};
    use yulius_domain::merge_queue as mq;
    let h = harness()?;
    let (admin, alice) = base(&h).await?;
    let s1 = h
        .store
        .sandbox(&SandboxId::parse("s1")?)
        .await?
        .ok_or("s1")?;
    let s2 = h
        .store
        .sandbox(&SandboxId::parse("s2")?)
        .await?
        .ok_or("s2")?;
    let entry = |at: i64, target: String, action: &'static str, detail: &'static str| {
        let store = h.store.clone();
        async move {
            store
                .audit(Timestamp(at), "yulius", action, Some(&target), Some(detail))
                .await
        }
    };
    let (one, two) = (s1.slug.clone(), format!("{}#7", s1.slug));
    entry(OCT1 + HOUR, one.clone(), mq::FORMED, "merge train #7, #8").await?;
    entry(
        OCT1 + 2 * HOUR,
        one.clone(),
        mq::DISSOLVED,
        "merge train #7, #8 (run r): #8's own run at abc ended failed: it leaves the candidate; \
         its pull requests merge one by one (why: member_left)",
    )
    .await?;
    entry(
        OCT1 + 2 * HOUR + 1,
        one.clone(),
        mq::FORMED,
        "merge train #7",
    )
    .await?;
    entry(
        OCT1 + 3 * HOUR,
        one.clone(),
        mq::MERGED,
        "merge train #7 merged",
    )
    .await?;
    entry(OCT1, two.clone(), "delivery.agent.merge_armed", "armed").await?;
    entry(OCT1 + 3 * HOUR, two, "delivery.agent.merge", "merged").await?;
    // A person's armed merge names the task by its id, and its merge by `<slug>#<n>`; another one
    // was armed before the range and merged in it.
    for (id, armed_at) in [("t-person", OCT1 + HOUR), ("t-early", OCT1 - 2 * HOUR)] {
        task(
            &h,
            TaskSpec {
                id,
                sandbox: "s1",
                title: id,
                harness: Harness::ClaudeCode,
                status: TaskStatus::Running,
                created: OCT1 - 3 * HOUR,
                closed: None,
            },
        )
        .await?;
        let number = h.store.task(&TaskId::parse(id)?).await?.ok_or(id)?.number;
        entry(armed_at, id.to_owned(), "delivery.person.merge_armed", "{}").await?;
        entry(
            OCT1 + 4 * HOUR,
            format!("{}#{number}", s1.slug),
            "delivery.person.merge",
            "merged",
        )
        .await?;
    }
    // Another sandbox's candidate, and an entry outside the range.
    entry(OCT1 + HOUR, s2.slug.clone(), mq::FORMED, "merge train #1").await?;
    entry(OCT1 - 30 * 24 * HOUR, one, mq::FORMED, "long ago").await?;
    let read = |who: &Creds, query: &str| {
        let (router, who, uri) = (
            h.router.clone(),
            who.clone(),
            format!("/api/analytics/merge-queue?from=2026-10-01&to=2026-10-07{query}"),
        );
        async move { call(&router, Method::GET, &uri, None, &who).await }
    };
    let seen = read(&alice, "").await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    let body = &seen.body;
    assert_eq!(
        (&body["formed"], &body["merged"], &body["dissolved"]),
        (&json!(2), &json!(1), &json!(1)),
        "{body}"
    );
    assert_eq!(body["dissolved_by"]["member_left"], 1, "{body}");
    // SPEC-497 MQW-004: each dissolved candidate is listed with its why and whole reason.
    let recent = body["dissolved_recent"]
        .as_array()
        .ok_or("dissolved_recent")?;
    assert_eq!(recent.len(), 1, "{body}");
    assert_eq!(recent[0]["why"], "member_left", "{body}");
    assert_eq!(recent[0]["sandbox"], s1.slug.as_str(), "{body}");
    assert!(
        recent[0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("(why: member_left)")),
        "{body}"
    );
    assert!(
        recent[0]["at"]
            .as_str()
            .is_some_and(|at| at.starts_with("2026-10-01T")),
        "{body}"
    );
    assert_eq!(body["days"][0]["day"], "2026-10-01", "{body}");
    // 180, 180 and 360 minutes.
    assert_eq!(body["merges"], 3, "{body}");
    assert_eq!(body["median_arm_to_merge_minutes"], 180.0);
    assert_eq!(read(&admin, "").await?.body["formed"], 3);
    let hidden = read(&alice, "&sandbox=s2").await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{}", hidden.body);
    Ok(())
}

/// SPEC-497 OFC-003: the merge queue's read counts CI minutes by how the runs ended: passed,
/// failed only for the infrastructure, cancelled, the thrown-away runs apart, and the share
/// outside the change; another sandbox's run only for the admin.
#[tokio::test]
async fn the_merge_queue_read_counts_ci_minutes_by_outcome() -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiJobUpdate, CiRunRecord, CiRunUpdate};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    let h = harness()?;
    let (admin, alice) = base(&h).await?;
    let min = 60_000;
    let cases = [
        (
            "s1",
            "p",
            RunState::Passed,
            Some(("address_pool", 3 * min, false)),
        ),
        (
            "s1",
            "o",
            RunState::Error,
            Some(("docker_socket", 2 * min, true)),
        ),
        ("s1", "c", RunState::Cancelled, None),
        ("s2", "q", RunState::Failed, None),
    ];
    for (n, (sandbox, id, end, infra)) in (0_i64..).zip(cases) {
        let at = OCT1 + HOUR + n * HOUR;
        let run = CiRunRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse(sandbox)?,
            reference: CiRef::PullRequest(1),
            sha: id.into(),
            source: GateSource::ProjectConfig,
            state: RunState::Queued,
            reason: None,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(at),
            started_at: None,
            ended_at: None,
        };
        let job = CiJobRecord {
            id: format!("{id}-job"),
            run_id: id.into(),
            ordinal: 0,
            name: "quality".into(),
            key: "quality".into(),
            command: "true".into(),
            matrix: None,
            state: RunState::Queued,
            reason: None,
            exit_code: None,
            output: String::new(),
            failures: None,
            started_at: None,
            ended_at: None,
            need: yulius_domain::resources::Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        };
        h.store.insert_ci_run(&run, &[job]).await?;
        let start = CiRunUpdate {
            started_at: Some(Timestamp(at)),
            ..CiRunUpdate::default()
        };
        h.store
            .transition_ci_run(id, &[RunState::Queued], RunState::Running, &start)
            .await?;
        let job_id = format!("{id}-job");
        h.store
            .transition_ci_job(
                &job_id,
                &[RunState::Queued],
                RunState::Running,
                &CiJobUpdate {
                    started_at: Some(Timestamp(at)),
                    ..CiJobUpdate::default()
                },
            )
            .await?;
        h.store
            .transition_ci_job(
                &job_id,
                &[RunState::Running],
                end,
                &CiJobUpdate {
                    ended_at: Some(Timestamp(at + 10 * min)),
                    infra,
                    ..CiJobUpdate::default()
                },
            )
            .await?;
        let ended = CiRunUpdate {
            ended_at: Some(Timestamp(at + 10 * min)),
            ..CiRunUpdate::default()
        };
        h.store
            .transition_ci_run(id, &[RunState::Running], end, &ended)
            .await?;
    }
    let read = |who: &Creds| {
        let (router, who) = (h.router.clone(), who.clone());
        async move {
            call(
                &router,
                Method::GET,
                "/api/analytics/merge-queue?from=2026-10-01&to=2026-10-07",
                None,
                &who,
            )
            .await
        }
    };
    let seen = read(&alice).await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(
        seen.body["ci_minutes"],
        json!({"passed": 10.0, "failed": 0.0, "failed_outside": 10.0, "cancelled": 10.0,
               "rerun": 5.0, "outside_share": 0.667}),
        "{}",
        seen.body
    );
    assert_eq!(seen.body["ci_minutes_by_day"][0]["day"], "2026-10-01");
    // The admin sees the other sandbox's failed run too.
    assert_eq!(read(&admin).await?.body["ci_minutes"]["failed"], 10.0);
    Ok(())
}

/// SPEC-497: once a minute the dispatcher keeps the host's load from CI (its CPUs busy since the
/// last reading, the CPUs running jobs reserved and measured, the jobs waiting), and
/// `GET /api/analytics/ci-load` reads it by hour for anyone signed in; a reversed range is
/// refused.
#[tokio::test]
async fn the_hosts_ci_load_is_kept_each_minute_and_read_by_hour() -> TestResult {
    use std::sync::atomic::Ordering;
    let h = harness()?;
    let (_admin, alice) = base(&h).await?;
    let start = OCT1 + 6 * 24 * HOUR; // 2026-10-07T00:00Z
    h.clock.0.store(start, Ordering::SeqCst);
    h.system.set_ticks(0, 0);
    h.app.ci_dispatch().await;
    // Twice in one minute: one reading.
    h.app.ci_dispatch().await;
    h.clock.0.store(start + 60_000, Ordering::SeqCst);
    h.system.set_ticks(300, 1000);
    h.app.ci_dispatch().await;
    let read = |query: &'static str| {
        let (router, who) = (h.router.clone(), alice.clone());
        async move { call(&router, Method::GET, query, None, &who).await }
    };
    let seen = read("/api/analytics/ci-load?from=2026-10-07&to=2026-10-07").await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(
        seen.body["hours"],
        json!([{"hour": "2026-10-07T00", "minutes": 2, "wait_job_minutes": 0,
                "busy_percent": 30.0, "held_cpus": 0.0, "measured_cpus": null}]),
        "{}",
        seen.body
    );
    assert_eq!(seen.body["busy_percent"], 30.0);
    let reversed = read("/api/analytics/ci-load?from=2026-10-08&to=2026-10-07").await?;
    assert_eq!(reversed.status, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}

/// Review round 2 of SPEC-493 fix 4: the merge queue's audit read keeps the newest entries
/// within its limit (oldest first), reads `<slug>` and `<slug>#<n>` but not `<slug>2#<n>`, and the
/// task ids it is given.
#[tokio::test]
async fn the_audit_read_of_a_sandbox_keeps_the_newest_entries() -> TestResult {
    use yulius_app::ports::store::AuditStore as _;
    let h = harness()?;
    for (at, target) in [
        (1, "app"),
        (2, "app#3"),
        (3, "t-person"),
        (4, "app2#1"),
        (5, "app#4"),
    ] {
        h.store
            .audit(
                Timestamp(at),
                "yulius",
                "delivery.train.formed",
                Some(target),
                Some("x"),
            )
            .await?;
    }
    let read = |limit: u32| {
        let store = h.store.clone();
        async move {
            store
                .audit_of_sandbox(
                    "app",
                    &["t-person".to_owned()],
                    &["delivery.train.formed"],
                    None,
                    None,
                    limit,
                )
                .await
        }
    };
    let all: Vec<(String, i64)> = read(10)
        .await?
        .into_iter()
        .map(|(target, entry)| (target, entry.at.0))
        .collect();
    assert_eq!(
        all,
        [
            ("app".to_owned(), 1),
            ("app#3".to_owned(), 2),
            ("t-person".to_owned(), 3),
            ("app#4".to_owned(), 5)
        ]
    );
    let newest: Vec<i64> = read(2).await?.into_iter().map(|(_, e)| e.at.0).collect();
    assert_eq!(newest, [3, 5], "the newest two, oldest first");
    Ok(())
}

/// SPEC-494 DFJ-013 (review round 1 of slices 5 to 7): the Docker read counts front restarts,
/// connections that gave way and Docker Needs you per week (named by its Monday) over HTTP,
/// from the audit entries of the sandboxes the caller sees and their tasks, within the range;
/// a hidden sandbox is not found and a reversed range is refused.
#[tokio::test]
async fn the_docker_read_counts_restarts_give_ways_and_needs_you_per_week() -> TestResult {
    use yulius_app::docker_trouble::{DOCKER_CI_LEFTOVERS, DOCKER_GAVE_WAY, DOCKER_RESTARTED};
    use yulius_app::needs_you::DOCKER_NEEDS_YOU;
    use yulius_app::ports::store::{AuditStore as _, SandboxStore as _};
    let h = harness()?;
    let (admin, alice) = base(&h).await?;
    let slug = |id: &'static str| {
        let store = h.store.clone();
        async move {
            Ok::<_, Box<dyn std::error::Error>>(
                store.sandbox(&SandboxId::parse(id)?).await?.ok_or(id)?.slug,
            )
        }
    };
    let (one, two) = (slug("s1").await?, slug("s2").await?);
    let entry = |at: i64, target: String, action: &'static str, detail: &'static str| {
        let store = h.store.clone();
        async move {
            store
                .audit(Timestamp(at), "yulius", action, Some(&target), Some(detail))
                .await
        }
    };
    task(
        &h,
        TaskSpec {
            id: "t-docker",
            sandbox: "s1",
            title: "t-docker",
            harness: Harness::ClaudeCode,
            status: TaskStatus::Running,
            created: OCT1 - 3 * HOUR,
            closed: None,
        },
    )
    .await?;
    // 2026-10-01 is a Thursday: week 2026-09-28; 2026-10-08 is in week 2026-10-05.
    entry(OCT1 + HOUR, one.clone(), DOCKER_RESTARTED, "it stayed full").await?;
    entry(OCT1 + 2 * HOUR, one.clone(), DOCKER_GAVE_WAY, "3").await?;
    entry(OCT1 + 7 * 24 * HOUR, one.clone(), DOCKER_GAVE_WAY, "2").await?;
    // SPEC-504 CLB-006: what CI jobs left, at their end and by the sweep.
    entry(
        OCT1 + 3 * HOUR,
        one.clone(),
        DOCKER_CI_LEFTOVERS,
        "3 1 job-end",
    )
    .await?;
    entry(
        OCT1 + 7 * 24 * HOUR,
        one.clone(),
        DOCKER_CI_LEFTOVERS,
        "2 2 sweep",
    )
    .await?;
    entry(
        OCT1 + 8 * 24 * HOUR,
        "t-docker".to_owned(),
        DOCKER_NEEDS_YOU,
        "Restart Docker",
    )
    .await?;
    // Another sandbox's restart, and one outside the range.
    entry(OCT1 + HOUR, two, DOCKER_RESTARTED, "a person asked").await?;
    entry(OCT1 - 30 * 24 * HOUR, one, DOCKER_RESTARTED, "long ago").await?;
    let read = |who: &Creds, query: &'static str| {
        let (router, who, uri) = (
            h.router.clone(),
            who.clone(),
            format!("/api/analytics/docker?from=2026-10-01&to=2026-10-14{query}"),
        );
        async move { call(&router, Method::GET, &uri, None, &who).await }
    };
    let seen = read(&alice, "").await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(
        seen.body["weeks"],
        json!([
            {"week": "2026-09-28", "restarts": 1, "gave_way": 3, "needs_you": 0,
             "ci_containers": 3, "ci_networks": 1, "ci_swept": 0},
            {"week": "2026-10-05", "restarts": 0, "gave_way": 2, "needs_you": 1,
             "ci_containers": 2, "ci_networks": 2, "ci_swept": 4},
        ]),
        "{}",
        seen.body
    );
    assert_eq!(
        (
            &seen.body["restarts"],
            &seen.body["gave_way"],
            &seen.body["needs_you"]
        ),
        (&json!(1), &json!(5), &json!(1)),
        "{}",
        seen.body
    );
    assert_eq!(
        (
            &seen.body["ci_containers"],
            &seen.body["ci_networks"],
            &seen.body["ci_swept"]
        ),
        (&json!(5), &json!(3), &json!(4)),
        "{}",
        seen.body
    );
    assert_eq!(read(&admin, "").await?.body["restarts"], 2);
    assert_eq!(read(&alice, "&sandbox=s1").await?.body["restarts"], 1);
    let hidden = read(&alice, "&sandbox=s2").await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{}", hidden.body);
    let reversed = call(
        &h.router,
        Method::GET,
        "/api/analytics/docker?from=2026-10-14&to=2026-10-01",
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        reversed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        reversed.body
    );
    Ok(())
}
