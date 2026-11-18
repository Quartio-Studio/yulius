//! The agent of a task created without one is drawn (SPEC-505 RND-001, ADR-0361): Claude Code or
//! Antigravity, uniformly among those with a usable subscription (Claude Code when neither has
//! one), so the two can be compared on the same mix of work; an agent given, or implied by the
//! model given, is kept. The task says whether its agent was drawn (`harness_choice`). The draw's
//! rolls are queued here (`App::queue_harness_rolls`); the in-memory harness has no session slot,
//! so a start that passes the account gate queues.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, perms, user_with_token,
};
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::store::AccountStore;
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

const CREATE: &str = "/api/sandboxes/s1/tasks";

async fn world() -> Result<(Harness, Creds), Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    Ok((h, alice))
}

/// A usable account `id` of `provider`.
async fn usable(h: &Harness, id: &str, provider: Provider) -> TestResult {
    account(h, id, provider, AccountStatus::Ok, true).await
}

/// An account `id` of `provider` in `status`, signed in when `signed_in`.
async fn account(
    h: &Harness,
    id: &str,
    provider: Provider,
    status: AccountStatus,
    signed_in: bool,
) -> TestResult {
    let credential: &[u8] = match provider {
        Provider::Google => br#"{"kind":"gemini_api_key","api_key":"SYNTHETIC-0123456789abcdef"}"#,
        _ => b"synthetic-subscription-credential",
    };
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider,
            label: id.into(),
            credential: if signed_in {
                Some(h.app.deps.crypto.seal(credential)?)
            } else {
                None
            },
            status,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            email_typed: false,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(())
}

/// Creates a task from `body` and returns its summary's agent and how it was picked.
async fn created(h: &Harness, alice: &Creds, body: Value) -> Result<(String, String), Error> {
    let r = call(&h.router, Method::POST, CREATE, Some(body), alice).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = r.body["id"].as_str().ok_or("id")?;
    let shown = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        alice,
    )
    .await?;
    assert_eq!(
        shown.body["harness_choice"], r.body["harness_choice"],
        "kept with the task: {:?}",
        shown.body
    );
    Ok((
        r.body["harness"].as_str().ok_or("harness")?.to_owned(),
        r.body["harness_choice"]
            .as_str()
            .ok_or("harness_choice")?
            .to_owned(),
    ))
}

fn pair(harness: &str, choice: &str) -> (String, String) {
    (harness.to_owned(), choice.to_owned())
}

/// RND-001: with Claude Code and Antigravity both usable, a task without an agent (or with
/// `auto`) is drawn between the two, each roll picking one; an agent given is kept, and so is the
/// one a model implies.
#[tokio::test]
async fn a_task_without_an_agent_is_drawn_between_claude_code_and_antigravity() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    usable(&h, "a-google", Provider::Google).await?;
    usable(&h, "a-codex", Provider::OpenAi).await?;
    h.app.queue_harness_rolls(&[0, 1, 3]);
    assert_eq!(
        created(&h, &alice, json!({"prompt": "One"})).await?,
        pair("claude", "random")
    );
    assert_eq!(
        created(&h, &alice, json!({"prompt": "Two"})).await?,
        pair("antigravity", "random")
    );
    assert_eq!(
        created(&h, &alice, json!({"prompt": "Three", "harness": "auto"})).await?,
        pair("antigravity", "random")
    );
    for (body, expected) in [
        (
            json!({"prompt": "Four", "harness": "claude"}),
            pair("claude", "chosen"),
        ),
        (
            json!({"prompt": "Five", "harness": "antigravity"}),
            pair("antigravity", "chosen"),
        ),
        // Codex is never drawn, only chosen.
        (
            json!({"prompt": "Six", "harness": "codex"}),
            pair("codex", "chosen"),
        ),
        (
            json!({"prompt": "Seven", "model": "claude-sonnet-5-5"}),
            pair("claude", "chosen"),
        ),
        (
            json!({"prompt": "Eight", "model": "gemini-4-carbon"}),
            pair("antigravity", "chosen"),
        ),
    ] {
        assert_eq!(created(&h, &alice, body.clone()).await?, expected, "{body}");
    }
    Ok(())
}

