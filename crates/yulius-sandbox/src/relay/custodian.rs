//! Short-lived descriptor custodian for an orderly restart (SPEC-577).
//!
//! The original server sends a batch on an inherited Unix stream. Each 'P' carries two
//! connected descriptors via SCM_RIGHTS, then a big-endian u32 and that many bytes of
//! private copy state. A 'D' record adds the original monotonic deadline (u64) before
//! the state size. An 'I' prefix carries the deadline and two u64 kernel socket cookies
//! without descriptors. A following 'T' transfers the two identified sockets and their state. 'E' ends the batch. The helper acknowledges with 'E' after binding
//! the successor's listener, and sends the same batch to its single accepted successor.
//! The successor acknowledges complete receipt with 'A'. The helper confirms with 'C'.
//! Until confirmation the helper can shut down every stream if delivery fails or expires.

use std::fs;
use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::net::{
    self, AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
    SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketType,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, Interest};
use tokio::net::{UnixListener, UnixStream};

const MAX_PAIRS: usize = 512; // 256 opaque tunnels and their matching out.sock relays.
const MAX_STATE: usize = 32 * 1024; // Two 8 KiB buffers, half-close state and ownership metadata.
const LIFETIME: Duration = Duration::from_secs(120);

/// Linux monotonic milliseconds, comparable between the departing and adopting processes.
#[must_use]
pub fn monotonic_ms() -> u64 {
    use rustix::time::{ClockId, clock_gettime};
    let now = clock_gettime(ClockId::Monotonic);
    let elapsed = Duration::try_from(now).unwrap_or_default();
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Kernel identities captured after both endpoints of an established copy have peers.
/// Linux retains the cookie when a duplicated TCP descriptor loses its peer name after FIN.
#[derive(Clone, Copy)]
pub struct Established([u64; 2]);

impl Established {
    /// Record both identities while the original connections are live.
    ///
    /// # Errors
    /// Either endpoint is disconnected or its kernel identity cannot be read.
    pub fn connected(left: impl AsFd, right: impl AsFd) -> io::Result<Self> {
        let cookie = |fd: std::os::fd::BorrowedFd<'_>| -> io::Result<u64> {
            net::getpeername(fd)?.ok_or(io::ErrorKind::NotConnected)?;
            Ok(net::sockopt::socket_cookie(fd)?)
        };
        Ok(Self([cookie(left.as_fd())?, cookie(right.as_fd())?]))
    }
}

