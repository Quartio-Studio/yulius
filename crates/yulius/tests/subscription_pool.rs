//! The subscription pool at the hook and API level with a fake clock (SPEC-001 ROT-010,
//! ROT-013; SPEC-002 ADR-17, §7.5): turn boundaries held while no subscription has capacity
//! (the agent gets nothing), the hold surviving a server restart, release when capacity
//! returns, and the capacity view with its forecast. Real processes: `pool_supervision.rs`.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::connect_info::MockConnectInfo;
use axum::http::Method;
use serde_json::json;
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness,
    seed_sandbox, setup_superadmin_creds,
};
use yulius_app::context::App;
use yulius_app::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, PauseKind, PauseRecord, TaskMode,
    TaskOrigin, TaskRecord, UsageSampleRecord,
};
use yulius_app::ports::store::{AccountStore, TaskStore, UserStore};
use yulius_app::ports::store_ext::CapacityStore;
use yulius_domain::harness::{Harness as Agent, Provider};
use yulius_domain::ids::{AccountId, AttemptId, TaskId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

const HOUR: i64 = 3_600_000;
const TOKEN: &str = "pool-task-token-0123456789abcdef";

fn now(h: &Harness) -> Timestamp {
    Timestamp(h.clock.0.load(Ordering::SeqCst))
}

fn advance(h: &Harness, ms: i64) {
    h.clock.0.fetch_add(ms, Ordering::SeqCst);
}

/// Usage JSON with the 5-hour window at `five` % resetting at `five_resets` and the weekly
/// window at `week` % resetting at `week_resets`.
fn usage(five: f64, five_resets: Timestamp, week: f64, week_resets: Timestamp) -> String {
    json!({
        "primary": {"used_percent": five, "window_minutes": 300, "resets_at": five_resets.0},
        "secondary": {"used_percent": week, "window_minutes": 10080, "resets_at": week_resets.0},
        "observed_at": null,
    })
    .to_string()
}

async fn add_account(
    h: &Harness,
    id: &str,
    usage_json: Option<String>,
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    h.store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::Anthropic,
            label: format!("Claude {id}"),
            credential: Some(h.app.deps.crypto.seal(id.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(account)
}

/// A running task in `mode` whose live attempt (task token [`TOKEN`]) runs on `account`.
async fn running_task(
    h: &Harness,
    mode: TaskMode,
    account: &AccountId,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let created = now(h);
    let task = h
        .store
        .insert_task(&TaskRecord {
            id: TaskId::parse("t-pool")?,
            sandbox_id: yulius_domain::ids::SandboxId::parse("s1")?,
            number: 0,
            title: "Pool".into(),
            prompt: "Build the thing".into(),
            harness: Agent::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: Some("feat/1-pool".into()),
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: yulius_domain::ids::UserId::parse("u1")?,
            created_at: created,
            updated_at: created,
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode,
        })
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at-pool")?,
            task_id: task.id.clone(),
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: Some(account.clone()),
            provider_session_id: Some("session-1".into()),
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/nonexistent/run".into(),
            token_hash: h.app.deps.crypto.sha256_hex(TOKEN.as_bytes()),
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: created,
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(task.id)
}

async fn seed(h: &Harness) -> TestResult {
    h.store
        .insert_user(&yulius_app::model::UserRecord {
            id: yulius_domain::ids::UserId::parse("u1")?,
            github_login: Some("alice".into()),
            github_id: Some(1),
            display_name: "Alice".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        })
        .await?;
    seed_sandbox(h, "s1", "fixture", "u1").await?;
    Ok(())
}

/// POSTs a hook as the agent's hook client would; returns (status, Retry-After, body).
async fn hook(
    internal: &axum::Router,
    event: &str,
    body: serde_json::Value,
) -> Result<(u16, Option<String>, serde_json::Value), Box<dyn std::error::Error>> {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/hooks/claude/{event}"))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))?;
    let response = internal.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.into_body().collect().await?.to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok((status, retry_after, body))
}

async fn status(h: &Harness, task: &TaskId) -> Result<TaskStatus, Box<dyn std::error::Error>> {
    Ok(h.store.task(task).await?.ok_or("task")?.status)
}

/// AC-ROT-10: with every subscription at 98 % or limited, the Stop hook of a task that would
/// work on (goal mode) is held: `202` with `Retry-After` and an empty body — nothing reaches
/// the agent — and it stays held while the pause lasts, also through a server restart. When
/// the account's window resets, the resume loop releases it in place and the next poll answers
/// normally.
#[tokio::test]
async fn an_exhausted_pool_holds_the_turn_until_capacity_returns() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let t0 = now(&h);
    let only = add_account(
        &h,
        "acc-only",
        Some(usage(
            98.0,
            t0.plus_millis(2 * HOUR),
            40.0,
            t0.plus_millis(90 * HOUR),
        )),
    )
    .await?;
    let task = running_task(&h, TaskMode::Goal, &only).await?;

    let (code, retry_after, body) = hook(&h.internal, "Stop", json!({"session_id": "s"})).await?;
    assert_eq!(code, 202, "{body}");
    assert_eq!(retry_after.as_deref(), Some("10"));
    assert!(
        body.is_null() || body == json!({}),
        "no output reaches the agent: {body}"
    );
    assert_eq!(status(&h, &task).await?, TaskStatus::WaitingCapacity);
    let pause = h.store.open_pause(&task).await?.ok_or("no pause")?;
    assert_eq!(pause.kind, PauseKind::Turn);
    assert_eq!(pause.account_id.as_ref(), Some(&only));
    // The UI says when it resumes: the account's 5-hour reset.
    let summary = h
        .app
        .task_summary(&h.store.task(&task).await?.ok_or("task")?)
        .await?;
    assert_eq!(summary.status, "waiting_capacity");
    assert_eq!(summary.resumes_at.as_deref(), Some("2026-10-07T02:00:00Z"));

    // Still no capacity: the loop leaves it paused and the poll is held again.
    let _ = h.app.resume_round().await;
    assert_eq!(hook(&h.internal, "Stop", json!({})).await?.0, 202);

    // A restart: a new server process on the same database still holds the turn.
    let restarted = App::new(h.app.config.clone(), h.app.deps.clone());
    let internal = yulius_server::internal_router(restarted.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5002))));
    let (code, _, _) = hook(&internal, "Stop", json!({})).await?;
    assert_eq!(code, 202, "the pause survives a restart");

    // The 5-hour window resets: the restarted server's loop releases the task in place.
    advance(&h, 2 * HOUR + 1_000);
    let _ = restarted.resume_round().await;
    assert_eq!(status(&h, &task).await?, TaskStatus::Running);
    assert!(h.store.open_pause(&task).await?.is_none());
    let (code, _, body) = hook(&internal, "Stop", json!({})).await?;
    assert_eq!(code, 200, "the held turn goes on: {body}");
    let closed = h.store.pauses_since(t0).await?;
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].outcome.as_deref(), Some("released"));
    assert_eq!(
        h.store.attempts(&task).await?.len(),
        1,
        "resumed in place: no new run"
    );
    Ok(())
}

