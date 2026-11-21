//! Orderly Linux server handoff for opaque agent tunnels and their matching outbound relays (SPEC-577).
use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use yulius_egress::EgressProxy;
use yulius_sandbox::cgroup;
use yulius_sandbox::relay::custodian;
use yulius_sandbox::relay::handoff::{CopyCancellation, OutboundRelays, TcpLink};

// Multiple records for the same enrolled socket cannot select a unique original copy.
fn unique_proxies(pairs: Vec<custodian::Pair>) -> Vec<(String, TcpLink, u64, custodian::Pair)> {
    let mut unique = BTreeMap::new();
    let mut proxies = Vec::new();
    for pair in pairs {
        if let Ok((sandbox, link, cookie)) = EgressProxy::enrolled_relay(&pair) {
            unique
                .entry((link, cookie))
                .and_modify(|only| *only = false)
                .or_insert(true);
            proxies.push((sandbox, link, cookie, pair));
        }
    }
    proxies.retain(|(_, link, cookie, _)| unique.get(&(*link, *cookie)) == Some(&true));
    proxies
}

// Namespace-mode proxy records use a distinct tag. A change of isolation mode cannot
// restore one side of a previously joined connection.
fn matches(
    pairs: Vec<custodian::Pair>,
) -> Vec<(String, TcpLink, custodian::Pair, custodian::Pair)> {
    let mut relays = BTreeMap::new();
    let mut proxies = Vec::new();
    for mut pair in pairs {
        if pair.state.starts_with(b"RL1\n") {
            if let (Ok(link), Ok(cookie)) = (
                TcpLink::at_relay(&pair),
                TcpLink::relay_cookie(&pair.sockets[1]),
            ) {
                relays
                    .entry((link, cookie))
                    .and_modify(|only| *only = None)
                    .or_insert(Some(pair));
            }
        } else if pair.state.starts_with(b"NS2\n") {
            pair.state[..3].copy_from_slice(b"OP1");
            proxies.push(pair);
        }
    }
    unique_proxies(proxies)
        .into_iter()
        .filter_map(|(sandbox, link, cookie, proxy)| {
            let relay = relays.remove(&(link, cookie))??;
            (proxy.expires.is_some() && proxy.expires == relay.expires)
                .then_some((sandbox, link, proxy, relay))
        })
        .collect()
}

fn restore_joined(proxy: &EgressProxy, relays: &OutboundRelays, pairs: Vec<custodian::Pair>) {
    for (sandbox, link, opaque, relay) in matches(pairs) {
        let (relay_cancel, proxy_cancel) = CopyCancellation::pair();
        if let Err(error) = relays.restore(&sandbox, link, relay, relay_cancel, || {
            proxy.restore_with_cancellation(opaque, proxy_cancel)
        }) {
            tracing::warn!(kind=?error.kind(),"cannot restore an opaque tunnel through its holder");
        }
    }
}

