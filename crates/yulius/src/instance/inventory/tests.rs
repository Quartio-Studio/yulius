use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use super::*;
use crate::instance::archive::Kind;
use crate::instance::export::{plan_observed, send};
use crate::instance::extract::IdMap;
use crate::instance::import::install;
use crate::instance::{FORMAT, IdRange, Manifest, Service};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tree(root: &Path) -> TestResult {
    fs::create_dir_all(root.join("sandboxes/sb/home"))?;
    fs::create_dir_all(root.join("sysroots/sb/upper"))?;
    fs::create_dir_all(root.join("system/base-1"))?;
    fs::write(root.join("yulius.db"), vec![7u8; 70_000])?;
    fs::write(root.join("master.key"), b"k")?;
    fs::write(root.join("sandboxes/sb/home/notes"), b"hello\n")?;
    fs::hard_link(
        root.join("sandboxes/sb/home/notes"),
        root.join("sandboxes/sb/home/notes-again"),
    )?;
    std::os::unix::fs::symlink("notes", root.join("sandboxes/sb/home/link"))?;
    fs::write(root.join("sysroots/sb/upper/tool"), vec![1u8; 1500])?;
    fs::set_permissions(
        root.join("sysroots/sb/upper/tool"),
        fs::Permissions::from_mode(0o755),
    )?;
    fs::write(root.join("system/base-1/big"), vec![1u8; 10_000])?;
    Ok(())
}

/// The manifest of a tree this user owns, to be installed at `target`.
fn manifest(root: &Path, target: &Path) -> Result<Manifest, Box<dyn std::error::Error>> {
    let meta = fs::metadata(root)?;
    Ok(Manifest {
        format: FORMAT,
        data_dir: target.display().to_string(),
        service: Service {
            unit: "yulius.service".into(),
            user: "yulius".into(),
            uid: meta.uid(),
            gid: meta.gid(),
        },
        ..Manifest::default()
    })
}

/// Exports `root`, installs it at `target` and returns the exporting side's inventory.
fn copy(root: &Path, target: &Path) -> Result<(Manifest, Inventory), Box<dyn std::error::Error>> {
    let m = manifest(root, target)?;
    let mut counter = Counter::new(IdSpace::of(&m));
    let plan = plan_observed(root, &mut |e| counter.add(e))?;
    let mut bytes = Vec::new();
    send(root, &plan, &m, &mut bytes, &mut |_| {}, &mut || Ok(()))?;
    let (uid, gid) = (m.service.uid, m.service.gid);
    install(
        bytes.as_slice(),
        target,
        None,
        &mut |_, _| {
            Ok(IdMap {
                uid: (uid, uid),
                gid: (gid, gid),
                subuid: None,
                subgid: None,
            })
        },
        false,
        &mut |_, _| {},
    )
    .map_err(|e| format!("{e:?}"))?;
    Ok((m, counter.finish()))
}

fn same_ids(m: &Manifest) -> IdMap {
    IdMap {
        uid: (m.service.uid, m.service.uid),
        gid: (m.service.gid, m.service.gid),
        subuid: None,
        subgid: None,
    }
}

#[test]
fn a_copy_has_its_sources_inventory() -> TestResult {
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    tree(src.path())?;
    let target = dst.path().join("data");
    let (m, old) = copy(src.path(), &target)?;
    let new = of_copy(&target, &same_ids(&m), &old)?;
    assert_eq!(difference(&old, &new), None, "{old:?}\n{new:?}");
    assert_eq!(old, new);
    // The folders in archive order, the base left out, a hard link counted once.
    let names: Vec<&str> = old.folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [".", "master.key", "sandboxes", "sysroots", "yulius.db"]
    );
    let sandboxes = &old.folders[2];
    assert_eq!((sandboxes.entries, sandboxes.bytes), (6, 6));
    assert_eq!(old.entries, 13);
    assert_eq!(old.bytes, 70_000 + 1 + 6 + 1500);
    Ok(())
}

