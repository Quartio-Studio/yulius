//! Instance updates through the API and the agent hooks (SPEC-001 OPS-006..OPS-009, SPEC-002
//! ADR-18), with a fake GitHub source, a fake update helper and a fake clock: the offer reaches
//! superadmins only, "Update now" needs the confirmed head of `main` and a helper, tasks keep
//! working while it builds, every turn boundary is held and starts queue while it installs, the
//! hold survives the restart, and the tasks are released one by one afterwards — or the update
//! is cancelled, gives up or is rolled back. The real helper: `src/update_helper/tests.rs`;
//! real processes through the hook client: `update_supervision.rs`.

use crate::support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::updates::{FakeHelper, FakeSource, FreezingHost, MID, NEW, OLD, services};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness,
    harness_with_host, seed_live_attempt_at, seed_sandbox, seed_task, setup_superadmin_creds,
};
use yulius_app::context::App;
use yulius_app::ports::session::RunFreeze;
use yulius_app::ports::store::{TaskStore, UserStore as _};
use yulius_app::ports::store_ext::AutomationStore as _;
use yulius_app::ports::updates::SourceError;
use yulius_app::supervisor::RunStart;
use yulius_contracts::update::{HelperAction, HelperState};
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

const WORKING: &str = "update-token-working-0123456789abcdef";
const IDLE: &str = "update-token-idle-0123456789abcdefgh";
const RUN_WORKING: &str = "/nonexistent/runs/a-t-working";
const RUN_IDLE: &str = "/nonexistent/runs/a-t-idle";

struct Instance {
    h: Harness,
    admin: Creds,
    member: Creds,
    source: Arc<FakeSource>,
    helper: Arc<FakeHelper>,
}

async fn instance() -> Result<Instance, Box<dyn std::error::Error>> {
    instance_on(harness()?).await
}

/// An instance whose process host freezes runs (SPEC-160).
async fn freezing_instance() -> Result<(Instance, Arc<FreezingHost>), Box<dyn std::error::Error>> {
    let host = FreezingHost::new();
    let i = instance_on(harness_with_host(host.clone())?).await?;
    Ok((i, host))
}

async fn instance_on(h: Harness) -> Result<Instance, Box<dyn std::error::Error>> {
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let member = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let source = FakeSource::behind();
    let helper = FakeHelper::installed();
    assert!(h.app.set_updates(services(OLD, &source, &helper)));
    Ok(Instance {
        h,
        admin,
        member,
        source,
        helper,
    })
}

async fn get(i: &Instance, who: &Creds) -> Result<Value, Box<dyn std::error::Error>> {
    let r = call(&i.h.router, Method::GET, "/api/update", None, who).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

async fn post(
    i: &Instance,
    path: &str,
    body: Option<Value>,
    who: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(&i.h.router, Method::POST, path, body, who).await
}

/// POSTs a hook as the agent's hook client would; returns (status, Retry-After, body).
async fn hook(
    internal: &axum::Router,
    token: &str,
    event: &str,
) -> Result<(u16, Option<String>, String), Box<dyn std::error::Error>> {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/hooks/claude/{event}"))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"hook_event_name": event, "session_id": "s"}).to_string(),
        ))?;
    let response = internal.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((
        status,
        retry_after,
        String::from_utf8_lossy(&bytes).into_owned(),
    ))
}

/// Two running tasks: one mid-turn, one idle at its prompt.
async fn two_tasks(i: &Instance) -> Result<(TaskId, TaskId), Box<dyn std::error::Error>> {
    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_sandbox(&i.h, "s1", "fixture", alice.as_str()).await?;
    let working = seed_task(
        &i.h,
        "t-working",
        "s1",
        alice.as_str(),
        TaskStatus::Running,
        None,
    )
    .await?;
    let idle = seed_task(
        &i.h,
        "t-idle",
        "s1",
        alice.as_str(),
        TaskStatus::Running,
        None,
    )
    .await?;
    seed_live_attempt_at(&i.h, "t-working", WORKING, RUN_WORKING).await?;
    seed_live_attempt_at(&i.h, "t-idle", IDLE, RUN_IDLE).await?;
    let now = i.h.app.now();
    i.h.store
        .set_activity(&working.id, Some(Activity::Working), now)
        .await?;
    i.h.store
        .set_activity(&idle.id, Some(Activity::Waiting), now)
        .await?;
    Ok((working.id, idle.id))
}

fn advance(h: &Harness, ms: i64) {
    h.clock.0.fetch_add(ms, Ordering::SeqCst);
}

