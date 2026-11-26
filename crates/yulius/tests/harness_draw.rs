//! Random harness selection across all harnesses with available subscriptions (SPEC-520,
//! ADR-0377): a task created without an agent (or with `auto`) and without a model draws uniformly
//! among `claude`, `codex` and `antigravity` whose subscriptions have non-exhausted capacity,
//! giving every eligible harness equal weight regardless of how many subscriptions it has; a
//! running task agent's unadorned `task_create` / `automation_create` keeps the calling task's
//! harness while explicit `auto` draws and a Superagent defaults to Claude Code;
//! automations default to `auto` (drawn per run) and
//! preserve their stored harness on partial updates.

use std::collections::BTreeSet;

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, perms, user_with_token,
};
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::store::{AccountStore, TaskStore as _};
use yulius_domain::harness::{Harness as Agent, Provider};
use yulius_domain::ids::{AccountId, SandboxId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

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

async fn insert_account(
    h: &Harness,
    id: &str,
    provider: Provider,
    credential: bool,
    status: AccountStatus,
    usage: Option<&AccountUsage>,
) -> TestResult {
    let raw: &[u8] = match provider {
        Provider::Google => br#"{"kind":"gemini_api_key","api_key":"SYNTHETIC-0123456789abcdef"}"#,
        _ => b"synthetic-subscription-credential",
    };
    let sealed = if credential {
        Some(h.app.deps.crypto.seal(raw)?)
    } else {
        None
    };
    let usage_json = usage.map(yulius_app::usage_json::to_json);
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider,
            label: id.into(),
            credential: sealed,
            status,
            usage_json,
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

/// A healthy non-exhausted account `id` of `provider`.
async fn available(h: &Harness, id: &str, provider: Provider) -> TestResult {
    insert_account(h, id, provider, true, AccountStatus::Ok, None).await
}

/// Every harness has at least one available subscription: 5 on Claude, 1 on Codex, 1 on
/// Antigravity (testing equal harness weight regardless of subscription count).
async fn uneven_all_available(h: &Harness) -> TestResult {
    for i in 1..=5 {
        available(h, &format!("a-claude-{i}"), Provider::Anthropic).await?;
    }
    available(h, "a-codex-1", Provider::OpenAi).await?;
    available(h, "a-google-1", Provider::Google).await
}

fn pair(harness: &str, choice: &str) -> (String, String) {
    (harness.to_owned(), choice.to_owned())
}

async fn created(h: &Harness, alice: &Creds, body: Value) -> Result<(String, String), Error> {
    let r = call(&h.router, Method::POST, CREATE, Some(body), alice).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let summary_pair = pair(
        r.body["harness"].as_str().ok_or("harness")?,
        r.body["harness_choice"].as_str().ok_or("harness_choice")?,
    );
    let id = r.body["id"].as_str().ok_or("id")?;
    let reread = shown(h, alice, id).await?;
    assert_eq!(summary_pair, reread, "create response matches GET");
    Ok(reread)
}

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

/// The HTTP request keeps the owner's choice, including an explicit default value.
#[tokio::test]
async fn codex_task_creation_preserves_effort_provenance() -> TestResult {
    let (h, alice) = world().await?;
    available(&h, "a-codex", Provider::OpenAi).await?;
    for (model, effort, expected) in [
        (None, None, "high"),
        (Some("gpt-6.2-astra"), None, "high"),
        (None, Some("high"), "high"),
        (Some("gpt-6.2-astra"), Some("xhigh"), "xhigh"),
        (Some("gpt-6.2-astra"), Some("max"), "max"),
        (Some("gpt-6.2-sol"), None, "high"),
    ] {
        let mut body = json!({"prompt":"Compare effort", "harness":"codex"});
        if let Some(model) = model {
            body["model"] = json!(model);
        }
        if let Some(effort) = effort {
            body["effort"] = json!(effort);
        }
        let response = call(&h.router, Method::POST, CREATE, Some(body), &alice).await?;
        assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
        let id = response.body["id"].as_str().ok_or("task id")?;
        let record = h
            .store
            .task(&yulius_domain::ids::TaskId::parse(id)?)
            .await?
            .ok_or("task")?;
        assert_eq!(record.effort.as_deref(), effort);
        let detail = call(
            &h.router,
            Method::GET,
            &format!("/api/tasks/{id}"),
            None,
            &alice,
        )
        .await?;
        assert_eq!(detail.status, StatusCode::OK);
        assert_eq!(detail.body["effort"], expected);
        assert_eq!(detail.body["effort_default"], effort.is_none());
        assert_eq!(detail.body["model_default"], model.is_none());
    }
    Ok(())
}

/// With 5 Claude, 1 Codex and 1 Antigravity subscriptions available,
/// tasks without an agent (or with `auto`) draw across all three harnesses as `random`; an
/// explicit harness or model-implied harness stays `chosen`.
#[tokio::test]
async fn a_task_without_an_agent_draws_across_all_harnesses_and_keeps_explicit_choices()
-> TestResult {
    let (h, alice) = world().await?;
    uneven_all_available(&h).await?;
    // Even with 5 Claude subscriptions against 1 Codex and 1 Antigravity, the candidate list is
    // `[claude, codex, antigravity]`: rolls 0..6 cycle 1:1:1 across the three harnesses.
    h.app.queue_harness_rolls(&[0, 1, 2, 3, 4, 5]);
    for expected in [
        "claude",
        "codex",
        "antigravity",
        "claude",
        "codex",
        "antigravity",
    ] {
        assert_eq!(
            created(&h, &alice, json!({"prompt": format!("Roll {expected}")})).await?,
            pair(expected, "random")
        );
    }
    let mut seen = BTreeSet::new();
    for n in 0..36 {
        let body = if n % 2 == 0 {
            json!({"prompt": format!("Task {n}")})
        } else {
            json!({"prompt": format!("Task {n}"), "harness": "auto"})
        };
        let (harness, choice) = created(&h, &alice, body).await?;
        assert_eq!(choice, "random", "task {n} was drawn");
        seen.insert(harness);
    }
    assert_eq!(
        seen,
        BTreeSet::from([
            "antigravity".to_owned(),
            "claude".to_owned(),
            "codex".to_owned(),
        ]),
        "all three harnesses are drawn"
    );
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
            json!({"prompt": "E", "model": "gemini-4-neon"}),
            "antigravity",
        ),
        (
            json!({"prompt": "F", "harness": "auto", "model": "gemini-4-neon"}),
            "antigravity",
        ),
        (
            json!({"prompt": "G", "model": "gemini-4-carbon"}),
            "antigravity",
        ),
        (
            json!({"prompt": "H", "harness": "auto", "model": "gemini-4-carbon"}),
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

/// A supplied GPT model chooses Codex independently of the subscription draw.
#[tokio::test]
async fn the_api_chooses_codex_for_gpt_and_preserves_unqualified_aliases() -> TestResult {
    let (h, alice) = world().await?;
    for (selection, expected) in [
        (json!({"model": "gpt-6.2-astra"}), "codex"),
        (
            json!({"harness": "auto", "model": " GPT-synthetic "}),
            "codex",
        ),
        (json!({"model": "opus"}), "claude"),
        (json!({"harness": "codex", "model": "o3"}), "codex"),
    ] {
        let mut body = selection.clone();
        body["prompt"] = json!("Check model routing");
        body["prepared"] = json!(true);
        let response = call(&h.router, Method::POST, CREATE, Some(body), &alice).await?;
        assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
        let id = response.body["id"].as_str().ok_or("id")?;
        let detail = call(
            &h.router,
            Method::GET,
            &format!("/api/tasks/{id}"),
            None,
            &alice,
        )
        .await?;
        assert_eq!(detail.status, StatusCode::OK);
        assert_eq!(detail.body["harness"], expected, "{selection}");
        assert_eq!(detail.body["harness_choice"], "chosen");
        assert_eq!(
            detail.body["model"],
            selection["model"].as_str().ok_or("model")?.trim()
        );
        assert_eq!(detail.body["model_default"], false);
    }
    Ok(())
}

#[tokio::test]
async fn a_blank_model_uses_the_named_harness_default() -> TestResult {
    let (h, alice) = world().await?;
    let response = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Default Gemini model", "prepared": true, "harness": "antigravity", "model": "  "})),
        &alice,
    ).await?;
    assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
    let id = response.body["id"].as_str().ok_or("id")?;
    let detail = call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.body["harness"], "antigravity");
    assert_eq!(
        detail.body["model"],
        yulius_domain::harness::DEFAULT_ANTIGRAVITY_MODEL
    );
    assert_eq!(detail.body["model_default"], true);
    Ok(())
}

#[tokio::test]
async fn an_explicit_harness_refuses_a_foreign_model_before_creation() -> TestResult {
    let (h, alice) = world().await?;
    for (harness, model, guidance) in [
        ("claude", "gpt-6.2-astra", "Codex"),
        ("claude", " Gemini-4-neon ", "Antigravity"),
        ("codex", "gemini-4-neon", "Antigravity"),
        ("codex", "claude-opus-5-5", "OpenAI model"),
        ("antigravity", "o3", "takes a Gemini model"),
    ] {
        let response = call(
            &h.router, Method::POST, CREATE,
            Some(json!({"prompt": "Conflicting choice", "prepared": true, "harness": harness, "model": model})),
            &alice,
        ).await?;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            response.body
        );
        assert!(
            response.body.to_string().contains(guidance),
            "{:?}",
            response.body
        );
    }
    assert!(
        h.store
            .tasks(&SandboxId::parse("s1")?, false)
            .await?
            .is_empty()
    );
    Ok(())
}

