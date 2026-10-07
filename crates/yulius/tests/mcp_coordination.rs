//! Yulius MCP: harness-independent task awareness, messages and steps (AC-COM-1/2, AC-STP-1).

mod support;

use std::net::SocketAddr;

use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, TestResult, call, harness};
use yulius_app::model::{
    AttemptRecord, LaunchKind, SandboxRecord, Sealed, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::store::{AttemptUpdate, SandboxStore, TaskStore, UserStore};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

async fn seed_run(
    h: &support::Harness,
    task: &str,
    harness_kind: Harness,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let sid = SandboxId::parse("s1")?;
    let t = h
        .store
        .insert_task(&TaskRecord {
            id: TaskId::parse(task)?,
            sandbox_id: sid,
            number: 0,
            title: format!("task {task}"),
            prompt: "do things".into(),
            harness: harness_kind,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: Some(format!("feat/{task}")),
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: UserId::parse("u1")?,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: yulius_app::model::TaskMode::Prompt,
            harness_proposal: None,
        })
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse(&format!("a-{task}"))?,
            task_id: t.id,
            seq: 1,
            harness: harness_kind,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/x".into(),
            token_hash: h.app.deps.crypto.sha256_hex(token.as_bytes()),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(())
}

async fn rpc(
    router: &axum::Router,
    token: &str,
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let r = call(
        router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    Ok(r.body)
}

async fn tool(
    router: &axum::Router,
    token: &str,
    name: &str,
    args: Value,
) -> Result<(Value, bool), Box<dyn std::error::Error>> {
    let body = rpc(
        router,
        token,
        "tools/call",
        json!({"name": name, "arguments": args}),
    )
    .await?;
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("text")?;
    let is_error = body["result"]["isError"].as_bool().unwrap_or(true);
    Ok((
        serde_json::from_str(text).unwrap_or(Value::String(text.to_owned())),
        is_error,
    ))
}

#[tokio::test]
async fn claude_and_codex_tasks_coordinate_through_yulius() -> TestResult {
    let h = harness()?;
    h.store
        .insert_user(&UserRecord {
            id: UserId::parse("u1")?,
            github_login: Some("alice".into()),
            github_id: None,
            display_name: "Alice".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        })
        .await?;
    h.store
        .insert_sandbox(&SandboxRecord {
            id: SandboxId::parse("s1")?,
            slug: "studio".into(),
            repo_owner: "Quartio-Studio".into(),
            repo_name: "studio".into(),
            github_repo_id: None,
            default_branch: "main".into(),
            profile_json: "{}".into(),
            port_base: 20000,
            port_count: 20,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: UserId::parse("u1")?,
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    seed_run(&h, "claude1", Harness::ClaudeCode, "tok-claude").await?;
    seed_run(&h, "codex1", Harness::Codex, "tok-codex").await?;
    let internal = yulius_server::internal_router(h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));

    let init = rpc(
        &internal,
        "tok-claude",
        "initialize",
        json!({"protocolVersion": "2025-06-18"}),
    )
    .await?;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "yulius");
    let list = rpc(&internal, "tok-claude", "tools/list", json!({})).await?;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"message_send") && names.contains(&"steps_set"));

    // Claude sees the parallel Codex task (COM-001).
    let (tasks, err) = tool(
        &internal,
        "tok-claude",
        "tasks_list",
        json!({"scope": "active"}),
    )
    .await?;
    assert!(!err);
    assert_eq!(tasks["tasks"][0]["harness"], "codex");
    assert_eq!(tasks["you"]["harness"], "claude");

    // Claude messages Codex by number; Codex reads it (COM-002).
    let codex_number = tasks["tasks"][0]["number"].as_u64().ok_or("number")?;
    let (sent, err) = tool(&internal, "tok-claude", "message_send", json!({"number": codex_number, "kind": "scope", "subject": "split", "body": "I take src/api; you take src/ui?"})).await?;
    assert!(!err, "{sent}");
    let (inbox, _) = tool(&internal, "tok-codex", "messages_inbox", json!({})).await?;
    assert_eq!(
        inbox["messages"][0]["body"],
        "I take src/api; you take src/ui?"
    );
    assert_eq!(inbox["messages"][0]["kind"], "scope");
    let (empty, _) = tool(&internal, "tok-codex", "messages_inbox", json!({})).await?;
    assert_eq!(
        empty["messages"].as_array().map(Vec::len),
        Some(0),
        "read messages are not repeated"
    );
    let (selfmsg, err) = tool(
        &internal,
        "tok-claude",
        "message_send",
        json!({"task_id": "claude1", "body": "x"}),
    )
    .await?;
    assert!(err, "cannot message yourself: {selfmsg}");

    // Steps: set, update, and status preserved on re-plan (STP-001/002).
    let (_, err) = tool(
        &internal,
        "tok-claude",
        "steps_set",
        json!({"steps": ["Write spec", "Implement", "Test"]}),
    )
    .await?;
    assert!(!err);
    let (after, _) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"ordinal": 1, "status": "done", "note": "spec at docs/specs/x.md"}),
    )
    .await?;
    assert_eq!(after["progress"]["done"], 1);
    assert_eq!(after["progress"]["current"], "Implement");
    let (replanned, _) = tool(
        &internal,
        "tok-claude",
        "steps_set",
        json!({"steps": ["Write spec", "Implement", "Test", "Open PR"]}),
    )
    .await?;
    assert_eq!(
        replanned["steps"][0]["status"], "done",
        "identical titles keep status"
    );
    let (bad, err) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"ordinal": 2, "status": "finished"}),
    )
    .await?;
    assert!(err, "invalid status refused: {bad}");

    // A run that ended can no longer use its token.
    h.store
        .transition_attempt(
            &AttemptId::parse("a-codex1")?,
            AttemptState::Exited,
            &AttemptUpdate::default(),
        )
        .await?;
    let denied = rpc(&internal, "tok-codex", "tools/list", json!({})).await?;
    assert_eq!(denied["error"]["code"], -32001);
    let none = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        &Creds {
            bearer: Some("tok-claude".into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(none.status, StatusCode::ACCEPTED);
    Ok(())
}
