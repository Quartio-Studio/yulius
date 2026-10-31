//! The relays across a sandbox's network boundary (SPEC-002 ADR-22 §4; frames in
//! [`yulius_contracts::sandbox`]).
//!
//! Inside the sandbox the holder serves `in.sock` (connections into the sandbox's ports) and
//! listens on each forwarded port, relaying to the server's `out.sock`. On the host the server
//! serves `out.sock` (to the forwarded targets) and listens on the host's loopback for each port
//! of the sandbox's range that something listens on inside, relaying through `in.sock`.
//!
//! Every connection is bounded: [`MAX_CONNECTIONS`] at once per direction and sandbox, the
//! header within [`HEADER_TIMEOUT`], the target within [`CONNECT_TIMEOUT`]. Bytes are copied
//! unchanged; nothing is parsed after the header.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use yulius_contracts::sandbox::{
    INBOUND_CONNECT, INBOUND_PORTS, ListeningPorts, RELAY_FORBIDDEN, RELAY_OK, RELAY_REFUSED,
};

/// Connections relayed at once per direction and sandbox.
pub const MAX_CONNECTIONS: usize = 256;
/// Time for a peer to send a relay header.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
/// Time to reach a relay target.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest [`ListeningPorts`] line accepted.
const MAX_PORTS_LINE: u64 = 64 * 1024;

/// Least time between two warnings that a relay dropped a connection at its limit.
const WARN_EVERY: Duration = Duration::from_secs(60);

/// Counts a connection a relay dropped at its limit (`limit`) and says so on the process's stderr
/// (the server's journal; inside a sandbox the holder's `holder.log` or the Docker front's
/// `docker-front.log`, where no tracing subscriber runs), at most every [`WARN_EVERY`] (`last`:
/// when it last did): the sandbox's processes hold every connection, so even their calls to Yulius
/// fail (SPEC-353 LAP-002).
fn warn_dropped(
    last: &mut Option<std::time::Instant>,
    relay: &str,
    port: Option<u16>,
    sandbox: Option<&str>,
    limit: usize,
) {
    let dropped = DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let dropped = Dropped {
        relay,
        port,
        sandbox,
        limit,
        dropped,
    };
    notice_dropped(&mut io::stderr().lock(), last, &dropped);
}

/// One dropped connection, as its notice names it.
struct Dropped<'a> {
    relay: &'a str,
    port: Option<u16>,
    sandbox: Option<&'a str>,
    limit: usize,
    dropped: u64,
}

/// Writes `notice` to `out` unless one was written within [`WARN_EVERY`] (`last`).
/// A failed write (a full disk under `holder.log`) is ignored: it never stops the relay.
fn notice_dropped(
    out: &mut dyn io::Write,
    last: &mut Option<std::time::Instant>,
    notice: &Dropped<'_>,
) {
    if last.is_some_and(|at| at.elapsed() < WARN_EVERY) {
        return;
    }
    *last = Some(std::time::Instant::now());
    let port = notice
        .port
        .map(|p| format!(" port={p}"))
        .unwrap_or_default();
    let sandbox = notice
        .sandbox
        .map(|s| format!(" sandbox={s}"))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "WARN sandbox relay: a connection was dropped at the limit; leftover processes may hold \
         them (docs/operations.md, \"Leftover agent processes\") relay={}{port}{sandbox} \
         limit={} dropped={}",
        notice.relay, notice.limit, notice.dropped
    );
    let _ = out.flush();
}

fn timeout_error(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, format!("{what} timed out"))
}

async fn with_timeout<T>(
    limit: Duration,
    what: &str,
    future: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::time::timeout(limit, future)
        .await
        .map_err(|_| timeout_error(what))?
}

/// Connects to a loopback port inside this network namespace: IPv4 first, then IPv6 (dev
/// servers that bind `localhost` may listen on `::1` only).
pub async fn connect_loopback(port: u16) -> io::Result<TcpStream> {
    let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);
    match with_timeout(CONNECT_TIMEOUT, "connect", TcpStream::connect(v4)).await {
        Ok(stream) => Ok(stream),
        Err(first) => with_timeout(CONNECT_TIMEOUT, "connect", TcpStream::connect(v6))
            .await
            .map_err(|_| first),
    }
}

