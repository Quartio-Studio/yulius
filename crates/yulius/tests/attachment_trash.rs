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
    world_in(harness_configured(|c| c.max_live_sessions = 0)?).await
}

/// [`world`] on the harness `h`.
async fn world_in(h: support::Harness) -> Result<(support::Harness, PathBuf), Error> {
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

/// SPEC-490 OWN-004: files restored into a task are an attachment to it, so they go only into
/// a task of the person the caller acts for; another person's task gets nothing and the trash
/// keeps the entry.
#[tokio::test]
async fn attachments_are_restored_only_into_ones_own_task() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    attach(&attachments, "t1", "a.txt", "precious")?;
    let (n1, bobs) = (number(&h, "t1").await?, number(&h, "t-bob").await?);
    let (deleted, is_error) = tool(&h, "task_delete", json!({"number": n1})).await?;
    assert!(!is_error, "{deleted}");

    let (refused, is_error) = tool(
        &h,
        "attachments_trash",
        json!({"restore": n1, "into": bobs}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(
        refused.to_string().contains("@bob") && refused.to_string().contains("only they steer it"),
        "{refused}"
    );
    assert!(
        !attachments.join("tasks/t-bob").exists(),
        "nothing restored"
    );
    assert_eq!(trash(&attachments).len(), 1, "the entry stays");
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

/// AC ATR-005: listing a sandbox that never had a deletion creates no folder.
#[tokio::test]
async fn listing_an_empty_trash_creates_nothing() -> TestResult {
    let (h, attachments) = world().await?;
    let (listed, is_error) = tool(&h, "attachments_trash", json!({})).await?;
    assert!(!is_error, "{listed}");
    assert_eq!(listed["trash"], json!([]), "{listed}");
    assert!(!attachments.join("trash").exists());
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

    // Audited with the calling agent (MGT-010); a person who only reads the project changes
    // nothing.
    let actor = format!("task:{}", number(&h, "t-agent").await?);
    let audit = h.store.automation_audit("t-later", 10).await?;
    let changes: Vec<&str> = audit
        .iter()
        .filter(|e| e.action == "task.start_changed")
        .map(|e| e.actor.as_str())
        .collect();
    assert_eq!(
        changes,
        [actor.as_str()],
        "one row, the caller's: {audit:?}"
    );
    let refused = mcp_tool(
        &h,
        "tok-bob",
        "task_update",
        json!({"number": later, "prepared": true}),
    )
    .await?;
    assert_eq!(refused["isError"], json!(true), "{refused}");
    let task = h
        .store
        .task(&TaskId::parse("t-later")?)
        .await?
        .ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Chained);
    Ok(())
}

/// AC ATR-001: an archived task's request and an automation's prompt name a folder as well.
#[tokio::test]
async fn archived_tasks_and_automations_name_attachments_too() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let one = attach(&attachments, "t1", "one.png", "1")?;
    let two = attach(&attachments, "t2", "two.png", "2")?;
    let prompt = format!("Compare with {}", one.display());
    seed_task_with_prompt(
        &h,
        "t-old",
        "s1",
        "u-alice",
        TaskStatus::Closed,
        None,
        &prompt,
    )
    .await?;
    let (archived, is_error) = tool(&h, "task_archive", json!({"task_id": "t-old"})).await?;
    assert!(!is_error, "{archived}");
    let (created, is_error) = tool(
        &h,
        "automation_create",
        json!({"name": "Nightly", "cron": "0 4 * * *", "prompt": format!("Check {}", two.display())}),
    )
    .await?;
    assert!(!is_error, "{created}");

    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    let kept = number(&h, "t-old").await?;
    assert_eq!(deleted["deleted"]["attachments"]["kept_for"], json!([kept]));
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t2"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"]["kept_for_automations"],
        json!(["Nightly"])
    );
    assert!(one.is_file() && two.is_file());
    assert!(trash(&attachments).is_empty());
    Ok(())
}

