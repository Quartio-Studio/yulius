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
use yulius_domain::connection_health::{ConnectionHealth, Limit, Refusal};

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
     has been idle long enough to give way: retry in a minute. Yulius shows this to the \
     sandbox's people and tells its tasks: do not raise a Needs you about it unless it lasts \
     more than 15 minutes";

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
    state_warned: Mutex<Option<Instant>>,
    at_limit_since_ms: Mutex<Option<i64>>,
    hangup_check: Duration,
    close_grace: Duration,
    state_every: Duration,
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
            state_warned: Mutex::new(None),
            at_limit_since_ms: Mutex::new(None),
            hangup_check,
            close_grace,
            state_every: STATE_EVERY,
        })
    }

    /// This gate writing its state every `every` (tests).
    #[cfg(test)]
    fn writing_every(mut self: Arc<Self>, every: Duration) -> Arc<Self> {
        if let Some(gate) = Arc::get_mut(&mut self) {
            gate.state_every = every;
        }
        self
    }

    /// Accepts on `listener` for as long as it runs, admitting each client and handing it to `handle`.
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
                    crate::engine_supervisor::answer_error(client, AT_LIMIT).await;
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
            "{} WARN docker front: {}; held={}/{} gave_way={} refused={} hung_up={} \
             closed_after_grace={}",
            utc(crate::relay::wall_ms()),
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

    /// Says that the state file could not be written, at most once a minute: the server reads
    /// nothing of the front without it (review round 2).
    fn unwritten(
        &self,
        path: &std::path::Path,
        error: &dyn std::fmt::Display,
        out: &mut dyn io::Write,
    ) {
        let mut last = self
            .state_warned
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < WARN_EVERY) {
            return;
        }
        *last = Some(Instant::now());
        let _ = writeln!(
            out,
            "{} WARN docker front: cannot write its state {}: {error}",
            utc(crate::relay::wall_ms()),
            path.display()
        );
        let _ = out.flush();
    }

    /// The gate's state now; `engine_running` as the front knows it.
    #[must_use]
    pub fn state(&self, engine_running: bool) -> DockerFrontState {
        let held = self.places.held();
        self.mark_at_limit(held >= self.max);
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

    /// Writes the state to `path` at most every [`STATE_EVERY`], when it changed, until aborted;
    /// `engine_running` is asked at each write.
    pub async fn keep_state<R, F>(self: Arc<Self>, path: PathBuf, engine_running: R)
    where
        R: Fn() -> F + Send + Sync,
        F: Future<Output = bool> + Send,
    {
        let mut every = tokio::time::interval(self.state_every);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
                match done {
                    Ok(Ok(())) => written = Some(state),
                    Ok(Err(error)) => self.unwritten(&path, &error, &mut io::stderr().lock()),
                    Err(error) => self.unwritten(&path, &error, &mut io::stderr().lock()),
                }
            }
        }
    }
}

/// `ms` since the epoch as UTC, `2026-11-15T22:04:05Z` (the front's log lines; no clock crate).
fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
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
                            // An ordinary call's client closes too: its engine ends its side a
                            // moment after the front's end of writing (review round 1).
                            return match tokio::time::timeout(hangup_check, &mut down).await {
                                Ok(_) => End::Done,
                                Err(_) => End::HungUp,
                            };
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
    /// The front whose totals were said, by pid.
    front: Option<u32>,
    /// When each kind was last said, and its total then.
    said: [(Option<Instant>, u64); 4],
    /// The front's refusals already reported as its sandbox's trouble.
    reported_refused: u64,
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
    /// This server started the front `pid`: all of its events are new.
    pub fn started(&mut self, pid: u32) {
        self.front = Some(pid);
        self.said = [(None, 0); 4];
        self.reported_refused = 0;
    }

    /// Takes `state` (read at `now`) and returns what to journal. A front first seen here that
    /// this server did not start (one adopted after a restart) has its history journaled once,
    /// but its past refusals are no new trouble (review round 1 of slice 2).
    pub fn observe(&mut self, state: &DockerFrontState, now: Instant) -> Vec<Grew> {
        if self.front != Some(state.pid) {
            self.front = Some(state.pid);
            self.said = [(None, 0); 4];
            self.reported_refused = state.refused;
        }
        let after = counts(state);
        let mut grew = Vec::new();
        for kind in [Kind::GaveWay, Kind::Refused, Kind::HungUp, Kind::Grace] {
            let (last, total) = &mut self.said[kind.index()];
            let now_total = after[kind.index()];
            if now_total <= *total || last.is_some_and(|at| now - at < WARN_EVERY) {
                continue;
            }
            grew.push(Grew {
                what: kind.says(),
                new: now_total - *total,
                total: now_total,
            });
            *last = Some(now);
            *total = now_total;
        }
        grew
    }
}

