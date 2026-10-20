//! Sessions as read-only documents: the Sessions API by sandbox access, full-text search,
//! redaction on serve, the agent tools `sessions_search`/`session_read` scoped to the agent's
//! sandbox, the onboarding plan of `yulius onboard` and the onboarding gate (AC-SES-1..5,
//! AC-IMP-4, AC-IMP-7).

use crate::support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::{OWNER, World};
use support::{Creds, TestResult, call, harness};
use yulius_app::model::{
    AttemptRecord, LaunchKind, ProviderSessionRecord, SandboxRecord, Sealed, TaskMode, TaskOrigin,
    TaskRecord, UserRecord,
};
use yulius_app::ports::session_docs::{SessionDocRecord, SessionDocStore, SessionKind};
use yulius_app::ports::store::{AccountStore, SandboxStore, TaskStore, UserStore};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::spec_format::{TemplateKind, scaffold_files};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// An agent secret's value (VLT): plain text no pattern catches, only the vault's matcher.
const AGENT_VALUE: &str = "plainvaultvalue42xyz";

/// Stores `AGENT_VALUE` as the sandbox's agent secret `DEPLOY_KEY`.
async fn agent_secret(
    app: &yulius_app::context::App,
    user: &UserRecord,
    sandbox: &SandboxId,
) -> TestResult {
    let ctx = yulius_app::auth::AuthContext {
        user: user.clone(),
        csrf_token: None,
        session_hash: None,
    };
    app.create_agent_secret(
        &ctx,
        sandbox.as_str(),
        &yulius_contracts::api::secrets::AgentSecretInput {
            name: "DEPLOY_KEY".into(),
            hosts: vec!["deploy.example.test".into()],
            value: AGENT_VALUE.into(),
            ..Default::default()
        },
    )
    .await?;
    Ok(())
}

/// A synthetic GitHub token built at run time (no literal credential in the source).
fn fake_token() -> String {
    format!("ghp_{}", "Zq7Rx".repeat(8))
}

fn line(value: &Value) -> String {
    format!("{value}\n")
}

/// A task transcript: a prompt, an assistant answer about the stripe webhook, a token pasted by
/// someone, a `.env` read and an edit.
fn task_transcript() -> String {
    [
        line(&json!({"type": "user", "timestamp": "2026-10-02T09:00:00.000Z", "message": {"role": "user", "content": "Handle Stripe webhooks"}})),
        line(&json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "text", "text": "Implemented the stripe webhook handler with idempotency keys."},
            {"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "/w/.env.local"}}
        ]}})),
        line(&json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "r1", "content": "     1→WEBHOOK_SIGNING=whsec_plain\n"}
        ]}})),
        line(&json!({"type": "user", "message": {"role": "user", "content": format!("use this token {} and the deploy code {AGENT_VALUE}", fake_token())}})),
        line(&json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "src/webhooks.ts", "old_string": "todo()", "new_string": "verifySignature(req)"}}
        ]}})),
    ]
    .concat()
}

fn imported_transcript(topic: &str) -> String {
    [
        line(&json!({"type": "user", "message": {"role": "user", "content": format!("How does the {topic} work?")}})),
        line(&json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "text", "text": format!("The {topic} retries three times with backoff.")}
        ]}})),
    ]
    .concat()
}

fn write(path: &Path, contents: &str) -> TestResult {
    std::fs::create_dir_all(path.parent().ok_or("parent")?)?;
    std::fs::write(path, contents)?;
    Ok(())
}

async fn insert_task(
    store: &impl TaskStore,
    sandbox: &SandboxId,
    id: &str,
    title: &str,
    creator: &UserId,
    status: TaskStatus,
) -> Result<TaskRecord, Error> {
    Ok(store
        .insert_task(&TaskRecord {
            id: TaskId::parse(id)?,
            sandbox_id: sandbox.clone(),
            number: 0,
            title: title.into(),
            prompt: title.into(),
            harness: Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status,
            activity: None,
            branch: Some(format!("feat/{id}")),
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: creator.clone(),
            owner: creator.clone(),
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: TaskMode::Prompt,
            supertask: false,
        })
        .await?)
}

