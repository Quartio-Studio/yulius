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
    let credential: &[u8] = match provider {
        Provider::Google => br#"{"kind":"gemini_api_key","api_key":"SYNTHETIC-0123456789abcdef"}"#,
        _ => b"synthetic-subscription-credential",
    };
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(credential)?),
            status: AccountStatus::Ok,
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
