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

#[test]
fn a_link_the_copy_brings_at_the_limits_mark_is_replaced_not_followed() -> TestResult {
    let host = tempfile::tempdir()?;
    let target = host.path().join("yulius");
    let elsewhere = host.path().join("planted");
    let bytes = exported_with(&target, &|src| {
        std::os::unix::fs::symlink(&elsewhere, src.join(yulius_app::limits::ADAPT_MARK))
    })?;
    run(&bytes, &target).map_err(|f| format!("{f:?}"))?;
    let mark = fs::symlink_metadata(target.join(yulius_app::limits::ADAPT_MARK))?;
    assert!(mark.is_file(), "the mark is a file of its own");
    assert!(!elsewhere.exists());
    Ok(())
}

// Passes of a move into a staged copy (SPEC-465).

use std::os::unix::fs::MetadataExt as _;

use crate::instance::export::{plan_pass, send_live, send_pass};
use crate::instance::inventory::{Counter, IdSpace, of_copy};
use crate::instance::receipts::Stage;
use crate::instance::{Pass, receipts};

/// An old host's data directory and receipts, and a new host's data directory.
struct Hosts {
    _tmp: tempfile::TempDir,
    old: PathBuf,
    new: PathBuf,
    stage: Stage,
    /// The next stream is cut to this many bytes on its way.
    cut: Option<usize>,
    /// The new host cannot sync.
    no_sync: bool,
    /// The streams sent, for a look at what they held.
    streams: Vec<Vec<u8>>,
}

/// The boot every test's passes run in, with a clock that was never set.
fn era() -> crate::instance::receipts::Era {
    crate::instance::receipts::Era {
        boot: "boot".into(),
        ahead: Some(0),
    }
}

fn hosts() -> Result<Hosts, Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let old = tmp.path().join("old");
    fs::create_dir_all(old.join("sandboxes/sb/home"))?;
    fs::create_dir_all(old.join("sysroots/sb/upper"))?;
    fs::write(old.join("yulius.db"), vec![9u8; 3000])?;
    fs::write(old.join("master.key"), b"key")?;
    fs::write(old.join("sandboxes/sb/home/notes"), b"hello\n")?;
    fs::hard_link(
        old.join("sandboxes/sb/home/notes"),
        old.join("sandboxes/sb/home/notes-again"),
    )?;
    std::os::unix::fs::symlink("notes", old.join("sandboxes/sb/home/link"))?;
    fs::write(old.join("sysroots/sb/upper/tool"), vec![1u8; 1500])?;
    Ok(Hosts {
        stage: Stage::open(&tmp.path().join("state/copy")),
        new: tmp.path().join("new"),
        old,
        _tmp: tmp,
        cut: None,
        no_sync: false,
        streams: Vec::new(),
    })
}

impl Hosts {
    fn manifest(&self, live: bool) -> Manifest {
        Manifest {
            pass: Some(Pass {
                stage: self.stage.id().unwrap_or_default().to_owned(),
                seq: self.stage.acked(),
                nonce: receipts::token(),
                live,
                kept: 0,
            }),
            ..manifest_for(&self.new)
        }
    }

    /// What the move does before every pass: the receipts follow the new host's marker.
    fn reconcile(&mut self) -> Result<receipts::Reconciled, String> {
        let found = self
            .stage
            .reconcile(Mark::read(&self.new).as_ref(), &era())?;
        if found.fresh {
            let _ = fs::remove_dir_all(&self.new);
            self.stage.renew(&receipts::token())?;
        }
        Ok(found)
    }

    fn land(&mut self, mut bytes: Vec<u8>) -> Result<Landing, Failure> {
        if let Some(cut) = self.cut.take() {
            bytes.truncate(cut);
        }
        self.streams.push(bytes.clone());
        let no_sync = self.no_sync;
        land(
            bytes.as_slice(),
            &self.new,
            None,
            &mut |_, _| Ok(ids()),
            &mut Ground {
                chown: false,
                sync: &mut |_| {
                    if no_sync {
                        Err(io::Error::other("Input/output error"))
                    } else {
                        Ok(())
                    }
                },
                tick: &mut |_, _| {},
            },
        )
    }