/// Starts the update to NEW and lets the fake helper finish the build.
async fn built(i: &Instance) -> Result<String, Box<dyn std::error::Error>> {
    post(i, "/api/admin/update/check", None, &i.admin).await?;
    let r = post(
        i,
        "/api/admin/update",
        Some(json!({"commit": NEW, "confirm": true})),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    let run = r.body["run"]["id"].as_str().ok_or("run id")?.to_owned();
    let build = i.helper.last().ok_or("no build request")?;
    i.helper.report(&build, HelperState::Done, "built", None);
    let _ = i.h.app.update_round().await;
    Ok(run)
}

/// AC-OPS-6: the build is known; a newer `main` is offered to superadmins only, with a summary
/// of its new commits; "Check now" is cached; an unreadable repository is reported.
#[tokio::test]
async fn a_newer_main_is_offered_to_superadmins_only() -> TestResult {
    let i = instance().await?;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["current"]["commit"], OLD);
    assert_eq!(view["current"]["short"], "1111111");
    assert_eq!(view["helper_installed"], true);
    assert!(view["available"].is_null(), "nothing checked yet");

    let r = post(&i, "/api/admin/update/check", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let available = &r.body["available"];
    assert_eq!(available["commit"], NEW);
    assert_eq!(available["summary"], "An update banner");
    assert_eq!(available["count"], 1, "merges are left out");
    assert_eq!(available["commits"][0]["title"], "An update banner");
    assert_eq!(available["relation"], "behind");
    assert_eq!(
        r.body["check"]["via"], "app",
        "the App's token read the repository"
    );
    assert_eq!(
        i.source.tokens.lock().map_err(|_| "lock")?.clone(),
        vec![Some("inst".to_owned())]
    );
    // A second "Check now" within 30 s reuses the result.
    post(&i, "/api/admin/update/check", None, &i.admin).await?;
    assert_eq!(i.source.tokens.lock().map_err(|_| "lock")?.len(), 1);

    // Members (here an instance administrator) see the build only.
    let member = get(&i, &i.member).await?;
    assert_eq!(member["current"]["commit"], OLD);
    for field in ["available", "check", "run", "helper_installed"] {
        assert!(member[field].is_null(), "{field}: {member}");
    }
    let r = post(&i, "/api/admin/update/check", None, &i.member).await?;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // An unreadable repository is said, and offers nothing.
    if let Ok(mut result) = i.source.result.lock() {
        *result = Err(SourceError::NoAccess);
    }
    advance(&i.h, 31_000);
    let r = post(&i, "/api/admin/update/check", None, &i.admin).await?;
    assert!(r.body["available"].is_null());
    let error = r.body["check"]["error"].as_str().unwrap_or_default();
    assert!(error.contains("YULIUS_UPDATE_TOKEN_FILE"), "{error}");
    Ok(())
}

/// #502 (SPEC-226 UPC-001): a "Check now" sent while another check compares (two tabs opening
/// the instance page) waits for it and reuses its result: one compare for both.
#[tokio::test]
async fn concurrent_checks_make_one_compare() -> TestResult {
    let i = instance().await?;
    let gate = Arc::new(tokio::sync::Notify::new());
    *i.source.gate.lock().map_err(|_| "lock")? = Some(gate.clone());
    let cookie = i.admin.cookie.as_deref().ok_or("cookie")?;
    let ctx =
        i.h.app
            .authenticate_cookie(cookie)
            .await?
            .ok_or("session")?;
    let first = tokio::spawn({
        let app = i.h.app.clone();
        let ctx = ctx.clone();
        async move { app.check_updates_now(&ctx).await }
    });
    tokio::time::timeout(Duration::from_secs(10), i.source.entered.notified()).await?;
    // Nothing in `check_updates_now` awaits before the checks' lock: one poll leaves the second
    // check queued on it, past its own look at the (stale) last check. Should a step before the
    // lock ever await, this poll could stop short and the test would no longer prove the re-check.
    let second = i.h.app.check_updates_now(&ctx);
    tokio::pin!(second);
    assert!(
        futures::poll!(&mut second).is_pending(),
        "waits for the first check"
    );
    i.source.gate.lock().map_err(|_| "lock")?.take();
    gate.notify_one();
    let first = first.await??;
    let second = second.await?;
    assert_eq!(
        i.source.tokens.lock().map_err(|_| "lock")?.len(),
        1,
        "one compare"
    );
    assert_eq!(
        serde_json::to_value(&second)?,
        serde_json::to_value(&first)?
    );
    assert_eq!(serde_json::to_value(&second)?["available"]["commit"], NEW);
    Ok(())
}

/// AC-OPS-7: "Update now" needs the superadmin, the confirmation, the offered head of `main`
/// and an installed helper; one run at a time; tasks keep working while it builds.
#[tokio::test]
async fn update_now_needs_confirmation_the_offered_commit_and_a_helper() -> TestResult {
    let i = instance().await?;
    let start = |commit: &str, confirm: bool| json!({"commit": commit, "confirm": confirm});
    let r = post(&i, "/api/admin/update", Some(start(NEW, true)), &i.admin).await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "not offered before a check");
    post(&i, "/api/admin/update/check", None, &i.admin).await?;
    let r = post(&i, "/api/admin/update", Some(start(NEW, true)), &i.member).await?;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = post(&i, "/api/admin/update", Some(start(NEW, false)), &i.admin).await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = post(
        &i,
        "/api/admin/update",
        Some(start("3333333", true)),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = post(&i, "/api/admin/update", Some(start(MID, true)), &i.admin).await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "only the offered head");
    i.helper.installed.store(false, Ordering::SeqCst);
    let r = post(&i, "/api/admin/update", Some(start(NEW, true)), &i.admin).await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "no helper");
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("helper")
    );
    i.helper.installed.store(true, Ordering::SeqCst);

    let r = post(&i, "/api/admin/update", Some(start(NEW, true)), &i.admin).await?;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    assert_eq!(r.body["phase"], "building");
    assert_eq!(r.body["run"]["state"], "building");
    assert_eq!(r.body["run"]["summary"], "An update banner");
    assert_eq!(r.body["run"]["can_cancel"], true);
    let request = i.helper.last().ok_or("no request")?;
    assert_eq!(request.action, HelperAction::Build);
    assert_eq!(request.commit, NEW);
    assert_eq!(
        request.token.as_deref(),
        Some("inst"),
        "the fetch reads with the App"
    );
    let id = r.body["run"]["id"].as_str().ok_or("id")?;
    assert_eq!(request.id, format!("{id}-build"));
    let r = post(&i, "/api/admin/update", Some(start(NEW, true)), &i.admin).await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "one update at a time");
    let audit = i.h.store.automation_audit(NEW, 10).await?;
    assert!(audit.iter().any(|e| e.action == "update.started"));
    Ok(())
}

async fn support_ctx(
    i: &Instance,
) -> Result<yulius_app::auth::AuthContext, Box<dyn std::error::Error>> {
    let user = i.h.store.superadmin().await?.ok_or("superadmin")?;
    Ok(yulius_app::auth::AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// AC-OPS-8: after the build, every turn boundary is held (`202`, `Retry-After`, no output) and
/// starts queue; the install is requested once every live task is at a pause point.
#[tokio::test]
async fn tasks_pause_at_their_turn_boundary_before_the_install() -> TestResult {
    let i = instance().await?;
    let (working, idle) = two_tasks(&i).await?;
    // While it builds, hooks answer as usual.
    post(&i, "/api/admin/update/check", None, &i.admin).await?;
    post(
        &i,
        "/api/admin/update",
        Some(json!({"commit": NEW, "confirm": true})),
        &i.admin,
    )
    .await?;
    let (code, _, _) = hook(&i.h.internal, IDLE, "Notification").await?;
    assert_eq!(code, 200);
    let build = i.helper.last().ok_or("build")?;
    i.helper.report(&build, HelperState::Done, "built", None);
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.member).await?;
    assert_eq!(view["phase"], "pausing", "everyone sees the pause");

    // The task mid-turn is not at a pause point: the install waits for it.
    for _ in 0..3 {
        let _ = i.h.app.update_round().await;
    }
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "pausing");
    assert_eq!(view["run"]["tasks"]["paused"], 1);
    assert_eq!(view["run"]["tasks"]["total"], 2);
    assert_eq!(view["run"]["tasks"]["waiting"][0]["id"], working.as_str());
    assert_eq!(view["run"]["can_proceed"], true);
    assert_eq!(
        i.helper.last().map(|r| r.action),
        Some(HelperAction::Build),
        "no install yet"
    );

    // Its turn ends: the Stop hook is held with no output for the agent.
    let (code, retry_after, body) = hook(&i.h.internal, WORKING, "Stop").await?;
    assert_eq!(code, 202);
    assert_eq!(retry_after.as_deref(), Some("3"));
    assert!(body.is_empty(), "nothing reaches the agent: {body}");
    // The idle task's next prompt waits too; other hooks answer as usual.
    let (code, retry_after, _) = hook(&i.h.internal, IDLE, "UserPromptSubmit").await?;
    assert_eq!((code, retry_after.as_deref()), (202, Some("3")));
    let (code, _, _) = hook(&i.h.internal, WORKING, "PreToolUse").await?;
    assert_eq!(code, 200);

    // A start meanwhile queues (a new task waits as `queued` for its start).
    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    let later = seed_task(
        &i.h,
        "t-later",
        "s1",
        alice.as_str(),
        TaskStatus::Queued,
        None,
    )
    .await?;
    i.h.app
        .start_run(&later.id, RunStart::Fresh { prompt: None }, None)
        .await?;
    assert_eq!(
        i.h.store.task(&later.id).await?.ok_or("task")?.status,
        TaskStatus::Queued
    );
    i.h.app.drain_queue().await;
    assert_eq!(
        i.h.store.task(&later.id).await?.ok_or("task")?.status,
        TaskStatus::Queued,
        "the queue waits for the update"
    );

    // Every task is at a pause point on two passes: the install is requested.
    let _ = i.h.app.update_round().await;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(install.action, HelperAction::Install);
    assert_eq!(install.commit, NEW);
    assert!(install.token.is_none(), "installing needs no credential");
    assert_eq!(get(&i, &i.member).await?["phase"], "restarting");
    // Task state is untouched: the agents and their tasks never learn of the pause.
    for task in [&working, &idle] {
        assert_eq!(
            i.h.store.task(task).await?.ok_or("task")?.status,
            TaskStatus::Running
        );
    }
    Ok(())
}

