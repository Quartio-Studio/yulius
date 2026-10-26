use std::fs;

use super::*;
use crate::instance::archive::{Entry, Kind, Writer};
use crate::instance::export::{plan, send};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn ids() -> IdMap {
    let (uid, gid) = (
        nix::unistd::geteuid().as_raw(),
        nix::unistd::getegid().as_raw(),
    );
    IdMap {
        uid: (uid, uid),
        gid: (gid, gid),
        subuid: None,
        subgid: None,
    }
}

fn manifest_for(target: &Path) -> Manifest {
    Manifest {
        format: FORMAT,
        schema: 1,
        data_dir: target.display().to_string(),
        ..Manifest::default()
    }
}

/// The export of a small instance, addressed to `target`.
fn exported(target: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let src = tempfile::tempdir()?;
    fs::create_dir_all(src.path().join("sandboxes/sb/home"))?;
    fs::write(src.path().join("yulius.db"), vec![9u8; 3000])?;
    fs::write(src.path().join("sandboxes/sb/home/.bashrc"), b"echo\n")?;
    let p = plan(src.path())?;
    let mut out = Vec::new();
    send(
        src.path(),
        &p,
        &manifest_for(target),
        &mut out,
        &mut |_| {},
        &mut || Ok(()),
    )?;
    Ok(out)
}

fn run(bytes: &[u8], target: &Path) -> Result<(Manifest, Report), Failure> {
    install(
        bytes,
        target,
        None,
        &mut |_, _| Ok(ids()),
        crate::instance::is_root(),
        &mut |_, _| {},
    )
}

#[test]
fn a_copy_installs_into_a_new_data_directory() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    let (m, report) = run(&bytes, &target).map_err(|f| format!("{f:?}"))?;
    assert_eq!(m.total_bytes, bytes.len() as u64);
    assert_eq!(report.entries, m.entries);
    assert_eq!(fs::read(target.join("yulius.db"))?, vec![9u8; 3000]);
    assert_eq!(
        fs::read(target.join("sandboxes/sb/home/.bashrc"))?,
        b"echo\n"
    );
    assert!(!target.join(INCOMPLETE).exists());
    Ok(())
}

#[test]
fn the_data_directory_must_be_the_exported_one() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    let other = host.path().join("elsewhere");
    let Err(Failure::Refused(why)) = run(&bytes, &other) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("exported from"), "{why}");
    assert!(!target.exists() && !other.exists());
    assert!(run(&bytes, &target).is_ok());
    Ok(())
}

#[test]
fn an_existing_instance_is_never_overwritten() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    for name in ["yulius.db", "sandboxes"] {
        fs::create_dir_all(&target)?;
        fs::write(target.join(name), b"live")?;
        let Err(Failure::Refused(why)) = run(&bytes, &target) else {
            return Err(format!("{name}: expected a refusal").into());
        };
        assert!(why.contains("already holds an instance"), "{why}");
        assert_eq!(fs::read(target.join(name))?, b"live");
        fs::remove_file(target.join(name))?;
    }
    Ok(())
}

#[test]
fn a_cut_stream_fails_and_blocks_the_next_import_until_removed() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    let Err(Failure::Failed(why)) = run(&bytes[..bytes.len() - 2048], &target) else {
        return Err("expected a failure".into());
    };
    assert!(why.contains("incomplete copy"), "{why}");
    assert!(target.join(INCOMPLETE).exists());
    let Err(Failure::Refused(why)) = run(&bytes, &target) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("did not finish"), "{why}");
    Ok(())
}

#[test]
fn a_damaged_copy_fails() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let mut bytes = exported(&target)?;
    let at = bytes
        .windows(9)
        .position(|w| w == b"yulius.db")
        .ok_or("no database entry")?
        + 600;
    bytes[at] ^= 1;
    let Err(Failure::Failed(why)) = run(&bytes, &target) else {
        return Err("expected a failure".into());
    };
    assert!(why.contains("damaged"), "{why}");
    Ok(())
}

/// An archive of `m` and the data directory entry only.
fn crafted(m: &Manifest) -> io::Result<Vec<u8>> {
    let content = m.to_bytes().map_err(io::Error::other)?;
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out);
    let head = Entry {
        path: MANIFEST.to_vec(),
        kind: Kind::File { size: MANIFEST_LEN },
        mode: 0o600,
        uid: 0,
        gid: 0,
        mtime: 0,
        xattrs: Vec::new(),
    };
    w.entry(&head, Some(&mut content.as_slice()), |_| {})?;
    let ids = ids();
    let root = Entry {
        path: b"data".to_vec(),
        kind: Kind::Dir,
        mode: 0o700,
        uid: ids.uid.0,
        gid: ids.gid.0,
        mtime: 0,
        xattrs: Vec::new(),
    };
    w.entry(&root, None, |_| {})?;
    w.finish()?;
    Ok(out)
}

