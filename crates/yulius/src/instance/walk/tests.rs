use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A small data directory with every kind of file the export meets.
fn tree() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    for d in [
        "sandboxes/sb/repo",
        "system/base-1",
        "runtime/sandboxes",
        "update",
        // The move helper's request (SPEC-470): asked of this host's root, never of another's.
        "migrate",
        "runs/a",
        // Claude Code built for this host's platform (SPEC-421): the new host installs its own.
        "claude-code/versions/2.1.311/bin",
    ] {
        fs::create_dir_all(root.join(d))?;
    }
    fs::write(root.join("yulius.db"), b"db")?;
    fs::write(root.join("yulius.db-wal"), b"wal")?;
    fs::write(root.join("yulius.db-shm"), b"shm")?;
    fs::write(root.join("master.key"), b"key")?;
    fs::set_permissions(root.join("master.key"), fs::Permissions::from_mode(0o600))?;
    fs::write(root.join("system/base-1/bin"), b"x")?;
    fs::write(root.join("update/request.json"), b"{}")?;
    fs::write(root.join("migrate/request.json"), b"{}")?;
    fs::write(root.join("sandboxes/sb/repo/a"), b"aaaa")?;
    fs::hard_link(
        root.join("sandboxes/sb/repo/a"),
        root.join("sandboxes/sb/repo/b"),
    )?;
    symlink("/etc/passwd", root.join("sandboxes/sb/out"))?;
    nix::unistd::mkfifo(&root.join("runs/a/fifo"), nix::sys::stat::Mode::S_IRWXU)?;
    let _socket = std::os::unix::net::UnixListener::bind(root.join("runs/a/sock"))?;
    Ok(dir)
}

fn listing(root: &Path) -> io::Result<(Vec<Entry>, Skipped)> {
    let mut seen = Vec::new();
    let skipped = walk(root, &mut |e, _| {
        seen.push(e.clone());
        Ok(())
    })?;
    Ok((seen, skipped))
}

#[test]
fn entries_come_parents_first_in_byte_order_without_what_the_new_host_makes() -> TestResult {
    let dir = tree()?;
    let (seen, skipped) = listing(dir.path())?;
    let paths: Vec<String> = seen
        .iter()
        .map(|e| String::from_utf8_lossy(&e.path).into_owned())
        .collect();
    assert_eq!(
        paths,
        [
            "data",
            "data/master.key",
            "data/runs",
            "data/runs/a",
            "data/sandboxes",
            "data/sandboxes/sb",
            "data/sandboxes/sb/out",
            "data/sandboxes/sb/repo",
            "data/sandboxes/sb/repo/a",
            "data/sandboxes/sb/repo/b",
            "data/yulius.db",
            "data/yulius.db-wal",
        ]
    );
    assert_eq!(skipped.special, 2, "the FIFO and the socket");
    Ok(())
}

#[test]
fn links_stay_links_and_hard_links_point_at_their_first_path() -> TestResult {
    let dir = tree()?;
    let (seen, _) = listing(dir.path())?;
    let find = |p: &str| {
        seen.iter()
            .find(|e| e.path == p.as_bytes())
            .cloned()
            .ok_or("missing")
    };
    assert_eq!(
        find("data/sandboxes/sb/out")?.kind,
        Kind::Symlink {
            target: b"/etc/passwd".to_vec()
        }
    );
    assert_eq!(
        find("data/sandboxes/sb/repo/a")?.kind,
        Kind::File { size: 4 }
    );
    assert_eq!(
        find("data/sandboxes/sb/repo/b")?.kind,
        Kind::HardLink {
            target: b"data/sandboxes/sb/repo/a".to_vec()
        }
    );
    let key = find("data/master.key")?;
    assert_eq!(key.mode, 0o600);
    assert_eq!(key.kind, Kind::File { size: 3 });
    Ok(())
}

#[test]
fn extended_attributes_are_kept() -> TestResult {
    let dir = tree()?;
    let file = dir.path().join("yulius.db");
    if let Err(e) = rustix::fs::lsetxattr(
        &file,
        "user.yulius-test",
        &[0, 1, 2][..],
        rustix::fs::XattrFlags::empty(),
    ) {
        eprintln!("skipped: this filesystem keeps no user attributes ({e})");
        return Ok(());
    }
    let (seen, _) = listing(dir.path())?;
    let db = seen
        .iter()
        .find(|e| e.path == b"data/yulius.db")
        .ok_or("no database")?;
    assert!(
        db.xattrs
            .contains(&(b"user.yulius-test".to_vec(), vec![0, 1, 2])),
        "{:?}",
        db.xattrs
    );
    Ok(())
}

