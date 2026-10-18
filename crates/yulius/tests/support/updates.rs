//! A fake `main` source and update helper (SPEC-002 ADR-18): what GitHub shows and what the root
//! helper reports, set by the test.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use yulius::wiring::pending::UnavailableSessionHost;
use yulius_app::error::AppResult;
use yulius_app::model::Secret;
use yulius_app::ports::session::{
    AttachedRun, LaunchPlan, RunFreeze, RunInspection, SessionHost, SpawnedRun,
};
use yulius_app::ports::updates::{
    ReleaseSource, ReleaseState, RemoteCommit, SourceError, UpdateHelper,
};
use yulius_app::updates::UpdateServices;
use yulius_contracts::update::{HelperRequest, HelperState, HelperStatus};
use yulius_domain::update::Relation;

/// The running build.
pub const OLD: &str = "1111111111111111111111111111111111111111";
/// A merge commit between.
pub const MID: &str = "2222222222222222222222222222222222222222";
/// The head of `main`.
pub const NEW: &str = "3333333333333333333333333333333333333333";

/// `main` as the fake GitHub shows it; records the credentials it was read with.
pub struct FakeSource {
    pub result: Mutex<Result<ReleaseState, SourceError>>,
    pub tokens: Mutex<Vec<Option<String>>>,
    /// Told each time a compare starts.
    pub entered: tokio::sync::Notify,
    /// While set, a compare waits (at most 30 s) until it is notified.
    pub gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl FakeSource {
    /// `main` one feature ahead of [`OLD`].
    pub fn behind() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(Ok(ReleaseState {
                head: NEW.into(),
                relation: Relation::Behind,
                commits: vec![
                    RemoteCommit {
                        sha: NEW.into(),
                        message: "feat(ui): an update banner (#140)".into(),
                        author: Some("octocat".into()),
                        date: Some("2026-10-10T20:00:00Z".into()),
                    },
                    RemoteCommit {
                        sha: MID.into(),
                        message: "Merge pull request #139 from Quartio-Studio/dev".into(),
                        author: None,
                        date: None,
                    },
                ],
            })),
            tokens: Mutex::new(Vec::new()),
            entered: tokio::sync::Notify::new(),
            gate: Mutex::new(None),
        })
    }
}

#[async_trait]
impl ReleaseSource for FakeSource {
    async fn compare(
        &self,
        token: Option<&Secret>,
        _build: Option<&str>,
    ) -> Result<ReleaseState, SourceError> {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.push(token.map(|t| t.expose().to_owned()));
        }
        self.entered.notify_one();
        let gate = self.gate.lock().ok().and_then(|g| g.clone());
        if let Some(gate) = gate {
            let _ = tokio::time::timeout(Duration::from_secs(30), gate.notified()).await;
        }
        self.result
            .lock()
            .map_err(|_| SourceError::Unavailable)?
            .clone()
    }
}

/// The root helper as the server sees it: requests written, a status to report.
#[derive(Default)]
pub struct FakeHelper {
    pub installed: AtomicBool,
    pub requests: Mutex<Vec<HelperRequest>>,
    pub status: Mutex<Option<HelperStatus>>,
    pub cleared: AtomicUsize,
    /// The kept re-check of the host (SPEC-203 HST-012).
    pub host_check: Mutex<Option<yulius_contracts::update::HostCheckReport>>,
}

impl FakeHelper {
    /// An installed helper.
    pub fn installed() -> Arc<Self> {
        let helper = Self::default();
        helper.installed.store(true, Ordering::SeqCst);
        Arc::new(helper)
    }

    /// The last request written.
    pub fn last(&self) -> Option<HelperRequest> {
        self.requests.lock().ok()?.last().cloned()
    }

    /// Reports on `request`.
    pub fn report(
        &self,
        request: &HelperRequest,
        state: HelperState,
        step: &str,
        error: Option<&str>,
    ) {
        if let Ok(mut status) = self.status.lock() {
            *status = Some(HelperStatus {
                version: 1,
                id: request.id.clone(),
                action: request.action,
                commit: request.commit.clone(),
                state,
                step: step.into(),
                progress: None,
                error: error.map(str::to_owned),
                previous: None,
                log_tail: vec!["error: example".into()],
                host_check: None,
                updated_at: 0,
            });
        }
    }
}

impl FakeHelper {
    /// Adds the new version's host re-check to the current status (SPEC-203 HST-012).
    pub fn report_host_check(&self, report: yulius_contracts::update::HostCheckReport) {
        if let Ok(mut status) = self.status.lock()
            && let Some(status) = status.as_mut()
        {
            status.host_check = Some(report);
        }
    }
}