/// AC ATR-002, ATR-004, ATR-005: an old entry goes at a deletion too; a full task takes no
/// restore; what a restore cannot move keeps its entry; a folder a parallel deletion trashed
/// first gets its task's number back.
#[tokio::test]
async fn the_trash_keeps_what_it_cannot_restore_and_numbers_what_others_moved() -> TestResult {
    let (h, attachments) = world().await?;
    let now = h.app.now().0 / 1000;
    let old = attachments
        .join("trash")
        .join(format!("{}-9-t-old", now - 8 * DAY_SECS));
    std::fs::create_dir_all(&old)?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t-full", "s1", "u-alice", TaskStatus::Prepared, None).await?;
    let (n1, n2) = (number(&h, "t1").await?, number(&h, "t2").await?);
    let full = number(&h, "t-full").await?;
    attach(&attachments, "t1", "a.txt", "a")?;
    std::fs::create_dir_all(attachments.join("tasks/t1/nested"))?;
    std::fs::write(attachments.join("tasks/t1/nested/b.txt"), "b")?;
    for i in 0..500 {
        attach(&attachments, "t-full", &format!("f{i}.txt"), "f")?;
    }

    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    assert!(
        !old.exists(),
        "an entry older than a week goes at a deletion"
    );

    let (refused, is_error) = tool(
        &h,
        "attachments_trash",
        json!({"restore": "t1", "into": full}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("more than 500"), "{refused}");

    let (restored, is_error) = tool(&h, "attachments_trash", json!({"restore": n1})).await?;
    assert!(!is_error, "{restored}");
    assert_eq!(
        std::fs::read_to_string(attachments.join("tasks/t-agent/a.txt"))?,
        "a"
    );
    assert_eq!(
        restored["restored"]["left_in_trash"],
        json!(["nested/"]),
        "{restored}"
    );
    let entries = trash(&attachments);
    let entry = entries.first().ok_or("the entry stays")?;
    assert_eq!(
        std::fs::read_to_string(attachments.join("trash").join(entry).join("nested/b.txt"))?,
        "b",
        "a folder in the entry is never removed by a restore"
    );

    // t2's folder was moved by another deletion before t2's own settling, without its number.
    std::fs::create_dir_all(attachments.join(format!("trash/{now}-0-t2")))?;
    std::fs::write(attachments.join(format!("trash/{now}-0-t2/c.txt")), "c")?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t2"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"]["trash"]["number"],
        json!(n2),
        "{deleted}"
    );
    assert!(
        attachments
            .join(format!("trash/{now}-{n2}-t2/c.txt"))
            .is_file()
    );
    Ok(())
}

/// AC ATR-002: a trash folder swapped for a link outside the sandbox receives nothing; the
/// deleted task's folder stays where it is.
#[tokio::test]
async fn a_trash_swapped_for_a_link_receives_nothing() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let file = attach(&attachments, "t1", "a.txt", "a")?;
    let outside = h.app.config.data_dir.join("host");
    std::fs::create_dir_all(&outside)?;
    std::os::unix::fs::symlink(&outside, attachments.join("trash"))?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"]["not_moved"],
        json!(true),
        "the answer says they stayed: {deleted}"
    );
    assert_eq!(
        std::fs::read_dir(&outside)?.count(),
        0,
        "nothing went outside"
    );
    assert!(file.is_file());
    let listed = mcp_tool(&h, "tok-agent", "attachments_trash", json!({})).await?;
    assert_eq!(listed["isError"], json!(true), "{listed}");
    assert_eq!(std::fs::read_dir(&outside)?.count(), 0);
    Ok(())
}

/// SPEC-397 NMV-002: deleting a task while `attachments` or its `tasks` is a link answers
/// `not_moved` (what is behind a link is never looked into, so the folder may still be there) and
/// changes nothing outside; an `attachments` that is a plain file answers nothing.
#[tokio::test]
async fn a_deletion_through_a_linked_attachments_folder_says_not_moved() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let outside = h.app.config.data_dir.join("host");
    let file = attach(&outside, "t1", "a.txt", "a")?;
    let old = outside.join("trash/1-0-t-old/old.txt");
    std::fs::create_dir_all(outside.join("trash/1-0-t-old"))?;
    std::fs::write(&old, "old")?;
    std::fs::remove_dir(&attachments)?;
    std::os::unix::fs::symlink(&outside, &attachments)?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"],
        json!({"not_moved": true}),
        "the answer says they stayed: {deleted}"
    );
    assert_eq!(
        std::fs::read_to_string(&file)?,
        "a",
        "nothing outside moved"
    );
    assert!(old.is_file(), "nothing outside was purged");

    seed_task(&h, "t3", "s1", "u-alice", TaskStatus::Closed, None).await?;
    attach(&outside, "t3", "c.txt", "c")?;
    std::fs::remove_file(&attachments)?;
    std::fs::create_dir(&attachments)?;
    std::os::unix::fs::symlink(outside.join("tasks"), attachments.join("tasks"))?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t3"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"],
        json!({"not_moved": true}),
        "{deleted}"
    );
    assert!(
        outside.join("tasks/t3/c.txt").is_file(),
        "nothing outside moved"
    );

    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Closed, None).await?;
    std::fs::remove_dir_all(&attachments)?;
    std::fs::write(&attachments, "")?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t2"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(deleted["deleted"]["attachments"], json!({}), "{deleted}");
    Ok(())
}

