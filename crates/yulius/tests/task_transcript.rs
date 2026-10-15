//! A task's live transcript for the people who watch it (SPEC-179 WAT-001, AC-WAT-1): the tail of
//! the latest session, reading on from `end` as the file grows, an unchanged file answered without
//! reading it, redaction as on session pages, read access, and a task without a session.

use crate::support;

use std::path::Path;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::{OWNER, World};
use support::{Creds, TestResult, call};
use yulius_app::model::{ProviderSessionRecord, TaskMode, TaskOrigin, TaskRecord};
use yulius_app::ports::store::{SandboxStore, TaskStore};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::spec_format::{TemplateKind, scaffold_files};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

/// A synthetic GitHub token built at run time (no literal credential in the source).
fn fake_token() -> String {
    format!("ghp_{}", "Wt4Ky".repeat(8))
}

fn line(value: &Value) -> String {
    format!("{value}\n")
}

fn prompt(text: &str) -> String {
    line(&json!({"type": "user", "message": {"role": "user", "content": text}}))
}

fn answer(text: &str) -> String {
    line(&json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}}))
}

fn append(path: &Path, text: &str) -> TestResult {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

async fn insert_task(
    store: &impl TaskStore,
    sandbox: &SandboxId,
    id: &str,
    owner: &UserId,
) -> Result<TaskRecord, Error> {
    Ok(store
        .insert_task(&TaskRecord {
            id: TaskId::parse(id)?,
            sandbox_id: sandbox.clone(),
            number: 0,
            title: "Live cart".into(),
            prompt: "Live cart".into(),
            harness: Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: Some(format!("feat/{id}")),
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: owner.clone(),
            owner: owner.clone(),
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

/// A lineage entry whose transcript is read from `transcript` (no native copy exists here).
async fn session(
    store: &impl TaskStore,
    task: &TaskRecord,
    session: &str,
    first_seen: i64,
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
            first_seen: Timestamp(first_seen),
            last_seen: Timestamp(first_seen + 1),
        })
        .await?;
    Ok(())
}

async fn get(w: &World, creds: &Creds, path: &str) -> Result<support::Resp, Error> {
    let resp = call(&w.h.router, Method::GET, path, None, creds).await?;
    Ok(resp)
}

fn lines_of(body: &Value) -> Vec<u64> {
    body["entries"]
        .as_array()
        .map(|entries| entries.iter().filter_map(|e| e["line"].as_u64()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn watchers_read_the_tail_of_a_task_s_live_session_and_follow_it() -> TestResult {
    let w = World::new("org").await?;
    let (alice_user, alice) = w.user("alice", false).await?;
    let (_, carol) = w.user("carol", false).await?;
    let (_, bob) = w.user("bob", false).await?;
    let files = scaffold_files("live", TemplateKind::Empty);
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect();
    w.remote(OWNER, "live", &refs, &[])?;
    w.grant("alice", "live", "admin");
    w.grant("carol", "live", "pull");
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "import", "repo": "Quartio-Studio/live"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let sid = SandboxId::parse(created.body["id"].as_str().ok_or("id")?)?;
    w.h.store.set_onboarding(&sid, "ready", None).await?;

    // A task that has not started yet: nothing to read, said plainly.
    let task = insert_task(&w.h.store, &sid, "t-live", &alice_user.id).await?;
    let path = format!("/api/tasks/{}/transcript", task.id);
    let empty = get(&w, &carol, &path).await?;
    assert_eq!(empty.status, StatusCode::OK, "{:?}", empty.body);
    assert_eq!(empty.body["session_id"], Value::Null);
    assert_eq!(empty.body["entries"], json!([]));
    assert_eq!((empty.body["end"].clone(), empty.body["bytes"].clone()), (json!(0), json!(0)));

    // An earlier session and the latest one: the watcher reads the latest.
    let old = w.scratch.path().join("old.jsonl");
    std::fs::write(&old, prompt("The first try"))?;
    session(&w.h.store, &task, "sess-old", 10, &old).await?;
    let live = w.scratch.path().join("live.jsonl");
    let mut text = String::new();
    for n in 0..5 {
        text.push_str(&prompt(&format!("Step {n}")));
        text.push_str(&answer(&format!("Done step {n}")));
    }
    text.push_str(&answer(&format!("the deploy token is {}", fake_token())));
    // A line still being written: not read until it is complete.
    text.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"half");
    std::fs::write(&live, &text)?;
    session(&w.h.store, &task, "sess-live", 20, &live).await?;

    let tail = get(&w, &carol, &format!("{path}?limit=4")).await?;
    assert_eq!(tail.status, StatusCode::OK, "{:?}", tail.body);
    assert_eq!(tail.body["session_id"], "sess-live");
    let doc = tail.body["doc_id"].as_str().ok_or("the session's document")?;
    let opened = get(&w, &carol, &format!("/api/sessions/{doc}")).await?;
    assert_eq!(opened.body["session_id"], "sess-live");
    assert_eq!(lines_of(&tail.body), vec![7, 8, 9, 10]);
    assert_eq!(tail.body["earlier"], true);
    assert_eq!(tail.body["next"], Value::Null);
    assert_eq!(tail.body["end"], 11);
    assert_eq!(tail.body["bytes"], text.len());
    let body = tail.body.to_string();
    assert!(
        !body.contains(&fake_token()) && body.contains("[REDACTED:github-token]"),
        "redacted as session pages are: {body}"
    );

    // Nothing changed: answered from the file's size, without entries.
    let seen = text.len();
    let same = get(&w, &carol, &format!("{path}?from=11&seen={seen}&session=sess-live")).await?;
    assert_eq!(same.status, StatusCode::OK, "{:?}", same.body);
    assert_eq!(same.body["entries"], json!([]));
    assert_eq!((same.body["end"].clone(), same.body["bytes"].clone()), (json!(11), json!(seen)));

    // The line is finished and another comes: read from `end`, the size moves on.
    append(&live, " done\"}]}}\n")?;
    append(&live, &prompt("Ship it"))?;
    let grown = get(&w, &carol, &format!("{path}?from=11&seen={seen}&session=sess-live")).await?;
    assert_eq!(lines_of(&grown.body), vec![11, 12]);
    assert_eq!(grown.body["end"], 13);
    assert!(grown.body["bytes"].as_u64().unwrap_or_default() > seen as u64);

    // A size seen for another session is no reason to skip the read.
    let other = get(&w, &carol, &format!("{path}?from=0&seen={seen}&session=sess-old")).await?;
    assert_eq!(other.body["entries"].as_array().map(Vec::len), Some(13));
    assert_eq!(other.body["session_id"], "sess-live");

    // Read access is enough; no access sees nothing.
    let owner = get(&w, &alice, &path).await?;
    assert_eq!(owner.status, StatusCode::OK);
    let hidden = get(&w, &bob, &path).await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let anonymous = get(&w, &Creds::default(), &path).await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let unknown = get(&w, &carol, "/api/tasks/t-nothing/transcript").await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    Ok(())
}
