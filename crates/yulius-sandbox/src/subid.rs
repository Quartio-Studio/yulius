//! Subordinate id ranges (`/etc/subuid`, `/etc/subgid`) of the service user.
//!
//! A sandbox's user namespace maps root to the service user and ids `1..=65535` to the first
//! range of at least [`REQUIRED_IDS`] ids that names the user (by login or numeric id), the
//! way `newuidmap` checks them.

use std::path::Path;

use yulius_contracts::sandbox::IdMap;

/// Ids a sandbox maps besides root: `1..=65535`, so every id a Debian package may use (system
/// users, `nobody` = 65534) exists inside.
pub const REQUIRED_IDS: u32 = 65_535;

/// The first usable range for `name`/`id` in the text of a subordinate id file.
#[must_use]
pub fn find_range(text: &str, name: &str, id: u32) -> Option<(u32, u32)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut fields = line.split(':');
            let owner = fields.next()?;
            let start: u32 = fields.next()?.trim().parse().ok()?;
            let count: u32 = fields.next()?.trim().parse().ok()?;
            let ours = owner == name || owner.parse::<u32>().ok() == Some(id);
            (ours && start > 0 && count >= REQUIRED_IDS && start.checked_add(count).is_some())
                .then_some((start, count))
        })
        .next()
}

/// The id map of `name`/`host` from the file at `path`, or why there is none.
///
/// # Errors
/// A message naming the file and what is missing (never secret).
pub fn id_map(path: &Path, name: &str, host: u32) -> Result<IdMap, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let (start, _) = find_range(&text, name, host).ok_or_else(|| {
        format!(
            "{} has no range of at least {} ids for {name} ({host})",
            path.display(),
            REQUIRED_IDS + 1
        )
    })?;
    Ok(IdMap {
        host,
        sub_start: start,
        count: REQUIRED_IDS,
    })
}

/// `newuidmap`/`newgidmap` arguments after the pid: root to `map.host`, `1..=count` to the
/// subordinate range.
#[must_use]
pub fn map_args(map: &IdMap) -> [String; 6] {
    [
        "0".into(),
        map.host.to_string(),
        "1".into(),
        "1".into(),
        map.sub_start.to_string(),
        map.count.to_string(),
    ]
}

/// The id inside the namespace that a host id shows as, if it is mapped.
#[must_use]
pub fn inside_id(map: &IdMap, host_id: u32) -> Option<u32> {
    if host_id == map.host {
        return Some(0);
    }
    let offset = host_id.checked_sub(map.sub_start)?;
    (offset < map.count).then_some(offset + 1)
}

/// The host id of an id inside the namespace, if it is mapped.
#[must_use]
pub fn host_id(map: &IdMap, inside: u32) -> Option<u32> {
    match inside {
        0 => Some(map.host),
        n if n <= map.count => map.sub_start.checked_add(n - 1),
        _ => None,
    }
}

/// This process's user: login name, uid and gid.
#[must_use]
pub fn current_user() -> (String, u32, u32) {
    let uid = nix::unistd::getuid();
    let gid = nix::unistd::getgid();
    let name = nix::unistd::User::from_uid(uid)
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_default();
    (name, uid.as_raw(), gid.as_raw())
}

/// The id maps of this process's user from `/etc/subuid` and `/etc/subgid`.
///
/// # Errors
/// What is missing (for `yulius doctor` and the server log).
pub fn current_maps() -> Result<(IdMap, IdMap), String> {
    let (name, uid, gid) = current_user();
    let users = id_map(Path::new("/etc/subuid"), &name, uid)?;
    let groups = id_map(Path::new("/etc/subgid"), &name, uid)?;
    Ok((
        users,
        IdMap {
            host: gid,
            ..groups
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# comment\nubuntu:100000:65536\nn3ci:165536:65536\n\nyulius:231072:1000\nyulius:300000:65536\n999:400000:65536\n";

    #[test]
    fn the_first_large_enough_range_of_the_user_is_used() {
        assert_eq!(find_range(FILE, "yulius", 999), Some((300_000, 65_536)));
        assert_eq!(
            find_range(FILE, "nobody-else", 999),
            Some((400_000, 65_536))
        );
        assert_eq!(find_range(FILE, "nobody-else", 5), None);
        assert_eq!(find_range("yulius:0:65536\n", "yulius", 1), None);
        assert_eq!(find_range("yulius:x:y\nbroken\n", "yulius", 1), None);
    }

    #[test]
    fn maps_translate_both_ways() {
        let map = IdMap {
            host: 999,
            sub_start: 231_072,
            count: REQUIRED_IDS,
        };
        assert_eq!(
            map_args(&map),
            ["0", "999", "1", "1", "231072", "65535"].map(str::to_owned)
        );
        assert_eq!(host_id(&map, 0), Some(999));
        assert_eq!(host_id(&map, 42), Some(231_113));
        assert_eq!(host_id(&map, 65_535), Some(296_606));
        assert_eq!(host_id(&map, 65_536), None);
        assert_eq!(inside_id(&map, 999), Some(0));
        assert_eq!(inside_id(&map, 231_113), Some(42));
        assert_eq!(inside_id(&map, 0), None);
        assert_eq!(inside_id(&map, 296_607), None);
    }

    #[test]
    fn missing_files_and_users_explain_themselves() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("subuid");
        let missing = id_map(&path, "yulius", 999).err().unwrap_or_default();
        assert!(missing.contains("cannot read"), "{missing}");
        std::fs::write(&path, "other:100000:65536\n")?;
        let none = id_map(&path, "yulius", 999).err().unwrap_or_default();
        assert!(
            none.contains("no range of at least 65536 ids for yulius"),
            "{none}"
        );
        std::fs::write(&path, "yulius:100000:65536\n")?;
        assert_eq!(id_map(&path, "yulius", 999)?.sub_start, 100_000);
        Ok(())
    }
}
