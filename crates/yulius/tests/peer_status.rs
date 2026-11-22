//! A task's agent reads the status of its project's other tasks, read-only (SPEC-625, ADR-0460):
//! owner, delivery, claims, waits and screen; never another project's; never more than its owner
//! may view; never a personal task's free text unless it holds the same person's secrets; and no
//! way to steer a task.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness, mcp_tool, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task, seed_task_with_prompt, tool_text,
};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::ports::store::{CoordinationStore as _, TaskStore as _, UserStore as _};
use yulius_app::ports::store_ext::DeliveryStore as _;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

const ALICE: &str = "peer-status-token-alice-0123456789abcdef";
const ALICE_TWO: &str = "peer-status-token-alice-two-0123456789ab";
const BOB: &str = "peer-status-token-bob-0123456789abcdefgh";
const AUTOMATION: &str = "peer-status-token-automation-0123456789a";
const BOB_WEB: &str = "peer-status-token-bob-web-0123456789abcd";

struct World {
    h: support::Harness,
    alice: Creds,
    /// Task numbers by id.
    numbers: std::collections::HashMap<&'static str, u32>,
}

impl World {
    fn number(&self, id: &str) -> u32 {
        self.numbers.get(id).copied().unwrap_or(0)
    }
}

/// Projects s1 (`acme/app`) and s2 (`acme/web`), written by alice and bob. Live tasks in s1:
/// alice's t1 and t6, bob's t2, an automation's t3 (alice created it); in s2 bob's t4.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    seed_sandbox(&h, "s2", "web", "u-alice").await?;
    for repo in ["acme/app", "acme/web"] {
        h.github.set_perm("alice", repo, perms("write"));
        h.github.set_perm("bob", repo, perms("write"));
    }
    let mut numbers = std::collections::HashMap::new();
    for (id, sandbox, owner, token) in [
        ("t1", "s1", "u-alice", ALICE),
        ("t2", "s1", "u-bob", BOB),
        ("t6", "s1", "u-alice", ALICE_TWO),
        ("t4", "s2", "u-bob", BOB_WEB),
    ] {
        // Alice's t1 asks for something only she should read when it is personal (AC-4).
        let prompt = if id == "t1" {
            "PRIVATE prompt"
        } else {
            "do things"
        };
        let task = seed_task_with_prompt(&h, id, sandbox, owner, TaskStatus::Running, None, prompt)
            .await?;
        seed_live_attempt(&h, id, token).await?;
        numbers.insert(id, task.number);
    }
    let mut automated = seed_task(&h, "t0", "s1", "u-alice", TaskStatus::Running, None).await?;
    automated.id = TaskId::parse("t3")?;
    automated.origin = yulius_app::model::TaskOrigin::Automation;
    let automated = h.store.insert_task(&automated).await?;
    numbers.insert("t3", automated.number);
    seed_live_attempt(&h, "t3", AUTOMATION).await?;
    Ok(World { h, alice, numbers })
}

/// A tool's record (or the text of an error) and whether it is an error.
async fn tool(
    w: &World,
    token: &str,
    name: &str,
    args: Value,
) -> Result<(Value, bool), Box<dyn std::error::Error>> {
    let result = mcp_tool(&w.h, token, name, args).await?;
    let error = result["isError"].as_bool().unwrap_or(false);
    Ok((tool_text(&result), error))
}

/// What an agent reads: the tool's text, without the record.
async fn agent_text(
    w: &World,
    token: &str,
    name: &str,
    args: Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let r = call(
        &w.h.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args}})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    Ok(r.body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("text")?
        .to_owned())
}

/// The task's plan with a noted step, a claim with a note, a question to its person and a line
/// on its screen, each carrying `mark`.
async fn busy(w: &World, id: &str, token: &str, mark: &str) -> TestResult {
    let (_, error) = tool(
        w,
        token,
        "steps_set",
        json!({"steps": [format!("{mark} step one"), "Step two"]}),
    )
    .await?;
    assert!(!error);
    let (_, error) = tool(
        w,
        token,
        "step_update",
        json!({"ordinal": 1, "status": "in_progress", "note": format!("{mark} note")}),
    )
    .await?;
    assert!(!error);
    let (claimed, error) = tool(
        w,
        token,
        "scope_claim",
        json!({"pattern": "docs/x.md", "note": format!("{mark} claim")}),
    )
    .await?;
    assert!(!error, "{claimed}");
    let (asked, error) = tool(
        w,
        token,
        "needs_you",
        json!({"action": format!("{mark} ask"), "happened": format!("{mark} happened")}),
    )
    .await?;
    assert!(!error, "{asked}");
    w.h.app
        .runtime(&TaskId::parse(id)?)
        .hub
        .output(1, format!("{mark} screen line\r\n").as_bytes());
    Ok(())
}