/// A task session read from a stored copy (as for a finished or imported lineage entry).
async fn task_session(
    store: &(impl TaskStore + ?Sized),
    task: &TaskRecord,
    session: &str,
    transcript: &Path,
) -> TestResult {
    store
        .upsert_provider_session(&ProviderSessionRecord {
            id: format!("p-{session}"),
            sandbox_id: task.sandbox_id.clone(),
            task_id: task.id.clone(),
            harness: Harness::ClaudeCode,
            session_id: session.into(),
            transcript_ref: Some(transcript.to_string_lossy().into_owned()),
            origin: "native".into(),
            first_seen: Timestamp(10),
            last_seen: Timestamp(20),
        })
        .await?;
    Ok(())
}

/// An imported session document with its stored transcript.
async fn imported_doc(
    store: &(impl SessionDocStore + ?Sized),
    data: &Path,
    sandbox: &SandboxId,
    id: &str,
    title: &str,
    author: &UserId,
    transcript: &str,
) -> Result<PathBuf, Error> {
    let path = data
        .join("sandboxes")
        .join(sandbox.as_str())
        .join("sessions")
        .join(id)
        .join(format!("sess-{id}.jsonl"));
    write(&path, transcript)?;
    store
        .insert_session_doc(&SessionDocRecord {
            id: id.into(),
            sandbox_id: sandbox.clone(),
            kind: SessionKind::Imported,
            task_id: None,
            harness: Harness::ClaudeCode,
            session_id: format!("sess-{id}"),
            title: title.into(),
            author_id: Some(author.clone()),
            git_branch: Some("main".into()),
            cwd_relative: None,
            base: None,
            transcript_path: path.to_string_lossy().into_owned(),
            sha256: Some("c".repeat(64)),
            bytes: transcript.len() as u64,
            redactions: 2,
            started_at: Some(Timestamp(30)),
            ended_at: Some(Timestamp(40)),
            continued_task_id: None,
            indexed_bytes: 0,
            indexed_lines: 0,
            index_full: false,
            created_at: Timestamp(50),
            updated_at: Timestamp(50),
        })
        .await?;
    Ok(path)
}

async fn get(w: &World, creds: &Creds, path: &str) -> Result<support::Resp, Error> {
    call(&w.h.router, Method::GET, path, None, creds).await
}

fn ready_repo(w: &World, name: &str) -> TestResult {
    let files = scaffold_files(name, TemplateKind::Empty);
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect();
    w.remote(OWNER, name, &refs, &[])?;
    Ok(())
}

async fn import(w: &World, creds: &Creds, repo: &str) -> Result<Value, Error> {
    let resp = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "import", "repo": repo})),
        creds,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::CREATED, "{:?}", resp.body);
    Ok(resp.body)
}

