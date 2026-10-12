//! Executing a CI run: its jobs in parallel through the process host, inside the sandbox's
//! containment (SPEC-002 ADR-21; RUN-001..003).
//!
//! A job is `/bin/sh -c <wrapper> yulius-ci <log> <command>`: the fixed wrapper sends the
//! job's output to its log file and then runs the command as its own `/bin/sh -c` argument, so
//! the configured shell text is never spliced into Yulius's script. The log is read while the
//! job runs (complete from the first byte, unlike a terminal attached after the start) and its
//! tail stored. The environment has no secret: no task token, provider credential or GitHub
//! token. `HOME` and `YULIUS_CI_CACHE` are persistent per sandbox, and each job keeps its own
//! persistent checkout, so caches (dependency downloads, build outputs) survive between runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::watch;
use yulius_domain::ci::{self as dom, RunState};

use super::{
    CiJobRecord, CiJobUpdate, CiRunRecord, CiRunUpdate, KILL_GRACE, MAX_LOG_FILE_BYTES,
    OUTPUT_BYTES,
};
use crate::context::App;
use crate::delivery::runner::OutputRing;
use crate::model::SandboxRecord;
use crate::ports::session::{Containment, LaunchPlan, RunEvent, RunExit, RunInspection, RunSignal};

/// The fixed wrapper: output to the log, stdin closed, then the configured command.
const WRAPPER: &str = "exec >>\"$1\" 2>&1 </dev/null; exec /bin/sh -c \"$2\"";
/// How often the log is read and the stored output refreshed.
const READ_EVERY: Duration = Duration::from_millis(500);
const FLUSH_EVERY_TICKS: u32 = 4;
/// Bytes read from the log per tick (the rest follows on the next ticks).
const READ_CHUNK: usize = 1 << 20;

/// Removes terminal escape sequences (CSI, OSC and two-byte escapes) and turns carriage
/// returns into line breaks, across chunk boundaries.
#[derive(Debug, Default)]
pub(super) struct AnsiStripper {
    state: Strip,
    last_cr: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Strip {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl AnsiStripper {
    pub(super) fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &byte in input {
            match self.state {
                Strip::Text => match byte {
                    0x1b => self.state = Strip::Escape,
                    b'\r' => {
                        out.push(b'\n');
                        self.last_cr = true;
                        continue;
                    }
                    b'\n' if self.last_cr => {}
                    0x08 | 0x07 => {}
                    _ => out.push(byte),
                },
                Strip::Escape => {
                    self.state = match byte {
                        b'[' => Strip::Csi,
                        b']' => Strip::Osc,
                        _ => Strip::Text,
                    }
                }
                Strip::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = Strip::Text;
                    }
                }
                Strip::Osc => match byte {
                    0x07 => self.state = Strip::Text,
                    0x1b => self.state = Strip::OscEscape,
                    _ => {}
                },
                Strip::OscEscape => self.state = Strip::Text,
            }
            self.last_cr = false;
        }
    }
}

/// How a job's process ended.
enum Ended {
    Exit(RunExit),
    Cancelled(String),
    TimedOut,
    OutputLimit,
    Lost,
}

/// The directory of a job's persistent checkout: the definition's name, plus the matrix value's
/// position for matrix jobs (siblings run in parallel and need their own).
fn checkout_name(job: &CiJobRecord, siblings: &[CiJobRecord]) -> String {
    let same: Vec<&CiJobRecord> = siblings.iter().filter(|j| j.key == job.key).collect();
    if same.len() <= 1 {
        return job.key.clone();
    }
    let index = same.iter().position(|j| j.id == job.id).unwrap_or(0) + 1;
    format!("{}.{index}", job.key)
}