pub(crate) async fn splice<A, B>(mut a: A, mut b: B)
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
}

// ---------------------------------------------------------------------------------------------
// Inside the sandbox (the holder).

/// Serves `in.sock`: [`INBOUND_PORTS`] with the listening ports of `first..=last` (from
/// `ports`), [`INBOUND_CONNECT`] to a port of that range. Runs until the listener fails.
pub async fn serve_inbound(
    listener: UnixListener,
    first: u16,
    last: u16,
    ports: Arc<dyn Fn() -> Vec<u16> + Send + Sync>,
) {
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut warned = None;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
            drop(stream);
            warn_dropped(&mut warned, "inbound", None, None, MAX_CONNECTIONS);
            continue;
        };
        let ports = Arc::clone(&ports);
        tokio::spawn(async move {
            let _ = inbound_connection(stream, first, last, ports.as_ref()).await;
            drop(permit);
        });
    }
}

async fn inbound_connection(
    mut stream: UnixStream,
    first: u16,
    last: u16,
    ports: &(dyn Fn() -> Vec<u16> + Send + Sync),
) -> io::Result<()> {
    let op = with_timeout(HEADER_TIMEOUT, "relay header", stream.read_u8()).await?;
    match op {
        INBOUND_PORTS => {
            let mut line = serde_json::to_vec(&ListeningPorts { ports: ports() })?;
            line.push(b'\n');
            stream.write_all(&line).await?;
            stream.shutdown().await
        }
        INBOUND_CONNECT => {
            let port = with_timeout(HEADER_TIMEOUT, "relay header", stream.read_u16()).await?;
            if !(first..=last).contains(&port) {
                stream.write_u8(RELAY_FORBIDDEN).await?;
                return Ok(());
            }
            match connect_loopback(port).await {
                Ok(target) => {
                    stream.write_u8(RELAY_OK).await?;
                    splice(stream, target).await;
                    Ok(())
                }
                Err(_) => stream.write_u8(RELAY_REFUSED).await,
            }
        }
        _ => Ok(()),
    }
}

/// The IPv4 address a forward listens on inside the sandbox: its loopback, or every address when
/// the sandbox has a Docker engine, whose containers reach the forwards through their bridge's
/// gateway (`host.docker.internal`, ADR-0151). `slirp4netns` forwards no port inward, so nothing
/// outside the sandbox reaches them either way.
#[must_use]
pub fn forward_address(containers: bool) -> Ipv4Addr {
    if containers {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    }
}

/// Listens on `port` at [`forward_address`] (and `[::1]:port` when available) inside the sandbox
/// and relays every connection to `out_sock` (the server), announcing `port`.
///
/// # Errors
/// The IPv4 listener cannot be bound.
pub async fn serve_forward(
    port: u16,
    out_sock: PathBuf,
    containers: bool,
) -> io::Result<JoinHandle<()>> {
    serve_forward_at(forward_address(containers), port, out_sock, true).await
}

/// Listens on `address:port` (and `[::1]:port` with `loopback_v6`) and relays every connection
/// to `out_sock`, announcing `port`. The Docker front serves a holder's forwards on the engine's
/// bridge gateway this way when the holder's own listen on loopback only (ADR-0158).
///
/// # Errors
/// The IPv4 listener cannot be bound.
pub async fn serve_forward_at(
    address: Ipv4Addr,
    port: u16,
    out_sock: PathBuf,
    loopback_v6: bool,
) -> io::Result<JoinHandle<()>> {
    forward_within(address, port, out_sock, loopback_v6, MAX_CONNECTIONS).await
}

/// Connections the relays of this process dropped at their limit (SPEC-353 LAP-002).
static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many connections the relays of this process dropped at their limit so far (tests).
#[cfg(test)]
fn dropped_connections() -> u64 {
    DROPPED.load(std::sync::atomic::Ordering::Relaxed)
}