#[test]
fn a_left_out_name_below_the_top_is_kept() -> TestResult {
    let dir = tree()?;
    fs::create_dir_all(dir.path().join("sandboxes/sb/system"))?;
    let (seen, _) = listing(dir.path())?;
    assert!(seen.iter().any(|e| e.path == b"data/sandboxes/sb/system"));
    Ok(())
}

#[test]
fn the_walk_shows_each_entry_s_inode_and_change_time() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;
    let dir = tree()?;
    let root = dir.path();
    let mut found = Vec::new();
    walk_seen(root, false, &copied, &mut |seen| {
        found.push((
            String::from_utf8_lossy(&seen.entry.path).into_owned(),
            (seen.ino, seen.dev, seen.nlink, seen.ctime),
        ));
        Ok(true)
    })?;
    let of = |name: &str| found.iter().find(|(path, _)| path == name).map(|f| f.1);
    let meta = fs::symlink_metadata(root.join("sandboxes/sb/repo/a"))?;
    let expected = (meta.ino(), meta.dev(), 2, (meta.ctime(), meta.ctime_nsec()));
    assert_eq!(of("data/sandboxes/sb/repo/a"), Some(expected));
    // The link is the same inode.
    assert_eq!(of("data/sandboxes/sb/repo/b"), Some(expected));
    let link = fs::symlink_metadata(root.join("sandboxes/sb/out"))?;
    assert_eq!(
        of("data/sandboxes/sb/out").map(|f| f.0),
        Some(link.ino()),
        "a link's own inode, not its target's"
    );
    Ok(())
}

#[test]
fn a_live_walk_leaves_out_what_is_gone_and_an_exact_one_fails() -> TestResult {
    let dir = tree()?;
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("sandboxes/sb/tmp/deep"))?;
    fs::write(root.join("sandboxes/sb/tmp/deep/f"), b"x")?;
    fs::write(root.join("sandboxes/sb/zz"), b"z")?;
    let run = |live: bool| {
        let mut paths = Vec::new();
        let walked = walk_seen(&root, live, &copied, &mut |seen| {
            let path = String::from_utf8_lossy(&seen.entry.path).into_owned();
            // Removed while the walk is in the directory before them.
            if path == "data/sandboxes/sb/repo" {
                let _ = fs::remove_dir_all(root.join("sandboxes/sb/tmp/deep"));
                let _ = fs::remove_file(root.join("sandboxes/sb/zz"));
            }
            // A directory that goes away once it was seen: nothing below it is walked.
            if path == "data/sandboxes/sb/tmp" {
                let _ = fs::remove_dir_all(root.join("sandboxes/sb/tmp"));
            }
            paths.push(path);
            Ok(true)
        });
        (walked, paths)
    };
    let (walked, paths) = run(true);
    let skipped = walked?;
    assert!(
        paths.contains(&"data/sandboxes/sb/tmp".to_owned()),
        "{paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("deep") || p.ends_with("/zz"))
    );
    assert_eq!(skipped.vanished, 2, "{paths:?}");
    assert!(paths.contains(&"data/yulius.db".to_owned()));

    fs::create_dir_all(root.join("sandboxes/sb/tmp/deep"))?;
    fs::write(root.join("sandboxes/sb/zz"), b"z")?;
    let (walked, _) = run(false);
    assert!(walked.is_err());
    Ok(())
}

#[test]
fn a_first_path_that_is_not_there_after_all_is_no_link_s_target() -> TestResult {
    let dir = tree()?;
    let mut kinds = Vec::new();
    walk_seen(dir.path(), true, &copied, &mut |seen| {
        let path = String::from_utf8_lossy(&seen.entry.path).into_owned();
        let there = path != "data/sandboxes/sb/repo/a";
        if there {
            kinds.push((path, seen.entry.kind.clone()));
        }
        Ok(there)
    })?;
    // `b` is the file now, not a link to a path that was never sent.
    let b = kinds
        .iter()
        .find(|(path, _)| path == "data/sandboxes/sb/repo/b")
        .map(|(_, kind)| kind.clone());
    assert_eq!(b, Some(Kind::File { size: 4 }));
    Ok(())
}

#[test]
fn an_open_file_s_attributes_are_its_path_s() -> TestResult {
    let dir = tree()?;
    let path = dir.path().join("master.key");
    let set = rustix::fs::setxattr(
        &path,
        "user.note",
        b"hello",
        rustix::fs::XattrFlags::empty(),
    );
    if set.is_err() {
        // A filesystem without user attributes: both read none.
        assert!(xattrs(&path)?.is_empty());
    }
    assert_eq!(xattrs_of(&fs::File::open(&path)?)?, xattrs(&path)?);
    Ok(())
}