/// Harnesses with 0 subscriptions, only unsigned/relogin/error subscriptions, or
/// only finished subscriptions (`Limited` or `>= 98%` in a window binding the default model) are
/// never drawn while another harness has capacity; once one subscription of a harness has room,
/// that harness re-enters the draw.
#[tokio::test]
async fn finished_and_missing_harnesses_are_excluded_while_another_has_capacity() -> TestResult {
    let (h, alice) = world().await?;
    let now = h.app.now();
    let future = Some(now.plus_millis(3_600_000));
    insert_account(
        &h,
        "a-claude-pending",
        Provider::Anthropic,
        false,
        AccountStatus::PendingLogin,
        None,
    )
    .await?;
    insert_account(
        &h,
        "a-claude-relogin",
        Provider::Anthropic,
        true,
        AccountStatus::Relogin,
        None,
    )
    .await?;
    insert_account(
        &h,
        "a-codex-err",
        Provider::OpenAi,
        true,
        AccountStatus::Error,
        None,
    )
    .await?;
    available(&h, "a-google-ok", Provider::Google).await?;

    // Only Antigravity has a usable subscription: every draw picks Antigravity.
    for i in 0..4 {
        assert_eq!(
            created(&h, &alice, json!({"prompt": format!("Only google {i}")})).await?,
            pair("antigravity", "random")
        );
    }

    // Add finished Claude subscriptions (one Limited, one with weekly_opus at 99% >= 98% threshold
    // for default Opus 5.5) and a bare-Limited Codex subscription without usage_json: neither
    // Claude nor Codex has capacity, so Antigravity is still the only eligible harness.
    let limited_usage = AccountUsage {
        limit_reached: true,
        limit_resets_at: future,
        ..AccountUsage::default()
    };
    insert_account(
        &h,
        "a-claude-limited",
        Provider::Anthropic,
        true,
        AccountStatus::Limited,
        Some(&limited_usage),
    )
    .await?;
    let opus_exhausted = AccountUsage {
        primary: Some(UsageWindow {
            used_percent: Some(12.0),
            resets_at: future,
            window_minutes: Some(300),
        }),
        weekly_opus: Some(UsageWindow {
            used_percent: Some(99.0),
            resets_at: future,
            window_minutes: Some(10080),
        }),
        ..AccountUsage::default()
    };
    insert_account(
        &h,
        "a-claude-opus-full",
        Provider::Anthropic,
        true,
        AccountStatus::Ok,
        Some(&opus_exhausted),
    )
    .await?;
    insert_account(
        &h,
        "a-codex-bare-limited",
        Provider::OpenAi,
        true,
        AccountStatus::Limited,
        None,
    )
    .await?;

    for i in 0..8 {
        assert_eq!(
            created(
                &h,
                &alice,
                json!({"prompt": format!("Still only google {i}")})
            )
            .await?,
            pair("antigravity", "random"),
            "finished Claude and Codex accounts must be skipped"
        );
    }

    // Adding one roomy Claude subscription beside the two finished ones puts Claude back in the
    // draw with one equal slot (`[claude, antigravity]`), while finished Codex stays out.
    available(&h, "a-claude-roomy", Provider::Anthropic).await?;
    h.app.queue_harness_rolls(&[0, 1, 2, 3]);
    for expected in ["claude", "antigravity", "claude", "antigravity"] {
        assert_eq!(
            created(&h, &alice, json!({"prompt": format!("Partial {expected}")})).await?,
            pair(expected, "random")
        );
    }
    Ok(())
}

