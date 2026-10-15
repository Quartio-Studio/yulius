//! Stages through the API (SPEC-137 STG-401, AC1, AC2, AC4, AC5): a task's lifecycle, a CI run
//! of its pull request and CD runs are recorded as their states change, then read back as the
//! task's timeline, the stages' percentiles and trend, the delivery figures and the export; a
//! person without access sees none of it.

use crate::support;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use support::{
    Creds, TestResult, call, configure_github_app, harness, perms, seed_github_user, seed_sandbox,
};
use tower::ServiceExt;
use yulius_app::ci::cd::{CdRunEnd, CdRunRecord};
use yulius_app::ci::{CiRunRecord, CiRunUpdate};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::model::{StepRecord, TaskMode, TaskOrigin, TaskRecord};
use yulius_app::ports::store::{CoordinationStore as _, TaskStore as _};
use yulius_app::ports::store_ext::{CdStore as _, CiStore as _, DeliveryStore as _};
use yulius_domain::cd::CdEnv;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, StepId, TaskId, UserId};
use yulius_domain::stages::ResourceUse;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

/// 2026-10-01T00:00:00Z.
const OCT1: i64 = 1_790_812_800_000;
const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;

struct Fixture {
    h: support::Harness,
    alice: Creds,
    bob: Creds,
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let h = harness()?;
    support::setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    h.github.set_perm("alice", "acme/app", perms("read"));
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    Ok(Fixture { h, alice, bob })
}

impl Fixture {
    async fn get(
        &self,
        path: &str,
        who: &Creds,
    ) -> Result<support::Resp, Box<dyn std::error::Error>> {
        call(&self.h.router, Method::GET, path, None, who).await
    }
}

fn task_record(id: &str) -> Result<TaskRecord, Box<dyn std::error::Error>> {
    Ok(TaskRecord {
        id: TaskId::parse(id)?,
        sandbox_id: SandboxId::parse("s1")?,
        number: 7,
        title: "Measure".into(),
        prompt: String::new(),
        harness: Harness::ClaudeCode,
        model: None,
        effort: None,
        workflow_id: None,
        status: TaskStatus::Queued,
        activity: None,
        branch: None,
        worktree_path: None,
        spec_path: None,
        origin: TaskOrigin::Ui,
        automation_run: None,
        created_by: UserId::parse("u-alice")?,
        owner: UserId::parse("u-alice")?,
        created_at: Timestamp(OCT1),
        updated_at: Timestamp(OCT1),
        closed_at: None,
        archived_at: None,
        queued_at: Some(Timestamp(OCT1)),
        gate_blocks: 0,
        wake_enabled: true,
        mode: TaskMode::Prompt,
        supertask: false,
    })
}

