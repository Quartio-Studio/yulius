//! Checkpoint only the outbound TCP leg identified by an established agent proxy connection.
use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsFd as _, OwnedFd};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rustix::net::{AddressFamily, sockopt};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use yulius_contracts::sandbox::HolderRecord;
use yulius_domain::connection_health::ConnectionHealth;

use super::custodian::{Established, Pair, monotonic_ms};
use super::{
    Control, CopyState, IDLE_FLOOR, MAX_CONNECTIONS, Place, Places, Report, Tracked, outbound_loop,
};

mod drain;
pub use drain::{CopyCancellation, CopyDrain};

/// The connected TCP leg between a holder's outbound relay and the internal proxy.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TcpLink {
    relay: SocketAddr,
    proxy: SocketAddr,
}

impl TcpLink {
    /// Inspect the original accepted client descriptor on the proxy side.
    ///
    /// # Errors
    /// The descriptor is not a connected TCP endpoint.
    pub fn at_proxy(fd: &impl std::os::fd::AsFd) -> io::Result<Self> {
        let tcp = std::net::TcpStream::from(fd.as_fd().try_clone_to_owned()?);
        Ok(Self {
            relay: tcp.peer_addr()?,
            proxy: tcp.local_addr()?,
        })
    }

    /// Kernel identity of the outbound relay's original internal TCP socket.
    ///
    /// # Errors
    /// The kernel could not identify the descriptor.
    pub fn relay_cookie(fd: &impl std::os::fd::AsFd) -> io::Result<u64> {
        Ok(sockopt::socket_cookie(fd)?)
    }

    /// Inspect the original upstream descriptor of a serialized outbound relay.
    ///
    /// # Errors
    /// The descriptor is not a connected TCP endpoint.
    pub fn at_relay(pair: &Pair) -> io::Result<Self> {
        let (owner, _) = Owner::decode(&pair.state)?;
        let link = owner.link;
        Self::verify_endpoint(pair, &pair.sockets[1], link.relay, link.proxy)?;
        Ok(link)
    }

    /// Verify the proxy's original internal connection, including any terminal read queue.
    ///
    /// # Errors
    /// A different address, unverified terminal socket, or transport error rejects the pair.
    pub fn verify_proxy(self, pair: &Pair) -> io::Result<Self> {
        Self::verify_endpoint(pair, &pair.sockets[0], self.proxy, self.relay)?;
        Ok(self)
    }