    /// One warm pass: every stable entry is old enough to earn its receipt.
    fn warm(&mut self) -> Result<Result<Landing, Failure>, Box<dyn std::error::Error>> {
        self.reconcile()?;
        let m = self.manifest(true);
        let nonce = m.pass.as_ref().map(|p| p.nonce.clone()).unwrap_or_default();
        self.stage.begin(&nonce, true, 1_000, &era())?;
        let mut out = Vec::new();
        send_live(
            &self.old,
            &m,
            &mut out,
            &mut self.stage,
            (i64::MAX / 2, 0),
            &mut |_| {},
        )?;
        Ok(self.land(out))
    }

    /// The frozen pass; the old host's inventory with it.
    fn frozen(
        &mut self,
    ) -> Result<
        (
            Result<Landing, Failure>,
            crate::instance::inventory::Inventory,
        ),
        Box<dyn std::error::Error>,
    > {
        self.reconcile()?;
        let m = self.manifest(false);
        let nonce = m.pass.as_ref().map(|p| p.nonce.clone()).unwrap_or_default();
        let mut counter = Counter::new(IdSpace::of(&self.manifest_of_old()));
        let plan = plan_pass(&self.old, Some(&self.stage), &mut |e| counter.add(e))?;
        self.stage.begin(&nonce, false, 2_000, &era())?;
        let mut out = Vec::new();
        send_pass(
            &self.old,
            &plan,
            &m,
            &mut out,
            Some(&mut self.stage),
            &mut |_| {},
            &mut || Ok(()),
        )?;
        Ok((self.land(out), counter.finish()))
    }

    fn manifest_of_old(&self) -> Manifest {
        let mut m = manifest_for(&self.new);
        let (uid, gid) = (ids().uid.0, ids().gid.0);
        m.service.uid = uid;
        m.service.gid = gid;
        m
    }

    /// The paths of the `data` entries in the stream sent last.
    fn sent(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut r = Reader::new(self.streams.last().ok_or("no stream")?.as_slice());
        let mut paths = Vec::new();
        while let Ok(Item::Entry(e)) = r.next_item() {
            if e.path.starts_with(b"data") {
                paths.push(String::from_utf8(e.path)?);
            }
        }
        Ok(paths)
    }

    fn mark(&self) -> Result<Mark, String> {
        Mark::read(&self.new).ok_or_else(|| "no staged copy".to_owned())
    }

    fn last(&self) -> Result<Landed, String> {
        self.mark()?.last.ok_or_else(|| "nothing landed".to_owned())
    }
}

fn arrived(result: Result<Landing, Failure>) -> Result<Landing, String> {
    result.map_err(|why| format!("{why:?}"))
}

fn failed(result: Result<Landing, Failure>) -> Result<String, String> {
    match result {
        Err(Failure::Failed(why)) => Ok(why),
        other => Err(format!("{other:?}")),
    }
}

fn refused(result: Result<Landing, Failure>) -> Result<String, String> {
    match result {
        Err(Failure::Refused(why)) => Ok(why),
        other => Err(format!("{other:?}")),
    }
}

#[test]
fn a_first_pass_stages_a_copy_and_the_next_sends_only_what_changed() -> TestResult {
    let mut h = hosts()?;
    let landed = arrived(h.warm()?)?;
    assert_eq!(landed.report.entries, 13);
    let mark = h.mark()?;
    assert_eq!(
        (mark.acked, mark.stage.as_str()),
        (1, h.stage.id().unwrap_or_default())
    );
    let last = h.last()?;
    assert!(last.complete && last.live && last.deferred.is_empty());
    assert_eq!(last.entries, 13);
    assert_eq!(fs::read(h.new.join("master.key"))?, b"key");
    // Staged is not installed: the marker is there, no limits mark.
    assert!(!h.new.join(yulius_app::limits::ADAPT_MARK).exists());

    fs::write(h.old.join("master.key"), b"KEY")?;
    arrived(h.warm()?)?;
    assert_eq!(h.sent()?, ["data/master.key"]);
    assert_eq!(fs::read(h.new.join("master.key"))?, b"KEY");
    assert_eq!(h.mark()?.acked, 2);
    // Nothing changed: nothing but the manifest travels.
    arrived(h.warm()?)?;
    assert!(h.sent()?.is_empty());
    assert_eq!((h.mark()?.acked, h.stage.acked()), (3, 2));
    Ok(())
}