/// A task waits 10 minutes, works, needs a person for 5, does a step, opens #12 (tested by a CI
/// run that waited 2 minutes for a lane) which merges after 30 minutes; dev deploys fail once
/// and then pass.
async fn lifecycle(f: &Fixture) -> TestResult {
    let store = &f.h.store;
    let task = TaskId::parse("t1")?;
    store.insert_task(&task_record("t1")?).await?;
    store
        .transition_task(
            &task,
            Some(TaskStatus::Queued),
            TaskStatus::Running,
            Some(Activity::Working),
            Timestamp(OCT1 + 10 * MIN),
        )
        .await?;
    store
        .set_activity(
            &task,
            Some(Activity::NeedsAttention),
            Timestamp(OCT1 + 20 * MIN),
        )
        .await?;
    store
        .set_activity(&task, Some(Activity::Working), Timestamp(OCT1 + 25 * MIN))
        .await?;
    let step = StepRecord {
        id: StepId::parse("st1")?,
        task_id: task.clone(),
        ordinal: 1,
        title: "Spans stored".into(),
        status: "pending".into(),
        note: None,
        updated_by: None,
        updated_at: None,
        effort: 1,
    };
    store
        .replace_steps(&task, &[step], "task:#7", Timestamp(OCT1 + 26 * MIN))
        .await?;
    store
        .update_step(
            &StepId::parse("st1")?,
            "in_progress",
            None,
            "task:#7",
            Timestamp(OCT1 + 30 * MIN),
        )
        .await?;
    store
        .update_step(
            &StepId::parse("st1")?,
            "done",
            None,
            "task:#7",
            Timestamp(OCT1 + 50 * MIN),
        )
        .await?;
    let open = DeliveryRecord {
        task_id: task.clone(),
        stage: DeliveryStage::PrOpen,
        pr_number: Some(12),
        updated_at: Timestamp(OCT1 + HOUR),
    };
    store
        .transition_delivery(&DeliveryStage::None, &open)
        .await?;
    let ci = CiRunRecord {
        id: "ci1".into(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::PullRequest(12),
        sha: "abc1234".into(),
        source: GateSource::ProjectConfig,
        state: RunState::Queued,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(OCT1 + HOUR),
        started_at: None,
        ended_at: None,
    };
    store.insert_ci_run(&ci, &[]).await?;
    let started = CiRunUpdate {
        started_at: Some(Timestamp(OCT1 + HOUR + 2 * MIN)),
        ..CiRunUpdate::default()
    };
    store
        .transition_ci_run("ci1", &[RunState::Queued], RunState::Running, &started)
        .await?;
    let ended = CiRunUpdate {
        ended_at: Some(Timestamp(OCT1 + HOUR + 20 * MIN)),
        ..CiRunUpdate::default()
    };
    store
        .transition_ci_run("ci1", &[RunState::Running], RunState::Passed, &ended)
        .await?;
    let merged = DeliveryRecord {
        stage: DeliveryStage::MergedIntegration,
        updated_at: Timestamp(OCT1 + HOUR + 30 * MIN),
        ..open
    };
    store
        .transition_delivery(&DeliveryStage::PrOpen, &merged)
        .await?;
    // cd1's use was measured (SPEC-154), cd2's not.
    let used = ResourceUse {
        cpu_ms: 42_000,
        memory_peak_mb: 300,
        disk_written_mb: Some(12),
    };
    for (id, start, state, usage) in [
        ("cd1", OCT1 + 2 * HOUR, RunState::Failed, Some(used)),
        ("cd2", OCT1 + 3 * HOUR, RunState::Passed, None),
    ] {
        store
            .insert_cd_run(&CdRunRecord {
                id: id.into(),
                sandbox_id: SandboxId::parse("s1")?,
                environment: CdEnv::Dev,
                branch: "dev".into(),
                sha: "def5678".into(),
                state: RunState::Running,
                reason: None,
                requested_by: "task:#7".into(),
                task_id: None,
                deploy: Some("scripts/cd/deploy.sh".into()),
                verify: None,
                note: None,
                output: String::new(),
                exit_code: None,
                started_at: Timestamp(start),
                ended_at: None,
            })
            .await?;
        store
            .finish_cd_run(
                id,
                &CdRunEnd {
                    state,
                    reason: None,
                    exit_code: Some(0),
                    output: String::new(),
                    ended_at: Timestamp(start + 10 * MIN),
                    usage,
                },
            )
            .await?;
    }
    Ok(())
}

fn find<'a>(spans: &'a Value, kind: &str, name: &str) -> Result<&'a Value, String> {
    spans
        .as_array()
        .and_then(|all| all.iter().find(|s| s["kind"] == kind && s["name"] == name))
        .ok_or_else(|| format!("no {kind} {name} in {spans}"))
}