pub(crate) async fn restore(
    proxy: &EgressProxy,
    directory: &Path,
    relays: Option<&OutboundRelays>,
) {
    match custodian::adopt(directory).await {
        Ok(pairs) => {
            if let Some(relays) = relays {
                restore_joined(proxy, relays, pairs);
            } else {
                for pair in pairs {
                    if let Err(error) = proxy.restore_opaque_tunnel(pair) {
                        tracing::warn!(kind=?error.kind(),"cannot restore an opaque tunnel");
                    }
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(kind=?error.kind(),"cannot adopt opaque tunnels"),
    }
    if let Some(relays) = relays {
        relays.begin();
    }
}

pub(crate) async fn deposit(
    proxy: &EgressProxy,
    directory: &Path,
    executable: &Path,
    relays: Option<&OutboundRelays>,
) -> io::Result<()> {
    let mut pairs = proxy.take_opaque_tunnels().await?;
    if let Some(relays) = relays {
        let mut replies = Vec::new();
        for (sandbox, link, cookie, mut pair) in unique_proxies(pairs) {
            let Some(expires) = pair.expires else {
                continue;
            };
            let Some(relay) = relays.capture_enrolled(&sandbox, link, cookie, expires) else {
                continue;
            };
            pair.state[..3].copy_from_slice(b"NS2");
            let (send, receive) = tokio::sync::oneshot::channel();
            let _ = send.send(pair);
            replies.extend([receive, relay]);
        }
        pairs = Vec::new();
        for (_, _, mut opaque, relay) in matches(
            tokio::time::timeout(Duration::from_secs(5), custodian::collect(replies)).await??,
        ) {
            opaque.state[..3].copy_from_slice(b"NS2");
            pairs.extend([opaque, relay]);
        }
    }
    if pairs.is_empty() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    custodian::deposit(executable, directory, pairs).await
}

// On a delegated systemd service, a surviving child must leave the unit startable (ADR-0298).
fn helper_leaf(
    root: &Path,
    own: &str,
    version: impl FnOnce() -> Option<u32>,
) -> io::Result<Option<PathBuf>> {
    let relative = Path::new(own.trim_start_matches('/'));
    let Some(parent) = relative
        .parent()
        .filter(|p| p.extension().is_some_and(|ext| ext == "service"))
    else {
        return Ok(None);
    };
    if relative
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid delegated service path",
        ));
    }
    let service = root.join(parent);
    if let Ok(unit) = cgroup::detect(root, own)
        && unit != service
    {
        return Ok(Some(unit.join(cgroup::LEGACY_LEAF)));
    }
    let controllers = std::fs::read_to_string(service.join("cgroup.subtree_control"))?;
    if controllers.trim().is_empty()
        || version().is_some_and(|v| v < crate::restart_safety::FIRST_AFFECTED)
    {
        return Ok(None);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no sessions unit for a tunnel helper across this service restart",
    ))
}