/// When every connected harness's subscriptions are currently limited (waiting to
/// reset), a random task still draws among those connected harnesses so it waits for capacity
/// instead of failing for a missing Claude account; with 0 usable accounts anywhere, 409 is
/// returned.
#[tokio::test]
async fn all_limited_connected_harness_queues_and_zero_accounts_refuses() -> TestResult {
    let (h, alice) = world().await?;
    let refused = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "No accounts yet"})),
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

    let now = h.app.now();
    let limited = AccountUsage {
        limit_reached: true,
        limit_resets_at: Some(now.plus_millis(3_600_000)),
        ..AccountUsage::default()
    };
    insert_account(
        &h,
        "a-google-limited",
        Provider::Google,
        true,
        AccountStatus::Limited,
        Some(&limited),
    )
    .await?;
    assert_eq!(
        created(&h, &alice, json!({"prompt": "Wait on Google"})).await?,
        pair("antigravity", "random")
    );
    Ok(())
}

async fn agent_of(h: &Harness, title: &str) -> Result<(String, String), Error> {
    let tasks = h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    let task = tasks.iter().find(|t| t.title == title).ok_or("no task")?;
    let summary = h.app.task_summary(task).await?;
    Ok(pair(&summary.harness, &summary.harness_choice))
}

/// A running task's `task_create` and `automation_create` without `harness` or
/// `model` keep the calling task's harness (`chosen`); explicit `auto` draws (`random`) or stores
/// `"auto"`. A Superagent defaults to Claude Code for tasks and to `auto` for
/// automations.
#[tokio::test]
async fn agent_follow_ups_inherit_the_parent_and_superagent_tasks_default_to_claude() -> TestResult
{
    let (h, alice) = world().await?;
    uneven_all_available(&h).await?;
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
    for (args, expected_harness, expected_choice) in [
        (
            json!({"prompt": "Follow-up", "title": "From Gemini"}),
            Some("antigravity"),
            "chosen",
        ),
        (
            json!({"prompt": "Follow-up", "title": "From Gemini, auto", "harness": "auto"}),
            None,
            "random",
        ),
        (
            json!({"prompt": "Follow-up", "title": "Gemini asks Sonnet", "model": "claude-sonnet-5-5"}),
            Some("claude"),
            "chosen",
        ),
        (
            json!({"prompt": "Follow-up", "title": "Gemini asks for GPT", "model": "gpt-6.2-astra"}),
            Some("codex"),
            "chosen",
        ),
        (
            json!({"prompt": "Follow-up", "title": "Gemini asks Codex", "harness": "codex"}),
            Some("codex"),
            "chosen",
        ),
    ] {
        let title = args["title"].as_str().ok_or("title")?.to_owned();
        let tool = support::mcp_tool(&h, "tok-gemini", "task_create", args).await?;
        assert_eq!(tool["isError"], false, "{tool}");
        let (got_harness, got_choice) = agent_of(&h, &title).await?;
        assert_eq!(got_choice, expected_choice, "{title}");
        if let Some(expected) = expected_harness {
            assert_eq!(got_harness, expected, "{title}");
        }
    }

    for (name, harness, expected) in [
        ("Caller default", None, "antigravity"),
        ("Explicit auto", Some("auto"), "auto"),
        ("Named claude", Some("claude"), "claude"),
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

    // The Superagent chooses Claude Code even with other providers available, with no model or
    // effort of its own: it runs Opus 5.5 at high.
    let superagent = superagent()?;
    h.app
        .tool_task_create(
            &superagent,
            &json!({"prompt": "From Superagent", "title": "Superagent default"}),
        )
        .await?;
    assert_eq!(
        agent_of(&h, "Superagent default").await?,
        pair("claude", "chosen")
    );
    let tasks = h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    let default = tasks
        .iter()
        .find(|t| t.title == "Superagent default")
        .ok_or("no task")?;
    assert_eq!((&default.model, &default.effort), (&None, &None));
    let explicit = h
        .app
        .tool_task_create(
            &superagent,
            &json!({"prompt": "Compare settings", "model": "gemini-4-neon", "effort": "high"}),
        )
        .await?;
    assert_eq!(explicit["task"]["harness"], "antigravity");
    assert_eq!(explicit["task"]["effort"], "high");
    h.app
        .tool_task_create(
            &superagent,
            &json!({"prompt": "Draw a harness", "title": "Superagent Random", "harness": "auto"}),
        )
        .await?;
    assert_eq!(agent_of(&h, "Superagent Random").await?.1, "random");
    h.app
        .tool_automation_create(
            &superagent,
            &json!({"name": "Superagent auto", "cron": "0 5 * * *", "prompt": "Check", "enabled": false}),
        )
        .await?;
    assert_eq!(
        automation_agent(&h, &alice, "Superagent auto").await?,
        json!("auto")
    );
    Ok(())
}

/// The sandbox Superagent of alice in `s1`: a caller without a task.
fn superagent() -> Result<yulius_app::coordination::ProjectCaller, Error> {
    Ok(yulius_app::coordination::ProjectCaller {
        sandbox: SandboxId::parse("s1")?,
        user: UserId::parse("u-alice")?,
        task: None,
        channel: yulius_app::coordination::Channel::Superagent,
        actor: "cockpit:alice".into(),
        detail: json!({"via": "superagent"}),
    })
}

/// The Superagent's default stays Claude Code while only Codex and Antigravity
/// can run: the start is refused naming Claude, and no task is made on another agent.
#[tokio::test]
async fn a_superagent_default_without_a_claude_account_is_refused_naming_claude() -> TestResult {
    let (h, _alice) = world().await?;
    available(&h, "a-codex-1", Provider::OpenAi).await?;
    available(&h, "a-google-1", Provider::Google).await?;
    let refused = h
        .app
        .tool_task_create(&superagent()?, &json!({"prompt": "From Superagent"}))
        .await
        .err()
        .ok_or("the default start is refused")?;
    assert!(
        refused
            .to_string()
            .contains("Connect a Claude account first"),
        "{refused}"
    );
    assert!(
        h.store
            .tasks(&SandboxId::parse("s1")?, false)
            .await?
            .is_empty()
    );
    Ok(())
}

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

/// Automations created without `harness` (or with `"auto"`) and ready-made
/// templates store `"auto"` and draw per run; `PUT` without `harness` preserves the stored agent
/// (`"antigravity"` or `"auto"`).
#[tokio::test]
async fn an_automation_defaults_to_auto_draws_each_run_and_keeps_its_agent_on_partial_put()
-> TestResult {
    let (h, alice) = world().await?;
    uneven_all_available(&h).await?;

    let base = json!({"name": "Daily", "cron": "0 9 1 1 *", "utc_offset_minutes": 0, "prompt": "Review yesterday's commits", "enabled": true});
    let created_auto = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/automations",
        Some(base.clone()),
        &alice,
    )
    .await?;
    assert_eq!(created_auto.status, StatusCode::CREATED);
    assert_eq!(created_auto.body["harness"], "auto");
    let auto_id = created_auto.body["id"].as_str().ok_or("id")?.to_owned();
    h.app.queue_harness_rolls(&[0, 1, 2]);
    for expected in ["claude", "codex", "antigravity"] {
        assert_eq!(
            run_agent(&h, &alice, &auto_id).await?,
            pair(expected, "random")
        );
    }

    // Partial PUT (omitting `harness`) on an `auto` automation keeps `auto`.
    let mut toggled = base.clone();
    toggled["enabled"] = json!(false);
    let put_auto = call(
        &h.router,
        Method::PUT,
        &format!("/api/automations/{auto_id}"),
        Some(toggled),
        &alice,
    )
    .await?;
    assert_eq!(put_auto.status, StatusCode::OK);
    assert_eq!(put_auto.body["harness"], "auto");

    // Switch to `antigravity`, verify partial PUT keeps `antigravity`, then switch back to `auto`.
    let mut named = base.clone();
    named["harness"] = json!("antigravity");
    let put_named = call(
        &h.router,
        Method::PUT,
        &format!("/api/automations/{auto_id}"),
        Some(named),
        &alice,
    )
    .await?;
    assert_eq!(put_named.body["harness"], "antigravity");
    assert_eq!(
        run_agent(&h, &alice, &auto_id).await?,
        pair("antigravity", "chosen")
    );

    let mut omit_on_named = base.clone();
    omit_on_named["enabled"] = json!(false);
    let kept = call(
        &h.router,
        Method::PUT,
        &format!("/api/automations/{auto_id}"),
        Some(omit_on_named),
        &alice,
    )
    .await?;
    assert_eq!(kept.body["harness"], "antigravity");

    let mut back_to_auto = base.clone();
    back_to_auto["harness"] = json!("auto");
    let back = call(
        &h.router,
        Method::PUT,
        &format!("/api/automations/{auto_id}"),
        Some(back_to_auto),
        &alice,
    )
    .await?;
    assert_eq!(back.body["harness"], "auto");

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

/// A drawn task says `random` in its summary and in the JSON and CSV analytics
/// exports; an explicit task says `chosen`; deleting a drawn task deletes its draw record.
#[tokio::test]
async fn analytics_and_deletion_record_drawn_and_chosen_tasks() -> TestResult {
    let (h, alice) = world().await?;
    available(&h, "a-claude", Provider::Anthropic).await?;
    let r = call(
        &h.router,
        Method::POST,
        CREATE,
        Some(json!({"prompt": "Drawn", "title": "Drawn"})),
        &alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.body["harness_choice"], "random");
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(
        created(
            &h,
            &alice,
            json!({"prompt": "Explicit", "title": "Explicit", "harness": "claude"})
        )
        .await?,
        pair("claude", "chosen")
    );
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
    assert_eq!(choice("Explicit"), Some(json!("chosen")));
    let csv =
        yulius_app::analytics::tasks_csv(&serde_json::from_value::<Vec<_>>(rows.body.clone())?);
    assert!(
        csv.lines()
            .any(|l| l.contains(",Drawn,") && l.contains(",claude,random,")),
        "{csv}"
    );
    assert!(
        csv.lines()
            .any(|l| l.contains(",Explicit,") && l.contains(",claude,chosen,")),
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