#[tokio::test]
async fn people_list_search_and_read_sessions_by_sandbox_access() -> TestResult {
    let w = World::new("org").await?;
    let (alice_user, alice) = w.user("alice", false).await?;
    let (_, carol) = w.user("carol", false).await?;
    let (_, bob) = w.user("bob", false).await?;
    ready_repo(&w, "ready")?;
    w.grant("alice", "ready", "admin");
    w.grant("carol", "ready", "pull");
    let ready = import(&w, &alice, "Quartio-Studio/ready").await?;
    let sid = SandboxId::parse(text(&ready["id"]))?;
    // Its environment task (F 2026-10-11) has finished: this test is about sessions.
    w.h.store.set_onboarding(&sid, "ready", None).await?;
    agent_secret(&w.h.app, &alice_user, &sid).await?;
    let task = insert_task(
        &w.h.store,
        &sid,
        "t-webhooks",
        "Stripe webhooks",
        &alice_user.id,
        TaskStatus::Closed,
    )
    .await?;
    let stored = w.scratch.path().join("task-copy.jsonl");
    write(&stored, &task_transcript())?;
    task_session(&w.h.store, &task, "sess-task", &stored).await?;
    imported_doc(
        &w.h.store,
        &w.data_dir(),
        &sid,
        "doc-queue",
        "How does the queue work?",
        &alice_user.id,
        &imported_transcript("queue"),
    )
    .await?;

    // The list holds both kinds, newest first, with authors and the task.
    let list = get(&w, &alice, &format!("/api/sandboxes/{sid}/sessions")).await?;
    assert_eq!(list.status, StatusCode::OK, "{:?}", list.body);
    let sessions = list.body["sessions"].as_array().ok_or("sessions")?.clone();
    assert_eq!(sessions.len(), 2);
    assert_eq!(list.body["authors"], json!(["alice"]));
    let task_doc = sessions
        .iter()
        .find(|s| s["kind"] == "task")
        .ok_or("task doc")?;
    assert_eq!(task_doc["task"]["number"], task.number);
    assert_eq!(task_doc["title"], "Stripe webhooks");
    assert_eq!(task_doc["author"]["login"], "alice");
    assert_eq!(
        task_doc["can_continue"], false,
        "a task continues from its task"
    );
    let imported = sessions
        .iter()
        .find(|s| s["kind"] == "imported")
        .ok_or("imported")?;
    assert_eq!(imported["redactions"], 2);
    assert_eq!(imported["can_delete"], true);
    let task_doc_id = text(&task_doc["id"]).to_owned();
    let only = get(
        &w,
        &alice,
        &format!("/api/sandboxes/{sid}/sessions?kind=imported"),
    )
    .await?;
    assert_eq!(only.body["sessions"].as_array().map(Vec::len), Some(1));
    let bad = get(&w, &alice, &format!("/api/sandboxes/{sid}/sessions?kind=x")).await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Full-text search finds passages with marked matches; secrets are not searchable.
    let found = get(
        &w,
        &carol,
        &format!("/api/sandboxes/{sid}/sessions?q=webhook%20idempotency"),
    )
    .await?;
    assert_eq!(found.status, StatusCode::OK, "{:?}", found.body);
    assert_eq!(found.body["query"], "webhook idempotency");
    let hits = &found.body["sessions"][0]["hits"];
    assert_eq!(found.body["sessions"][0]["id"], task_doc_id.as_str());
    assert_eq!(hits[0]["line"], 1);
    assert!(
        hits[0]["snippet"]
            .as_array()
            .ok_or("snippet")?
            .iter()
            .any(|part| part["hit"] == true && text(&part["text"]).eq_ignore_ascii_case("webhook"))
    );
    let queue = get(
        &w,
        &alice,
        &format!("/api/sandboxes/{sid}/sessions?q=backoff"),
    )
    .await?;
    assert_eq!(queue.body["sessions"][0]["id"], "doc-queue");
    let secret = get(
        &w,
        &alice,
        &format!("/api/sandboxes/{sid}/sessions?q={}", fake_token()),
    )
    .await?;
    assert_eq!(
        secret.body["sessions"],
        json!([]),
        "redacted before indexing"
    );

    // The read-only transcript: messages, tool calls with diffs, results; secrets redacted.
    let page = get(
        &w,
        &carol,
        &format!("/api/sessions/{task_doc_id}/transcript"),
    )
    .await?;
    assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
    let body = page.body.to_string();
    assert!(
        !body.contains(&fake_token()),
        "the pasted token is redacted"
    );
    assert!(
        !body.contains("whsec_plain"),
        "a .env file read is redacted"
    );
    assert!(body.contains("[REDACTED:github-token]"));
    assert!(
        !body.contains(AGENT_VALUE) && body.contains("[secret:DEPLOY_KEY]"),
        "the sandbox's agent secrets are redacted by the vault: {body}"
    );
    let entries = page.body["entries"].as_array().ok_or("entries")?;
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[1]["blocks"][1]["type"], "tool_call");
    assert_eq!(entries[1]["blocks"][1]["summary"], "/w/.env.local");
    assert_eq!(
        entries[4]["blocks"][0]["diff"]["hunks"][0]["new"],
        "verifySignature(req)"
    );
    assert_eq!(page.body["next"], Value::Null);
    let paged = get(
        &w,
        &carol,
        &format!("/api/sessions/{task_doc_id}/transcript?from=1&limit=2"),
    )
    .await?;
    assert_eq!(paged.body["entries"][0]["line"], 1);
    assert_eq!(paged.body["next"], 3);

    // Read access reads; continuing and deleting need more; no access sees nothing.
    let carol_view = get(&w, &carol, "/api/sessions/doc-queue").await?;
    assert_eq!(carol_view.body["can_continue"], false);
    assert_eq!(carol_view.body["can_delete"], false);
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/sessions/doc-queue/continue",
        None,
        &carol,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let refused = call(
        &w.h.router,
        Method::DELETE,
        "/api/sessions/doc-queue",
        None,
        &carol,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    for path in [
        format!("/api/sandboxes/{sid}/sessions"),
        "/api/sessions/doc-queue".to_owned(),
        format!("/api/sessions/{task_doc_id}/transcript"),
    ] {
        let hidden = get(&w, &bob, &path).await?;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{path}");
    }
    let anonymous = get(&w, &Creds::default(), "/api/sessions/doc-queue").await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // Continuing an imported session starts an agent: without a usable Claude account it is
    // refused and creates no task (SPEC-246 CLA-002).
    w.h.store
        .delete_account(&yulius_domain::ids::AccountId::parse("a-exhausted")?)
        .await?;
    let tasks_before = w.h.store.tasks(&sid, true).await?.len();
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/sessions/doc-queue/continue",
        None,
        &alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert_eq!(refused.body["error"]["code"], "claude_account_required");
    assert_eq!(w.h.store.tasks(&sid, true).await?.len(), tasks_before);
    assert_eq!(
        get(&w, &alice, "/api/sessions/doc-queue").await?.body["continued_in"],
        Value::Null
    );

    // The uploader deletes an imported session (files and index); a task session stays.
    let not_task = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/sessions/{task_doc_id}"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(not_task.status, StatusCode::UNPROCESSABLE_ENTITY);
    let deleted = call(
        &w.h.router,
        Method::DELETE,
        "/api/sessions/doc-queue",
        None,
        &alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(
        !w.data_dir()
            .join("sandboxes")
            .join(sid.as_str())
            .join("sessions/doc-queue")
            .exists()
    );
    let gone = get(
        &w,
        &alice,
        &format!("/api/sandboxes/{sid}/sessions?q=backoff"),
    )
    .await?;
    assert_eq!(gone.body["sessions"], json!([]));
    Ok(())
}

#[tokio::test]
async fn no_task_starts_before_onboarding_is_ready_and_the_plan_says_why() -> TestResult {
    let w = World::new("org").await?;
    let (alice_user, alice) = w.user("alice", false).await?;
    let (carol_user, carol) = w.user("carol", false).await?;
    w.remote(OWNER, "shop", &[("README.md", "# Shop\n")], &[])?;
    w.grant("alice", "shop", "admin");
    w.grant("alice", "fresh", "maintain");
    w.remote(OWNER, "fresh", &[("README.md", "# Fresh\n")], &[])?;
    w.grant("carol", "shop", "push");
    // Added in the browser, so onboarding starts at once (SPEC-051).
    let alice_browser = w.browser(&alice_user).await?;
    let shop = import(&w, &alice_browser, "Quartio-Studio/shop").await?;
    assert_eq!(shop["onboarding"], "preparing");
    let sid = SandboxId::parse(text(&shop["id"]))?;

    // While preparing, continuing a resting task is refused; new tasks, chained or not, wait
    // after the preparation task (U-08, F 2026-10-11).
    let earlier = insert_task(
        &w.h.store,
        &sid,
        "t-earlier",
        "Earlier work",
        &carol_user.id,
        TaskStatus::Closed,
    )
    .await?;
    let continued = call(
        &w.h.router,
        Method::POST,
        &format!("/api/tasks/{}/continue", earlier.id),
        Some(json!({})),
        &carol,
    )
    .await?;
    assert_eq!(
        continued.status,
        StatusCode::CONFLICT,
        "{:?}",
        continued.body
    );
    assert!(text(&continued.body["error"]["message"]).contains("being prepared"));
    let created = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/tasks"),
        Some(json!({"prompt": "add a cart", "harness": "claude"})),
        &carol,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["status"], "chained");
    assert_eq!(created.body["after"]["number"], 1);
    let prep =
        w.h.store
            .sandbox(&sid)
            .await?
            .ok_or("sandbox")?
            .onboarding_task_id
            .ok_or("prep")?;
    let chained = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/tasks"),
        Some(json!({"prompt": "after it", "harness": "claude", "after_task_id": prep.as_str()})),
        &carol,
    )
    .await?;
    assert_eq!(chained.status, StatusCode::CREATED, "{:?}", chained.body);
    assert_eq!(chained.body["status"], "chained");

    // The CLI's plan shows the state and what is pending, and which sessions are needed.
    let digest = "d".repeat(64);
    let plan = call(
        &w.h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": OWNER, "name": "shop", "sessions": [
            {"harness": "claude", "session_id": "s-1", "sha256": digest, "size": 10}
        ]})),
        &carol,
    )
    .await?;
    assert_eq!(plan.status, StatusCode::OK, "{:?}", plan.body);
    assert_eq!(plan.body["login"], "carol");
    let onboarding = &plan.body["sandbox"]["onboarding"];
    assert_eq!(onboarding["state"], "preparing");
    assert_eq!(onboarding["ready"], false);
    assert_eq!(onboarding["task"]["number"], 1);
    assert!(
        !onboarding["pending"]
            .as_array()
            .ok_or("pending")?
            .is_empty(),
        "{onboarding:?}"
    );
    assert_eq!(
        plan.body["sandbox"]["sessions_url"],
        "https://yulius.test/s/shop/sessions"
    );
    assert_eq!(plan.body["sessions"][0]["state"], "new");
    assert_eq!(plan.body["refusal"], Value::Null);
    let fresh = call(
        &w.h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": OWNER, "name": "fresh"})),
        &alice,
    )
    .await?;
    assert_eq!(fresh.body["sandbox"], Value::Null);
    assert_eq!(fresh.body["can_create"], true);
    let denied = call(
        &w.h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": OWNER, "name": "fresh"})),
        &carol,
    )
    .await?;
    assert_eq!(denied.body["can_create"], false);
    assert!(denied.body["refusal"].is_string(), "{:?}", denied.body);
    let foreign = call(
        &w.h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": "someone-else", "name": "x"})),
        &alice,
    )
    .await?;
    assert!(text(&foreign.body["refusal"]).contains("only repositories of Quartio-Studio"));

    // Once an admin skips (or the preparation lands), tasks start again.
    let skip = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/onboarding/skip"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(skip.status, StatusCode::OK);
    let continued = call(
        &w.h.router,
        Method::POST,
        &format!("/api/tasks/{}/continue", earlier.id),
        Some(json!({})),
        &carol,
    )
    .await?;
    assert_eq!(continued.status, StatusCode::OK, "{:?}", continued.body);
    let plan = call(
        &w.h.router,
        Method::POST,
        "/api/onboard/plan",
        Some(json!({"owner": OWNER, "name": "shop"})),
        &carol,
    )
    .await?;
    assert_eq!(plan.body["sandbox"]["onboarding"]["ready"], true);
    assert_eq!(plan.body["sandbox"]["onboarding"]["pending"], json!([]));
    Ok(())
}

