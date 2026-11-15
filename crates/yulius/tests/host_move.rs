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

/// SPEC-483 MOV-002: a mark that is not even text (a damaged file) is still a mark: the lost
/// tasks come back and the mark goes, rather than being met again at every start.
#[tokio::test]
async fn a_mark_that_is_not_text_still_brings_the_tasks_back_and_goes() -> TestResult {
    let h = moved_instance().await?;
    let data = h.app.config.data_dir.clone();
    std::fs::write(data.join(MOVE_PENDING), [0xff, 0xfe, b'{'])?;
    drop(h.app.recover().await);
    assert_eq!(status(&h, "t-bob").await?, TaskStatus::Queued);
    assert!(!data.join(MOVE_PENDING).exists(), "the mark stays");
    let report: MoveReport = serde_json::from_str(&std::fs::read_to_string(data.join(MOVED))?)?;
    assert_eq!((report.queued, report.from), (2, None));
    Ok(())
}

/// A process host that finds every run ended, cleanly, while Yulius was down.
struct EndedHost;

#[async_trait::async_trait]
impl yulius_app::ports::session::SessionHost for EndedHost {
    async fn spawn(
        &self,
        _: &std::path::Path,
        _: &str,
        _: &str,
        _: &yulius_app::ports::session::LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::SpawnedRun> {
        Err(yulius_app::error::AppError::Upstream(
            "no spawn here".into(),
        ))
    }
    async fn attach(
        &self,
        _: &std::path::Path,
        _: &[u8; 32],
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::AttachedRun> {
        Err(yulius_app::error::AppError::Upstream(
            "no attach here".into(),
        ))
    }
    fn inspect(&self, _: &std::path::Path) -> yulius_app::ports::session::RunInspection {
        yulius_app::ports::session::RunInspection::Exited(yulius_app::ports::session::RunExit {
            code: Some(0),
            signal: None,
            spawn_failed: false,
            usage: None,
        })
    }
    fn terminate(&self, _: &std::path::Path) -> yulius_app::error::AppResult<()> {
        Ok(())
    }
}

/// A task the old server left switching, and one it left running, each with a run that is found
/// ended.
async fn ended_while_down() -> Result<Harness, Box<dyn std::error::Error>> {
    let h = harness_with(HarnessOptions {
        session_host: Some(std::sync::Arc::new(EndedHost)),
        configure: Some(Box::new(|config| config.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    for (task, status) in [
        ("t-switching", TaskStatus::Switching),
        ("t-left", TaskStatus::Running),
    ] {
        seed_task(&h, task, "s1", "u-alice", status, None).await?;
        seed_live_attempt(&h, task, &format!("token-{task}")).await?;
    }
    Ok(h)
}

/// SPEC-483 SWT-002 (review, round 3): the server died while a task was switching, and the run
/// that switch had asked to end is found ended at the next start. The switch was lost with the
/// server: the task is lost to the restart, as one found with no run is, and a move brings it
/// back. It is not closed as a task whose session left by itself is.
#[tokio::test]
async fn a_task_left_switching_whose_run_ended_is_lost_to_the_restart() -> TestResult {
    let h = ended_while_down().await?;
    drop(h.app.recover().await);
    assert_eq!(status(&h, "t-switching").await?, LOST);
    // The session of a running task left while Yulius was down: the task follows it.
    assert_eq!(status(&h, "t-left").await?, TaskStatus::Closed);
    for task in ["t-switching", "t-left"] {
        let run = h
            .store
            .attempt(&AttemptId::parse(&format!("a-{task}"))?)
            .await?;
        assert_eq!(
            run.ok_or("no run")?.exit_reason.as_deref(),
            Some("exited"),
            "{task}"
        );
    }

    let moved = ended_while_down().await?;
    let data = moved.app.config.data_dir.clone();
    std::fs::write(
        data.join(MOVE_PENDING),
        r#"{"version":1,"from":"https://old.example","to":"https://new.example","at":1}"#,
    )?;
    drop(moved.app.recover().await);
    assert_eq!(status(&moved, "t-switching").await?, TaskStatus::Queued);
    assert_eq!(status(&moved, "t-left").await?, TaskStatus::Closed);
    let report: MoveReport = serde_json::from_str(&std::fs::read_to_string(data.join(MOVED))?)?;
    assert_eq!((report.queued, report.left, report.failed), (1, 0, 0));
    Ok(())
}
