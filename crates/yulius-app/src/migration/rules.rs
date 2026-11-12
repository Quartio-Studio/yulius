//! What a move's request may name (SPEC-470 MVB-002). The server checks a request with these
//! rules before it writes it, and the root helper checks it again before it acts: the file is
//! the service user's, so the helper trusts none of it.

use yulius_contracts::migrate::{
    MAX_DATA_DISKS, MAX_DISK_BYTES, MAX_TARGET_BYTES, MoveAction, MoveRequest,
};

use crate::github_callback::public_url_ok;

/// Whether `id` is a request id: 8–64 of `[A-Za-z0-9-]`.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Whether `destination` is `[user@]host` and nothing `ssh` could read as an option.
#[must_use]
pub fn destination_ok(destination: &str) -> bool {
    let name = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-:[]".contains(c))
    };
    match destination.split_once('@') {
        Some((user, host)) => name(user) && name(host) && !user.contains([':', '[', ']']),
        None => name(destination),
    }
}

/// The host of a destination, without its user.
#[must_use]
pub fn host_part(destination: &str) -> &str {
    destination
        .split_once('@')
        .map_or(destination, |(_, host)| host)
}

/// Whether `disk` can name a blank disk of the new host: `/dev/` and a device's name, which
/// the new host compares with the disks it finds blank.
#[must_use]
pub fn data_disk_ok(disk: &str) -> bool {
    disk.len() <= MAX_DISK_BYTES
        && disk.strip_prefix("/dev/").is_some_and(|name| {
            !name.is_empty()
                && !name.starts_with('/')
                && !name.ends_with('/')
                && !name.contains("//")
                && !name.contains("..")
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        })
}

