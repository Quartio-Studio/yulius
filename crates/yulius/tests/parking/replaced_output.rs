//! SPEC-614 (#1624): output a run printed that reaches the task's terminal only after a
//! replacement run attached is not shown on the replacement's screen, with real processes
//! (supervisor → detached `yulius ptyhost` → fake agent). A session host wrapping the real one
//! injects synthetic output into a run's own reader and holds it there until the test releases it.

use crate::support::{TestResult, eventually};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{Notify, mpsc};
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::error::AppResult;
use yulius_app::hub::HubFrame;
use yulius_app::ports::session::{
    AttachedRun, LaunchPlan, RunEvent, RunInspection, RunReader, SessionHost, SpawnedRun,
};
use yulius_contracts::api::secrets::AgentSecretInput;
use yulius_domain::ids::{TaskId, UserId};

use super::{
    Error, FakeAgentOptions, Point, answered, data_dir, hooked, locate_fake_agent,
    park_through_terminal_exit, pause_at_key, request_one_park, runtime, screen,
};
use crate::support::live::pool::ScriptedUsage;

/// What a held reader hands on once released, and the signals around it.
struct Held {
    events: Vec<RunEvent>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    delivered: Arc<Notify>,
}

/// The real session host, whose readers take synthetic events from the test.
struct HoldingHost {
    inner: PtySessionHost,
    readers: Mutex<HashMap<PathBuf, mpsc::UnboundedSender<Held>>>,
}

/// Signals of one held injection: the reader took it, the test releases it, and the reader
/// asked for its next event after handing on every injected one (so their handling is over).
struct Injection {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    delivered: Arc<Notify>,
}

impl HoldingHost {
    fn new() -> Self {
        Self {
            inner: PtySessionHost::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius"))),
            readers: Mutex::new(HashMap::new()),
        }
    }

    /// Makes the reader of the run in `run_dir` stop at its next event, wait for the release and
    /// then hand on `events` before anything more of its host.
    fn hold(&self, run_dir: &Path, events: Vec<RunEvent>) -> Result<Injection, Error> {
        let injection = Injection {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            delivered: Arc::new(Notify::new()),
        };
        self.readers
            .lock()
            .map_err(|_| "readers")?
            .get(run_dir)
            .ok_or("the run's reader is not attached")?
            .send(Held {
                events,
                entered: Arc::clone(&injection.entered),
                release: Arc::clone(&injection.release),
                delivered: Arc::clone(&injection.delivered),
            })
            .map_err(|_| "the run's reader is gone")?;
        Ok(injection)
    }
}

/// A run's reader: its host's events, forwarded by a task so that waiting for one can give way
/// to an injection without losing what the host sent.
struct HoldingReader {
    host: mpsc::UnboundedReceiver<AppResult<Option<RunEvent>>>,
    held: mpsc::UnboundedReceiver<Held>,
    queue: VecDeque<RunEvent>,
    delivered: Option<Arc<Notify>>,
}

#[async_trait::async_trait]
impl RunReader for HoldingReader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        if let Some(event) = self.queue.pop_front() {
            return Ok(Some(event));
        }
        if let Some(delivered) = self.delivered.take() {
            delivered.notify_one();
        }
        tokio::select! {
            biased;
            Some(held) = self.held.recv() => {
                held.entered.notify_one();
                held.release.notified().await;
                self.queue = held.events.into();
                self.delivered = Some(held.delivered);
                Ok(self.queue.pop_front())
            }
            event = self.host.recv() => event.unwrap_or(Ok(None)),
        }
    }
}

#[async_trait::async_trait]
impl SessionHost for HoldingHost {
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
        let attached = self.inner.attach(run_dir, token).await?;
        let mut inner = attached.reader;
        let (to_reader, host) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let event = inner.next().await;
                let more = matches!(event, Ok(Some(_)));
                if to_reader.send(event).is_err() || !more {
                    return;
                }
            }
        });
        let (injector, held) = mpsc::unbounded_channel();
        if let Ok(mut readers) = self.readers.lock() {
            readers.insert(run_dir.to_path_buf(), injector);
        }
        Ok(AttachedRun {
            welcome: attached.welcome,
            reader: Box::new(HoldingReader {
                host,
                held,
                queue: VecDeque::new(),
                delivered: None,
            }),
            writer: attached.writer,
        })
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        self.inner.inspect(run_dir)
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        self.inner.terminate(run_dir)
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        self.inner.background_shells(child_pid)
    }
}

