//! Orderly Linux server handoff for opaque agent tunnels and their matching outbound relays (SPEC-577).
use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use yulius_egress::EgressProxy;
use yulius_sandbox::cgroup;
use yulius_sandbox::relay::custodian;
use yulius_sandbox::relay::handoff::{OutboundRelays, TcpLink};

// Namespace-mode proxy records use a distinct tag. A change of isolation mode cannot
// restore one side of a previously joined connection.
fn matches(
    pairs: Vec<custodian::Pair>,
) -> Vec<(String, TcpLink, custodian::Pair, custodian::Pair)> {
    let mut relays = BTreeMap::new();
    let mut proxies = Vec::new();
    for mut pair in pairs {
        if pair.state.starts_with(b"RL1\n") {
            if let Ok(link) = TcpLink::at_relay(&pair) {
                relays.insert(link, pair);
            }
        } else if pair.state.starts_with(b"NS1\n") {
            pair.state[..3].copy_from_slice(b"OP1");
            if let Ok((sandbox, link)) = EgressProxy::relay_link(&pair) {
                proxies.push((sandbox, link, pair));
            }
        }
    }
    proxies
        .into_iter()
        .filter_map(|(sandbox, link, proxy)| {
            let relay = relays.remove(&link)?;
            (proxy.expires.is_some() && proxy.expires == relay.expires)
                .then_some((sandbox, link, proxy, relay))
        })
        .collect()
}

pub(crate) async fn restore(
    proxy: &EgressProxy,
    directory: &Path,
    relays: Option<&OutboundRelays>,
) {
    match custodian::adopt(directory).await {
        Ok(pairs) => {
            if let Some(relays) = relays {
                for (sandbox, link, opaque, relay) in matches(pairs) {
                    if let Err(error) = relays.restore(&sandbox, link, relay, || {
                        proxy.restore_opaque_tunnel(opaque)
                    }) {
                        tracing::warn!(kind=?error.kind(),"cannot restore an opaque tunnel through its holder");
                    }
                }
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
    if let Some(relays) = relays {
        relays.stop_admissions();
    }
    let mut pairs = proxy.take_opaque_tunnels().await?;
    if let Some(relays) = relays {
        let mut replies = Vec::new();
        for mut pair in pairs {
            let Ok((sandbox, link)) = EgressProxy::relay_link(&pair) else {
                continue;
            };
            let Some(expires) = pair.expires else {
                continue;
            };
            let Some(relay) = relays.capture(&sandbox, link, expires) else {
                continue;
            };
            pair.state[..3].copy_from_slice(b"NS1");
            let (send, receive) = tokio::sync::oneshot::channel();
            let _ = send.send(pair);
            replies.extend([receive, relay]);
        }
        pairs = Vec::new();
        for (_, _, mut opaque, relay) in matches(
            tokio::time::timeout(Duration::from_secs(5), custodian::collect(replies)).await??,
        ) {
            opaque.state[..3].copy_from_slice(b"NS1");
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
        let end = Some(custodian::monotonic_ms() + 5_000);
        let holder = serde_json::json!({
            "version": 1, "sandbox": sandbox, "base_id": "fixture",
            "inner_pid": std::process::id(), "bwrap_pid": 0, "user_ns": 1,
            "net_ns": 2, "mnt_ns": 3, "boot_id": "fixture", "started_at_ms": 1,
            "system_root": "/fixture"
        });
        let relay_owner =
            serde_json::json!({"holder": holder, "link": link, "order": 1, "last": 0});
        let proxy_owner =
            serde_json::json!({"sandbox": sandbox, "run": "fixture", "link": link, "last": 0});
        let relay = custodian::Pair {
            sockets: [relay_client.into(), upstream.into()],
            state: format!("RL1\n{relay_owner}\n").into_bytes(),
            expires: end,
            established: None,
        };
        let proxy = custodian::Pair {
            sockets: [proxy_client.into(), origin.into()],
            state: format!("NS1\n{proxy_owner}\n").into_bytes(),
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