    fn verify_endpoint(
        pair: &Pair,
        fd: &OwnedFd,
        local: SocketAddr,
        peer: SocketAddr,
    ) -> io::Result<()> {
        let tcp = std::net::TcpStream::from(fd.as_fd().try_clone_to_owned()?);
        if tcp.local_addr()? != local {
            return Err(invalid());
        }
        match tcp.peer_addr() {
            Ok(observed) if observed == peer => Ok(()),
            Ok(_) => Err(invalid()),
            Err(error)
                if error.kind() == io::ErrorKind::NotConnected
                    && pair.established.is_some()
                    && pair.resumable()? =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

// The probe is a duplicate of the actual internal socket. Drop it as soon as CONNECT enrolls
// the relay, or with the admission place if this is an ordinary HTTP or intercepted connection.
pub(super) struct Enrollment {
    cookie: u64,
    available: Option<(std::net::TcpStream, CopyDrain)>,
}

pub(super) type Capture = oneshot::Sender<(u64, oneshot::Sender<Pair>)>;
type Request = oneshot::Receiver<(u64, oneshot::Sender<Pair>)>;

#[derive(Serialize, Deserialize)]
struct Owner {
    link: TcpLink,
    holder: HolderRecord,
    order: u64,
    last: u64,
}

impl Owner {
    fn decode(state: &[u8]) -> io::Result<(Self, &[u8])> {
        let bytes = state.strip_prefix(b"RL1\n").ok_or_else(invalid)?;
        let at = bytes.iter().position(|b| *b == b'\n').ok_or_else(invalid)?;
        Ok((serde_json::from_slice(&bytes[..at])?, &bytes[at + 1..]))
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid outbound relay handoff")
}

struct Flow {
    link: TcpLink,
    client: UnixStream,
    upstream: TcpStream,
    copy: CopyState,
    established: Option<Established>,
}

impl Flow {
    async fn run(
        mut self,
        place: Place,
        holder: &HolderRecord,
        expires: Option<u64>,
        request: Request,
        drain: CopyDrain,
    ) -> io::Result<()> {
        let mut client = Tracked {
            inner: self.client,
            place,
        };
        let lifetime = async {
            if let Some(end) = expires {
                tokio::time::sleep(Duration::from_millis(end.saturating_sub(monotonic_ms()))).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let (expires, reply) = tokio::select! {
            biased;
            ()=lifetime => return Err(io::ErrorKind::TimedOut.into()),
            request=request => request.map_err(|_| io::ErrorKind::BrokenPipe)?,
            result=async {
                self.copy.transfer(&mut client, &mut self.upstream).await?;
                drain.finish().await;
                Ok(())
            } => return result,
        };
        let owner = Owner {
            link: self.link,
            holder: holder.clone(),
            order: client.place.id,
            last: client.place.activity.last(),
        };
        let mut state = b"RL1\n".to_vec();
        serde_json::to_writer(&mut state, &owner)?;
        state.push(b'\n');
        self.copy.encode(&mut state)?;
        reply
            .send(Pair {
                sockets: [
                    client.inner.into_std()?.into(),
                    self.upstream.into_std()?.into(),
                ],
                state,
                expires: Some(expires),
                established: self.established,
            })
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}

// Register after the real TCP connect, before copying the first payload byte. The place's
// eviction sender owns this registration too, so a removed connection can never be captured.
pub(super) async fn transfer(
    client: Tracked<UnixStream>,
    upstream: TcpStream,
    copy: CopyState,
    holder: &HolderRecord,
    expires: Option<u64>,
) -> io::Result<()> {
    let link = TcpLink {
        relay: upstream.local_addr()?,
        proxy: upstream.peer_addr()?,
    };
    let (capture, request) = oneshot::channel();
    let cancellation = CopyCancellation::unclaimed();
    let drain = cancellation.drain();
    let enrollment = upstream
        .as_fd()
        .try_clone_to_owned()
        .ok()
        .and_then(|probe| {
            Some(Enrollment {
                cookie: TcpLink::relay_cookie(&upstream).ok()?,
                available: Some((probe.into(), drain.clone())),
            })
        });
    {
        let mut slots = client
            .place
            .places
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slots.stopped {
            return Ok(());
        }
        let Some((_, control)) = slots
            .connections
            .controls_mut()
            .find(|(id, _)| *id == client.place.id)
        else {
            return Ok(());
        };
        control.capture = Some((link, capture));
        control.enrollment = enrollment;
    }
    let established = Established::connected(&client.inner, &upstream).ok();
    let flow = Flow {
        link,
        established,
        client: client.inner,
        upstream,
        copy,
    };
    cancellation
        .run_copy(flow.run(client.place, holder, expires, request, drain))
        .await
}

struct Saved {
    flow: Flow,
    cookie: u64,
    expires: u64,
    owner: Owner,
}

impl Saved {
    fn decode(pair: Pair, holder: &HolderRecord, link: TcpLink) -> io::Result<Self> {
        if !pair.resumable()?
            || TcpLink::at_relay(&pair)? != link
            || sockopt::socket_domain(&pair.sockets[0])? != AddressFamily::UNIX
        {
            return Err(invalid());
        }
        let (mut owner, copy_state) = Owner::decode(&pair.state)?;
        // Upkeep can replace slirp4netns without replacing this holder or its outbound relay.
        owner.holder.slirp_pid = holder.slirp_pid;
        let now = monotonic_ms();
        let expires = pair.expires.ok_or_else(invalid)?;
        if owner.holder != *holder
            || !crate::holder::alive(holder)
            || owner.last > now
            || expires <= now
        {
            return Err(invalid());
        }
        let copy = CopyState::decode(copy_state)?;
        let cookie = TcpLink::relay_cookie(&pair.sockets[1])?;
        let [left, right] = pair.sockets;
        let left = std::os::unix::net::UnixStream::from(left);
        let right = std::net::TcpStream::from(right);
        left.set_nonblocking(true)?;
        right.set_nonblocking(true)?;
        let flow = Flow {
            link,
            established: pair.established,
            client: UnixStream::from_std(left)?,
            upstream: TcpStream::from_std(right)?,
            copy,
        };
        Ok(Self {
            flow,
            cookie,
            expires,
            owner,
        })
    }
}

struct Resume {
    cancellation: CopyCancellation,
    flow: Flow,
    expires: u64,
    place: Place,
    request: Request,
    evicted: oneshot::Receiver<()>,
}

struct Startup {
    saved: Vec<(Saved, CopyCancellation)>,
    ready: oneshot::Sender<Vec<Resume>>,
}

struct Outbound {
    holder: Arc<HolderRecord>,
    places: Arc<Places>,
    startup: Mutex<Option<Startup>>,
}

impl Outbound {
    fn begin(&self) {
        let Some(mut startup) = self
            .startup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        startup.saved.sort_by_key(|(saved, _)| saved.owner.order);
        let mut slots = self
            .places
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut copies = Vec::with_capacity(startup.saved.len());
        for (saved, cancellation) in startup.saved {
            if saved.expires <= monotonic_ms() {
                continue;
            }
            let (end, evicted) = oneshot::channel();
            let (capture, request) = oneshot::channel();
            let control = Control {
                enrollment: Some(Enrollment {
                    cookie: saved.cookie,
                    available: None,
                }),
                _end: end,
                capture: Some((saved.flow.link, capture)),
            };
            if let Ok(admitted) = slots
                .connections
                .restore_by((), (), control, saved.owner.last)
            {
                let place = Place {
                    places: Arc::clone(&self.places),
                    id: admitted.id,
                    activity: admitted.activity,
                };
                copies.push(Resume {
                    cancellation,
                    flow: saved.flow,
                    expires: saved.expires,
                    place,
                    request,
                    evicted,
                });
            }
        }
        drop(slots);
        let _ = startup.ready.send(copies);
    }
}

#[derive(Default, PartialEq, Eq)]
enum Phase {
    Restoring,
    #[default]
    Serving,
    Stopping,
}

#[derive(Default)]
struct Holders {
    phase: Phase,
    relays: BTreeMap<String, Arc<Outbound>>,
}

/// Active host-side outbound relays. Each registered holder owns at most its normal number
/// of places, including the sockets reserved for restoration before its first accept.
#[derive(Default)]
pub struct OutboundRelays(Mutex<Holders>);

struct Registration {
    relays: Arc<OutboundRelays>,
    sandbox: String,
    outbound: Arc<Outbound>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut holders = self.relays.0.lock().unwrap_or_else(PoisonError::into_inner);
        if holders
            .relays
            .get(&self.sandbox)
            .is_some_and(|r| Arc::ptr_eq(r, &self.outbound))
        {
            holders.relays.remove(&self.sandbox);
        }
    }
}

impl OutboundRelays {
    /// Defer accepts while the starting manager discovers the old holders.
    pub fn defer(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).phase = Phase::Restoring;
    }

    /// Account for every restored copy in its old admission order before serving new clients.
    pub fn begin(&self) {
        let mut holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for relay in holders.relays.values() {
            relay.begin();
        }
        holders.phase = Phase::Serving;
    }

    /// Prevent new admissions from evicting a relay being matched to its opaque tunnel.
    pub fn stop_admissions(&self) {
        let mut holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        holders.phase = Phase::Stopping;
        for relay in holders.relays.values() {
            relay
                .places
                .table
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stopped = true;
        }
    }

    /// Bind an authorized CONNECT to its original outbound socket while both TCP ends
    /// still report one another as live peers. A terminal socket cannot enroll a new copy.
    pub fn enroll(&self, sandbox: &str, client: &TcpStream) -> Option<(u64, CopyCancellation)> {
        let link = TcpLink::at_proxy(client).ok()?;
        let holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let relay = holders.relays.get(sandbox)?;
        let mut slots = relay
            .places
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut candidates = slots.connections.controls_mut().filter_map(|(_, control)| {
            if control.capture.as_ref()?.0 != link {
                return None;
            }
            let enrollment = control.enrollment.as_mut()?;
            let (probe, _) = enrollment.available.as_ref()?;
            (probe.local_addr().ok()? == link.relay && probe.peer_addr().ok()? == link.proxy)
                .then_some(enrollment)
        });
        let original = candidates.next()?;
        if candidates.next().is_some() || TcpLink::at_proxy(client).ok()? != link {
            return None;
        }
        let (_, drain) = original.available.as_ref()?;
        let cancellation = drain.enroll()?;
        original.available.take();
        Some((original.cookie, cancellation))
    }

    /// Request the uniquely matching original relay still admitted for this sandbox.
    pub fn capture(
        &self,
        sandbox: &str,
        link: TcpLink,
        expires: u64,
    ) -> Option<oneshot::Receiver<Pair>> {
        self.capture_matching(sandbox, link, None, expires)
    }

    /// Pause the exact kernel socket enrolled by this opaque CONNECT.
    pub fn capture_enrolled(
        &self,
        sandbox: &str,
        link: TcpLink,
        cookie: u64,
        expires: u64,
    ) -> Option<oneshot::Receiver<Pair>> {
        self.capture_matching(sandbox, link, Some(cookie), expires)
    }

    fn capture_matching(
        &self,
        sandbox: &str,
        link: TcpLink,
        cookie: Option<u64>,
        expires: u64,
    ) -> Option<oneshot::Receiver<Pair>> {
        let holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let relay = holders.relays.get(sandbox)?;
        let mut slots = relay
            .places
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut matches = slots.connections.controls_mut().filter(|(_, c)| {
            c.capture.as_ref().is_some_and(|(k, _)| *k == link)
                && cookie.is_none_or(|id| {
                    c.enrollment
                        .as_ref()
                        .is_some_and(|e| e.cookie == id && e.available.is_none())
                })
        });
        let (_, control) = matches.next()?;
        // A terminal TCP tuple can be reused while the old Unix client is still draining.
        if matches.next().is_some() {
            return None;
        }
        let (_, capture) = control.capture.take()?;
        let (send, receive) = oneshot::channel();
        capture.send((expires, send)).ok()?;
        Some(receive)
    }

    /// Admit the associated opaque tunnel only once its original holder, relay sockets, clock
    /// and place can be restored. A failed admission releases the held relay sockets.
    ///
    /// # Errors
    /// Invalid records, a missing holder or lost capacity reject this pair.
    pub fn restore(
        &self,
        sandbox: &str,
        link: TcpLink,
        pair: Pair,
        cancellation: CopyCancellation,
        admit_proxy: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let outbound = holders.relays.get(sandbox).ok_or_else(invalid)?;
        let saved = Saved::decode(pair, &outbound.holder, link)?;
        let mut startup = outbound
            .startup
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let startup = startup.as_mut().ok_or_else(invalid)?;
        if startup.saved.len() >= outbound.places.max
            || startup
                .saved
                .iter()
                .any(|(s, _)| s.owner.order == saved.owner.order || s.cookie == saved.cookie)
        {
            return Err(invalid());
        }
        admit_proxy()?;
        startup.saved.push((saved, cancellation));
        Ok(())
    }

    pub(crate) fn serve(
        self: &Arc<Self>,
        listener: UnixListener,
        targets: Arc<BTreeMap<u16, SocketAddr>>,
        sandbox: String,
        holder: HolderRecord,
        health: Arc<ConnectionHealth>,
    ) -> impl Future<Output = ()> + Send + 'static {
        let report = Report {
            health,
            sandbox: sandbox.clone(),
            #[cfg(test)]
            pause: None,
        };
        let places = Places::reporting(MAX_CONNECTIONS, IDLE_FLOOR, Some(report));
        let (ready, receive) = oneshot::channel();
        let holder = Arc::new(holder);
        let outbound = Arc::new(Outbound {
            holder: Arc::clone(&holder),
            places: Arc::clone(&places),
            startup: Mutex::new(Some(Startup {
                saved: Vec::new(),
                ready,
            })),
        });
        let registration = Registration {
            relays: Arc::clone(self),
            sandbox: sandbox.clone(),
            outbound: Arc::clone(&outbound),
        };
        {
            let mut holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if holders.phase == Phase::Stopping {
                places
                    .table
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stopped = true;
            }
            if holders.phase != Phase::Restoring {
                outbound.begin();
            }
            holders.relays.insert(sandbox.clone(), outbound);
        }
        async move {
            let _registration = registration;
            let Ok(restored) = receive.await else {
                return;
            };
            let mut copies = JoinSet::new();
            for resume in restored {
                let holder = Arc::clone(&holder);
                copies.spawn(async move {
                    let drain = resume.cancellation.drain();
                    resume
                        .cancellation
                        .run(
                            resume.evicted,
                            resume.flow.run(
                                resume.place,
                                &holder,
                                Some(resume.expires),
                                resume.request,
                                drain,
                            ),
                        )
                        .await;
                });
            }
            outbound_loop(listener, targets, sandbox, places, copies, Some(holder)).await;
        }
    }
}

#[cfg(test)]
mod tests;
