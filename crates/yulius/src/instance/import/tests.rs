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
    exported_with(target, &|_| Ok(()))
}

/// The same, after `more` added to the instance's data directory.
fn exported_with(
    target: &Path,
    more: &dyn Fn(&Path) -> io::Result<()>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let src = tempfile::tempdir()?;
    fs::create_dir_all(src.path().join("sandboxes/sb/home"))?;
    fs::write(src.path().join("yulius.db"), vec![9u8; 3000])?;
    fs::write(src.path().join("sandboxes/sb/home/.bashrc"), b"echo\n")?;
    more(src.path())?;
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
    crafted_with(m, &[])
}

/// An archive of `m`, the data directory entry and `more` entries without content.
fn crafted_with(m: &Manifest, more: &[Entry]) -> io::Result<Vec<u8>> {
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
    for e in more {
        w.entry(e, None, |_| {})?;
    }
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

/// SPEC-460 MIG-024: a move builds the new host's sandbox base before its copy; what a copy
/// never holds may be there and is left alone.
#[test]
fn the_new_hosts_own_sandbox_base_may_be_there() -> TestResult {
    if crate::instance::is_root() {
        eprintln!("skipped: root owns the target, which is refused as a fresh disk");
        return Ok(());
    }
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    fs::create_dir_all(target.join("system/base-9"))?;
    fs::write(target.join("system/base-9/ready"), b"this host's")?;
    fs::create_dir_all(target.join("runtime"))?;
    let bytes = exported(&target)?;
    run(&bytes, &target).map_err(|f| format!("{f:?}"))?;
    assert_eq!(
        fs::read(target.join("system/base-9/ready"))?,
        b"this host's"
    );
    assert!(target.join("yulius.db").is_file());
    // Anything else still is not an empty data directory.
    let other = host.path().join("other");
    fs::create_dir_all(other.join("systems"))?;
    let bytes = exported(&other)?;
    assert!(matches!(run(&bytes, &other), Err(Failure::Refused(_))));
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

/// SPEC-456 BNS-010: the next steps name what `host setup` does for bwrap only on a host that
/// lacks it, and never tell anyone to write a rule by hand.
#[test]
fn the_next_steps_name_the_namespace_rule_only_when_the_host_lacks_it() {
    let url = "https://yulius.example.org";
    let lacking = next_steps(url, "yulius.service", true);
    assert!(
        lacking.starts_with(
            "Next:\n  1. sudo yulius host setup --public-url https://yulius.example.org\n     \
             (lets bwrap start the user namespaces sandboxes run in, which this host does not \
             allow yet; writes yulius.service, builds the sandbox base for this host, checks \
             Docker, starts Yulius)\n  2. "
        ),
        "{lacking}"
    );
    let ready = next_steps(url, "yulius.service", false);
    assert!(
        ready.starts_with(
            "Next:\n  1. sudo yulius host setup --public-url https://yulius.example.org\n     \
             (writes yulius.service, builds the sandbox base for this host, checks Docker, \
             starts Yulius)\n  2. "
        ),
        "{ready}"
    );
    assert!(!ready.contains("bwrap"), "{ready}");
    for text in [&lacking, &ready] {
        assert!(!text.to_lowercase().contains("apparmor"), "{text}");
        assert!(text.contains("\n  4. Keep the old instance"), "{text}");
    }
}

#[test]
fn a_finished_import_leaves_the_limits_mark_for_the_first_start() -> TestResult {
    use std::os::unix::fs::MetadataExt;
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    run(&bytes, &target).map_err(|f| format!("{f:?}"))?;
    let mark = fs::symlink_metadata(target.join(yulius_app::limits::ADAPT_MARK))?;
    assert!(mark.is_file());
    assert_eq!((mark.uid(), mark.gid()), (ids().uid.1, ids().gid.1));
    Ok(())
}

#[test]
fn an_import_that_did_not_finish_leaves_no_limits_mark() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let bytes = exported(&target)?;
    let Err(Failure::Failed(_)) = run(&bytes[..bytes.len() - 2048], &target) else {
        return Err("expected a failure".into());
    };
    assert!(!target.join(yulius_app::limits::ADAPT_MARK).exists());
    Ok(())
}

/// An export leaves the mark out (SPEC-467 LFT-005); an archive of an older build, or one made
/// by hand, may still bring its name.
#[test]
fn a_link_the_copy_brings_at_the_limits_mark_is_replaced_not_followed() -> TestResult {
    use std::os::unix::ffi::OsStrExt as _;
    const MARK: &str = yulius_app::limits::ADAPT_MARK;
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let elsewhere = host.path().join("planted");
    let link = Entry {
        path: format!("data/{MARK}").into_bytes(),
        kind: Kind::Symlink {
            target: elsewhere.as_os_str().as_bytes().to_vec(),
        },
        mode: 0o777,
        uid: ids().uid.0,
        gid: ids().gid.0,
        mtime: 0,
        xattrs: Vec::new(),
    };
    let mut m = manifest_for(&target);
    // The manifest has one length whatever it says, so the archive's is known from a first one.
    m.total_bytes = crafted_with(&m, std::slice::from_ref(&link))?.len() as u64;
    m.entries = 2;
    let bytes = crafted_with(&m, std::slice::from_ref(&link))?;
    run(&bytes, &target).map_err(|f| format!("{f:?}"))?;
    let mark = fs::symlink_metadata(target.join(MARK))?;
    assert!(mark.is_file(), "the mark is a file of its own");
    assert!(!elsewhere.exists());

    // And the export itself no longer brings one.
    let bytes = exported_with(&target, &|src| {
        std::os::unix::fs::symlink(&elsewhere, src.join(MARK))
    })?;
    let mut r = Reader::new(bytes.as_slice());
    let mut paths = Vec::new();
    while let Item::Entry(e) = r.next_item()? {
        io::copy(&mut r.data_reader(), &mut io::sink())?;
        paths.push(String::from_utf8_lossy(&e.path).into_owned());
    }
    assert!(paths.contains(&"data/yulius.db".to_owned()), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains(MARK)), "{paths:?}");
    Ok(())
}
