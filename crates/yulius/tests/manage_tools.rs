//! Managing the project through the Yulius MCP server (SPEC-037): a task's agent creates tasks
//! with `task_create` on its creator's authority, within a per-project bound.

mod support;

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use support::{
    TestResult, add_sandbox, harness_configured, mcp_tool, perms, seed_live_attempt, seed_task,
    tool_text, user_with_token,
};
use yulius_app::model::TaskOrigin;
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;

type Error = Box<dyn std::error::Error>;

const HOUR: i64 = 60 * 60 * 1000;

/// Sandbox `s1` (studio) with alice (write) and bob (read); task `t-agent` of alice running with
/// the task token `tok-agent`. Created tasks stay queued (no live-session capacity).
async fn world() -> Result<support::Harness, Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    user_with_token(&h, "u-alice", "alice", false).await?;
    user_with_token(&h, "u-bob", "bob", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    seed_task(&h, "t-agent", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    Ok(h)
}

async fn task_create(
    h: &support::Harness,
    token: &str,
    args: Value,
) -> Result<(Value, bool), Error> {
    let result = mcp_tool(h, token, "task_create", args).await?;
    Ok((
        tool_text(&result),
        result["isError"].as_bool().unwrap_or(true),
    ))
}

/// AC-MGT-1: an agent creates a task, also chained after its own; the task belongs to the
/// agent's creator, has origin `agent` and is queued like one from New task (MGT-001, MGT-002).
#[tokio::test]
async fn an_agent_creates_tasks_for_its_creator() -> TestResult {
    let h = world().await?;
    let (created, is_error) = task_create(
        &h,
        "tok-agent",
        json!({"prompt": "Align docs/ with the latest changes", "title": "Align docs"}),
    )
    .await?;
    assert!(!is_error, "{created}");
    let id = TaskId::parse(created["task"]["task_id"].as_str().ok_or("task_id")?)?;
    let task = h.store.task(&id).await?.ok_or("task")?;
    assert_eq!(task.title, "Align docs");
    assert_eq!(task.prompt, "Align docs/ with the latest changes");
    assert_eq!(task.created_by, UserId::parse("u-alice")?);
    assert_eq!(task.origin, TaskOrigin::Agent);
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(created["task"]["number"], json!(task.number));
    assert!(
        created["task"]["branch"]
            .as_str()
            .is_some_and(|b| b.starts_with("feat/"))
    );

    // After the agent's own task: chained until it is done.
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let (chained, is_error) = task_create(
        &h,
        "tok-agent",
        json!({"prompt": "Then release notes", "after_task": agent.number}),
    )
    .await?;
    assert!(!is_error, "{chained}");
    assert_eq!(chained["task"]["status"], "chained");

    // A start rule, as on New task.
    let (ruled, is_error) = task_create(
        &h,
        "tok-agent",
        json!({"prompt": "When both are done", "start_rule": format!("#{} done and #{} done", agent.number, task.number)}),
    )
    .await?;
    assert!(!is_error, "{ruled}");
    assert_eq!(ruled["task"]["status"], "chained");
    Ok(())
}

/// Invalid requests are refused and create nothing.
#[tokio::test]
async fn task_create_refuses_invalid_requests() -> TestResult {
    let h = world().await?;
    let before = h.store.tasks(&SandboxId::parse("s1")?, false).await?.len();
    for args in [
        json!({}),
        json!({"prompt": "   "}),
        json!({"prompt": "x", "after_task": 999}),
        json!({"prompt": "x", "mode": "sideways"}),
        json!({"prompt": "x", "after_task": 1, "start_rule": "#1 done"}),
    ] {
        let (text, is_error) = task_create(&h, "tok-agent", args.clone()).await?;
        assert!(is_error, "{args} was accepted: {text}");
        assert!(
            !text.to_string().contains("unknown tool"),
            "{args}: refused for the wrong reason: {text}"
        );
    }
    assert_eq!(
        h.store.tasks(&SandboxId::parse("s1")?, false).await?.len(),
        before
    );
    Ok(())
}

/// MGT-002: an agent whose creator can no longer work in the project creates nothing.
#[tokio::test]
async fn an_agent_whose_creator_lost_access_creates_nothing() -> TestResult {
    let h = world().await?;
    let (text, is_error) = task_create(&h, "tok-agent", json!({"prompt": "Do it"})).await?;
    assert!(!is_error, "{text}");
    let sid = SandboxId::parse("s1")?;
    let before = h.store.tasks(&sid, false).await?.len();
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("read"));
    // Once the cached access is stale, GitHub is asked again.
    h.clock
        .0
        .fetch_add(h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    let (text, is_error) = task_create(&h, "tok-agent", json!({"prompt": "Do it again"})).await?;
    assert!(is_error, "{text}");
    assert_eq!(h.store.tasks(&sid, false).await?.len(), before);
    Ok(())
}

/// MGT-003: at most 20 tool-created tasks per project and rolling hour.
#[tokio::test]
async fn tool_created_tasks_are_bounded_per_hour() -> TestResult {
    let h = world().await?;
    for i in 0..yulius_app::manage::MAX_TOOL_TASKS_PER_HOUR {
        let (text, is_error) =
            task_create(&h, "tok-agent", json!({"prompt": format!("Task {i}")})).await?;
        assert!(!is_error, "task {i}: {text}");
    }
    let (text, is_error) = task_create(&h, "tok-agent", json!({"prompt": "One more"})).await?;
    assert!(is_error, "{text}");
    assert!(
        text.as_str().is_some_and(|t| t.contains("tasks an hour")),
        "{text}"
    );
    // Allowed again once the hour has passed.
    h.clock.0.fetch_add(HOUR + 1, Ordering::SeqCst);
    let (text, is_error) = task_create(&h, "tok-agent", json!({"prompt": "Later"})).await?;
    assert!(!is_error, "{text}");
    Ok(())
}

/// MGT-002: an agent whose creator lost write access creates or changes no automation either.
#[tokio::test]
async fn an_agent_whose_creator_lost_access_changes_no_automation() -> TestResult {
    let h = world().await?;
    let daily = json!({"name": "Align docs", "cron": "0 4 * * *", "prompt": "Align the docs"});
    let created = mcp_tool(&h, "tok-agent", "automation_create", daily.clone()).await?;
    assert_eq!(created["isError"], false, "{created}");
    let id = tool_text(&created)["automation"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("read"));
    h.clock
        .0
        .fetch_add(h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    let refused = mcp_tool(&h, "tok-agent", "automation_create", daily).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    let refused = mcp_tool(
        &h,
        "tok-agent",
        "automation_update",
        json!({"automation_id": id, "enabled": false}),
    )
    .await?;
    assert_eq!(refused["isError"], true, "{refused}");
    Ok(())
}

/// MGT-002: a disabled creator's agent creates nothing.
#[tokio::test]
async fn an_agent_whose_creator_is_disabled_creates_nothing() -> TestResult {
    use yulius_app::ports::store::UserStore;
    let h = world().await?;
    h.store
        .update_user_flags(&UserId::parse("u-alice")?, Some(Some(h.app.now())), None)
        .await?;
    let (text, is_error) = task_create(&h, "tok-agent", json!({"prompt": "Do it"})).await?;
    assert!(is_error, "{text}");
    assert!(text.to_string().contains("disabled"), "{text}");
    Ok(())
}

/// The tools a Cockpit runs without asking are among those it is served, so a rename cannot
/// silently drop one from the allowlist (MGT-004, MGT-006).
#[test]
fn the_cockpits_read_only_tools_are_served_to_it() {
    for tool in yulius_harness::launch::COCKPIT_READ_ONLY_TOOLS {
        assert!(
            yulius_app::mcp::COCKPIT_TOOLS.contains(&tool),
            "`{tool}` is allowed but not served"
        );
    }
}