/// AC-OPS-8/9: the server that starts after the install still holds every boundary until the
/// helper reports, then releases the tasks one by one and ends the run; a rolled-back install
/// releases them as well and says why.
#[tokio::test]
async fn the_hold_survives_the_restart_and_tasks_resume_staggered() -> TestResult {
    let i = instance().await?;
    let (working, _idle) = two_tasks(&i).await?;
    let run = built(&i).await?;
    hook(&i.h.internal, WORKING, "Stop").await?;
    let _ = i.h.app.update_round().await;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(install.action, HelperAction::Install);
    assert!(
        i.h.app.deploy_permit().is_err(),
        "no deploy starts while it installs (SPEC-342)"
    );
    i.helper
        .report(&install, HelperState::Running, "stopping", None);

    // The new build starts on the same database.
    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(NEW, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    let internal = yulius_server::internal_router(restarted.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5003))));
    let (code, _, _) = hook(&internal, WORKING, "Stop").await?;
    assert_eq!(code, 202, "held from the first call after the restart");
    i.helper
        .report(&install, HelperState::Running, "checking", None);
    let _ = restarted.update_round().await;
    assert_eq!(hook(&internal, WORKING, "Stop").await?.0, 202);
    assert!(
        restarted.deploy_permit().is_err(),
        "a server restarted mid-install closes the gate on its first pass (SPEC-342 DPU-007)"
    );

    // The helper's check passed: the tasks are released one by one.
    i.helper
        .report(&install, HelperState::Done, "succeeded", None);
    let _ = restarted.update_round().await;
    let _ = restarted.update_round().await;
    let (code, _, _) = hook(&internal, WORKING, "Stop").await?;
    assert_eq!(code, 200, "the held turn goes on");
    assert!(restarted.deploy_permit().is_ok(), "deploys start again");
    // (Without subscriptions the pool itself holds a new prompt, with its own Retry-After.)
    let (_, retry_after, _) = hook(&internal, IDLE, "UserPromptSubmit").await?;
    assert_ne!(
        retry_after.as_deref(),
        Some("3"),
        "no longer held for the update"
    );
    let ctx = support_ctx(&i).await?;
    let view = restarted.update_view(&ctx).await?;
    let last = view.run.ok_or("run")?;
    assert_eq!(last.id, run);
    assert_eq!(last.state, "succeeded");
    assert_eq!(last.message.as_deref(), Some("Updated to 3333333."));
    assert!(view.phase.is_none());
    assert_eq!(view.current.commit.as_deref(), Some(NEW));
    assert!(
        i.h.store
            .automation_audit(NEW, 20)
            .await?
            .iter()
            .any(|e| e.action == "update.finished" && e.detail.as_deref() == Some("succeeded"))
    );
    assert_eq!(
        i.h.store.task(&working).await?.ok_or("task")?.status,
        TaskStatus::Running
    );
    Ok(())
}

/// SPEC-203 HST-012: the update succeeded, but the new version's re-check found that sandboxes
/// lost Docker: the outcome says so, with the reason, not only the helper's log.
#[tokio::test]
async fn a_docker_regression_after_an_update_is_in_its_outcome() -> TestResult {
    use yulius_contracts::update::{HostCheckReport, HostLine, HostOutcome};
    let i = instance().await?;
    two_tasks(&i).await?;
    built(&i).await?;
    i.h.app.proceed_update(&support_ctx(&i).await?).await?;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    i.helper
        .report(&install, HelperState::Done, "succeeded", None);
    i.helper.report_host_check(HostCheckReport {
        ok: false,
        lines: vec![HostLine {
            name: "kernel modules".into(),
            outcome: HostOutcome::Failed,
            detail: "cannot load overlay: modprobe: FATAL: Module overlay not found".into(),
        }],
        error: None,
        commit: Some(NEW.into()),
        checked_at: 1,
    });
    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(NEW, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    let _ = restarted.update_round().await;
    let _ = restarted.update_round().await;
    let view = restarted.update_view(&support_ctx(&i).await?).await?;
    let run = view.run.ok_or("run")?;
    assert_eq!(run.state, "succeeded");
    let docker = run.docker.ok_or("no Docker re-check in the outcome")?;
    assert!(!docker.ok);
    assert!(
        docker
            .problems()
            .join(" ")
            .contains("Module overlay not found"),
        "{docker:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_rolled_back_install_releases_the_tasks_and_says_why() -> TestResult {
    let i = instance().await?;
    two_tasks(&i).await?;
    built(&i).await?;
    i.h.app.proceed_update(&support_ctx(&i).await?).await?;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(
        install.action,
        HelperAction::Install,
        "continue without waiting"
    );
    i.helper.report(
        &install,
        HelperState::RolledBack,
        "rolled_back",
        Some("The new server did not become healthy within 120 s."),
    );
    // The previous build runs again.
    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(OLD, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    let internal = yulius_server::internal_router(restarted.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5004))));
    let _ = restarted.update_round().await;
    let _ = restarted.update_round().await;
    assert_eq!(hook(&internal, WORKING, "Stop").await?.0, 200);
    let view = restarted.update_view(&support_ctx(&i).await?).await?;
    let run = view.run.ok_or("run")?;
    assert_eq!(run.state, "rolled_back");
    assert!(
        run.error
            .unwrap_or_default()
            .contains("did not become healthy")
    );
    assert_eq!(run.log_tail, vec!["error: example".to_owned()]);
    Ok(())
}

