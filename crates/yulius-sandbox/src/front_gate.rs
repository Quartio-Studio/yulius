//! The Docker front's admission and connection ends (SPEC-494 DFJ-001 to DFJ-005, ADR-0344).
//!
//! Every mode of the front admits through one [`Gate`]: [`MAX_CONNECTIONS`] places, of which the
//! one idle longest past [`IDLE_FLOOR`] gives way to a new connection at the limit (SPEC-355's
//! rule). With every place active, the new connection is answered a Docker-API 503 saying so
//! ([`AT_LIMIT`]), at most [`REFUSING_AT_ONCE`] at once.
//!
//! [`Gate::splice`] passes bytes both ways and ends the connection in bounded time once one side
//! is done: a client that hung up entirely (its peer closed: `POLLHUP`, looked at every
//! [`HANGUP_CHECK`]) ends it at once, an engine that ended its side gives the client
//! [`CLOSE_GRACE`]. A client that only shut its writing keeps reading the engine's answer.
//!
//! Each kind of event is said on stderr (`docker-front.log`) at most once a minute, and the
//! gate's [`DockerFrontState`] is written to `docker-front-state.json` for the server.

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, ReadBuf};
use tokio::net::unix::OwnedReadHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, oneshot};
use yulius_contracts::sandbox::DockerFrontState;

use crate::relay::{Places, Toucher};

pub use crate::relay::{IDLE_FLOOR, MAX_CONNECTIONS};

/// How often a connection whose client stopped sending looks whether the client hung up.
pub const HANGUP_CHECK: Duration = Duration::from_secs(1);
/// Time the client has to close once the engine ended its side.
pub const CLOSE_GRACE: Duration = Duration::from_secs(30);
/// Refusals answered at once; beyond them a connection at the limit is closed unanswered.
pub const REFUSING_AT_ONCE: usize = 16;
/// Least time between two writes of the state file.
pub const STATE_EVERY: Duration = Duration::from_secs(5);
/// Least time between two log lines of one kind.
const WARN_EVERY: Duration = Duration::from_secs(60);
/// The refusal at the limit, for the calls that meet it (DFJ-001, DFJ-007).
pub const AT_LIMIT: &str = "the sandbox's Docker front holds all of its connections and none \
     has been idle long enough to give way. Yulius sees this and restarts the sandbox's Docker \
     when it stays so; nobody needs to be asked. Retry in a minute";

/// What ended a connection, besides both sides closing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Both sides finished.
    Done,
    /// The client hung up entirely; the engine's side was closed.
    HungUp,
    /// The engine ended and the client did not close within the grace.
    Grace,
}

/// The kinds of events the front logs and counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    GaveWay,
    Refused,
    HungUp,
    Grace,
}

impl Kind {
    fn index(self) -> usize {
        self as usize
    }

    fn says(self) -> &'static str {
        match self {
            Self::GaveWay => "at the limit, the longest-idle connection was closed for a new one",
            Self::Refused => "at the limit, a new connection was refused (every connection active)",
            Self::HungUp => "a connection whose client hung up was ended",
            Self::Grace => "a connection whose engine side ended was closed after the grace",
        }
    }
}

/// The front's admission (see the module docs).
pub struct Gate {
    places: Arc<Places>,
    max: usize,
    refusing: Arc<Semaphore>,
    counts: [AtomicU64; 4],
    warned: Mutex<[Option<Instant>; 4]>,
    at_limit_since_ms: Mutex<Option<i64>>,
    hangup_check: Duration,
    close_grace: Duration,
}

/// An admitted connection: its client and its place (freed when dropped).
pub struct Admitted {
    client: UnixStream,
    toucher: Toucher,
    _place: crate::relay::Place,
}

