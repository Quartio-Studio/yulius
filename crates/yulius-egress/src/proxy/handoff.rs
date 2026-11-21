//! Linux restart handoff for established opaque agent tunnels (SPEC-577).
use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_sandbox::relay::custodian::{Established, Pair, collect, monotonic_ms};
use yulius_sandbox::relay::handoff::{CopyCancellation, CopyDrain, OutboundRelays, TcpLink};

use super::{
    Arc, Bytes, CONNECTION_MAX, CopyState, EgressProxy, PoisonError, Slot, SocketAddr, TcpStream,
    TokioIo, Tracked, Upgraded, oneshot,
};

pub(super) type Capture = oneshot::Sender<oneshot::Sender<Pair>>;

#[derive(Serialize, Deserialize)]
struct Ownership {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<TcpLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay_cookie: Option<u64>,
    sandbox: String,
    run: String,
    last: u64,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid opaque tunnel handoff")
}

fn tunnel_state(pair: &Pair) -> io::Result<(Ownership, &[u8])> {
    let bytes = pair.state.strip_prefix(b"OP1\n").ok_or_else(invalid)?;
    let at = bytes.iter().position(|b| *b == b'\n').ok_or_else(invalid)?;
    Ok((serde_json::from_slice(&bytes[..at])?, &bytes[at + 1..]))
}

fn remaining(last: u64, expires: u64, now: u64) -> io::Result<Duration> {
    if last > now {
        return Err(invalid());
    }
    let millis = expires
        .checked_sub(now)
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
    let remaining = Duration::from_millis(millis);
    if remaining.is_zero() || remaining > CONNECTION_MAX {
        return Err(invalid());
    }
    Ok(remaining)
}

fn register(slot: &Slot) -> Option<oneshot::Receiver<oneshot::Sender<Pair>>> {
    let mut table = slot
        .slots
        .table
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if table.stopped {
        return None;
    }
    let (_, control) = table
        .connections
        .controls_mut()
        .find(|(id, _)| *id == slot.id)?;
    let (capture, request) = oneshot::channel();
    control.capture = Some(capture);
    Some(request)
}

struct Tunnel {
    link: Option<TcpLink>,
    relay_cookie: Option<u64>,
    client: TcpStream,
    upstream: TcpStream,
    copy: CopyState,
    established: Option<Established>,
    run: String,
    expires: u64,
}