/// AC-OPS-8: pausing is bounded (the update is cancelled after 30 minutes and nothing stays
/// held), a person can cancel while it builds or pauses, and a helper that never answers fails
/// the update.
#[tokio::test]
async fn pausing_is_bounded_and_updates_can_be_cancelled() -> TestResult {
    let i = instance().await?;
    two_tasks(&i).await?;
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    advance(&i.h, 30 * 60_000 + 1);
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "cancelled");
    assert_eq!(
        view["run"]["error"],
        "1 task(s) were still in a turn after 30 minutes; nothing was installed.",
        "the idle task counts as paused"
    );
    assert!(view["phase"].is_null());
    assert_eq!(
        hook(&i.h.internal, WORKING, "Stop").await?.0,
        200,
        "nothing held"
    );

    // Cancel while building.
    advance(&i.h, 31_000);
    post(&i, "/api/admin/update/check", None, &i.admin).await?;
    let r = post(
        &i,
        "/api/admin/update",
        Some(json!({"commit": NEW, "confirm": true})),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    let cleared = i.helper.cleared.load(Ordering::SeqCst);
    let r = post(&i, "/api/admin/update/cancel", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["run"]["state"], "cancelled");
    assert!(
        i.helper.cleared.load(Ordering::SeqCst) > cleared,
        "the request is removed"
    );
    let r = post(&i, "/api/admin/update/cancel", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // A helper that never acknowledges: the request is written again, then the update fails.
    let r = post(
        &i,
        "/api/admin/update",
        Some(json!({"commit": NEW, "confirm": true})),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    if let Ok(mut status) = i.helper.status.lock() {
        *status = None;
    }
    let before = i.helper.requests.lock().map_err(|_| "lock")?.len();
    advance(&i.h, 21_000);
    let _ = i.h.app.update_round().await;
    assert_eq!(
        i.helper.requests.lock().map_err(|_| "lock")?.len(),
        before + 1,
        "written again"
    );
    advance(&i.h, 70_000);
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "failed");
    assert!(
        view["run"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("did not respond")
    );
    Ok(())
}

/// SPEC-319 (#533): the 30-minute cancel message counts every task still in a turn, not the
/// page's list, which shows at most 20 of them.
#[tokio::test]
async fn the_cancel_message_counts_every_task_still_in_a_turn() -> TestResult {
    let i = instance().await?;
    two_tasks(&i).await?;
    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    for n in 0..20 {
        let id = format!("t-busy-{n:02}");
        let task = seed_task(&i.h, &id, "s1", alice.as_str(), TaskStatus::Running, None).await?;
        seed_live_attempt_at(
            &i.h,
            &id,
            &format!("update-token-busy-{n:02}-0123456789abcdef"),
            &format!("/nonexistent/runs/a-{id}"),
        )
        .await?;
        i.h.store
            .set_activity(&task.id, Some(Activity::Working), i.h.app.now())
            .await?;
    }
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["tasks"]["total"], 22);
    assert_eq!(view["run"]["tasks"]["paused"], 1);
    assert_eq!(
        view["run"]["tasks"]["waiting"]
            .as_array()
            .map_or(0, Vec::len),
        20,
        "the page lists at most 20"
    );
    advance(&i.h, 30 * 60_000 + 1);
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "cancelled");
    assert_eq!(
        view["run"]["error"],
        "21 task(s) were still in a turn after 30 minutes; nothing was installed."
    );
    Ok(())
}

/// AC-OPS-12 (SPEC-160): with a host that freezes, both tasks — one mid-turn — are frozen on the
/// first round after the build, no hook is held, and the install follows on the next round.
#[tokio::test]
async fn every_task_freezes_at_once_and_the_install_follows() -> TestResult {
    let (i, host) = freezing_instance().await?;
    let (working, idle) = two_tasks(&i).await?;
    built(&i).await?;
    assert!(
        host.frozen().is_empty(),
        "nothing is frozen while it builds"
    );
    let _ = i.h.app.update_round().await;
    let mut frozen = host.frozen();
    frozen.sort();
    assert_eq!(frozen, vec![RUN_IDLE.to_owned(), RUN_WORKING.to_owned()]);
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "pausing");
    assert_eq!(view["run"]["tasks"]["paused"], 2);
    assert_eq!(view["run"]["tasks"]["total"], 2);
    assert_eq!(view["run"]["tasks"]["waiting"], json!([]));
    assert_eq!(
        view["run"]["message"],
        "Every task is paused. Restarting next."
    );
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(
        install.action,
        HelperAction::Install,
        "no turn was waited for"
    );
    // Nothing reached the tasks: their states are untouched and nothing was thawed.
    for task in [&working, &idle] {
        assert_eq!(
            i.h.store.task(task).await?.ok_or("task")?.status,
            TaskStatus::Running
        );
    }
    assert!(host.thawed().is_empty());
    Ok(())
}

/// AC-OPS-11/12: a run the host cannot freeze still pauses at its turn boundary, and nothing is
/// frozen while it works (the others would sit frozen for up to the 30-minute bound); once it is
/// held, the others freeze and the install follows.
#[tokio::test]
async fn a_run_that_cannot_be_frozen_is_waited_for_before_any_freeze() -> TestResult {
    let (i, host) = freezing_instance().await?;
    let (working, _idle) = two_tasks(&i).await?;
    host.answer(RUN_WORKING, RunFreeze::Unsupported);
    built(&i).await?;
    for _ in 0..3 {
        let _ = i.h.app.update_round().await;
    }
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "pausing");
    assert_eq!(
        view["run"]["tasks"]["paused"], 1,
        "the idle task is at its boundary"
    );
    assert_eq!(view["run"]["tasks"]["waiting"][0]["id"], working.as_str());
    assert_eq!(view["run"]["message"], "Pausing tasks: 1 of 2 paused.");
    assert_eq!(i.helper.last().map(|r| r.action), Some(HelperAction::Build));
    assert!(host.frozen().is_empty(), "nothing frozen while it works");
    assert!(host.thawed().is_empty(), "not even for a moment");

    // Its turn ends and is held: the others freeze at once, then the install.
    let (code, _, _) = hook(&i.h.internal, WORKING, "Stop").await?;
    assert_eq!(code, 202);
    let _ = i.h.app.update_round().await;
    assert_eq!(host.frozen(), vec![RUN_IDLE.to_owned()]);
    let _ = i.h.app.update_round().await;
    assert_eq!(
        i.helper.last().map(|r| r.action),
        Some(HelperAction::Install)
    );
    Ok(())
}