impl App {
    /// Executes one run (the dispatcher marked its sandbox busy); always releases it.
    pub(super) async fn ci_execute(
        self: Arc<Self>,
        run: CiRunRecord,
        cancel: watch::Receiver<Option<String>>,
    ) {
        self.ci_execute_inner(&run, cancel).await;
        if let Ok(mut executing) = self.ci.executing.lock() {
            executing.remove(&run.id);
        }
        if let Ok(mut busy) = self.ci.busy.lock() {
            busy.remove(run.sandbox_id.as_str());
        }
        self.ci_wake();
    }

    async fn ci_execute_inner(
        self: &Arc<Self>,
        run: &CiRunRecord,
        cancel: watch::Receiver<Option<String>>,
    ) {
        let store = &self.deps.store;
        let start = CiRunUpdate {
            started_at: Some(self.now()),
            ..CiRunUpdate::default()
        };
        match store
            .transition_ci_run(&run.id, &[RunState::Queued], RunState::Running, &start)
            .await
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "starting a CI run failed");
                return;
            }
        }
        self.ci_publish(&run.id).await;
        let jobs = store.ci_jobs(&run.id, false).await.unwrap_or_default();
        let sandbox = match store.sandbox(&run.sandbox_id).await {
            Ok(Some(sandbox)) if sandbox.removed_at.is_none() => Some(sandbox),
            _ => None,
        };
        let prepared = match &sandbox {
            Some(sandbox) => self.ci_prepare(sandbox, run).await,
            None => Err("the sandbox was removed".to_owned()),
        };
        match (sandbox, prepared) {
            (Some(sandbox), Ok(())) => {
                let sandbox = Arc::new(sandbox);
                // Fail fast: the run's jobs share a cancel signal that follows the run's own
                // (superseded, stopped) and fires when one of them fails, so a failed `quality`
                // no longer leaves `e2e` holding the sandbox's CI lane for half an hour.
                let (siblings_tx, siblings) = watch::channel(cancel.borrow().clone());
                let siblings_tx = Arc::new(siblings_tx);
                {
                    let mut outer = cancel.clone();
                    let forward = Arc::clone(&siblings_tx);
                    tokio::spawn(async move {
                        while outer.changed().await.is_ok() {
                            let reason = outer.borrow().clone();
                            if reason.is_some() {
                                let _ = forward.send(reason);
                                break;
                            }
                        }
                    });
                }
                let mut handles = Vec::with_capacity(jobs.len());
                for job in &jobs {
                    let app = Arc::clone(self);
                    let (sandbox, run, job) = (Arc::clone(&sandbox), run.clone(), job.clone());
                    let checkout = checkout_name(&job, &jobs);
                    let job_cancel = siblings.clone();
                    let stop_siblings = Arc::clone(&siblings_tx);
                    handles.push(tokio::spawn(async move {
                        Arc::clone(&app)
                            .ci_job(&sandbox, &run, &job, &checkout, job_cancel)
                            .await;
                        let failed =
                            app.deps
                                .store
                                .ci_jobs(&run.id, false)
                                .await
                                .is_ok_and(|all| {
                                    all.iter()
                                        .any(|j| j.id == job.id && j.state == RunState::Failed)
                                });
                        if failed && stop_siblings.borrow().is_none() {
                            let _ = stop_siblings
                                .send(Some(format!("stopped early: job {} failed", job.name)));
                        }
                    }));
                }
                for handle in handles {
                    let _ = handle.await;
                }
            }
            (_, prepared) => {
                let reason = prepared
                    .err()
                    .unwrap_or_else(|| "the sandbox was removed".to_owned());
                for job in &jobs {
                    let _ = store
                        .transition_ci_job(
                            &job.id,
                            &[RunState::Queued],
                            RunState::Error,
                            &CiJobUpdate {
                                reason: Some(reason.clone()),
                                ended_at: Some(self.now()),
                                ..CiJobUpdate::default()
                            },
                        )
                        .await;
                }
            }
        }
        let jobs = store.ci_jobs(&run.id, false).await.unwrap_or_default();
        let states: Vec<RunState> = jobs.iter().map(|j| j.state).collect();
        let mut state = dom::aggregate(&states);
        if state.is_active() {
            // Every job task ended: an active job here lost its runner.
            state = RunState::Error;
        }
        if cancel.borrow().is_some() {
            // A cancelled run (superseded or stopped) ends cancelled whatever its killed jobs
            // reported: a job killed while it was starting can end as an error (a slow host
            // loses that race), and the run must not then read as broken.
            state = RunState::Cancelled;
        }
        let reason = match state {
            RunState::Cancelled => cancel
                .borrow()
                .clone()
                .or_else(|| first_reason(&jobs, state)),
            RunState::Error => first_reason(&jobs, state),
            _ => None,
        };
        let end = CiRunUpdate {
            reason,
            ended_at: Some(self.now()),
            ..CiRunUpdate::default()
        };
        if let Err(err) = store
            .transition_ci_run(&run.id, &[RunState::Running], state, &end)
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "recording a CI run's end failed");
        }
        tracing::info!(run = %run.id, state = state.as_str(), "CI run ended");
        self.ci_publish(&run.id).await;
        // A merge armed while these checks ran happens now (SPEC-011).
        let app = Arc::clone(self);
        let sandbox = run.sandbox_id.clone();
        tokio::spawn(async move { app.merge_armed(&sandbox).await });
    }

    /// Makes sure the run's commit is in the sandbox clone (fetching once if not).
    async fn ci_prepare(&self, sandbox: &SandboxRecord, run: &CiRunRecord) -> Result<(), String> {
        let repo = self.sandbox_repo(&sandbox.id);
        if matches!(
            self.deps.git.resolve_commit(&repo, &run.sha).await,
            Ok(Some(_))
        ) {
            return Ok(());
        }
        if let Err(err) = self.dlv_refresh_clone(sandbox).await {
            return Err(format!(
                "fetching the repository failed: {}",
                err.public_message()
            ));
        }
        match self.deps.git.resolve_commit(&repo, &run.sha).await {
            Ok(Some(_)) => Ok(()),
            _ => Err(format!("commit {} is not in the repository", run.sha)),
        }
    }

    /// Runs one job: waits for a slot (or cancellation), runs it and records the end.
    async fn ci_job(
        self: Arc<Self>,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        checkout: &str,
        mut cancel: watch::Receiver<Option<String>>,
    ) {
        let store = &self.deps.store;
        let slots = self.ci_slots();
        let permit = tokio::select! {
            permit = slots.acquire_owned() => permit.ok(),
            reason = wait_cancel(&mut cancel) => {
                let _ = store
                    .transition_ci_job(
                        &job.id,
                        &[RunState::Queued],
                        RunState::Cancelled,
                        &CiJobUpdate {
                            reason: Some(reason),
                            ended_at: Some(self.now()),
                            ..CiJobUpdate::default()
                        },
                    )
                    .await;
                return;
            }
        };
        let Some(_permit) = permit else {
            return;
        };
        let started = CiJobUpdate {
            started_at: Some(self.now()),
            ..CiJobUpdate::default()
        };
        if !matches!(
            store
                .transition_ci_job(&job.id, &[RunState::Queued], RunState::Running, &started)
                .await,
            Ok(true)
        ) {
            return;
        }
        let mut ring = OutputRing::new(OUTPUT_BYTES);
        let ended = self
            .ci_job_process(sandbox, run, job, checkout, &mut ring, &mut cancel)
            .await;
        let (state, exit_code, reason) = match ended {
            Ok(Ended::Exit(exit)) if exit.spawn_failed => (
                RunState::Error,
                None,
                Some("the job could not be started".to_owned()),
            ),
            Ok(Ended::Exit(exit)) if exit.is_clean() => (RunState::Passed, Some(0), None),
            Ok(Ended::Exit(exit)) => {
                let code = exit.code.or_else(|| exit.signal.map(|signal| 128 + signal));
                (RunState::Failed, code, None)
            }
            Ok(Ended::Cancelled(reason)) => (RunState::Cancelled, None, Some(reason)),
            Ok(Ended::TimedOut) => (
                RunState::Error,
                None,
                Some(format!(
                    "timed out after {}",
                    dom::format_duration(
                        i64::try_from(self.config.ci.job_timeout.as_millis()).unwrap_or(i64::MAX)
                    )
                )),
            ),
            Ok(Ended::OutputLimit) => (
                RunState::Error,
                None,
                Some("the job wrote more than 256 MiB of output".to_owned()),
            ),
            Ok(Ended::Lost) => (
                RunState::Error,
                None,
                Some("the job's process host was lost".to_owned()),
            ),
            Err(reason) => (RunState::Error, None, Some(reason)),
        };
        if let Some(reason) = &reason {
            ring.push_str(&format!("\n[yulius] {reason}\n"));
        }
        let end = CiJobUpdate {
            reason,
            exit_code,
            output: Some(ring.text()),
            started_at: None,
            ended_at: Some(self.now()),
        };
        if let Err(err) = store
            .transition_ci_job(&job.id, &[RunState::Running], state, &end)
            .await
        {
            tracing::warn!(job = %job.id, error = %err, "recording a CI job's end failed");
        }
    }

    fn ci_job_env(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        home: &Path,
        cache: &Path,
    ) -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        let mut env = vec![
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), path),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("CI".to_owned(), "true".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ("YULIUS_CI".to_owned(), "1".to_owned()),
            ("YULIUS_CI_RUN".to_owned(), run.id.clone()),
            ("YULIUS_CI_JOB".to_owned(), job.name.clone()),
            ("YULIUS_CI_SHA".to_owned(), run.sha.clone()),
            ("YULIUS_CI_REF".to_owned(), run.reference.key()),
            (
                "YULIUS_CI_CACHE".to_owned(),
                cache.to_string_lossy().into_owned(),
            ),
            ("YULIUS_SANDBOX".to_owned(), sandbox.slug.clone()),
        ];
        if let Some(value) = &job.matrix {
            env.push(("YULIUS_CI_MATRIX".to_owned(), value.clone()));
        }
        env
    }

    /// The job's containment: the sandbox's holder when it is isolated (SPEC-002 ADR-22: the
    /// job sees the packages the sandbox installed and its private network), else the previous
    /// containment.
    fn ci_containment(
        &self,
        sandbox: &SandboxRecord,
        home: &Path,
        join: Option<crate::ports::isolation::SandboxJoin>,
    ) -> Containment {
        if !self.config.contained {
            return Containment::None;
        }
        Containment::Bubblewrap {
            sandbox_root: self.sandbox_dir(&sandbox.id),
            home: home.to_path_buf(),
            writable: Vec::new(),
            readonly: self
                .config
                .contained_readonly
                .iter()
                .cloned()
                .chain(join.iter().flat_map(|j| j.extra_readonly.iter().cloned()))
                .collect(),
            binds: Vec::new(),
            identity: None,
            join: join.map(Box::new),
        }
    }

    async fn ci_job_process(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        checkout_name: &str,
        ring: &mut OutputRing,
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<Ended, String> {
        let ci_dir = self.ci_dir(&sandbox.id);
        let checkout = ci_dir.join("jobs").join(checkout_name);
        let (home, cache, logs) = (
            ci_dir.join("home"),
            ci_dir.join("cache"),
            ci_dir.join("logs"),
        );
        for dir in [&home, &cache, &logs] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("preparing the CI directory failed: {e}"))?;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        self.deps
            .git
            .ci_checkout(&repo, &checkout, &run.sha, &ci_dir)
            .await
            .map_err(|err| format!("checking out {} failed: {}", run.sha, err.public_message()))?;
        let log = logs.join(format!("{}.log", job.id));
        let _ = std::fs::remove_file(&log);
        std::fs::File::create(&log).map_err(|e| format!("creating the job log failed: {e}"))?;
        // Held until the job ends: the sandbox's holder is not stopped as idle meanwhile.
        let (join, _lease) = self
            .isolation_join(sandbox)
            .await
            .map_err(|e| e.public_message())?
            .unzip();
        let plan = LaunchPlan {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                WRAPPER.into(),
                "yulius-ci".into(),
                log.to_string_lossy().into_owned(),
                job.command.clone(),
            ],
            cwd: checkout.clone(),
            env: self.ci_job_env(sandbox, run, job, &home, &cache),
            secret_env: Vec::new(),
            files: Vec::new(),
            containment: self.ci_containment(sandbox, &home, join),
        };
        let run_dir = self.ci_run_dir(&job.id);
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| format!("creating the run directory failed: {e}"))?;
        let token = self.deps.crypto.random_bytes();
        let outcome = self
            .ci_watch(&run_dir, job, &plan, &token, &log, ring, cancel)
            .await;
        let _ = std::fs::remove_dir_all(&run_dir);
        let _ = std::fs::remove_file(&log);
        outcome
    }

    #[allow(clippy::too_many_arguments)] // One job's whole context; split further it only moves.
    async fn ci_watch(
        &self,
        run_dir: &Path,
        job: &CiJobRecord,
        plan: &LaunchPlan,
        token: &[u8; 32],
        log: &Path,
        ring: &mut OutputRing,
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<Ended, String> {
        let host = &self.deps.session_host;
        host.spawn(run_dir, &job.id, &job.run_id, plan, token, (160, 48))
            .await
            .map_err(|err| format!("the job could not be started: {}", err.public_message()))?;
        let attached = match host.attach(run_dir, token).await {
            Ok(attached) => attached,
            Err(err) => {
                let _ = host.terminate(run_dir);
                return Err(format!(
                    "the job's process host did not answer: {}",
                    err.public_message()
                ));
            }
        };
        let (mut reader, mut writer) = (attached.reader, attached.writer);
        let mut tail = LogTail::new(log.to_path_buf());
        let deadline = tokio::time::Instant::now() + self.config.ci.job_timeout;
        let mut tick = tokio::time::interval(READ_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ticks = 0u32;
        let mut stop: Option<Ended> = None;
        let ended = loop {
            tokio::select! {
                event = reader.next() => match event {
                    Ok(Some(RunEvent::Exit(exit))) => break stop.unwrap_or(Ended::Exit(exit)),
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break match host.inspect(run_dir) {
                        RunInspection::Exited(exit) => stop.unwrap_or(Ended::Exit(exit)),
                        _ => {
                            let _ = host.terminate(run_dir);
                            stop.unwrap_or(Ended::Lost)
                        }
                    },
                },
                reason = wait_cancel(cancel), if stop.is_none() => {
                    let _ = writer.signal(RunSignal::Kill).await;
                    stop = Some(Ended::Cancelled(reason));
                }
                () = tokio::time::sleep_until(deadline), if stop.is_none() => {
                    let _ = writer.signal(RunSignal::Kill).await;
                    stop = Some(Ended::TimedOut);
                }
                () = tokio::time::sleep(KILL_GRACE), if stop.is_some() => {
                    // A killed job whose exit is not seen: its host goes (it reaps the group).
                    let _ = host.terminate(run_dir);
                    break stop.take().unwrap_or(Ended::Lost);
                }
                _ = tick.tick() => {
                    match tail.read(ring).await {
                        Ok(size) if size > MAX_LOG_FILE_BYTES && stop.is_none() => {
                            let _ = writer.signal(RunSignal::Kill).await;
                            stop = Some(Ended::OutputLimit);
                        }
                        _ => {}
                    }
                    ticks += 1;
                    if ticks.is_multiple_of(FLUSH_EVERY_TICKS) {
                        let _ = self.deps.store.set_ci_job_output(&job.id, &ring.text()).await;
                    }
                }
            }
        };
        // What the job wrote last (bounded: one more read of at most a chunk per pass).
        for _ in 0..8 {
            match tail.read(ring).await {
                Ok(_) if tail.caught_up() => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        Ok(ended)
    }
}

fn first_reason(jobs: &[CiJobRecord], state: RunState) -> Option<String> {
    jobs.iter()
        .filter(|job| job.state == state)
        .find_map(|job| {
            job.reason
                .clone()
                .map(|reason| format!("{}: {reason}", job.name))
        })
}

/// Resolves once the run is cancelled, with the reason.
async fn wait_cancel(cancel: &mut watch::Receiver<Option<String>>) -> String {
    loop {
        if let Some(reason) = cancel.borrow().clone() {
            return reason;
        }
        if cancel.changed().await.is_err() {
            // The sender is gone (the run ended): never resolves.
            std::future::pending::<()>().await;
        }
    }
}

/// Follows a job's log file.
struct LogTail {
    path: PathBuf,
    offset: u64,
    size: u64,
    strip: AnsiStripper,
}

impl LogTail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            size: 0,
            strip: AnsiStripper::default(),
        }
    }

    fn caught_up(&self) -> bool {
        self.offset >= self.size
    }

    /// Reads at most [`READ_CHUNK`] new bytes into `ring`; returns the file size.
    async fn read(&mut self, ring: &mut OutputRing) -> std::io::Result<u64> {
        let mut file = tokio::fs::File::open(&self.path).await?;
        self.size = file.metadata().await?.len();
        if self.size <= self.offset {
            return Ok(self.size);
        }
        // Skip what the ring would discard anyway when far behind.
        let behind = self.size - self.offset;
        let keep = u64::try_from(OUTPUT_BYTES).unwrap_or(u64::MAX);
        if behind > keep.saturating_mul(4) {
            let skipped = behind - keep;
            ring.push_str(&format!("\n[yulius] {skipped} bytes of output skipped\n"));
            self.offset += skipped;
        }
        file.seek(std::io::SeekFrom::Start(self.offset)).await?;
        let want = usize::try_from(self.size - self.offset)
            .unwrap_or(READ_CHUNK)
            .min(READ_CHUNK);
        let mut buf = vec![0u8; want];
        let mut filled = 0;
        while filled < want {
            let n = file.read(&mut buf[filled..]).await?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        self.offset += u64::try_from(filled).unwrap_or(0);
        let mut clean = Vec::with_capacity(filled);
        self.strip.push(&buf[..filled], &mut clean);
        ring.push(&clean);
        Ok(self.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&[u8]]) -> String {
        let mut stripper = AnsiStripper::default();
        let mut out = Vec::new();
        for chunk in chunks {
            stripper.push(chunk, &mut out);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn escapes_are_removed_across_chunks() {
        assert_eq!(strip(&[b"\x1b[1;31mred\x1b[0m plain"]), "red plain");
        assert_eq!(strip(&[b"a\x1b[", b"32", b"mb"]), "ab");
        assert_eq!(strip(&[b"\x1b]0;title\x07x"]), "x");
        assert_eq!(strip(&[b"\x1b]8;;http://x\x1b\\link"]), "link");
        assert_eq!(strip(&[b"one\r\ntwo\rthree\n"]), "one\ntwo\nthree\n");
        assert_eq!(strip(&[b"bell\x07\x08!"]), "bell!");
    }

    #[test]
    fn matrix_siblings_get_their_own_checkouts() {
        let job = |id: &str, key: &str| CiJobRecord {
            id: id.into(),
            run_id: "r".into(),
            ordinal: 0,
            name: key.into(),
            key: key.into(),
            command: "x".into(),
            matrix: None,
            state: RunState::Queued,
            reason: None,
            exit_code: None,
            output: String::new(),
            started_at: None,
            ended_at: None,
        };
        let jobs = vec![job("a", "lint"), job("b", "test"), job("c", "test")];
        assert_eq!(checkout_name(&jobs[0], &jobs), "lint");
        assert_eq!(checkout_name(&jobs[1], &jobs), "test.1");
        assert_eq!(checkout_name(&jobs[2], &jobs), "test.2");
    }
}
