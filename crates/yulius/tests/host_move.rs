//! The first start after a move to another host (SPEC-460 MIG-050): the copy carries a mark, and
//! the start that finds it puts the tasks whose runs were lost with the old host back in the
//! queue, whoever owns them.

use yulius_app::host_move::{MOVE_PENDING, MOVED, MoveReport};
use yulius_app::ports::store::TaskStore as _;
use yulius_domain::ids::{AttemptId, TaskId, UserId};
use yulius_domain::task::{StopReason, TaskStatus};

use crate::support::{
    Harness, HarnessOptions, TestResult, harness_with, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task,
};

/// Four tasks whose runs the old host took with it (two people's, one a person had paused, one
/// being stopped) and one that rested already. No session may start, so a queued task stays
/// queued.
async fn moved_instance() -> Result<Harness, Box<dyn std::error::Error>> {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|config| config.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    for (task, owner, status) in [
        ("t-alice", "u-alice", TaskStatus::Running),
        ("t-bob", "u-bob", TaskStatus::Running),
        ("t-paused", "u-alice", TaskStatus::Running),
        ("t-stopping", "u-alice", TaskStatus::Stopping),
    ] {
        seed_task(&h, task, "s1", owner, status, None).await?;
        seed_live_attempt(&h, task, &format!("token-{task}")).await?;
    }
    h.store
        .set_person_pause(
            &TaskId::parse("t-paused")?,
            &UserId::parse("u-alice")?,
            &AttemptId::parse("a-t-paused")?,
            h.app.now(),
        )
        .await?;
    let rested = TaskStatus::Stopped(StopReason::Cancelled);
    seed_task(&h, "t-rested", "s1", "u-alice", rested, None).await?;
    Ok(h)
}

async fn status(h: &Harness, task: &str) -> Result<TaskStatus, Box<dyn std::error::Error>> {
    let task = h.store.task(&TaskId::parse(task)?).await?;
    Ok(task.ok_or("no such task")?.status)
}

const LOST: TaskStatus = TaskStatus::Stopped(StopReason::ServerLost);

#[tokio::test]
async fn the_first_start_after_a_move_queues_the_tasks_that_were_live() -> TestResult {
    let h = moved_instance().await?;
    let data = h.app.config.data_dir.clone();
    std::fs::write(
        data.join(MOVE_PENDING),
        r#"{"version":1,"from":"https://old.example","to":"https://new.example","at":1}"#,
    )?;

    drop(h.app.recover().await);

    // Whoever owns them; a task a person had paused or was stopping is left as lost.
    assert_eq!(status(&h, "t-alice").await?, TaskStatus::Queued);
    assert_eq!(status(&h, "t-bob").await?, TaskStatus::Queued);
    assert_eq!(status(&h, "t-paused").await?, LOST);
    assert_eq!(status(&h, "t-stopping").await?, LOST);
    assert_eq!(
        status(&h, "t-rested").await?,
        TaskStatus::Stopped(StopReason::Cancelled)
    );

    // The result is kept for the move's checklist and the mark is gone.
    assert!(!data.join(MOVE_PENDING).exists());
    let report: MoveReport = serde_json::from_str(&std::fs::read_to_string(data.join(MOVED))?)?;
    assert_eq!((report.queued, report.left, report.failed), (2, 2, 0));
    assert_eq!(report.from.as_deref(), Some("https://old.example"));
    assert_eq!(report.to.as_deref(), Some("https://new.example"));

    // The next start is an ordinary one.
    drop(h.app.recover().await);
    assert_eq!(status(&h, "t-alice").await?, TaskStatus::Queued);
    assert_eq!(status(&h, "t-paused").await?, LOST);
    Ok(())
}

#[tokio::test]
async fn a_start_without_the_mark_queues_nothing() -> TestResult {
    let h = moved_instance().await?;
    drop(h.app.recover().await);
    for task in ["t-alice", "t-bob", "t-paused", "t-stopping"] {
        assert_eq!(status(&h, task).await?, LOST, "{task}");
    }
    assert!(!h.app.config.data_dir.join(MOVED).exists());
    Ok(())
}

#[tokio::test]
async fn a_mark_that_cannot_be_read_still_brings_the_tasks_back() -> TestResult {
    let h = moved_instance().await?;
    let data = h.app.config.data_dir.clone();
    std::fs::write(data.join(MOVE_PENDING), "not json")?;
    drop(h.app.recover().await);
    assert_eq!(status(&h, "t-bob").await?, TaskStatus::Queued);
    let report: MoveReport = serde_json::from_str(&std::fs::read_to_string(data.join(MOVED))?)?;
    assert_eq!((report.queued, report.from), (2, None));
    Ok(())
}