impl UpdateHelper for FakeHelper {
    fn installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }
    fn request(&self, request: &HelperRequest) -> Result<(), String> {
        self.requests
            .lock()
            .map_err(|_| "lock".to_owned())?
            .push(request.clone());
        Ok(())
    }
    fn host_check(&self) -> Option<yulius_contracts::update::HostCheckReport> {
        self.host_check.lock().ok()?.clone()
    }
    fn status(&self) -> Option<HelperStatus> {
        self.status.lock().ok()?.clone()
    }
    fn clear_request(&self) {
        self.cleared.fetch_add(1, Ordering::SeqCst);
    }
}

/// Update services of a server built from `build`, without automatic checks, resuming tasks
/// 20 ms apart.
pub fn services(build: &str, source: &Arc<FakeSource>, helper: &Arc<FakeHelper>) -> UpdateServices {
    let mut services = UpdateServices::new(Some(build.into()));
    services.source = Some(source.clone() as Arc<dyn ReleaseSource>);
    services.helper = Some(helper.clone() as Arc<dyn UpdateHelper>);
    services.check_every = None;
    services.resume_stagger = Duration::from_millis(20);
    services
}

/// A process host that freezes runs as the cgroup freezer would (SPEC-160): runs answer
/// [`RunFreeze::Frozen`] unless set otherwise; every freeze, thaw and sweep is recorded.
pub struct FreezingHost {
    /// What freezing a run directory answers (default frozen).
    pub answers: Mutex<HashMap<String, RunFreeze>>,
    /// Run directories frozen and not thawed.
    pub frozen: Mutex<Vec<String>>,
    /// Run directories thawed, in order.
    pub thawed: Mutex<Vec<String>>,
    /// [`SessionHost::thaw_all`] calls.
    pub sweeps: AtomicUsize,
    /// The runs the last sweep kept frozen.
    pub kept: Mutex<Vec<String>>,
    /// [`SessionHost::freeze`] calls.
    pub freezes: AtomicUsize,
    /// Told when the next freeze starts.
    pub entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    /// The next freeze waits for this.
    pub gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl FreezingHost {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(HashMap::new()),
            frozen: Mutex::new(Vec::new()),
            thawed: Mutex::new(Vec::new()),
            sweeps: AtomicUsize::new(0),
            kept: Mutex::new(Vec::new()),
            freezes: AtomicUsize::new(0),
            entered: Mutex::new(None),
            gate: Mutex::new(None),
        })
    }

    /// Freezing `run_dir` answers `answer` from now on.
    pub fn answer(&self, run_dir: &str, answer: RunFreeze) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.insert(run_dir.into(), answer);
        }
    }

    pub fn frozen(&self) -> Vec<String> {
        self.frozen.lock().map(|f| f.clone()).unwrap_or_default()
    }

    pub fn thawed(&self) -> Vec<String> {
        self.thawed.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl SessionHost for FreezingHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        UnavailableSessionHost
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }
    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        UnavailableSessionHost.attach(run_dir, token).await
    }
    fn inspect(&self, run_dir: &Path) -> RunInspection {
        UnavailableSessionHost.inspect(run_dir)
    }
    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        UnavailableSessionHost.terminate(run_dir)
    }
    fn freeze(&self, run_dir: &Path) -> RunFreeze {
        self.freezes.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.entered.lock().ok().and_then(|mut e| e.take()) {
            let _ = entered.send(());
        }
        if let Some(gate) = self.gate.lock().ok().and_then(|mut g| g.take()) {
            let _ = gate.recv_timeout(Duration::from_secs(30));
        }
        let dir = run_dir.to_string_lossy().into_owned();
        let answer = self
            .answers
            .lock()
            .ok()
            .and_then(|a| a.get(&dir).copied())
            .unwrap_or(RunFreeze::Frozen);
        if answer != RunFreeze::Unsupported
            && let Ok(mut frozen) = self.frozen.lock()
            && !frozen.contains(&dir)
        {
            frozen.push(dir);
        }
        answer
    }
    fn can_freeze(&self, run_dir: &Path) -> bool {
        let dir = run_dir.to_string_lossy().into_owned();
        self.answers
            .lock()
            .ok()
            .and_then(|a| a.get(&dir).copied())
            .is_none_or(|answer| answer != RunFreeze::Unsupported)
    }
    fn thaw(&self, run_dir: &Path) {
        let dir = run_dir.to_string_lossy().into_owned();
        if let Ok(mut frozen) = self.frozen.lock() {
            frozen.retain(|d| *d != dir);
        }
        if let Ok(mut thawed) = self.thawed.lock() {
            thawed.push(dir);
        }
    }
    /// Counts the sweep and records what it kept; frozen runs stay recorded, so a test sees
    /// which ones were thawed one by one.
    fn thaw_all(&self, keep: &[std::path::PathBuf]) -> usize {
        self.sweeps.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut kept) = self.kept.lock() {
            *kept = keep
                .iter()
                .map(|k| k.to_string_lossy().into_owned())
                .collect();
        }
        0
    }
}
