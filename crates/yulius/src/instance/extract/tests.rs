use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};

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
    let p = plan(s)?;
    let mut bytes = Vec::new();
    send(
        s,
        &p,
        &Manifest::default(),
        &mut bytes,
        &mut |_| {},
        &mut || Ok(()),
    )?;

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

#[test]
fn a_hard_link_to_a_file_already_there_is_refused() -> TestResult {
    let dst = tempfile::tempdir()?;
    fs::write(dst.path().join("old"), b"not from the copy")?;
    let bytes = archive(&[
        (entry("data", Kind::Dir), b""),
        (
            entry(
                "data/h",
                Kind::HardLink {
                    target: b"data/old".to_vec(),
                },
            ),
            b"",
        ),
    ])?;
    let err = install(&bytes, dst.path(), same_ids())
        .err()
        .ok_or("expected an error")?;
    assert!(err.to_string().contains("not in the copy"), "{err}");
    assert_eq!(fs::metadata(dst.path().join("old"))?.nlink(), 1);
    Ok(())
}

// Onto a move's staged copy (SPEC-465 WCP-008, WCP-009).

/// Puts hand-made entries onto `dst`; the entries left out, and the extractor before it ends.
fn put_onto(dst: &Path, onto: Onto, entries: &[(Entry, &[u8])]) -> io::Result<Extractor> {
    let mut ex = Extractor::onto(dst, same_ids(), root(), onto)?;
    for (e, data) in entries {
        let mut data = *data;
        ex.put(e, &mut data)?;
    }
    Ok(ex)
}

fn file(path: &str, data: &[u8]) -> Entry {
    entry(
        path,
        Kind::File {
            size: data.len() as u64,
        },
    )
}

/// A staged copy as a first pass left it.
fn staged() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dst = tempfile::tempdir()?;
    put_onto(
        dst.path(),
        Onto::Warm,
        &[
            (entry("data", Kind::Dir), b""),
            (entry("data/dir", Kind::Dir), b""),
            (file("data/dir/a", b"one"), b"one"),
            (file("data/dir/b", b"bee"), b"bee"),
            (entry("data/dir/sub", Kind::Dir), b""),
            (file("data/dir/sub/deep", b"deep"), b"deep"),
            (file("data/top", b"top"), b"top"),
        ],
    )?
    .finish()?;
    Ok(dst)
}

#[test]
fn a_directory_that_is_not_sent_keeps_its_time_when_a_file_in_it_is_rewritten() -> TestResult {
    let dst = staged()?;
    let d = dst.path();
    assert_eq!(fs::metadata(d.join("dir"))?.mtime(), 1_700_000_000);
    // Only the file changed on the old host: its directory is not in the stream.
    let mut newer = file("data/dir/a", b"two!");
    newer.mtime = 1_700_000_500;
    let ex = put_onto(d, Onto::Warm, &[(newer, b"two!")])?;
    assert!(ex.deferred().is_empty());
    ex.finish()?;
    assert_eq!(fs::read(d.join("dir/a"))?, b"two!");
    assert_eq!(fs::metadata(d.join("dir/a"))?.mtime(), 1_700_000_500);
    assert_eq!(fs::metadata(d.join("dir"))?.mtime(), 1_700_000_000);
    assert_eq!(fs::metadata(d.join("dir/sub"))?.mtime(), 1_700_000_000);

    // A directory that is sent takes its entry's time.
    let mut dir = entry("data/dir", Kind::Dir);
    dir.mtime = 1_700_000_900;
    put_onto(
        d,
        Onto::Warm,
        &[(dir, b""), (file("data/dir/new", b"n"), b"n")],
    )?
    .finish()?;
    assert_eq!(fs::metadata(d.join("dir"))?.mtime(), 1_700_000_900);
    // And what was there stays: a warm pass removes nothing.
    assert_eq!(fs::read(d.join("dir/b"))?, b"bee");
    Ok(())
}

