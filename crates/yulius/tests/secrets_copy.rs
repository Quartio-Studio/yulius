//! Copies of production secrets agents ask for (SPEC-440, ADR-0284): an agent asks with
//! `secret_request_copy`; a person who manages the secret and the project allows the copy after a
//! two-factor unlock, which creates a new agent secret of the same value bound to the hosts asked
//! for, or dismisses it. The production secret itself stays production's.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness, perms, seed_github_user, seed_live_attempt, seed_sandbox,
    seed_task,
};
use yulius_app::ports::store::TaskStore as _;
use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};

const VALUE: &str = concat!("n3dev_", "ci_token_synthetic_7Hq2");
const OTHER: &str = concat!("sk_live_", "51CopyOtherValue9q2w");
const TOKEN: &str = "copy-task-token-s1-0123456789abcdef";
const TOKEN_2: &str = "copy-task-token-s1-second-0123456789";
const HOST: &str = "165-245-200-246.sslip.io";

struct World {
    h: support::Harness,
    writer: Creds,
    /// alice in the web UI (a session, which a two-factor unlock needs).
    browser: Creds,
    reader: Creds,
    /// The production secret `N3_DEV_CI_TOKEN`.
    prod: String,
}

/// Sandbox s1 (`app`): alice writes it, bob reads it; tasks t1 and t2 (alice's) run with live MCP
/// tokens. `N3_DEV_CI_TOKEN` is a production secret of s1; `STRIPE_KEY` an ordinary one.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = harness()?;
    // A set-up instance with GitHub sign-in, so alice also has a web session.
    support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("read"));
    for (task, token) in [("t1", TOKEN), ("t2", TOKEN_2)] {
        let dir = h
            .app
            .config
            .data_dir
            .join("sandboxes/s1/worktrees")
            .join(task);
        std::fs::create_dir_all(&dir)?;
        seed_task(&h, task, "s1", "u-alice", TaskStatus::Running, Some(&dir)).await?;
        seed_live_attempt(&h, task, token).await?;
    }
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"name": "N3_DEV_CI_TOKEN", "value": VALUE, "injection": "bearer",
            "description": "CI token of N3.1 dev", "auto_allow": false}),
        ),
        &writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let prod = created.body["id"].as_str().ok_or("id")?.to_owned();
    let production = call(
        &h.router,
        Method::POST,
        &format!("/api/agent-secrets/{prod}/cd-prod"),
        Some(json!({"allow": true})),
        &writer,
    )
    .await?;
    assert_eq!(production.status, StatusCode::OK, "{}", production.body);
    let other = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "STRIPE_KEY", "value": OTHER})),
        &writer,
    )
    .await?;
    assert_eq!(other.status, StatusCode::CREATED, "{}", other.body);
    let browser = crate::secrets::admit_again(&h, "alice", 501).await?;
    Ok(World {
        h,
        writer,
        browser,
        reader,
        prod,
    })
}

async fn ask(w: &World, token: &str, args: Value) -> Result<Value, Box<dyn std::error::Error>> {
    support::mcp_tool(&w.h, token, "secret_request_copy", args).await
}

fn copy_args(hosts: &[&str]) -> Value {
    json!({"secret": "N3_DEV_CI_TOKEN", "name": "N3_DEV_CI_TOKEN_AGENT", "hosts": hosts,
        "reason": "run test-b1 rollouts on dev"})
}

/// The production secret as `creds` sees it in s1's list.
async fn listed(w: &World, creds: &Creds) -> Result<Value, Box<dyn std::error::Error>> {
    let list = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        creds,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    list.body
        .as_array()
        .ok_or("list")?
        .iter()
        .find(|s| s["id"] == w.prod.as_str())
        .cloned()
        .ok_or_else(|| "production secret not listed".into())
}

async fn answer(
    w: &World,
    creds: &Creds,
    allow: bool,
    name: &str,
    hosts: &[&str],
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/s1/agent-secrets/{}/copy", w.prod),
        Some(json!({"allow": allow, "name": name, "hosts": hosts})),
        creds,
    )
    .await
}