#[test]
fn a_stream_that_is_not_the_next_of_the_stage_here_changes_nothing() -> TestResult {
    let mut h = hosts()?;
    arrived(h.warm()?)?;
    h.reconcile()?;
    let before = fs::read(h.new.join(INCOMPLETE))?;
    let stream = |h: &mut Hosts,
                  change: &dyn Fn(&mut Pass)|
     -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut m = h.manifest(true);
        if let Some(pass) = &mut m.pass {
            change(pass);
        }
        let mut book = Stage::open(&h.old.join("no-such-state"));
        book.renew("x")?;
        book.begin("n", true, 0, &era())?;
        let mut out = Vec::new();
        send_live(&h.old, &m, &mut out, &mut book, (0, 0), &mut |_| {})?;
        fs::remove_dir_all(h.old.join("no-such-state"))?;
        Ok(out)
    };
    // Another stage, a pass too far, a pass again.
    let other = stream(&mut h, &|pass| pass.stage = "another".into())?;
    let why = refused(h.land(other))?;
    assert!(
        why.contains("holds the staged copy") && why.contains("after 1 passes"),
        "{why}"
    );
    let ahead = stream(&mut h, &|pass| pass.seq = 2)?;
    refused(h.land(ahead))?;
    let again = stream(&mut h, &|pass| pass.seq = 0)?;
    refused(h.land(again))?;
    assert_eq!(fs::read(h.new.join(INCOMPLETE))?, before);

    // No staged copy there: only a first pass may begin one, and never beside other files.
    let empty = h.new.with_file_name("empty");
    let into = |bytes: Vec<u8>, target: &Path| {
        let mut r = Reader::new(bytes.as_slice());
        let m = manifest(&mut r).map_err(Failure::Refused)?;
        take_in(
            r,
            &m,
            target,
            &m.service.user,
            ids(),
            &mut Ground {
                chown: false,
                sync: &mut |_| Ok(()),
                tick: &mut |_, _| {},
            },
        )
    };
    let later = stream(&mut h, &|pass| pass.seq = 1)?;
    let why = refused(into(later, &empty))?;
    assert!(why.contains("holds no staged copy"), "{why}");
    assert!(!empty.exists());
    // A plain import that did not finish is not a stage.
    fs::create_dir_all(&empty)?;
    fs::write(empty.join(INCOMPLETE), b"")?;
    let first = stream(&mut h, &|pass| pass.seq = 0)?;
    let why = refused(into(first.clone(), &empty))?;
    assert!(
        why.contains("did not finish and is not a move's staged copy"),
        "{why}"
    );
    fs::remove_file(empty.join(INCOMPLETE))?;
    fs::write(empty.join("yulius.db"), b"an instance")?;
    let why = refused(into(first, &empty))?;
    assert!(why.contains("already holds an instance"), "{why}");
    Ok(())
}

#[test]
fn a_whole_copy_and_a_pass_are_not_taken_for_each_other() -> TestResult {
    let mut h = hosts()?;
    let target = h.new.clone();
    // `yulius instance import` refuses a pass.
    h.cut = Some(0);
    let _ = h.warm()?;
    h.reconcile()?;
    let m = h.manifest(true);
    h.stage.begin("n", true, 0, &era())?;
    let mut pass = Vec::new();
    send_live(&h.old, &m, &mut pass, &mut h.stage, (0, 0), &mut |_| {})?;
    let _ = fs::remove_dir_all(&target);
    match run(&pass, &target) {
        Err(Failure::Refused(why)) => assert!(
            why.contains("one pass of `yulius instance migrate`"),
            "{why}"
        ),
        other => return Err(format!("{other:?}").into()),
    }
    assert!(!target.exists());
    // And a move's receiver refuses a whole copy.
    let whole = exported(&target)?;
    let why = refused(h.land(whole))?;
    assert!(why.contains("a whole copy, not a pass"), "{why}");
    assert!(!target.exists());
    Ok(())
}

#[test]
fn a_cut_pass_is_acknowledged_up_to_what_arrived_and_the_next_sends_the_rest() -> TestResult {
    let mut h = hosts()?;
    // The link breaks after the data directory and the key, inside the next header.
    h.cut = Some(10_500);
    let why = failed(h.warm()?)?;
    assert!(why.contains("the stream ended early"), "{why}");
    assert!(
        why.contains("keeps the 2 entries of this pass that arrived"),
        "{why}"
    );
    let last = h.last()?;
    assert_eq!((last.entries, last.complete), (2, false));
    assert_eq!(h.mark()?.acked, 1);

    // The same command again: what was acknowledged is not sent again.
    let found = h.reconcile()?;
    assert_eq!((found.fresh, found.adopted), (false, 2));
    assert!(!h.stage.whole());
    arrived(h.warm()?)?;
    let sent = h.sent()?;
    assert!(!sent.contains(&"data".to_owned()) && !sent.contains(&"data/master.key".to_owned()));
    assert!(sent.contains(&"data/yulius.db".to_owned()), "{sent:?}");
    assert_eq!(sent.len(), 11, "{sent:?}");
    h.reconcile()?;
    assert!(h.stage.whole());
    assert_eq!(fs::read(h.new.join("yulius.db"))?, vec![9u8; 3000]);
    Ok(())
}

