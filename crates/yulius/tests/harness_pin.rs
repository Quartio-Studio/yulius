//! A task's agent is set, never drawn (SPEC-514, ADR-0368): a task created without an agent runs
//! the one its model implies, else Claude Code, whatever subscriptions are usable; an agent's
//! follow-up task or automation keeps the calling task's agent; an automation runs the agent it
//! names. Tasks drawn before (SPEC-505 RND-001) still say so. The in-memory harness has no session
//! slot, so a start that passes the account gate queues.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, perms, user_with_token,
};
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::store::{AccountStore, TaskStore as _};
use yulius_domain::harness::{Harness as Agent, Provider};
use yulius_domain::ids::{AccountId, SandboxId};
use yulius_domain::task::TaskStatus;
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

/// Every agent usable: Claude Code, Codex and Antigravity.
async fn all_usable(h: &Harness) -> TestResult {
    usable(h, "a-claude", Provider::Anthropic).await?;
    usable(h, "a-google", Provider::Google).await?;
    usable(h, "a-codex", Provider::OpenAi).await
}

fn pair(harness: &str, choice: &str) -> (String, String) {
    (harness.to_owned(), choice.to_owned())
}

/// Creates a task from `body` and returns its summary's agent and how it was picked, as created
/// and as read again.
async fn created(h: &Harness, alice: &Creds, body: Value) -> Result<(String, String), Error> {
    let r = call(&h.router, Method::POST, CREATE, Some(body), alice).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = r.body["id"].as_str().ok_or("id")?;
    shown(h, alice, id).await
}

/// A task's agent and how it was picked, as `GET /api/tasks/{id}` says.
async fn shown(h: &Harness, alice: &Creds, id: &str) -> Result<(String, String), Error> {
    let r = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    Ok(pair(
        r.body["harness"].as_str().ok_or("harness")?,
        r.body["harness_choice"].as_str().ok_or("harness_choice")?,
    ))
}

/// PIN-001 (AC-1): with every agent usable, tasks created without an agent (or with `auto`) all
/// run Claude Code, chosen; an agent given is kept, and so is the one a model implies.
#[tokio::test]
async fn a_task_without_an_agent_runs_claude_code_every_time() -> TestResult {
    let (h, alice) = world().await?;
    all_usable(&h).await?;
    for n in 0..10 {
        let body = if n % 2 == 0 {
            json!({"prompt": format!("Task {n}")})
        } else {
            json!({"prompt": format!("Task {n}"), "harness": "auto"})
        };
        assert_eq!(
            created(&h, &alice, body).await?,
            pair("claude", "chosen"),
            "task {n}"
        );
    }
    for (body, expected) in [
        (json!({"prompt": "A", "harness": "claude"}), "claude"),
        (
            json!({"prompt": "B", "harness": "antigravity"}),
            "antigravity",
        ),
        (json!({"prompt": "C", "harness": "codex"}), "codex"),
        (
            json!({"prompt": "D", "model": "claude-sonnet-5-5"}),
            "claude",
        ),
        (
            json!({"prompt": "E", "model": "gemini-4-carbon"}),
            "antigravity",
        ),
        (
            json!({"prompt": "F", "harness": "auto", "model": "gemini-4-carbon"}),
            "antigravity",
        ),
    ] {
        assert_eq!(
            created(&h, &alice, body.clone()).await?,
            pair(expected, "chosen"),
            "{body}"
        );
    }
    Ok(())
}

/// PIN-001 (AC-1): with only Antigravity usable, a task without an agent is still Claude Code's,
/// so it is refused for want of a Claude account instead of running on Gemini.
#[tokio::test]
async fn a_task_without_an_agent_never_falls_to_another_agent() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-google", Provider::Google).await?;
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
    let tasks = h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    assert!(tasks.is_empty(), "nothing was created: {tasks:?}");
    assert_eq!(
        created(
            &h,
            &alice,
            json!({"prompt": "Two", "harness": "antigravity"})
        )
        .await?,
        pair("antigravity", "chosen")
    );
    Ok(())
}

/// The agent of the task titled `title` in `s1`.
async fn agent_of(h: &Harness, title: &str) -> Result<(String, String), Error> {
    let tasks = h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    let task = tasks.iter().find(|t| t.title == title).ok_or("no task")?;
    let summary = h.app.task_summary(task).await?;
    Ok(pair(&summary.harness, &summary.harness_choice))
}