impl Tunnel {
    async fn transfer(
        mut self,
        slot: Slot,
        request: oneshot::Receiver<oneshot::Sender<Pair>>,
        drain: CopyDrain,
    ) -> io::Result<()> {
        let mut upstream = Tracked::new(self.upstream, &slot);
        let timeout = remaining(slot.activity.last(), self.expires, monotonic_ms())?;
        let reply = tokio::select! {
            result = async {
                self.copy.transfer(&mut self.client, &mut upstream).await?;
                drain.finish().await;
                Ok(())
            } => return result,
            () = tokio::time::sleep(timeout) => return Err(io::ErrorKind::TimedOut.into()),
            reply = request => reply.map_err(|_| io::ErrorKind::BrokenPipe)?,
        };
        let owner = Ownership {
            link: self.link,
            relay_cookie: self.relay_cookie,
            sandbox: slot.sandbox.clone(),
            run: self.run,
            last: slot.activity.last(),
        };
        let mut state = b"OP1\n".to_vec();
        serde_json::to_writer(&mut state, &owner)?;
        state.push(b'\n');
        self.copy.encode(&mut state)?;
        let pair = Pair {
            expires: Some(self.expires),
            established: self.established,
            sockets: [
                self.client.into_std()?.into(),
                upstream.inner.into_std()?.into(),
            ],
            state,
        };
        reply
            .send(pair)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}

// Axum's auto server wraps the IO. The standalone HTTP/1 server passes it to Hyper directly.
fn tcp_upgrade(upgraded: Upgraded) -> Result<(TcpStream, Bytes), Upgraded> {
    match hyper_util::server::conn::auto::upgrade::downcast::<TokioIo<crate::HttpInput>>(upgraded) {
        Ok(parts) => Ok((parts.io.into_inner().0, parts.read_buf)),
        Err(upgraded) => upgraded
            .downcast::<TokioIo<crate::HttpInput>>()
            .map(|parts| (parts.io.into_inner().0, parts.read_buf)),
    }
}

pub(super) async fn agent_tunnel(
    addrs: Vec<SocketAddr>,
    upgraded: Upgraded,
    slot: Slot,
    run: &str,
    relays: Option<&OutboundRelays>,
) {
    let (client, prefix) = match tcp_upgrade(upgraded) {
        Ok(parts) => parts,
        Err(upgraded) => return super::tunnel(addrs, upgraded, slot).await,
    };
    let Some(upstream) = super::connect_tunnel(&addrs).await else {
        return;
    };
    let expires = monotonic_ms()
        .saturating_add(u64::try_from(CONNECTION_MAX.as_millis()).unwrap_or(u64::MAX));
    let Ok(copy) = CopyState::from_prefix(&prefix) else {
        return;
    };
    drop(prefix);
    let Some(request) = register(&slot) else {
        return;
    };
    let established = Established::connected(&client, &upstream).ok();
    let (relay_cookie, cancellation) = match relays.and_then(|r| r.enroll(&slot.sandbox, &client)) {
        Some((cookie, cancellation)) => (Some(cookie), cancellation),
        None => (None, CopyCancellation::default()),
    };
    let drain = cancellation.drain();
    let tunnel = Tunnel {
        link: TcpLink::at_proxy(&client).ok(),
        relay_cookie,
        established,
        client,
        upstream,
        copy,
        run: run.to_owned(),
        expires,
    };
    let _ = cancellation
        .run_copy(tunnel.transfer(slot, request, drain))
        .await;
}

impl EgressProxy {
    /// Bind the live namespace manager before accepting CONNECT requests.
    pub fn bind_holder_relays(&self, relays: Arc<OutboundRelays>) {
        let _ = self.holder_relays.set(relays);
    }

    /// Identify the original relay enrolled by an opaque connection.
    ///
    /// # Errors
    /// This record has no live enrollment or no longer identifies its original socket.
    pub fn enrolled_relay(pair: &Pair) -> io::Result<(String, TcpLink, u64)> {
        let (owner, _) = tunnel_state(pair)?;
        let cookie = owner.relay_cookie.ok_or_else(invalid)?;
        let link = owner.link.ok_or_else(invalid)?.verify_proxy(pair)?;
        Ok((owner.sandbox, link, cookie))
    }

    /// Pause the established opaque TCP copies in admission order, after stopping new requests.
    /// Eligible tasks yield their sockets and pending bytes. Closing or evicted copies yield none.
    ///
    /// # Errors
    /// Returns a timeout if the copies do not yield within the shutdown bound.
    pub async fn take_opaque_tunnels(&self) -> io::Result<Vec<Pair>> {
        let replies: Vec<_> = {
            let mut admissions = self
                .slots
                .table
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            admissions.stopped = true;
            admissions
                .connections
                .controls_mut()
                .filter_map(|(_, control)| {
                    let capture = control.capture.take()?;
                    let (reply, receive) = oneshot::channel();
                    capture.send(reply).ok()?;
                    Some(receive)
                })
                .collect()
        };
        tokio::time::timeout(Duration::from_secs(5), collect(replies)).await?
    }

    /// Identify the authenticated sandbox and original accepted TCP leg of a captured tunnel.
    ///
    /// # Errors
    /// The record no longer represents a live opaque tunnel.
    pub fn relay_link(pair: &Pair) -> io::Result<(String, TcpLink)> {
        let (owner, _) = tunnel_state(pair)?;
        let link = match owner.link {
            Some(link) => link.verify_proxy(pair)?,
            None => TcpLink::at_proxy(&pair.sockets[0])?,
        };
        Ok((owner.sandbox, link))
    }