#[test]
fn a_damaged_pass_counts_only_up_to_the_last_entry_whose_digest_is_the_senders() -> TestResult {
    let mut h = hosts()?;
    h.reconcile()?;
    let m = h.manifest(true);
    let nonce = m.pass.as_ref().map(|p| p.nonce.clone()).unwrap_or_default();
    h.stage.begin(&nonce, true, 1_000, &era())?;
    let mut out = Vec::new();
    send_live(
        &h.old,
        &m,
        &mut out,
        &mut h.stage,
        (i64::MAX / 2, 0),
        &mut |_| {},
    )?;
    // A byte of the first file's content changes on the way; every header still reads.
    let at = (0..out.len() - 3)
        .step_by(512)
        .find(|&at| &out[at..at + 4] == b"key\0")
        .ok_or("no key in the stream")?;
    out[at] ^= 0x01;
    let why = failed(h.land(out))?;
    assert!(why.contains("checksum does not match"), "{why}");
    assert_ne!(fs::read(h.new.join("master.key"))?, b"key");
    // All entries arrived, none after the damage has the sender's digest before it: only the
    // data directory itself counts, and the damaged file is sent again.
    let found = h.reconcile()?;
    assert_eq!((found.fresh, found.adopted), (false, 0));
    arrived(h.warm()?)?;
    assert_eq!(h.sent()?.len(), 13);
    assert_eq!(fs::read(h.new.join("master.key"))?, b"key");
    Ok(())
}

#[test]
fn a_pass_that_cannot_be_synced_is_not_acknowledged() -> TestResult {
    let mut h = hosts()?;
    arrived(h.warm()?)?;
    fs::write(h.old.join("master.key"), b"KEY")?;
    h.no_sync = true;
    let why = failed(h.warm()?)?;
    assert!(
        why.contains("could not be synced and marked")
            && why.contains("what it holds is no longer known"),
        "{why}"
    );
    // The stream stays as it was marked before its first entry: never closed.
    let last = h.last()?;
    assert!(last.unknown && !last.complete);
    assert_eq!((h.mark()?.acked, last.entries), (2, 0));
    // The old host learns that nothing is known and sends everything again.
    h.no_sync = false;
    let found = h.reconcile()?;
    assert_eq!(
        (
            found.fresh,
            found.adopted,
            h.stage.acked(),
            h.stage.receipts()
        ),
        (false, 0, 2, 0)
    );
    arrived(h.warm()?)?;
    assert_eq!(h.sent()?.len(), 13);
    assert_eq!(fs::read(h.new.join("master.key"))?, b"KEY");
    Ok(())
}

/// The old tree changes in every way a pass must follow.
fn churn(old: &Path) -> io::Result<()> {
    let home = old.join("sandboxes/sb/home");
    // Rewritten in place, same size.
    fs::write(old.join("master.key"), b"KEY")?;
    // Deleted, with a directory.
    fs::remove_dir_all(old.join("sysroots/sb"))?;
    // A file becomes a directory and a directory a file.
    fs::remove_file(home.join("link"))?;
    fs::create_dir(home.join("link"))?;
    fs::write(home.join("link/inside"), b"in")?;
    fs::remove_file(old.join("was-file"))?;
    fs::create_dir(old.join("was-file"))?;
    // The hard link is broken: two files.
    fs::remove_file(home.join("notes-again"))?;
    fs::write(home.join("notes-again"), b"own\n")?;
    // New, with a link.
    fs::write(home.join("fresh"), b"fresh")?;
    fs::hard_link(home.join("fresh"), old.join("fresh-link"))
}

