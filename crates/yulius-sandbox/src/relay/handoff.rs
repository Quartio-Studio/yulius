//! Checkpoint only the outbound TCP leg identified by an established agent proxy connection.
use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsFd as _, OwnedFd};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use yulius_contracts::sandbox::HolderRecord;
use yulius_domain::connection_health::ConnectionHealth;

use super::custodian::{Established, Pair};
use super::{CopyState, IDLE_FLOOR, MAX_CONNECTIONS, Places, Report, Tracked, outbound_loop};

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

pub(super) type Capture = oneshot::Sender<(u64, oneshot::Sender<Pair>)>;

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

// Register after the real TCP connect, before copying the first payload byte. The place's
// eviction sender owns this registration too, so a removed connection can never be captured.
pub(super) async fn transfer(
    mut client: Tracked<UnixStream>,
    mut upstream: TcpStream,
    holder: &HolderRecord,
) -> io::Result<()> {
    let link = TcpLink {
        relay: upstream.local_addr()?,
        proxy: upstream.peer_addr()?,
    };
    let (capture, request) = oneshot::channel();
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
    }
    let established = Established::connected(&client.inner, &upstream).ok();
    let mut copy = CopyState::default();
    let (expires, reply) = tokio::select! {
        biased;
        request = request => match request { Ok(request) => request, Err(_) => return Ok(()) },
        result = copy.transfer(&mut client, &mut upstream) => return result,
    };
    let owner = Owner {
        link,
        holder: holder.clone(),
        order: client.place.id,
        last: client.place.activity.last(),
    };
    let mut state = b"RL1\n".to_vec();
    serde_json::to_writer(&mut state, &owner)?;
    state.push(b'\n');
    copy.encode(&mut state)?;
    let _ = reply.send(Pair {
        sockets: [client.inner.into_std()?.into(), upstream.into_std()?.into()],
        state,
        expires: Some(expires),
        established,
    });
    Ok(())
}

#[derive(Default)]
struct Holders {
    stopping: bool,
    relays: BTreeMap<String, Arc<Places>>,
}

/// Active outbound relays and their existing admission tables, keyed by sandbox.
#[derive(Default)]
pub struct OutboundRelays(Mutex<Holders>);

struct Registration {
    hub: Arc<OutboundRelays>,
    sandbox: String,
    places: Arc<Places>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut holders = self.hub.0.lock().unwrap_or_else(PoisonError::into_inner);
        if holders
            .relays
            .get(&self.sandbox)
            .is_some_and(|p| Arc::ptr_eq(p, &self.places))
        {
            holders.relays.remove(&self.sandbox);
        }
    }
}

impl OutboundRelays {
    /// Quiesce active and subsequently registered relays before HTTP shutdown begins.
    pub fn stop_admissions(&self) {
        let mut holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        holders.stopping = true;
        for places in holders.relays.values() {
            places
                .table
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stopped = true;
        }
    }

    /// Request the matching original relay, if it is still admitted for this sandbox.
    pub fn capture(
        &self,
        sandbox: &str,
        link: TcpLink,
        expires: u64,
    ) -> Option<oneshot::Receiver<Pair>> {
        let holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let places = holders.relays.get(sandbox)?;
        let mut slots = places.table.lock().unwrap_or_else(PoisonError::into_inner);
        let (_, control) = slots
            .connections
            .controls_mut()
            .find(|(_, c)| c.capture.as_ref().is_some_and(|(k, _)| *k == link))?;
        let (_, capture) = control.capture.take()?;
        let (send, receive) = oneshot::channel();
        capture.send((expires, send)).ok()?;
        Some(receive)
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
        let registration = Registration {
            hub: Arc::clone(self),
            sandbox: sandbox.clone(),
            places: Arc::clone(&places),
        };
        {
            let mut holders = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if holders.stopping {
                places
                    .table
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stopped = true;
            }
            holders.relays.insert(sandbox.clone(), Arc::clone(&places));
        }
        async move {
            let _registration = registration;
            outbound_loop(
                listener,
                targets,
                sandbox,
                places,
                JoinSet::new(),
                Some(Arc::new(holder)),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests;