#[test]
fn a_link_that_is_a_file_by_the_time_it_is_read_is_gone() -> TestResult {
    let dir = tree()?;
    let root = dir.path();
    assert_eq!(link_text(&root.join("sandboxes/sb/out"))?, b"/etc/passwd");
    // A link when the walk looked at it, a file when its text is read.
    let late = link_text(&root.join("master.key"));
    assert!(late.as_ref().is_err_and(gone), "{late:?}");
    // What cannot be opened at all (a socket took a file's place) is gone too.
    let socket = std::io::Error::from_raw_os_error(rustix::io::Errno::NXIO.raw_os_error());
    assert!(gone(&socket));
    assert!(!gone(&std::io::Error::from_raw_os_error(
        rustix::io::Errno::ACCESS.raw_os_error()
    )));
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_live_walk_stays_in_the_directories_it_opened() -> TestResult {
    // A directory that is renamed away under a walk, with a link to somewhere else under its
    // name: the walk goes on in the directory it opened and never follows the name.
    let dir = tempfile::tempdir()?;
    let (root, outside) = (dir.path().join("data"), dir.path().join("outside"));
    fs::create_dir_all(root.join("sb/dir/sub"))?;
    fs::write(root.join("sb/dir/a"), b"a")?;
    fs::write(root.join("sb/dir/sub/inner"), b"inner")?;
    std::os::unix::fs::symlink("a", root.join("sb/dir/z-link"))?;
    fs::create_dir_all(outside.join("sub"))?;
    fs::write(outside.join("sub/planted"), b"planted")?;
    fs::write(outside.join("planted"), b"planted")?;
    let mut paths = Vec::new();
    let skipped = walk_seen(&root, true, &copied, &mut |seen| {
        let path = String::from_utf8_lossy(&seen.entry.path).into_owned();
        if path == "data/sb/dir/a" {
            fs::rename(root.join("sb/dir"), root.join("sb/aside"))?;
            std::os::unix::fs::symlink(&outside, root.join("sb/dir"))?;
        }
        // What the walk shows can be looked at and opened where it is.
        let meta = fs::symlink_metadata(seen.disk)?;
        assert_eq!((meta.ino(), meta.dev()), (seen.ino, seen.dev), "{path}");
        paths.push(path);
        Ok(true)
    })?;
    assert_eq!(
        paths,
        [
            "data",
            "data/sb",
            "data/sb/dir",
            "data/sb/dir/a",
            "data/sb/dir/sub",
            "data/sb/dir/sub/inner",
            "data/sb/dir/z-link",
        ]
    );
    assert_eq!(skipped, Skipped::default());
    Ok(())
}

/// SPEC-470 MVB-020: the directory the move helper's request lives in is one a copy leaves
/// out, so what was asked of this host's root never arrives on another host as asked there.
#[test]
fn the_move_helpers_request_never_travels() {
    assert!(left_out(yulius_contracts::migrate::REQUEST_DIR.as_bytes()));
}

/// SPEC-467 LFT-005: every import writes its own limits mark, so the old host's never travels,
/// whatever sits at its name. Only at the top: the name means nothing below it.
#[test]
fn the_imports_limits_mark_is_left_out_at_the_top_only() -> TestResult {
    const MARK: &str = yulius_app::limits::ADAPT_MARK;
    let top = format!("data/{MARK}");
    for make in [
        (|at| fs::write(at, b"")) as fn(&Path) -> io::Result<()>,
        |at| symlink("/etc/passwd", at),
        |at| fs::create_dir_all(at.join("below")),
    ] {
        let dir = tree()?;
        make(&dir.path().join(MARK))?;
        fs::write(dir.path().join(".bashrc"), b"# another dotfile travels\n")?;
        fs::write(dir.path().join("sandboxes/sb").join(MARK), b"")?;
        let (seen, _) = listing(dir.path())?;
        let paths: Vec<String> = seen
            .iter()
            .map(|e| String::from_utf8_lossy(&e.path).into_owned())
            .collect();
        assert!(!paths.iter().any(|p| p.starts_with(&top)), "{paths:?}");
        assert!(paths.contains(&"data/.bashrc".to_owned()), "{paths:?}");
        assert!(
            paths.contains(&format!("data/sandboxes/sb/{MARK}")),
            "{paths:?}"
        );
        assert!(left_out(MARK.as_bytes()) && !left_out(b".bashrc"));
    }
    Ok(())
}