/// Already-connected endpoints and their bounded private state. Never log this state.
pub struct Pair {
    /// The client-side and destination-side stream, in copy order.
    pub sockets: [OwnedFd; 2],
    /// Original Linux monotonic deadline, when the owning flow has one.
    pub expires: Option<u64>,
    /// The original identities of a timed copy enrolled while both sockets had peers.
    pub established: Option<Established>,
    /// Caller-encoded copy cursor and admission metadata.
    pub state: Vec<u8>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn stream_socket(fd: &OwnedFd) -> io::Result<()> {
    if net::sockopt::socket_type(fd)? == SocketType::STREAM
        && !net::sockopt::socket_acceptconn(fd)?
        && matches!(
            net::sockopt::socket_domain(fd)?,
            AddressFamily::INET | AddressFamily::INET6 | AddressFamily::UNIX
        )
    {
        Ok(())
    } else {
        Err(invalid("handoff requires stream sockets"))
    }
}

impl Pair {
    /// Verify that this copy can resume on the original sockets. A TCP receive queue
    /// remains readable after the final FIN even when Linux has discarded the peer name.
    ///
    /// # Errors
    /// Invalid state size, timing metadata, stream roles or identities, or a failed
    /// kernel query for the original socket.
    pub fn resumable(&self) -> io::Result<bool> {
        check_capacity(1, self.state.len())?;
        if self.established.is_some() && self.expires.is_none() {
            return Err(invalid("identified handoff requires a deadline"));
        }
        for (index, fd) in self.sockets.iter().enumerate() {
            stream_socket(fd)?;
            if let Some(ids) = self.established
                && net::sockopt::socket_cookie(fd)? != ids.0[index]
            {
                return Err(invalid("handoff socket identity changed"));
            }
        }
        for fd in &self.sockets {
            match net::getpeername(fd) {
                Ok(Some(_)) => continue,
                Err(rustix::io::Errno::NOTCONN) => {
                    if self
                        .expires
                        .is_some_and(|end| time_left(end, monotonic_ms()).is_zero())
                    {
                        continue;
                    }
                    if self.established.is_some() {
                        let mut byte = [0u8; 1];
                        let flags = RecvFlags::PEEK.union(RecvFlags::DONTWAIT);
                        if terminal_queue(net::recv(fd, &mut byte, flags).map(|_| ())) {
                            continue;
                        }
                    }
                    return Ok(false);
                }
                Ok(None) => return Err(invalid("handoff requires connected streams")),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(true)
    }
}

// At this point the original socket has been identified, but Linux reports no peer.
// A terminal transport error ends only this copy. An interrupted or not-yet-ready
// nonblocking read leaves the normal bounded transfer responsible for its next read.
fn terminal_queue(result: rustix::io::Result<()>) -> bool {
    matches!(
        result,
        Ok(()) | Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN)
    )
}

async fn message(stream: &UnixStream) -> io::Result<(u8, Vec<OwnedFd>)> {
    stream
        .async_io(Interest::READABLE, || {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
            let mut ancillary = RecvAncillaryBuffer::new(&mut space);
            let mut tag = [0];
            let result = net::recvmsg(
                stream,
                &mut [IoSliceMut::new(&mut tag)],
                &mut ancillary,
                RecvFlags::CMSG_CLOEXEC,
            )?;
            // A stream header has one byte. At EOF the untouched zero tag below is invalid.
            if result.flags.contains(ReturnFlags::CTRUNC) {
                return Err(invalid("incomplete tunnel handoff header"));
            }
            let mut sockets = Vec::new();
            for message in ancillary.drain() {
                let RecvAncillaryMessage::ScmRights(rights) = message else {
                    return Err(invalid("unexpected tunnel handoff control message"));
                };
                sockets.extend(rights);
            }
            match (tag[0], sockets.len()) {
                (b'I' | b'E', 0) => {}
                (b'P' | b'D' | b'T', 2) => {
                    for fd in &sockets {
                        stream_socket(fd)?;
                    }
                }
                _ => return Err(invalid("invalid tunnel handoff message")),
            }
            Ok((tag[0], sockets))
        })
        .await
}

async fn header(stream: &mut UnixStream) -> io::Result<Option<Pair>> {
    let (tag, sockets) = message(stream).await?;
    let (sockets, expires, established) = match tag {
        b'E' => return Ok(None),
        b'P' => (sockets, None, None),
        b'D' => (sockets, Some(stream.read_u64().await?), None),
        b'I' => {
            // No descriptors are in our custody while these fields may be interrupted.
            let expires = stream.read_u64().await?;
            let ids = Established([stream.read_u64().await?, stream.read_u64().await?]);
            let (tag, sockets) = message(stream).await?;
            if tag != b'T' {
                return Err(invalid("expected identified tunnel sockets"));
            }
            (sockets, Some(expires), Some(ids))
        }
        _ => return Err(invalid("unknown tunnel handoff header")),
    };
    let sockets = sockets
        .try_into()
        .map_err(|_| invalid("expected a socket pair"))?;
    Ok(Some(Pair {
        sockets,
        expires,
        established,
        state: Vec::new(),
    }))
}

fn check_capacity(count: usize, state_bytes: usize) -> io::Result<()> {
    if count > MAX_PAIRS || state_bytes > MAX_STATE {
        return Err(invalid("tunnel handoff exceeds capacity"));
    }
    Ok(())
}

async fn receive(stream: &mut UnixStream) -> io::Result<PendingDelivery> {
    receive_with(stream, Pair::resumable).await
}

async fn receive_with(
    stream: &mut UnixStream,
    resumable: impl Fn(&Pair) -> io::Result<bool>,
) -> io::Result<PendingDelivery> {
    let mut pending = PendingDelivery(Vec::new());
    let mut accepted = false;
    while let Some(pair) = pending.during(header(stream)).await? {
        // Validate roles and identities before granting rollback authority. A lost
        // connection still occupies one input frame and is drained under the size bound.
        let connected = resumable(&pair)?;
        accepted |= connected;
        pending.0.push(connected.then_some(pair));
        let size = pending.during(stream.read_u32()).await? as usize;
        check_capacity(pending.0.len(), size)?;
        let mut state = vec![0; size];
        pending.during(stream.read_exact(&mut state)).await?;
        if let Some(Some(pair)) = pending.0.last_mut() {
            pair.state = state;
        }
    }
    if !accepted && !pending.0.is_empty() {
        return Err(io::ErrorKind::NotConnected.into());
    }
    Ok(pending)
}

async fn send_header(stream: &UnixStream, sockets: &[OwnedFd; 2], tag: u8) -> io::Result<()> {
    stream
        .async_io(Interest::WRITABLE, || {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
            let mut ancillary = SendAncillaryBuffer::new(&mut space);
            let fds = sockets.each_ref().map(AsFd::as_fd);
            if !ancillary.push(SendAncillaryMessage::ScmRights(&fds)) {
                return Err(invalid("insufficient ancillary space"));
            }
            if net::sendmsg(
                stream,
                &[IoSlice::new(&[tag])],
                &mut ancillary,
                SendFlags::NOSIGNAL,
            )? != 1
            {
                return Err(invalid("incomplete tunnel handoff write"));
            }
            Ok(())
        })
        .await
}

async fn send_record(stream: &mut UnixStream, pair: &Pair) -> io::Result<()> {
    match (pair.expires, pair.established) {
        (Some(expires), Some(ids)) => {
            stream.write_u8(b'I').await?;
            stream.write_u64(expires).await?;
            stream.write_u64(ids.0[0]).await?;
            stream.write_u64(ids.0[1]).await?;
            send_header(stream, &pair.sockets, b'T').await?;
        }
        (Some(expires), None) => {
            send_header(stream, &pair.sockets, b'D').await?;
            stream.write_u64(expires).await?;
        }
        (None, None) => send_header(stream, &pair.sockets, b'P').await?,
        (None, Some(_)) => return Err(invalid("identified handoff requires a deadline")),
    }
    stream
        .write_u32(u32::try_from(pair.state.len()).map_err(|_| invalid("copy state too large"))?)
        .await?;
    stream.write_all(&pair.state).await
}

struct SocketPath {
    path: PathBuf,
    inode: u64,
}

// The directory lock belongs to this process and is released even on SIGKILL. A new helper
// can reclaim a stale socket without unlinking another helper's live adoption listener.
fn lock_directory(directory: &Path) -> io::Result<fs::File> {
    let lock = fs::File::open(directory)?;
    lock.try_lock()?;
    Ok(lock)
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => fs::remove_file(path),
        Ok(_) => Err(invalid("expected a socket in the handoff directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn private_directory(directory: &Path) -> io::Result<u32> {
    let owner = nix::unistd::geteuid().as_raw();
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "expected private service directory",
        ));
    }
    Ok(owner)
}

fn service_peer(stream: &UnixStream, owner: u32) -> io::Result<()> {
    if stream.peer_cred()?.uid() != owner {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unexpected handoff peer",
        ));
    }
    Ok(())
}

// SCM_RIGHTS already queued to a stalled receiver can outlive the sending control socket.
// Retain the original endpoints until confirmation so an incomplete transfer can close
// both halves even if a duplicate is still in the receiver's queue or descriptor table.
struct PendingDelivery(Vec<Option<Pair>>);

fn shut_down(pair: &Pair) {
    for socket in &pair.sockets {
        // An expired frame may contain a TCP endpoint whose peer has already gone.
        // shutdown on an unconnected Linux socket can disable a future owner's writes.
        if net::getpeername(socket).is_ok_and(|peer| peer.is_some()) {
            let _ = net::shutdown(socket, net::Shutdown::Both);
        }
    }
}