#[test]
fn after_the_frozen_pass_the_staged_copy_is_the_old_tree() -> TestResult {
    let mut h = hosts()?;
    fs::write(h.old.join("was-file"), b"f")?;
    fs::create_dir(h.old.join("was-dir"))?;
    fs::write(h.old.join("was-dir/below"), b"b")?;
    arrived(h.warm()?)?;
    churn(&h.old)?;
    fs::remove_dir_all(h.old.join("was-dir"))?;
    fs::write(h.old.join("was-dir"), b"now a file")?;
    // A warm pass follows what it can: nothing is removed.
    arrived(h.warm()?)?;
    let last = h.last()?;
    assert_eq!(last.deferred.len(), 1, "{:?}", h.sent()?);
    assert!(h.new.join("was-dir/below").exists() && h.new.join("sysroots/sb/upper/tool").exists());
    assert!(h.new.join("sandboxes/sb/home/link/inside").exists());

    // More changes right before the freeze.
    fs::write(h.old.join("sandboxes/sb/home/fresh"), b"FRESH")?;
    // This host's own, which stays.
    fs::create_dir(h.new.join("system"))?;
    fs::write(h.new.join("system/base"), b"host")?;
    let (landed, old) = h.frozen()?;
    let landed = arrived(landed)?;
    assert!(landed.removed >= 3, "{}", landed.removed);
    let last = h.last()?;
    assert!(last.complete && !last.live && last.deferred.is_empty());
    assert_eq!(fs::read(h.new.join("was-dir"))?, b"now a file");
    assert!(!h.new.join("sysroots/sb").exists() && h.new.join("sysroots").is_dir());
    assert_eq!(fs::read(h.new.join("fresh-link"))?, b"FRESH");
    assert_eq!(fs::metadata(h.new.join("fresh-link"))?.nlink(), 2);
    assert_eq!(
        fs::metadata(h.new.join("sandboxes/sb/home/notes"))?.nlink(),
        1
    );
    assert_eq!(fs::read(h.new.join("system/base"))?, b"host");
    assert!(h.new.join(yulius_app::limits::ADAPT_MARK).exists());
    // The proof of the move: both hosts count the same.
    let map = ids();
    let new = of_copy(&h.new, &map, &old)?;
    assert_eq!(crate::instance::inventory::difference(&old, &new), None);
    assert_eq!(old.entries, 15);

    // The move did not go on (its proof, its start): the next pass sends what the frozen one
    // sent, which earned nothing, and nothing else.
    let found = h.reconcile()?;
    assert!(!found.fresh);
    arrived(h.warm()?)?;
    let sent = h.sent()?;
    assert!(
        sent.contains(&"data/sandboxes/sb/home/fresh".to_owned()),
        "{sent:?}"
    );
    assert!(!sent.contains(&"data/yulius.db".to_owned()), "{sent:?}");
    Ok(())
}

#[test]
fn a_frozen_pass_that_is_cut_removes_nothing_and_leaves_no_limits_mark() -> TestResult {
    let mut h = hosts()?;
    arrived(h.warm()?)?;
    fs::remove_file(h.old.join("master.key"))?;
    fs::write(h.old.join("yulius.db"), vec![8u8; 3000])?;
    h.cut = Some(9_500);
    let (landed, _) = h.frozen()?;
    failed(landed)?;
    assert!(h.new.join("master.key").exists());
    assert!(!h.new.join(yulius_app::limits::ADAPT_MARK).exists());
    let last = h.last()?;
    assert!(!last.complete && !last.unknown);
    // The receipts stand; the same command sends the rest and then removes.
    let found = h.reconcile()?;
    assert!(!found.fresh);
    let (landed, old) = h.frozen()?;
    arrived(landed)?;
    assert!(!h.new.join("master.key").exists());
    let new = of_copy(&h.new, &ids(), &old)?;
    assert_eq!(crate::instance::inventory::difference(&old, &new), None);
    Ok(())
}

#[test]
fn the_staged_copy_s_marker_is_never_an_entry() -> TestResult {
    let mut h = hosts()?;
    fs::write(h.old.join(INCOMPLETE), b"left by hand")?;
    let why = failed(h.warm()?)?;
    assert!(why.contains("a name of the staged copy's marker"), "{why}");
    assert!(Mark::read(&h.new).is_some());
    // Nor is the name the marker has while it is rewritten.
    let mut h = hosts()?;
    fs::write(h.old.join(INCOMPLETE_NEW), b"left by hand")?;
    let why = failed(h.warm()?)?;
    assert!(why.contains("..yulius-import-incomplete.tmp"), "{why}");
    Ok(())
}