#[tokio::test]
async fn a_task_s_timeline_has_its_states_step_pull_request_and_ci_wait() -> TestResult {
    let f = fixture().await?;
    lifecycle(&f).await?;
    let r = f
        .get(
            "/api/sandboxes/s1/stages/timeline?subject=task:t1",
            &f.alice,
        )
        .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let spans = &r.body["spans"];
    let queued = find(spans, "state", "queued")?;
    assert_eq!(
        (queued["duration_ms"].as_i64(), queued["source"].as_str()),
        (Some(10 * MIN), Some("recorded"))
    );
    assert_eq!(find(spans, "state", "needs_you")?["duration_ms"], 5 * MIN);
    assert_eq!(
        find(spans, "step", "Spans stored")?["duration_ms"],
        20 * MIN
    );
    let pr = find(spans, "delivery", "pr_open")?;
    assert_eq!(
        (
            pr["duration_ms"].as_i64(),
            pr["outcome"].as_str(),
            pr["detail"]["pr"].as_i64()
        ),
        (Some(30 * MIN), Some("merged_integration"), Some(12))
    );
    let related = &r.body["related"];
    assert_eq!(find(related, "queue", "queue")?["duration_ms"], 2 * MIN);
    assert_eq!(find(related, "run", "run")?["duration_ms"], 18 * MIN);
    assert_eq!(find(related, "run", "run")?["subject_id"], "ci1");

    let ci = f
        .get(
            "/api/sandboxes/s1/stages/timeline?subject=ci_run:ci1",
            &f.alice,
        )
        .await?;
    assert_eq!(
        ci.body["spans"].as_array().map(Vec::len),
        Some(2),
        "{}",
        ci.body
    );
    for bad in ["task", "nope:t1", "task:"] {
        let r = f
            .get(
                &format!("/api/sandboxes/s1/stages/timeline?subject={bad}"),
                &f.alice,
            )
            .await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            r.body
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_summary_has_percentiles_a_daily_trend_and_delivery_figures() -> TestResult {
    let f = fixture().await?;
    lifecycle(&f).await?;
    let r = f
        .get(
            "/api/sandboxes/s1/stages/summary?from=2026-09-30&to=2026-10-02",
            &f.alice,
        )
        .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let rows = &r.body["stages"];
    let row = |subject: &str, kind: &str, name: &str| {
        rows.as_array()
            .and_then(|all| {
                all.iter().find(|s| {
                    s["subject_kind"] == subject && s["kind"] == kind && s["name"] == name
                })
            })
            .cloned()
            .ok_or_else(|| format!("no {subject} {kind} {name} in {rows}"))
    };
    let run = row("ci_run", "run", "run")?;
    assert_eq!(
        (
            run["count"].as_u64(),
            run["p50_ms"].as_i64(),
            run["p90_ms"].as_i64()
        ),
        (Some(1), Some(18 * MIN), Some(18 * MIN))
    );
    assert_eq!(
        row("ci_run", "queue", "queue · waiting for a lane")?["p50_ms"],
        2 * MIN
    );
    assert_eq!(row("task", "step", "step")?["count"], 1);
    let cd = row("cd_run", "run", "run")?;
    assert_eq!(
        (
            cd["count"].as_u64(),
            cd["failed"].as_u64(),
            cd["environment"].as_str()
        ),
        (Some(2), Some(1), Some("dev"))
    );
    // SPEC-154 USE-005: the percentiles of what the runs that measured it used.
    assert_eq!(
        (
            cd["cpu_p50_ms"].as_i64(),
            cd["cpu_p90_ms"].as_i64(),
            cd["memory_peak_p50_mb"].as_i64(),
            cd["memory_peak_p90_mb"].as_i64()
        ),
        (Some(42_000), Some(42_000), Some(300), Some(300))
    );
    assert!(run.get("cpu_p50_ms").is_none(), "unknown, not 0: {run}");
    let days = r.body["trend"]["days"].as_array().ok_or("days")?;
    assert_eq!(days.len(), 3);
    assert_eq!(
        (
            days[1]["date"].as_str(),
            days[1]["count"].as_u64(),
            days[1]["p50_ms"].as_i64()
        ),
        (Some("2026-10-01"), Some(1), Some(18 * MIN))
    );
    let dev = &r.body["delivery"][0];
    assert_eq!(
        (
            dev["environment"].as_str(),
            dev["deployments"].as_u64(),
            dev["passed"].as_u64()
        ),
        (Some("dev"), Some(2), Some(1))
    );
    assert_eq!(dev["change_failure_rate"], 0.5);
    // Merged at 01:30, deployed by the run that started at 03:00 and ended at 03:10.
    assert_eq!(dev["lead_time_p50_ms"], HOUR + 40 * MIN);
    assert_eq!(dev["time_to_restore_p50_ms"], HOUR);
    assert_eq!(r.body["delivery"][1]["deployments"], 0);
    assert!(
        r.body["options"]["environments"]
            .as_array()
            .is_some_and(|e| e.contains(&Value::from("dev")))
    );

    // Filters and a chosen trend.
    let r = f.get("/api/sandboxes/s1/stages/summary?from=2026-10-01&to=2026-10-01&subject=task&trend=task:state:needs_you", &f.alice).await?;
    assert!(
        r.body["stages"]
            .as_array()
            .is_some_and(|all| all.iter().all(|s| s["subject_kind"] == "task")),
        "{}",
        r.body
    );
    assert_eq!(r.body["trend"]["days"][0]["p50_ms"], 5 * MIN);
    for bad in [
        "from=2026-10-02&to=2026-10-01",
        "from=2025-01-01&to=2026-10-01",
        "subject=nope",
        "from=yesterday",
    ] {
        let r = f
            .get(&format!("/api/sandboxes/s1/stages/summary?{bad}"), &f.alice)
            .await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            r.body
        );
    }
    Ok(())
}

#[tokio::test]
async fn spans_export_as_csv_and_json_and_stay_inside_their_sandbox() -> TestResult {
    let f = fixture().await?;
    lifecycle(&f).await?;
    let json = f
        .get(
            "/api/sandboxes/s1/stages/spans?from=2026-10-01&to=2026-10-01&subject=cd_run",
            &f.alice,
        )
        .await?;
    assert_eq!(json.status, StatusCode::OK, "{}", json.body);
    assert_eq!(json.body.as_array().map(Vec::len), Some(2), "{}", json.body);
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/api/sandboxes/s1/stages/spans?from=2026-10-01&to=2026-10-01&format=csv");
    if let Some(token) = &f.alice.bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(cookie) = &f.alice.cookie {
        req = req.header(header::COOKIE, support::cookie_header(cookie));
    }
    let req = req.body(Body::empty())?;
    let resp = f.h.router.clone().oneshot(req).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let text = |name| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    let (content_type, disposition) = (
        text(header::CONTENT_TYPE),
        text(header::CONTENT_DISPOSITION),
    );
    assert!(content_type.starts_with("text/csv"), "{content_type}");
    assert!(
        disposition.contains("yulius-stages-2026-10-01-2026-10-01.csv"),
        "{disposition}"
    );
    let body = String::from_utf8(resp.into_body().collect().await?.to_bytes().to_vec())?;
    let mut lines = body.lines();
    assert_eq!(
        lines.next(),
        Some(yulius_app::stages::SPAN_COLUMNS.join(",").as_str())
    );
    assert!(lines.count() >= 8);
    let bad = f
        .get("/api/sandboxes/s1/stages/spans?format=xml", &f.alice)
        .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Bob has no access: the sandbox is not there for him.
    for path in [
        "/api/sandboxes/s1/stages/timeline?subject=task:t1",
        "/api/sandboxes/s1/stages/summary",
        "/api/sandboxes/s1/stages/spans",
    ] {
        let r = f.get(path, &f.bob).await?;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}: {}", r.body);
    }
    Ok(())
}

