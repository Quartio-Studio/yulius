//! Shared fixtures moved from `parking`.

use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use crate::support::live::first_message::{hooked, screen};
use crate::support::live::pool::ScriptedUsage;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::coordination::ProjectCaller;
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store::TaskStore;
use yulius_app::race::Point;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

type Error = Box<dyn std::error::Error>;

/// A hooked app whose idle sessions park at once (`park_after_ms` 1) under a limit of `slots`.
pub(crate) async fn app(data: &Path, slots: u32, park_after_ms: u64) -> Result<Arc<App>, Error> {
    Ok(app_with_pulls(data, slots, park_after_ms).await?.0)
}

pub(crate) async fn app_with_pulls(
    data: &Path,
    slots: u32,
    park_after_ms: u64,
) -> Result<(Arc<App>, Arc<crate::support::FakeGitHub>), Error> {
    configured_app_with_pulls(data, slots, park_after_ms, |_| {}).await
}

pub(crate) async fn configured_app_with_pulls(
    data: &Path,
    slots: u32,
    park_after_ms: u64,
    configure: impl FnOnce(&mut yulius_app::context::AppConfig),
) -> Result<(Arc<App>, Arc<crate::support::FakeGitHub>), Error> {
    app_with_pulls_and_clock(
        data,
        slots,
        park_after_ms,
        Arc::new(yulius::wiring::clock::SystemClock),
        configure,
    )
    .await
}

pub(crate) async fn app_with_pulls_and_clock(
    data: &Path,
    slots: u32,
    park_after_ms: u64,
    clock: Arc<dyn yulius_app::ports::Clock>,
    configure: impl FnOnce(&mut yulius_app::context::AppConfig),
) -> Result<(Arc<App>, Arc<crate::support::FakeGitHub>), Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let agent = FakeAgentOptions {
        answers_first_message: true,
        // A resume's prompt reaches the agent, as `claude --resume <id> <prompt>` gives it.
        resume_prompts: true,
        ..hooked()
    };
    let github = Arc::new(crate::support::FakeGitHub::default());
    let (app, _) = crate::support::live::Live {
        usage: Arc::new(ScriptedUsage::default()),
        clock,
        github: github.clone(),
        ..crate::support::live::Live::new(Some(&fake), agent)
    }
    .start(data, 0, |config| {
        config.max_live_sessions = slots;
        config.park_after_ms = park_after_ms;
        configure(config);
    })
    .await?;
    Ok((app, github))
}

pub(crate) async fn status_of(app: &App, id: &TaskId) -> (Option<TaskStatus>, Option<Activity>) {
    match TaskStore::task(app.deps.store.as_ref(), id).await {
        Ok(Some(task)) => (Some(task.status), task.activity),
        _ => (None, None),
    }
}

/// How long [`parked`] waits for a park.
const PARK_WAIT: Duration = Duration::from_secs(30);

/// Parks `id`: a pass runs at each look until it parked, so a pass that came in the millisecond
/// its state last changed (quiet 0 of a 1 ms grace) is followed by another.
pub(crate) async fn parked(app: &Arc<App>, id: &TaskId) -> TestResult {
    parked_within(app, id, PARK_WAIT).await
}