fn time_left(expires: u64, now: u64) -> Duration {
    Duration::from_millis(expires.saturating_sub(now))
}

impl PendingDelivery {
    // A pair that expires releases its state and both original handles. Shutdown also
    // reaches any rights already queued at the receiving process.
    fn expire(&mut self, now: u64) -> Option<Duration> {
        let mut next = None;
        for slot in &mut self.0 {
            if let Some(expires) = slot.as_ref().and_then(|p| p.expires) {
                let wait = time_left(expires, now);
                if wait.is_zero() {
                    if let Some(pair) = slot.take() {
                        shut_down(&pair);
                    }
                } else {
                    next = Some(next.map_or(wait, |old: Duration| old.min(wait)));
                }
            }
        }
        next
    }

    // Receipt, accept, send and confirmation all run under the original per-pair clocks.
    async fn during<T>(&mut self, operation: impl Future<Output = io::Result<T>>) -> io::Result<T> {
        tokio::pin!(operation);
        loop {
            let Some(wait) = self.expire(monotonic_ms()) else {
                return operation.await;
            };
            tokio::select! {
                biased;
                () = tokio::time::sleep(wait) => {}
                result = &mut operation => return result,
            }
        }
    }

    async fn send(&mut self, stream: &mut UnixStream) -> io::Result<()> {
        for index in 0..self.0.len() {
            self.expire(monotonic_ms());
            let Some(pair) = &mut self.0[index] else {
                continue;
            };
            // Only the record being written has transient duplicates. All originals remain
            // available for shutdown if the successor stalls in the middle of this frame.
            let [left, right] = &pair.sockets;
            let sending = Pair {
                sockets: [
                    left.as_fd().try_clone_to_owned()?,
                    right.as_fd().try_clone_to_owned()?,
                ],
                expires: pair.expires,
                established: pair.established,
                state: std::mem::take(&mut pair.state),
            };
            self.during(send_record(stream, &sending)).await?;
        }
        self.during(stream.write_all(b"E")).await
    }

    fn release(mut self) -> Vec<Pair> {
        self.expire(monotonic_ms());
        std::mem::take(&mut self.0).into_iter().flatten().collect()
    }
}

impl Drop for PendingDelivery {
    fn drop(&mut self) {
        for pair in self.0.iter().flatten() {
            shut_down(pair);
        }
    }
}

async fn hold(control: &mut UnixStream, directory: &Path) -> io::Result<()> {
    let owner = private_directory(directory)?;
    service_peer(control, owner)?;
    let _lock = lock_directory(directory)?;
    let mut pending = receive(control).await?;
    let path = directory.join("tunnels.sock");
    remove_stale_socket(&path)?;
    let listener = UnixListener::bind(&path)?;
    let guard = SocketPath {
        inode: fs::symlink_metadata(&path)?.ino(),
        path,
    };
    // From the moment the batch is sealed, the last original deadline covers the
    // acknowledgement, accept, all frames and final confirmation. Untimed records
    // remain bounded by the helper's overall lifetime.
    let last = pending.0.iter().flatten().try_fold(0, |end: u64, pair| {
        pair.expires.map(|expires| end.max(expires))
    });
    let delivery = async {
        pending.during(control.write_all(b"E")).await?;
        let (mut successor, _) = pending.during(listener.accept()).await?;
        drop(listener);
        drop(guard);
        service_peer(&successor, owner)?;
        net::sockopt::set_socket_send_buffer_size(&successor, MAX_STATE)?;
        pending.send(&mut successor).await?;
        if pending.during(successor.read_u8()).await? != b'A' {
            return Err(invalid("batch receipt was not acknowledged"));
        }
        pending.during(successor.write_all(b"C")).await?;
        pending.0.clear(); // Confirmed. Drop originals without shutting down the sockets.
        Ok(())
    };
    if let Some(last) = last {
        let wait = time_left(last, monotonic_ms()).min(LIFETIME);
        tokio::time::timeout(wait, delivery).await.unwrap_or(Ok(()))
    } else {
        delivery.await
    }
}

async fn hold_until(
    control: &mut UnixStream,
    directory: &Path,
    expiry: impl Future<Output = ()>,
) -> io::Result<()> {
    tokio::select! {
        biased;
        () = expiry => Err(io::ErrorKind::TimedOut.into()),
        result = hold(control, directory) => result,
    }
}

/// Runs the internal helper with the old server's connected Unix control socket on stdin.
/// One deadline covers receiving the batch, waiting for the successor and sending the batch.
///
/// # Errors
/// Returns an error for invalid input, loss of either control connection or expiry.
pub fn run(directory: &Path) -> io::Result<()> {
    let input = StdUnixStream::from(io::stdin().as_fd().try_clone_to_owned()?);
    input.set_nonblocking(true)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut control = UnixStream::from_std(input)?;
            hold_until(&mut control, directory, tokio::time::sleep(LIFETIME)).await
        })
}

/// Collect the replies to a bounded capture in admission order. Keep each original
/// deadline running while other copies finish pausing. Dropped senders yield no pair.
///
/// # Errors
/// Rejects too many captures or a failed collection task. The caller bounds the whole wait.
pub async fn collect(replies: Vec<tokio::sync::oneshot::Receiver<Pair>>) -> io::Result<Vec<Pair>> {
    check_capacity(replies.len(), 0)?;
    let mut pending = PendingDelivery((0..replies.len()).map(|_| None).collect());
    let mut waiting = tokio::task::JoinSet::new();
    for (index, reply) in replies.into_iter().enumerate() {
        waiting.spawn(async move { (index, reply.await.ok()) });
    }
    while let Some(result) = pending
        .during(async { Ok(waiting.join_next().await) })
        .await?
    {
        let (index, pair) = result?;
        if let Some(pair) = pair
            && pair.resumable()?
        {
            pending.0[index] = Some(pair);
        }
    }
    Ok(pending.release())
}