/// PIN-002 (AC-2): a task an agent creates with `task_create` without an agent or a model runs
/// the calling task's agent; a model or an agent named is kept.
#[tokio::test]
async fn a_task_an_agent_creates_keeps_the_calling_tasks_agent() -> TestResult {
    let (h, _alice) = world().await?;
    all_usable(&h).await?;
    support::seed_task_of(
        &h,
        "t-gemini",
        "s1",
        "u-alice",
        TaskStatus::Running,
        None,
        "do things",
        Agent::Antigravity,
    )
    .await?;
    support::seed_live_attempt(&h, "t-gemini", "tok-gemini").await?;
    support::seed_task(&h, "t-claude", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&h, "t-claude", "tok-claude").await?;
    for (token, args, expected) in [
        (
            "tok-gemini",
            json!({"prompt": "Follow-up", "title": "From Gemini"}),
            "antigravity",
        ),
        (
            "tok-gemini",
            json!({"prompt": "Follow-up", "title": "From Gemini, auto", "harness": "auto"}),
            "antigravity",
        ),
        (
            "tok-gemini",
            json!({"prompt": "Follow-up", "title": "Gemini asks Sonnet", "model": "claude-sonnet-5-5"}),
            "claude",
        ),
        (
            "tok-gemini",
            json!({"prompt": "Follow-up", "title": "Gemini asks Codex", "harness": "codex"}),
            "codex",
        ),
        (
            "tok-claude",
            json!({"prompt": "Follow-up", "title": "From Claude"}),
            "claude",
        ),
    ] {
        let title = args["title"].as_str().ok_or("title")?.to_owned();
        let tool = support::mcp_tool(&h, token, "task_create", args).await?;
        assert_eq!(tool["isError"], false, "{tool}");
        assert_eq!(
            agent_of(&h, &title).await?,
            pair(expected, "chosen"),
            "{title}"
        );
    }
    Ok(())
}

/// The stored agent of the automation named `name` in `s1`.
async fn automation_agent(h: &Harness, alice: &Creds, name: &str) -> Result<Value, Error> {
    let listed = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s1/automations",
        None,
        alice,
    )
    .await?;
    let found = listed
        .body
        .as_array()
        .and_then(|l| l.iter().find(|a| a["name"] == name).cloned())
        .ok_or("listed")?;
    Ok(found["harness"].clone())
}

/// PIN-002 (AC-2): an automation an agent creates without an agent runs the calling task's.
#[tokio::test]
async fn an_automation_an_agent_creates_keeps_the_calling_tasks_agent() -> TestResult {
    let (h, alice) = world().await?;
    all_usable(&h).await?;
    support::seed_task_of(
        &h,
        "t-gemini",
        "s1",
        "u-alice",
        TaskStatus::Running,
        None,
        "do things",
        Agent::Antigravity,
    )
    .await?;
    support::seed_live_attempt(&h, "t-gemini", "tok-gemini").await?;
    for (name, harness, expected) in [
        ("Absent", None, "antigravity"),
        ("Auto", Some("auto"), "antigravity"),
        ("Named", Some("claude"), "claude"),
    ] {
        let mut args = json!({"name": name, "cron": "0 4 * * *", "prompt": "Align the docs", "enabled": false});
        if let Some(harness) = harness {
            args["harness"] = json!(harness);
        }
        let tool = support::mcp_tool(&h, "tok-gemini", "automation_create", args).await?;
        assert_eq!(tool["isError"], false, "{tool}");
        assert_eq!(
            automation_agent(&h, &alice, name).await?,
            json!(expected),
            "{name}"
        );
    }
    Ok(())
}

/// PIN-003 (AC-3): an automation saved without an agent, or as `auto`, runs Claude Code, stored
/// so; every run is a Claude Code task, chosen; a ready-made automation runs Claude Code too.
#[tokio::test]
async fn an_automation_runs_the_agent_it_names() -> TestResult {
    let (h, alice) = world().await?;
    all_usable(&h).await?;
    let mut ids = Vec::new();
    for (name, harness, expected) in [
        ("Absent", None, "claude"),
        ("Auto", Some("auto"), "claude"),
        ("Gemini", Some("antigravity"), "antigravity"),
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
        assert_eq!(r.body["harness"], json!(expected), "{:?}", r.body);
        assert_eq!(automation_agent(&h, &alice, name).await?, json!(expected));
        ids.push((r.body["id"].as_str().ok_or("id")?.to_owned(), expected));
    }
    for _round in 0..3 {
        for (id, expected) in &ids {
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
            assert_eq!(shown(&h, &alice, task).await?, pair(expected, "chosen"));
        }
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
    assert_eq!(template.body["harness"], "claude");
    Ok(())
}

/// Runs automation `id` now and returns its task's agent and how it was picked. A run now is keyed
/// by the time, so the clock moves on first: each call is a run of its own, whose task it stops.
async fn run_agent(h: &Harness, alice: &Creds, id: &str) -> Result<(String, String), Error> {
    h.clock
        .0
        .fetch_add(60_000, std::sync::atomic::Ordering::SeqCst);
    let run = call(
        &h.router,
        Method::POST,
        &format!("/api/automations/{id}/run"),
        None,
        alice,
    )
    .await?;
    assert_eq!(run.body["state"], "task_created", "{:?}", run.body);
    let task = run.body["task_id"].as_str().ok_or("task")?;
    let agent = shown(h, alice, task).await?;
    // A run is skipped while the previous one's task is active: this one ends here.
    let stopped = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{task}/stop"),
        None,
        alice,
    )
    .await?;
    assert_eq!(stopped.status, StatusCode::OK, "{:?}", stopped.body);
    Ok(agent)
}