/// [`serve_forward_at`] with `max` connections at once (tests use a small one).
async fn forward_within(
    address: Ipv4Addr,
    port: u16,
    out_sock: PathBuf,
    loopback_v6: bool,
    max: usize,
) -> io::Result<JoinHandle<()>> {
    let v4 = TcpListener::bind((address, port)).await?;
    let v6 = if loopback_v6 {
        TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await.ok()
    } else {
        None
    };
    let limit = Arc::new(Semaphore::new(max));
    Ok(tokio::spawn(async move {
        let mut warned = None;
        loop {
            let accepted = match &v6 {
                Some(v6) => tokio::select! {
                    a = v4.accept() => a,
                    a = v6.accept() => a,
                },
                None => v4.accept().await,
            };
            let Ok((stream, _)) = accepted else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
                warn_dropped(&mut warned, "forward", Some(port), None, max);
                continue;
            };
            let out_sock = out_sock.clone();
            tokio::spawn(async move {
                let _ = forward_connection(stream, port, &out_sock).await;
                drop(permit);
            });
        }
    }))
}

async fn forward_connection(stream: TcpStream, port: u16, out_sock: &Path) -> io::Result<()> {
    let mut server = with_timeout(CONNECT_TIMEOUT, "relay", UnixStream::connect(out_sock)).await?;
    server.write_u16(port).await?;
    let status = with_timeout(HEADER_TIMEOUT, "relay status", server.read_u8()).await?;
    if status == RELAY_OK {
        splice(stream, server).await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// On the host (the server).

/// Serves `out.sock`: a connection announces a port and is relayed to its target.
pub async fn serve_outbound(
    listener: UnixListener,
    targets: Arc<BTreeMap<u16, SocketAddr>>,
    sandbox: String,
) {
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut warned = None;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
            drop(stream);
            warn_dropped(
                &mut warned,
                "outbound",
                None,
                Some(&sandbox),
                MAX_CONNECTIONS,
            );
            continue;
        };
        let targets = Arc::clone(&targets);
        tokio::spawn(async move {
            let _ = outbound_connection(stream, &targets).await;
            drop(permit);
        });
    }
}

async fn outbound_connection(
    mut stream: UnixStream,
    targets: &BTreeMap<u16, SocketAddr>,
) -> io::Result<()> {
    let port = with_timeout(HEADER_TIMEOUT, "relay header", stream.read_u16()).await?;
    let Some(target) = targets.get(&port) else {
        return stream.write_u8(RELAY_FORBIDDEN).await;
    };
    match with_timeout(CONNECT_TIMEOUT, "connect", TcpStream::connect(*target)).await {
        Ok(upstream) => {
            stream.write_u8(RELAY_OK).await?;
            splice(stream, upstream).await;
            Ok(())
        }
        Err(_) => stream.write_u8(RELAY_REFUSED).await,
    }
}

/// Asks the holder at `in_sock` which ports of its range listen inside.
///
/// # Errors
/// The holder is unreachable or answers off-contract.
pub async fn query_ports(in_sock: &Path) -> io::Result<Vec<u16>> {
    let mut stream = with_timeout(CONNECT_TIMEOUT, "relay", UnixStream::connect(in_sock)).await?;
    stream.write_u8(INBOUND_PORTS).await?;
    let mut line = Vec::new();
    with_timeout(
        HEADER_TIMEOUT,
        "ports",
        (&mut stream).take(MAX_PORTS_LINE).read_to_end(&mut line),
    )
    .await?;
    let ports: ListeningPorts = serde_json::from_slice(&line)?;
    Ok(ports.ports)
}

async fn inbound_relay(mut client: TcpStream, port: u16, in_sock: &Path) -> io::Result<()> {
    let mut holder = with_timeout(CONNECT_TIMEOUT, "relay", UnixStream::connect(in_sock)).await?;
    holder.write_u8(INBOUND_CONNECT).await?;
    holder.write_u16(port).await?;
    let status = with_timeout(HEADER_TIMEOUT, "relay status", holder.read_u8()).await?;
    if status == RELAY_OK {
        splice(&mut client, holder).await;
    } else {
        let _ = client.shutdown().await;
    }
    Ok(())
}