/// Checks what `request` names for its action.
///
/// # Errors
/// The rule it breaks, in words that never repeat the request's content.
pub fn check_request(request: &MoveRequest) -> Result<(), String> {
    if request.action == MoveAction::Abort {
        // An abort is of the recorded move: it could only name another one.
        let names_nothing = request.target.is_none()
            && request.ssh_port.is_none()
            && request.public_url.is_none()
            && request.data_disks.is_empty();
        return if names_nothing {
            Ok(())
        } else {
            Err("an abort names no host, port, URL or disk".into())
        };
    }
    let Some(target) = request.target.as_deref() else {
        return Err("the new host is missing".into());
    };
    if target.len() > MAX_TARGET_BYTES || !destination_ok(target) {
        return Err("the new host must be `root@host` or `host`".into());
    }
    if target
        .split_once('@')
        .is_some_and(|(user, _)| user != "root")
    {
        return Err("the new host is reached as root: `root@host`".into());
    }
    if !request.ssh_port.is_some_and(|port| port != 0) {
        return Err("the SSH port must be 1 to 65535".into());
    }
    if request
        .public_url
        .as_deref()
        .is_some_and(|url| url.len() > MAX_TARGET_BYTES || !public_url_ok(url))
    {
        return Err("the public URL must be `https://host`, with nothing after it".into());
    }
    let disks = &request.data_disks;
    if disks.len() > MAX_DATA_DISKS {
        return Err("data disks: one, or two for a mirror".into());
    }
    if !disks.iter().all(|disk| data_disk_ok(disk)) {
        return Err("a data disk must be a device of the new host: `/dev/…`".into());
    }
    if disks.len() == 2 && disks[0] == disks[1] {
        return Err("a mirror needs two different disks".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use yulius_contracts::migrate::{MoveAction, MoveRequest, VERSION};

    use super::{check_request, data_disk_ok, destination_ok, host_part, valid_id};

    fn plan() -> MoveRequest {
        MoveRequest {
            version: VERSION,
            id: "0123abcd-plan".into(),
            action: MoveAction::Plan,
            target: Some("root@new.example".into()),
            ssh_port: Some(22),
            public_url: None,
            data_disks: Vec::new(),
            requested_at: 1,
        }
    }

    fn refused(change: impl FnOnce(&mut MoveRequest)) -> String {
        let mut request = plan();
        change(&mut request);
        check_request(&request).err().unwrap_or_default()
    }

    /// MVB-002: what a plan and a move may name.
    #[test]
    fn a_plan_names_a_host_a_port_a_url_and_at_most_two_disks() {
        assert_eq!(check_request(&plan()), Ok(()));
        for action in [MoveAction::Plan, MoveAction::Move] {
            let full = MoveRequest {
                action,
                target: Some("new-host".into()),
                ssh_port: Some(65535),
                public_url: Some("https://yulius.example.com".into()),
                data_disks: vec!["/dev/nvme0n1".into(), "/dev/disk/by-id/nvme-1.2".into()],
                ..plan()
            };
            assert_eq!(check_request(&full), Ok(()), "{action:?}");
        }
        for target in [
            "185.189.47.160",
            "root@[2001:db8::1]",
            "root@new_host.internal",
        ] {
            assert_eq!(refused(|r| r.target = Some(target.into())), "", "{target}");
        }
    }

    /// MVB-002: a target that is not `[root@]host`, as `ssh` and the command take it.
    #[test]
    fn a_target_that_is_not_root_at_a_host_is_refused() {
        assert_eq!(refused(|r| r.target = None), "the new host is missing");
        for target in [
            "",
            "-oProxyCommand=x",
            "root@-host",
            "root@",
            "@host",
            "root@new host",
            "root@host;id",
            "root@host$(id)",
            "root@host\n",
            "a@b@c",
            "ro:ot@host",
        ] {
            assert_eq!(
                refused(|r| r.target = Some(target.into())),
                "the new host must be `root@host` or `host`",
                "{target:?}"
            );
        }
        for target in [
            "admin@new.example",
            "yulius@new.example",
            "Root@new.example",
        ] {
            assert_eq!(
                refused(|r| r.target = Some(target.into())),
                "the new host is reached as root: `root@host`",
                "{target}"
            );
        }
        // 255 bytes at most.
        let long = format!("root@{}", "a".repeat(250));
        assert_eq!(long.len(), 255);
        assert_eq!(refused(|r| r.target = Some(long.clone())), "");
        assert_eq!(
            refused(|r| r.target = Some(format!("{long}a"))),
            "the new host must be `root@host` or `host`"
        );
    }

    #[test]
    fn a_port_a_url_or_a_disk_outside_the_rules_is_refused() {
        const PORT: &str = "the SSH port must be 1 to 65535";
        assert_eq!(refused(|r| r.ssh_port = None), PORT);
        assert_eq!(refused(|r| r.ssh_port = Some(0)), PORT);
        assert_eq!(refused(|r| r.ssh_port = Some(1)), "");

        const URL: &str = "the public URL must be `https://host`, with nothing after it";
        for url in [
            "http://new.example",
            "https://new.example/",
            "https://new.example/path",
            "https://new.example:8443",
            "https://",
            "https://-x",
            "new.example",
            "--yes",
        ] {
            assert_eq!(refused(|r| r.public_url = Some(url.into())), URL, "{url}");
        }
        let long = format!("https://{}", "a".repeat(247));
        assert_eq!(long.len(), 255);
        assert_eq!(refused(|r| r.public_url = Some(long.clone())), "");
        assert_eq!(refused(|r| r.public_url = Some(format!("{long}a"))), URL);

        const DISK: &str = "a data disk must be a device of the new host: `/dev/…`";
        for disk in [
            "",
            "/dev/",
            "vdc",
            "/etc/passwd",
            "/dev/../etc/passwd",
            "/dev/vd c",
            "/dev/vdc;id",
            "--yes",
            "/dev/vdc\n",
            "/dev/vdc/",
            "/dev//vdc",
        ] {
            assert!(!data_disk_ok(disk), "{disk:?}");
            assert_eq!(
                refused(|r| r.data_disks = vec![disk.into()]),
                DISK,
                "{disk:?}"
            );
        }
        let long = format!("/dev/{}", "a".repeat(123));
        assert_eq!(long.len(), 128);
        assert!(data_disk_ok(&long));
        assert!(!data_disk_ok(&format!("{long}a")));
        assert_eq!(
            refused(|r| r.data_disks = vec!["/dev/a".into(), "/dev/b".into(), "/dev/c".into()]),
            "data disks: one, or two for a mirror"
        );
        assert_eq!(
            refused(|r| r.data_disks = vec!["/dev/a".into(), "/dev/a".into()]),
            "a mirror needs two different disks"
        );
    }

    /// MVB-002: an abort is of the recorded move, whatever it would name.
    #[test]
    fn an_abort_names_nothing() {
        let abort = MoveRequest {
            action: MoveAction::Abort,
            target: None,
            ssh_port: None,
            public_url: None,
            data_disks: Vec::new(),
            ..plan()
        };
        assert_eq!(check_request(&abort), Ok(()));
        const WHY: &str = "an abort names no host, port, URL or disk";
        let changes: [fn(&mut MoveRequest); 4] = [
            |r| r.target = Some("root@new.example".into()),
            |r| r.ssh_port = Some(22),
            |r| r.public_url = Some("https://new.example".into()),
            |r| r.data_disks = vec!["/dev/vdc".into()],
        ];
        for change in changes {
            let mut named = abort.clone();
            change(&mut named);
            assert_eq!(check_request(&named), Err(WHY.to_owned()));
        }
    }

    #[test]
    fn a_request_id_is_8_to_64_plain_characters() {
        for good in ["12345678", "a-B-9-z-0", &"x".repeat(64)] {
            assert!(valid_id(good), "{good}");
        }
        for bad in [
            "",
            "1234567",
            &"x".repeat(65),
            "abc_defgh",
            "abc defgh",
            "abcdefg/",
            "abcdefgh\n",
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    /// What `ssh` takes as `[user@]host` (SPEC-460 MIG-010).
    #[test]
    fn a_destination_is_a_host_with_an_optional_user() {
        for good in [
            "new-host",
            "root@new-host",
            "root@10.0.0.1",
            "root@[::1]",
            "a.b_c",
        ] {
            assert!(destination_ok(good), "{good}");
        }
        for bad in [
            "", "-x", "root@", "@x", "a b", "a@b@c", "[x]@h", "a:b@h", "x\ty",
        ] {
            assert!(!destination_ok(bad), "{bad:?}");
        }
        assert_eq!(host_part("root@185.189.47.160"), "185.189.47.160");
        assert_eq!(host_part("new-host"), "new-host");
    }
}