/// PIN-003 (review round 1): an update keeps the automation's agent unless it names another. The
/// Automations list's enable switch sends no agent: pausing and re-enabling an Antigravity
/// automation keeps it on Antigravity; `codex` and `antigravity` given are stored and run, and
/// `auto` stores Claude Code.
#[tokio::test]
async fn an_automation_keeps_its_agent_through_an_update() -> TestResult {
    let (h, alice) = world().await?;
    all_usable(&h).await?;
    let body = json!({"name": "Review", "cron": "0 9 1 1 *", "utc_offset_minutes": 0, "prompt": "Review yesterday's commits", "enabled": true, "harness": "antigravity"});
    let r = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(body.clone()),
        &alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    let put = |body: Value| {
        let (router, alice, id) = (&h.router, &alice, &id);
        async move {
            let r = call(
                router,
                Method::PUT,
                &format!("/api/automations/{id}"),
                Some(body),
                alice,
            )
            .await?;
            assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
            Ok::<Value, Error>(r.body["harness"].clone())
        }
    };
    let mut without = body.clone();
    without.as_object_mut().ok_or("object")?.remove("harness");
    for enabled in [false, true] {
        without["enabled"] = json!(enabled);
        assert_eq!(
            put(without.clone()).await?,
            json!("antigravity"),
            "enabled {enabled}"
        );
        assert_eq!(
            automation_agent(&h, &alice, "Review").await?,
            json!("antigravity")
        );
    }
    assert_eq!(
        run_agent(&h, &alice, &id).await?,
        pair("antigravity", "chosen")
    );
    for (harness, stored) in [
        ("codex", "codex"),
        ("auto", "claude"),
        ("antigravity", "antigravity"),
    ] {
        let mut changed = body.clone();
        changed["harness"] = json!(harness);
        assert_eq!(put(changed).await?, json!(stored), "{harness}");
        assert_eq!(automation_agent(&h, &alice, "Review").await?, json!(stored));
        assert_eq!(
            run_agent(&h, &alice, &id).await?,
            pair(stored, "chosen"),
            "{harness}"
        );
    }
    Ok(())
}

/// PIN-005 (AC-5): a task drawn before this change still says so, in its summary and the
/// analytics export; a task created now says chosen; deleting a drawn task removes its draw.
#[tokio::test]
async fn a_task_drawn_before_still_says_so() -> TestResult {
    let (h, alice) = world().await?;
    usable(&h, "a-claude", Provider::Anthropic).await?;
    let r = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Drawn", "title": "Drawn"})),
        &alice,
    )
    .await?;
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    created(&h, &alice, json!({"prompt": "Now", "title": "Now"})).await?;
    h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_harness_draws (task_id, drawn_at) VALUES ('{id}', 1)"
        ))
        .await?;
    assert_eq!(shown(&h, &alice, &id).await?, pair("claude", "random"));
    let rows = call(
        &h.router,
        Method::GET,
        "/api/analytics/tasks?format=json&sandbox=s1",
        None,
        &alice,
    )
    .await?;
    assert_eq!(rows.status, StatusCode::OK, "{:?}", rows.body);
    let choice = |title: &str| {
        rows.body
            .as_array()
            .and_then(|l| l.iter().find(|r| r["title"] == title))
            .map(|r| r["harness_choice"].clone())
    };
    assert_eq!(choice("Drawn"), Some(json!("random")));
    assert_eq!(choice("Now"), Some(json!("chosen")));
    // The CSV export has `harness_choice` right after `harness` (SPEC-505 RND-003).
    let csv =
        yulius_app::analytics::tasks_csv(&serde_json::from_value::<Vec<_>>(rows.body.clone())?);
    let row = |title: &str| {
        csv.lines()
            .find(|l| l.contains(&format!(",{title},")))
            .map(str::to_owned)
    };
    assert!(
        row("Drawn").is_some_and(|l| l.contains(",claude,random,")),
        "{csv}"
    );
    assert!(
        row("Now").is_some_and(|l| l.contains(",claude,chosen,")),
        "{csv}"
    );
    let task = yulius_domain::ids::TaskId::parse(&id)?;
    assert!(h.store.harness_drawn(&task).await?);
    assert!(h.store.delete_task(&task).await?);
    assert!(
        !h.store.harness_drawn(&task).await?,
        "the draw went with the task"
    );
    Ok(())
}