/// Longest state file the server reads.
const STATE_MAX_BYTES: u64 = 16 * 1024;

/// The state file at `path` when it is a regular file of at most 16 KiB written by the front
/// `pid` (SPEC-494 DFJ-005): never following a link, never waiting on a FIFO.
#[must_use]
pub fn read_state(path: &std::path::Path, pid: u32) -> Option<DockerFrontState> {
    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;
    use std::io::Read as _;
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = std::fs::File::from(open(path, flags, Mode::empty()).ok()?);
    if !file.metadata().is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(STATE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > STATE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice::<DockerFrontState>(&bytes)
        .ok()
        .filter(|state| state.pid == pid)
}

impl StateWatch {
    /// Reports `state` as `sandbox`'s Docker trouble (SPEC-494 DFJ-006), after
    /// [`StateWatch::observe`] took it: a front that refused since the last report, or holds its
    /// whole limit, begins or keeps the trouble; one that holds at most seven eighths of its
    /// limit and refused nothing since ends it.
    pub fn report(&mut self, state: &DockerFrontState, sandbox: &str, health: &ConnectionHealth) {
        let new = state.refused.saturating_sub(self.reported_refused);
        self.reported_refused = state.refused;
        if new > 0 || state.held >= state.max {
            let refusal = Refusal {
                limit: Limit::Docker,
                sandbox: Some(sandbox.to_owned()),
                held: state.held,
                max: state.max,
                holders: Vec::new(),
            };
            health.refused_times(refusal, new, crate::relay::wall_ms());
        } else if health.open(Limit::Docker, Some(sandbox)) {
            health.held(Limit::Docker, Some(sandbox), state.held);
        }
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
                            b"keep" => {
                                // A keep-alive engine: answers, waits for the client's end, and
                                // closes a few ms after it, as an HTTP server does.
                                let _ = stream.write_all(b"engine:keep").await;
                                let mut rest = [0u8; 64];
                                while matches!(stream.read(&mut rest).await, Ok(n) if n > 0) {}
                                tokio::time::sleep(Duration::from_millis(5)).await;
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
        let body = refused.split("\r\n\r\n").nth(1).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(body)?;
        assert_eq!(json["message"].as_str(), Some(AT_LIMIT), "{refused}");
        let state = gate.state(false);
        assert_eq!((state.refused, state.gave_way), (1, 0), "{state:?}");
        assert!(state.at_limit_since_ms.is_some(), "{state:?}");
        // Below the limit again, it is no longer at its limit.
        drop(_a);
        until("room again", || gate.state(false).held == 1).await?;
        assert_eq!(gate.state(false).at_limit_since_ms, None);
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

    /// Review round 1: an ordinary call whose client closes right after the answer, while a
    /// keep-alive engine closes its side a few ms later, is not a hang-up.
    #[tokio::test]
    async fn an_ordinary_call_closed_after_its_answer_is_not_a_hang_up() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        // The engine's end comes 5 ms after the client's; the check's own wait is wide enough
        // for a loaded machine.
        let gate = Gate::with(64, IDLE_FLOOR, Duration::from_secs(20), CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        for _ in 0..20 {
            let mut client = send(&socket, "keep").await?;
            let mut head = [0u8; 11];
            client.read_exact(&mut head).await?;
            assert_eq!(&head, b"engine:keep");
            drop(client);
        }
        until("every place free", || gate.state(false).held == 0).await?;
        assert_eq!(gate.state(false).hung_up, 0, "{:?}", gate.state(false));
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// Review round 1: a client that shut its writing and later closes entirely is a hang-up
    /// then, not before.
    #[tokio::test]
    async fn a_half_close_followed_by_a_close_ends_at_the_close() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(4, IDLE_FLOOR, Duration::from_millis(20), CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let mut client = send(&socket, "hold").await?;
        client.shutdown().await?;
        until("held", || gate.state(false).held == 1).await?;
        // Many checks pass while the client still reads.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(gate.state(false).held, 1, "a half-close is not a hang-up");
        drop(client);
        until("the place is free", || gate.state(false).held == 0).await?;
        assert_eq!(gate.state(false).hung_up, 1);
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// Review round 1: refusals are answered at most [`REFUSING_AT_ONCE`] at once; beyond, a
    /// connection is closed unanswered, and an answered refusal returns its permit.
    #[tokio::test]
    async fn refusals_are_bounded_and_their_permits_return() -> TestResult {
        let dir = tempfile::tempdir()?;
        let engine = Engine::start(dir.path())?;
        let gate = Gate::with(1, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE);
        let front = front(dir.path(), &engine, Arc::clone(&gate))?;
        let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
        let _held = send(&socket, "hold").await?;
        until("held", || gate.state(false).held == 1).await?;
        // Refused callers that never send their request's head keep their permits (up to 5 s).
        let mut silent = Vec::new();
        for _ in 0..REFUSING_AT_ONCE {
            silent.push(UnixStream::connect(&socket).await?);
        }
        let all = u64::try_from(REFUSING_AT_ONCE)?;
        until("all refused", || gate.state(false).refused == all).await?;
        let mut beyond = UnixStream::connect(&socket).await?;
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(4), beyond.read_to_end(&mut out)).await??;
        assert!(
            out.is_empty(),
            "closed unanswered: {}",
            String::from_utf8_lossy(&out)
        );
        drop(silent);
        // Their refusals end (the head's read ends at their close) and give the permits back.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let answered = answer(send(&socket, "GET / HTTP/1.1\r\n\r\n").await?).await?;
            if answered.starts_with("HTTP/1.1 503") {
                break;
            }
            assert!(Instant::now() < deadline, "no permit came back");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        front.stop(Duration::from_secs(1)).await;
        engine.task.abort();
        Ok(())
    }

    /// Review round 1: a front in refuse mode is bounded the same way: at its limit a new caller
    /// gets the limit's answer, not the mode's.
    #[tokio::test]
    async fn a_refusing_front_at_its_limit_says_so() -> TestResult {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let gate = Gate::with(1, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE);
        let front = Front::start_through(
            &paths,
            &DockerServe::Refuse {
                reason: "no engine".into(),
            },
            &[],
            Box::new(|| Ok(())),
            Arc::clone(&gate),
        )?;
        // Holds its place while its refusal waits for a head it never sends.
        let _silent = UnixStream::connect(paths.docker_socket()).await?;
        until("held", || gate.state(false).held == 1).await?;
        let refused = answer(send(&paths.docker_socket(), "GET / HTTP/1.1\r\n\r\n").await?).await?;
        assert!(
            refused.contains("holds all of its connections"),
            "{refused}"
        );
        assert!(!refused.contains("no engine"), "{refused}");
        front.stop(Duration::from_secs(1)).await;
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
        // Past the minute, the give-ways not yet said are said, though none came since.
        let grew = watch.observe(&state_of(7, 5, 1), start + Duration::from_secs(90));
        assert_eq!(grew.len(), 1, "{grew:?}");
        assert_eq!((grew[0].new, grew[0].total), (2, 5));
        // Nothing new: nothing said.
        assert!(
            watch
                .observe(&state_of(7, 5, 1), start + Duration::from_secs(200))
                .is_empty()
        );
        // A new front's events count from zero.
        let grew = watch.observe(&state_of(8, 2, 0), start + Duration::from_secs(201));
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

    fn state_at(pid: u32, held: usize, refused: u64) -> DockerFrontState {
        DockerFrontState {
            held,
            max: 256,
            refused,
            ..state_of(pid, 0, 0)
        }
    }

    /// SPEC-494 AC-5 (DFJ-006): a front at its limit opens one `docker` trouble of its sandbox,
    /// told once; refusals within it update its count without telling; it ends once the front
    /// holds at most seven eighths and refused nothing since.
    #[test]
    fn a_full_front_is_one_docker_trouble_of_its_sandbox_until_it_has_room() {
        let health = ConnectionHealth::default();
        let told = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&told);
        health.on_change(move |trouble, began| {
            sink.lock().unwrap_or_else(PoisonError::into_inner).push((
                trouble.limit,
                trouble.sandbox.clone(),
                began,
            ));
        });
        let mut watch = StateWatch::default();
        watch.started(7);
        let mut read = |state: DockerFrontState| {
            watch.observe(&state, Instant::now());
            watch.report(&state, "s1", &health);
        };
        read(state_at(7, 200, 0));
        assert!(health.troubles().is_empty(), "room left: no trouble");
        read(state_at(7, 256, 0));
        read(state_at(7, 256, 3));
        read(state_at(7, 240, 5));
        let open = health.troubles();
        assert_eq!(open.len(), 1, "{open:?}");
        assert_eq!(
            (
                open[0].limit,
                open[0].sandbox.as_deref(),
                open[0].refused,
                open[0].held
            ),
            (Limit::Docker, Some("s1"), 5, 240)
        );
        // Above seven eighths with no new refusal: still open.
        read(state_at(7, 240, 5));
        assert_eq!(health.troubles().len(), 1);
        read(state_at(7, 224, 5));
        assert!(health.troubles().is_empty(), "room again: ended");
        assert_eq!(
            *told.lock().unwrap_or_else(PoisonError::into_inner),
            vec![
                (Limit::Docker, Some("s1".to_owned()), true),
                (Limit::Docker, Some("s1".to_owned()), false)
            ]
        );
        // A refusal alone (the front below its limit) begins a new one.
        read(state_at(7, 10, 6));
        assert_eq!(health.troubles().len(), 1);
    }

    /// Slice 2 review round 1: after a server restart, an adopted front's past refusals are no
    /// new trouble; a refusal after that is.
    #[test]
    fn an_adopted_fronts_past_refusals_are_no_new_trouble() {
        let health = ConnectionHealth::default();
        let mut watch = StateWatch::default();
        let mut read = |state: DockerFrontState| {
            watch.observe(&state, Instant::now());
            watch.report(&state, "s1", &health);
        };
        read(state_at(9, 3, 4));
        assert!(health.troubles().is_empty(), "{:?}", health.troubles());
        read(state_at(9, 3, 5));
        assert_eq!(health.troubles().len(), 1);
    }

    /// Review round 1 (DFJ-005): the server reads only a regular, bounded state file of the
    /// front it knows: never a link, a FIFO (it would wait), a large file or another front's.
    #[test]
    fn the_server_reads_only_its_fronts_bounded_regular_state_file() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("docker-front-state.json");
        crate::holder::write_json(&path, &state_of(7, 1, 0))?;
        assert_eq!(read_state(&path, 7), Some(state_of(7, 1, 0)));
        assert_eq!(read_state(&path, 8), None, "another front's");
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link)?;
        assert_eq!(read_state(&link, 7), None, "a link");
        let fifo = dir.path().join("fifo.json");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600))?;
        assert_eq!(read_state(&fifo, 7), None, "a FIFO");
        let large = dir.path().join("large.json");
        let mut padded = serde_json::to_vec(&state_of(7, 1, 0))?;
        padded.resize(20 * 1024, b' ');
        assert!(serde_json::from_slice::<DockerFrontState>(&padded).is_ok());
        std::fs::write(&large, padded)?;
        assert_eq!(read_state(&large, 7), None, "too large");
        assert_eq!(read_state(&dir.path().join("none.json"), 7), None);
        Ok(())
    }

    /// Review round 1 (DFJ-005): an unchanged state is not written again.
    #[tokio::test]
    async fn an_unchanged_state_is_not_written_again() -> TestResult {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("docker-front-state.json");
        let gate = Gate::with(4, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE)
            .writing_every(Duration::from_millis(20));
        let writer = tokio::spawn(Arc::clone(&gate).keep_state(path.clone(), || async { false }));
        until("written", || path.exists()).await?;
        let stamp = |path: &std::path::Path| -> io::Result<(u64, i64, i64)> {
            let meta = std::fs::metadata(path)?;
            Ok((meta.ino(), meta.mtime(), meta.mtime_nsec()))
        };
        let first = stamp(&path)?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // A replaced file may take back an earlier inode (ext4): its time tells it too.
        assert_eq!(stamp(&path)?, first, "rewritten unchanged");
        writer.abort();
        Ok(())
    }

    /// Review round 2: a state file that cannot be written is said, once a minute.
    #[test]
    fn a_state_that_cannot_be_written_is_said_once_a_minute() {
        let gate = Gate::with(4, IDLE_FLOOR, HANGUP_CHECK, CLOSE_GRACE);
        let mut out = Vec::new();
        let path = std::path::Path::new("/nowhere/docker-front-state.json");
        gate.unwritten(path, &"No such file or directory", &mut out);
        gate.unwritten(path, &"No such file or directory", &mut out);
        let text = String::from_utf8_lossy(&out);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains("cannot write its state /nowhere/docker-front-state.json"),
            "{text}"
        );
    }

    /// Review round 2: the front's log lines say when, in UTC.
    #[test]
    fn log_lines_carry_their_time_in_utc() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        // The incident's repair, 2026-11-15 22:08:00 UTC.
        assert_eq!(utc(1_763_244_480_000), "2025-11-15T22:08:00Z");
        assert_eq!(utc(1_794_780_480_999), "2026-11-15T22:08:00Z");
        assert_eq!(utc(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    /// Review round 2 (RUST-010): a state written by an older front, without the fields added
    /// later, is still read; fields are only ever added, with defaults.
    #[test]
    fn a_state_without_later_fields_is_still_read() -> TestResult {
        let old = r#"{"pid":7,"held":3,"max":256,"gave_way":0,"refused":0,"hung_up":1,"closed_after_grace":0}"#;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("docker-front-state.json");
        std::fs::write(&path, old)?;
        let state = read_state(&path, 7).ok_or("an older front's state was not read")?;
        assert_eq!(
            (state.held, state.at_limit_since_ms, state.engine_running),
            (3, None, false)
        );
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