/// [`parked`] with its own bound (#1649). The bound covers the whole wait: a pass still running
/// at the deadline is cancelled there. The failure says whether a pass was still running or
/// the last one ended without parking, and the last park stage the task reached.
pub(crate) async fn parked_within(app: &Arc<App>, id: &TaskId, bound: Duration) -> TestResult {
    let key = Path::new(id.as_str());
    let began = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + bound;
    let mut passes = 0_u32;
    // When a pass first decided a park: later passes find the task still Running while its
    // session ends, and overwrite the last stage with where they gave up.
    let mut decided = None;
    let mut outcome = "no pass ran";
    while tokio::time::Instant::now() < deadline {
        let look = async {
            if status_of(app, id).await.0 == Some(TaskStatus::Running) {
                passes += 1;
                if app.park_idle_sessions_now().await > 0 && decided.is_none() {
                    decided = Some(began.elapsed());
                }
            }
            status_of(app, id).await.0 == Some(TaskStatus::Parked)
        };
        match tokio::time::timeout_at(deadline, look).await {
            Ok(true) => return Ok(()),
            Ok(false) if passes == 0 => outcome = "the task was not running, so no pass ran",
            Ok(false) => outcome = "the last pass ended without parking",
            Err(_) => {
                outcome = "a pass was still running at the deadline";
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let last = yulius_app::race::last_reached(key).filter(|(_, at)| *at >= began);
    let begun = decided.map_or_else(String::new, |at: Duration| {
        format!("; a pass began a park {at:.1?} into the wait (its count covers every task)")
    });
    let stage = match last {
        Some((point, at)) => format!(
            "{} ({:.1?} into the wait)",
            park_stage(point),
            at.duration_since(began)
        ),
        None => "none: no pass reached this task's park (idle facts or another task)".into(),
    };
    let (status, activity) = tokio::time::timeout(Duration::from_secs(1), status_of(app, id))
        .await
        .unwrap_or_default();
    Err(format!(
        "timed out waiting for the task to park within {bound:?}: {outcome} after {passes} \
         pass(es){begun}; last stage reached: {stage}; task {status:?}, {activity:?}"
    )
    .into())
}

/// What a park pass does after reaching `point` (#1649).
fn park_stage(point: Point) -> String {
    match point {
        Point::ParkVerifying => {
            "verifying the task's pull requests (GitHub reads, local Git checks)"
        }
        Point::ParkPrsVerified => "probing the session's processes",
        Point::ParkProbed => "taking the launch lock and rechecking the idle facts",
        Point::ParkPrepared => {
            "rechecking delivery, plan, spec and pull requests under the launch lock"
        }
        Point::ParkDecided => "asking the session to end after the park was decided",
        Point::ParkStateRecorded => "restoring the parked screen",
        Point::LiveRowPrNews => {
            "reading the idle facts (a pass that ended here found the task not parkable)"
        }
        other => return format!("{other:?}"),
    }
    .into()
}

/// The task's agent asks its person to choose (`needs_you`).
pub(crate) async fn ask(app: &App, id: &TaskId) -> TestResult {
    let asked = NeedsYouRecord {
        action: "Choose the colour".into(),
        happened: None,
        problem: Some("Two options".into()),
        decision: true,
        link: None,
        at: 1,
        cause: None,
        named: Default::default(),
    };
    app.deps
        .store
        .set_task_needs_you(id, Some(&asked), app.now())
        .await?;
    Ok(())
}

pub(crate) async fn asks(app: &App, id: &TaskId) -> Result<bool, Error> {
    Ok(app.deps.store.task_needs_you(id).await?.is_some())
}

pub(crate) async fn assert_cold_screen(
    data: &Path,
    id: &TaskId,
    caller: &ProjectCaller,
    expected: &str,
) -> TestResult {
    let restart = self::app(data, 8, 1).await?;
    let ctx = AuthContext {
        user: restart
            .deps
            .store
            .user(&caller.user)
            .await?
            .ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    // Read through MCP first, before a page can warm the terminal hub.
    let remote = yulius_app::remote::RemoteCaller {
        ctx: ctx.clone(),
        token_id: "cold-screen".into(),
        scope: yulius_domain::permission::TokenScope::Read,
        login: "admin".into(),
        superagent: None,
    };
    let request = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "task_screen", "arguments": {"project":"acme/pool","task_id":id.as_str()},
            "_meta": {"yulius/structured":true}}
    }))?;
    let result = restart
        .remote_mcp_handle(&remote, request)
        .await
        .and_then(|reply| reply.result)
        .ok_or("task_screen")?;
    assert_eq!(
        crate::support::tool_text(&result)["screen"],
        expected.trim_end()
    );
    let terminal = restart
        .terminal_open(
            &ctx,
            id.as_str(),
            matches!(
                status_of(&restart, id).await.0,
                Some(TaskStatus::Queued | TaskStatus::WaitingCapacity | TaskStatus::Starting)
            )
            .then_some((22, 5)),
        )
        .await?;
    assert_eq!(screen(&restart, id), expected);
    assert_eq!(
        restart.terminal_state(id).await?.parked_screen,
        expected.contains("Yulius parked")
    );
    drop(terminal);
    Ok(())
}
