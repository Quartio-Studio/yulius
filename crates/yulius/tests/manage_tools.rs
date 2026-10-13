//! Managing the project through the Yulius MCP server (SPEC-037): a task's agent creates tasks
//! with `task_create` on its creator's authority, within a per-project bound.

use crate::support;

use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use support::{
    TestResult, add_sandbox, harness_configured, mcp_tool, perms, seed_live_attempt, seed_task,
    tool_text, user_with_token,
};
use yulius_app::model::TaskOrigin;
use yulius_app::ports::store::TaskStore;
use yulius_app::ports::store_ext::AutomationStore;
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
    assert!(
        tool_text(&refused).to_string().contains("write access"),
        "refused for its access: {refused}"
    );
    let refused = mcp_tool(
        &h,
        "tok-agent",
        "automation_update",
        json!({"automation_id": id, "enabled": false}),
    )
    .await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        tool_text(&refused).to_string().contains("write access"),
        "refused for its access: {refused}"
    );
    let stored = h.store.automation(&id).await?.ok_or("automation")?;
    assert!(stored.enabled, "the refused update changed nothing");
    assert_eq!(
        h.store.automations(&SandboxId::parse("s1")?).await?.len(),
        1,
        "the refused create added nothing"
    );
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

/// The audit entries about `target`, newest first, as `(action, actor)`.
async fn audits(h: &support::Harness, target: &str) -> Result<Vec<(String, String)>, Error> {
    Ok(h.store
        .automation_audit(target, 20)
        .await?
        .into_iter()
        .map(|e| (e.action, e.actor))
        .collect())
}

/// AC-MGT-5 (MGT-008, MGT-010): an agent stops, archives and deletes another task of its
/// project as the UI would, each audited with the agent as the actor.
#[tokio::test]
async fn an_agent_stops_archives_and_deletes_another_task() -> TestResult {
    let h = world().await?;
    let (created, is_error) = task_create(&h, "tok-agent", json!({"prompt": "Side work"})).await?;
    assert!(!is_error, "{created}");
    let number = created["task"]["number"].as_u64().ok_or("number")?;
    let id = TaskId::parse(created["task"]["task_id"].as_str().ok_or("task_id")?)?;
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let actor = format!("task:{}", agent.number);

    let stopped = mcp_tool(&h, "tok-agent", "task_stop", json!({"number": number})).await?;
    assert_eq!(stopped["isError"], false, "{stopped}");
    let task = h.store.task(&id).await?.ok_or("task")?;
    assert!(task.status.is_resting(), "{:?}", task.status);
    assert_eq!(tool_text(&stopped)["task"]["number"], json!(number));

    let archived = mcp_tool(
        &h,
        "tok-agent",
        "task_archive",
        json!({"task_id": id.as_str()}),
    )
    .await?;
    assert_eq!(archived["isError"], false, "{archived}");
    assert!(
        h.store
            .task(&id)
            .await?
            .ok_or("task")?
            .archived_at
            .is_some()
    );
    let restored = mcp_tool(
        &h,
        "tok-agent",
        "task_archive",
        json!({"number": number, "archived": false}),
    )
    .await?;
    assert_eq!(restored["isError"], false, "{restored}");
    assert!(
        h.store
            .task(&id)
            .await?
            .ok_or("task")?
            .archived_at
            .is_none()
    );

    let deleted = mcp_tool(&h, "tok-agent", "task_delete", json!({"number": number})).await?;
    assert_eq!(deleted["isError"], false, "{deleted}");
    assert!(h.store.task(&id).await?.is_none(), "deleted");

    let entries = audits(&h, id.as_str()).await?;
    for action in [
        "task.stopped",
        "task.archived",
        "task.unarchived",
        "task.deleted",
    ] {
        assert!(
            entries.iter().any(|(a, by)| a == action && *by == actor),
            "{action} by {actor}: {entries:?}"
        );
    }
    Ok(())
}

/// AC-MGT-6 (MGT-009): a task's agent cannot stop, archive or delete its own task.
#[tokio::test]
async fn an_agent_cannot_stop_archive_or_delete_its_own_task() -> TestResult {
    let h = world().await?;
    for tool in ["task_stop", "task_archive", "task_delete"] {
        let refused = mcp_tool(&h, "tok-agent", tool, json!({"task_id": "t-agent"})).await?;
        assert_eq!(refused["isError"], true, "{tool}: {refused}");
        assert!(
            tool_text(&refused).to_string().contains("itself"),
            "{tool} refused for the wrong reason: {refused}"
        );
    }
    let own = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    assert_eq!(own.status, TaskStatus::Running);
    assert!(own.archived_at.is_none());
    Ok(())
}