/// The host side of one sandbox's dev-server ports: a loopback listener per port that listens
/// inside, kept in step by [`InboundPorts::sync`].
pub struct InboundPorts {
    in_sock: PathBuf,
    listeners: HashMap<u16, JoinHandle<()>>,
    limit: Arc<Semaphore>,
}

impl InboundPorts {
    /// Relays through the holder at `in_sock`.
    #[must_use]
    pub fn new(in_sock: PathBuf) -> Self {
        Self {
            in_sock,
            listeners: HashMap::new(),
            limit: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        }
    }

    /// Ports currently relayed.
    #[must_use]
    pub fn relayed(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self.listeners.keys().copied().collect();
        ports.sort_unstable();
        ports
    }

    /// Opens host listeners for `wanted` ports not yet relayed and closes the others. A port the
    /// host cannot bind (another process holds it, for example a run from before isolation) is
    /// skipped and retried at the next sync.
    pub async fn sync(&mut self, wanted: &[u16]) {
        self.listeners.retain(|port, task| {
            let keep = wanted.contains(port) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for &port in wanted {
            if self.listeners.contains_key(&port) {
                continue;
            }
            let Ok(v4) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await else {
                tracing::debug!(port, "host port busy; not relayed yet");
                continue;
            };
            let v6 = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await.ok();
            let in_sock = self.in_sock.clone();
            let limit = Arc::clone(&self.limit);
            let task = tokio::spawn(accept_inbound(v4, v6, port, in_sock, limit));
            self.listeners.insert(port, task);
        }
    }

    /// Closes every listener.
    pub fn close(&mut self) {
        for (_, task) in self.listeners.drain() {
            task.abort();
        }
    }
}

impl Drop for InboundPorts {
    fn drop(&mut self) {
        self.close();
    }
}

async fn accept_inbound(
    v4: TcpListener,
    v6: Option<TcpListener>,
    port: u16,
    in_sock: PathBuf,
    limit: Arc<Semaphore>,
) {
    let mut warned = None;
    loop {
        let accepted = match &v6 {
            Some(v6) => tokio::select! {
                a = v4.accept() => a,
                a = v6.accept() => a,
            },
            None => v4.accept().await,
        };
        let Ok((client, _)) = accepted else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
            warn_dropped(&mut warned, "preview", Some(port), None, MAX_CONNECTIONS);
            continue;
        };
        let in_sock = in_sock.clone();
        tokio::spawn(async move {
            let _ = inbound_relay(client, port, &in_sock).await;
            drop(permit);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn echo_server() -> io::Result<(u16, JoinHandle<()>)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = stream.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        Ok((port, task))
    }

    async fn round_trip(stream: &mut TcpStream, text: &[u8]) -> io::Result<Vec<u8>> {
        stream.write_all(text).await?;
        let mut back = vec![0; text.len()];
        with_timeout(Duration::from_secs(5), "echo", stream.read_exact(&mut back)).await?;
        Ok(back)
    }

    /// The inbound path end to end on one host: a host listener → `in.sock` → a loopback port.
    #[tokio::test]
    async fn inbound_ports_reach_listeners_in_range_and_list_them() -> TestResult {
        let dir = tempfile::tempdir()?;
        let in_sock = dir.path().join("in.sock");
        let (target, _echo) = echo_server().await?;
        let listener = UnixListener::bind(&in_sock)?;
        let reported = Arc::new(move || vec![target]);
        let server = tokio::spawn(serve_inbound(listener, target, target, reported));

        assert_eq!(query_ports(&in_sock).await?, vec![target]);

        // A direct CONNECT for the port: OK, then the bytes come back.
        let mut raw = UnixStream::connect(&in_sock).await?;
        raw.write_u8(INBOUND_CONNECT).await?;
        raw.write_u16(target).await?;
        assert_eq!(raw.read_u8().await?, RELAY_OK);
        raw.write_all(b"ping").await?;
        let mut back = [0u8; 4];
        raw.read_exact(&mut back).await?;
        assert_eq!(&back, b"ping");

        // Out of range: forbidden.
        let mut raw = UnixStream::connect(&in_sock).await?;
        raw.write_u8(INBOUND_CONNECT).await?;
        raw.write_u16(target.wrapping_add(1)).await?;
        assert_eq!(raw.read_u8().await?, RELAY_FORBIDDEN);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn closed_ports_are_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let in_sock = dir.path().join("in.sock");
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = free.local_addr()?.port();
        drop(free);
        let listener = UnixListener::bind(&in_sock)?;
        let server = tokio::spawn(serve_inbound(listener, port, port, Arc::new(Vec::new)));
        let mut raw = UnixStream::connect(&in_sock).await?;
        raw.write_u8(INBOUND_CONNECT).await?;
        raw.write_u16(port).await?;
        assert_eq!(raw.read_u8().await?, RELAY_REFUSED);
        server.abort();
        Ok(())
    }

    /// The outbound path: a forwarded port inside → `out.sock` → the host target; ports without
    /// a target are forbidden.
    #[tokio::test]
    async fn forwards_reach_only_their_targets() -> TestResult {
        let dir = tempfile::tempdir()?;
        let out_sock = dir.path().join("out.sock");
        let (target, _echo) = echo_server().await?;
        let forwarded = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let inside_port = forwarded.local_addr()?.port();
        drop(forwarded);
        let targets = Arc::new(BTreeMap::from([(
            inside_port,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), target),
        )]));
        let server = tokio::spawn(serve_outbound(
            UnixListener::bind(&out_sock)?,
            targets,
            "s1".to_owned(),
        ));
        let forward = serve_forward(inside_port, out_sock.clone(), false).await?;

        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, inside_port)).await?;
        assert_eq!(round_trip(&mut client, b"hello").await?, b"hello");
        // Only the loopback address without an engine.
        assert!(
            TcpStream::connect((Ipv4Addr::new(127, 0, 0, 2), inside_port))
                .await
                .is_err()
        );