/// SPEC-625 AC-1 (PSV-001): alice's task reads bob's task of the same project: its owner, its
/// pull request and stage, its claims, what it waits for and its screen, in its record and in
/// the text its agent reads.
#[tokio::test]
async fn a_task_reads_a_neighbours_owner_delivery_claims_waits_and_screen() -> TestResult {
    let w = world().await?;
    busy(&w, "t2", BOB, "BOB").await?;
    w.h.store
        .transition_delivery(
            &DeliveryStage::None,
            &DeliveryRecord {
                task_id: TaskId::parse("t2")?,
                stage: DeliveryStage::PrOpen,
                pr_number: Some(41),
                updated_at: Timestamp(1),
            },
        )
        .await?;
    let bob = w.number("t2");
    let (peer, error) = tool(&w, ALICE, "task_get", json!({"number": bob})).await?;
    assert!(!error, "{peer}");
    assert_eq!(peer["owner"], "bob", "{peer}");
    assert_eq!(peer["delivery"]["stage"], "pr_open", "{peer}");
    assert_eq!(peer["delivery"]["pr"], 41, "{peer}");
    assert_eq!(peer["delivery"]["merge_armed"], false, "{peer}");
    assert_eq!(
        peer["claims"],
        json!([{"pattern": "docs/x.md", "note": "BOB claim"}]),
        "{peer}"
    );
    let waits = peer["waits_for"].as_array().ok_or("waits_for")?;
    assert!(
        waits
            .iter()
            .any(|w| w.as_str().is_some_and(|w| w.contains("person"))),
        "it asked its person: {peer}"
    );
    assert_eq!(peer["screen"]["from"], "live", "{peer}");
    assert!(
        peer["screen"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("BOB screen line")),
        "{peer}"
    );
    assert_eq!(peer["steps_list"][0]["note"], "BOB note", "{peer}");
    assert!(
        peer.get("withheld").is_none(),
        "nothing is withheld: {peer}"
    );

    let text = agent_text(&w, ALICE, "task_get", json!({"number": bob})).await?;
    for said in [
        "Owner: @bob",
        "PR #41 (pr_open)",
        "Claims: docs/x.md (\"BOB claim\")",
        "Waits for:",
        "Screen (live):",
        "BOB screen line",
    ] {
        assert!(text.contains(said), "`{said}` in:\n{text}");
    }
    Ok(())
}

/// SPEC-625 AC-2 (PSV-002): a task of another project is not found by id, its claims never show
/// among a neighbour's, and no list shows it.
#[tokio::test]
async fn a_task_never_reads_another_projects_tasks() -> TestResult {
    let w = world().await?;
    busy(&w, "t4", BOB_WEB, "WEB").await?;
    busy(&w, "t2", BOB, "BOB").await?;
    let (refused, error) = tool(&w, ALICE, "task_get", json!({"task_id": "t4"})).await?;
    assert!(error, "another project's task: {refused}");
    let (peer, error) = tool(&w, ALICE, "task_get", json!({"task_id": "t2"})).await?;
    assert!(!error, "{peer}");
    assert_eq!(
        peer["claims"],
        json!([{"pattern": "docs/x.md", "note": "BOB claim"}]),
        "only t2's own claim, not t4's in the other project: {peer}"
    );
    let (listed, error) = tool(&w, ALICE, "tasks_list", json!({"scope": "all"})).await?;
    assert!(!error);
    let all = listed.to_string();
    assert!(
        !all.contains("\"t4\"") && !all.contains("WEB"),
        "no task of s2: {listed}"
    );
    Ok(())
}

