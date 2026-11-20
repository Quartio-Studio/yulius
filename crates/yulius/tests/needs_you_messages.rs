//! SPEC-578 regression tests: actionable Needs You explanations, progressing/agent-recoverable CI
//! states, deduplicated wait-episode history, remote MCP / HTTP parity, access boundaries, and
//! secret redaction.

use serde_json::json;
use yulius_app::auth::AuthContext;
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::coordination::{Channel, ProjectCaller};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::intake::{HoldRecord, hold_state};
use yulius_app::model::StepRecord;
use yulius_app::ports::github::GhPull;
use yulius_app::ports::store::{CoordinationStore, TaskStore, UserStore};
use yulius_app::ports::store_ext::{CiStore, DeliveryStore, IntakeStore};
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{SandboxId, StepId, TaskId, UserId};
use yulius_domain::intake::PromptClass;
use yulius_domain::notify::{AskedOf, NoticeKind, TaskNotice};
use yulius_domain::resources::Need;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::support::{
    Harness, harness, mcp_tool, perms, seed_github_user, seed_live_attempt, seed_sandbox, seed_task,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TOKEN: &str = "needs-you-msg-token";

async fn auth_ctx(h: &Harness, id: &str) -> Result<AuthContext, Box<dyn std::error::Error>> {
    let user = h
        .store
        .user(&UserId::parse(id)?)
        .await?
        .ok_or("no such user")?;
    Ok(AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

fn remote_caller(sandbox: &str, user: &str) -> Result<ProjectCaller, Box<dyn std::error::Error>> {
    Ok(ProjectCaller {
        sandbox: SandboxId::parse(sandbox)?,
        user: UserId::parse(user)?,
        actor: "remote:alice".into(),
        channel: Channel::Remote,
        detail: json!({}),
        task: None,
    })
}

fn make_step(
    task_id: &TaskId,
    ordinal: u32,
    title: &str,
    status: &str,
) -> Result<StepRecord, Box<dyn std::error::Error>> {
    Ok(StepRecord {
        id: StepId::parse(&format!("st-{ordinal}"))?,
        task_id: task_id.clone(),
        ordinal,
        title: title.to_owned(),
        status: status.to_owned(),
        effort: 1,
        note: None,
        updated_by: Some("agent".into()),
        updated_at: Some(Timestamp(1_000)),
    })
}

/// SPEC-578 (#540 shape): a task that stops or parks with 5 open plan steps immediately surfaces
/// the open count, next open step title, concrete next action, and task link identically across
/// `tasks_list`, `task_get`, and HTTP task list/detail; repeated polls keep a single timestamped
/// wait-history entry, while a later distinct stop records a second entry.
#[tokio::test]
async fn stopped_task_with_five_open_steps_explains_reason_and_deduplicates_history() -> TestResult
{
    let h = harness()?;
    let _alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "yulius", "u-alice").await?;
    h.github.set_perm("alice", "acme/yulius", perms("write"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    let task_id = TaskId::parse("t1")?;
    h.store
        .set_activity(&task_id, Some(Activity::Waiting), Timestamp(1_000))
        .await?;
    let steps = vec![
        make_step(&task_id, 1, "Sync dev branch", "done")?,
        make_step(&task_id, 2, "Verify parked PR state", "in_progress")?,
        make_step(&task_id, 3, "Add parked-task regressions", "pending")?,
        make_step(&task_id, 4, "Update architecture docs", "pending")?,
        make_step(&task_id, 5, "Run independent review", "pending")?,
        make_step(&task_id, 6, "Merge on green CI", "pending")?,
    ];
    h.store
        .replace_steps(&task_id, &steps, "agent", Timestamp(1_000))
        .await?;

    let owner = auth_ctx(&h, "u-alice").await?;
    let caller = remote_caller("s1", "u-alice")?;

    // Repeated polls of remote MCP and HTTP entry points keep exact parity and 1 history row.
    for _ in 0..3 {
        let list = h
            .app
            .tool_tasks_list(&caller, &json!({"scope": "needs_you"}))
            .await?;
        let detail = h.app.tool_task_get(&caller, &json!({"number": 1})).await?;
        let row = &list["tasks"][0];
        assert_eq!(row["shows"], "needs_you");
        assert_eq!(detail["shows"], "needs_you");
        assert_eq!(row["needs_you_notice"], detail["needs_you_notice"]);
        let happened = row["needs_you_notice"]["happened"]
            .as_str()
            .ok_or("happened")?;
        let action = row["needs_you_notice"]["action"].as_str().ok_or("action")?;
        assert!(
            happened.contains("5 open") && happened.contains("Verify parked PR state"),
            "open steps explanation must name count and next open step: {happened}"
        );
        assert!(
            action.contains("continue the remaining plan steps"),
            "must give supported next action: {action}"
        );
        assert_eq!(
            detail["wait_history"].as_array().map(Vec::len),
            Some(1),
            "repeated polls must keep a single wait history row: {detail}"
        );

        let http_detail = h.app.task_detail(&owner, "t1").await?;
        let http_notice = http_detail
            .summary
            .needs_you_notice
            .as_ref()
            .ok_or("http notice")?;
        assert_eq!(http_notice.happened, happened);
        assert_eq!(http_notice.action, action);
        assert_eq!(http_detail.wait_history.len(), 1);
    }

    // Agent resumes working -> wait history episode is marked resolved.
    h.store
        .set_activity(&task_id, Some(Activity::Working), Timestamp(20_000))
        .await?;
    let working_detail = h.app.task_detail(&owner, "t1").await?;
    assert_eq!(working_detail.summary.needs_you_notice, None);
    assert_eq!(working_detail.wait_history.len(), 1);
    assert!(
        working_detail.wait_history[0].resolved_at.is_some(),
        "working state must resolve previous wait episode"
    );

    // Later distinct stop with 3 open steps at a later timestamp creates a second history entry.
    let later_steps = vec![
        make_step(&task_id, 1, "Sync dev branch", "done")?,
        make_step(&task_id, 2, "Verify parked PR state", "done")?,
        make_step(&task_id, 3, "Add parked-task regressions", "done")?,
        make_step(&task_id, 4, "Update architecture docs", "in_progress")?,
        make_step(&task_id, 5, "Run independent review", "pending")?,
        make_step(&task_id, 6, "Merge on green CI", "pending")?,
    ];
    h.store
        .replace_steps(&task_id, &later_steps, "agent", Timestamp(45_000))
        .await?;
    h.store
        .set_activity(&task_id, Some(Activity::Waiting), Timestamp(50_000))
        .await?;
    let second_detail = h.app.task_detail(&owner, "t1").await?;
    let second_notice = second_detail
        .summary
        .needs_you_notice
        .as_ref()
        .ok_or("second notice")?;
    assert!(
        second_notice.happened.contains("3 open")
            && second_notice.happened.contains("Update architecture docs"),
        "{second_notice:?}"
    );
    assert_eq!(
        second_detail.wait_history.len(),
        2,
        "a distinct later stop must record a second history entry: {:?}",
        second_detail.wait_history
    );
    assert!(second_detail.wait_history[0].resolved_at.is_none());
    assert!(second_detail.wait_history[1].resolved_at.is_some());
    Ok(())
}

/// SPEC-578 (#553 shape): a failing PR check on a live session whose failed CI run has not been
/// handed to the agent yet reflects Working (`yulius_wait`); a newer progressing CI run reflects
/// Working (`InReview`); once stopped/parked with a failing run needing human attention, Needs You
/// names the PR, CI run id, failed job, failing test, CI run link, and concrete next action.
#[tokio::test]
async fn failing_pr_check_distinguishes_agent_recoverable_progressing_and_human_action()
-> TestResult {
    let h = harness()?;
    let _alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "yulius", "u-alice").await?;
    h.github.set_perm("alice", "acme/yulius", perms("write"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    let task_id = TaskId::parse("t1")?;
    h.store
        .set_activity(&task_id, Some(Activity::Waiting), Timestamp(1_000))
        .await?;
    let sandbox_id = SandboxId::parse("s1")?;
    let owner = auth_ctx(&h, "u-alice").await?;
    let caller = remote_caller("s1", "u-alice")?;

    // Open PR #1536 linked to t1's branch.
    let branch = h
        .store
        .task(&task_id)
        .await?
        .and_then(|t| t.branch)
        .unwrap_or_else(|| "feat/t1".to_owned());
    h.store
        .transition_delivery(
            &DeliveryStage::None,
            &DeliveryRecord {
                task_id: task_id.clone(),
                stage: DeliveryStage::PrOpen,
                pr_number: Some(1536),
                updated_at: Timestamp(1_000),
            },
        )
        .await?;
    {
        let mut pulls = h.github.pulls.lock().map_err(|_| "pulls")?;
        pulls.push(GhPull {
            number: 1536,
            html_url: "https://github.com/acme/yulius/pull/1536".into(),
            title: "Fix PR check".into(),
            state: "open".into(),
            merged: false,
            base: "dev".into(),
            head: branch,
            head_sha: "deadbeef01".into(),
            mergeable: Some(true),
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    }
    h.github.set_checks(
        "deadbeef01",
        yulius_app::ports::github::ChecksState::Failure,
    );

    // Record a failed Yulius CI run with a single failing test.
    let failed_run = CiRunRecord {
        id: "run_1536_fail".into(),
        sandbox_id: sandbox_id.clone(),
        reference: CiRef::PullRequest(1536),
        sha: "deadbeef01".into(),
        source: GateSource::ProjectConfig,
        state: RunState::Failed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(10_000),
        started_at: Some(Timestamp(10_100)),
        ended_at: Some(Timestamp(12_000)),
    };
    let failed_job = CiJobRecord {
        id: "job_1536_test".into(),
        run_id: failed_run.id.clone(),
        ordinal: 0,
        name: "test".into(),
        key: "test".into(),
        command: "scripts/dev test".into(),
        matrix: None,
        state: RunState::Failed,
        reason: None,
        exit_code: Some(1),
        output: String::new(),
        failures: Some("FAIL [   0.2s] (1/1) browser::handoff_regression\n".into()),
        started_at: Some(Timestamp(10_100)),
        ended_at: Some(Timestamp(12_000)),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    h.store.insert_ci_run(&failed_run, &[failed_job]).await?;
    let _ = h.app.git_overview(&owner, "s1").await?;

    // Case 1: Live running session (runtime exists) with untold failed Yulius CI run ->
    // agent-recoverable (`pr_news_due` keeps task Working, NOT Needs You!).
    let _runtime = h.app.runtime(&task_id);
    let recoverable = h.app.tool_task_get(&caller, &json!({"number": 1})).await?;
    assert_eq!(
        recoverable["shows"], "working",
        "untold agent-recoverable CI failure on live session must show Working: {recoverable}"
    );
    assert!(recoverable.get("needs_you_notice").is_none());

    // Case 2: Progressing newer CI run supersedes stale failing observation -> Working!
    let running_run = CiRunRecord {
        id: "run_1536_running".into(),
        sandbox_id: sandbox_id.clone(),
        reference: CiRef::PullRequest(1536),
        sha: "deadbeef02".into(),
        source: GateSource::ProjectConfig,
        state: RunState::Running,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(20_000),
        started_at: Some(Timestamp(20_100)),
        ended_at: None,
    };
    let running_job = CiJobRecord {
        id: "job_1536_running".into(),
        run_id: running_run.id.clone(),
        ordinal: 0,
        name: "test".into(),
        key: "test".into(),
        command: "scripts/dev test".into(),
        matrix: None,
        state: RunState::Running,
        reason: None,
        exit_code: None,
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(20_100)),
        ended_at: None,
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    h.store.insert_ci_run(&running_run, &[running_job]).await?;
    let progressing = h.app.tool_task_get(&caller, &json!({"number": 1})).await?;
    assert_eq!(
        progressing["shows"], "working",
        "progressing newer CI run must reflect Working: {progressing}"
    );

    // Case 3: Newer run finishes Failed and task is parked (no live agent recovering it) ->
    // Needs You with exact PR #, run id, failed job, failing test, CI run link, and action!
    let final_fail = CiRunRecord {
        id: "run_1536_final".into(),
        sandbox_id: sandbox_id.clone(),
        reference: CiRef::PullRequest(1536),
        sha: "deadbeef02".into(),
        source: GateSource::ProjectConfig,
        state: RunState::Failed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(30_000),
        started_at: Some(Timestamp(30_100)),
        ended_at: Some(Timestamp(32_000)),
    };
    let final_job = CiJobRecord {
        id: "job_1536_final".into(),
        run_id: final_fail.id.clone(),
        ordinal: 0,
        name: "test".into(),
        key: "test".into(),
        command: "scripts/dev test".into(),
        matrix: None,
        state: RunState::Failed,
        reason: None,
        exit_code: Some(1),
        output: String::new(),
        failures: Some("FAIL [   0.2s] (1/1) browser::handoff_regression\n".into()),
        started_at: Some(Timestamp(30_100)),
        ended_at: Some(Timestamp(32_000)),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    h.store
        .insert_ci_run(&final_fail, std::slice::from_ref(&final_job))
        .await?;
    h.store
        .transition_ci_job(
            &final_job.id,
            &[RunState::Failed],
            RunState::Failed,
            &yulius_app::ci::CiJobUpdate {
                exit_code: Some(1),
                failures: final_job.failures.clone(),
                ..yulius_app::ci::CiJobUpdate::default()
            },
        )
        .await?;
    h.store
        .transition_task(
            &task_id,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            Timestamp(35_000),
        )
        .await?;

    let list = h
        .app
        .tool_tasks_list(&caller, &json!({"scope": "needs_you"}))
        .await?;
    let detail = h.app.tool_task_get(&caller, &json!({"number": 1})).await?;
    let row = &list["tasks"][0];
    assert_eq!(row["shows"], "needs_you");
    assert_eq!(row["needs_you_notice"], detail["needs_you_notice"]);
    let notice = &row["needs_you_notice"];
    let happened = notice["happened"].as_str().ok_or("happened")?;
    let action = notice["action"].as_str().ok_or("action")?;
    let link = notice["link"].as_str().ok_or("link")?;
    assert!(
        happened.contains("#1536")
            && happened.contains("run_1536_final")
            && happened.contains("`test`")
            && happened.contains("handoff_regression"),
        "failing PR notice must identify PR, run, job, and failing test: {happened}"
    );
    assert!(
        action.contains("Inspect CI run run_1536_final"),
        "must give concrete CI action: {action}"
    );
    assert!(
        link.ends_with("/s/yulius/ci?run=run_1536_final"),
        "must link directly to the failed CI run: {link}"
    );

    // Notification text for this derived Needs You state carries What happened, To do, and link.
    let chat_notice = TaskNotice {
        kind: NoticeKind::NeedsYou,
        status: TaskStatus::Parked,
        harness: yulius_domain::harness::Harness::ClaudeCode,
        sandbox: "yulius",
        number: 1,
        title: "Fix PR check",
        asked: Some(AskedOf {
            happened: Some(happened),
            action,
            problem: None,
            decision: false,
        }),
        shared: None,
        link,
    };
    let rendered = chat_notice.text();
    assert!(rendered.contains("What happened:") && rendered.contains("handoff_regression"));
    assert!(rendered.contains("To do:") && rendered.contains("Inspect CI run run_1536_final"));
    assert!(rendered.contains("/s/yulius/ci?run=run_1536_final"));
    Ok(())
}

/// SPEC-578 access boundary: a held prompt's typed excerpt stays hidden from another task's agent
/// in both `held_prompt` and `needs_you_notice`, while still giving a clear actionable explanation.
#[tokio::test]
async fn held_prompt_access_boundary_never_leaks_excerpt_to_task_agent() -> TestResult {
    let h = harness()?;
    let _alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "yulius", "u-alice").await?;
    h.github.set_perm("alice", "acme/yulius", perms("write"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Running, None).await?;
    let t1 = TaskId::parse("t1")?;
    h.store
        .set_activity(&t1, Some(Activity::Working), Timestamp(1_000))
        .await?;
    let t2_record = h.store.task(&TaskId::parse("t2")?).await?.ok_or("t2")?;
    let secret_words = "PRIVATE_OWNER_NOTE_DO_NOT_LEAK";
    h.store
        .insert_hold(&HoldRecord {
            id: "hold-1".into(),
            task_id: t1.clone(),
            class: PromptClass::NewScope,
            prompt: secret_words.into(),
            prompt_sha256: "deadbeef".into(),
            percent: 95,
            typed_by: Some(UserId::parse("u-alice")?),
            state: hold_state::PENDING.into(),
            created_at: Timestamp(1_000),
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        })
        .await?;

    let owner_caller = remote_caller("s1", "u-alice")?;
    let agent_caller = ProjectCaller {
        task: Some(t2_record),
        ..remote_caller("s1", "u-alice")?
    };

    let owner_view = h
        .app
        .tool_task_get(&owner_caller, &json!({"number": 1}))
        .await?;
    assert_eq!(owner_view["held_prompt"]["excerpt"], secret_words);
    assert_eq!(owner_view["needs_you_notice"]["decision"], true);

    let agent_view = h
        .app
        .tool_task_get(&agent_caller, &json!({"number": 1}))
        .await?;
    let agent_json = serde_json::to_string(&agent_view)?;
    assert!(
        !agent_json.contains(secret_words),
        "held prompt excerpt must never leak to another task's agent: {agent_json}"
    );
    assert_eq!(agent_view["shows"], "needs_you");
    assert!(
        agent_view["needs_you_notice"]["happened"]
            .as_str()
            .is_some_and(|s| s.contains("held by the scope check")),
        "{agent_view}"
    );
    Ok(())
}

/// SPEC-578 redaction: a credential shape inside `needs_you` is redacted across `needs_you`,
/// `needs_you_notice`, `wait_history`, `tasks_list`, and `task_get`.
#[tokio::test]
async fn secrets_in_needs_you_are_redacted_across_projections_and_history() -> TestResult {
    let h = harness()?;
    let _alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "yulius", "u-alice").await?;
    h.github.set_perm("alice", "acme/yulius", perms("write"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;

    let fake_token = "ghp_1234567890abcdef1234567890abcdef1234";
    let res = mcp_tool(
        &h,
        TOKEN,
        "needs_you",
        json!({
            "happened": format!("Auth failed with {fake_token}"),
            "action": format!("Rotate token {fake_token} in Settings")
        }),
    )
    .await?;
    assert_eq!(res["isError"], false, "{res}");

    let caller = remote_caller("s1", "u-alice")?;
    let owner = auth_ctx(&h, "u-alice").await?;
    let list = h
        .app
        .tool_tasks_list(&caller, &json!({"scope": "needs_you"}))
        .await?;
    let detail = h.app.tool_task_get(&caller, &json!({"number": 1})).await?;
    let http = h.app.task_detail(&owner, "t1").await?;

    for (label, text) in [
        ("tasks_list", serde_json::to_string(&list)?),
        ("task_get", serde_json::to_string(&detail)?),
        ("http_detail", serde_json::to_string(&http)?),
    ] {
        assert!(
            !text.contains(fake_token),
            "credential must be redacted in {label}: {text}"
        );
    }
    assert_eq!(http.wait_history.len(), 1);
    assert!(!http.wait_history[0].happened.contains(fake_token));
    assert!(!http.wait_history[0].action.contains(fake_token));
    Ok(())
}