/// RND-001: the draw is among the agents with a usable subscription: Antigravity alone gets every
/// drawn task, Claude Code alone too, and with neither the task is Claude Code's (whose start is
/// then refused for want of a Claude account, as before).
#[tokio::test]
async fn the_draw_is_among_the_agents_with_a_usable_subscription() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-google", Provider::Google).await?;
    h.app.queue_harness_rolls(&[0, 0]);
    for prompt in ["One", "Two"] {
        assert_eq!(
            created(&h, &alice, json!({"prompt": prompt})).await?,
            pair("antigravity", "random")
        );
    }

    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    h.app.queue_harness_rolls(&[1]);
    assert_eq!(
        created(&h, &alice, json!({"prompt": "One"})).await?,
        pair("claude", "random")
    );

    let (h, alice) = world().await?;
    let refused = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "One"})),
        &alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert!(
        refused
            .body
            .to_string()
            .contains("Connect a Claude account first"),
        "{:?}",
        refused.body
    );
    Ok(())
}

/// RND-001: a task an agent creates with `task_create` is drawn too, unless the agent names one.
#[tokio::test]
async fn a_task_an_agent_creates_is_drawn_unless_it_names_an_agent() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    let (h, _alice) = world().await?;
    support::seed_task(
        &h,
        "t-agent",
        "s1",
        "u-alice",
        yulius_domain::task::TaskStatus::Running,
        None,
    )
    .await?;
    support::seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    usable(&h, "a-google", Provider::Google).await?;
    h.app.queue_harness_rolls(&[1]);
    for (args, title) in [
        (
            json!({"prompt": "Drawn follow-up", "title": "Drawn"}),
            "Drawn",
        ),
        (
            json!({"prompt": "Named follow-up", "title": "Named", "harness": "claude"}),
            "Named",
        ),
    ] {
        let tool = support::mcp_tool(&h, "tok-agent", "task_create", args).await?;
        assert!(!tool.to_string().contains("\"isError\":true"), "{tool}");
        let tasks = h
            .store
            .tasks(&yulius_domain::ids::SandboxId::parse("s1")?, false)
            .await?;
        let task = tasks.iter().find(|t| t.title == title).ok_or(title)?;
        let summary = h.app.task_summary(task).await?;
        let expected = if title == "Drawn" {
            ("antigravity", "random")
        } else {
            ("claude", "chosen")
        };
        assert_eq!(
            (summary.harness.as_str(), summary.harness_choice.as_str()),
            expected,
            "{title}"
        );
    }
    Ok(())
}

/// RND-002: an automation without an agent says `auto` and draws its task's agent for each run
/// (recorded as drawn); one with an agent keeps it.
#[tokio::test]
async fn an_automation_without_an_agent_draws_one_for_each_run() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    usable(&h, "a-google", Provider::Google).await?;
    let mut runs = Vec::new();
    for (name, harness) in [
        ("Drawn one", None),
        ("Drawn two", None),
        ("Named", Some("claude")),
    ] {
        let mut body = json!({"name": name, "cron": "0 9 1 1 *", "utc_offset_minutes": 0, "prompt": "Review yesterday's commits", "enabled": false});
        if let Some(harness) = harness {
            body["harness"] = json!(harness);
        }
        let r = call(
            &h.router,
            Method::POST,
            "/api/sandboxes/s1/automations",
            Some(body),
            &alice,
        )
        .await?;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        assert_eq!(
            r.body["harness"],
            json!(harness.unwrap_or("auto")),
            "{:?}",
            r.body
        );
        runs.push(r.body["id"].as_str().ok_or("id")?.to_owned());
    }
    h.app.queue_harness_rolls(&[1, 0]);
    let mut got = Vec::new();
    for id in &runs {
        let run = call(
            &h.router,
            Method::POST,
            &format!("/api/automations/{id}/run"),
            None,
            &alice,
        )
        .await?;
        assert_eq!(run.body["state"], "task_created", "{:?}", run.body);
        let task = run.body["task_id"].as_str().ok_or("task")?;
        let shown = call(
            &h.router,
            Method::GET,
            &format!("/api/tasks/{task}"),
            None,
            &alice,
        )
        .await?;
        got.push((
            shown.body["harness"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            shown.body["harness_choice"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        ));
    }
    assert_eq!(
        got,
        [
            pair("antigravity", "random"),
            pair("claude", "random"),
            pair("claude", "chosen")
        ]
    );
    Ok(())
}

/// RND-003: the analytics export lists each task's agent and whether it was drawn, as JSON and
/// as a CSV column after `harness`, so the two agents can be compared on drawn tasks.
#[tokio::test]
async fn the_export_says_which_agents_were_drawn() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    usable(&h, "a-google", Provider::Google).await?;
    h.app.queue_harness_rolls(&[1]);
    created(&h, &alice, json!({"prompt": "Drawn", "title": "Drawn"})).await?;
    created(
        &h,
        &alice,
        json!({"prompt": "Named", "title": "Named", "harness": "claude"}),
    )
    .await?;
    let json_rows = call(
        &h.router,
        Method::GET,
        "/api/analytics/tasks?format=json&sandbox=s1",
        None,
        &alice,
    )
    .await?;
    assert_eq!(json_rows.status, StatusCode::OK, "{:?}", json_rows.body);
    let rows = json_rows.body.as_array().ok_or("rows")?;
    let row = |title: &str| {
        rows.iter()
            .find(|r| r["title"] == title)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        (
            row("Drawn")["harness"].clone(),
            row("Drawn")["harness_choice"].clone()
        ),
        (json!("antigravity"), json!("random"))
    );
    assert_eq!(
        (
            row("Named")["harness"].clone(),
            row("Named")["harness_choice"].clone()
        ),
        (json!("claude"), json!("chosen"))
    );
    let columns = yulius_contracts::api::analytics::TASK_ROW_COLUMNS;
    let at = columns
        .iter()
        .position(|c| *c == "harness")
        .ok_or("harness column")?;
    assert_eq!(columns.get(at + 1), Some(&"harness_choice"));
    let csv = yulius_app::analytics::tasks_csv(&serde_json::from_value::<Vec<_>>(
        json_rows.body.clone(),
    )?);
    let drawn = csv
        .lines()
        .find(|l| l.contains(",Drawn,"))
        .ok_or("the drawn row")?;
    assert!(drawn.contains(",antigravity,random,"), "{drawn}");
    Ok(())
}

