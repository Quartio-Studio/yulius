//! Exercise the actual outgoing relay, its table and the original socket endpoints.
use super::*;
use crate::holder::{boot_id, ns_inode};
use crate::relay::custodian::monotonic_ms;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use yulius_contracts::sandbox::{HOLDER_RECORD_VERSION, RELAY_OK};

fn holder() -> io::Result<HolderRecord> {
    let pid = std::process::id();
    let inode = |kind| ns_inode(pid, kind).ok_or_else(|| io::Error::other("fixture namespace"));
    Ok(HolderRecord {
        version: HOLDER_RECORD_VERSION,
        sandbox: "fixture".into(),
        base_id: "fixture-base".into(),
        inner_pid: pid,
        bwrap_pid: 0,
        slirp_pid: None,
        boot_id: boot_id(),
        user_ns: inode("user")?,
        net_ns: inode("net")?,
        mnt_ns: inode("mnt")?,
        started_at_ms: 1,
        system_root: "/fixture".into(),
        engine: false,
    })
}

#[tokio::test]
async fn the_same_registered_holder_captures_only_its_original_outbound_copy() -> io::Result<()> {
    check_captured_relay(false).await
}

#[tokio::test]
async fn a_half_closed_holder_drains_the_terminal_internal_receive_queue() -> io::Result<()> {
    check_captured_relay(true).await
}

async fn check_captured_relay(final_reply: bool) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let holder = holder()?;
        let hub = Arc::new(OutboundRelays::default());
        let dir = tempfile::tempdir()?;
        let address = dir.path().join("out.sock");
        let target = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = target.local_addr()?.port();
        let targets = Arc::new(BTreeMap::from([(port, target.local_addr()?)]));
        let health = Arc::new(ConnectionHealth::default());
        let serving = tokio::spawn(hub.serve(
            UnixListener::bind(&address)?,
            Arc::clone(&targets),
            holder.sandbox.clone(),
            holder.clone(),
            Arc::clone(&health),
        ));
        let started = monotonic_ms();
        let mut client = UnixStream::connect(&address).await?;
        client.write_u16(port).await?;
        assert_eq!(client.read_u8().await?, RELAY_OK);
        let (mut peer, _) = target.accept().await?;
        client.write_all(b"live").await?;
        assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"live");
        let link = TcpLink::at_proxy(&peer.as_fd().try_clone_to_owned()?)?;
        if final_reply {
            client.shutdown().await?;
            assert_eq!(peer.read(&mut [0]).await?, 0);
        }
        hub.stop_admissions();
        let expires = monotonic_ms() + 4_000;
        assert!(hub.capture("another-holder", link, expires).is_none());
        let mut pair = hub
            .capture(&holder.sandbox, link, expires)
            .ok_or_else(invalid)?
            .await
            .map_err(io::Error::other)?;
        assert!(hub.capture(&holder.sandbox, link, expires).is_none());
        serving.abort();
        assert!(serving.await.is_err_and(|e| e.is_cancelled()));
        peer.write_all(b"down").await?;
        // Even if one address still agrees, a reconnected internal leg is a different copy.
        let mut wrong_local = link.relay;
        wrong_local.set_port(0);
        let mut wrong_peer = link.proxy;
        wrong_peer.set_port(0);
        assert!(
            TcpLink::verify_endpoint(&pair, &pair.sockets[1], wrong_local, link.proxy).is_err()
        );
        assert!(TcpLink::verify_endpoint(&pair, &pair.sockets[1], link.relay, wrong_peer).is_err());
        if final_reply {
            peer.shutdown().await?;
            let retained = std::net::TcpStream::from(pair.sockets[1].as_fd().try_clone_to_owned()?);
            while retained.peer_addr().is_ok() {
                tokio::task::yield_now().await;
            }
            // A record without the original socket identities cannot claim a terminal queue.
            let established = pair.established.take();
            assert!(TcpLink::at_relay(&pair).is_err());
            pair.established = established;
            let deadline = pair.expires.take();
            assert!(TcpLink::at_relay(&pair).is_err());
            pair.expires = deadline;
        }
        assert!(TcpLink::at_relay(&pair)? == link);
        let (owner, saved) = Owner::decode(&pair.state)?;
        assert_eq!(owner.holder, holder);
        assert!((started..=monotonic_ms()).contains(&owner.last));
        assert_eq!(pair.expires, Some(expires));
        let mut copy = CopyState::decode(saved)?;
        let [left, right] = pair.sockets;
        let mut left = UnixStream::from_std(left.into())?;
        let mut right = TcpStream::from_std(right.into())?;
        let retained = tokio::spawn(async move { copy.transfer(&mut left, &mut right).await });
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"down");
        if !final_reply {
            client.write_all(b"next").await?;
            assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"next");
            client.shutdown().await?;
            assert_eq!(peer.read(&mut [0]).await?, 0);
            peer.shutdown().await?;
        }
        assert_eq!(client.read(&mut [0]).await?, 0);
        retained.await.map_err(io::Error::other)??;
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn shutdown_quiesces_a_replacement_relay_and_holders_registered_late() -> io::Result<()> {
    // Capacity notices go to process stderr and use a process-wide counter. Isolate both
    // observations from tests that intentionally fill a relay.
    const CHILD: &str = "YULIUS_TEST_QUIET_RELAY_STOP";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::time::timeout(Duration::from_secs(8),
            tokio::process::Command::new(std::env::current_exe()?)
                .args(["--exact", "relay::handoff::tests::shutdown_quiesces_a_replacement_relay_and_holders_registered_late", "--nocapture"])
                .env(CHILD, "1").kill_on_drop(true).output()).await??;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("connection was dropped at the limit"),
            "{stderr}"
        );
        assert!(output.status.success(), "{stderr}");
        return Ok(());
    }
    let before = super::super::dropped_connections();
    tokio::time::timeout(Duration::from_secs(3), async {
        let holder = holder()?;
        let relays = Arc::new(OutboundRelays::default());
        let dir = tempfile::tempdir()?;
        let start = |name| -> io::Result<_> {
            Ok(tokio::spawn(relays.serve(
                UnixListener::bind(dir.path().join(name))?,
                Arc::new(BTreeMap::new()),
                holder.sandbox.clone(),
                holder.clone(),
                Arc::new(ConnectionHealth::default()),
            )))
        };
        let old = start("old.sock")?;
        let current = start("current.sock")?;
        old.abort();
        assert!(old.await.is_err_and(|e| e.is_cancelled()));
        relays.stop_admissions();
        let late = start("late.sock")?;
        for name in ["current.sock", "late.sock"] {
            // An admitted relay waits for the port header. A stopped relay closes immediately.
            let mut peer = UnixStream::connect(dir.path().join(name)).await?;
            assert_eq!(peer.read(&mut [0]).await?, 0);
        }
        for server in [current, late] {
            server.abort();
            assert!(server.await.is_err_and(|e| e.is_cancelled()));
        }
        assert!(
            relays
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .relays
                .is_empty()
        );
        assert_eq!(super::super::dropped_connections(), before);
        Ok(())
    })
    .await?
}