/// Send a batch to a helper that survives this process. On success the helper has bound its
/// single-consumer listener and holds all the sockets. An empty batch starts no helper.
///
/// # Errors
/// Fails on invalid capacity, directory permissions, failure to start or missing acknowledgement.
pub async fn deposit(executable: &Path, directory: &Path, pairs: Vec<Pair>) -> io::Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    check_capacity(pairs.len(), 0)?;
    let mut connected = Vec::with_capacity(pairs.len());
    for pair in pairs {
        if pair.resumable()? {
            connected.push(pair);
        }
    }
    if connected.is_empty() {
        return Err(io::ErrorKind::NotConnected.into());
    }
    private_directory(directory)?;
    let (mut stream, child_socket) = UnixStream::pair()?;
    let stdin = OwnedFd::from(child_socket.into_std()?);
    let mut child = tokio::process::Command::new(executable)
        .arg("sandbox-host")
        .arg("hold-tunnels")
        .arg("--directory")
        .arg(directory)
        .process_group(0)
        .env_clear()
        .stdin(std::process::Stdio::from(stdin))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Reap while the depositing server is around. When it exits the helper is reparented.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    tokio::time::timeout(LIFETIME, async {
        let mut pending = PendingDelivery(connected.into_iter().map(Some).collect());
        pending.send(&mut stream).await?;
        if pending.during(stream.read_u8()).await? != b'E' {
            return Err(invalid("handoff was not acknowledged"));
        }
        pending.0.clear();
        Ok(())
    })
    .await?
}