/// AC-ROT-10: a prompt submitted while the pool is exhausted waits at `UserPromptSubmit`; the
/// prompt is kept for a resume on another account. A task in prompt mode that only goes idle
/// at Stop is not held.
#[tokio::test]
async fn a_prompt_waits_at_submit_and_an_idle_turn_end_is_not_held() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let t0 = now(&h);
    let limited = json!({"primary": null, "secondary": null, "limit_reached": true,
        "limit_resets_at": t0.plus_millis(HOUR).0, "observed_at": t0.0})
    .to_string();
    let only = add_account(&h, "acc-only", Some(limited)).await?;
    let task = running_task(&h, TaskMode::Prompt, &only).await?;

    // The agent finished its turn and would go idle: nothing to hold.
    let (code, _, _) = hook(&h.internal, "Stop", json!({})).await?;
    assert_eq!(code, 200);
    assert_eq!(status(&h, &task).await?, TaskStatus::Running);

    // The next prompt waits for capacity.
    let prompt = json!({"prompt": "Now add the tests", "hook_event_name": "UserPromptSubmit"});
    let (code, _, body) = hook(&h.internal, "UserPromptSubmit", prompt.clone()).await?;
    assert_eq!(code, 202, "{body}");
    let pause = h.store.open_pause(&task).await?.ok_or("no pause")?;
    assert_eq!(pause.kind, PauseKind::Prompt);
    assert_eq!(pause.continuation.as_deref(), Some("Now add the tests"));
    assert_eq!(
        hook(&h.internal, "UserPromptSubmit", prompt.clone())
            .await?
            .0,
        202
    );

    advance(&h, HOUR + 1_000);
    let _ = h.app.resume_round().await;
    let (code, _, _) = hook(&h.internal, "UserPromptSubmit", prompt).await?;
    assert_eq!(code, 200, "the prompt goes on once the limit reset");
    assert_eq!(status(&h, &task).await?, TaskStatus::Running);
    Ok(())
}