/// AC-OPS-11: a run the kernel never confirms frozen is thawed after five passes and paused at
/// its turn boundary instead, and the others are thawed while it works.
#[tokio::test]
async fn a_run_that_never_settles_falls_back_to_its_turn_boundary() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    host.answer(RUN_WORKING, RunFreeze::Freezing);
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    assert_eq!(host.frozen().len(), 2);
    let view = get(&i, &i.admin).await?;
    assert_eq!(
        view["run"]["tasks"]["paused"], 1,
        "not confirmed: not paused"
    );
    assert_eq!(
        view["run"]["tasks"]["waiting"],
        json!([]),
        "being frozen is not still in a turn"
    );
    for _ in 0..4 {
        let _ = i.h.app.update_round().await;
    }
    let mut thawed = host.thawed();
    thawed.sort();
    assert_eq!(thawed, vec![RUN_IDLE.to_owned(), RUN_WORKING.to_owned()]);
    assert!(host.frozen().is_empty());
    assert_eq!(i.helper.last().map(|r| r.action), Some(HelperAction::Build));

    assert_eq!(hook(&i.h.internal, WORKING, "Stop").await?.0, 202);
    let _ = i.h.app.update_round().await;
    assert_eq!(
        host.frozen(),
        vec![RUN_IDLE.to_owned()],
        "refused runs are not asked again"
    );
    let _ = i.h.app.update_round().await;
    assert_eq!(
        i.helper.last().map(|r| r.action),
        Some(HelperAction::Install)
    );
    Ok(())
}

/// SPEC-319 (#533): a run still being frozen at the 30-minute bound is not paused: the cancel
/// message counts it, though the page does not list it as still in a turn.
#[tokio::test]
async fn the_cancel_message_counts_a_run_still_being_frozen() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    host.answer(RUN_WORKING, RunFreeze::Freezing);
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    advance(&i.h, 30 * 60_000 + 1);
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "cancelled");
    assert_eq!(
        view["run"]["error"],
        "1 task(s) were still in a turn after 30 minutes; nothing was installed."
    );
    Ok(())
}

/// AC-OPS-13 (review): a cancel that lands while a freeze pass runs never leaves a run frozen.
#[tokio::test]
async fn a_cancel_during_a_freeze_pass_leaves_nothing_frozen() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    built(&i).await?;
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    *host.entered.lock().map_err(|_| "lock")? = Some(entered_tx);
    *host.gate.lock().map_err(|_| "lock")? = Some(gate);
    let app = i.h.app.clone();
    let round = tokio::spawn(async move { app.update_round().await });
    tokio::task::spawn_blocking(move || entered.recv_timeout(std::time::Duration::from_secs(30)))
        .await??;
    // The first freeze is under way: the person cancels.
    let ctx = support_ctx(&i).await?;
    let app = i.h.app.clone();
    let mut cancel = tokio::spawn(async move { app.cancel_update(&ctx).await.map(|_| ()) });
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let view = get(&i, &i.admin).await?;
            if view["run"]["state"] == "cancelled" {
                return Ok::<_, Box<dyn std::error::Error>>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    // Without serialization the cancel's thaw would finish here, before the freeze is recorded.
    let early = tokio::time::timeout(std::time::Duration::from_millis(200), &mut cancel).await;
    release.send(()).map_err(|_| "release")?;
    round.await?;
    match early {
        Ok(done) => done??,
        Err(_) => cancel.await??,
    }
    // Thawed by the cancel itself, before any later round's backstop.
    assert!(host.frozen().is_empty(), "left frozen: {:?}", host.frozen());
    // The overtaken pass closed the deploy gate after the cancel opened it; the next round, with
    // no update open, opens it again (SPEC-342 DPU-007, review round 2).
    assert!(
        i.h.app.deploy_permit().is_err(),
        "the overtaken pass closed it"
    );
    let _ = i.h.app.update_round().await;
    assert!(i.h.app.deploy_permit().is_ok(), "deploys start again");
    Ok(())
}

