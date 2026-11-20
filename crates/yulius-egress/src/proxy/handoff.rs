//! Linux restart handoff for established opaque agent tunnels (SPEC-577).
use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_sandbox::relay::custodian::{Pair, monotonic_ms};

use super::{
    Arc, Bytes, CONNECTION_MAX, CopyState, EgressProxy, PoisonError, Slot, SocketAddr, TcpStream,
    TokioIo, Tracked, Upgraded, oneshot,
};

pub(super) type Capture = oneshot::Sender<oneshot::Sender<Pair>>;

#[derive(Serialize, Deserialize)]
struct Ownership {
    sandbox: String,
    run: String,
    last: u64,
    expires: u64,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid opaque tunnel handoff")
}

fn remaining(expires: u64, now: u64) -> io::Result<Duration> {
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
    let (_, control) = table.controls_mut().find(|(id, _)| *id == slot.id)?;
    let (capture, request) = oneshot::channel();
    control.capture = Some(capture);
    Some(request)
}

struct Tunnel {
    client: TcpStream,
    upstream: TcpStream,
    copy: CopyState,
    run: String,
    expires: u64,
}

impl Tunnel {
    async fn transfer(
        mut self,
        slot: Slot,
        request: oneshot::Receiver<oneshot::Sender<Pair>>,
    ) -> io::Result<()> {
        let mut upstream = Tracked::new(self.upstream, &slot);
        let timeout = remaining(self.expires, monotonic_ms())?;
        let reply = tokio::select! {
            result = self.copy.transfer(&mut self.client, &mut upstream) => return result,
            () = tokio::time::sleep(timeout) => return Ok(()),
            reply = request => match reply { Ok(reply) => reply, Err(_) => return Ok(()) },
        };
        let owner = Ownership {
            sandbox: slot.sandbox.clone(),
            run: self.run,
            last: slot.activity.last(),
            expires: self.expires,
        };
        let mut state = b"OP1\n".to_vec();
        serde_json::to_writer(&mut state, &owner)?;
        state.push(b'\n');
        self.copy.encode(&mut state)?;
        let pair = Pair {
            sockets: [
                self.client.into_std()?.into(),
                upstream.inner.into_std()?.into(),
            ],
            state,
        };
        let _ = reply.send(pair);
        Ok(())
    }
}

// Axum's auto server wraps the IO. The standalone HTTP/1 server passes it to Hyper directly.
fn tcp_upgrade(upgraded: Upgraded) -> Result<(TcpStream, Bytes), Upgraded> {
    match hyper_util::server::conn::auto::upgrade::downcast::<TokioIo<TcpStream>>(upgraded) {
        Ok(parts) => Ok((parts.io.into_inner(), parts.read_buf)),
        Err(upgraded) => upgraded
            .downcast::<TokioIo<TcpStream>>()
            .map(|parts| (parts.io.into_inner(), parts.read_buf)),
    }
}

pub(super) async fn agent_tunnel(
    addrs: Vec<SocketAddr>,
    upgraded: Upgraded,
    slot: Slot,
    run: &str,
) {
    let (mut client, prefix) = match tcp_upgrade(upgraded) {
        Ok(parts) => parts,
        Err(upgraded) => return super::tunnel(addrs, upgraded, slot).await,
    };
    let Some(upstream) = super::connect_tunnel(&addrs).await else {
        return;
    };
    let expires = monotonic_ms()
        .saturating_add(u64::try_from(CONNECTION_MAX.as_millis()).unwrap_or(u64::MAX));
    let mut upstream = Tracked::new(upstream, &slot);
    let Ok(Ok(copy)) = tokio::time::timeout(
        CONNECTION_MAX,
        CopyState::with_prefix(&mut client, &mut upstream, &prefix),
    )
    .await
    else {
        return;
    };
    drop(prefix);
    let Some(request) = register(&slot) else {
        return;
    };
    let tunnel = Tunnel {
        client,
        upstream: upstream.inner,
        copy,
        run: run.to_owned(),
        expires,
    };
    let _ = tunnel.transfer(slot, request).await;
}

impl EgressProxy {
    /// Pause the established opaque TCP copies in admission order, after stopping new requests.
    /// Eligible tasks yield their sockets and pending bytes. Closing or evicted copies yield none.
    ///
    /// # Errors
    /// Returns a timeout if the copies do not yield within the shutdown bound.
    pub async fn take_opaque_tunnels(&self) -> io::Result<Vec<Pair>> {
        let replies: Vec<_> = self
            .slots
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .controls_mut()
            .filter_map(|(_, control)| {
                let capture = control.capture.take()?;
                let (reply, receive) = oneshot::channel();
                capture.send(reply).ok()?;
                Some(receive)
            })
            .collect();
        let results =
            tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(replies))
                .await?;
        Ok(results.into_iter().filter_map(Result::ok).collect())
    }

    /// Reattach one established TCP copy before opening the replacement's HTTP listeners.
    /// Uses the existing connected sockets and original idle and lifetime clocks.
    ///
    /// # Errors
    /// Invalid, expired or over-capacity records release their sockets and consume no place.
    pub fn restore_opaque_tunnel(&self, pair: Pair) -> io::Result<()> {
        let state = pair.state.strip_prefix(b"OP1\n").ok_or_else(invalid)?;
        let at = state.iter().position(|b| *b == b'\n').ok_or_else(invalid)?;
        let owner: Ownership = serde_json::from_slice(&state[..at])?;
        remaining(owner.expires, monotonic_ms())?;
        if owner.last > monotonic_ms() {
            return Err(invalid());
        }
        let copy = CopyState::decode(&state[at + 1..])?;
        let [client, upstream] = pair.sockets;
        let tcp = |fd| {
            let socket = std::net::TcpStream::from(fd);
            socket.peer_addr()?;
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
        let admitted = self
            .slots
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .restore_by(
                owner.sandbox.clone(),
                owner.run.clone(),
                control,
                owner.last,
            )
            .map_err(io::Error::other)?;
        let slot = Slot {
            slots: Arc::clone(&self.slots),
            id: admitted.id,
            activity: admitted.activity,
            sandbox: owner.sandbox,
        };
        let tunnel = Tunnel {
            client,
            upstream,
            copy,
            run: owner.run,
            expires: owner.expires,
        };
        tokio::spawn(async move {
            tokio::select! { _ = tunnel.transfer(slot, request) => {}, _ = ended => {} }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_lifetime_bounds_both_sides_of_a_restart() -> io::Result<()> {
        let now = 10_000;
        let longest = now + u64::try_from(CONNECTION_MAX.as_millis()).map_err(io::Error::other)?;
        assert_eq!(remaining(longest, now)?, CONNECTION_MAX);
        assert_eq!(remaining(now + 1, now)?, Duration::from_millis(1));
        for deadline in [now - 1, now, longest + 1] {
            assert!(remaining(deadline, now).is_err());
        }
        Ok(())
    }
}