/// SPEC-625 AC-3 (PSV-003): a task reads only while its owner may view the project: a disabled
/// owner's task, or one whose owner lost read access, is refused.
#[tokio::test]
async fn a_task_reads_only_what_its_owner_may_view() -> TestResult {
    let w = world().await?;
    let alice_task = w.number("t1");
    w.h.store
        .update_user_flags(&UserId::parse("u-bob")?, Some(Some(Timestamp(5))), None)
        .await?;
    for (name, args) in [
        ("tasks_list", json!({})),
        ("task_get", json!({"number": alice_task})),
    ] {
        let (refused, error) = tool(&w, BOB, name, args).await?;
        assert!(error, "{name} for a disabled owner: {refused}");
        assert!(
            refused.as_str().is_some_and(|t| t.contains("disabled")),
            "{name}: {refused}"
        );
    }
    // Alice loses every access to `acme/web`: her task there would read nothing (her task in
    // `acme/app` still reads).
    seed_task(&w.h, "t7", "s2", "u-alice", TaskStatus::Running, None).await?;
    let token = "peer-status-token-alice-web-0123456789ab";
    seed_live_attempt(&w.h, "t7", token).await?;
    w.h.github.set_perm(
        "alice",
        "acme/web",
        yulius_domain::permission::GitHubPermissions {
            admin: false,
            maintain: false,
            push: false,
            triage: false,
            pull: false,
        },
    );
    // Once the cached access is past its staleness bound, GitHub is asked again.
    w.h.clock.0.fetch_add(
        w.h.app.config.permission_stale_ms + 1,
        std::sync::atomic::Ordering::SeqCst,
    );
    let (refused, error) = tool(&w, token, "tasks_list", json!({})).await?;
    assert!(error, "no read access: {refused}");
    let (_, error) = tool(&w, ALICE, "task_get", json!({"number": w.number("t2")})).await?;
    assert!(!error, "alice still reads in acme/app");
    Ok(())
}

/// Every string of `value` and its text, joined: where a mark could hide.
fn shows(value: &Value) -> String {
    value.to_string()
}

/// SPEC-625 AC-4 (PSV-004): with alice's personal secret in the project, bob's task and the
/// automation's task see none of her task's request, plan texts, claim notes, question or
/// screen, nor its transcript; her other task, which holds the same secret, sees them.
#[tokio::test]
async fn a_personal_tasks_free_text_stays_with_tasks_holding_the_same_secrets() -> TestResult {
    let w = world().await?;
    busy(&w, "t1", ALICE, "PRIVATE").await?;
    let added = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"scope": "personal", "name": "MY_TOKEN", "value": "alice-personal-5150a1ce",
            "hosts": ["api.mine.test"], "injection": "bearer"}),
        ),
        &w.alice,
    )
    .await?;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    let number = w.number("t1");

    for reader in [BOB, AUTOMATION] {
        let (peer, error) = tool(&w, reader, "task_get", json!({"number": number})).await?;
        assert!(!error, "{peer}");
        assert!(!shows(&peer).contains("PRIVATE"), "{reader}: {peer}");
        assert!(
            peer["withheld"]
                .as_str()
                .is_some_and(|why| why.contains("personal")),
            "{peer}"
        );
        // Its shape stays.
        assert_eq!(peer["owner"], "alice", "{peer}");
        assert_eq!(peer["branch"], "feat/t1", "{peer}");
        assert_eq!(peer["claims"][0]["pattern"], "docs/x.md", "{peer}");
        assert_eq!(peer["steps"]["total"], 2, "{peer}");
        assert_eq!(peer["shows"], "needs_you", "{peer}");
        let text = agent_text(&w, reader, "task_get", json!({"number": number})).await?;
        assert!(!text.contains("PRIVATE"), "{reader}:\n{text}");
        assert!(text.contains("Withheld:"), "{reader}:\n{text}");

        let (listed, error) = tool(&w, reader, "tasks_list", json!({"scope": "all"})).await?;
        assert!(!error);
        assert!(!shows(&listed).contains("PRIVATE"), "{reader}: {listed}");
        let (needing, _) = tool(&w, reader, "tasks_list", json!({"scope": "needs_you"})).await?;
        assert!(!shows(&needing).contains("PRIVATE"), "{reader}: {needing}");

        let (refused, error) =
            tool(&w, reader, "session_read", json!({"task_number": number})).await?;
        assert!(error, "{reader}: {refused}");
        assert!(
            refused.as_str().is_some_and(|t| t.contains("personal")),
            "{reader}: {refused}"
        );
    }

    // Claims show their paths but not the personal task's note or step (scopes_list,
    // scope_claim's overlap, task_create's planned overlap).
    let (listed, error) = tool(&w, BOB, "scopes_list", json!({})).await?;
    assert!(!error);
    assert!(!shows(&listed).contains("PRIVATE"), "{listed}");
    assert!(shows(&listed).contains("docs/x.md"), "{listed}");
    let (overlap, error) = tool(&w, BOB, "scope_claim", json!({"pattern": "docs/**"})).await?;
    assert!(!error, "{overlap}");
    assert_eq!(
        overlap["conflicts"][0]["their_pattern"], "docs/x.md",
        "{overlap}"
    );
    assert!(!shows(&overlap).contains("PRIVATE"), "{overlap}");
    let (planned, error) = tool(
        &w,
        AUTOMATION,
        "task_create",
        json!({"prompt": "Edit the docs", "paths": ["docs/x.md"]}),
    )
    .await?;
    assert!(!error, "{planned}");
    assert_eq!(planned["created"], false, "{planned}");
    assert!(!shows(&planned).contains("PRIVATE"), "{planned}");
    let (theirs, _) = tool(&w, ALICE_TWO, "scopes_list", json!({})).await?;
    assert!(shows(&theirs).contains("PRIVATE claim"), "{theirs}");

    // Alice's other task holds the same secret: it reads everything.
    let (own, error) = tool(&w, ALICE_TWO, "task_get", json!({"number": number})).await?;
    assert!(!error, "{own}");
    assert!(own.get("withheld").is_none(), "{own}");
    for said in [
        "PRIVATE prompt",
        "PRIVATE note",
        "PRIVATE claim",
        "PRIVATE ask",
        "PRIVATE screen line",
    ] {
        assert!(shows(&own).contains(said), "`{said}`: {own}");
    }
    Ok(())
}