pub(crate) fn place_helper(root: &Path, membership: &Path) -> io::Result<()> {
    let membership = std::fs::read_to_string(membership)?;
    if let Some(own) = cgroup::own_path(&membership)
        && let Some(leaf) = helper_leaf(root, &own, || {
            crate::doctor::output_of(Path::new("/usr/bin/systemctl"), &["--version"])
                .and_then(|text| crate::restart_safety::systemd_version(&text))
        })?
    {
        std::fs::create_dir_all(&leaf)?;
        cgroup::join(&leaf)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(sandbox: &str) -> io::Result<yulius_contracts::sandbox::HolderRecord> {
        use yulius_contracts::sandbox::{HOLDER_RECORD_VERSION, HolderRecord};
        use yulius_sandbox::holder::{boot_id, ns_inode};
        let pid = std::process::id();
        let inode = |ns| ns_inode(pid, ns).ok_or_else(|| io::Error::other("fixture namespace"));
        Ok(HolderRecord {
            version: HOLDER_RECORD_VERSION,
            sandbox: sandbox.into(),
            base_id: "fixture".into(),
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

    // Supply the two original endpoints of one internal TCP connection. The relay's Unix
    // client and the proxy's destination remain open so refusal is visible to both peers.
    fn joined(
        sandbox: &str,
    ) -> io::Result<(
        custodian::Pair,
        custodian::Pair,
        std::os::unix::net::UnixStream,
        std::net::TcpStream,
    )> {
        use std::net::{TcpListener, TcpStream};
        use std::os::unix::net::UnixStream;

        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let upstream = TcpStream::connect(listener.local_addr()?)?;
        let (proxy_client, _) = listener.accept()?;
        let origin = TcpStream::connect(listener.local_addr()?)?;
        let (destination, _) = listener.accept()?;
        let (relay_client, client) = UnixStream::pair()?;
        let link = TcpLink::at_proxy(&proxy_client)?;
        let end = Some(custodian::monotonic_ms() + 60_000);
        let holder = holder(sandbox)?;
        let relay_owner =
            serde_json::json!({"holder": holder, "link": link, "order": 1, "last": 0});
        let proxy_owner = serde_json::json!({"sandbox": sandbox, "run": "fixture", "link": link, "relay_cookie": TcpLink::relay_cookie(&upstream)?, "last": 0});
        let relay = custodian::Pair {
            sockets: [relay_client.into(), upstream.into()],
            state: format!("RL1\n{relay_owner}\n\0\0\0\0\0\0").into_bytes(),
            expires: end,
            established: None,
        };
        let proxy = custodian::Pair {
            sockets: [proxy_client.into(), origin.into()],
            state: format!("NS2\n{proxy_owner}\n\0\0\0\0\0\0").into_bytes(),
            expires: end,
            established: None,
        };
        Ok((proxy, relay, client, destination))
    }

    #[test]
    fn a_namespace_restart_requires_both_original_legs_with_the_same_deadline() -> io::Result<()> {
        use std::io::Read as _;

        let (first, first_relay, _first_client, _first_origin) = joined("first")?;
        let (second, second_relay, _second_client, _second_origin) = joined("second")?;
        let (mut wrong_time, wrong_time_relay, mut client, mut origin) = joined("different-time")?;
        wrong_time.expires = wrong_time.expires.map(|end| end + 1);
        let (mut no_time, mut no_time_relay, _no_time_client, _no_time_origin) = joined("no-time")?;
        no_time.expires = None;
        no_time_relay.expires = None;
        let (mut direct, direct_relay, _direct_client, _direct_origin) = joined("direct")?;
        direct.state[..3].copy_from_slice(b"OP1");
        let (mut unstamped, unstamped_relay, _old_client, _old_origin) = joined("old-format")?;
        unstamped.state[..3].copy_from_slice(b"NS1");
        let (unpaired, missing, _unpaired_client, _unpaired_origin) = joined("unpaired")?;
        drop(missing);
        let matched = matches(vec![
            second_relay,
            first,
            wrong_time,
            second,
            wrong_time_relay,
            no_time_relay,
            no_time,
            first_relay,
            direct,
            direct_relay,
            unstamped,
            unstamped_relay,
            unpaired,
        ]);
        assert_eq!(matched.len(), 2);
        for ((sandbox, link, proxy, relay), expected) in
            matched.into_iter().zip(["first", "second"])
        {
            assert_eq!(sandbox, expected);
            assert!(TcpLink::at_proxy(&proxy.sockets[0])? == link);
            assert!(TcpLink::at_relay(&relay)? == link);
            assert_eq!(EgressProxy::relay_link(&proxy)?.0, expected);
        }
        client.set_read_timeout(Some(Duration::from_secs(2)))?;
        origin.set_read_timeout(Some(Duration::from_secs(2)))?;
        assert_eq!(client.read(&mut [0])?, 0);
        assert_eq!(origin.read(&mut [0])?, 0);
        Ok(())
    }

    #[test]
    fn the_recorded_relay_incarnation_must_match_its_original_socket() -> io::Result<()> {
        let (mut opaque, relay, _client, _origin) = joined("fixture")?;
        // Supply a well-formed proxy record for a different synthetic relay incarnation.
        let (header, copy) = opaque.state[4..].split_at(
            opaque.state[4..]
                .iter()
                .position(|byte| *byte == b'\n')
                .ok_or(io::ErrorKind::InvalidData)?,
        );
        let mut owner: serde_json::Value = serde_json::from_slice(header)?;
        owner["relay_cookie"] = 0.into();
        let mut state = b"NS2\n".to_vec();
        serde_json::to_writer(&mut state, &owner)?;
        state.extend_from_slice(copy);
        opaque.state = state;
        let (healthy, healthy_relay, _healthy_client, _healthy_origin) = joined("healthy")?;
        let retained = matches(vec![opaque, healthy, relay, healthy_relay]);
        assert_eq!(
            retained.len(),
            1,
            "unrelated relay incarnation was retained"
        );
        assert_eq!(retained[0].0, "healthy");
        Ok(())
    }

    #[test]
    fn duplicate_records_release_the_ambiguous_peers_and_keep_the_unique_tunnel() -> io::Result<()>
    {
        use std::io::Read as _;
        use std::time::Duration;
        for duplicate_relay in [true, false] {
            let (opaque, relay, mut client, mut origin) = joined("ambiguous")?;
            let original = if duplicate_relay { &relay } else { &opaque };
            let duplicate = custodian::Pair {
                sockets: [
                    original.sockets[0].try_clone()?,
                    original.sockets[1].try_clone()?,
                ],
                state: original.state.clone(),
                expires: original.expires,
                established: original.established,
            };
            let (healthy, healthy_relay, _client, _origin) = joined("healthy")?;
            let retained = matches(vec![relay, healthy, opaque, healthy_relay, duplicate]);
            let owners: Vec<_> = retained
                .iter()
                .map(|(name, _, _, _)| name.as_str())
                .collect();
            assert_eq!(owners, ["healthy"], "ambiguous records must reconnect");
            client.set_read_timeout(Some(Duration::from_secs(2)))?;
            origin.set_read_timeout(Some(Duration::from_secs(2)))?;
            assert_eq!(client.read(&mut [0])?, 0);
            assert_eq!(origin.read(&mut [0])?, 0);
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_the_restored_relay_releases_the_idle_proxy_place()
    -> Result<(), Box<dyn std::error::Error>> {
        joined_cleanup(false).await
    }

    #[tokio::test]
    async fn evicting_the_restored_proxy_releases_the_idle_relay_place()
    -> Result<(), Box<dyn std::error::Error>> {
        joined_cleanup(true).await
    }

    async fn joined_cleanup(evict_proxy: bool) -> Result<(), Box<dyn std::error::Error>> {
        use clap::Parser;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use yulius_contracts::sandbox::IdMap;
        use yulius_sandbox::holder::{StatePaths, write_json};
        use yulius_sandbox::manager::{IdleConfig, IsolationConfig, Manager};

        #[derive(Parser)]
        struct Fixture {
            #[command(flatten)]
            args: crate::serve::ServeArgs,
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let dir = tempfile::tempdir()?;
            let args = Fixture::try_parse_from([
                "fixture",
                "--ci",
                "off",
                "--data",
                dir.path().to_str().ok_or("fixture directory")?,
            ])?
            .args;
            let (app, _) = crate::serve::build_app(&args)?;
            let _ = rustls::crypto::ring::default_provider().install_default();
            let proxy = crate::wiring::egress::egress_proxy(
                &app,
                yulius_egress::EgressConfig {
                    max_connections: 1,
                    max_per_sandbox: 1,
                    idle_floor: Duration::ZERO,
                    ..Default::default()
                },
            )
            .await?;
            let state = StatePaths::new(dir.path().join("runtime/sandboxes/s1"));
            std::fs::create_dir_all(&state.dir)?;
            write_json(&state.record(), &holder("s1")?)?;
            let map = IdMap {
                host: 1000,
                sub_start: 100_000,
                count: 65_535,
            };
            let manager = Manager::new(IsolationConfig {
                data_dir: dir.path().into(),
                exe: "/fixture".into(),
                bwrap: "/fixture".into(),
                newuidmap: "/fixture".into(),
                newgidmap: "/fixture".into(),
                slirp4netns: None,
                uid: map,
                gid: map,
                delegation: None,
                limits: Default::default(),
                disk_max: None,
                forwards: BTreeMap::new(),
                sudo_shim: String::new(),
                auto_build: false,
                idle: IdleConfig {
                    stop_after: Duration::from_secs(60),
                },
                docker: Err("fixture".into()),
                health: app.connection_health(),
            });
            let relays = std::sync::Arc::clone(&manager.outbound_relays);
            relays.defer();
            assert!(manager.adopt_one("s1").await);
            let (opaque, relay, client, origin) = joined("s1")?;
            let link = TcpLink::at_proxy(&opaque.sockets[0])?;
            let expiry = opaque.expires.ok_or("fixture deadline")?;
            restore_joined(&proxy, &relays, vec![opaque, relay]);
            relays.begin();
            client.set_nonblocking(true)?;
            origin.set_nonblocking(true)?;
            let mut client = tokio::net::UnixStream::from_std(client)?;
            let mut origin = tokio::net::TcpStream::from_std(origin)?;
            client.write_all(b"held").await?;
            assert_eq!(origin.read_u32().await?.to_be_bytes(), *b"held");

            if evict_proxy {
                let target = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
                let port = target.local_addr()?.port();
                let browser = yulius_egress::BrowserEgress {
                    sandbox: "s1".into(),
                    ports: port..=port,
                    pages: None,
                    listeners: Vec::new(),
                };
                let request = axum::http::Request::connect(target.local_addr()?.to_string())
                    .body(axum::body::Body::empty())?;
                assert_eq!(proxy.handle_browser(&browser, request).await.status(), 200);
            } else {
                drop(manager); // The live holder's serving task owns and cancels its relay copies.
            }
            // Both original peers stay idle with their write halves open.
            assert_eq!(origin.read(&mut [0]).await?, 0);
            assert_eq!(client.read(&mut [0]).await?, 0);
            assert!(
                relays.capture("s1", link, expiry).is_none(),
                "the idle relay still holds a place"
            );
            assert!(
                proxy.take_opaque_tunnels().await?.is_empty(),
                "the idle proxy still holds a place"
            );
            Ok(())
        })
        .await?
    }

    #[test]
    fn a_helper_can_outlive_its_unit_only_where_the_next_server_can_start() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let slice = dir.path().join("yulius.slice");
        let service = slice.join("yulius.service");
        let sessions = slice.join("yulius-sandboxes.service");
        let own = "/yulius.slice/yulius.service/server";
        std::fs::create_dir_all(service.join("server"))?;
        std::fs::write(service.join("cgroup.subtree_control"), "+cpu")?;
        assert!(helper_leaf(dir.path(), own, || Some(258)).is_err());
        assert!(helper_leaf(dir.path(), own, || None).is_err());
        assert_eq!(helper_leaf(dir.path(), own, || Some(257))?, None);
        std::fs::write(service.join("cgroup.subtree_control"), "")?;
        assert_eq!(helper_leaf(dir.path(), own, || Some(258))?, None);
        std::fs::write(service.join("cgroup.subtree_control"), "+cpu")?;
        std::fs::create_dir(&sessions)?;
        for name in ["cgroup.procs", "cgroup.subtree_control"] {
            std::fs::write(sessions.join(name), "")?;
        }
        std::fs::write(slice.join("cgroup.procs"), "")?;
        assert_eq!(
            helper_leaf(dir.path(), own, || None)?,
            Some(sessions.join("legacy"))
        );
        // The production cgroup writer accepts only its actual filesystem root. Reaching
        // that check proves placement tried the selected leaf and propagates a failed move.
        let membership = dir.path().join("member");
        std::fs::write(&membership, format!("0::{own}\n"))?;
        assert_eq!(
            place_helper(dir.path(), &membership)
                .err()
                .map(|e| e.kind()),
            Some(io::ErrorKind::InvalidInput)
        );
        assert!(sessions.join(cgroup::LEGACY_LEAF).is_dir());
        assert_eq!(helper_leaf(dir.path(), "/", || None)?, None);
        assert!(helper_leaf(dir.path(), "/../outside.service/server", || None).is_err());
        Ok(())
    }
}
