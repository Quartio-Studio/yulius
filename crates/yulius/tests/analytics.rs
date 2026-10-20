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
    for hidden in ["id", "last_error", "in_use_by", "priority", "enabled"] {
        assert!(
            a.get(hidden).is_none(),
            "{hidden} is for administrators only"
        );
    }
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