/// SPEC-625 AC-5 (PSV-005): reading a neighbour changes nothing of it, the read says it is
/// read-only, and a task's agent is served no tool that messages or steers another task.
#[tokio::test]
async fn reading_a_neighbour_changes_nothing_and_no_steering_tool_is_served() -> TestResult {
    let w = world().await?;
    busy(&w, "t2", BOB, "BOB").await?;
    let id = TaskId::parse("t2")?;
    let before = w.h.store.task(&id).await?.ok_or("t2")?;
    let steps_before = w.h.store.steps(&id).await?;
    let messages_before = w.h.store.task_messages(&id, 50).await?.len();
    let (peer, error) = tool(&w, ALICE, "task_get", json!({"number": before.number})).await?;
    assert!(!error, "{peer}");
    assert_eq!(peer["owner"], "bob", "the peer view answered: {peer}");
    let after = w.h.store.task(&id).await?.ok_or("t2")?;
    assert_eq!(
        (after.status, after.activity, after.updated_at, &after.owner),
        (
            before.status,
            before.activity,
            before.updated_at,
            &before.owner
        )
    );
    assert_eq!(w.h.store.steps(&id).await?, steps_before);
    assert_eq!(
        w.h.store.task_messages(&id, 50).await?.len(),
        messages_before
    );

    let list = call(
        &w.h.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})),
        &Creds {
            bearer: Some(ALICE.into()),
            ..Creds::default()
        },
    )
    .await?;
    let tools = list.body["result"]["tools"].as_array().ok_or("tools")?;
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for steering in [
        "task_message",
        "sandbox_task_message",
        "task_restart",
        "task_pause",
        "task_unpause",
        "task_start_now",
        "task_edit",
        "task_close",
        "tasks_bulk",
        "task_merge",
        "task_held_prompt",
        "task_question_dismiss",
        "task_switch",
        "task_model",
    ] {
        assert!(!names.contains(&steering), "`{steering}` is not served");
        let (refused, error) = tool(
            &w,
            ALICE,
            steering,
            json!({"number": before.number, "text": "stop that"}),
        )
        .await?;
        assert!(error, "`{steering}` is refused: {refused}");
    }
    let described = tools
        .iter()
        .find(|t| t["name"] == "task_get")
        .and_then(|t| t["description"].as_str())
        .ok_or("task_get")?;
    assert!(described.contains("read-only"), "{described}");
    assert_eq!(
        w.h.store.task_messages(&id, 50).await?.len(),
        messages_before,
        "no refused call wrote to the task"
    );
    Ok(())
}
