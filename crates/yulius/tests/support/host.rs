//! A process host double whose methods answer what a test scripts.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::session::{
    AttachedRun, BackgroundProbe, LaunchPlan, RunInspection, SessionHost, SpawnedRun, StrayProcess,
};

type Slot<F> = Option<Box<F>>;

/// A [`SessionHost`] whose scripted methods answer from their slot. Spawn, terminate and an
/// unscripted method of the slots below ask `inner` when there is one; without it, spawn and
/// attach are refused, `inspect` finds nothing, `terminate` succeeds and the probes keep the
/// trait's answers. The freeze methods and `loopback_ports_idle` always keep the trait's
/// answers, also with `inner`. Every spawn's run directory and plan are recorded in `plans`.
#[derive(Default)]
// The slots are boxed closures over borrowed arguments; an alias per slot would only rename them.
#[allow(clippy::type_complexity)]
pub struct ScriptedHost {
    pub inner: Option<Arc<dyn SessionHost>>,
    pub plans: Mutex<Vec<(PathBuf, LaunchPlan)>>,
    pub attach: Slot<dyn Fn(&Path) -> AppResult<AttachedRun> + Send + Sync>,
    pub inspect: Slot<dyn Fn(&Path) -> RunInspection + Send + Sync>,
    pub background_shells: Slot<dyn Fn(u32) -> Option<u32> + Send + Sync>,
    pub background_probe: Slot<dyn Fn(u32) -> Option<BackgroundProbe> + Send + Sync>,
    pub reap_strays: Slot<dyn Fn(&[StrayProcess]) -> usize + Send + Sync>,
}

impl ScriptedHost {
    /// The run directories and plans spawn was given, in order.
    pub fn runs(&self) -> Vec<(PathBuf, LaunchPlan)> {
        self.plans
            .lock()
            .map(|runs| runs.clone())
            .unwrap_or_default()
    }

    /// The plans spawn was given, in order.
    pub fn plans(&self) -> Vec<LaunchPlan> {
        self.runs().into_iter().map(|(_, plan)| plan).collect()
    }
}

#[async_trait]
impl SessionHost for ScriptedHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        if let Ok(mut plans) = self.plans.lock() {
            plans.push((run_dir.to_path_buf(), plan.clone()));
        }
        match &self.inner {
            Some(inner) => {
                inner
                    .spawn(run_dir, attempt_id, task_id, plan, token, size)
                    .await
            }
            None => Err(AppError::Upstream("recorded, not started".into())),
        }
    }

    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        match (&self.attach, &self.inner) {
            (Some(attach), _) => attach(run_dir),
            (None, Some(inner)) => inner.attach(run_dir, token).await,
            (None, None) => Err(AppError::NotFound),
        }
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        match (&self.inspect, &self.inner) {
            (Some(inspect), _) => inspect(run_dir),
            (None, Some(inner)) => inner.inspect(run_dir),
            (None, None) => RunInspection::Missing,
        }
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        self.inner
            .as_ref()
            .map_or(Ok(()), |inner| inner.terminate(run_dir))
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        match (&self.background_shells, &self.inner) {
            (Some(shells), _) => shells(child_pid),
            (None, Some(inner)) => inner.background_shells(child_pid),
            (None, None) => None,
        }
    }

    fn background_probe(&self, child_pid: u32) -> Option<BackgroundProbe> {
        match (&self.background_probe, &self.inner) {
            (Some(probe), _) => probe(child_pid),
            (None, Some(inner)) => inner.background_probe(child_pid),
            (None, None) => self
                .background_shells(child_pid)
                .map(|shells| BackgroundProbe {
                    shells,
                    ..BackgroundProbe::default()
                }),
        }
    }

    fn reap_strays(&self, strays: &[StrayProcess]) -> usize {
        match (&self.reap_strays, &self.inner) {
            (Some(reap), _) => reap(strays),
            (None, Some(inner)) => inner.reap_strays(strays),
            (None, None) => 0,
        }
    }
}
