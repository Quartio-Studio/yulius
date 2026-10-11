//! Yulius MCP: task awareness, messages and steps between parallel tasks (AC-COM-1/2, AC-STP-1).

mod support;

use std::net::SocketAddr;

use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, TestResult, call, harness};
use yulius_app::model::{
    AttemptRecord, LaunchKind, SandboxRecord, Sealed, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::store::{
    AttemptUpdate, CoordinationStore, SandboxStore, TaskFieldUpdate, TaskStore, UserStore,
};
use yulius_contracts::events::UiEvent;
use yulius_domain::cron::CivilDate;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::spec_format::spec_template;
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
async fn parallel_tasks_coordinate_through_yulius() -> TestResult {
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
    seed_run(&h, "claude2", Harness::ClaudeCode, "tok-other").await?;
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
    for announced in [
        "project_context",
        "scope_claim",
        "scope_release",
        "scopes_list",
        "spec_check",
    ] {
        assert!(names.contains(&announced), "#50: `{announced}` is served");
    }

    // A task sees the parallel task (COM-001).
    let (tasks, err) = tool(
        &internal,
        "tok-claude",
        "tasks_list",
        json!({"scope": "active"}),
    )
    .await?;
    assert!(!err);
    assert_eq!(tasks["tasks"][0]["harness"], "claude");
    assert_eq!(tasks["you"]["harness"], "claude");

    // It messages the other task by number; the other task reads it (COM-002).
    let other_number = tasks["tasks"][0]["number"].as_u64().ok_or("number")?;
    let (sent, err) = tool(&internal, "tok-claude", "message_send", json!({"number": other_number, "kind": "scope", "subject": "split", "body": "I take src/api; you take src/ui?"})).await?;
    assert!(!err, "{sent}");
    let (inbox, _) = tool(&internal, "tok-other", "messages_inbox", json!({})).await?;
    assert_eq!(
        inbox["messages"][0]["body"],
        "I take src/api; you take src/ui?"
    );
    assert_eq!(inbox["messages"][0]["kind"], "scope");
    let (empty, _) = tool(&internal, "tok-other", "messages_inbox", json!({})).await?;
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

    // #46: an answer to the people (a reply to a person's message) is shown in the task's thread
    // and published to the UI; no task receives it.
    let mut events = h.app.events.subscribe();
    let (answer, err) = tool(
        &internal,
        "tok-claude",
        "message_send",
        json!({"to": "people", "body": "The app is up on port 20000"}),
    )
    .await?;
    assert!(!err, "{answer}");
    assert_eq!(answer["to"], "people");
    let thread = h
        .store
        .task_messages(&TaskId::parse("claude1")?, 50)
        .await?;
    assert!(
        thread
            .iter()
            .any(|e| e.message.body == "The app is up on port 20000" && e.message.to_task.is_none()),
        "shown in the task's Messages"
    );
    let published = events.try_recv()?;
    match published.event {
        UiEvent::Message { task_id, message } => {
            assert_eq!(task_id, "claude1");
            assert_eq!(message.body, "The app is up on port 20000");
        }
        other => return Err(format!("unexpected event {other:?}").into()),
    }
    let (others, _) = tool(&internal, "tok-other", "messages_inbox", json!({})).await?;
    assert_eq!(
        others["messages"].as_array().map(Vec::len),
        Some(0),
        "no task receives an answer to people"
    );

    // Scope claims (COM-003, AC-COM-3, #50): overlaps are reported to both tasks.
    let (claimed, err) = tool(
        &internal,
        "tok-claude",
        "scope_claim",
        json!({"patterns": ["src/api/", "docs/x.md", "src/api/"], "note": "API work"}),
    )
    .await?;
    assert!(!err, "{claimed}");
    assert_eq!(claimed["claimed"], json!(["src/api/**", "docs/x.md"]));
    assert_eq!(claimed["conflicts"], json!([]));
    let (overlap, err) = tool(
        &internal,
        "tok-other",
        "scope_claim",
        json!({"pattern": "src/**/*.rs"}),
    )
    .await?;
    assert!(!err, "{overlap}");
    assert_eq!(overlap["conflicts"].as_array().map(Vec::len), Some(1));
    assert_eq!(overlap["conflicts"][0]["pattern"], "src/**/*.rs");
    assert_eq!(overlap["conflicts"][0]["their_pattern"], "src/api/**");
    assert_eq!(overlap["conflicts"][0]["their_note"], "API work");
    let (told, _) = tool(&internal, "tok-claude", "messages_inbox", json!({})).await?;
    let told = told["messages"].as_array().ok_or("messages")?;
    assert!(
        told.iter().any(|m| m["kind"] == "scope"
            && m["body"]
                .as_str()
                .is_some_and(|b| b.contains("`src/**/*.rs` overlaps your claim `src/api/**`"))),
        "the other task is told: {told:?}"
    );
    let (listed, _) = tool(&internal, "tok-other", "scopes_list", json!({})).await?;
    let claims = listed["claims"].as_array().ok_or("claims")?;
    assert_eq!(claims.len(), 3, "a repeated pattern is claimed once");
    assert!(
        claims
            .iter()
            .any(|c| c["pattern"] == "src/**/*.rs" && c["mine"] == true)
    );
    assert_eq!(listed["conflicts"].as_array().map(Vec::len), Some(1));
    for bad in ["/etc/passwd", "../x", "src//a"] {
        let (refused, err) = tool(
            &internal,
            "tok-claude",
            "scope_claim",
            json!({"pattern": bad}),
        )
        .await?;
        assert!(err, "{bad} is refused: {refused}");
    }
    let (released, _) = tool(
        &internal,
        "tok-claude",
        "scope_release",
        json!({"pattern": "src/api/"}),
    )
    .await?;
    assert_eq!(released["released"], 1);
    let (after, _) = tool(&internal, "tok-other", "scopes_list", json!({})).await?;
    assert_eq!(after["conflicts"].as_array().map(Vec::len), Some(0));

    // #73: claiming a pattern the task already holds tells nobody again; a malformed release
    // is refused instead of releasing every claim; claims per task are bounded.
    let (_, err) = tool(
        &internal,
        "tok-claude",
        "scope_claim",
        json!({"pattern": "src/**/*.rs"}),
    )
    .await?;
    assert!(!err);
    let (_, _) = tool(&internal, "tok-other", "messages_inbox", json!({})).await?;
    let (again, err) = tool(
        &internal,
        "tok-claude",
        "scope_claim",
        json!({"pattern": "src/**/*.rs"}),
    )
    .await?;
    assert!(!err, "{again}");
    let (quiet, _) = tool(&internal, "tok-other", "messages_inbox", json!({})).await?;
    assert_eq!(
        quiet["messages"].as_array().map(Vec::len),
        Some(0),
        "a repeated claim does not message the other task again: {quiet}"
    );
    // `patterns` releases exactly those claims; a malformed release is refused, not "all".
    let (partial, err) = tool(
        &internal,
        "tok-claude",
        "scope_release",
        json!({"patterns": ["docs/x.md"]}),
    )
    .await?;
    assert!(!err, "{partial}");
    assert_eq!(partial["released"], 1, "only the named claim: {partial}");
    let (refused, err) = tool(
        &internal,
        "tok-claude",
        "scope_release",
        json!({"pattern": ["src/**/*.rs"]}),
    )
    .await?;
    assert!(err, "a malformed release is refused: {refused}");
    let (still, _) = tool(&internal, "tok-claude", "scopes_list", json!({})).await?;
    assert!(
        still["claims"]
            .as_array()
            .ok_or("claims")?
            .iter()
            .any(|c| c["pattern"] == "src/**/*.rs" && c["mine"] == true),
        "the other claim was kept: {still}"
    );
    let mut capped = false;
    for batch in 0..4 {
        let patterns: Vec<String> = (0..20).map(|i| format!("cap/{batch}/{i}.txt")).collect();
        let (_, err) = tool(
            &internal,
            "tok-claude",
            "scope_claim",
            json!({"patterns": patterns}),
        )
        .await?;
        if err {
            capped = true;
            break;
        }
    }
    assert!(capped, "claims per task are bounded");

    // spec_check runs the Stop gate's validator (DSC-003); project_context rebuilds the briefing.
    // Without a workflow and without a worktree (so without changes) no spec is needed yet
    // (DSC-013: features and fixes need one, research and questions do not).
    let (no_tree, err) = tool(&internal, "tok-claude", "spec_check", json!({})).await?;
    assert!(!err, "{no_tree}");
    assert_eq!(no_tree["spec"]["required"], false, "{no_tree}");
    assert_eq!(no_tree["spec"]["valid"], true);
    let worktree = h._dir.path().join("wt-claude2");
    std::fs::create_dir_all(worktree.join("docs/specs"))?;
    let date = CivilDate::new(2026, 10, 9).ok_or("date")?;
    std::fs::write(
        worktree.join("docs/specs/split.md"),
        spec_template("Split work", &TaskId::parse("claude2")?, date),
    )?;
    h.store
        .update_task(
            &TaskId::parse("claude2")?,
            &TaskFieldUpdate {
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                spec_path: Some("docs/specs/split.md".into()),
                ..TaskFieldUpdate::default()
            },
            Timestamp(10),
        )
        .await?;
    let (checked, err) = tool(&internal, "tok-other", "spec_check", json!({})).await?;
    assert!(!err, "{checked}");
    assert_eq!(checked["spec"]["path"], "docs/specs/split.md");
    // The changed paths cannot be read (no Git repository): a spec is required, to be safe.
    assert_eq!(checked["spec"]["required"], true, "{checked}");
    assert_eq!(checked["spec"]["valid"], false);
    assert!(
        checked["spec"]["problems"].as_array().is_some_and(|p| p
            .iter()
            .any(|x| x == "Section '## Goal' (line 10) is empty.")),
        "{checked}"
    );
    assert!(
        checked["spec"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("does not satisfy the Yulius format")),
        "{checked}"
    );
    assert!(
        checked["docs"]["problems"]
            .as_array()
            .is_some_and(|p| !p.is_empty()),
        "a checkout without project docs is reported: {checked}"
    );
    let (context, err) = tool(&internal, "tok-claude", "project_context", json!({})).await?;
    assert!(!err, "{context}");
    assert!(
        context["context"]
            .as_str()
            .is_some_and(|c| c.contains("studio") && c.contains("scope_claim")),
        "{context}"
    );

    // Steps: set, update, and status preserved on re-plan (STP-001/002); a plan of similar-effort
    // steps whose progress arrives steadily (STP-006..008).
    let (planned, err) = tool(
        &internal,
        "tok-claude",
        "steps_set",
        json!({"steps": ["Write spec", "Implement", "Test"]}),
    )
    .await?;
    assert!(!err, "{planned}");
    assert!(
        planned["nudges"]
            .as_array()
            .is_some_and(|n| n.iter().any(|x| x
                .as_str()
                .is_some_and(|t| t.contains("name a phase, not a slice")))),
        "phase titles are nudged: {planned}"
    );
    let (after, _) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"ordinal": 1, "status": "done", "note": "spec at docs/specs/x.md"}),
    )
    .await?;
    assert_eq!(after["progress"]["done"], 1);
    assert_eq!(after["progress"]["percent"], 33);
    assert_eq!(after["progress"]["current"], "Implement");
    let (replanned, err) = tool(
        &internal,
        "tok-claude",
        "steps_set",
        json!({"steps": [
            "Write spec",
            {"title": "Parse the CSV upload", "effort": 2},
            {"title": "Store rows per tenant", "status": "in_progress"},
            "Show the import summary"
        ]}),
    )
    .await?;
    assert!(!err, "{replanned}");
    assert_eq!(
        replanned["steps"][0]["status"], "done",
        "identical titles keep status"
    );
    assert_eq!(replanned["steps"][1]["effort"], 2);
    assert_eq!(replanned["progress"]["percent"], 20, "1 of 5 effort units");
    // Bounds: duplicates, effort, too many steps and blocked without a note are refused.
    let many: Vec<String> = (0..31).map(|n| format!("Slice {n}")).collect();
    for (args, why) in [
        (json!({"steps": ["A", "a"]}), "appears twice"),
        (json!({"steps": [{"title": "A", "effort": 5}]}), "effort 5"),
        (json!({"steps": many}), "at most 30"),
        (
            json!({"steps": [{"title": "A", "status": "blocked"}]}),
            "needs a note",
        ),
        (
            json!({"steps": [{"title": "Write spec", "status": "pending"}]}),
            "stays finished",
        ),
    ] {
        let (refused, err) = tool(&internal, "tok-claude", "steps_set", args).await?;
        assert!(err, "refused: {refused}");
        assert!(refused.to_string().contains(why), "{why}: {refused}");
    }
    // At most two steps in progress; a finished step stays finished for the agent.
    let (_, err) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"title": "parse the csv upload", "status": "in_progress"}),
    )
    .await?;
    assert!(!err, "title match ignores case and spacing");
    let (third, err) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"ordinal": 4, "status": "in_progress"}),
    )
    .await?;
    assert!(err, "a third open step is refused: {third}");
    assert!(third.to_string().contains("already in progress"), "{third}");
    let (reopen, err) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"ordinal": 1, "status": "in_progress"}),
    )
    .await?;
    assert!(
        err && reopen.to_string().contains("add a follow-up step"),
        "{reopen}"
    );
    let (skip, err) = tool(
        &internal,
        "tok-claude",
        "step_update",
        json!({"ordinal": 4, "status": "skipped"}),
    )
    .await?;
    assert!(err && skip.to_string().contains("needs a note"), "{skip}");
    let (got, err) = tool(&internal, "tok-claude", "steps_get", json!({})).await?;
    assert!(!err);
    assert!(
        got["nudges"].as_array().is_some_and(|n| n.iter().any(|x| x
            .as_str()
            .is_some_and(|t| t.contains("in progress at once")))),
        "two open steps are nudged: {got}"
    );
    assert!(got["steps"][1]["started_at"].is_string(), "{got}");
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
            &AttemptId::parse("a-claude2")?,
            AttemptState::Exited,
            &AttemptUpdate::default(),
        )
        .await?;
    let denied = rpc(&internal, "tok-other", "tools/list", json!({})).await?;
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