async fn seed_agent(
    h: &support::Harness,
    sandbox: &str,
    task: &str,
    token: &str,
) -> Result<TaskRecord, Error> {
    let t = insert_task(
        &h.store,
        &SandboxId::parse(sandbox)?,
        task,
        &format!("task {task}"),
        &UserId::parse("u1")?,
        TaskStatus::Running,
    )
    .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse(&format!("a-{task}"))?,
            task_id: t.id.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
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
    Ok(t)
}

async fn mcp_tool(
    router: &axum::Router,
    token: &str,
    name: &str,
    args: Value,
) -> Result<(Value, bool), Error> {
    let r = call(
        router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args, "_meta": {"yulius/structured": true}}})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    let text = r.body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("text")?;
    let is_error = r.body["result"]["isError"].as_bool().unwrap_or(true);
    // The tool's record (asked for with `_meta`), or the text of an error.
    let record = r.body["result"]
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| Value::String(text.to_owned()));
    Ok((record, is_error))
}

#[tokio::test]
async fn agents_search_and_read_only_their_own_sandbox_sessions() -> TestResult {
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
    for (id, slug, port) in [("s1", "studio", 20000), ("s2", "product", 20020)] {
        h.store
            .insert_sandbox(&SandboxRecord {
                id: SandboxId::parse(id)?,
                slug: slug.into(),
                repo_owner: OWNER.into(),
                repo_name: slug.into(),
                github_repo_id: None,
                default_branch: "main".into(),
                profile_json: "{}".into(),
                port_base: port,
                port_count: 20,
                onboarding_state: "not_needed".into(),
                onboarding_task_id: None,
                created_by: UserId::parse("u1")?,
                created_at: Timestamp(1),
                removed_at: None,
            })
            .await?;
    }
    let data = h.app.config.data_dir.clone();
    let me = seed_agent(&h, "s1", "me", "tok-me").await?;
    let other = insert_task(
        &h.store,
        &SandboxId::parse("s1")?,
        "other",
        "Stripe webhooks",
        &UserId::parse("u1")?,
        TaskStatus::Closed,
    )
    .await?;
    let other_copy = data.join("copies/other.jsonl");
    write(&other_copy, &task_transcript())?;
    task_session(&h.store, &other, "sess-other", &other_copy).await?;
    let mine_copy = data.join("copies/me.jsonl");
    write(&mine_copy, &imported_transcript("webhook"))?;
    task_session(&h.store, &me, "sess-me", &mine_copy).await?;
    let author = UserId::parse("u1")?;
    imported_doc(
        &h.store,
        &data,
        &SandboxId::parse("s1")?,
        "doc-s1",
        "Queue design",
        &author,
        &imported_transcript("queue"),
    )
    .await?;
    imported_doc(
        &h.store,
        &data,
        &SandboxId::parse("s2")?,
        "doc-s2",
        "Secret queue of the other project",
        &author,
        &imported_transcript("queue"),
    )
    .await?;
    let owner = UserRecord {
        id: UserId::parse("u-owner")?,
        github_login: Some("owner".into()),
        github_id: None,
        display_name: "Owner".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    h.store.insert_user(&owner).await?;
    agent_secret(&h.app, &owner, &SandboxId::parse("s1")?).await?;
    let internal = yulius_server::internal_router(h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));

    // Search: task transcripts and imported sessions of this sandbox only, not the asker's own.
    let (found, err) = mcp_tool(
        &internal,
        "tok-me",
        "sessions_search",
        json!({"query": "webhook"}),
    )
    .await?;
    assert!(!err, "{found}");
    let results = found["results"].as_array().ok_or("results")?;
    assert_eq!(results.len(), 1, "{found}");
    assert_eq!(results[0]["task_number"], other.number);
    assert_eq!(results[0]["kind"], "task");
    assert!(
        text(&results[0]["matches"][0]["text"])
            .to_lowercase()
            .contains("«webhook")
    );
    let (found, _) = mcp_tool(
        &internal,
        "tok-me",
        "sessions_search",
        json!({"query": "queue backoff"}),
    )
    .await?;
    let ids: Vec<&str> = found["results"]
        .as_array()
        .ok_or("results")?
        .iter()
        .map(|r| text(&r["id"]))
        .collect();
    assert_eq!(ids, vec!["doc-s1"], "never another sandbox's sessions");
    let (found, _) = mcp_tool(
        &internal,
        "tok-me",
        "sessions_search",
        json!({"query": "queue", "kind": "task"}),
    )
    .await?;
    assert_eq!(found["results"], json!([]));
    let (missing, err) = mcp_tool(&internal, "tok-me", "sessions_search", json!({})).await?;
    assert!(err, "{missing}");

    // Read: by id or task number, bounded, redacted; another sandbox's document is not found.
    let (read, err) =
        mcp_tool(&internal, "tok-me", "session_read", json!({"id": "doc-s1"})).await?;
    assert!(!err, "{read}");
    assert!(text(&read["text"]).contains("The queue retries three times"));
    assert_eq!(read["next_from"], Value::Null);
    let (by_task, _) = mcp_tool(
        &internal,
        "tok-me",
        "session_read",
        json!({"task_number": other.number, "from": 1, "limit": 2}),
    )
    .await?;
    let body = text(&by_task["text"]);
    assert!(body.starts_with("[line 1] assistant"), "{body}");
    assert!(body.contains("→ Read /w/.env.local"));
    assert!(body.contains("WEBHOOK_SIGNING=[REDACTED:"), "{body}");
    assert!(!body.contains("whsec_plain"));
    assert_eq!(by_task["next_from"], 3);
    let (whole, _) = mcp_tool(
        &internal,
        "tok-me",
        "session_read",
        json!({"task_number": other.number}),
    )
    .await?;
    let whole = text(&whole["text"]);
    assert!(
        !whole.contains(AGENT_VALUE) && whole.contains("[secret:DEPLOY_KEY]"),
        "{whole}"
    );
    let (bounded, _) = mcp_tool(
        &internal,
        "tok-me",
        "session_read",
        json!({"task_number": other.number, "max_chars": 1000}),
    )
    .await?;
    assert!(text(&bounded["text"]).len() <= 1100);
    let (foreign, err) =
        mcp_tool(&internal, "tok-me", "session_read", json!({"id": "doc-s2"})).await?;
    assert!(err, "{foreign}");
    let (nothing, err) = mcp_tool(&internal, "tok-me", "session_read", json!({})).await?;
    assert!(err, "{nothing}");

    // The tools are listed and announced.
    let r = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})),
        &Creds {
            bearer: Some("tok-me".into()),
            ..Creds::default()
        },
    )
    .await?;
    let names: Vec<&str> = r.body["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"sessions_search") && names.contains(&"session_read"));
    Ok(())
}
