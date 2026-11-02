//! A deleted task's attachments (SPEC-396, ADR-0253): a folder another task's request names stays
//! where it is, any other waits 7 days in the sandbox's trash, from which `attachments_trash`
//! restores its files into a task; `task_update` changes a waiting task's start in place.

use crate::support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use support::{
    TestResult, add_sandbox, harness_configured, mcp_tool, perms, seed_live_attempt, seed_task,
    seed_task_with_prompt, tool_text, user_with_token,
};
use yulius_app::ports::store::TaskStore;
use yulius_app::ports::store_ext::AutomationStore;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

type Error = Box<dyn std::error::Error>;

const DAY_SECS: i64 = 24 * 3600;

/// Sandbox `s1` with alice (write) and bob (read); their tasks `t-agent` and `t-bob` run with the
/// task tokens `tok-agent` and `tok-bob`.
async fn world() -> Result<(support::Harness, PathBuf), Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    support::exhausted_subscription(&h).await?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    user_with_token(&h, "u-alice", "alice", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    seed_task(&h, "t-agent", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    user_with_token(&h, "u-bob", "bob", false).await?;
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    seed_task(&h, "t-bob", "s1", "u-bob", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-bob", "tok-bob").await?;
    let attachments = h.app.config.data_dir.join("sandboxes/s1/attachments");
    std::fs::create_dir_all(&attachments)?;
    Ok((h, attachments))
}

/// Puts `name` with `body` among task `task`'s attachments; returns its path.
fn attach(attachments: &Path, task: &str, name: &str, body: &str) -> Result<PathBuf, Error> {
    let folder = attachments.join("tasks").join(task);
    std::fs::create_dir_all(&folder)?;
    std::fs::write(folder.join(name), body)?;
    Ok(folder.join(name))
}

/// The names in the trash folder.
fn trash(attachments: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(attachments.join("trash"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn number(h: &support::Harness, id: &str) -> Result<u32, Error> {
    Ok(h.store
        .task(&TaskId::parse(id)?)
        .await?
        .ok_or("task")?
        .number)
}

async fn tool(h: &support::Harness, name: &str, args: Value) -> Result<(Value, bool), Error> {
    let result = mcp_tool(h, "tok-agent", name, args).await?;
    Ok((
        tool_text(&result),
        result["isError"].as_bool().unwrap_or(true),
    ))
}

/// AC ATR-001..003: the incident of 2026-11-02. #63's request names #61's screenshot, so deleting
/// #61 keeps it at its path and says so; deleting #63 then sends #61's folder to the trash.
#[tokio::test]
async fn attachments_another_request_names_stay_until_the_last_naming_task_goes() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t61", "s1", "u-alice", TaskStatus::Prepared, None).await?;
    let shot = attach(&attachments, "t61", "pasted-20261101-141114.png", "png")?;
    let prompt = format!("Fix the layout shown in {}", shot.display());
    seed_task_with_prompt(
        &h,
        "t63",
        "s1",
        "u-alice",
        TaskStatus::Prepared,
        None,
        &prompt,
    )
    .await?;
    let n63 = number(&h, "t63").await?;

    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t61"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"]["kept_for"],
        json!([n63]),
        "{deleted}"
    );
    assert_eq!(
        std::fs::read_to_string(&shot)?,
        "png",
        "the named file stays"
    );
    assert!(trash(&attachments).is_empty());

    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t63"})).await?;
    assert!(!is_error, "{deleted}");
    assert!(!shot.exists(), "nobody names it now");
    let entries = trash(&attachments);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry = entries.first().ok_or("entry")?;
    assert!(entry.ends_with("-0-t61"), "{entry}");
    assert_eq!(
        std::fs::read_to_string(
            attachments
                .join("trash")
                .join(entry)
                .join("pasted-20261101-141114.png")
        )?,
        "png"
    );
    Ok(())
}

/// AC ATR-002, ATR-005: a deleted task's folder nobody names goes to the trash; the tool lists it
/// and restores its files into another task under free names, then into the caller's own task.
#[tokio::test]
async fn a_deleted_tasks_attachments_are_restored_from_the_trash() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Prepared, None).await?;
    seed_task(&h, "t3", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let (n1, n2) = (number(&h, "t1").await?, number(&h, "t2").await?);
    attach(&attachments, "t1", "a.txt", "precious")?;
    attach(&attachments, "t2", "a.txt", "other")?;
    attach(&attachments, "t3", "b.txt", "mine")?;

    let (deleted, is_error) = tool(&h, "task_delete", json!({"number": n1})).await?;
    assert!(!is_error, "{deleted}");
    let gone = &deleted["deleted"]["attachments"]["trash"];
    assert_eq!(gone["task_id"], "t1", "{deleted}");
    assert_eq!(gone["number"], json!(n1), "{deleted}");
    assert_eq!(gone["files"], json!(["a.txt"]), "{deleted}");
    assert!(gone["restorable_until"].is_string(), "{deleted}");
    assert!(!attachments.join("tasks/t1").exists());

    let (listed, is_error) = tool(&h, "attachments_trash", json!({})).await?;
    assert!(!is_error, "{listed}");
    assert_eq!(listed["trash"][0]["task_id"], "t1", "{listed}");
    assert_eq!(listed["trash"][0]["files"], json!(["a.txt"]), "{listed}");

    let (unknown, is_error) = tool(&h, "attachments_trash", json!({"restore": "nope"})).await?;
    assert!(is_error, "{unknown}");

    let (restored, is_error) =
        tool(&h, "attachments_trash", json!({"restore": n1, "into": n2})).await?;
    assert!(!is_error, "{restored}");
    let into = attachments.join("tasks/t2/a-2.txt");
    assert_eq!(
        restored["restored"]["paths"],
        json!([std::path::absolute(&into)?]),
        "{restored}"
    );
    assert_eq!(std::fs::read_to_string(into)?, "precious");
    assert_eq!(
        std::fs::read_to_string(attachments.join("tasks/t2/a.txt"))?,
        "other"
    );
    assert!(trash(&attachments).is_empty(), "the emptied entry goes");
    let audit = h.store.automation_audit("task:t2", 10).await?;
    assert!(
        audit.iter().any(|e| e.action == "attachment.restored"),
        "{audit:?}"
    );

    // Without `into`, a task's agent restores into its own task.
    tool(&h, "task_delete", json!({"task_id": "t3"})).await?;
    let (restored, is_error) = tool(&h, "attachments_trash", json!({"restore": "t3"})).await?;
    assert!(!is_error, "{restored}");
    assert_eq!(
        std::fs::read_to_string(attachments.join("tasks/t-agent/b.txt"))?,
        "mine"
    );

    // Restoring needs write access: bob, who reads the project, sees the trash but restores
    // nothing.
    tool(&h, "task_delete", json!({"task_id": "t2"})).await?;
    let listed = mcp_tool(&h, "tok-bob", "attachments_trash", json!({})).await?;
    assert_eq!(listed["isError"], json!(false), "{listed}");
    let refused = mcp_tool(&h, "tok-bob", "attachments_trash", json!({"restore": "t2"})).await?;
    assert_eq!(refused["isError"], json!(true), "{refused}");
    assert_eq!(trash(&attachments).len(), 1);
    Ok(())
}

/// AC ATR-004: an entry older than 7 days is removed at the next listing; a younger one stays.
#[tokio::test]
async fn trash_entries_older_than_a_week_are_removed() -> TestResult {
    let (h, attachments) = world().await?;
    let now = h.app.now().0 / 1000;
    for (entry, age) in [("old", 8 * DAY_SECS), ("young", 6 * DAY_SECS)] {
        let folder = attachments
            .join("trash")
            .join(format!("{}-5-t-{entry}", now - age));
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join("x.txt"), entry)?;
    }
    let (listed, is_error) = tool(&h, "attachments_trash", json!({})).await?;
    assert!(!is_error, "{listed}");
    let entries = trash(&attachments);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(
        entries.iter().all(|e| e.ends_with("-5-t-young")),
        "{entries:?}"
    );
    assert_eq!(listed["trash"][0]["task_id"], "t-young", "{listed}");
    Ok(())
}

/// AC ATR-006: a prepared task is chained after another in place (same id and number); a task
/// that has started is refused.
#[tokio::test]
async fn task_update_changes_a_waiting_tasks_start_in_place() -> TestResult {
    let (h, _) = world().await?;
    seed_task(&h, "t-first", "s1", "u-alice", TaskStatus::Queued, None).await?;
    seed_task(&h, "t-later", "s1", "u-alice", TaskStatus::Prepared, None).await?;
    let (first, later) = (number(&h, "t-first").await?, number(&h, "t-later").await?);

    let (updated, is_error) = tool(
        &h,
        "task_update",
        json!({"number": later, "after_task": first}),
    )
    .await?;
    assert!(!is_error, "{updated}");
    let task = h
        .store
        .task(&TaskId::parse("t-later")?)
        .await?
        .ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Chained);
    assert_eq!(task.number, later);
    assert_eq!(updated["task"]["number"], json!(later), "{updated}");

    let (refused, is_error) = tool(
        &h,
        "task_update",
        json!({"task_id": "t-agent", "prepared": true}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("has started"), "{refused}");
    Ok(())
}