        let mut raw = UnixStream::connect(&out_sock).await?;
        raw.write_u16(inside_port.wrapping_add(1)).await?;
        assert_eq!(raw.read_u8().await?, RELAY_FORBIDDEN);
        forward.abort();
        server.abort();
        Ok(())
    }

    /// SPEC-353 LAP-002 (review round 3): a forward relay at its limit drops the next connection
    /// and says so (counted, and written to the process's stderr: the holder's `holder.log`).
    #[tokio::test]
    async fn a_forward_at_its_limit_counts_the_connection_it_drops() -> TestResult {
        let dir = tempfile::tempdir()?;
        let out_sock = dir.path().join("out.sock");
        // `out.sock` accepts every relayed connection and keeps it open.
        let outbound = UnixListener::bind(&out_sock)?;
        let (held_tx, mut held) = tokio::sync::mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let mut open = Vec::new();
            while let Ok((mut stream, _)) = outbound.accept().await {
                if stream.read_u16().await.is_ok() && stream.write_u8(RELAY_OK).await.is_ok() {
                    open.push(stream);
                    let _ = held_tx.send(());
                }
            }
        });
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = free.local_addr()?.port();
        drop(free);
        let forward = forward_within(Ipv4Addr::LOCALHOST, port, out_sock, false, 2).await?;
        let mut clients = Vec::new();
        for _ in 0..2 {
            clients.push(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?);
            tokio::time::timeout(Duration::from_secs(10), held.recv())
                .await?
                .ok_or("out.sock ended")?;
        }
        let before = dropped_connections();
        let mut third = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        // Dropped: the client reads the end of the stream (or a reset).
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(10), third.read(&mut byte)).await?;
        assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
        assert!(dropped_connections() > before);
        forward.abort();
        server.abort();
        Ok(())
    }

    /// A writer whose every write fails (a full disk under `holder.log`).
    struct Full;
    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("no space left on device"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("no space left on device"))
        }
    }

    /// SPEC-353 LAP-002 (review round 4): the drop notice names the relay, its port and sandbox,
    /// the limit and the count, at most once a minute, and a log that cannot be written never
    /// stops the relay (no panic).
    #[test]
    fn the_drop_notice_names_the_relay_and_never_fails_it() {
        let notice = Dropped {
            relay: "forward",
            port: Some(8081),
            sandbox: Some("s1"),
            limit: 256,
            dropped: 7,
        };
        let mut last = None;
        let mut out = Vec::new();
        notice_dropped(&mut out, &mut last, &notice);
        let line = String::from_utf8_lossy(&out).into_owned();
        for part in [
            "sandbox relay: a connection was dropped at the limit",
            "relay=forward",
            "port=8081",
            "sandbox=s1",
            "limit=256",
            "dropped=7",
        ] {
            assert!(line.contains(part), "{part}: {line}");
        }
        let mut again = Vec::new();
        notice_dropped(&mut again, &mut last, &notice);
        assert!(again.is_empty(), "once a minute");
        let mut never = None;
        notice_dropped(&mut Full, &mut never, &notice);
        assert!(
            never.is_some(),
            "a failed write still counts as the minute's notice"
        );
    }

    /// What the stand-in for Yulius's internal listener answers to every request.
    const ANSWER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

    /// A stand-in for Yulius's internal listener (`127.0.0.1:8081`): answers every request with
    /// [`ANSWER`] and keeps the connection open (HTTP keep-alive).
    async fn yulius_stub() -> io::Result<u16> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 || stream.write_all(ANSWER).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Ok(port)
    }

    /// The server's side of a sandbox's relay, `out.sock` to a [`yulius_stub`], in a runtime of
    /// its own: dropping it closes every socket it holds, as the server process's exit does.
    struct ServerProcess {
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl ServerProcess {
        fn start(out_sock: PathBuf, inside_port: u16) -> io::Result<Self> {
            let (ready_tx, ready) = std::sync::mpsc::channel();
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let thread = std::thread::spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let started = async {
                        let target = yulius_stub().await?;
                        let _ = std::fs::remove_file(&out_sock);
                        let listener = UnixListener::bind(&out_sock)?;
                        let targets = Arc::new(BTreeMap::from([(
                            inside_port,
                            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), target),
                        )]));
                        tokio::spawn(serve_outbound(listener, targets, "s1".to_owned()));
                        io::Result::Ok(())
                    };
                    let _ = ready_tx.send(started.await.map_err(|e| e.to_string()));
                    let _ = stopped.await;
                });
                // The runtime drops here with every task and socket of this "process".
            });
            ready
                .recv()
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
            Ok(Self {
                stop: Some(stop),
                thread: Some(thread),
            })
        }

        /// The process exits: every connection it held closes.
        fn exit(mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// One request through the forward on `port`, as the Superagent's `--scrub-exec` asks for
    /// its fingerprints: `Ok` once Yulius answered, else what the client saw.
    async fn agent_call(port: u16) -> Result<(), String> {
        let call = async {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
            request(&mut stream).await
        };
        call.await.map_err(|e| format!("{:?}: {e}", e.kind()))
    }

    /// Sends one request on `stream` and reads Yulius's answer.
    async fn request(stream: &mut TcpStream) -> io::Result<()> {
        stream
            .write_all(
                b"POST /internal/scrub/fingerprints HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                  Content-Length: 0\r\n\r\n",
            )
            .await?;
        let mut back = vec![0; ANSWER.len()];
        with_timeout(
            Duration::from_secs(10),
            "answer",
            stream.read_exact(&mut back),
        )
        .await?;
        if back == ANSWER {
            Ok(())
        } else {
            Err(io::Error::other("another answer"))
        }
    }

    /// SPEC-355 (2026-10-30, quartio-v2): leftover processes hold every connection of the
    /// sandbox's forward to Yulius, so the Superagent's own calls are reset (`ECONNRESET at
    /// http://127.0.0.1:8081`). A server restart does not free them: the holder keeps each
    /// connection whose server end closed until its client closes too, and a leftover process
    /// never does. The forward gives way to the new call instead (the longest-idle connection).
    #[tokio::test]
    async fn the_agent_gets_through_a_forward_full_of_leftovers_before_and_after_a_restart()
    -> TestResult {
        let dir = tempfile::tempdir()?;
        let out_sock = dir.path().join("out.sock");
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = free.local_addr()?.port();
        drop(free);
        let server = ServerProcess::start(out_sock.clone(), port)?;
        let forward = forward_within(Ipv4Addr::LOCALHOST, port, out_sock.clone(), false, 4).await?;
        // Leftover processes: each made a call and keeps its connection, idle, for good.
        let mut leftovers = Vec::new();
        for _ in 0..4 {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
            request(&mut stream).await?;
            leftovers.push(stream);
        }
        let before = agent_call(port).await;

        server.exit();
        // The server's ends are closed (the relay passed that on), and the leftovers still hold
        // theirs.
        let mut byte = [0u8; 1];
        for stream in &mut leftovers {
            let read = with_timeout(Duration::from_secs(10), "end", stream.read(&mut byte)).await;
            assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
        }
        let restarted = ServerProcess::start(out_sock.clone(), port)?;
        let after = agent_call(port).await;
        assert!(
            before.is_ok() && after.is_ok(),
            "the agent's call before the restart: {before:?}, after it: {after:?}"
        );
        drop(leftovers);
        forward.abort();
        restarted.exit();
        Ok(())
    }

    /// SPEC-187 DKR-007: with a Docker engine the forwards listen on every address of the
    /// sandbox's network, so containers reach them through their bridge's gateway (here another
    /// address than `127.0.0.1`).
    #[tokio::test]
    async fn with_an_engine_forwards_listen_on_every_address() -> TestResult {
        assert_eq!(forward_address(false), Ipv4Addr::LOCALHOST);
        assert_eq!(forward_address(true), Ipv4Addr::UNSPECIFIED);
        let dir = tempfile::tempdir()?;
        let out_sock = dir.path().join("out.sock");
        let (target, _echo) = echo_server().await?;
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let inside_port = free.local_addr()?.port();
        drop(free);
        let targets = Arc::new(BTreeMap::from([(
            inside_port,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), target),
        )]));
        let server = tokio::spawn(serve_outbound(
            UnixListener::bind(&out_sock)?,
            targets,
            "s1".to_owned(),
        ));
        let forward = serve_forward(inside_port, out_sock.clone(), true).await?;
        let mut client = TcpStream::connect((Ipv4Addr::new(127, 0, 0, 2), inside_port)).await?;
        assert_eq!(
            round_trip(&mut client, b"from a bridge").await?,
            b"from a bridge"
        );
        forward.abort();
        server.abort();
        Ok(())
    }

    /// The host listeners follow the holder's list: opened for listening ports, closed after.
    #[tokio::test]
    async fn host_listeners_follow_the_listening_ports() -> TestResult {
        let dir = tempfile::tempdir()?;
        let in_sock = dir.path().join("in.sock");
        // The "inside" echo server is on a free port P; the holder relays P; the host side must
        // open its own listener on P, which collides on one host. So relay a port Q whose host
        // listener we open, while the holder's loopback target is the echo server's port.
        let (target, _echo) = echo_server().await?;
        let listener = UnixListener::bind(&in_sock)?;
        let server = tokio::spawn(serve_inbound(listener, 0, u16::MAX, Arc::new(Vec::new)));
        let mut ports = InboundPorts::new(in_sock.clone());
        // The echo port itself is busy on this host: skipped, not an error.
        ports.sync(&[target]).await;
        assert!(ports.relayed().is_empty());
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let host_port = free.local_addr()?.port();
        drop(free);
        ports.sync(&[host_port]).await;
        assert_eq!(ports.relayed(), vec![host_port]);
        // Nothing listens on host_port "inside" (same host): the relay answers refused and the
        // client sees the connection close.
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, host_port)).await?;
        let mut buf = [0u8; 1];
        let read = with_timeout(Duration::from_secs(5), "eof", client.read(&mut buf)).await;
        assert!(matches!(read, Ok(0) | Err(_)));
        ports.sync(&[]).await;
        assert!(ports.relayed().is_empty());
        server.abort();
        Ok(())
    }
}