/// Receive the previous server's complete batch. Partial batches drop all received descriptors.
/// A missing socket is reported as `NotFound` so startup can distinguish an ordinary cold start.
///
/// # Errors
/// Fails on directory or peer verification, invalid input, disconnection or deadline expiry.
pub async fn adopt(directory: &Path) -> io::Result<Vec<Pair>> {
    let owner = private_directory(directory)?;
    tokio::time::timeout(LIFETIME, async {
        let path = directory.join("tunnels.sock");
        let mut stream = match UnixStream::connect(&path).await {
            Ok(stream) => stream,
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                let _lock = lock_directory(directory)?;
                remove_stale_socket(&path)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        service_peer(&stream, owner)?;
        let mut pairs = receive(&mut stream).await?;
        pairs.during(stream.write_all(b"A")).await?;
        if pairs.during(stream.read_u8()).await? != b'C' {
            return Err(invalid("adoption was not confirmed"));
        }
        Ok(pairs.release())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn expiry_while_another_copy_is_still_pausing_releases_both_peers() -> io::Result<()> {
        let (send, short) = tokio::sync::oneshot::channel();
        let (delayed, other) = tokio::sync::oneshot::channel();
        let (left, mut client) = UnixStream::pair()?;
        let (right, mut upstream) = UnixStream::pair()?;
        let _ = send.send(Pair {
            established: None,
            sockets: [left.into_std()?.into(), right.into_std()?.into()],
            state: b"expired".to_vec(),
            expires: Some(monotonic_ms() + 200),
        });
        let collection = tokio::spawn(collect(vec![short, other]));
        tokio::time::timeout(Duration::from_secs(2), async {
            assert_eq!(client.read(&mut [0]).await?, 0);
            assert_eq!(upstream.read(&mut [0]).await?, 0);
            io::Result::Ok(())
        })
        .await??;
        assert!(!collection.is_finished());
        drop(delayed);
        assert!(collection.await??.is_empty());
        Ok(())
    }
    #[test]
    fn handoff_clock_uses_the_same_epoch_as_other_processes() -> io::Result<()> {
        use rustix::time::{ClockId, clock_gettime};
        let kernel_ms = || {
            Duration::try_from(clock_gettime(ClockId::Monotonic))
                .map(|elapsed| elapsed.as_millis())
                .map_err(io::Error::other)
        };
        let before = kernel_ms()?;
        let handoff = u128::from(monotonic_ms());
        assert!((before..=kernel_ms()?).contains(&handoff));
        Ok(())
    }

    async fn send(stream: &mut UnixStream, records: &[Pair]) -> io::Result<()> {
        for record in records {
            send_record(stream, record).await?;
        }
        stream.write_all(b"E").await
    }

    fn pair(state: Vec<u8>) -> io::Result<(Pair, [UnixStream; 2])> {
        let (left, a) = UnixStream::pair()?;
        let (right, b) = UnixStream::pair()?;
        Ok((
            Pair {
                established: None,
                expires: None,
                sockets: [left.into_std()?.into(), right.into_std()?.into()],
                state,
            },
            [a, b],
        ))
    }

    #[tokio::test]
    async fn custody_wakes_at_the_earliest_deadline_and_releases_at_its_exact_tick()
    -> io::Result<()> {
        let (mut short, mut peers) = pair(Vec::new())?;
        let (mut long, _live) = pair(Vec::new())?;
        short.expires = Some(10_450);
        long.expires = Some(12_000);
        let mut pending = PendingDelivery(vec![Some(long), Some(short)]);
        assert_eq!(pending.expire(10_000), Some(Duration::from_millis(450)));
        assert_eq!(pending.expire(10_449), Some(Duration::from_millis(1)));
        assert_eq!(pending.expire(10_450), Some(Duration::from_millis(1550)));
        assert!(pending.0[1].is_none());
        for peer in &mut peers {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), peer.read(&mut [0])).await??,
                0
            );
        }
        assert!(pending.expire(12_001).is_none());
        assert!(pending.0.iter().all(Option::is_none));
        Ok(())
    }

    #[tokio::test]
    async fn an_unclaimed_batch_expires_and_releases_both_peers() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
        let (mut old, mut control) = UnixStream::pair()?;
        let (saved, mut peers) = pair(b"pending".to_vec())?;
        let serving = async {
            let (expire, expired) = tokio::sync::oneshot::channel();
            let holding = hold_until(&mut control, dir.path(), async {
                let _ = expired.await;
            });
            tokio::pin!(holding);
            tokio::select! {
                result = &mut holding => return result.and(Err(invalid("helper ended before acknowledgement"))),
                answer = async {
                    send(&mut old, &[saved]).await?;
                    old.read_u8().await
                } => assert_eq!(answer?, b'E'),
            }
            assert!(dir.path().join("tunnels.sock").exists());
            let _ = expire.send(());
            assert!(
                holding
                    .await
                    .is_err_and(|error| error.kind() == io::ErrorKind::TimedOut)
            );
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(5), serving).await??;
        assert!(!dir.path().join("tunnels.sock").exists());
        for peer in &mut peers {
            assert_eq!(peer.read(&mut [0]).await?, 0);
        }
        Ok(())
    }

    #[tokio::test]
    async fn last_pair_expiring_frees_the_waiting_helpers_listener_and_lock() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let dir = tempfile::tempdir()?;
            fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
            let (mut old, mut control) = UnixStream::pair()?;
            let (mut saved, mut peers) = pair(b"short".to_vec())?;
            saved.expires = Some(monotonic_ms() + 200);
            let path = dir.path().to_owned();
            let helper = tokio::spawn(async move { hold(&mut control, &path).await });
            send(&mut old, &[saved]).await?;
            assert_eq!(old.read_u8().await?, b'E');
            for peer in &mut peers {
                assert_eq!(peer.read(&mut [0]).await?, 0);
            }
            helper.await??;
            assert!(!dir.path().join("tunnels.sock").exists());
            lock_directory(dir.path())?;
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn last_expiry_releases_the_helper_place_with_a_successor_stalled_at_receipt()
    -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let dir = tempfile::tempdir()?;
            fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
            let (mut old, mut control) = UnixStream::pair()?;
            let (mut saved, mut peers) = pair(b"awaiting acknowledgement".to_vec())?;
            saved.expires = Some(monotonic_ms() + 500);
            let path = dir.path().to_owned();
            let helper = tokio::spawn(async move { hold(&mut control, &path).await });
            send(&mut old, &[saved]).await?;
            assert_eq!(old.read_u8().await?, b'E');
            let mut stalled = UnixStream::connect(dir.path().join("tunnels.sock")).await?;
            let received = receive(&mut stalled).await?.release();
            assert_eq!(received.len(), 1);
            for peer in &mut peers {
                assert_eq!(peer.read(&mut [0]).await?, 0);
            }
            // The successor keeps its original receipt and both duplicates open.
            helper.await??;
            assert_eq!(stalled.read(&mut [0]).await?, 0);
            lock_directory(dir.path())?;
            drop(received);
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn queued_expired_tcp_rights_do_not_discard_the_live_tcp_transfer() -> io::Result<()> {
        use tokio::net::{TcpListener, TcpStream};
        async fn tcp_pair() -> io::Result<(Pair, [TcpStream; 2])> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let a = TcpStream::connect(address).await?;
            let (left, _) = listener.accept().await?;
            let b = TcpStream::connect(address).await?;
            let (right, _) = listener.accept().await?;
            Ok((
                Pair {
                    established: None,
                    sockets: [left.into_std()?.into(), right.into_std()?.into()],
                    state: b"private cursor".to_vec(),
                    expires: Some(monotonic_ms() + 60_000),
                },
                [a, b],
            ))
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut short, [mut closing, _]) = tcp_pair().await?;
            let (long, [mut client, mut peer]) = tcp_pair().await?;
            short.expires = Some(monotonic_ms() + 100);
            // The helper queues both descriptor frames before the successor reads them.
            let (mut sender, mut receiver) = UnixStream::pair()?;
            send_record(&mut sender, &short).await?;
            send_record(&mut sender, &long).await?;
            sender.write_all(b"E").await?;
            drop(long);
            closing.shutdown().await?;
            // On real TCP a completed FIN exchange makes getpeername return ENOTCONN,
            // even on the descriptors already in the receiver's Unix socket queue.
            let mut held = PendingDelivery(vec![Some(short)]);
            assert_eq!(held.during(closing.read(&mut [0])).await?, 0);
            client.write_all(b"queued").await?;
            let mut batch = receive(&mut receiver).await?.release();
            assert_eq!(batch.len(), 1);
            let survivor = batch
                .pop()
                .ok_or_else(|| invalid("missing live TCP pair"))?;
            let [left, right] = survivor.sockets;
            let mut left = TcpStream::from_std(left.into())?;
            let mut right = TcpStream::from_std(right.into())?;
            let mut incoming = [0; 6];
            left.read_exact(&mut incoming).await?;
            assert_eq!(&incoming, b"queued");
            right.write_all(b"fresh").await?;
            let mut marker = [0; 5];
            peer.read_exact(&mut marker).await?;
            assert_eq!(&marker, b"fresh");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn expiration_preserves_frame_bounds_and_stream_type_checks() -> io::Result<()> {
        let (mut sender, mut receiver) = UnixStream::pair()?;
        let (record, _peers) = pair(Vec::new())?;
        send_header(&sender, &record.sockets, b'D').await?;
        sender.write_u64(0).await?;
        sender
            .write_u32(u32::try_from(MAX_STATE + 1).map_err(io::Error::other)?)
            .await?;
        sender.shutdown().await?;
        assert!(
            receive(&mut receiver)
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
        );

        let (mut sender, mut receiver) = UnixStream::pair()?;
        let (datagram, _peer) = std::os::unix::net::UnixDatagram::pair()?;
        let [left, _] = record.sockets;
        send_header(&sender, &[left, datagram.into()], b'D').await?;
        sender.write_u64(0).await?;
        sender.write_u32(0).await?;
        sender.write_all(b"E").await?;
        assert!(
            receive(&mut receiver)
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
        );

        let (mut sender, mut receiver) = UnixStream::pair()?;
        let (mut live, _peers) = pair(Vec::new())?;
        live.expires = Some(monotonic_ms() + 60_000);
        // A fresh TCP endpoint passes role and family checks but has never had a peer.
        live.sockets[0] = net::socket(AddressFamily::INET, SocketType::STREAM, None)?;
        send(&mut sender, &[live]).await?;
        assert!(
            receive(&mut receiver)
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::NotConnected)
        );
        Ok(())
    }

    #[tokio::test]
    async fn collecting_a_capture_checks_identity_before_its_deadline_owns_the_original()
    -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (other, _other_peers) = pair(Vec::new())?;
            let (mut captured, [mut client, _right]) = pair(b"cursor".to_vec())?;
            let retained = captured.sockets[0].as_fd().try_clone_to_owned()?;
            let mut original = UnixStream::from_std(retained.into())?;
            let end = monotonic_ms() + 100;
            captured.expires = Some(end);
            captured.established = Some(Established::connected(
                &other.sockets[0],
                &captured.sockets[1],
            )?);
            let (reply, ready) = tokio::sync::oneshot::channel();
            assert!(reply.send(captured).is_ok());
            assert!(
                collect(vec![ready])
                    .await
                    .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
            );
            // Passing the refused record's original deadline cannot take the sender's connection.
            assert!(
                tokio::time::timeout(time_left(end, monotonic_ms()), client.read_u8())
                    .await
                    .is_err()
            );
            client.write_all(b"kept").await?;
            assert_eq!(original.read_u32().await?.to_be_bytes(), *b"kept");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn collecting_rejects_out_of_bounds_state_and_untimed_identities_before_custody()
    -> io::Result<()> {
        let mut refused = Vec::new();
        for oversized in [true, false] {
            let (mut captured, _peers) = pair(Vec::new())?;
            if oversized {
                captured.state.resize(MAX_STATE + 1, 0);
            } else {
                captured.established = Some(Established::connected(
                    &captured.sockets[0],
                    &captured.sockets[1],
                )?);
            }
            let (reply, ready) = tokio::sync::oneshot::channel();
            assert!(reply.send(captured).is_ok());
            refused.push(
                collect(vec![ready])
                    .await
                    .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData),
            );
        }
        assert_eq!(
            refused,
            [true, true],
            "oversized state and untimed identity"
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_sender_supplies_complete_identities_before_transferring_ownership()
    -> io::Result<()> {
        let (mut record, _peers) = pair(b"saved".to_vec())?;
        let ids = Established::connected(&record.sockets[0], &record.sockets[1])?;
        let end = monotonic_ms() + 3_000;
        record.expires = Some(end);
        record.established = Some(ids);
        let (mut sender, mut receiver) = UnixStream::pair()?;
        send_record(&mut sender, &record).await?;
        // Read one byte of the valid producer's first message, including any attached handles.
        let mut bytes = [0u8; 1];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        net::recvmsg(
            &receiver,
            &mut [IoSliceMut::new(&mut bytes)],
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC,
        )?;
        assert!(
            ancillary.drain().next().is_none(),
            "metadata has no socket ownership"
        );
        assert_eq!(bytes, [b'I']);
        assert_eq!(receiver.read_u64().await?, end);
        assert_eq!(receiver.read_u64().await?, ids.0[0]);
        assert_eq!(receiver.read_u64().await?, ids.0[1]);
        Ok(())
    }

    #[tokio::test]
    async fn an_interrupted_identity_prefix_leaves_the_original_connection_with_its_sender()
    -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            for wait_for_expiry in [false, true] {
                let (record, [mut client, _right]) = pair(Vec::new())?;
                let ids = Established::connected(&record.sockets[0], &record.sockets[1])?;
                let (mut sender, mut receiver) = UnixStream::pair()?;
                let mut receiving = tokio::spawn(async move { receive(&mut receiver).await });
                let end = monotonic_ms() + 50;
                sender.write_u8(b'I').await?;
                sender.write_u64(end).await?;
                sender.write_u64(ids.0[0]).await?;
                if wait_for_expiry {
                    assert!(
                        tokio::time::timeout(time_left(end, monotonic_ms()), &mut receiving)
                            .await
                            .is_err()
                    );
                }
                sender.shutdown().await?;
                assert!(
                    receiving
                        .await?
                        .is_err_and(|e| e.kind() == io::ErrorKind::UnexpectedEof)
                );
                let [left, _right] = record.sockets;
                let mut original = UnixStream::from_std(left.into())?;
                client.write_all(b"kept").await?;
                assert_eq!(original.read_u32().await?.to_be_bytes(), *b"kept");
            }
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_terminal_transport_timeout_leaves_the_other_original_stream_usable() -> io::Result<()>
    {
        tokio::time::timeout(Duration::from_secs(3), async {
            // Inject the terminal read outcome of a timed-out TCP stream at the queue check.
            let (ended, _closed) = pair(Vec::new())?;
            let ended_cookie = net::sockopt::socket_cookie(&ended.sockets[0])?;
            let (healthy, [mut client, mut peer]) = pair(b"healthy".to_vec())?;
            let (mut old, mut next) = UnixStream::pair()?;
            send(&mut old, &[ended, healthy]).await?;
            let mut received = receive_with(&mut next, |candidate| {
                if net::sockopt::socket_cookie(&candidate.sockets[0])? == ended_cookie {
                    Ok(terminal_queue(Err(rustix::io::Errno::TIMEDOUT)))
                } else {
                    candidate.resumable()
                }
            })
            .await?
            .release();
            assert_eq!(received.len(), 1);
            let saved = received
                .pop()
                .ok_or_else(|| invalid("missing healthy stream"))?;
            let [left, right] = saved.sockets;
            let mut left = UnixStream::from_std(left.into())?;
            let mut right = UnixStream::from_std(right.into())?;
            client.write_all(b"live").await?;
            assert_eq!(left.read_u32().await?.to_be_bytes(), *b"live");
            right.write_all(b"back").await?;
            assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"back");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn an_identity_from_another_pair_cannot_take_its_live_endpoints() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (other, _peers) = pair(Vec::new())?;
            let (mut record, [mut peer, _right]) = pair(b"kept".to_vec())?;
            let retained = record.sockets[0].as_fd().try_clone_to_owned()?;
            let mut original = UnixStream::from_std(retained.into())?;
            record.established = Some(Established::connected(
                &other.sockets[0],
                &record.sockets[1],
            )?);
            record.expires = Some(monotonic_ms() + 3_000);
            assert!(
                record
                    .resumable()
                    .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
            );
            let (mut sender, mut receiver) = UnixStream::pair()?;
            send(&mut sender, &[record]).await?;
            assert!(
                receive(&mut receiver)
                    .await
                    .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
            );
            peer.write_all(b"original").await?;
            let mut marker = [0; 8];
            original.read_exact(&mut marker).await?;
            assert_eq!(&marker, b"original");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_disconnected_record_still_requires_a_complete_bounded_frame() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            for (size, expected) in [
                (5, io::ErrorKind::UnexpectedEof),
                (
                    u32::try_from(MAX_STATE + 1).map_err(io::Error::other)?,
                    io::ErrorKind::InvalidData,
                ),
            ] {
                let (mut sender, mut receiver) = UnixStream::pair()?;
                let (live, [mut left, mut right]) = pair(b"live".to_vec())?;
                send_record(&mut sender, &live).await?;
                let (mut lost, _peers) = pair(Vec::new())?;
                lost.sockets[0] = net::socket(AddressFamily::INET, SocketType::STREAM, None)?;
                send_header(&sender, &lost.sockets, b'P').await?;
                sender.write_u32(size).await?;
                sender.write_all(b"x").await?;
                sender.shutdown().await?;
                assert!(
                    receive(&mut receiver)
                        .await
                        .is_err_and(|e| e.kind() == expected)
                );
                // Incomplete delivery shuts down even the fully received predecessor.
                assert_eq!(left.read(&mut [0]).await?, 0);
                assert_eq!(right.read(&mut [0]).await?, 0);
            }
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn an_expired_record_cannot_claim_or_shut_down_a_listening_socket() -> io::Result<()> {
        use tokio::net::{TcpListener, TcpStream};
        tokio::time::timeout(Duration::from_secs(3), async {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let (mut sender, mut receiver) = UnixStream::pair()?;
            let (mut saved, _peers) = pair(b"expired".to_vec())?;
            saved.expires = Some(0);
            saved.sockets[1] = listener.as_fd().try_clone_to_owned()?;
            send(&mut sender, &[saved]).await?;
            let result = receive(&mut receiver).await;
            assert!(
                net::sockopt::socket_acceptconn(&listener)?,
                "the sender retained a live listener"
            );
            assert!(result.is_err_and(|e| e.kind() == io::ErrorKind::InvalidData));
            let client = TcpStream::connect(address).await?;
            assert_eq!(listener.accept().await?.1, client.local_addr()?);
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn an_expired_disconnected_descriptor_can_still_be_used_by_its_original_owner()
    -> io::Result<()> {
        use tokio::net::{TcpListener, TcpSocket};
        tokio::time::timeout(Duration::from_secs(3), async {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let unconnected = TcpSocket::new_v4()?;
            let (mut short, mut peers) = pair(Vec::new())?;
            short.sockets[0] = unconnected.as_fd().try_clone_to_owned()?;
            short.expires = Some(0);
            let (mut sender, mut receiver) = UnixStream::pair()?;
            send(&mut sender, &[short]).await?;
            assert!(receive(&mut receiver).await?.release().is_empty());
            assert_eq!(peers[1].read(&mut [0]).await?, 0);
            let mut client = unconnected.connect(listener.local_addr()?).await?;
            let (mut peer, _) = listener.accept().await?;
            client.write_all(b"okay").await?;
            assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"okay");
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_partial_timed_frame_expires_before_its_producer_finishes_writing() -> io::Result<()>
    {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (mut sender, mut receiver) = UnixStream::pair()?;
            let (short, mut peers) = pair(Vec::new())?;
            let (long, _live_peers) = pair(b"survivor".to_vec())?;
            let reading = tokio::spawn(async move { receive(&mut receiver).await });
            send_header(&sender, &short.sockets, b'D').await?;
            sender.write_u64(monotonic_ms() + 100).await?;
            sender.write_u32(6).await?;
            sender.write_all(b"pre").await?;
            for peer in &mut peers {
                assert_eq!(peer.read(&mut [0]).await?, 0);
            }
            assert!(!reading.is_finished());
            sender.write_all(b"fix").await?;
            send(&mut sender, &[long]).await?;
            let live = reading.await??.release();
            assert_eq!(live.len(), 1);
            assert_eq!(live[0].state, b"survivor");
            drop(short);
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn a_short_or_oversize_record_drops_the_whole_batch() -> io::Result<()> {
        for size in [MAX_STATE + 1, 10] {
            let (mut source, mut dest) = UnixStream::pair()?;
            let (record, mut peers) = pair(Vec::new())?;
            let (previous, mut previous_peers) = pair(Vec::new())?;
            send_header(&source, &previous.sockets, b'P').await?;
            source.write_u32(0).await?;
            drop(previous);
            // Send a real pair, then truncate the state or declare more than the limit.
            send_header(&source, &record.sockets, b'P').await?;
            drop(record);
            source
                .write_u32(u32::try_from(size).map_err(io::Error::other)?)
                .await?;
            source.shutdown().await?;
            let error = receive(&mut dest)
                .await
                .err()
                .ok_or_else(|| invalid("accepted invalid batch"))?;
            assert_eq!(
                error.kind(),
                if size == 10 {
                    io::ErrorKind::UnexpectedEof
                } else {
                    io::ErrorKind::InvalidData
                }
            );
            for peer in peers.iter_mut().chain(&mut previous_peers) {
                assert_eq!(peer.read(&mut [0]).await?, 0);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_cold_start_reclaims_only_the_stale_socket() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
        let path = dir.path().join("tunnels.sock");
        drop(UnixListener::bind(&path)?);
        assert!(
            adopt(dir.path())
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::ConnectionRefused)
        );
        assert!(!path.exists());
        fs::write(&path, b"keep")?;
        assert!(remove_stale_socket(&path).is_err());
        assert_eq!(fs::read(&path)?, b"keep");
        Ok(())
    }

    #[tokio::test]
    async fn a_public_directory_or_published_symlink_cannot_receive_state() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::set_permissions(dir.path(), Permissions::from_mode(0o755))?;
        assert!(
            adopt(dir.path())
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::PermissionDenied)
        );
        let private = tempfile::tempdir()?;
        fs::set_permissions(private.path(), Permissions::from_mode(0o700))?;
        let alias = private.path().join("alias");
        std::os::unix::fs::symlink(private.path(), &alias)?;
        assert!(
            adopt(&alias)
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::PermissionDenied)
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_empty_deposit_needs_no_helper_and_oversize_state_fails_before_spawn()
    -> io::Result<()> {
        let missing = Path::new("/no-such-custodian-executable");
        deposit(missing, missing, vec![]).await?;
        let (large, _) = pair(vec![0; MAX_STATE + 1])?;
        assert!(
            deposit(missing, missing, vec![large])
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
        );
        let (full, _) = pair(vec![0; MAX_STATE])?;
        assert!(
            deposit(missing, missing, vec![full])
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_pair_requires_two_connected_streams_and_an_end_has_no_descriptors() -> io::Result<()>
    {
        let (mut sender, mut receiver) = UnixStream::pair()?;
        sender.write_all(b"P").await?;
        assert!(header(&mut receiver).await.is_err());
        sender.write_all(b"?").await?;
        assert!(header(&mut receiver).await.is_err());
        let (files, _) = pair(vec![])?;
        let (datagram, _peer) = std::os::unix::net::UnixDatagram::pair()?;
        let nonstream: OwnedFd = datagram.into();
        let [stream, _] = files.sockets;
        send_header(&sender, &[stream, nonstream], b'P').await?;
        assert!(header(&mut receiver).await.is_err());
        // A terminator carrying descriptors and a truncated rights array both reject the batch.
        let (record, _) = pair(vec![])?;
        for (tag, count) in [(b'E', 2), (b'P', 4)] {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(4))];
            let mut ancillary = SendAncillaryBuffer::new(&mut space);
            let fds = [record.sockets[0].as_fd(); 4];
            assert!(ancillary.push(SendAncillaryMessage::ScmRights(&fds[..count])));
            net::sendmsg(
                &sender,
                &[IoSlice::new(&[tag])],
                &mut ancillary,
                SendFlags::NOSIGNAL,
            )?;
            assert!(header(&mut receiver).await.is_err());
        }
        sender.shutdown().await?;
        assert!(header(&mut receiver).await.is_err());
        Ok(())
    }

    #[test]
    fn the_batch_fits_every_tunnel_and_relay_but_rejects_one_extra_record_or_byte() -> io::Result<()>
    {
        check_capacity(512, 32 * 1024)?;
        assert!(check_capacity(513, 0).is_err());
        assert!(check_capacity(1, 32 * 1024 + 1).is_err());
        Ok(())
    }

    #[test]
    fn cleanup_keeps_a_replacement_inode() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tunnels.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        let guard = SocketPath {
            inode: fs::symlink_metadata(&path)?.ino(),
            path: path.clone(),
        };
        fs::remove_file(&path)?;
        fs::write(&path, b"replacement")?;
        drop(guard);
        assert_eq!(fs::read(&path)?, b"replacement");
        drop(listener);
        Ok(())
    }

    #[tokio::test]
    async fn the_control_socket_peer_must_match_the_service_directory_owner() -> io::Result<()> {
        let (stream, _) = UnixStream::pair()?;
        let uid = stream.peer_cred()?.uid();
        service_peer(&stream, uid)?;
        assert!(
            service_peer(&stream, uid.wrapping_add(1))
                .is_err_and(|e| e.kind() == io::ErrorKind::PermissionDenied)
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_path_lookups_propagate_and_a_private_file_is_not_a_directory() -> io::Result<()>
    {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("file");
        fs::write(&file, b"keep")?;
        fs::set_permissions(&file, Permissions::from_mode(0o600))?;
        assert!(private_directory(&file).is_err());
        assert!(
            remove_stale_socket(&file.join("missing"))
                .is_err_and(|e| e.kind() == io::ErrorKind::NotADirectory)
        );
        let _lock = lock_directory(dir.path())?;
        // A cold lookup, even while a helper is receiving, stays a missing socket.
        fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
        assert!(
            adopt(dir.path())
                .await
                .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        );
        Ok(())
    }

    #[tokio::test]
    async fn expiry_closes_connections_already_sent_to_an_unconfirmed_consumer() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let dir = tempfile::tempdir()?;
            fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
            let (mut old, mut control) = UnixStream::pair()?;
            let (saved, mut peers) = pair(b"private state".to_vec())?;
            let (expire, expired) = tokio::sync::oneshot::channel();
            let serving = hold_until(&mut control, dir.path(), async {
                let _ = expired.await;
            });
            let stalled = async {
                send(&mut old, &[saved]).await?;
                assert_eq!(old.read_u8().await?, b'E');
                let mut successor = UnixStream::connect(dir.path().join("tunnels.sock")).await?;
                // Keep the duplicate descriptors and control connection open without acknowledging.
                let received = receive(&mut successor).await?.release();
                assert_eq!(received.len(), 1);
                let _ = expire.send(());
                for peer in &mut peers {
                    assert_eq!(peer.read(&mut [0]).await?, 0);
                }
                drop((successor, received));
                Ok::<_, io::Error>(())
            };
            let (held, received) = tokio::join!(serving, stalled);
            received?;
            assert!(held.is_err_and(|e| e.kind() == io::ErrorKind::TimedOut));
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn adoption_discards_received_descriptors_if_confirmation_is_missing() -> io::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let dir = tempfile::tempdir()?;
            fs::set_permissions(dir.path(), Permissions::from_mode(0o700))?;
            let listener = UnixListener::bind(dir.path().join("tunnels.sock"))?;
            let (record, mut peers) = pair(b"partial transaction".to_vec())?;
            let sending = async {
                let (mut stream, _) = listener.accept().await?;
                send(&mut stream, &[record]).await?;
                assert_eq!(stream.read_u8().await?, b'A');
                // Lose the helper after acknowledgement, before its final confirmation.
                Ok::<_, io::Error>(())
            };
            let (sent, adopted) = tokio::join!(sending, adopt(dir.path()));
            sent?;
            assert!(adopted.is_err_and(|e| e.kind() == io::ErrorKind::UnexpectedEof));
            for peer in &mut peers {
                assert_eq!(peer.read(&mut [0]).await?, 0);
            }
            Ok(())
        })
        .await?
    }
}
