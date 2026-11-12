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