/// The parking fixture (idle sessions park at once, a stop escalates after a second) on `host`.
async fn app_on(data: &Path, host: Arc<HoldingHost>) -> Result<Arc<App>, Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let agent = FakeAgentOptions {
        answers_first_message: true,
        resume_prompts: true,
        ..hooked()
    };
    let (app, _) = crate::support::live::Live {
        usage: Arc::new(ScriptedUsage::default()),
        clock: Arc::new(yulius::wiring::clock::SystemClock),
        host: Some(host),
        ..crate::support::live::Live::new(Some(&fake), agent)
    }
    .start(data, 0, |config| {
        config.max_live_sessions = 8;
        config.park_after_ms = 1;
        config.stop_escalation_ms = 1000;
    })
    .await?;
    Ok(app)
}

/// Output frames' text a viewer received so far.
fn received(frames: &mut tokio::sync::broadcast::Receiver<HubFrame>) -> String {
    let mut text = String::new();
    while let Ok(frame) = frames.try_recv() {
        if let HubFrame::Output { bytes, .. } | HubFrame::Snapshot { bytes, .. } = frame {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    text
}

/// Resumes the parked or lost task with a message and waits for the replacement's answer.
async fn resume_in_replacement(
    app: &Arc<App>,
    id: &TaskId,
    caller: &yulius_app::coordination::ProjectCaller,
) -> TestResult {
    let answer = app
        .tool_task_message(
            caller,
            &json!({"number": 1, "text": "Text for the second run only"}),
        )
        .await?;
    assert_eq!(answer["delivered"], "resumed");
    let answered = eventually(
        "the replacement's answer",
        Duration::from_secs(30),
        || async { screen(app, id).contains("Text for the second run only") },
    )
    .await;
    if answered.is_err() {
        eprintln!("screen:\n{}", screen(app, id));
    }
    answered
}

const HELD: &str = "Old run chunk held in transit";

/// AC-1 (TRO-001, TRO-004): a chunk of the first run held between its host and the hub while
/// that run is lost and a replacement attaches is not drawn on the replacement's screen nor sent
/// to its viewers once released; the replacement's own output still reaches both.
#[test]
fn a_held_chunk_of_an_old_run_does_not_reach_its_replacement() -> TestResult {
    held_event_of_an_old_run(|seq| RunEvent::Output {
        seq,
        bytes: format!("\r\n{HELD}\r\n").into_bytes(),
    })
}

/// AC-3 (TRO-002): a host snapshot of the first run held the same way does not replace the
/// replacement's screen once released.
#[test]
fn a_held_snapshot_of_an_old_run_does_not_reach_its_replacement() -> TestResult {
    held_event_of_an_old_run(|seq| RunEvent::Snapshot {
        seq,
        bytes: format!("\x1b[H\x1b[2J{HELD}").into_bytes(),
    })
}

/// Holds the event `held` makes (it shows [`HELD`]) in the first run's reader while that run is
/// lost and a replacement attaches, then releases it and checks the replacement's screen and
/// viewers.
fn held_event_of_an_old_run(held: fn(u64) -> RunEvent) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let host = Arc::new(HoldingHost::new());
        let app = app_on(&data, Arc::clone(&host)).await?;
        let (id, caller) = answered(&app, &data).await?;
        let old = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("the first run")?;
        let seq = app.runtime(&id).hub.output_seq() + 1;
        let held = host.hold(Path::new(&old.run_dir), vec![held(seq)])?;
        tokio::time::timeout(Duration::from_secs(10), held.entered.notified()).await?;
        // The park ends the first run; its reader, held, loses it to the escalation.
        let mut frames = app.runtime(&id).hub.watch();
        request_one_park(&app).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if matches!(frames.recv().await?, HubFrame::Exit { .. }) {
                    return Ok::<_, tokio::sync::broadcast::error::RecvError>(());
                }
            }
        })
        .await??;
        resume_in_replacement(&app, &id, &caller).await?;
        let replacement = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("the replacement run")?;
        assert_ne!(replacement.id, old.id);

        let mut viewer = app.runtime(&id).hub.watch();
        held.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), held.delivered.notified()).await?;
        let shown = screen(&app, &id);
        assert!(
            !shown.contains(HELD) && shown.contains("Text for the second run only"),
            "the old run's event changed the replacement's screen:\n{shown}"
        );
        let seen = received(&mut viewer);
        assert!(
            !seen.contains(HELD),
            "the old run's event reached the replacement's viewers: {seen:?}"
        );

        app.tool_task_message(
            &caller,
            &json!({"number": 1, "text": "Later text of the replacement"}),
        )
        .await?;
        eventually(
            "the replacement's later answer",
            Duration::from_secs(30),
            || async { screen(&app, &id).contains("Later text of the replacement") },
        )
        .await?;
        let seen = received(&mut viewer);
        assert!(
            seen.contains("Later text of the replacement") && !seen.contains(HELD),
            "{seen:?}"
        );
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