#[test]
fn a_stream_is_marked_before_its_first_entry_so_a_receive_that_dies_says_so() -> TestResult {
    let mut h = hosts()?;
    arrived(h.warm()?)?;
    fs::write(h.old.join("master.key"), b"KEY")?;
    fs::write(h.old.join("sandboxes/sb/home/notes"), b"HELLO\n")?;
    h.reconcile()?;
    let m = h.manifest(true);
    let nonce = m.pass.as_ref().map(|p| p.nonce.clone()).unwrap_or_default();
    h.stage.begin(&nonce, true, 1_000, &era())?;
    let mut out = Vec::new();
    send_live(
        &h.old,
        &m,
        &mut out,
        &mut h.stage,
        (i64::MAX / 2, 0),
        &mut |_| {},
    )?;
    // What the marker says while the stream is taken in.
    let mut during = Vec::new();
    let new = h.new.clone();
    let landed = land(
        out.as_slice(),
        &new,
        None,
        &mut |_, _| Ok(ids()),
        &mut Ground {
            chown: false,
            sync: &mut |_| Ok(()),
            tick: &mut |_, _| during.extend(fs::read_to_string(new.join(INCOMPLETE))),
        },
    );
    arrived(landed)?;
    assert!(!during.is_empty());
    let open = Mark::parse(&during[0]).ok_or("no marker during the stream")?;
    let last = open.last.clone().ok_or("nothing in it")?;
    assert_eq!((open.acked, last.nonce.as_str()), (2, nonce.as_str()));
    assert!(last.unknown && !last.complete && last.entries == 0);
    // Closed: known again.
    assert!(h.last()?.complete && !h.last()?.unknown);

    // The receive was killed instead: that marker is all there is. Nothing the old host knew
    // holds any more; the stage goes on and the next pass sends everything into it.
    fs::write(h.new.join(INCOMPLETE), &during[0])?;
    let found = h.reconcile()?;
    assert_eq!(
        (found.fresh, h.stage.receipts(), h.stage.acked()),
        (false, 0, 2)
    );
    arrived(h.warm()?)?;
    assert_eq!(h.sent()?.len(), 13);
    assert_eq!(fs::read(h.new.join("master.key"))?, b"KEY");
    assert_eq!(h.mark()?.acked, 3);
    Ok(())
}

#[test]
fn a_frozen_pass_that_removed_a_tree_and_broke_leaves_nothing_known() -> TestResult {
    let mut h = hosts()?;
    fs::create_dir(h.old.join("aaa"))?;
    fs::write(h.old.join("aaa/below"), b"b")?;
    arrived(h.warm()?)?;
    // A directory became a file: the frozen pass removes what was below it on the new host,
    // which the old host never sent as a removal. Then the link breaks, inside the database.
    fs::remove_dir_all(h.old.join("aaa"))?;
    fs::write(h.old.join("aaa"), b"now a file")?;
    fs::write(h.old.join("yulius.db"), vec![8u8; 3000])?;
    h.cut = Some(10_900);
    let (landed, _) = h.frozen()?;
    failed(landed)?;
    assert_eq!(fs::read(h.new.join("aaa"))?, b"now a file");
    let last = h.last()?;
    assert!(!last.complete && last.unknown, "{last:?}");
    assert_eq!(last.entries, 2);
    // Nothing the old host knew holds any more: everything is sent again, into the same stage.
    let found = h.reconcile()?;
    assert_eq!((found.fresh, h.stage.receipts()), (false, 0));
    arrived(h.warm()?)?;
    assert_eq!(h.sent()?.len(), 14);
    let (landed, old) = h.frozen()?;
    arrived(landed)?;
    let new = of_copy(&h.new, &ids(), &old)?;
    assert_eq!(crate::instance::inventory::difference(&old, &new), None);
    Ok(())
}

#[test]
fn a_pass_says_of_the_host_only_what_is_news() {
    use yulius_contracts::update::{HostLine, HostOutcome};
    let line = |outcome| HostLine {
        name: "service user".into(),
        outcome,
        detail: String::new(),
    };
    // A whole copy says every line; a pass, which comes many times, only what changed or failed.
    for outcome in [
        HostOutcome::Ok,
        HostOutcome::Changed,
        HostOutcome::Manual,
        HostOutcome::Failed,
    ] {
        assert!(worth_saying(&line(outcome), false));
        assert_eq!(
            worth_saying(&line(outcome), true),
            outcome != HostOutcome::Ok
        );
    }
}