impl Gate {
    /// A gate of [`MAX_CONNECTIONS`] places and the production bounds.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Self::with(MAX_CONNECTIONS, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE)
    }

    /// A gate of `max` places giving way after `floor`, with these bounds (tests use small ones).
    #[must_use]
    pub fn with(
        max: usize,
        floor: Duration,
        hangup_check: Duration,
        close_grace: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            places: Places::new(max, floor),
            max,
            refusing: Arc::new(Semaphore::new(REFUSING_AT_ONCE)),
            counts: Default::default(),
            warned: Mutex::new([None; 4]),
            at_limit_since_ms: Mutex::new(None),
            hangup_check,
            close_grace,
        })
    }

    /// Accepts on `listener` until it fails, admitting each client and handing it to `handle`.
    pub async fn serve<H, F>(self: Arc<Self>, listener: UnixListener, handle: H)
    where
        H: Fn(Admitted) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let Some(admitted) = self.admit(client) else {
                continue;
            };
            let work = handle(admitted.0);
            let ended = admitted.1;
            tokio::spawn(async move {
                tokio::select! {
                    () = work => {}
                    _ = ended => {}
                }
            });
        }
    }

    /// A place for `client`, giving way or refusing at the limit (DFJ-001). The receiver resolves
    /// once the connection gave way to another.
    fn admit(self: &Arc<Self>, client: UnixStream) -> Option<(Admitted, oneshot::Receiver<()>)> {
        let Some((place, ended, idle)) = self.places.admit() else {
            self.mark_at_limit(true);
            self.event(Kind::Refused, &mut io::stderr().lock());
            if let Ok(permit) = Arc::clone(&self.refusing).try_acquire_owned() {
                tokio::spawn(async move {
                    crate::engine_supervisor::refuse(client, AT_LIMIT).await;
                    drop(permit);
                });
            }
            return None;
        };
        if idle.is_some() {
            self.event(Kind::GaveWay, &mut io::stderr().lock());
        }
        self.mark_at_limit(self.places.held() >= self.max);
        Some((
            Admitted {
                client,
                toucher: place.toucher(),
                _place: place,
            },
            ended,
        ))
    }

    fn mark_at_limit(&self, full: bool) {
        let mut since = self
            .at_limit_since_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match (full, *since) {
            (true, None) => *since = Some(crate::relay::wall_ms()),
            (false, Some(_)) => *since = None,
            _ => {}
        }
    }

    /// Counts an event of `kind` and says it on `out` unless one of its kind was said within a
    /// minute (DFJ-004). A failed write never stops the front.
    fn event(&self, kind: Kind, out: &mut dyn io::Write) {
        self.counts[kind.index()].fetch_add(1, Ordering::Relaxed);
        let mut warned = self.warned.lock().unwrap_or_else(PoisonError::into_inner);
        let last = &mut warned[kind.index()];
        if last.is_some_and(|at| at.elapsed() < WARN_EVERY) {
            return;
        }
        *last = Some(Instant::now());
        drop(warned);
        let state = self.state(false);
        let _ = writeln!(
            out,
            "WARN docker front: {}; held={}/{} gave_way={} refused={} hung_up={} \
             closed_after_grace={}",
            kind.says(),
            state.held,
            state.max,
            state.gave_way,
            state.refused,
            state.hung_up,
            state.closed_after_grace
        );
        let _ = out.flush();
    }

    /// The gate's state now; `engine_running` as the front knows it.
    #[must_use]
    pub fn state(&self, engine_running: bool) -> DockerFrontState {
        let held = self.places.held();
        if held < self.max {
            self.mark_at_limit(false);
        }
        let count = |kind: Kind| self.counts[kind.index()].load(Ordering::Relaxed);
        DockerFrontState {
            pid: std::process::id(),
            held,
            max: self.max,
            gave_way: count(Kind::GaveWay),
            refused: count(Kind::Refused),
            hung_up: count(Kind::HungUp),
            closed_after_grace: count(Kind::Grace),
            at_limit_since_ms: *self
                .at_limit_since_ms
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            engine_running,
        }
    }

    /// Passes bytes between an admitted client and `engine` until both are done, or until the
    /// client hung up or the engine's end outlived the grace (DFJ-002, DFJ-003).
    pub async fn splice(&self, admitted: Admitted, engine: UnixStream) -> End {
        let Admitted {
            client,
            toucher,
            _place,
        } = admitted;
        let end = splice_bounded(
            client,
            engine,
            &toucher,
            self.hangup_check,
            self.close_grace,
        )
        .await;
        drop(_place);
        match end {
            End::HungUp => self.event(Kind::HungUp, &mut io::stderr().lock()),
            End::Grace => self.event(Kind::Grace, &mut io::stderr().lock()),
            End::Done => {}
        }
        end
    }

    /// Answers an admitted client a Docker-API error naming `reason`.
    pub async fn refuse(admitted: Admitted, reason: &str) {
        crate::engine_supervisor::refuse(admitted.client, reason).await;
    }

    /// Writes the state to `path` every [`STATE_EVERY`] when it changed, until aborted;
    /// `engine_running` is asked at each write.
    pub async fn keep_state<R, F>(self: Arc<Self>, path: PathBuf, engine_running: R)
    where
        R: Fn() -> F + Send + Sync,
        F: Future<Output = bool> + Send,
    {
        let mut every = tokio::time::interval(STATE_EVERY);
        let mut written: Option<DockerFrontState> = None;
        loop {
            every.tick().await;
            let state = self.state(engine_running().await);
            if written.as_ref() != Some(&state) {
                let file = path.clone();
                let value = state.clone();
                let done =
                    tokio::task::spawn_blocking(move || crate::holder::write_json(&file, &value))
                        .await;
                if matches!(done, Ok(Ok(()))) {
                    written = Some(state);
                }
            }
        }
    }
}