const SECRET: &str = concat!("tail_", "value_9f2c71ab3d");

/// AC-2 (TRO-001, TRO-004): the first run prints synthetic text ending in the beginning of an
/// agent secret, so its terminal holds that tail. The tail's release is held while the run parks
/// (its parked screen shows until the replacement attaches) and a replacement resumes it; once
/// released, the tail is neither drawn on the replacement's screen nor sent to its viewers.
#[test]
fn a_delayed_tail_of_an_old_run_does_not_reach_its_replacement() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let host = Arc::new(HoldingHost::new());
        let app = app_on(&data, Arc::clone(&host)).await?;
        let (id, caller) = answered(&app, &data).await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let user = app
            .deps
            .store
            .user(&UserId::parse("u-admin")?)
            .await?
            .ok_or("admin")?;
        app.create_agent_secret(
            &AuthContext {
                user,
                csrf_token: None,
                session_hash: None,
                token: None,
            },
            task.sandbox_id.as_str(),
            &AgentSecretInput {
                name: "TAIL_KEY".into(),
                hosts: vec!["api.tail.test".into()],
                value: SECRET.into(),
                ..AgentSecretInput::default()
            },
        )
        .await?;
        let old = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("the first run")?;
        let tail = &SECRET[..10];
        let (releasing, release_tail) = pause_at_key(Point::TerminalTailReady, old.id.as_str());
        let tail_done = Arc::new(Notify::new());
        let done = Arc::clone(&tail_done);
        yulius_app::race::on(
            Point::TerminalTailDone,
            Path::new(old.id.as_str()),
            move || done.notify_one(),
        );
        let seq = app.runtime(&id).hub.output_seq() + 1;
        let printed = host.hold(
            Path::new(&old.run_dir),
            vec![RunEvent::Output {
                seq,
                bytes: format!("\r\nOld run tail {tail}").into_bytes(),
            }],
        )?;
        printed.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), printed.delivered.notified()).await?;
        tokio::time::timeout(Duration::from_secs(10), releasing.notified()).await?;
        assert!(screen(&app, &id).contains("Old run tail"));
        assert!(!screen(&app, &id).contains(tail), "the tail is held");

        park_through_terminal_exit(&app, &id).await?;
        let parked = screen(&app, &id);
        assert!(
            parked.contains("ECHO: Budget: answer with an echo") && !parked.contains(tail),
            "the parked screen shows until a replacement attaches:\n{parked}"
        );
        resume_in_replacement(&app, &id, &caller).await?;

        let mut viewer = app.runtime(&id).hub.watch();
        release_tail.notify_one();
        tokio::time::timeout(Duration::from_secs(10), tail_done.notified()).await?;
        assert!(
            !screen(&app, &id).contains(tail),
            "the old run's tail was drawn on the replacement's screen:\n{}",
            screen(&app, &id)
        );
        let seen = received(&mut viewer);
        assert!(
            !seen.contains(tail),
            "the old run's tail reached the replacement's viewers: {seen:?}"
        );
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}