/// AC-OPS-13: after the restart the runs thaw one by one as their tasks are released, and the
/// rest is swept; a server that starts with no update thaws what a crash left frozen, one that
/// starts while the update installs keeps the runs frozen.
#[tokio::test]
async fn frozen_tasks_thaw_one_by_one_after_the_restart() -> TestResult {
    let (i, host) = freezing_instance().await?;
    let (working, _idle) = two_tasks(&i).await?;
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(install.action, HelperAction::Install);
    i.helper
        .report(&install, HelperState::Running, "stopping", None);

    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(NEW, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    assert_eq!(host.sweeps.load(Ordering::SeqCst), 0, "still installing");
    assert_eq!(host.frozen().len(), 2);
    let _ = restarted.update_round().await;
    assert!(host.thawed().is_empty(), "frozen until the helper reports");
    assert!(
        restarted.held_for_update(&working),
        "paused across the restart"
    );

    i.helper
        .report(&install, HelperState::Done, "succeeded", None);
    let _ = restarted.update_round().await;
    let _ = restarted.update_round().await;
    let mut thawed = host.thawed();
    thawed.sort();
    assert_eq!(thawed, vec![RUN_IDLE.to_owned(), RUN_WORKING.to_owned()]);
    assert!(host.frozen().is_empty());
    assert!(host.sweeps.load(Ordering::SeqCst) >= 1, "the rest is swept");
    assert!(!restarted.held_for_update(&working), "released");
    let view = restarted.update_view(&support_ctx(&i).await?).await?;
    assert_eq!(view.run.ok_or("run")?.state, "succeeded");

    // A later start with no update open sweeps what a crash would have left frozen.
    let sweeps = host.sweeps.load(Ordering::SeqCst);
    let again = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    again.set_updates(services(NEW, &i.source, &i.helper));
    again.restore_update_hold().await;
    assert_eq!(host.sweeps.load(Ordering::SeqCst), sweeps + 1);
    Ok(())
}

/// AC-OPS-13: a cancel while pausing thaws every frozen run at once; so does a rolled-back
/// install.
#[tokio::test]
async fn a_cancel_or_a_rollback_thaws_every_frozen_task() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    // One run is not confirmed frozen yet, so the update stays pausing.
    host.answer(RUN_WORKING, RunFreeze::Freezing);
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    assert_eq!(host.frozen().len(), 2);
    let r = post(&i, "/api/admin/update/cancel", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(host.frozen().is_empty(), "thawed at once");
    let mut thawed = host.thawed();
    thawed.sort();
    assert_eq!(thawed, vec![RUN_IDLE.to_owned(), RUN_WORKING.to_owned()]);

    // A rolled-back install resumes and thaws too.
    host.answer(RUN_WORKING, RunFreeze::Frozen);
    advance(&i.h, 31_000);
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(install.action, HelperAction::Install);
    assert_eq!(host.frozen().len(), 2);
    i.helper.report(
        &install,
        HelperState::RolledBack,
        "rolled_back",
        Some("The new server did not become healthy within 120 s."),
    );
    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(OLD, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    let thawed_before = host.thawed().len();
    let _ = restarted.update_round().await;
    let _ = restarted.update_round().await;
    assert!(host.frozen().is_empty());
    let mut thawed = host.thawed().split_off(thawed_before);
    thawed.sort();
    assert_eq!(
        thawed,
        vec![RUN_IDLE.to_owned(), RUN_WORKING.to_owned()],
        "each run thawed by the resume itself"
    );
    let view = restarted.update_view(&support_ctx(&i).await?).await?;
    assert_eq!(view.run.ok_or("run")?.state, "rolled_back");
    Ok(())
}

/// Review round 2: "Continue without waiting" freezes every run that can be frozen, even while a
/// run that cannot be frozen is still in its turn; a server restarted mid-drain sweeps the runs
/// its predecessor froze while such a run works.
#[tokio::test]
async fn continuing_without_waiting_still_freezes_what_it_can() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    host.answer(RUN_WORKING, RunFreeze::Unsupported);
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    assert!(host.frozen().is_empty());

    // A server restarted mid-drain does not know what its predecessor froze: it sweeps.
    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(OLD, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    let sweeps = host.sweeps.load(Ordering::SeqCst);
    let _ = restarted.update_round().await;
    assert!(host.sweeps.load(Ordering::SeqCst) > sweeps);

    restarted.proceed_update(&support_ctx(&i).await?).await?;
    let _ = restarted.update_round().await;
    assert_eq!(host.frozen(), vec![RUN_IDLE.to_owned()]);
    assert_eq!(
        i.helper.last().map(|r| r.action),
        Some(HelperAction::Install)
    );
    Ok(())
}

/// The pause of `task` in its summary, as JSON (null without one).
async fn pause_of(i: &Instance, task: &TaskId) -> Result<Value, Box<dyn std::error::Error>> {
    let record = i.h.store.task(task).await?.ok_or("task")?;
    let summary = i.h.app.task_summary(&record).await?;
    Ok(serde_json::to_value(summary)?["pause"].clone())
}

/// The pauses the `task.updated` events received so far said, by task id.
fn published_pauses(
    rx: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
) -> Vec<(String, Value)> {
    let mut seen = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let yulius_contracts::events::UiEvent::TaskUpdated { task } = envelope.event {
            let pause = serde_json::to_value(&task)
                .map(|v| v["pause"].clone())
                .unwrap_or_default();
            seen.push((task.id.clone(), pause));
        }
    }
    seen
}

/// AC-PAU-1 (SPEC-191 PAU-001/002): an update's pause is in the summary of every live task, as
/// `pending` until its run is frozen, and the lists hear about it as it goes on and off.
#[tokio::test]
async fn an_update_s_pause_is_in_the_task_summary_and_published() -> TestResult {
    let (i, _host) = freezing_instance().await?;
    let (working, idle) = two_tasks(&i).await?;
    assert_eq!(pause_of(&i, &working).await?, Value::Null);
    let mut rx = i.h.app.events.subscribe();

    built(&i).await?;
    assert_eq!(
        pause_of(&i, &working).await?,
        json!({"by": "update", "user": null, "since": null, "pending": true}),
        "the hold is on, the run not frozen yet"
    );
    let published = published_pauses(&mut rx);
    for task in [&working, &idle] {
        assert!(
            published
                .iter()
                .any(|(id, pause)| id == task.as_str() && pause["by"] == "update"),
            "{task} was published paused: {published:?}"
        );
    }

    let _ = i.h.app.update_round().await;
    assert_eq!(pause_of(&i, &working).await?["pending"], false, "frozen");
    assert!(
        published_pauses(&mut rx)
            .iter()
            .any(|(id, pause)| id == working.as_str() && pause["pending"] == false),
        "the freeze was published"
    );

    let r = post(&i, "/api/admin/update/cancel", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(pause_of(&i, &working).await?, Value::Null, "released");
    let published = published_pauses(&mut rx);
    for task in [&working, &idle] {
        assert!(
            published
                .iter()
                .any(|(id, pause)| id == task.as_str() && pause.is_null()),
            "{task} was published unpaused: {published:?}"
        );
    }
    Ok(())
}

/// AC-PER-2 (SPEC-191 PER-002/003): a task a person paused stays frozen through an update: the
/// install, the restart, the release one by one and every sweep leave its run frozen, while the
/// other task thaws; the restarted server freezes it again.
#[tokio::test]
async fn an_update_never_wakes_a_task_a_person_paused() -> TestResult {
    let (i, host) = freezing_instance().await?;
    let (working, _idle) = two_tasks(&i).await?;
    i.h.github
        .set_perm("alice", "acme/fixture", support::perms("write"));
    let r = post(&i, "/api/tasks/t-working/pause", None, &i.member).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["pause"]["by"], "person");
    assert_eq!(host.frozen(), vec![RUN_WORKING.to_owned()]);

    built(&i).await?;
    let _ = i.h.app.update_round().await;
    let _ = i.h.app.update_round().await;
    let install = i.helper.last().ok_or("install")?;
    assert_eq!(install.action, HelperAction::Install);
    assert_eq!(
        pause_of(&i, &working).await?["by"],
        "person",
        "a person's pause wins"
    );
    i.helper
        .report(&install, HelperState::Running, "stopping", None);

    let restarted = App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_updates(services(NEW, &i.source, &i.helper));
    restarted.restore_update_hold().await;
    let freezes = host.freezes.load(Ordering::SeqCst);
    restarted.restore_person_pauses().await;
    assert!(
        host.freezes.load(Ordering::SeqCst) > freezes,
        "the restarted server freezes it again"
    );
    i.helper
        .report(&install, HelperState::Done, "succeeded", None);
    let _ = restarted.update_round().await;
    let _ = restarted.update_round().await;
    assert_eq!(
        host.thawed(),
        vec![RUN_IDLE.to_owned()],
        "only the other task thawed"
    );
    assert_eq!(host.frozen(), vec![RUN_WORKING.to_owned()]);
    assert!(host.sweeps.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        host.kept.lock().map(|k| k.clone()).unwrap_or_default(),
        vec![RUN_WORKING.to_owned()],
        "the sweep kept it"
    );

    Ok(())
}

/// AC-PER-4 (SPEC-191 PER-002): resumed while an update holds it, a task stays frozen until the
/// update releases it with the others.
#[tokio::test]
async fn a_task_resumed_during_an_update_waits_for_the_update() -> TestResult {
    let (i, host) = freezing_instance().await?;
    let (working, _idle) = two_tasks(&i).await?;
    i.h.github
        .set_perm("alice", "acme/fixture", support::perms("write"));
    let r = post(&i, "/api/tasks/t-working/pause", None, &i.member).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    built(&i).await?;
    let _ = i.h.app.update_round().await;
    let r = post(&i, "/api/tasks/t-working/resume", None, &i.member).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(host.thawed().is_empty(), "the update keeps it paused");
    assert_eq!(pause_of(&i, &working).await?["by"], "update");
    let r = post(&i, "/api/admin/update/cancel", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(
        host.thawed().contains(&RUN_WORKING.to_owned()),
        "released with the others: {:?}",
        host.thawed()
    );
    Ok(())
}

/// Review N-L2 (SPEC-191 PER-004): stopping a task nobody paused does not wait for an update's
/// freeze pass in progress.
#[tokio::test]
async fn a_stop_does_not_wait_for_a_freeze_pass() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    i.h.github
        .set_perm("alice", "acme/fixture", support::perms("write"));
    built(&i).await?;
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    *host.entered.lock().map_err(|_| "lock")? = Some(entered_tx);
    *host.gate.lock().map_err(|_| "lock")? = Some(gate);
    let app = i.h.app.clone();
    let round = tokio::spawn(async move { app.update_round().await });
    tokio::task::spawn_blocking(move || entered.recv_timeout(std::time::Duration::from_secs(30)))
        .await??;
    // The freeze pass holds the lock: a stop of a task nobody paused goes on.
    let stopped = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        post(&i, "/api/tasks/t-idle/stop", None, &i.member),
    )
    .await;
    release.send(()).map_err(|_| "release")?;
    round.await?;
    let r = stopped.map_err(|_| "the stop waited for the freeze pass")??;
    assert!(r.status.is_success(), "{} {}", r.status, r.body);
    Ok(())
}