    /// Reattach one established TCP copy before opening the replacement's HTTP listeners.
    /// Uses the existing connected sockets and original idle and lifetime clocks.
    ///
    /// # Errors
    /// Invalid, expired or over-capacity records release their sockets and consume no place.
    pub fn restore_opaque_tunnel(&self, pair: Pair) -> io::Result<()> {
        self.restore_with_cancellation(pair, CopyCancellation::default())
    }

    /// Restore an opaque copy with the administrative cancellation of its matching holder relay.
    ///
    /// # Errors
    /// Invalid records, elapsed lifetimes and unavailable admission places reject the copy.
    pub fn restore_with_cancellation(
        &self,
        pair: Pair,
        cancellation: CopyCancellation,
    ) -> io::Result<()> {
        let (owner, copy_state) = tunnel_state(&pair)?;
        let expires = pair.expires.ok_or_else(invalid)?;
        remaining(owner.last, expires, monotonic_ms())?;
        let copy = CopyState::decode(copy_state)?;
        if !pair.resumable()? {
            return Err(io::ErrorKind::NotConnected.into());
        }
        let established = pair
            .established
            .or_else(|| Established::connected(&pair.sockets[0], &pair.sockets[1]).ok());
        let [client, upstream] = pair.sockets;
        let tcp = |fd| {
            let socket = std::net::TcpStream::from(fd);
            socket.local_addr()?; // Reject non-IP sockets, including a connected Unix stream.
            socket.set_nonblocking(true)?;
            TcpStream::from_std(socket)
        };
        let client = tcp(client)?;
        let upstream = tcp(upstream)?;
        let (end, ended) = oneshot::channel();
        let (capture, request) = oneshot::channel();
        let control = super::Control {
            _end: end,
            capture: Some(capture),
        };
        let admitted = {
            let mut admissions = self
                .slots
                .table
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if admissions.stopped {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            admissions
                .connections
                .restore_by(
                    owner.sandbox.clone(),
                    owner.run.clone(),
                    control,
                    owner.last,
                )
                .map_err(io::Error::other)?
        };
        let slot = Slot {
            slots: Arc::clone(&self.slots),
            id: admitted.id,
            activity: admitted.activity,
            sandbox: owner.sandbox,
        };
        let tunnel = Tunnel {
            link: owner.link.or_else(|| TcpLink::at_proxy(&client).ok()),
            relay_cookie: owner.relay_cookie,
            established,
            client,
            upstream,
            copy,
            run: owner.run,
            expires,
        };
        tokio::spawn(async move {
            let drain = cancellation.drain();
            cancellation
                .run(ended, tunnel.transfer(slot, request, drain))
                .await;
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_evicted_new_tunnel_releases_its_idle_relay_in_either_poll_order() -> io::Result<()>
    {
        use crate::proxy::{Connections, Slots};
        use tokio::io::AsyncReadExt as _;
        use tokio::net::{TcpListener, UnixStream};

        for (capture_polled, abandoned_reply) in [(false, false), (true, false), (true, true)] {
            let slots = Arc::new(Slots {
                table: std::sync::Mutex::new(Connections::with_owners(1, 1, 0).into()),
                health: Default::default(),
                share: 1,
                total: 1,
                pause: None,
            });
            let (end, ended) = oneshot::channel();
            let admitted = slots
                .admit("s1", "run", false, end)
                .ok_or_else(invalid)?
                .0
                .map_err(io::Error::other)?;
            let slot = Slot {
                slots: Arc::clone(&slots),
                id: admitted.id,
                activity: admitted.activity,
                sandbox: "s1".into(),
            };
            let request = register(&slot).ok_or_else(invalid)?;
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let mut internal = TcpStream::connect(listener.local_addr()?).await?;
            let (client, _) = listener.accept().await?;
            let upstream = TcpStream::connect(listener.local_addr()?).await?;
            let (mut origin, _) = listener.accept().await?;
            let (mut unix, mut agent) = UnixStream::pair()?;
            let (relay, proxy) = CopyCancellation::pair();
            let copying = tokio::spawn(async move {
                relay
                    .run_copy(async {
                        CopyState::default()
                            .transfer(&mut unix, &mut internal)
                            .await
                    })
                    .await
            });
            let tunnel = Tunnel {
                link: TcpLink::at_proxy(&client).ok(),
                relay_cookie: None,
                client,
                upstream,
                copy: CopyState::default(),
                established: None,
                run: "run".into(),
                expires: monotonic_ms() + 5000,
            };
            let drain = proxy.drain();
            let serving = proxy.run_copy(tunnel.transfer(slot, request, drain));
            // Removing the control closes both the eviction and checkpoint channels.
            {
                let mut admissions = slots.table.lock().unwrap_or_else(PoisonError::into_inner);
                if abandoned_reply {
                    let (reply, receiver) = oneshot::channel();
                    drop(receiver);
                    let (_, control) = admissions
                        .connections
                        .controls_mut()
                        .next()
                        .ok_or_else(invalid)?;
                    control
                        .capture
                        .take()
                        .ok_or_else(invalid)?
                        .send(reply)
                        .map_err(|_| invalid())?;
                }
                assert!(admissions.connections.close(admitted.id));
            }
            if capture_polled {
                let _ = serving.await;
            } else {
                let _ = ended.await;
                drop(serving);
            }
            // Both external write halves remain open. Ordinary FIN cannot end the relay copy.
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), origin.read(&mut [0])).await??,
                0
            );
            tokio::time::timeout(Duration::from_secs(2), copying)
                .await?
                .map_err(io::Error::other)??;
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), agent.read(&mut [0])).await??,
                0
            );
        }
        Ok(())
    }

    // HTTP/1 stops parsing at the end of the CONNECT headers. A read that straddles that
    // boundary must leave no more private bytes in Hyper than one copy direction can retain.
    #[tokio::test]
    async fn internal_tcp_reads_bound_the_prefix_while_excess_bytes_stay_in_the_kernel()
    -> io::Result<()> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut peer = TcpStream::connect(listener.local_addr()?).await?;
        let (socket, _) = listener.accept().await?;
        let mut input = crate::HttpInput(socket);
        let marker = vec![0x73; 32768];
        peer.write_all(&marker).await?;
        let mut buf = vec![0; marker.len()];
        let n = input.read(&mut buf).await?;
        assert!(n <= 8192, "HTTP upgrade prefetched {n} bytes");
        input.read_exact(&mut buf[n..]).await?;
        assert_eq!(buf, marker);
        // Bounding the read still exposes the TCP write operations Hyper uses.
        use tokio::io::AsyncWrite as _;
        assert!(input.is_write_vectored());
        let parts = [io::IoSlice::new(b"vec"), io::IoSlice::new(b"tor")];
        assert_eq!(input.write_vectored(&parts).await?, 6);
        input.flush().await?;
        input.shutdown().await?;
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), peer.read_to_end(&mut received)).await??;
        assert_eq!(received, b"vector");
        Ok(())
    }

    #[test]
    fn the_original_lifetime_bounds_both_sides_of_a_restart() -> io::Result<()> {
        let now = 10_000;
        let longest = now + u64::try_from(CONNECTION_MAX.as_millis()).map_err(io::Error::other)?;
        assert_eq!(remaining(now, longest, now)?, CONNECTION_MAX);
        assert_eq!(remaining(now - 1, now + 1, now)?, Duration::from_millis(1));
        assert!(remaining(now + 1, longest, now).is_err());
        for deadline in [now - 1, now, longest + 1] {
            assert!(remaining(now, deadline, now).is_err());
        }
        Ok(())
    }
}