/// A half of a split connection whose bytes mark the connection active.
struct Touching<S> {
    inner: S,
    toucher: Toucher,
}

impl<S: AsyncRead + Unpin> AsyncRead for Touching<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.toucher.touch();
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Touching<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if matches!(poll, Poll::Ready(Ok(n)) if n > 0) {
            self.toucher.touch();
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Copies `from` to `to` until `from` ends (or either fails), then shuts `to`'s writing and
/// gives `from` back.
async fn pass<R, W>(mut from: R, mut to: W) -> R
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let _ = tokio::io::copy(&mut from, &mut to).await;
    let _ = to.shutdown().await;
    from
}

/// See [`Gate::splice`].
async fn splice_bounded(
    client: UnixStream,
    engine: UnixStream,
    toucher: &Toucher,
    hangup_check: Duration,
    close_grace: Duration,
) -> End {
    let (client_read, client_write) = client.into_split();
    let (engine_read, engine_write) = engine.into_split();
    let up = pass(
        Touching {
            inner: client_read,
            toucher: toucher.clone(),
        },
        engine_write,
    );
    let down = pass(
        engine_read,
        Touching {
            inner: client_write,
            toucher: toucher.clone(),
        },
    );
    tokio::pin!(up, down);
    tokio::select! {
        client_read = &mut up => {
            // The client stopped sending: it may still read, or be gone entirely.
            let mut check = tokio::time::interval(hangup_check);
            loop {
                tokio::select! {
                    _ = &mut down => return End::Done,
                    _ = check.tick() => {
                        if hung_up(&client_read.inner) {
                            return End::HungUp;
                        }
                    }
                }
            }
        }
        _ = &mut down => {
            // The engine ended its side, passed on as the client's end of reading.
            match tokio::time::timeout(close_grace, &mut up).await {
                Ok(_) => End::Done,
                Err(_) => End::Grace,
            }
        }
    }
}

/// Whether the client's socket's peer closed entirely (`POLLHUP`). A peer that only shut its
/// writing is not hung up: it still reads.
fn hung_up(client: &OwnedReadHalf) -> bool {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::AsFd as _;
    let stream: &UnixStream = client.as_ref();
    let mut fds = [PollFd::new(stream.as_fd(), PollFlags::empty())];
    matches!(poll(&mut fds, PollTimeout::ZERO), Ok(n) if n > 0)
        && fds[0]
            .revents()
            .is_some_and(|r| r.intersects(PollFlags::POLLHUP | PollFlags::POLLERR))
}

/// What the server journals of a front's state (SPEC-494 DFJ-005): each kind of event that grew
/// since the last state it read, at most once a minute per kind.
#[derive(Debug, Default)]
pub struct StateWatch {
    seen: Option<DockerFrontState>,
    said: [Option<Instant>; 4],
}

/// A kind of event that grew, as the journal says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grew {
    /// What happened.
    pub what: &'static str,
    /// How many since the last state read.
    pub new: u64,
    /// How many since the front started.
    pub total: u64,
}

impl StateWatch {
    /// Takes `state` (read at `now`) and returns what to journal. A state of another front
    /// (another pid) counts from zero.
    pub fn observe(&mut self, state: &DockerFrontState, now: Instant) -> Vec<Grew> {
        let before = self
            .seen
            .as_ref()
            .filter(|seen| seen.pid == state.pid)
            .map_or([0; 4], counts);
        let after = counts(state);
        self.seen = Some(state.clone());
        let mut grew = Vec::new();
        for kind in [Kind::GaveWay, Kind::Refused, Kind::HungUp, Kind::Grace] {
            let i = kind.index();
            if after[i] <= before[i] || self.said[i].is_some_and(|at| now - at < WARN_EVERY) {
                continue;
            }
            self.said[i] = Some(now);
            grew.push(Grew {
                what: kind.says(),
                new: after[i] - before[i],
                total: after[i],
            });
        }
        grew
    }
}