const UPDATING: &str = "nothing ran: Yulius is being updated";

/// A project `s1` alice may push to, whose delivery profile has an environment with a command.
async fn deploying_project(i: &Instance) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ports::store::SandboxStore as _;
    i.h.github.set_perm(
        "alice",
        "acme/fixture",
        yulius_domain::permission::GitHubPermissions {
            admin: false,
            maintain: false,
            push: true,
            triage: false,
            pull: true,
        },
    );
    let profile = json!({
        "preset": "product", "integration_branch": "dev", "production_branch": "main",
        "merge": "merge", "promote": "fast_forward", "auto_promote": false,
        "environments": [
            {"name": "dev-vm", "after": "integration", "verify": "command", "command": "true"}
        ]
    });
    i.h.store
        .set_profile(
            &yulius_domain::ids::SandboxId::parse("s1")?,
            &profile.to_string(),
        )
        .await?;
    Ok(())
}

/// A CD run of `s1`'s dev recorded as running (its executor elsewhere).
async fn running_deploy(i: &Instance, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ci::cd::CdRunRecord;
    use yulius_app::ports::store_ext::CdStore as _;
    i.h.store
        .insert_cd_run(&CdRunRecord {
            id: id.into(),
            sandbox_id: yulius_domain::ids::SandboxId::parse("s1")?,
            environment: "dev".into(),
            stage: yulius_domain::cd::CdStage::Dev,
            branch: "dev".into(),
            sha: "0123456789abcdef0123456789abcdef01234567".into(),
            state: yulius_domain::ci::RunState::Running,
            reason: None,
            requested_by: "alice".into(),
            task_id: None,
            deploy: Some("sh scripts/cd/deploy.sh dev".into()),
            verify: None,
            note: None,
            output: String::new(),
            exit_code: None,
            started_at: i.h.app.now(),
            ended_at: None,
        })
        .await?;
    Ok(())
}

/// Ends the seeded CD run `id` as passed.
async fn deploy_ends(i: &Instance, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ci::cd::CdRunEnd;
    use yulius_app::ports::store_ext::CdStore as _;
    i.h.store
        .finish_cd_run(
            id,
            &CdRunEnd {
                state: yulius_domain::ci::RunState::Passed,
                reason: None,
                exit_code: Some(0),
                output: "[yulius] done\n".into(),
                ended_at: i.h.app.now(),
                usage: None,
            },
        )
        .await?;
    Ok(())
}

/// Starts the update to NEW and reports its build done, without driving it further.
async fn build_reported(i: &Instance) -> Result<(), Box<dyn std::error::Error>> {
    post(i, "/api/admin/update/check", None, &i.admin).await?;
    let r = post(
        i,
        "/api/admin/update",
        Some(json!({"commit": NEW, "confirm": true})),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    let build = i.helper.last().ok_or("no build request")?;
    i.helper.report(&build, HelperState::Done, "built", None);
    Ok(())
}

/// SPEC-342 DPU-001/DPU-007: once the build is done no deploy starts until the update ended:
/// a person's CD deploy and an environment command run are refused before anything ran; the
/// cancel opens the gate again.
#[tokio::test]
async fn new_deploys_are_refused_while_the_update_installs() -> TestResult {
    use yulius_app::ports::store_ext::{CdStore as _, DeliveryStore as _};
    let i = instance().await?;
    two_tasks(&i).await?;
    deploying_project(&i).await?;
    let open = post(
        &i,
        "/api/sandboxes/s1/cd/dev/deploy",
        Some(json!({})),
        &i.member,
    )
    .await?;
    assert!(
        !open.body.to_string().contains(UPDATING),
        "no update yet: {}",
        open.body
    );
    built(&i).await?;
    assert_eq!(get(&i, &i.admin).await?["run"]["state"], "pausing");
    let cd = post(
        &i,
        "/api/sandboxes/s1/cd/dev/deploy",
        Some(json!({})),
        &i.member,
    )
    .await?;
    assert_eq!(cd.status, StatusCode::CONFLICT, "{}", cd.body);
    assert!(cd.body.to_string().contains(UPDATING), "{}", cd.body);
    let env = post(
        &i,
        "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
        Some(json!({"confirm": true})),
        &i.member,
    )
    .await?;
    assert_eq!(env.status, StatusCode::CONFLICT, "{}", env.body);
    assert!(env.body.to_string().contains(UPDATING), "{}", env.body);
    assert!(i.h.store.running_cd_runs().await?.is_empty(), "no CD run");
    assert!(
        i.h.store.running_environment_runs().await?.is_empty(),
        "no environment run"
    );
    let r = post(&i, "/api/admin/update/cancel", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let after = post(
        &i,
        "/api/sandboxes/s1/cd/dev/deploy",
        Some(json!({})),
        &i.member,
    )
    .await?;
    assert!(
        !after.body.to_string().contains(UPDATING),
        "the cancel opened the gate: {}",
        after.body
    );
    Ok(())
}

/// SPEC-342 DPU-003/DPU-006: a deploy running when the build is done keeps the update building
/// (nothing frozen, nothing held, no install) and is listed; once it ended, tasks pause and the
/// install follows.
#[tokio::test]
async fn a_running_deploy_is_waited_for_before_tasks_pause() -> TestResult {
    let (i, host) = freezing_instance().await?;
    two_tasks(&i).await?;
    deploying_project(&i).await?;
    running_deploy(&i, "deploying").await?;
    build_reported(&i).await?;
    for _ in 0..3 {
        let _ = i.h.app.update_round().await;
    }
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "building", "{view}");
    assert_eq!(
        view["run"]["message"],
        "Built. Waiting for 1 deploy(s) to finish before pausing tasks; new deploys are refused \
         until the update installed."
    );
    assert_eq!(
        view["run"]["deploys"],
        json!([{"sandbox": "fixture", "environment": "dev", "kind": "cd", "run": "deploying"}])
    );
    assert_eq!(view["run"]["can_proceed"], false);
    assert!(host.frozen().is_empty(), "no task is frozen");
    assert_eq!(
        hook(&i.h.internal, WORKING, "Stop").await?.0,
        200,
        "no turn boundary is held"
    );
    let cd = post(
        &i,
        "/api/sandboxes/s1/cd/dev/deploy",
        Some(json!({})),
        &i.member,
    )
    .await?;
    assert!(cd.body.to_string().contains(UPDATING), "{}", cd.body);
    assert_eq!(
        i.helper.last().ok_or("request")?.action,
        HelperAction::Build,
        "no install request"
    );
    deploy_ends(&i, "deploying").await?;
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "pausing", "{view}");
    assert_eq!(view["run"]["deploys"], json!([]));
    let _ = i.h.app.update_round().await;
    let _ = i.h.app.update_round().await;
    assert_eq!(
        i.helper.last().ok_or("install")?.action,
        HelperAction::Install
    );
    Ok(())
}