async fn activity(w: &World, task: &str) -> Result<Option<Activity>, Box<dyn std::error::Error>> {
    Ok(w.h
        .store
        .task(&TaskId::parse(task)?)
        .await?
        .ok_or("task")?
        .activity)
}

async fn asker(w: &World, task: &str) -> Result<String, Box<dyn std::error::Error>> {
    let number =
        w.h.store
            .task(&TaskId::parse(task)?)
            .await?
            .ok_or("task")?
            .number;
    Ok(format!("#{number}"))
}

/// SPEC-440 CPY-001, CPY-003..CPY-005, CPY-007 (AC-1, AC-2): a task asks for a copy of a
/// production secret and waits; a person allows it after their two-factor code; the copy has the
/// production value, is bound to the host asked for and is the agents' to use, while the
/// production secret keeps no hosts and no `.env` allowance; the task works on.
#[tokio::test]
async fn a_person_allows_the_copy_an_agent_asked_for() -> TestResult {
    let w = world().await?;
    let asked = ask(&w, TOKEN, copy_args(&[HOST])).await?;
    assert_ne!(asked["isError"], true, "{asked}");
    let text = support::tool_text(&asked);
    assert_eq!(text["requested"], "N3_DEV_CI_TOKEN_AGENT", "{text}");
    assert_eq!(text["copy_of"], "N3_DEV_CI_TOKEN", "{text}");
    assert_eq!(text["hosts"], json!([HOST]), "{text}");
    assert!(
        text["link"]
            .as_str()
            .is_some_and(|l| l.ends_with("/s/app/secrets?section=agents")),
        "{text}"
    );
    assert!(!asked.to_string().contains(VALUE), "{asked}");
    assert_eq!(activity(&w, "t1").await?, Some(Activity::NeedsAttention));

    // The Secrets page shows it on the production secret; only a manager may answer.
    let me = asker(&w, "t1").await?;
    let seen = listed(&w, &w.writer).await?;
    assert_eq!(
        seen["copy_request"],
        json!({"name": "N3_DEV_CI_TOKEN_AGENT", "hosts": [HOST], "requested_by": me,
            "reason": "run test-b1 rollouts on dev", "can_answer": true}),
        "{seen}"
    );
    assert_eq!(
        listed(&w, &w.reader).await?["copy_request"]["can_answer"],
        false
    );
    let refused = answer(&w, &w.reader, true, "N3_DEV_CI_TOKEN_AGENT", &[HOST]).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);

    // What the person saw must be what waits.
    for (name, hosts) in [
        ("N3_DEV_CI_TOKEN_AGENT", vec!["other.example.test"]),
        ("ELSE", vec![HOST]),
    ] {
        let stale = answer(&w, &w.writer, true, name, &hosts).await?;
        assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    }

    // Allowing needs a fresh two-factor unlock, as a reveal does; nothing is created before it.
    // An API token never allows one (no session to unlock).
    let token = answer(&w, &w.writer, true, "N3_DEV_CI_TOKEN_AGENT", &[HOST]).await?;
    assert_eq!(token.status, StatusCode::FORBIDDEN, "{}", token.body);
    let locked = answer(&w, &w.browser, true, "N3_DEV_CI_TOKEN_AGENT", &[HOST]).await?;
    assert_eq!(locked.status, StatusCode::FORBIDDEN, "{}", locked.body);
    assert_eq!(
        locked.body["error"]["code"], "reauth_required",
        "{}",
        locked.body
    );
    crate::secrets::enroll(&w.h, &w.browser).await?;
    let names: Vec<String> =
        w.h.store
            .vault_agent_in(&SandboxId::parse("s1")?)
            .await?
            .into_iter()
            .map(|r| r.name)
            .collect();
    assert!(
        !names.contains(&"N3_DEV_CI_TOKEN_AGENT".to_owned()),
        "{names:?}"
    );
    let allowed = answer(&w, &w.browser, true, "N3_DEV_CI_TOKEN_AGENT", &[HOST]).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(allowed.body["id"], w.prod.as_str(), "{}", allowed.body);
    assert!(
        allowed.body.get("copy_request").is_none(),
        "{}",
        allowed.body
    );
    assert!(!allowed.body.to_string().contains(VALUE));

    let records = w.h.store.vault_agent_in(&SandboxId::parse("s1")?).await?;
    let copy = records
        .iter()
        .find(|r| r.name == "N3_DEV_CI_TOKEN_AGENT")
        .ok_or("no copy")?;
    assert_eq!(copy.sandbox_id, Some(SandboxId::parse("s1")?));
    assert_eq!(copy.hosts, vec![HOST.to_owned()]);
    assert_eq!(copy.injection, "bearer");
    assert!(!copy.cd_prod && !copy.auto_allow);
    assert_eq!(copy.env_files, EnvFiles::No);
    assert_eq!(copy.created_by, "u-alice");
    assert!(
        copy.description.starts_with(&format!(
            "Copy of N3_DEV_CI_TOKEN for agents (asked by {me}"
        )),
        "{}",
        copy.description
    );
    let source = records.iter().find(|r| r.id == w.prod).ok_or("source")?;
    assert!(source.cd_prod && source.hosts.is_empty(), "{source:?}");
    assert_eq!(source.env_files, EnvFiles::No);

    // Its value is the production secret's.
    let revealed = call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{}/reveal", copy.id),
        Some(json!({"fields": ["value"]})),
        &w.browser,
    )
    .await?;
    assert_eq!(revealed.status, StatusCode::OK, "{}", revealed.body);
    assert_eq!(revealed.body["value"], VALUE);

    // The task works on; the copy is listed for agents; the request is answered once.
    assert_eq!(activity(&w, "t1").await?, Some(Activity::Working));
    let list = support::mcp_tool(&w.h, TOKEN, "secrets_list", json!({})).await?;
    let list = support::tool_text(&list);
    let entry = list["secrets"]
        .as_array()
        .ok_or("secrets")?
        .iter()
        .find(|s| s["name"] == "N3_DEV_CI_TOKEN_AGENT")
        .ok_or("copy not listed")?;
    assert_eq!(entry["hosts"], json!([HOST]), "{entry}");
    assert_eq!(entry["production"], false, "{entry}");
    let again = answer(&w, &w.writer, true, "N3_DEV_CI_TOKEN_AGENT", &[HOST]).await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);

    let activity_log = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    let log = activity_log.body.to_string();
    for action in [
        "vault.agent.copy_request",
        "vault.agent.copy_allow",
        "vault.agent.create",
    ] {
        assert!(log.contains(action), "{action}: {log}");
    }
    assert!(!log.contains(VALUE));
    Ok(())
}

