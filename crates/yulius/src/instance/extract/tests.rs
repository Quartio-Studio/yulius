use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::sync::atomic::AtomicBool;

use super::*;
use crate::instance::archive::{Item, Reader, Writer};
use crate::instance::export::{plan, send};
use crate::instance::{Manifest, walk};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn own() -> (u32, u32) {
    (
        nix::unistd::geteuid().as_raw(),
        nix::unistd::getegid().as_raw(),
    )
}

fn same_ids() -> IdMap {
    let (uid, gid) = own();
    IdMap {
        uid: (uid, uid),
        gid: (gid, gid),
        subuid: None,
        subgid: None,
    }
}

fn root() -> bool {
    crate::instance::is_root()
}

/// Feeds every `data` entry of `bytes` to an extractor into `dst`.
fn install(bytes: &[u8], dst: &Path, map: IdMap) -> io::Result<Report> {
    let mut ex = Extractor::new(dst, map, root())?;
    let mut r = Reader::new(bytes);
    loop {
        match r.next_item()? {
            Item::End => return ex.finish(),
            Item::Entry(e) if e.path.starts_with(b"data") => {
                ex.put(&e, &mut r.data_reader())?;
            }
            Item::Entry(_) => {}
        }
    }
}

/// An archive of hand-made entries (no manifest).
fn archive(entries: &[(Entry, &[u8])]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out);
    for (e, data) in entries {
        let mut data = *data;
        w.entry(e, Some(&mut data), |_| {})?;
    }
    w.finish()?;
    Ok(out)
}

fn entry(path: &str, kind: Kind) -> Entry {
    let (uid, gid) = own();
    Entry {
        path: path.as_bytes().to_vec(),
        kind,
        mode: 0o755,
        uid,
        gid,
        mtime: 1_700_000_000,
        xattrs: Vec::new(),
    }
}

fn listing(root: &Path) -> io::Result<Vec<Entry>> {
    let mut seen = Vec::new();
    walk::walk(root, &mut |e, _| {
        seen.push(e.clone());
        Ok(())
    })?;
    Ok(seen)
}

#[test]
fn an_exported_tree_installs_as_it_was() -> TestResult {
    let src = tempfile::tempdir()?;
    let s = src.path();
    fs::create_dir_all(s.join("sandboxes/sb/repo/.git"))?;
    fs::create_dir_all(s.join("sysroots/sb/upper/etc"))?;
    fs::write(s.join("yulius.db"), vec![3u8; 100_000])?;
    fs::write(s.join("master.key"), b"key")?;
    fs::set_permissions(s.join("master.key"), fs::Permissions::from_mode(0o600))?;
    fs::write(s.join("sandboxes/sb/repo/run.sh"), b"#!/bin/sh\n")?;
    fs::set_permissions(
        s.join("sandboxes/sb/repo/run.sh"),
        fs::Permissions::from_mode(0o750),
    )?;
    fs::hard_link(
        s.join("sandboxes/sb/repo/run.sh"),
        s.join("sandboxes/sb/repo/.git/same"),
    )?;
    symlink("../repo/run.sh", s.join("sandboxes/sb/link"))?;
    symlink("/nowhere/at/all", s.join("sandboxes/sb/dangling"))?;
    let xattr = rustix::fs::lsetxattr(
        s.join("sysroots/sb/upper/etc"),
        "user.overlay.opaque",
        b"y",
        XattrFlags::empty(),
    )
    .is_ok();
    let stop = AtomicBool::new(false);
    let p = plan(s, &stop)?;
    let mut bytes = Vec::new();
    send(s, &p, &Manifest::default(), &mut bytes, &stop, &mut |_| {})?;

    let dst = tempfile::tempdir()?;
    let report = install(&bytes, dst.path(), same_ids())?;
    assert_eq!(report.entries, p.entries);
    assert_eq!(report.foreign_owners, 0);
    assert_eq!(listing(dst.path())?, listing(s)?);
    let a = fs::metadata(dst.path().join("sandboxes/sb/repo/run.sh"))?;
    let b = fs::metadata(dst.path().join("sandboxes/sb/repo/.git/same"))?;
    assert_eq!(a.ino(), b.ino(), "hard link kept");
    assert_eq!(fs::read(dst.path().join("yulius.db"))?, vec![3u8; 100_000]);
    if xattr {
        assert!(
            listing(dst.path())?
                .iter()
                .any(|e| e.xattrs == [(b"user.overlay.opaque".to_vec(), b"y".to_vec())])
        );
    }
    Ok(())
}

#[test]
fn paths_outside_the_data_directory_are_refused() {
    for path in [
        "../x",
        "/etc/x",
        "data/../x",
        "data//x",
        "data/./x",
        "other/x",
        "datax",
    ] {
        assert!(components(path.as_bytes()).is_err(), "{path}");
    }
    assert!(components(b"data").is_ok_and(|c| c.is_empty()));
    assert!(components(b"data/a/b").is_ok_and(|c| c.len() == 2));
}

#[test]
fn nothing_is_written_through_a_link_of_the_copy() -> TestResult {
    let outside = tempfile::tempdir()?;
    let dst = tempfile::tempdir()?;
    let target = outside.path().to_string_lossy().into_owned();
    let bytes = archive(&[
        (entry("data", Kind::Dir), b""),
        (
            entry(
                "data/l",
                Kind::Symlink {
                    target: target.clone().into_bytes(),
                },
            ),
            b"",
        ),
        (entry("data/l/evil", Kind::File { size: 4 }), b"evil"),
    ])?;
    let err = install(&bytes, dst.path(), same_ids())
        .err()
        .ok_or("expected an error")?;
    assert!(err.to_string().contains("refused"), "{err}");
    assert_eq!(fs::read_dir(outside.path())?.count(), 0);
    Ok(())
}