/// Review L9: a task of another sandbox read through this sandbox's timeline shows nothing: not
/// its spans and not this sandbox's CI runs that share its pull request's number.
#[tokio::test]
async fn a_task_of_another_sandbox_shows_no_timeline_here() -> TestResult {
    let f = fixture().await?;
    lifecycle(&f).await?;
    seed_sandbox(&f.h, "s2", "other", "u-alice").await?;
    let mut other = task_record("t2")?;
    other.sandbox_id = SandboxId::parse("s2")?;
    other.number = 1;
    f.h.store.insert_task(&other).await?;
    let task = TaskId::parse("t2")?;
    let open = DeliveryRecord {
        task_id: task,
        stage: DeliveryStage::PrOpen,
        pr_number: Some(12),
        updated_at: Timestamp(OCT1),
    };
    f.h.store
        .transition_delivery(&DeliveryStage::None, &open)
        .await?;
    let r = f
        .get(
            "/api/sandboxes/s1/stages/timeline?subject=task:t2",
            &f.alice,
        )
        .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.body["spans"].as_array().map(Vec::len),
        Some(0),
        "{}",
        r.body
    );
    assert_eq!(
        r.body["related"].as_array().map(Vec::len),
        Some(0),
        "{}",
        r.body
    );
    assert_eq!(r.body["truncated"], false);
    Ok(())
}