/// SPEC-440 CPY-001, CPY-002, CPY-006 (AC-3, AC-4): the tool refuses what is not a copy of a
/// production secret, saying what to do instead; the production refusals point to it; asking the
/// same copy again adds the asker, asking another replaces it; a dismissal creates nothing,
/// needs no unlock and lets every asking task work on.
#[tokio::test]
async fn copies_are_asked_for_production_secrets_and_dismissed() -> TestResult {
    let w = world().await?;
    let refusal = |result: &Value| {
        assert_eq!(result["isError"], true, "{result}");
        support::tool_text(result)
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let not_production = ask(
        &w,
        TOKEN,
        json!({"secret": "STRIPE_KEY", "name": "STRIPE_COPY", "hosts": [HOST]}),
    )
    .await?;
    assert!(
        refusal(&not_production).contains("secret_request_hosts"),
        "{not_production}"
    );
    let unknown = ask(
        &w,
        TOKEN,
        json!({"secret": "NOPE", "name": "NOPE_COPY", "hosts": [HOST]}),
    )
    .await?;
    assert!(refusal(&unknown).contains("secrets_list"), "{unknown}");
    let taken = ask(
        &w,
        TOKEN,
        json!({"secret": "N3_DEV_CI_TOKEN", "name": "STRIPE_KEY", "hosts": [HOST]}),
    )
    .await?;
    assert!(refusal(&taken).contains("already applies"), "{taken}");
    let reserved = ask(
        &w,
        TOKEN,
        json!({"secret": "N3_DEV_CI_TOKEN", "name": "VIDOC_API_KEY", "hosts": [HOST]}),
    )
    .await?;
    assert!(refusal(&reserved).contains("reserved"), "{reserved}");
    let no_hosts = ask(&w, TOKEN, copy_args(&[])).await?;
    assert!(
        refusal(&no_hosts).contains("at least one host"),
        "{no_hosts}"
    );
    assert!(
        w.h.store
            .vault_copy_requests(&SandboxId::parse("s1")?)
            .await?
            .is_empty()
    );
    assert_ne!(activity(&w, "t1").await?, Some(Activity::NeedsAttention));

    // CPY-006: the production refusal names the way.
    let hosts = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_request_hosts",
        json!({"secret": "N3_DEV_CI_TOKEN", "hosts": [HOST]}),
    )
    .await?;
    assert!(refusal(&hosts).contains("secret_request_copy"), "{hosts}");

    // CPY-002: the same copy again adds the asker; another copy replaces the request.
    let (one, two) = (asker(&w, "t1").await?, asker(&w, "t2").await?);
    ask(&w, TOKEN, copy_args(&[HOST])).await?;
    let joined = ask(&w, TOKEN_2, copy_args(&[HOST])).await?;
    assert_eq!(
        support::tool_text(&joined)["requested_by"],
        format!("{one}, {two}"),
        "{joined}"
    );
    let again = ask(&w, TOKEN, copy_args(&[HOST])).await?;
    assert_eq!(
        support::tool_text(&again)["requested_by"],
        format!("{one}, {two}"),
        "an asker is named once"
    );
    assert_eq!(activity(&w, "t1").await?, Some(Activity::NeedsAttention));
    assert_eq!(activity(&w, "t2").await?, Some(Activity::NeedsAttention));
    let replaced = ask(&w, TOKEN_2, copy_args(&["dev.example.test"])).await?;
    assert_eq!(
        support::tool_text(&replaced)["requested_by"],
        two,
        "{replaced}"
    );
    let seen = listed(&w, &w.writer).await?;
    assert_eq!(seen["copy_request"]["hosts"], json!(["dev.example.test"]));
    // t1's request is gone with the replaced one: it works on once the store says so.
    assert_eq!(activity(&w, "t2").await?, Some(Activity::NeedsAttention));

    // A dismissal needs no unlock and creates nothing.
    let dismissed = answer(
        &w,
        &w.writer,
        false,
        "N3_DEV_CI_TOKEN_AGENT",
        &["dev.example.test"],
    )
    .await?;
    assert_eq!(dismissed.status, StatusCode::OK, "{}", dismissed.body);
    assert!(dismissed.body.get("copy_request").is_none());
    assert!(
        w.h.store
            .vault_copy_requests(&SandboxId::parse("s1")?)
            .await?
            .is_empty()
    );
    assert!(
        !w.h.store
            .vault_agent_in(&SandboxId::parse("s1")?)
            .await?
            .iter()
            .any(|r| r.name == "N3_DEV_CI_TOKEN_AGENT")
    );
    assert_eq!(activity(&w, "t1").await?, Some(Activity::Working));
    assert_eq!(activity(&w, "t2").await?, Some(Activity::Working));
    let nothing = answer(
        &w,
        &w.writer,
        false,
        "N3_DEV_CI_TOKEN_AGENT",
        &["dev.example.test"],
    )
    .await?;
    assert_eq!(nothing.status, StatusCode::CONFLICT, "{}", nothing.body);

    // CPY-007: deleting the production secret removes its requests, and the task works on.
    ask(&w, TOKEN, copy_args(&[HOST])).await?;
    assert_eq!(activity(&w, "t1").await?, Some(Activity::NeedsAttention));
    let deleted = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/agent-secrets/{}", w.prod),
        None,
        &w.writer,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.body);
    assert!(
        w.h.store
            .vault_copy_requests(&SandboxId::parse("s1")?)
            .await?
            .is_empty()
    );
    assert_eq!(activity(&w, "t1").await?, Some(Activity::Working));
    Ok(())
}