fn registered_table(places: &Arc<Places>) -> OutboundRelays {
    OutboundRelays(Mutex::new(Holders {
        stopping: false,
        relays: BTreeMap::from([("fixture".into(), Arc::clone(places))]),
    }))
}

#[test]
fn an_ambiguous_internal_link_leaves_both_capture_controls_unconsumed() -> io::Result<()> {
    let places = Places::new(2, IDLE_FLOOR);
    let hub = registered_table(&places);
    let _first = places.admit().ok_or_else(invalid)?;
    let _second = places.admit().ok_or_else(invalid)?;
    let link = TcpLink {
        relay: ([127, 0, 0, 1], 20113).into(),
        proxy: ([127, 0, 0, 1], 20114).into(),
    };
    let mut requests = Vec::new();
    {
        let mut slots = places.table.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, control) in slots.connections.controls_mut() {
            let (send, request) = oneshot::channel();
            control.capture = Some((link, send));
            requests.push(request);
        }
    }
    hub.stop_admissions();
    assert!(
        hub.capture("fixture", link, monotonic_ms() + 5_000)
            .is_none()
    );
    assert_eq!(requests.len(), 2);
    for request in &mut requests {
        assert!(matches!(
            request.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn capture_saves_the_reply_already_read_into_the_blocked_unix_writer() -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let places = Places::new(1, IDLE_FLOOR);
        let hub = registered_table(&places);
        let (place, _ended, _) = places.admit().ok_or_else(invalid)?;
        let (mut client, writer) = UnixStream::pair()?;
        rustix::net::sockopt::set_socket_send_buffer_size(&writer, 4096)?;
        writer.writable().await?;
        let mut queued = Vec::new();
        loop {
            match writer.try_write(&[0x35; 4096]) {
                Ok(count) => queued.extend(std::iter::repeat_n(0x35, count)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let upstream = TcpStream::connect(listener.local_addr()?).await?;
        let (mut peer, _) = listener.accept().await?;
        let link = TcpLink::at_proxy(&peer)?;
        let inspect = upstream.as_fd().try_clone_to_owned()?;
        let marker = b"synthetic pending reply";
        peer.write_all(marker).await?;
        while rustix::io::ioctl_fionread(&inspect)? != marker.len() as u64 {
            tokio::task::yield_now().await;
        }
        let holder = holder()?;
        let copying = tokio::spawn(async move {
            transfer(
                Tracked {
                    inner: writer,
                    place,
                },
                upstream,
                &holder,
            )
            .await
        });
        while rustix::io::ioctl_fionread(&inspect)? != 0 {
            tokio::task::yield_now().await;
        }
        hub.stop_admissions();
        let pair = hub
            .capture("fixture", link, monotonic_ms() + 5_000)
            .ok_or_else(invalid)?
            .await
            .map_err(io::Error::other)?;
        copying.await.map_err(io::Error::other)??;
        let (_, saved) = Owner::decode(&pair.state)?;
        assert!(
            saved.ends_with(marker),
            "capture lost the pending Unix write"
        );
        let mut copy = CopyState::decode(saved)?;
        let [left, right] = pair.sockets;
        let mut left = UnixStream::from_std(left.into())?;
        let mut right = TcpStream::from_std(right.into())?;
        let resumed = tokio::spawn(async move { copy.transfer(&mut left, &mut right).await });
        peer.write_all(b" next").await?;
        peer.shutdown().await?;
        client.shutdown().await?;
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).await?;
        queued.extend_from_slice(marker);
        queued.extend_from_slice(b" next");
        assert_eq!(reply, queued);
        assert_eq!(peer.read(&mut [0]).await?, 0);
        resumed.await.map_err(io::Error::other)??;
        Ok(())
    })
    .await?
}