/// RND-001 (slice E review): an agent whose accounts must all sign in again, or hold no sign-in,
/// is left out of the draw.
#[tokio::test]
async fn an_agent_without_a_usable_account_is_not_drawn() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    account(
        &h,
        "a-google-relogin",
        Provider::Google,
        AccountStatus::Relogin,
        true,
    )
    .await?;
    account(
        &h,
        "a-google-pending",
        Provider::Google,
        AccountStatus::Ok,
        false,
    )
    .await?;
    h.app.queue_harness_rolls(&[1]);
    assert_eq!(
        created(&h, &alice, json!({"prompt": "One"})).await?,
        pair("claude", "random")
    );
    Ok(())
}

/// RND-001 (slice E review): deleting a drawn task removes its draw.
#[tokio::test]
async fn deleting_a_drawn_task_removes_its_draw() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    let r = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "One"})),
        &alice,
    )
    .await?;
    let id = yulius_domain::ids::TaskId::parse(r.body["id"].as_str().ok_or("id")?)?;
    assert!(h.store.harness_drawn(&id).await?);
    assert!(h.store.delete_task(&id).await?);
    assert!(
        !h.store.harness_drawn(&id).await?,
        "the draw went with the task"
    );
    Ok(())
}

/// RND-002 (slice E review): an automation's agent changes to `auto` and back, stored so, and a
/// ready-made automation draws its tasks' agent.
#[tokio::test]
async fn an_automation_changes_to_auto_and_back_and_templates_draw() -> TestResult {
    let (h, alice) = world().await?;
    let body = json!({"name": "Review", "cron": "0 9 1 1 *", "utc_offset_minutes": 0, "prompt": "Review yesterday's commits", "enabled": false, "harness": "claude"});
    let r = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(body.clone()),
        &alice,
    )
    .await?;
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    for harness in ["auto", "antigravity", "claude"] {
        let mut changed = body.clone();
        changed["harness"] = json!(harness);
        let put = call(
            &h.router,
            Method::PUT,
            &format!("/api/automations/{id}"),
            Some(changed),
            &alice,
        )
        .await?;
        assert_eq!(put.status, StatusCode::OK, "{:?}", put.body);
        let listed = call(
            &h.router,
            Method::GET,
            "/api/sandboxes/s1/automations",
            None,
            &alice,
        )
        .await?;
        let stored = listed
            .body
            .as_array()
            .and_then(|l| l.iter().find(|a| a["id"] == json!(id)).cloned())
            .ok_or("listed")?;
        assert_eq!(stored["harness"], json!(harness));
    }
    let template = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/automation-templates/docs-up-to-date/enable",
        None,
        &alice,
    )
    .await?;
    assert_eq!(template.status, StatusCode::CREATED, "{:?}", template.body);
    assert_eq!(template.body["harness"], "auto");
    Ok(())
}
