//! Shared fixtures moved from `first_message`.

use crate::support;
use crate::support::eventually;
use crate::support::live::Live;
use crate::support::live::pool::ScriptedUsage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use support::TestResult;
use support::agents::FakeAgentOptions;
use yulius::wiring::clock::SystemClock;
use yulius_app::context::{App, AppConfig};
use yulius_app::hub::HubFrame;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{TaskMode, TaskOrigin};
use yulius_app::ports::Clock as _;
use yulius_app::ports::store::TaskStore;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::task::Activity;
use yulius_domain::time::Timestamp;

type BoxError = Box<dyn std::error::Error>;

/// A server: the app and its internal listener on `port` (0 = any free port), as `yulius serve`
/// would start it, with `probe` reading the subscriptions' usage.
pub(crate) async fn start(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    probe: Arc<ScriptedUsage>,
) -> Result<(Arc<App>, u16), BoxError> {
    start_with(data, fake, agent, port, probe, |_| {}).await
}

/// [`start`] with `configure` applied to the app's configuration.
pub(crate) async fn start_with(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    probe: Arc<ScriptedUsage>,
    configure: impl FnOnce(&mut AppConfig),
) -> Result<(Arc<App>, u16), BoxError> {
    Live {
        usage: probe,
        ..Live::new(Some(fake), agent)
    }
    .start(data, port, configure)
    .await
}

/// The system clock plus an offset (ms) the test moves: real processes and their hooks keep
/// their pace while minutes pass for the server. Start it behind and move it forward. A hook's
/// send time, which its client stamps on the system clock, reads with the offset in force when
/// it was sent, so it keeps its order with the server's times also when it arrives late, after
/// a move (SPEC-636; it used to read as its arrival, #1615).
pub(crate) struct ShiftedClock(std::sync::Mutex<Vec<(i64, i64)>>);

impl ShiftedClock {
    /// A clock `offset_ms` from the system's.
    pub(crate) fn new(offset_ms: i64) -> Self {
        Self(std::sync::Mutex::new(vec![(i64::MIN, offset_ms)]))
    }

    /// Runs the clock `offset_ms` from the system's from now on.
    pub(crate) fn shift(&self, offset_ms: i64) {
        let at = SystemClock.now().0;
        self.shifts().push((at, offset_ms));
    }

    /// Moves the clock `ms` forward from now on.
    pub(crate) fn advance(&self, ms: i64) {
        let at = SystemClock.now().0;
        let mut shifts = self.shifts();
        let offset = shifts.last().map_or(0, |(_, offset)| *offset) + ms;
        shifts.push((at, offset));
    }

    /// The offset in force at the system clock's `wall_ms`.
    fn offset_at(&self, wall_ms: i64) -> i64 {
        let shifts = self.shifts();
        shifts
            .iter()
            .rev()
            .find(|(from, _)| *from <= wall_ms)
            .map_or(0, |(_, offset)| *offset)
    }

    fn shifts(&self) -> std::sync::MutexGuard<'_, Vec<(i64, i64)>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl yulius_app::ports::Clock for ShiftedClock {
    fn now(&self) -> Timestamp {
        let system = SystemClock.now().0;
        Timestamp(system + self.offset_at(system))
    }

    fn of_wall(&self, wall: Timestamp) -> Timestamp {
        Timestamp(wall.0 + self.offset_at(wall.0))
    }
}

pub(crate) fn hooked() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    }
}

pub(crate) async fn new_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    prompt: &str,
    mode: TaskMode,
) -> Result<TaskId, BoxError> {
    Ok(app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: prompt.into(),
            harness: Harness::ClaudeCode,
            title: Some("First message".into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode,
            supertask: false,
        })
        .await?
        .id)
}

pub(crate) fn screen(app: &App, id: &TaskId) -> String {
    app.runtime(id).hub.screen_text()
}

pub(crate) async fn activity(app: &App, id: &TaskId) -> Option<Activity> {
    app.deps
        .store
        .task(id)
        .await
        .ok()
        .flatten()
        .and_then(|t| t.activity)
}

/// The notices sent to the task's viewers so far on `watch`.
pub(crate) fn notices(watch: &mut tokio::sync::broadcast::Receiver<HubFrame>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let HubFrame::Notice(text) = frame {
            out.push(text);
        }
    }
    out
}

pub(crate) async fn stop(app: &Arc<App>, id: &TaskId) -> TestResult {
    app.stop_task_now(id).await?;
    eventually("the stop", Duration::from_secs(30), || async {
        TaskStore::task(app.deps.store.as_ref(), id)
            .await
            .ok()
            .flatten()
            .is_some_and(|t| !t.status.is_live())
    })
    .await
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

pub(crate) fn data_dir() -> Result<(tempfile::TempDir, PathBuf), BoxError> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yf").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}