/// SPEC-397 NMV-002: a deletion whose sandbox's tasks cannot be read (the automations table
/// renamed away) keeps every folder; the answer says `not_moved` when the deleted task had one.
#[tokio::test]
async fn a_deletion_with_unreadable_tasks_says_not_moved_when_the_folder_stayed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("yulius.db");
    let (h, attachments) = world_in(support::harness_with(support::HarnessOptions {
        store: Some(yulius_store::SqliteStore::open(&db_path)?),
        dir: Some(dir),
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..support::HarnessOptions::default()
    })?)
    .await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let file = attach(&attachments, "t1", "a.txt", "a")?;
    let db = rusqlite::Connection::open(&db_path)?;
    db.execute_batch("ALTER TABLE automations RENAME TO automations_gone")?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"],
        json!({"not_moved": true}),
        "{deleted}"
    );
    assert!(file.is_file(), "the folder stayed");
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t2"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(deleted["deleted"]["attachments"], json!({}), "{deleted}");
    db.execute_batch("ALTER TABLE automations_gone RENAME TO automations")?;
    Ok(())
}

/// AC ATR-002: while a deletion settles, a task its first reading missed (created or unarchived
/// meanwhile) keeps its folder, and the deleted task's folder moved by a parallel deletion first
/// gets its number back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settling_asks_again_before_it_moves_a_folder() -> TestResult {
    let (h, attachments) = world().await?;
    let proto = seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let n1 = number(&h, "t1").await?;
    attach(&attachments, "t1", "a.txt", "a")?;
    let late = attach(&attachments, "t-late", "late.txt", "late")?;
    let (store, runtime) = (h.store.clone(), tokio::runtime::Handle::current());
    let mut record = proto.clone();
    record.id = TaskId::parse("t-late")?;
    record.number = 0;
    let (tasks, now) = (attachments.join("tasks"), h.app.now().0 / 1000);
    yulius_app::race::on(
        yulius_app::race::Point::AttachmentsSettle,
        &tasks.clone(),
        move || {
            let _ = runtime.block_on(store.insert_task(&record));
            let _ = std::fs::create_dir_all(tasks.with_file_name("trash"));
            let _ = std::fs::rename(
                tasks.join("t1"),
                tasks.with_file_name("trash").join(format!("{now}-0-t1")),
            );
        },
    );
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    assert!(late.is_file(), "the late task's folder stays");
    assert_eq!(
        deleted["deleted"]["attachments"]["trash"]["number"],
        json!(n1),
        "{deleted}"
    );
    assert_eq!(trash(&attachments), [format!("{now}-{n1}-t1")]);
    Ok(())
}

/// AC ATR-005: a restored file gets a name that needs no quoting, whatever the trash held.
#[tokio::test]
async fn restored_names_need_no_quoting() -> TestResult {
    let (h, attachments) = world().await?;
    let entry = attachments
        .join("trash")
        .join(format!("{}-7-t-gone", h.app.now().0 / 1000));
    std::fs::create_dir_all(&entry)?;
    std::fs::write(entry.join("x;$(id).png"), "x")?;
    let (restored, is_error) = tool(&h, "attachments_trash", json!({"restore": "t-gone"})).await?;
    assert!(!is_error, "{restored}");
    let path = restored["restored"]["paths"][0].as_str().ok_or("path")?;
    let name = Path::new(path).file_name().ok_or("name")?.to_string_lossy();
    assert!(
        name.ends_with(".png")
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c)),
        "{name}"
    );
    assert_eq!(std::fs::read_to_string(path)?, "x");
    Ok(())
}

/// SPEC-413 ANR-003 (#1031): with more folders in `attachments/tasks/` than settling lists
/// (`MAX_PRUNED_ENTRIES`, 10,000; planted by a sandbox process), deleting a task still moves its
/// folder to the trash and says so, never `{}`. The planted names are no task ids, so they stay.
/// Whether the cap hides `t1` depends on the file system's listing order (creation order on
/// tmpfs, hash order on ext4): this guards the answer end to end, and the unit test
/// `the_deleted_tasks_folder_is_listed_past_the_cap` (a cap of 0) is the deterministic proof.
#[tokio::test]
async fn a_deletion_past_ten_thousand_planted_folders_still_trashes_its_own() -> TestResult {
    let (h, attachments) = world().await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let tasks = attachments.join("tasks");
    for n in 0..=10_000 {
        std::fs::create_dir_all(tasks.join(format!("p.{n:05}")))?;
    }
    attach(&attachments, "t1", "a.txt", "a")?;
    let (deleted, is_error) = tool(&h, "task_delete", json!({"task_id": "t1"})).await?;
    assert!(!is_error, "{deleted}");
    assert_eq!(
        deleted["deleted"]["attachments"]["trash"]["files"],
        json!(["a.txt"]),
        "{deleted}"
    );
    assert!(!tasks.join("t1").exists());
    assert_eq!(
        std::fs::read_dir(&tasks)?.count(),
        10_001,
        "planted ones stay"
    );
    Ok(())
}