/// MGT-008: the UI's rules hold (a live task is neither archived nor deleted before it is
/// stopped), and a task of another project is not found.
#[tokio::test]
async fn task_tools_follow_the_uis_rules() -> TestResult {
    let h = world().await?;
    seed_task(&h, "t-live", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-live", "tok-live").await?;
    for tool in ["task_archive", "task_delete"] {
        let refused = mcp_tool(&h, "tok-agent", tool, json!({"task_id": "t-live"})).await?;
        assert_eq!(refused["isError"], true, "{tool}: {refused}");
        assert!(
            tool_text(&refused).to_string().contains("stop the task"),
            "{tool}: {refused}"
        );
    }
    assert!(h.store.task(&TaskId::parse("t-live")?).await?.is_some());

    add_sandbox(&h, "s2", "other", 20010).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/other", perms("write"));
    seed_task(&h, "t-other", "s2", "u-alice", TaskStatus::Queued, None).await?;
    for tool in ["task_stop", "task_archive", "task_delete"] {
        let refused = mcp_tool(&h, "tok-agent", tool, json!({"task_id": "t-other"})).await?;
        assert_eq!(refused["isError"], true, "{tool}: {refused}");
        assert!(
            tool_text(&refused).to_string().contains("not found"),
            "{tool}: {refused}"
        );
    }
    let other = h
        .store
        .task(&TaskId::parse("t-other")?)
        .await?
        .ok_or("other")?;
    assert_eq!(other.status, TaskStatus::Queued);
    Ok(())
}

/// AC-MGT-7 (MGT-008, MGT-011): once the creator lost write access, the agent stops, deletes
/// and runs nothing.
#[tokio::test]
async fn an_agent_whose_creator_lost_access_stops_and_deletes_nothing() -> TestResult {
    let h = world().await?;
    seed_task(&h, "t-side", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let created = mcp_tool(
        &h,
        "tok-agent",
        "automation_create",
        json!({"name": "Nightly", "cron": "0 4 * * *", "prompt": "Check things"}),
    )
    .await?;
    assert_eq!(created["isError"], false, "{created}");
    let automation = tool_text(&created)["automation"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("read"));
    h.clock
        .0
        .fetch_add(h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    for (tool, args) in [
        ("task_stop", json!({"task_id": "t-side"})),
        ("task_archive", json!({"task_id": "t-side"})),
        ("task_delete", json!({"task_id": "t-side"})),
        ("automation_delete", json!({"automation_id": automation})),
        ("automation_run_now", json!({"automation_id": automation})),
    ] {
        let refused = mcp_tool(&h, "tok-agent", tool, args).await?;
        assert_eq!(refused["isError"], true, "{tool}: {refused}");
        assert!(
            tool_text(&refused).to_string().contains("write access"),
            "{tool} refused for the wrong reason: {refused}"
        );
    }
    let side = h
        .store
        .task(&TaskId::parse("t-side")?)
        .await?
        .ok_or("side")?;
    assert_eq!(side.status, TaskStatus::Queued);
    assert!(h.store.automation(&automation).await?.is_some());
    assert!(h.store.runs(&automation, 10).await?.is_empty(), "no run");
    Ok(())
}

/// MGT-011: an agent runs an automation now and deletes it, as the UI does, audited with the
/// agent; another project's automation is not found.
#[tokio::test]
async fn an_agent_runs_an_automation_now_and_deletes_it() -> TestResult {
    let h = world().await?;
    let created = mcp_tool(
        &h,
        "tok-agent",
        "automation_create",
        json!({"name": "Nightly", "cron": "0 4 * * *", "prompt": "Check things", "enabled": false}),
    )
    .await?;
    let id = tool_text(&created)["automation"]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let actor = format!(
        "task:{}",
        h.store
            .task(&TaskId::parse("t-agent")?)
            .await?
            .ok_or("agent")?
            .number
    );

    let ran = mcp_tool(
        &h,
        "tok-agent",
        "automation_run_now",
        json!({"automation_id": id}),
    )
    .await?;
    assert_eq!(ran["isError"], false, "{ran}");
    assert_eq!(tool_text(&ran)["run"]["state"], "task_created", "{ran}");
    let task_id = TaskId::parse(
        tool_text(&ran)["run"]["task_id"]
            .as_str()
            .ok_or("task_id")?,
    )?;
    let task = h.store.task(&task_id).await?.ok_or("run task")?;
    assert_eq!(task.created_by, UserId::parse("u-alice")?);

    let deleted = mcp_tool(
        &h,
        "tok-agent",
        "automation_delete",
        json!({"automation_id": id}),
    )
    .await?;
    assert_eq!(deleted["isError"], false, "{deleted}");
    assert!(h.store.automation(&id).await?.is_none());
    assert!(h.store.task(&task_id).await?.is_some(), "its tasks stay");

    let entries = audits(&h, &id).await?;
    for action in ["automation.run_now", "automation.deleted"] {
        assert!(
            entries.iter().any(|(a, by)| a == action && *by == actor),
            "{action} by {actor}: {entries:?}"
        );
    }
    let missing = mcp_tool(
        &h,
        "tok-agent",
        "automation_delete",
        json!({"automation_id": id}),
    )
    .await?;
    assert_eq!(missing["isError"], true, "{missing}");
    assert!(
        tool_text(&missing).to_string().contains("not found"),
        "{missing}"
    );
    Ok(())
}

/// MGT-012: none of the tools that stop, delete or run something is pre-allowed in the
/// Cockpit: Claude Code asks the person first.
#[test]
fn the_cockpit_asks_before_every_changing_tool() {
    for tool in [
        "task_stop",
        "task_archive",
        "task_delete",
        "automation_delete",
        "automation_run_now",
        "automation_template_enable",
    ] {
        assert!(
            yulius_app::mcp::COCKPIT_TOOLS.contains(&tool),
            "`{tool}` is not served to the Cockpit"
        );
        assert!(
            !yulius_harness::launch::COCKPIT_READ_ONLY_TOOLS.contains(&tool),
            "`{tool}` runs without asking"
        );
    }
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
