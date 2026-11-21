//! Exercise the actual outgoing relay, its table and the original socket endpoints.
use super::*;
use crate::holder::{boot_id, ns_inode};
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

async fn record(
    holder: &HolderRecord,
    order: u64,
    last: u64,
    expires: u64,
) -> io::Result<(Pair, TcpLink, UnixStream, TcpStream)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let upstream = TcpStream::connect(listener.local_addr()?).await?;
    let (peer, _) = listener.accept().await?;
    let (left, client) = UnixStream::pair()?;
    let link = TcpLink::at_proxy(&peer.as_fd().try_clone_to_owned()?)?;
    let owner = Owner {
        link,
        holder: holder.clone(),
        order,
        last,
    };
    let mut state = b"RL1\n".to_vec();
    serde_json::to_writer(&mut state, &owner)?;
    state.push(b'\n');
    CopyState::default().encode(&mut state)?;
    let pair = Pair {
        established: Some(Established::connected(&left, &upstream)?),
        sockets: [left.into_std()?.into(), upstream.into_std()?.into()],
        expires: Some(expires),
        state,
    };
    Ok((pair, link, client, peer))
}

#[tokio::test]
async fn startup_reserves_the_old_places_in_order_and_retains_idle_activity() -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let holder = holder()?;
        let hub = OutboundRelays::default();
        hub.defer();
        let places = Places::new(2, IDLE_FLOOR);
        let (ready, receive) = oneshot::channel();
        let outbound = Arc::new(Outbound {
            holder: Arc::new(holder.clone()),
            places: Arc::clone(&places),
            startup: Mutex::new(Some(Startup {
                ready,
                saved: Vec::new(),
            })),
        });
        hub.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .relays
            .insert(holder.sandbox.clone(), outbound);
        let now = monotonic_ms();
        let expires = now + 5_000;
        let (newer, new_link, _new_client, _new_peer) = record(&holder, 17, now, expires).await?;
        let old_activity = now.saturating_sub(1);
        let (older, old_link, _old_client, _old_peer) =
            record(&holder, 3, old_activity, expires).await?;
        // A replay of either the original internal socket or its admission order cannot
        // reserve another place. Retain the original relay while the duplicate is dropped.
        let mut replay = Pair {
            sockets: [
                newer.sockets[0].as_fd().try_clone_to_owned()?,
                newer.sockets[1].as_fd().try_clone_to_owned()?,
            ],
            established: newer.established,
            state: b"RL1\n".to_vec(),
            expires: newer.expires,
        };
        let (mut owner, cursor) = Owner::decode(&newer.state)?;
        owner.order += 1;
        serde_json::to_writer(&mut replay.state, &owner)?;
        replay.state.push(b'\n');
        replay.state.extend_from_slice(cursor);
        hub.restore(
            &holder.sandbox,
            new_link,
            newer,
            CopyCancellation::default(),
            || Ok(()),
        )?;
        assert!(
            hub.restore(
                &holder.sandbox,
                new_link,
                replay,
                CopyCancellation::default(),
                || { panic!("a replay admitted a second proxy") }
            )
            .is_err()
        );
        let (duplicate, duplicate_link, mut duplicate_client, _duplicate_peer) =
            record(&holder, 17, now, expires).await?;
        assert!(
            hub.restore(
                &holder.sandbox,
                duplicate_link,
                duplicate,
                CopyCancellation::default(),
                || { panic!("a repeated admission order admitted a second proxy") }
            )
            .is_err()
        );
        assert_eq!(duplicate_client.read(&mut [0]).await?, 0);

        hub.restore(
            &holder.sandbox,
            old_link,
            older,
            CopyCancellation::default(),
            || Ok(()),
        )?;
        let (excess, excess_link, mut client, mut peer) = record(&holder, 99, now, expires).await?;
        let mut asked = false;
        assert!(
            hub.restore(
                &holder.sandbox,
                excess_link,
                excess,
                CopyCancellation::default(),
                || {
                    asked = true;
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!asked);
        assert_eq!(client.read(&mut [0]).await?, 0);
        assert_eq!(peer.read(&mut [0]).await?, 0);
        hub.begin();
        let [mut oldest, mut active]: [Resume; 2] = receive
            .await
            .map_err(io::Error::other)?
            .try_into()
            .map_err(|_| io::Error::other("lost a place"))?;
        assert_eq!(oldest.place.activity.last(), old_activity);
        assert_eq!(active.place.activity.last(), now);
        assert_eq!(oldest.expires, expires);
        // Advance the admission table's clock to the exact floor, even on a newly booted host.
        let admit_at = |time| {
            let (end, _) = oneshot::channel();
            places
                .table
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .connections
                .admit((), Control::new(end), time)
        };
        let boundary = old_activity + 300_000;
        assert!(admit_at(boundary - 1).is_err());
        active.place.activity.touch(boundary - 1);
        let evicted = admit_at(boundary).map_err(io::Error::other)?.evicted;
        assert_eq!(evicted.map(|old| old.idle_ms), Some(300_000));
        assert_eq!(
            oldest.evicted.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        assert_eq!(
            active.evicted.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        assert!(admit_at(boundary).is_err());
        drop(active);
        assert!(places.admit().is_some());
        Ok(())
    })
    .await?
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
        let mut holder = holder()?;
        holder.slirp_pid = Some(41);
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
        let next = Arc::new(OutboundRelays::default());
        next.defer();
        let next_address = dir.path().join("replacement.sock");
        // Upkeep may replace slirp4netns while this holder and its Unix forward keep running.
        let mut refreshed = holder.clone();
        refreshed.slirp_pid = Some(42);
        let replacement = tokio::spawn(next.serve(
            UnixListener::bind(&next_address)?,
            targets,
            holder.sandbox.clone(),
            refreshed,
            health,
        ));
        peer.write_all(b"down").await?;
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
        next.restore(
            &holder.sandbox,
            link,
            pair,
            CopyCancellation::default(),
            || Ok(()),
        )?;
        next.begin();
        next.begin();
        assert_eq!(client.read_u32().await?.to_be_bytes(), *b"down");
        if final_reply {
            assert_eq!(client.read(&mut [0]).await?, 0);
        } else {
            client.write_all(b"next").await?;
            assert_eq!(peer.read_u32().await?.to_be_bytes(), *b"next");
        }
        replacement.abort();
        assert!(replacement.await.is_err_and(|e| e.is_cancelled()));
        assert_eq!(client.read(&mut [0]).await?, 0);
        assert_eq!(peer.read(&mut [0]).await?, 0);
        assert!(next.capture(&holder.sandbox, link, expires).is_none());
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn a_changed_holder_or_clock_releases_its_original_sockets_without_admitting_a_proxy()
-> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let holder = holder()?;
        let relays = Arc::new(OutboundRelays::default());
        relays.defer();
        let temp = tempfile::tempdir()?;
        let serving = tokio::spawn(relays.serve(
            UnixListener::bind(temp.path().join("out.sock"))?,
            Arc::new(BTreeMap::new()),
            holder.sandbox.clone(),
            holder.clone(),
            Arc::new(ConnectionHealth::default()),
        ));
        let mut replaced = holder.clone();
        replaced.started_at_ms += 1;
        let now = monotonic_ms();
        for (record_owner, last, expires) in [
            (&replaced, now, now + 5000),
            (&holder, now + 5000, now + 5000),
            (&holder, now, now),
        ] {
            let (pair, link, mut client, mut peer) = record(record_owner, 7, last, expires).await?;
            let mut asked = false;
            assert!(
                relays
                    .restore(
                        &holder.sandbox,
                        link,
                        pair,
                        CopyCancellation::default(),
                        || {
                            asked = true;
                            Ok(())
                        }
                    )
                    .is_err()
            );
            assert!(!asked);
            assert_eq!(client.read(&mut [0]).await?, 0);
            assert_eq!(peer.read(&mut [0]).await?, 0);
        }
        // A saved record and the registry can agree even though the recorded namespace ended.
        let mut stale = holder.clone();
        stale.net_ns = stale.net_ns.saturating_add(1);
        let (pair, link, mut client, mut peer) = record(&stale, 7, now, now + 5000).await?;
        assert!(Saved::decode(pair, &stale, link).is_err());
        assert_eq!(client.read(&mut [0]).await?, 0);
        assert_eq!(peer.read(&mut [0]).await?, 0);
        let (refused, link, mut client, mut peer) = record(&holder, 9, now, now + 5000).await?;
        let failure = relays.restore(
            &holder.sandbox,
            link,
            refused,
            CopyCancellation::default(),
            || Err(io::ErrorKind::WouldBlock.into()),
        );
        assert!(failure.is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock));
        assert_eq!(client.read(&mut [0]).await?, 0);
        assert_eq!(peer.read(&mut [0]).await?, 0);
        // The same admission order is now free. The restored connection retains its end time.
        let (timed, link, mut live, mut origin) =
            record(&holder, 9, now, monotonic_ms() + 1_000).await?;
        relays.restore(
            &holder.sandbox,
            link,
            timed,
            CopyCancellation::default(),
            || Ok(()),
        )?;
        relays.begin();
        live.write_all(b"last").await?;
        assert_eq!(origin.read_u32().await?.to_be_bytes(), *b"last");
        assert_eq!(live.read(&mut [0]).await?, 0);
        assert_eq!(origin.read(&mut [0]).await?, 0);
        assert!(
            relays
                .capture(&holder.sandbox, link, monotonic_ms() + 5000)
                .is_none()
        );
        serving.abort();
        assert!(serving.await.is_err_and(|e| e.is_cancelled()));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn shutdown_quiesces_a_replacement_relay_and_holders_registered_late() -> io::Result<()> {
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
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn the_internal_tcp_leg_belongs_to_the_original_two_endpoints() -> io::Result<()> {
    let holder = holder()?;
    let now = monotonic_ms();
    let (pair, link, _client, peer) = record(&holder, 1, now, now + 5000).await?;
    let (other, _new_link, _new_client, _new_peer) = record(&holder, 2, now, now + 5000).await?;
    assert!(Saved::decode(other, &holder, link).is_err());
    // One side of a reconnected link might keep its listening address. Both must match.
    let relay_fd = &pair.sockets[1];
    let mut wrong_local = link.relay;
    wrong_local.set_port(0);
    let mut wrong_peer = link.proxy;
    wrong_peer.set_port(0);
    assert!(TcpLink::verify_endpoint(&pair, relay_fd, wrong_local, link.proxy).is_err());
    assert!(TcpLink::verify_endpoint(&pair, relay_fd, link.relay, wrong_peer).is_err());
    TcpLink::verify_endpoint(&pair, relay_fd, link.relay, link.proxy)?;
    assert!(TcpLink::at_proxy(&peer)? == link);
    Ok(())
}