#[test]
fn a_file_that_differs_names_its_folder() -> TestResult {
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    tree(src.path())?;
    let target = dst.path().join("data");
    let (m, old) = copy(src.path(), &target)?;

    // One byte more in a file.
    fs::write(target.join("sysroots/sb/upper/tool"), vec![1u8; 1501])?;
    let new = of_copy(&target, &same_ids(&m), &old)?;
    let why = difference(&old, &new).ok_or("no difference")?;
    assert!(why.starts_with("sysroots: "), "{why}");
    assert!(
        why.contains("4 entries, 1500 bytes on the old host")
            && why.contains("4 entries, 1501 bytes in the copy"),
        "{why}"
    );

    // The same bytes with another mode: only the fingerprint tells.
    fs::write(target.join("sysroots/sb/upper/tool"), vec![1u8; 1500])?;
    fs::set_permissions(
        target.join("sysroots/sb/upper/tool"),
        fs::Permissions::from_mode(0o700),
    )?;
    let new = of_copy(&target, &same_ids(&m), &old)?;
    let why = difference(&old, &new).ok_or("no difference")?;
    assert!(
        why.starts_with("sysroots: ") && why.contains("owner, mode, time"),
        "{why}"
    );
    Ok(())
}

#[test]
fn a_missing_or_extra_entry_is_a_difference() -> TestResult {
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    tree(src.path())?;
    let target = dst.path().join("data");
    let (m, old) = copy(src.path(), &target)?;
    fs::remove_file(target.join("master.key"))?;
    let new = of_copy(&target, &same_ids(&m), &old)?;
    let why = difference(&old, &new).ok_or("no difference")?;
    assert!(
        why.starts_with("master.key: ") && why.contains("missing"),
        "{why}"
    );

    let target = dst.path().join("again");
    let (m, old) = copy(src.path(), &target)?;
    fs::write(target.join("sandboxes/sb/home/extra"), b"x")?;
    let new = of_copy(&target, &same_ids(&m), &old)?;
    let why = difference(&old, &new).ok_or("no difference")?;
    assert!(
        why.starts_with("sandboxes: ") && why.contains("7 entries, 7 bytes in the copy"),
        "{why}"
    );
    Ok(())
}

#[test]
fn what_the_new_host_adds_at_the_top_is_not_counted() -> TestResult {
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    tree(src.path())?;
    let target = dst.path().join("data");
    let (m, old) = copy(src.path(), &target)?;
    // A skeleton dotfile, a mark of the import, the sandbox base: the new host's own.
    fs::write(target.join(".bash_logout"), b"# skel\n")?;
    fs::write(target.join(".yulius-limits-adapt"), b"")?;
    fs::create_dir_all(target.join("system/base-2"))?;
    let new = of_copy(&target, &same_ids(&m), &old)?;
    assert_eq!(difference(&old, &new), None);
    Ok(())
}

/// SPEC-467 LFT-005: an import whose first start never consumed its limits mark leaves it on
/// what later becomes the old host of a move. The import writes its own on the new host, at
/// another time: were the mark part of the copy, the proof would fail after the whole copy.
#[test]
fn a_limits_mark_left_on_the_old_host_is_not_the_copy() -> TestResult {
    const MARK: &str = yulius_app::limits::ADAPT_MARK;
    let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    tree(src.path())?;
    fs::write(src.path().join(MARK), b"")?;
    fs::File::options()
        .write(true)
        .open(src.path().join(MARK))?
        .set_modified(long_ago)?;
    let target = dst.path().join("data");
    let (m, old) = copy(src.path(), &target)?;
    // The import's own mark is there, written now.
    let installed = fs::symlink_metadata(target.join(MARK))?;
    assert!(installed.is_file() && installed.mtime() != 1_700_000_000);
    let new = of_copy(&target, &same_ids(&m), &old)?;
    assert_eq!(difference(&old, &new), None, "{old:?}\n{new:?}");
    // Counted on neither host: the same folders and entries as without it.
    let names: Vec<&str> = old.folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [".", "master.key", "sandboxes", "sysroots", "yulius.db"]
    );
    assert_eq!((old.entries, new.entries), (13, 13));

    // Whatever sits at that name: a directory there travels as little, and the import still
    // ends with its own mark instead of failing on it after the whole copy.
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    tree(src.path())?;
    fs::create_dir_all(src.path().join(MARK).join("below"))?;
    let target = dst.path().join("data");
    let (m, old) = copy(src.path(), &target)?;
    assert!(fs::symlink_metadata(target.join(MARK))?.is_file());
    let new = of_copy(&target, &same_ids(&m), &old)?;
    assert_eq!(difference(&old, &new), None, "{old:?}\n{new:?}");
    assert_eq!(old.entries, 13);
    Ok(())
}

