//! SPEC-353 LAP-003/LAP-004: the strays of a run (processes its sandbox init adopted, other than
//! the agent) are reported and, once an idle task has had them for the bound, reaped, while a
//! working task's are left alone. On 2026-10-30 about 60 leaked test jobs, each polling
//! `sleep 0.05`, lived for days in an idle session's sandbox.

use crate::support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use support::{HarnessOptions, harness_with, seed_github_user, seed_sandbox, seed_task};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{AttemptRecord, LaunchKind};
use yulius_app::ports::session::{
    AttachedRun, BackgroundProbe, LaunchPlan, RunInspection, SessionHost, SpawnedRun, StrayProcess,
};
use yulius_app::ports::store::{CoordinationStore as _, TaskStore as _};
use yulius_domain::harness::Harness as Agent;
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

/// A session host whose probe reports `strays` and which records what it is asked to reap.
#[derive(Default)]
struct StrayHost {
    strays: Mutex<Vec<StrayProcess>>,
    reaped: Mutex<Vec<StrayProcess>>,
    probes: AtomicU32,
}

#[async_trait::async_trait]
impl SessionHost for StrayHost {
    async fn spawn(
        &self,
        _: &std::path::Path,
        _: &str,
        _: &str,
        _: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        Err(AppError::Conflict("no runs here".into()))
    }
    async fn attach(&self, _: &std::path::Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        Err(AppError::Conflict("no runs here".into()))
    }
    fn inspect(&self, _: &std::path::Path) -> RunInspection {
        RunInspection::Missing
    }
    fn terminate(&self, _: &std::path::Path) -> AppResult<()> {
        Ok(())
    }
    fn background_probe(&self, _: u32) -> Option<BackgroundProbe> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        Some(BackgroundProbe {
            strays: self.strays.lock().ok()?.clone(),
            ..BackgroundProbe::default()
        })
    }
    fn reap_strays(&self, strays: &[StrayProcess]) -> usize {
        let mut reaped = self
            .reaped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reaped.extend(strays.iter().cloned());
        strays.iter().map(|stray| stray.processes.len()).sum()
    }
}

impl StrayHost {
    fn set(&self, strays: Vec<StrayProcess>) {
        *self
            .strays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = strays;
    }
    fn reaped(&self) -> Vec<StrayProcess> {
        self.reaped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// A job pair the quartio-v2 tests leaked: `sh -c …`, `bash …/job-<hex>.sh` and its `sleep`.
fn job(pid: u32, started: u64) -> StrayProcess {
    StrayProcess {
        processes: vec![(pid, started), (pid + 1, started), (pid + 2, started + 1)],
    }
}

struct World {
    h: support::Harness,
    host: Arc<StrayHost>,
    task: TaskId,
}

/// A task of `activity` whose live run's agent is process 4242.
async fn world(activity: Activity) -> BoxResult<World> {
    let host = Arc::new(StrayHost::default());
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        ..HarnessOptions::default()
    })?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    let task = seed_task(&h, "t-idle", "s1", "u-alice", TaskStatus::Running, None)
        .await?
        .id;
    h.store
        .set_activity(&task, Some(activity), h.app.now())
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at-idle")?,
            task_id: task.clone(),
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/nonexistent/run".into(),
            token_hash: "hash-idle".into(),
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: Some(4242),
            child_pid: Some(4242),
            started_at: h.app.now(),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(World { h, host, task })
}

impl World {
    /// Probes the task's run now (a change of its background agents asks for a probe).
    async fn probe(&self) {
        let before = self.host.probes.load(Ordering::SeqCst);
        let agents = before % 2 + 1;
        self.h
            .app
            .background_agents_observed(&self.task, agents)
            .await;
        assert!(
            self.host.probes.load(Ordering::SeqCst) > before,
            "no probe ran"
        );
    }

    fn advance(&self, ms: i64) {
        self.h.clock.0.fetch_add(ms, Ordering::SeqCst);
    }
}

/// LAP-004: an idle task's strays live the bound from when they were first seen, then their
/// whole trees are reaped; a stray seen later gets its own bound.
#[tokio::test]
async fn an_idle_task_s_strays_are_reaped_after_the_bound() -> BoxResult<()> {
    let w = world(Activity::Waiting).await?;
    let bound = w.h.app.config.stray_reap_after_ms;
    assert_eq!(bound, 2 * 3600 * 1000, "ADR-0240");
    w.host.set(vec![job(500, 77), job(600, 90)]);
    w.probe().await;
    assert!(w.host.reaped().is_empty(), "first seen: reported only");
    w.advance(bound - 1);
    w.probe().await;
    assert!(w.host.reaped().is_empty(), "within the bound");
    // A stray that appears now, and one whose pid another process took (a new start time).
    w.host.set(vec![job(500, 77), job(600, 91), job(700, 120)]);
    w.advance(1);
    w.probe().await;
    assert_eq!(
        w.host.reaped(),
        vec![job(500, 77)],
        "only the one seen for the bound"
    );
    w.advance(bound);
    w.probe().await;
    assert_eq!(
        w.host.reaped(),
        vec![job(500, 77), job(500, 77), job(600, 91), job(700, 120)],
        "a stray still there is reaped again; the later ones after their own bound"
    );
    Ok(())
}

/// LAP-004: a working task's processes are not probed, so its strays are left to its turn.
#[tokio::test]
async fn a_working_task_s_strays_are_left_alone() -> BoxResult<()> {
    let w = world(Activity::Working).await?;
    w.host.set(vec![job(500, 77)]);
    w.h.app.background_agents_observed(&w.task, 1).await;
    w.advance(w.h.app.config.stray_reap_after_ms * 2);
    w.h.app.background_agents_observed(&w.task, 2).await;
    assert!(w.host.reaped().is_empty());
    Ok(())
}