/// AC-ROT-12: `GET /api/capacity` reports the pool, the burn rate, the forecast and the
/// recommendation from the readings, runs and pauses of the last week, by the formula of
/// SPEC-002 ADR-17 §2.4; ids and task counts only for administrators.
#[tokio::test]
async fn the_capacity_view_reports_burn_forecast_and_recommendation() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, false).await?;
    let alice_id = h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_sandbox(&h, "s1", "fixture", alice_id.as_str()).await?;
    let t0 = now(&h);
    let at = |hours: i64| t0.plus_millis(hours * HOUR);
    let a = add_account(&h, "acc-a", Some(usage(10.0, at(2), 60.0, at(48)))).await?;
    let b = add_account(&h, "acc-b", Some(usage(20.0, at(3), 90.0, at(5)))).await?;
    add_account(&h, "acc-c", None).await?;
    // 24 points over 24 h on a and b: 2 points/h.
    for (account, before, after, resets) in [(&a, 36.0, 60.0, at(48)), (&b, 66.0, 90.0, at(5))] {
        h.store
            .record_usage_samples(
                &[
                    UsageSampleRecord {
                        account_id: account.clone(),
                        at: at(-24),
                        window: "seven_day".into(),
                        used_percent: before,
                        resets_at: Some(resets),
                    },
                    UsageSampleRecord {
                        account_id: account.clone(),
                        at: t0,
                        window: "seven_day".into(),
                        used_percent: after,
                        resets_at: Some(resets),
                    },
                ],
                at(-200),
            )
            .await?;
    }
    // One task ran for the last 40 hours; it was held for capacity for 8 of them.
    h.store
        .insert_task(&TaskRecord {
            id: TaskId::parse("t1")?,
            sandbox_id: yulius_domain::ids::SandboxId::parse("s1")?,
            number: 0,
            title: "T".into(),
            prompt: "p".into(),
            harness: Agent::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: alice_id.clone(),
            created_at: at(-40),
            updated_at: at(-40),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: TaskMode::Prompt,
        })
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at1")?,
            task_id: TaskId::parse("t1")?,
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: Some(a.clone()),
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/nonexistent".into(),
            token_hash: "x".into(),
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: at(-40),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    h.store
        .insert_pause(&PauseRecord {
            id: "p1".into(),
            task_id: TaskId::parse("t1")?,
            kind: PauseKind::Turn,
            account_id: Some(a.clone()),
            continuation: None,
            started_at: at(-10),
            ended_at: None,
            outcome: None,
        })
        .await?;
    h.store
        .end_pause(&TaskId::parse("t1")?, at(-2), "released")
        .await?;

    let view = call(&h.router, Method::GET, "/api/capacity", None, &admin).await?;
    assert_eq!(view.status, 200, "{}", view.body);
    let v = &view.body;
    assert_eq!(v["threshold"], 98.0);
    assert_eq!(v["subscriptions"], 3);
    assert_eq!(v["unknown_usage"], 1);
    assert_eq!(v["available"], 3);
    assert_eq!(v["weekly_left"], 46.0, "38 + 8 points");
    assert_eq!(v["five_hour_left"], 166.0, "88 + 78 points");
    assert_eq!(v["burn_24h"], 2.0);
    assert_eq!(v["burn_7d"], 2.0);
    // 46 points at 2/h with b's reset at 5 h (+90) and a's at 48 h (+60): 98 h from now.
    assert_eq!(v["runs_out_at"], "2026-10-11T02:00:00Z");
    assert_eq!(v["paused_hours_7d"], 8.0);
    assert_eq!(v["active_hours_7d"], 32.0);
    // max(2 × 168 / 98 = 3.43, 3 × 40 / 32 = 3.75) → 4.
    assert_eq!(v["needed"], 4);
    assert_eq!(v["add"], 1);
    assert_eq!(
        v["recommendation"],
        "Add about 1 subscription to stop pausing"
    );
    assert_eq!(v["paused_tasks"], 0);
    assert!(v["resumes_at"].is_null());
    let first = &v["accounts"][0];
    assert_eq!(first["label"], "Claude acc-a");
    assert_eq!(first["id"], "acc-a");
    assert_eq!(first["in_use_by"], 1);
    assert_eq!(first["available"], true);

    let member = call(&h.router, Method::GET, "/api/capacity", None, &alice).await?;
    assert_eq!(member.status, 200);
    let first = &member.body["accounts"][0];
    assert!(first.get("id").is_none() && first.get("in_use_by").is_none());
    assert_eq!(member.body["add"], 1);
    let anonymous = call(
        &h.router,
        Method::GET,
        "/api/capacity",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, 401);
    Ok(())
}

/// No subscription at all and a task waiting: the forecast asks for one, and the capacity
/// view counts the paused task.
#[tokio::test]
async fn an_empty_pool_with_waiting_work_asks_for_a_subscription() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let quiet = call(&h.router, Method::GET, "/api/capacity", None, &admin).await?;
    assert_eq!(quiet.status, 200);
    assert_eq!(quiet.body["subscriptions"], 0);
    assert_eq!(quiet.body["recommendation"], "Capacity is sufficient");
    seed(&h).await?;
    let gone = add_account(&h, "acc-gone", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &gone).await?;
    h.store.delete_account(&gone).await?;
    h.app
        .pause_task(&task, PauseKind::Start, None, None)
        .await?;
    let view = call(&h.router, Method::GET, "/api/capacity", None, &admin).await?;
    assert_eq!(view.body["paused_tasks"], 1);
    assert_eq!(view.body["add"], 1);
    assert_eq!(
        view.body["recommendation"],
        "Add a subscription to start working"
    );
    let _ = Arc::clone(&h.app);
    Ok(())
}
