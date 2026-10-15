//! The real detached ptyhost behind a gate the test controls: the next attach after
//! [`GatedHost::arm`] waits, its process already spawned, until [`GatedHost::release`], so a test
//! can drop a request mid-start or hold one start while others run (SPEC-077). With
//! [`GatedHost::refuse_ends`] its runs cannot be ended: a shutdown request is dropped and
//! `terminate` fails, as for a process that survives SIGKILL (SPEC-146).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;
use yulius::wiring::session::PtySessionHost;
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::session::{
    AttachedRun, LaunchPlan, RunInspection, RunSignal, RunWriter, SessionHost, SpawnedRun,
};

/// A held attach goes ahead by itself after this long, so a failing test cannot hang.
const HOLD_LIMIT: Duration = Duration::from_secs(60);

/// [`PtySessionHost`] whose next attach can be held.
pub struct GatedHost {
    inner: PtySessionHost,
    armed: AtomicBool,
    reached: watch::Sender<bool>,
    released: watch::Sender<bool>,
    /// Runs cannot be ended: shutdown requests are dropped and `terminate` fails.
    refusing: Arc<AtomicBool>,
}

impl GatedHost {
    /// The real host running `exe`, not armed.
    #[must_use]
    pub fn new(exe: PathBuf) -> Self {
        Self {
            inner: PtySessionHost::new(exe),
            armed: AtomicBool::new(false),
            reached: watch::channel(false).0,
            released: watch::channel(false).0,
            refusing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// While `refuse` holds, no run can be ended: shutdown requests are dropped and `terminate`
    /// fails (a process that survives SIGKILL); its own exit is still reported.
    pub fn refuse_ends(&self, refuse: bool) {
        self.refusing.store(refuse, Ordering::SeqCst);
    }

    /// Holds the next attach until [`Self::release`].
    pub fn arm(&self) {
        self.reached.send_replace(false);
        self.released.send_replace(false);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Returns once the armed attach is held (its process runs, the start is not finished).
    pub async fn reached(&self) {
        let mut reached = self.reached.subscribe();
        let _ = reached.wait_for(|held| *held).await;
    }

    /// Lets the held attach go ahead.
    pub fn release(&self) {
        self.released.send_replace(true);
    }
}

#[async_trait]
impl SessionHost for GatedHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        self.inner
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }

    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        if self.armed.swap(false, Ordering::SeqCst) {
            let mut released = self.released.subscribe();
            self.reached.send_replace(true);
            let _ = tokio::time::timeout(HOLD_LIMIT, released.wait_for(|go| *go)).await;
        }
        let mut attached = self.inner.attach(run_dir, token).await?;
        attached.writer = Box::new(RefusingWriter {
            inner: attached.writer,
            refusing: Arc::clone(&self.refusing),
        });
        Ok(attached)
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        self.inner.inspect(run_dir)
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        if self.refusing.load(Ordering::SeqCst) {
            return Err(AppError::Upstream(
                "cannot terminate the session: a process survived SIGKILL (test)".into(),
            ));
        }
        self.inner.terminate(run_dir)
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        self.inner.background_shells(child_pid)
    }
}

/// A run's writer whose shutdown requests are dropped while its host refuses ends.
struct RefusingWriter {
    inner: Box<dyn RunWriter>,
    refusing: Arc<AtomicBool>,
}

#[async_trait]
impl RunWriter for RefusingWriter {
    async fn input(&mut self, bytes: &[u8]) -> AppResult<()> {
        self.inner.input(bytes).await
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> AppResult<()> {
        self.inner.resize(cols, rows).await
    }

    async fn signal(&mut self, signal: RunSignal) -> AppResult<()> {
        if self.refusing.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.inner.signal(signal).await
    }

    async fn shutdown(&mut self, grace_ms: u32) -> AppResult<()> {
        if self.refusing.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.inner.shutdown(grace_ms).await
    }
}