#[test]
fn a_file_becomes_a_directory_in_any_pass_and_a_directory_a_file_only_frozen() -> TestResult {
    let dst = staged()?;
    let d = dst.path();
    // `top` is a directory now, with a file in it.
    let ex = put_onto(
        d,
        Onto::Warm,
        &[
            (entry("data/top", Kind::Dir), b""),
            (file("data/top/in", b"in"), b"in"),
            // `dir` is a file now: a warm pass does not remove what is below it.
            (file("data/dir", b"file"), b"file"),
            (file("data/zed", b"z"), b"z"),
        ],
    )?;
    assert_eq!(ex.deferred(), [2]);
    ex.finish()?;
    assert_eq!(fs::read(d.join("top/in"))?, b"in");
    assert!(d.join("dir").is_dir());
    assert_eq!(fs::read(d.join("dir/sub/deep"))?, b"deep");
    assert_eq!(fs::read(d.join("zed"))?, b"z");

    // The frozen pass replaces it, with everything below.
    for kind in [
        Kind::Symlink {
            target: b"zed".to_vec(),
        },
        Kind::File { size: 4 },
    ] {
        fs::create_dir_all(d.join("dir/sub"))?;
        fs::write(d.join("dir/sub/deep"), b"deep")?;
        let is_link = matches!(kind, Kind::Symlink { .. });
        let ex = put_onto(d, Onto::Frozen, &[(entry("data/dir", kind), b"file")])?;
        assert!(ex.deferred().is_empty());
        ex.finish()?;
        let meta = fs::symlink_metadata(d.join("dir"))?;
        assert_eq!((meta.is_symlink(), meta.is_file()), (is_link, !is_link));
        fs::remove_file(d.join("dir"))?;
    }

    // A new copy still refuses it: nothing may be there.
    fs::create_dir(d.join("dir"))?;
    let refused = put_onto(d, Onto::New, &[(file("data/dir", b"file"), b"file")]);
    assert!(refused.is_err_and(|e| {
        e.to_string()
            .contains("is a directory where the copy has a file")
    }));
    Ok(())
}

#[test]
fn a_hard_link_may_name_a_file_of_an_earlier_pass() -> TestResult {
    let dst = staged()?;
    let d = dst.path();
    let link = |target: &str| {
        entry(
            "data/dir/link",
            Kind::HardLink {
                target: target.as_bytes().to_vec(),
            },
        )
    };
    // A new copy knows only the files of its own stream.
    assert!(put_onto(d, Onto::New, &[(link("data/dir/a"), b"")]).is_err());
    for onto in [Onto::Warm, Onto::Frozen] {
        put_onto(d, onto, &[(link("data/dir/a"), b"")])?.finish()?;
        assert_eq!(
            fs::metadata(d.join("dir/link"))?.ino(),
            fs::metadata(d.join("dir/a"))?.ino()
        );
        // Linking changed the directory, which was not sent: it keeps its time.
        assert_eq!(fs::metadata(d.join("dir"))?.mtime(), 1_700_000_000);
        // The test's own removal moves the time; put it back for the next round.
        fs::remove_file(d.join("dir/link"))?;
        fs::File::open(d.join("dir"))?
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000))?;
    }

    // Sent again, the file is a new one: the link sent with it names the new one, and the
    // earlier link keeps the old content until it is sent too.
    fs::hard_link(d.join("dir/a"), d.join("dir/old-link"))?;
    put_onto(
        d,
        Onto::Warm,
        &[
            (file("data/dir/a", b"new"), b"new"),
            (link("data/dir/a"), b""),
        ],
    )?
    .finish()?;
    assert_eq!(fs::read(d.join("dir/link"))?, b"new");
    assert_eq!(fs::read(d.join("dir/old-link"))?, b"one");

    // Never to this host's own files, a missing file, or outside.
    fs::create_dir(d.join("system"))?;
    fs::write(d.join("system/base"), b"host")?;
    for target in [
        "data/system/base",
        "data/dir/none",
        "data/../etc/passwd",
        "data",
    ] {
        let refused = put_onto(d, Onto::Warm, &[(link(target), b"")]);
        assert!(refused.is_err(), "{target}");
    }
    // What is at the link's place stays when its link is refused.
    assert_eq!(fs::read(d.join("dir/link"))?, b"new");
    Ok(())
}