#[test]
fn a_length_other_than_the_manifests_fails() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let mut m = manifest_for(&target);
    m.total_bytes = 1;
    let Err(Failure::Failed(why)) = run(&crafted(&m)?, &target) else {
        return Err("expected a failure".into());
    };
    assert!(why.contains("its manifest says 1"), "{why}");
    Ok(())
}

/// A change to a manifest.
type Change = fn(&mut Manifest);

#[test]
fn what_cannot_work_is_refused_before_anything_is_written() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let cases: [(Change, &str); 4] = [
        (|m| m.format = 2, "format 2"),
        (|m| m.schema = u32::MAX, "install a Yulius at least as new"),
        (|m| m.total_bytes = u64::MAX, "is free"),
        (|m| m.data_dir = "relative".into(), "exported from"),
    ];
    for (change, says) in cases {
        let mut m = manifest_for(&target);
        change(&mut m);
        let Err(Failure::Refused(why)) = run(&crafted(&m)?, &target) else {
            return Err(format!("{says}: expected a refusal").into());
        };
        assert!(why.contains(says), "{why}");
        assert!(!target.exists(), "{says}");
    }
    Ok(())
}

#[test]
fn a_refused_host_preparation_writes_nothing() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    let result = install(
        bytes.as_slice(),
        &target,
        None,
        &mut |_, _| Err("no ids".into()),
        false,
        &mut |_, _| {},
    );
    assert_eq!(result.err(), Some(Failure::Refused("no ids".into())));
    assert!(!target.exists());
    Ok(())
}

#[test]
fn a_tar_that_is_not_an_export_is_refused() -> TestResult {
    let mut out = Vec::new();
    let mut w = Writer::new(&mut out);
    let e = Entry {
        path: b"data".to_vec(),
        kind: Kind::Dir,
        mode: 0o700,
        uid: 0,
        gid: 0,
        mtime: 0,
        xattrs: Vec::new(),
    };
    w.entry(&e, None, |_| {})?;
    w.finish()?;
    let Err(Failure::Refused(why)) = run(&out, Path::new("/var/lib/yulius")) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("not a Yulius export"), "{why}");
    Ok(())
}

#[test]
fn a_path_with_dot_dot_is_refused() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("x/../yulius");
    let bytes = exported(&target)?;
    let Err(Failure::Refused(why)) = run(&bytes, &target) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("not a plain absolute path"), "{why}");
    assert!(!host.path().join("yulius").exists());
    Ok(())
}

/// A copy whose manifest names someone's home must not reach it: the target holds files.
#[test]
fn a_directory_that_holds_files_is_never_a_target() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("home");
    fs::create_dir_all(target.join(".ssh"))?;
    fs::write(target.join(".ssh/authorized_keys"), b"mine")?;
    let bytes = exported(&target)?;
    let Err(Failure::Refused(why)) = run(&bytes, &target) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("only into a new or empty"), "{why}");
    assert_eq!(fs::read(target.join(".ssh/authorized_keys"))?, b"mine");
    Ok(())
}

#[test]
fn skeleton_dotfiles_and_lost_found_may_be_there() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    fs::create_dir_all(target.join("lost+found"))?;
    fs::write(target.join(".bashrc"), b"skel")?;
    let bytes = exported(&target)?;
    if crate::instance::is_root() {
        // Root owns what the test made; give it to the copy's service user (the test's own,
        // which is root: refused as a fresh disk would be).
        let Err(Failure::Refused(why)) = run(&bytes, &target) else {
            return Err("expected a refusal".into());
        };
        assert!(why.contains("belongs to root"), "{why}");
        return Ok(());
    }
    run(&bytes, &target).map_err(|f| format!("{f:?}"))?;
    Ok(())
}

#[test]
fn a_target_of_another_user_is_refused() -> TestResult {
    if !crate::instance::is_root() {
        eprintln!("skipped: needs root to give the directory away");
        return Ok(());
    }
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    fs::create_dir_all(&target)?;
    std::os::unix::fs::chown(&target, Some(65_534), Some(65_534))?;
    let bytes = exported(&target)?;
    let Err(Failure::Refused(why)) = run(&bytes, &target) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("not to the copy's service user"), "{why}");
    Ok(())
}

#[test]
fn a_link_at_the_marker_is_refused_and_not_followed() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    fs::create_dir_all(&target)?;
    let elsewhere = host.path().join("planted");
    std::os::unix::fs::symlink(&elsewhere, target.join(INCOMPLETE))?;
    let bytes = exported(&target)?;
    let Err(Failure::Refused(why)) = run(&bytes, &target) else {
        return Err("expected a refusal".into());
    };
    assert!(why.contains("did not finish"), "{why}");
    assert!(!elsewhere.exists());
    Ok(())
}