fn entry(path: &str, kind: Kind, uid: u32, gid: u32, mode: u32) -> Entry {
    Entry {
        path: path.as_bytes().to_vec(),
        kind,
        mode,
        uid,
        gid,
        mtime: 1_700_000_000,
        xattrs: vec![
            (b"security.capability".to_vec(), b"cap".to_vec()),
            (b"trusted.overlay.opaque".to_vec(), b"y".to_vec()),
            (b"user.note".to_vec(), b"n".to_vec()),
        ],
    }
}

fn old_host() -> IdSpace {
    IdSpace {
        uid: 998,
        gid: 997,
        subuid: Some(IdRange {
            start: 100_000,
            count: 65_536,
        }),
        subgid: Some(IdRange {
            start: 100_000,
            count: 65_536,
        }),
    }
}

fn new_host() -> IdMap {
    IdMap {
        uid: (998, 1001),
        gid: (997, 1002),
        subuid: Some((100_000, 300_000, 65_536)),
        subgid: Some((100_000, 400_000, 65_536)),
    }
}

#[test]
fn an_entry_reads_the_same_on_both_hosts_across_an_id_remap() -> TestResult {
    let old = entry(
        "data/sysroots/a/upper/bin/su",
        Kind::File { size: 3 },
        100_000,
        100_042,
        0o4755,
    );
    let mut new = old.clone();
    (new.uid, new.gid) = (300_000, 400_042);
    // The import restores only users' and overlay attributes.
    new.xattrs
        .retain(|(name, _)| name != b"security.capability");
    let there = as_exported(&new, &new_host()).ok_or("an id of the copy is foreign")?;
    assert_eq!(old_host().as_installed(&old), there);
    assert_eq!(there.uid, 100_000);
    assert_eq!(there.mode, 0o4755);
    assert_eq!(there.xattrs.len(), 2);
    Ok(())
}

#[test]
fn a_foreign_owner_reads_as_the_service_user_without_setuid() -> TestResult {
    // Root's file with setuid on the old host: the import gives it to the service user.
    let old = entry(
        "data/sandboxes/a/tool",
        Kind::File { size: 3 },
        0,
        100_005,
        0o6755,
    );
    let shown = old_host().as_installed(&old);
    assert_eq!((shown.uid, shown.gid, shown.mode), (998, 100_005, 0o755));

    let mut new = old.clone();
    (new.uid, new.gid, new.mode) = (1001, 400_005, 0o755);
    let there = as_exported(&new, &new_host()).ok_or("an id of the copy is foreign")?;
    assert_eq!(shown, there);

    // An id of neither the service nor its range cannot be in a copy.
    new.uid = 5;
    assert_eq!(as_exported(&new, &new_host()), None);
    Ok(())
}

#[test]
fn links_whiteouts_and_the_data_directorys_time_are_as_the_import_leaves_them() {
    let ids = old_host();
    let link = entry(
        "data/a/link",
        Kind::Symlink {
            target: b"x".to_vec(),
        },
        998,
        997,
        0o777,
    );
    assert!(ids.as_installed(&link).xattrs.is_empty());
    let whiteout = entry("data/a/gone", Kind::Whiteout, 998, 997, 0o600);
    assert!(ids.as_installed(&whiteout).xattrs.is_empty());
    // Removing the import's marker touches the data directory: its time is not compared.
    let top = entry("data", Kind::Dir, 998, 997, 0o700);
    assert_eq!(ids.as_installed(&top).mtime, 0);
    let below = entry("data/a", Kind::Dir, 998, 997, 0o700);
    assert_eq!(ids.as_installed(&below).mtime, 1_700_000_000);
}