#[test]
fn a_link_whose_file_waits_for_the_frozen_pass_waits_with_it() -> TestResult {
    let dst = staged()?;
    let d = dst.path();
    // On the old host `dir/sub` is a file with a second name `dir/b`; here it is a directory.
    let entries = [
        (file("data/dir/sub", b"file"), &b"file"[..]),
        (
            entry(
                "data/dir/z",
                Kind::HardLink {
                    target: b"data/dir/sub".to_vec(),
                },
            ),
            &b""[..],
        ),
    ];
    let ex = put_onto(d, Onto::Warm, &entries)?;
    assert_eq!(ex.deferred(), [0, 1]);
    ex.finish()?;
    assert!(d.join("dir/sub").is_dir() && !d.join("dir/z").exists());
    let ex = put_onto(d, Onto::Frozen, &entries)?;
    assert!(ex.deferred().is_empty());
    ex.finish()?;
    assert_eq!(fs::read(d.join("dir/z"))?, b"file");
    assert_eq!(fs::metadata(d.join("dir/z"))?.nlink(), 2);
    Ok(())
}

#[test]
fn a_directory_sent_again_loses_the_attributes_its_entry_no_longer_carries() -> TestResult {
    let dst = staged()?;
    let d = dst.path();
    let mut with = entry("data/dir", Kind::Dir);
    with.xattrs = vec![
        (b"user.keep".to_vec(), b"1".to_vec()),
        (b"user.drop".to_vec(), b"2".to_vec()),
    ];
    let report = put_onto(d, Onto::Warm, &[(with.clone(), b"")])?.finish()?;
    if report.skipped_xattrs > 0 {
        // A filesystem without user attributes: nothing to keep or drop.
        return Ok(());
    }
    assert_eq!(walk::xattrs(&d.join("dir"))?.len(), 2);
    with.xattrs.pop();
    put_onto(d, Onto::Warm, &[(with.clone(), b"")])?.finish()?;
    assert_eq!(walk::xattrs(&d.join("dir"))?, with.xattrs);
    // The data directory itself too.
    let mut top = entry("data", Kind::Dir);
    top.xattrs = vec![(b"user.top".to_vec(), b"t".to_vec())];
    put_onto(d, Onto::Frozen, &[(top.clone(), b"")])?.finish()?;
    top.xattrs.clear();
    put_onto(d, Onto::Frozen, &[(top, b"")])?.finish()?;
    assert!(walk::xattrs(d)?.is_empty());
    Ok(())
}

#[test]
fn the_frozen_pass_removes_what_the_old_host_no_longer_holds() -> TestResult {
    let dst = staged()?;
    let d = dst.path();
    // This host's own, which no copy carries.
    fs::create_dir(d.join("system"))?;
    fs::write(d.join("system/base"), b"host")?;
    fs::write(d.join(".marker"), b"")?;
    // A link out of the copy that an earlier pass brought: removed, never followed.
    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("f"), b"outside")?;
    symlink(outside.path(), d.join("dir/sub/out"))?;

    // The old host holds `dir` with `a`, and `top`; `b`, `sub` and all below are gone.
    let mut kept: Vec<[u8; 16]> = ["data", "data/dir", "data/dir/a", "data/top"]
        .iter()
        .map(|p| path_hash(p.as_bytes()))
        .collect();
    kept.sort_unstable();
    let mut ex = put_onto(d, Onto::Frozen, &[])?;
    let spared = |name: &[u8]| name == b"system" || name == b".marker";
    let removed = ex.prune(&kept, &spared)?;
    ex.finish()?;
    // b, sub, sub/deep, sub/out.
    assert_eq!(removed, 4);
    assert!(!d.join("dir/b").exists() && !d.join("dir/sub").exists());
    assert_eq!(fs::read(d.join("dir/a"))?, b"one");
    assert_eq!(fs::read(d.join("top"))?, b"top");
    assert_eq!(fs::read(d.join("system/base"))?, b"host");
    assert!(d.join(".marker").exists());
    assert_eq!(fs::read(outside.path().join("f"))?, b"outside");
    // `dir` was not sent: removing below it did not move its time.
    assert_eq!(fs::metadata(d.join("dir"))?.mtime(), 1_700_000_000);

    // A top-level name the old host no longer holds goes too, unless it is spared.
    kept.retain(|digest| *digest != path_hash(b"data/top"));
    let mut ex = put_onto(d, Onto::Frozen, &[])?;
    assert_eq!(ex.prune(&kept, &spared)?, 1);
    ex.finish()?;
    assert!(!d.join("top").exists() && d.join("system/base").exists());
    Ok(())
}