fn counts(state: &DockerFrontState) -> [u64; 4] {
    [
        state.gave_way,
        state.refused,
        state.hung_up,
        state.closed_after_grace,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker_front::Front;
    use crate::holder::StatePaths;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::AsyncReadExt as _;
    use yulius_contracts::sandbox::DockerServe;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// An engine as the incident's: `hold` is never answered nor ended (an exec of a container
    /// whose task cannot be deleted), `ping` is answered and ended, `late` is answered after a
    /// while, `bye` is answered and the engine's side ends at once. `closed` counts the
    /// connections whose front side the engine saw close.
    struct Engine {
        socket: PathBuf,
        closed: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Engine {
        fn start(dir: &std::path::Path) -> io::Result<Self> {
            let socket = dir.join("engine.sock");
            let listener = UnixListener::bind(&socket)?;
            let closed = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&closed);
            let task = tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let counter = Arc::clone(&counter);
                    tokio::spawn(async move {
                        let mut buf = [0u8; 64];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        match &buf[..n] {
                            b"ping" => {
                                let _ = stream.write_all(b"engine:ping").await;
                            }
                            b"late" => {
                                tokio::time::sleep(Duration::from_millis(600)).await;
                                let _ = stream.write_all(b"engine:late").await;
                            }
                            b"bye" => {
                                let _ = stream.write_all(b"engine:bye").await;
                                return;
                            }
                            _ => {
                                // Never answers, never ends its side; sees the front's close.
                                let mut rest = [0u8; 64];
                                loop {
                                    match stream.read(&mut rest).await {
                                        Ok(0) | Err(_) => break,
                                        Ok(_) => {}
                                    }
                                }
                                // The front only shut its writing: wait for the whole close,
                                // sending nothing (an exec that prints nothing).
                                while !peer_closed(&stream) {
                                    tokio::time::sleep(Duration::from_millis(20)).await;
                                }
                                counter.fetch_add(1, Ordering::SeqCst);
                                return;
                            }
                        }
                        let _ = stream.shutdown().await;
                    });
                }
            });
            Ok(Self {
                socket,
                closed,
                task,
            })
        }
    }

    fn peer_closed(stream: &UnixStream) -> bool {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        use std::os::fd::AsFd as _;
        let mut fds = [PollFd::new(stream.as_fd(), PollFlags::empty())];
        matches!(poll(&mut fds, PollTimeout::ZERO), Ok(n) if n > 0)
    }

    /// A front relaying to `engine` through `gate`.
    fn front(dir: &std::path::Path, engine: &Engine, gate: Arc<Gate>) -> io::Result<Front> {
        let paths = StatePaths::new(dir.to_path_buf());
        Front::start_through(
            &paths,
            &DockerServe::Relay {
                socket: engine.socket.to_string_lossy().into_owned(),
            },
            &[],
            Box::new(|| Ok(())),
            gate,
        )
    }

    async fn send(socket: &std::path::Path, text: &str) -> io::Result<UnixStream> {
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_all(text.as_bytes()).await?;
        Ok(stream)
    }

    async fn answer(mut stream: UnixStream) -> io::Result<String> {
        let mut out = String::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut out))
            .await
            .map_err(|_| io::Error::other("no answer"))??;
        Ok(out)
    }

    async fn until(what: &str, mut condition: impl FnMut() -> bool) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            if Instant::now() > deadline {
                return Err(io::Error::other(format!("never: {what}")));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    /// SPEC-494 AC-1 (2026-11-15, quartio-v2): a front whose places are all held by calls the
    /// engine never ends still serves a new call; the longest-idle one gives way.
    #[tokio::test]
    async fn a_front_full_of_calls_the_engine_never_ends_still_serves_a_new_one() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(4, Duration::ZERO, HANGUP_CHECK, CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(send(&socket, "hold").await?);
        }
        until("four held", || gate.state(false).held == 4).await?;
        assert_eq!(answer(send(&socket, "ping").await?).await?, "engine:ping");
        let state = gate.state(false);
        assert_eq!(state.gave_way, 1, "{state:?}");
        let mut first = held.remove(0);
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(10), first.read(&mut byte)).await?;
        assert!(
            matches!(read, Ok(0) | Err(_)),
            "the oldest gave way: {read:?}"
        );
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// SPEC-494 AC-1: with every place active, a new call is answered a Docker-API 503 that says
    /// the front is at its limit and Yulius repairs it, never dropped without a word.
    #[tokio::test]
    async fn a_front_of_active_calls_refuses_with_a_docker_error() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(2, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let _a = send(&socket, "hold").await?;
        let _b = send(&socket, "hold").await?;
        until("two held", || gate.state(false).held == 2).await?;
        let refused = answer(send(&socket, "GET /_ping HTTP/1.1\r\n\r\n").await?).await?;
        assert!(refused.starts_with("HTTP/1.1 503 "), "{refused}");
        assert!(
            refused.contains("Docker front holds all of its connections"),
            "{refused}"
        );
        assert!(refused.contains("nobody needs to be asked"), "{refused}");
        let state = gate.state(false);
        assert_eq!((state.refused, state.gave_way), (1, 0), "{state:?}");
        assert!(state.at_limit_since_ms.is_some(), "{state:?}");
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// SPEC-494 AC-2, the leak's root: a client that sent a call and hung up, while the engine
    /// never ends it, frees its place, and the engine's side is closed.
    #[tokio::test]
    async fn a_client_that_hung_up_frees_its_place_though_the_engine_never_ends() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(4, IDLE_FLOOR, Duration::from_millis(50), CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let client = send(&socket, "hold").await?;
        until("held", || gate.state(false).held == 1).await?;
        drop(client);
        until("the place is free", || gate.state(false).held == 0).await?;
        until("the engine's side closed", || {
            engine.closed.load(Ordering::SeqCst) == 1
        })
        .await?;
        assert_eq!(gate.state(false).hung_up, 1);
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// SPEC-494 AC-2: a client that only shut its writing (`docker run -i … < file`) is not hung
    /// up: it still gets the engine's late answer.
    #[tokio::test]
    async fn a_client_that_only_shut_its_writing_still_gets_the_answer() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(4, IDLE_FLOOR, Duration::from_millis(50), CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let mut client = send(&socket, "late").await?;
        client.shutdown().await?;
        assert_eq!(answer(client).await?, "engine:late");
        let state = gate.state(false);
        assert_eq!(state.hung_up, 0, "{state:?}");
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// SPEC-494 AC-3: once the engine ended its side, a client that never closes its own is
    /// closed after the grace.
    #[tokio::test]
    async fn a_client_that_never_closes_after_the_engine_ended_is_closed_after_the_grace()
    -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(4, IDLE_FLOOR, HANGUP_CHECK, Duration::from_millis(200));
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let mut client = send(&socket, "bye").await?;
        let mut head = [0u8; 10];
        client.read_exact(&mut head).await?;
        assert_eq!(&head, b"engine:bye");
        // The engine's end reached the client, which keeps its socket open and silent.
        assert_eq!(client.read(&mut head).await?, 0);
        until("closed after the grace", || {
            gate.state(false).closed_after_grace == 1
        })
        .await?;
        until("the place is free", || gate.state(false).held == 0).await?;
        drop(client);
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// SPEC-494 AC-4: many events of a kind within a minute write one line, with the totals;
    /// another kind writes its own.
    #[test]
    fn events_are_said_once_a_minute_per_kind_with_their_totals() -> TestResult {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let _guard = runtime.enter();
        let gate = Gate::with(4, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE);
        let mut out = Vec::new();
        for _ in 0..5 {
            gate.event(Kind::GaveWay, &mut out);
        }
        gate.event(Kind::HungUp, &mut out);
        let text = String::from_utf8(out)?;
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(
            lines[0].contains("longest-idle connection was closed"),
            "{text}"
        );
        assert!(lines[0].contains("gave_way=1"), "{text}");
        assert!(lines[1].contains("client hung up"), "{text}");
        assert!(
            lines[1].contains("gave_way=5 refused=0 hung_up=1"),
            "{text}"
        );
        Ok(())
    }

    fn state_of(pid: u32, gave_way: u64, hung_up: u64) -> DockerFrontState {
        DockerFrontState {
            pid,
            held: 1,
            max: 4,
            gave_way,
            refused: 0,
            hung_up,
            closed_after_grace: 0,
            at_limit_since_ms: None,
            engine_running: true,
        }
    }

    /// SPEC-494 DFJ-005, AC-4: the server journals each kind that grew once a minute, counting
    /// a new front's events from zero.
    #[test]
    fn the_server_journals_what_grew_once_a_minute_per_kind() {
        let mut watch = StateWatch::default();
        let start = Instant::now();
        let grew = watch.observe(&state_of(7, 3, 0), start);
        assert_eq!(grew.len(), 1, "{grew:?}");
        assert_eq!((grew[0].new, grew[0].total), (3, 3));
        assert!(grew[0].what.contains("longest-idle"), "{grew:?}");
        // Within the minute: more give-ways are not said again, a hang-up is.
        let grew = watch.observe(&state_of(7, 5, 1), start + Duration::from_secs(30));
        assert_eq!(grew.len(), 1, "{grew:?}");
        assert!(grew[0].what.contains("hung up"), "{grew:?}");
        // Nothing grew: nothing said.
        assert!(
            watch
                .observe(&state_of(7, 5, 1), start + Duration::from_secs(90))
                .is_empty()
        );
        // A new front's events count from zero, past the minute.
        let grew = watch.observe(&state_of(8, 2, 0), start + Duration::from_secs(91));
        assert_eq!(grew.len(), 1, "{grew:?}");
        assert_eq!((grew[0].new, grew[0].total), (2, 2));
    }

    /// SPEC-494 root cause, against a real engine (`YULIUS_REAL_DOCKER` names its socket and a
    /// running container): an exec whose client hung up frees its place while the exec runs.
    /// Opt-in: `YULIUS_REAL_DOCKER=/run/yulius/docker/docker.sock:<container> cargo test -p
    /// yulius-sandbox -- --ignored real_engine`.
    #[tokio::test]
    #[ignore = "needs a real Docker engine and a running container"]
    async fn a_real_engines_exec_whose_client_hung_up_frees_its_place() -> TestResult {
        let Ok(real) = std::env::var("YULIUS_REAL_DOCKER") else {
            return Err("YULIUS_REAL_DOCKER is not set".into());
        };
        let (socket, container) = real.rsplit_once(':').ok_or("socket:container")?;
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let gate = Gate::with(4, IDLE_FLOOR, Duration::from_millis(200), CLOSE_GRACE);
        let front = Front::start_through(
            &paths,
            &DockerServe::Relay {
                socket: socket.to_owned(),
            },
            &[],
            Box::new(|| Ok(())),
            Arc::clone(&gate),
        )?;
        let body =
            r#"{"AttachStdin":true,"AttachStdout":true,"AttachStderr":true,"Cmd":["sleep","600"]}"#;
        let created = answer(
            send(
                &paths.docker_socket(),
                &format!(
                    "POST /containers/{container}/exec HTTP/1.1\r\nHost: d\r\nContent-Type: \
                     application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
            .await?,
        )
        .await?;
        let id = created
            .split("\"Id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .ok_or(format!("no exec id: {created}"))?;
        let start = r#"{"Detach":false,"Tty":false}"#;
        let mut exec = send(
            &paths.docker_socket(),
            &format!(
                "POST /exec/{id}/start HTTP/1.1\r\nHost: d\r\nContent-Type: application/json\r\n\
                 Connection: Upgrade\r\nUpgrade: tcp\r\nContent-Length: {}\r\n\r\n{start}",
                start.len()
            ),
        )
        .await?;
        // The whole answer's head, as a client reads it: nothing is left unread (a socket closed
        // with unread bytes resets its peer, which would end the call anyway).
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            exec.read_exact(&mut byte).await?;
            head.push(byte[0]);
        }
        assert!(head.starts_with(b"HTTP/1.1 101"), "not upgraded: {head:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut more = [0u8; 64];
        let pending = tokio::time::timeout(Duration::from_millis(1), exec.read(&mut more)).await;
        assert!(pending.is_err(), "the exec printed: {pending:?}");
        until("held", || gate.state(false).held == 1).await?;
        // The test is killed mid-exec: its client hangs up while `sleep 600` runs.
        drop(exec);
        until("the place is free", || gate.state(false).held == 0).await?;
        assert_eq!(gate.state(false).hung_up, 1);
        front.stop(Duration::from_secs(1)).await;
        Ok(())
    }

    /// SPEC-494 DFJ-005: the front writes its state for the server.
    #[tokio::test]
    async fn the_front_writes_its_state_for_the_server() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(4, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let _held = send(&paths.docker_socket(), "hold").await?;
        let deadline = Instant::now() + STATE_EVERY * 3;
        loop {
            let state = crate::holder::read_json::<DockerFrontState>(&paths.docker_state());
            if let Ok(state) = state
                && state.held == 1
            {
                assert_eq!((state.max, state.pid), (4, std::process::id()));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no state with the held connection"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }
}