#[test]
fn an_inventory_survives_its_json() -> TestResult {
    let src = tempfile::tempdir()?;
    tree(src.path())?;
    let m = manifest(src.path(), src.path())?;
    let mut counter = Counter::new(IdSpace::of(&m));
    plan_observed(src.path(), &mut |e| counter.add(e))?;
    let inventory = counter.finish();
    let back: Inventory = serde_json::from_str(&serde_json::to_string(&inventory)?)?;
    assert_eq!(inventory, back);
    assert_eq!(inventory.fingerprint.len(), 64);
    Ok(())
}

/// As the real import runs: as root, with owners remapped to another service user and another
/// subordinate range. The copy still reads as the old host's tree.
#[test]
fn a_copy_installed_with_remapped_owners_has_its_sources_inventory() -> TestResult {
    if !crate::instance::is_root() {
        eprintln!("skipped: needs root to give files to other ids");
        return Ok(());
    }
    use std::os::unix::fs::{chown, lchown};
    let (src, dst) = (tempfile::tempdir()?, tempfile::tempdir()?);
    let root = src.path();
    tree(root)?;
    // A sandbox's root has only its own ids: these are not mapped there.
    if chown(root.join("master.key"), Some(100_042), Some(5001)).is_err() {
        eprintln!("skipped: this root cannot give files to ids outside its namespace");
        return Ok(());
    }
    // The old host: service 5000:5001, its subordinate ids from 100000; one file of root's
    // with setuid, which the import gives to the service user without it.
    for entry in [
        "",
        "yulius.db",
        "master.key",
        "sandboxes",
        "sandboxes/sb",
        "sandboxes/sb/home",
        "sandboxes/sb/home/notes",
        "sysroots",
        "sysroots/sb",
    ] {
        chown(root.join(entry), Some(5000), Some(5001))?;
    }
    lchown(root.join("sandboxes/sb/home/link"), Some(5000), Some(5001))?;
    chown(root.join("sysroots/sb/upper"), Some(100_000), Some(100_000))?;
    chown(
        root.join("sysroots/sb/upper/tool"),
        Some(100_042),
        Some(100_007),
    )?;
    fs::write(root.join("sandboxes/sb/rootfile"), b"x")?;
    fs::set_permissions(
        root.join("sandboxes/sb/rootfile"),
        fs::Permissions::from_mode(0o4755),
    )?;
    let range = Some(IdRange {
        start: 100_000,
        count: 65_536,
    });
    let target = dst.path().join("data");
    let mut m = manifest(root, &target)?;
    (m.subuid, m.subgid) = (range, range);
    let mut counter = Counter::new(IdSpace::of(&m));
    let plan = plan_observed(root, &mut |e| counter.add(e))?;
    let old = counter.finish();
    let mut bytes = Vec::new();
    send(root, &plan, &m, &mut bytes, &mut |_| {}, &mut || Ok(()))?;
    // The new host: service 6000:6001, subordinate ids from 300000 and 400000.
    let map = IdMap {
        uid: (5000, 6000),
        gid: (5001, 6001),
        subuid: Some((100_000, 300_000, 65_536)),
        subgid: Some((100_000, 400_000, 65_536)),
    };
    fs::create_dir_all(&target)?;
    chown(&target, Some(6000), Some(6001))?;
    install(
        bytes.as_slice(),
        &target,
        Some("nobody-checks-the-name-here"),
        &mut |_, _| Ok(map),
        true,
        &mut |_, _| {},
    )
    .map_err(|e| format!("{e:?}"))?;
    let tool = fs::metadata(target.join("sysroots/sb/upper/tool"))?;
    assert_eq!((tool.uid(), tool.gid()), (300_042, 400_007));
    let folded = fs::metadata(target.join("sandboxes/sb/rootfile"))?;
    assert_eq!((folded.uid(), folded.mode() & 0o7777), (6000, 0o755));

    let new = of_copy(&target, &map, &old)?;
    assert_eq!(difference(&old, &new), None);
    // An owner the import never gives is not passed over.
    chown(target.join("master.key"), Some(77), Some(6001))?;
    assert!(of_copy(&target, &map, &old).is_err());
    Ok(())
}
