//! Orderly Linux server handoff for direct opaque agent tunnels (SPEC-577).
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path, PathBuf};
use yulius_sandbox::cgroup;

use yulius_egress::EgressProxy;
use yulius_sandbox::relay::custodian;

pub(crate) async fn restore(proxy: &EgressProxy, directory: &Path) {
    match custodian::adopt(directory).await {
        Ok(pairs) => {
            for pair in pairs {
                if let Err(error) = proxy.restore_opaque_tunnel(pair) {
                    tracing::warn!(kind = ?error.kind(), "cannot restore an opaque tunnel");
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(kind = ?error.kind(), "cannot adopt opaque tunnels"),
    }
}

pub(crate) async fn deposit(
    proxy: &EgressProxy,
    directory: &Path,
    executable: &Path,
) -> io::Result<()> {
    let pairs = proxy.take_opaque_tunnels().await?;
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
