use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};

use super::*;

/// A small data directory with every kind of file the export meets.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for d in ["sandboxes/sb/repo", "system/base-1", "runtime/sandboxes", "update", "runs/a"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("yulius.db"), b"db").unwrap();
    fs::write(root.join("yulius.db-wal"), b"wal").unwrap();
    fs::write(root.join("yulius.db-shm"), b"shm").unwrap();
    fs::write(root.join("master.key"), b"key").unwrap();
    fs::set_permissions(root.join("master.key"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(root.join("system/base-1/bin"), b"x").unwrap();
    fs::write(root.join("update/request.json"), b"{}").unwrap();
    fs::write(root.join("sandboxes/sb/repo/a"), b"aaaa").unwrap();
    fs::hard_link(root.join("sandboxes/sb/repo/a"), root.join("sandboxes/sb/repo/b")).unwrap();
    symlink("/etc/passwd", root.join("sandboxes/sb/out")).unwrap();
    nix::unistd::mkfifo(&root.join("runs/a/fifo"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(root.join("runs/a/sock")).unwrap();
    dir
}

fn listing(root: &Path) -> (Vec<Entry>, Skipped) {
    let mut seen = Vec::new();
    let skipped = walk(root, &mut |e, _| {
        seen.push(e.clone());
        Ok(())
    })
    .unwrap();
    (seen, skipped)
}

#[test]
fn entries_come_parents_first_in_byte_order_without_what_the_new_host_makes() {
    let dir = tree();
    let (seen, skipped) = listing(dir.path());
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
}

#[test]
fn links_stay_links_and_hard_links_point_at_their_first_path() {
    let dir = tree();
    let (seen, _) = listing(dir.path());
    let find = |p: &str| seen.iter().find(|e| e.path == p.as_bytes()).unwrap().clone();
    assert_eq!(
        find("data/sandboxes/sb/out").kind,
        Kind::Symlink {
            target: b"/etc/passwd".to_vec()
        }
    );
    assert_eq!(find("data/sandboxes/sb/repo/a").kind, Kind::File { size: 4 });
    assert_eq!(
        find("data/sandboxes/sb/repo/b").kind,
        Kind::HardLink {
            target: b"data/sandboxes/sb/repo/a".to_vec()
        }
    );
    let key = find("data/master.key");
    assert_eq!(key.mode, 0o600);
    assert_eq!(key.kind, Kind::File { size: 3 });
}

#[test]
fn extended_attributes_are_kept() {
    let dir = tree();
    let file = dir.path().join("yulius.db");
    if let Err(e) = rustix::fs::lsetxattr(
        &file,
        "user.yulius-test",
        &[0, 1, 2][..],
        rustix::fs::XattrFlags::empty(),
    ) {
        eprintln!("skipped: this filesystem keeps no user attributes ({e})");
        return;
    }
    let (seen, _) = listing(dir.path());
    let db = seen.iter().find(|e| e.path == b"data/yulius.db").unwrap();
    assert!(
        db.xattrs
            .contains(&(b"user.yulius-test".to_vec(), vec![0, 1, 2])),
        "{:?}",
        db.xattrs
    );
}

#[test]
fn a_left_out_name_below_the_top_is_kept() {
    let dir = tree();
    fs::create_dir_all(dir.path().join("sandboxes/sb/system")).unwrap();
    let (seen, _) = listing(dir.path());
    assert!(seen.iter().any(|e| e.path == b"data/sandboxes/sb/system"));
}