/// SPEC-342 DPU-003: a delivery environment run running when the build is done is waited for as a
/// CD run is.
#[tokio::test]
async fn a_running_environment_run_is_waited_for() -> TestResult {
    use yulius_app::delivery::{EnvironmentRunKind, EnvironmentRunRecord, EnvironmentRunState};
    use yulius_app::ports::store_ext::DeliveryStore as _;
    let i = instance().await?;
    two_tasks(&i).await?;
    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    i.h.store
        .insert_environment_run(&EnvironmentRunRecord {
            id: "env-running".into(),
            sandbox_id: yulius_domain::ids::SandboxId::parse("s1")?,
            environment: "dev-vm".into(),
            kind: EnvironmentRunKind::Command,
            requested_by: alice,
            command: Some("true".into()),
            commit: None,
            state: EnvironmentRunState::Running,
            output: String::new(),
            exit_code: None,
            started_at: i.h.app.now(),
            ended_at: None,
        })
        .await?;
    build_reported(&i).await?;
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "building", "{view}");
    assert_eq!(
        view["run"]["deploys"],
        json!([{"sandbox": "fixture", "environment": "dev-vm", "kind": "environment", "run": "env-running"}])
    );
    i.h.store
        .finish_environment_run(
            "env-running",
            EnvironmentRunState::Succeeded,
            "done\n",
            Some(0),
            i.h.app.now(),
        )
        .await?;
    let _ = i.h.app.update_round().await;
    assert_eq!(get(&i, &i.admin).await?["run"]["state"], "pausing");
    Ok(())
}

/// SPEC-342 DPU-004: the longest a deploy can legitimately last (an agent-state wait and its
/// deploy, each up to the 30-minute job timeout; an environment run's 35-minute preparation) and
/// 5 minutes is waited for; a deploy still running past it cancels the update and opens the gate.
#[tokio::test]
async fn a_deploy_past_the_bound_cancels_the_update() -> TestResult {
    let i = instance().await?;
    two_tasks(&i).await?;
    deploying_project(&i).await?;
    running_deploy(&i, "stuck").await?;
    build_reported(&i).await?;
    let _ = i.h.app.update_round().await;
    advance(&i.h, 70 * 60_000);
    let _ = i.h.app.update_round().await;
    assert_eq!(get(&i, &i.admin).await?["run"]["state"], "building");
    advance(&i.h, 1);
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "cancelled", "{view}");
    assert_eq!(
        view["run"]["error"],
        "1 deploy(s) were still running 70 minutes after the build; nothing was installed."
    );
    let cd = post(
        &i,
        "/api/sandboxes/s1/cd/dev/deploy",
        Some(json!({})),
        &i.member,
    )
    .await?;
    assert!(!cd.body.to_string().contains(UPDATING), "{}", cd.body);
    Ok(())
}

/// SPEC-342 DPU-002: a deploy past the gate and not recorded yet (it holds its permit) keeps the
/// update waiting; once it is gone, tasks pause.
#[tokio::test]
async fn a_starting_deploy_is_waited_for() -> TestResult {
    let i = instance().await?;
    two_tasks(&i).await?;
    let permit = i.h.app.deploy_permit()?;
    build_reported(&i).await?;
    let _ = i.h.app.update_round().await;
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "building", "{view}");
    assert_eq!(
        view["run"]["message"],
        "Built. Waiting for 1 deploy(s) to finish before pausing tasks; new deploys are refused \
         until the update installed."
    );
    assert_eq!(view["run"]["deploys"], json!([]), "nothing recorded yet");
    drop(permit);
    let _ = i.h.app.update_round().await;
    assert_eq!(get(&i, &i.admin).await?["run"]["state"], "pausing");
    Ok(())
}

/// SPEC-342 DPU-005: a deploy running while the update pauses tasks (a server restarted mid-drain)
/// keeps the install from being requested, even after "Continue without waiting".
#[tokio::test]
async fn a_deploy_running_while_draining_holds_the_install() -> TestResult {
    let i = instance().await?;
    two_tasks(&i).await?;
    built(&i).await?;
    running_deploy(&i, "late").await?;
    let r = post(&i, "/api/admin/update/proceed", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    for _ in 0..3 {
        let _ = i.h.app.update_round().await;
    }
    let view = get(&i, &i.admin).await?;
    assert_eq!(view["run"]["state"], "pausing", "{view}");
    assert_eq!(view["run"]["deploys"][0]["run"], "late");
    assert_eq!(
        view["run"]["message"],
        "Waiting for 1 deploy(s) to finish before restarting."
    );
    assert_eq!(view["run"]["can_proceed"], false, "nothing to skip");
    assert_eq!(
        i.helper.last().ok_or("request")?.action,
        HelperAction::Build
    );
    deploy_ends(&i, "late").await?;
    let _ = i.h.app.update_round().await;
    assert_eq!(
        i.helper.last().ok_or("install")?.action,
        HelperAction::Install
    );
    Ok(())
}