#[test]
fn a_link_already_in_the_data_directory_is_replaced_not_followed() -> TestResult {
    let outside = tempfile::tempdir()?;
    let dst = tempfile::tempdir()?;
    symlink(outside.path(), dst.path().join("sandboxes"))?;
    let bytes = archive(&[
        (entry("data", Kind::Dir), b""),
        (entry("data/sandboxes", Kind::Dir), b""),
        (entry("data/sandboxes/f", Kind::File { size: 2 }), b"ok"),
    ])?;
    install(&bytes, dst.path(), same_ids())?;
    assert_eq!(fs::read_dir(outside.path())?.count(), 0);
    assert!(fs::symlink_metadata(dst.path().join("sandboxes"))?.is_dir());
    assert_eq!(fs::read(dst.path().join("sandboxes/f"))?, b"ok");
    Ok(())
}

#[test]
fn a_hard_link_must_name_a_file_of_the_copy() -> TestResult {
    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("secret"), b"s")?;
    let target = outside.path().to_string_lossy().into_owned();
    for link in [
        "data/l/secret",
        "data/missing",
        "data/d",
        "data/../etc/passwd",
    ] {
        let dst = tempfile::tempdir()?;
        let bytes = archive(&[
            (entry("data", Kind::Dir), b""),
            (entry("data/d", Kind::Dir), b""),
            (
                entry(
                    "data/l",
                    Kind::Symlink {
                        target: target.clone().into_bytes(),
                    },
                ),
                b"",
            ),
            (
                entry(
                    "data/h",
                    Kind::HardLink {
                        target: link.as_bytes().to_vec(),
                    },
                ),
                b"",
            ),
        ])?;
        let err = install(&bytes, dst.path(), same_ids())
            .err()
            .ok_or("expected an error")?;
        assert!(err.to_string().contains("refused"), "{link}: {err}");
        assert!(
            fs::symlink_metadata(dst.path().join("h")).is_err(),
            "{link}"
        );
    }
    assert_eq!(fs::metadata(outside.path().join("secret"))?.nlink(), 1);
    Ok(())
}

#[test]
fn a_foreign_owner_becomes_the_service_user_without_setuid() -> TestResult {
    let (uid, gid) = own();
    let dst = tempfile::tempdir()?;
    // The exporting service was 4242; this file belonged to root there.
    let map = IdMap {
        uid: (4242, uid),
        gid: (4242, gid),
        subuid: None,
        subgid: None,
    };
    let mut setuid = entry("data/sudo", Kind::File { size: 1 });
    (setuid.uid, setuid.gid, setuid.mode) = (0, 0, 0o4755);
    let mut mine = entry("data/mine", Kind::File { size: 1 });
    (mine.uid, mine.gid, mine.mode) = (4242, 4242, 0o2750);
    let mut root_dir = entry("data", Kind::Dir);
    (root_dir.uid, root_dir.gid, root_dir.mode) = (4242, 4242, 0o700);
    let bytes = archive(&[(root_dir, b""), (setuid, b"x"), (mine, b"y")])?;
    let report = install(&bytes, dst.path(), map)?;
    assert_eq!(report.foreign_owners, 1);
    let sudo = fs::metadata(dst.path().join("sudo"))?;
    assert_eq!(sudo.mode() & 0o7777, 0o755);
    assert_eq!(sudo.uid(), uid);
    assert_eq!(
        fs::metadata(dst.path().join("mine"))?.mode() & 0o7777,
        0o2750
    );
    Ok(())
}

#[test]
fn only_user_and_overlay_attributes_are_restored() -> TestResult {
    let dst = tempfile::tempdir()?;
    let mut f = entry("data/f", Kind::File { size: 1 });
    f.xattrs = vec![
        (b"security.capability".to_vec(), vec![1, 2]),
        (b"system.posix_acl_access".to_vec(), vec![2]),
        (b"trusted.evil".to_vec(), vec![3]),
        (b"user.kept".to_vec(), b"v".to_vec()),
    ];
    let bytes = archive(&[(entry("data", Kind::Dir), b""), (f, b"z")])?;
    let report = install(&bytes, dst.path(), same_ids())?;
    let user_ok = report.skipped_xattrs == 3;
    assert!(
        report.skipped_xattrs == 3 || report.skipped_xattrs == 4,
        "{report:?}"
    );
    let attrs = walk::xattrs(&dst.path().join("f"))?;
    assert!(
        attrs.iter().all(|(n, _)| n.starts_with(b"user.")),
        "{attrs:?}"
    );
    if user_ok {
        assert_eq!(attrs, [(b"user.kept".to_vec(), b"v".to_vec())]);
    }
    Ok(())
}

#[test]
fn ids_move_by_service_and_range_offset() {
    let map = IdMap {
        uid: (998, 1001),
        gid: (997, 1002),
        subuid: Some((100_000, 300_000, 65_536)),
        subgid: Some((100_000, 400_000, 65_536)),
    };
    assert_eq!(map.uid(998), Some(1001));
    assert_eq!(map.uid(100_000), Some(300_000));
    assert_eq!(map.uid(165_535), Some(365_535));
    assert_eq!(map.uid(165_536), None);
    assert_eq!(map.uid(99_999), None);
    assert_eq!(map.uid(0), None);
    assert_eq!(map.gid(997), Some(1002));
    assert_eq!(map.gid(100_001), Some(400_001));
}
