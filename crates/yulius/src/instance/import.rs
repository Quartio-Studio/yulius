//! `yulius instance import` (SPEC-329 IMP-001..006): read the manifest, refuse what cannot work
//! before writing anything, make the service user and its ids, extract as the stream arrives,
//! check the checksum and length, and say what comes next.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

use super::archive::{Item, Reader};
use super::extract::{Extractor, IdMap, Onto, Report};
use super::{FORMAT, KEPT, MANIFEST, MANIFEST_LEN, Manifest, Pass, Progress, REFUSED};
use crate::wiring::update_files::write_atomic;

/// The marker of an import that has not finished, at the data directory's top. A move's staged
/// copy keeps its [`Mark`] in it until the copy is proven.
pub const INCOMPLETE: &str = ".yulius-import-incomplete";
/// The marker while it is rewritten ([`write_atomic`]).
const INCOMPLETE_NEW: &str = "..yulius-import-incomplete.tmp";
/// The names a staged copy keeps for its marker at the data directory's top: no entry of a
/// copy may have them.
pub const MARKERS: [&str; 2] = [INCOMPLETE, INCOMPLETE_NEW];

/// What the marker of a staged copy says (SPEC-465 WCP-005): the copy a move fills in passes
/// until it is proven. An empty marker is a plain import that did not finish.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// 1.
    pub version: u32,
    /// The staged copy's id.
    pub stage: String,
    /// Streams acknowledged: installed (wholly or up to where they were cut) and synced.
    pub acked: u64,
    /// What the last of them installed.
    #[serde(default)]
    pub last: Option<Landed>,
}

/// What one stream left in a staged copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Landed {
    /// The stream's own name.
    pub nonce: String,
    /// Its entries taken in, in stream order.
    pub entries: u64,
    /// The stream's digest (8 bytes, hex) before its entry number `entries`, and before the
    /// entry before that one: the old host adopts receipts only up to a digest it recorded.
    pub sums: [String; 2],
    /// The whole stream arrived and its checksum matched.
    pub complete: bool,
    /// It was read while the instance ran.
    pub live: bool,
    /// The entries (by number) that were not installed: a warm pass removes no directory.
    #[serde(default)]
    pub deferred: Vec<u64>,
    /// What the old host knew of the staged copy no longer holds. Written before a stream's
    /// first entry, so a receive that dies says so; kept by a stream that removed paths other
    /// than its own entries' and did not complete.
    #[serde(default)]
    pub unknown: bool,
}

impl Mark {
    /// The marker of the data directory `target`; `None` when there is none or it names no
    /// stage.
    #[must_use]
    pub fn read(target: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(target.join(INCOMPLETE)).ok()?)
    }

    /// A marker's text as a stage; `None` for a plain import's empty marker.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str::<Self>(text)
            .ok()
            .filter(|mark| mark.version == 1 && !mark.stage.is_empty())
    }
}

/// Options of `yulius instance import`.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// The archive; standard input when `None` or `-`.
    pub file: Option<PathBuf>,
    /// The data directory, chosen here (never by the copy) and refused unless it is the one the
    /// copy was exported from.
    pub data_dir: PathBuf,
    /// The service user when the data directory does not exist yet (default: the exported one).
    pub user: Option<String>,
}

/// Why an import did not happen or did not finish.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// Refused before anything was written.
    Refused(String),
    /// Failed after writing began: the data directory holds an incomplete copy.
    Failed(String),
}

/// Free space, and free inodes when the filesystem counts them (btrfs reports none), of the
/// filesystem that will hold `dir` (its nearest existing ancestor).
fn free(dir: &Path) -> Option<(u64, Option<u64>)> {
    let mut at = Some(dir);
    while let Some(path) = at {
        if let Ok(st) = rustix::fs::statvfs(path) {
            let inodes = (st.f_files > 0).then_some(st.f_favail);
            return Some((st.f_bavail.saturating_mul(st.f_frsize), inodes));
        }
        at = path.parent();
    }
    None
}

/// Whether `name` at the top of an existing target may stay: the filesystem's `lost+found`, the
/// dotfiles `useradd --create-home` copies from `/etc/skel`, and what a copy never holds because
/// each host makes its own ([`super::walk::LEFT_OUT`]: the sandbox base a move builds before
/// its copy, SPEC-460 MIG-024).
fn may_be_there(name: &std::ffi::OsStr, meta: &std::fs::Metadata) -> bool {
    let name = name.to_string_lossy();
    (name == "lost+found" && meta.is_dir())
        || (name.starts_with('.') && meta.is_file())
        || super::walk::LEFT_OUT.contains(&name.as_ref())
}

/// What an existing target must be (IMP-001): a directory, not a link, holding nothing but
/// [`may_be_there`], owned by `user` (or by root when empty, which the person fixes first).
fn existing_target(target: &Path, user: &str) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return Ok(());
    };
    let shown = target.display();
    if !meta.is_dir() {
        return Err(format!("{shown} exists and is not a directory"));
    }
    let entries = std::fs::read_dir(target).map_err(|e| format!("{shown}: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{shown}: {e}"))?;
        let name = entry.file_name();
        let kept = entry.metadata().is_ok_and(|m| may_be_there(&name, &m));
        if kept {
            continue;
        }
        if name == "yulius.db" || name == "sandboxes" {
            return Err(format!(
                "{shown} already holds an instance ({}); an import never overwrites one",
                name.to_string_lossy()
            ));
        }
        return Err(format!(
            "{shown} holds {:?}: an import goes only into a new or empty data directory",
            name.to_string_lossy()
        ));
    }
    use std::os::unix::fs::MetadataExt as _;
    let passwd = super::read_text(Path::new("/etc/passwd")).unwrap_or_default();
    let owner = super::user_by_uid(&passwd, meta.uid())
        .map_or_else(|| meta.uid().to_string(), |(name, _)| name);
    if meta.uid() == 0 {
        return Err(format!(
            "{shown} belongs to root (a new disk?); give it to the service user first: `sudo \
             useradd --system --home-dir {shown} --shell /usr/sbin/nologin {user}; sudo chown \
             {user}: {shown}; sudo chmod 0700 {shown}`"
        ));
    }
    if owner != user {
        return Err(format!(
            "{shown} belongs to {owner}, not to the copy's service user {user} (--user names \
             another)"
        ));
    }
    Ok(())
}

/// The checks before anything is written (IMP-001): the format, the schema, the target the
/// person chose (never the manifest's alone), what is already there, and room for the copy.
///
/// # Errors
/// Each refusal, saying why and what to do.
pub fn check(m: &Manifest, target: &Path, user: &str, own_schema: u32) -> Result<(), String> {
    if m.pass.is_some() {
        return Err(
            "the stream is one pass of `yulius instance migrate`, not a whole copy: only that \
             command installs it"
                .into(),
        );
    }
    check_stream(m, target, own_schema)?;
    if std::fs::symlink_metadata(target.join(INCOMPLETE)).is_ok() {
        return Err(format!(
            "an earlier import into {} did not finish: remove that directory (`sudo rm -rf {}`) \
             and import again",
            target.display(),
            target.display()
        ));
    }
    existing_target(target, user)?;
    if let Some((bytes, inodes)) = free(target) {
        if bytes < m.total_bytes {
            return Err(format!(
                "the copy needs {}, {} is free where {} goes",
                yulius_import::human_bytes(m.total_bytes),
                yulius_import::human_bytes(bytes),
                target.display()
            ));
        }
        if let Some(inodes) = inodes
            && inodes < m.entries
        {
            return Err(format!(
                "the copy has {} files and directories, the filesystem of {} has {inodes} inodes \
                 free",
                m.entries,
                target.display()
            ));
        }
    }
    Ok(())
}

/// What holds for a whole copy and for a pass alike: a format and schema this build reads, and
/// the target being the plain path the stream was read from.
fn check_stream(m: &Manifest, target: &Path, own_schema: u32) -> Result<(), String> {
    if m.format != FORMAT {
        return Err(format!(
            "the copy is in format {}, this Yulius reads format {FORMAT}",
            m.format
        ));
    }
    if m.schema > own_schema {
        return Err(format!(
            "the copy's database is newer than this Yulius knows (schema {} > {own_schema}): \
             install a Yulius at least as new as the exported one (build {})",
            m.schema,
            m.yulius_build.as_deref().unwrap_or("unknown")
        ));
    }
    let plain = target.is_absolute()
        && target
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && target.parent().is_some();
    if !plain {
        return Err(format!(
            "{} is not a plain absolute path below /",
            target.display()
        ));
    }
    if Path::new(&m.data_dir) != target {
        return Err(format!(
            "the copy was exported from {}; import it into the same path (`--data-dir {}`): Git \
             worktrees, the database and sandbox files hold absolute paths under it",
            m.data_dir, m.data_dir
        ));
    }
    Ok(())
}

/// Reads the manifest, the archive's first entry.
pub(crate) fn manifest<R: Read>(r: &mut Reader<R>) -> Result<Manifest, String> {
    let first = r.next_item().map_err(|e| e.to_string())?;
    let Item::Entry(e) = first else {
        return Err("not a Yulius export: it has no manifest".into());
    };
    if e.path != MANIFEST || e.size() != MANIFEST_LEN {
        return Err("not a Yulius export: it does not start with its manifest".into());
    }
    let mut bytes = Vec::new();
    r.data_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("not a Yulius export: its manifest: {e}"))
}

/// Installs the archive read from `input`: manifest checks, `prepare` (the host's user and ids,
/// then the id map), the incomplete marker, every entry, the checksum and length, the marker
/// removed. `tick` gets the bytes read.
///
/// # Errors
/// [`Failure::Refused`] before anything was written, else [`Failure::Failed`].
pub fn install(
    input: impl Read,
    target: &Path,
    user: Option<&str>,
    prepare: &mut dyn FnMut(&Manifest, &Path) -> Result<IdMap, String>,
    chown: bool,
    tick: &mut dyn FnMut(&Manifest, u64),
) -> Result<(Manifest, Report), Failure> {
    let mut r = Reader::new(input);
    let m = manifest(&mut r).map_err(Failure::Refused)?;
    let user = user.unwrap_or(&m.service.user);
    check(&m, target, user, yulius_store::schema_version()).map_err(Failure::Refused)?;
    let map = prepare(&m, target).map_err(Failure::Refused)?;
    let owner = (map.uid.1, map.gid.1);
    let failed = |e: io::Error| {
        Failure::Failed(format!(
            "{e}; {} holds an incomplete copy: remove it (`sudo rm -rf {}`) before importing \
             again",
            target.display(),
            target.display()
        ))
    };
    std::fs::create_dir_all(target).map_err(|e| Failure::Refused(e.to_string()))?;
    // Created new: a link planted at the marker's name is never followed.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target.join(INCOMPLETE))
        .map_err(|e| Failure::Refused(format!("cannot mark the import as unfinished: {e}")))?;
    let mut ex = Extractor::new(target, map, chown).map_err(failed)?;
    loop {
        match r.next_item().map_err(failed)? {
            Item::End => break,
            Item::Entry(e) => {
                ex.put(&e, &mut r.data_reader()).map_err(failed)?;
                tick(&m, r.read_bytes());
            }
        }
    }
    let report = ex.finish().map_err(failed)?;
    if r.read_bytes() != m.total_bytes {
        return Err(failed(io::Error::other(format!(
            "the copy is {} bytes long, its manifest says {}",
            r.read_bytes(),
            m.total_bytes
        ))));
    }
    limits_mark(target, chown.then_some(owner)).map_err(failed)?;
    std::fs::remove_file(target.join(INCOMPLETE)).map_err(failed)?;
    Ok((m, report))
}

/// What a pass of a move left in the staged copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// The stream's manifest.
    pub manifest: Manifest,
    /// What the extraction changed from the archive.
    pub report: Report,
    /// The marker as the pass left it: what the old host adopts.
    pub mark: Mark,
    /// Entries removed because the old host no longer holds them (the frozen pass).
    pub removed: u64,
}

/// What a pass needs of the host it lands on.
pub struct Ground<'a> {
    /// Own files by their mapped ids (off only where the caller cannot: tests not run as
    /// root).
    pub chown: bool,
    /// Syncs the filesystem that holds the data directory ([`super::export::sync`]).
    pub sync: &'a mut dyn FnMut(&Path) -> io::Result<()>,
    /// Gets the bytes read so far.
    pub tick: &'a mut dyn FnMut(&Manifest, u64),
}

/// Installs one pass of a move read from `input` (SPEC-465 WCP-005, WCP-008, WCP-011): the
/// checks a whole copy gets, `prepare` (the host's user and ids, then the id map), then
/// [`take_in`].
///
/// # Errors
/// [`Failure::Refused`] before anything was written, else [`Failure::Failed`]: the staged copy
/// keeps what arrived, and its marker says how much.
pub fn land(
    input: impl Read,
    target: &Path,
    user: Option<&str>,
    prepare: &mut dyn FnMut(&Manifest, &Path) -> Result<IdMap, String>,
    ground: &mut Ground<'_>,
) -> Result<Landing, Failure> {
    let mut r = Reader::new(input);
    let m = manifest(&mut r).map_err(Failure::Refused)?;
    if m.pass.is_none() {
        return Err(Failure::Refused(
            "the stream is a whole copy, not a pass of a move".into(),
        ));
    }
    check_stream(&m, target, yulius_store::schema_version()).map_err(Failure::Refused)?;
    let user = user.unwrap_or(&m.service.user).to_owned();
    // Before the host is touched: a stream that is not of the stage here changes nothing.
    stage_of(&m, target).map_err(Failure::Refused)?;
    let map = prepare(&m, target).map_err(Failure::Refused)?;
    take_in(r, &m, target, &user, map, ground)
}

/// The staged copy's marker when the pass of `m` builds on it; `None` when the pass begins a
/// stage in a data directory that holds none.
fn stage_of(m: &Manifest, target: &Path) -> Result<Option<Mark>, String> {
    let pass = m.pass.as_ref().ok_or("the stream is not a pass")?;
    let shown = target.display();
    let marked = std::fs::symlink_metadata(target.join(INCOMPLETE)).is_ok();
    match Mark::read(target) {
        Some(mark) if mark.stage == pass.stage && mark.acked == pass.seq => Ok(Some(mark)),
        Some(mark) => Err(format!(
            "{shown} holds the staged copy {} after {} passes; the stream is pass {} of {}",
            mark.stage, mark.acked, pass.seq, pass.stage
        )),
        None if marked => Err(format!(
            "an earlier import into {shown} did not finish and is not a move's staged copy"
        )),
        None if pass.seq == 0 => Ok(None),
        None => Err(format!(
            "{shown} holds no staged copy; the stream is pass {} of {}",
            pass.seq, pass.stage
        )),
    }
}

fn write_mark(target: &Path, mark: &Mark) -> io::Result<()> {
    let json = serde_json::to_vec(mark).map_err(io::Error::other)?;
    write_atomic(&target.join(INCOMPLETE), &json, 0o600)?;
    // The name too: the old host skips what this says is here.
    File::open(target)?.sync_all()
}

/// The names at a staged copy's top that are this host's own: what a copy never carries and
/// the marker.
fn own_name(name: &[u8]) -> bool {
    !super::walk::copied(name) || name == INCOMPLETE.as_bytes() || name == INCOMPLETE_NEW.as_bytes()
}

/// Takes the entries of a pass into the staged copy at `target`, after its manifest `m` was
/// read from `r` and found fit for this host. The stream must be the next of the stage here,
/// or the first into an empty directory. The marker names the stream before its first entry
/// and says that nothing is known of the copy, which is what a receive that dies leaves.
/// Whatever the stream ends with, the entries that arrived stay: the directories' times are
/// set, the filesystem is synced and the marker then says how many entries were taken in and
/// what the stream's digest was there, so the old host adopts exactly them. A frozen pass that
/// arrived whole also removes what its kept paths do not name and leaves the limits mark.
///
/// # Errors
/// [`Failure::Refused`] when the stream is not of the stage here (nothing was written), else
/// [`Failure::Failed`].
pub fn take_in<R: Read>(
    mut r: Reader<R>,
    m: &Manifest,
    target: &Path,
    user: &str,
    map: IdMap,
    ground: &mut Ground<'_>,
) -> Result<Landing, Failure> {
    let pass = m
        .pass
        .clone()
        .ok_or_else(|| Failure::Refused("the stream is not a pass".into()))?;
    let stage = stage_of(m, target).map_err(Failure::Refused)?;
    let marked = |e: io::Error| Failure::Refused(format!("cannot mark the staged copy: {e}"));
    if stage.is_none() {
        existing_target(target, user).map_err(Failure::Refused)?;
        std::fs::create_dir_all(target).map_err(|e| Failure::Refused(e.to_string()))?;
        // Created new: a link planted at the marker's name is never followed.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target.join(INCOMPLETE))
            .map_err(marked)?;
    }
    // Until this stream is closed below, nothing the old host knew of the staged copy holds:
    // a receive that dies (a kill, a power cut) leaves files half written and directories'
    // times moved, and this marker is all that says so.
    let open = Mark {
        version: 1,
        stage: pass.stage.clone(),
        acked: pass.seq + 1,
        last: Some(Landed {
            nonce: pass.nonce.clone(),
            live: pass.live,
            unknown: true,
            ..Landed::default()
        }),
    };
    write_mark(target, &open).map_err(marked)?;
    let onto = if pass.live { Onto::Warm } else { Onto::Frozen };
    let mut ex = Extractor::onto(target, map, ground.chown, onto)
        .map_err(|e| Failure::Failed(e.to_string()))?;
    let Arrived {
        taken,
        sums,
        kept,
        mut failure,
    } = entries(&mut r, m, &pass, &mut ex, ground);
    let whole = failure.is_none();
    let mut removed = 0;
    if let (true, Some(mut kept)) = (whole, kept) {
        kept.sort_unstable();
        match ex.prune(&kept, &own_name) {
            Ok(count) => removed = count,
            Err(e) => {
                failure = Some(format!(
                    "cannot remove what the old host no longer holds: {e}"
                ))
            }
        }
    }
    let deferred = ex.deferred().to_vec();
    // What the old host knows of the staged copy no longer holds when the pass removed paths
    // it never sent and broke, or when the directories it wrote into kept the time of that.
    let mut unknown = ex.cleared();
    let report = ex.finish().unwrap_or_else(|e| {
        unknown = true;
        failure.get_or_insert(format!("cannot set the directories' times: {e}"));
        Report::default()
    });
    if failure.is_none()
        && !pass.live
        && let Err(e) = limits_mark(target, ground.chown.then_some((map.uid.1, map.gid.1)))
    {
        failure = Some(e.to_string());
    }
    let mark = Mark {
        version: 1,
        stage: pass.stage.clone(),
        acked: pass.seq + 1,
        last: Some(Landed {
            nonce: pass.nonce.clone(),
            entries: taken,
            sums: sums.map(hex::encode),
            complete: failure.is_none(),
            live: pass.live,
            deferred,
            unknown: unknown && failure.is_some(),
        }),
    };
    // Acknowledged only once it is on disk: first what arrived, then the marker that says so.
    let acknowledged = (ground.sync)(target).and_then(|()| write_mark(target, &mark));
    match (failure, acknowledged) {
        (None, Ok(())) => Ok(Landing {
            manifest: m.clone(),
            report,
            mark,
            removed,
        }),
        (Some(why), Ok(())) => Err(Failure::Failed(format!(
            "{why}; the staged copy in {} keeps the {taken} entries of this pass that arrived",
            target.display()
        ))),
        (why, Err(e)) => Err(Failure::Failed(format!(
            "{}the staged copy in {} could not be synced and marked ({e}): what it holds is no \
             longer known, and the next pass sends everything again",
            why.map(|why| format!("{why}; ")).unwrap_or_default(),
            target.display()
        ))),
    }
}

/// What arrived of a pass.
struct Arrived {
    /// Entries taken in.
    taken: u64,
    /// The stream's digest before the next entry, and before the last one taken.
    sums: [[u8; 8]; 2],
    /// The kept paths of an exact pass.
    kept: Option<Vec<[u8; 16]>>,
    /// Why the stream did not arrive whole.
    failure: Option<String>,
}

/// Reads a pass's entries into `ex`.
fn entries<R: Read>(
    r: &mut Reader<R>,
    m: &Manifest,
    pass: &Pass,
    ex: &mut Extractor,
    ground: &mut Ground<'_>,
) -> Arrived {
    let mut taken = 0u64;
    // Where the writer stood before its first entry; a stream that ends here fails below.
    let _ = r.end_entry();
    let mut sums = [r.sum(); 2];
    let mut kept: Option<Vec<[u8; 16]>> = None;
    let markers = MARKERS.map(|name| [b"data/", name.as_bytes()].concat());
    let ended: io::Result<()> = loop {
        let e = match r.next_item() {
            Ok(Item::End) => break Ok(()),
            Ok(Item::Entry(e)) => e,
            Err(e) => break Err(e),
        };
        if kept.is_some() {
            break Err(io::Error::other("an entry follows the kept paths"));
        }
        if e.path == KEPT {
            let mut bytes = Vec::new();
            if let Err(e) = r.data_reader().read_to_end(&mut bytes) {
                break Err(e);
            }
            if pass.live || bytes.len() as u64 != pass.kept.saturating_mul(16) {
                break Err(io::Error::other(
                    "the kept paths are not what the manifest says",
                ));
            }
            kept = Some(bytes.as_chunks::<16>().0.to_vec());
            continue;
        }
        if markers.contains(&e.path) {
            break Err(io::Error::other(format!(
                "the old data directory holds {}, a name of the staged copy's marker",
                String::from_utf8_lossy(&e.path[5..])
            )));
        }
        if let Err(e) = ex
            .put(&e, &mut r.data_reader())
            .and_then(|()| r.end_entry())
        {
            break Err(e);
        }
        taken += 1;
        sums = [r.sum(), sums[0]];
        (ground.tick)(m, r.read_bytes());
    };
    let failure = match ended {
        Err(e) => Some(e.to_string()),
        Ok(()) if !pass.live && r.read_bytes() != m.total_bytes => Some(format!(
            "the pass is {} bytes long, its manifest says {}",
            r.read_bytes(),
            m.total_bytes
        )),
        Ok(()) if !pass.live && kept.is_none() => {
            Some("an exact pass came without its kept paths".to_owned())
        }
        Ok(()) => None,
    };
    Arrived {
        taken,
        sums,
        kept,
        failure,
    }
}

/// Leaves the mark that makes the first start here size the limits for this host (SPEC-459
/// HSL-005), owned by the service user. Whatever the copy brought at its name is replaced, a
/// link never followed.
fn limits_mark(target: &Path, owner: Option<(u32, u32)>) -> io::Result<()> {
    let mark = target.join(yulius_app::limits::ADAPT_MARK);
    match std::fs::remove_file(&mark) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&mark)?;
    if let Some((uid, gid)) = owner {
        std::os::unix::fs::fchown(&file, Some(uid), Some(gid))?;
    }
    Ok(())
}

/// `host setup`'s options for the copy's instance on this host.
fn host_options(m: &Manifest, target: &Path, user: Option<&str>) -> crate::host_setup::Options {
    crate::host_setup::Options {
        data_dir: target.to_path_buf(),
        user: user.unwrap_or(&m.service.user).to_owned(),
        public_url: None,
        exe: std::env::current_exe().unwrap_or_default(),
        smoke: false,
        service: m.service.unit.clone(),
        update_state: None,
    }
}

/// Whether this host cannot start a sandbox's namespace yet (SPEC-456 BNS-010): `host setup`'s
/// item in check mode, which changes nothing.
fn lacks_namespaces(m: &Manifest, user: Option<&str>) -> bool {
    let mut host = crate::host_setup::system::RealHost { quiet: true };
    let lines = crate::host_setup::Run::new(
        &mut host,
        crate::host_setup::Mode::Check,
        host_options(m, Path::new(&m.data_dir), user),
        &mut |_| {},
    )
    .namespaces();
    crate::host_setup::lacks_namespaces(&lines)
}

/// What to do after an import. `namespaces` says that bwrap cannot start a sandbox's namespace
/// here yet, which the setup fixes first.
fn next_steps(url: &str, unit: &str, namespaces: bool) -> String {
    let first = if namespaces {
        "lets bwrap start the user namespaces sandboxes run in, which this host does not allow \
         yet; "
    } else {
        ""
    };
    format!(
        "Next:\n  1. sudo yulius host setup --public-url {url}\n     ({first}writes {unit}, \
         builds the sandbox base for this host, checks Docker, starts Yulius)\n  2. Copy other \
         Environment= lines you changed in the old {unit} (keys), then `sudo systemctl restart \
         {unit}`. Resource limits are sized for this host at the first start (Settings, \
         Instance shows them): leave the old YULIUS_MAX_SESSIONS, YULIUS_CI_* and \
         YULIUS_SANDBOX_*_GB numbers behind.\n  3. Point the reverse proxy and DNS here.\n  4. \
         Keep the old instance frozen or stopped (`yulius instance export --move` does that): \
         both would run the same agents on the same repositories."
    )
}

/// Whether a line of the host's preparation is said: all of them for a whole copy, only what
/// is not `ok` for a pass of a move, which comes many times (`only_news`).
fn worth_saying(line: &yulius_contracts::update::HostLine, only_news: bool) -> bool {
    !only_news || line.outcome != yulius_contracts::update::HostOutcome::Ok
}

/// Makes the service user and its ids with `host setup`'s items and maps the copy's ids to them.
/// With `only_news` it says only what it changed or could not do.
pub(crate) fn prepare_host(
    m: &Manifest,
    target: &Path,
    user: Option<&str>,
    only_news: bool,
) -> Result<IdMap, String> {
    let opts = host_options(m, target, user);
    let mut host = crate::host_setup::system::RealHost { quiet: false };
    let mut print = |line: &yulius_contracts::update::HostLine| {
        if worth_saying(line, only_news) {
            eprintln!("{}", crate::host_setup::format_line(line));
        }
    };
    let lines = crate::host_setup::Run::new(
        &mut host,
        crate::host_setup::Mode::Setup,
        opts.clone(),
        &mut print,
    )
    .user_and_ids();
    if !crate::host_setup::passed(&lines) {
        return Err("the service user or its subordinate ids could not be made (see above)".into());
    }
    id_map(m, &opts.user)
}

/// How the copy's ids map to this host's service user `user` and its subordinate ranges.
pub(crate) fn id_map(m: &Manifest, user: &str) -> Result<IdMap, String> {
    let passwd = super::read_text(Path::new("/etc/passwd")).unwrap_or_default();
    let (uid, gid) =
        super::user_by_name(&passwd, user).ok_or_else(|| format!("no user {user} on this host"))?;
    let range = |file: &str, old: Option<super::IdRange>| -> Result<_, String> {
        let Some(old) = old else { return Ok(None) };
        let text = super::read_text(Path::new(file)).unwrap_or_default();
        let (start, count) = yulius_sandbox::subid::find_range(&text, user, uid)
            .ok_or_else(|| format!("{user} has no range in {file}"))?;
        if count < old.count {
            return Err(format!(
                "{user}'s range in {file} has {count} ids, the copy's had {}",
                old.count
            ));
        }
        Ok(Some((old.start, start, old.count)))
    };
    Ok(IdMap {
        uid: (m.service.uid, uid),
        gid: (m.service.gid, gid),
        subuid: range("/etc/subuid", m.subuid)?,
        subgid: range("/etc/subgid", m.subgid)?,
    })
}

/// `yulius instance import`.
#[must_use]
pub fn main(opts: &ImportOptions) -> ExitCode {
    let refused = |why: &str| {
        eprintln!("yulius instance import: {why}");
        ExitCode::from(REFUSED)
    };
    if !super::is_root() {
        return refused(
            "run it as root (sudo): it makes the service user and owns files by its ids",
        );
    }
    let input: Box<dyn Read> = match opts.file.as_deref() {
        None => Box::new(io::stdin().lock()),
        Some(p) if p == Path::new("-") => Box::new(io::stdin().lock()),
        Some(p) => match File::open(p) {
            Ok(f) => Box::new(f),
            Err(e) => return refused(&format!("{}: {e}", p.display())),
        },
    };
    let input = BufReader::with_capacity(1 << 20, input);
    let mut progress: Option<Progress> = None;
    let mut prepare = |m: &Manifest, target: &Path| {
        eprintln!(
            "Importing {} ({} entries) into {}, exported from {} by build {}.",
            yulius_import::human_bytes(m.total_bytes),
            m.entries,
            target.display(),
            m.os_release.as_deref().unwrap_or("an unknown host"),
            m.yulius_build.as_deref().unwrap_or("unknown")
        );
        let here =
            super::read_text(Path::new("/etc/os-release")).and_then(|t| super::os_release(&t));
        if m.os_release.is_some() && here != m.os_release {
            eprintln!(
                "Warning: this host runs {}, the copy comes from {}: packages installed in \
                 sandboxes sit on this host's system and may need reinstalling.",
                here.as_deref().unwrap_or("an unknown system"),
                m.os_release.as_deref().unwrap_or_default()
            );
        }
        prepare_host(m, target, opts.user.as_deref(), false)
    };
    let result = install(
        input,
        &opts.data_dir,
        opts.user.as_deref(),
        &mut prepare,
        true,
        &mut |m, done| {
            progress
                .get_or_insert_with(|| Progress::new("Imported", m.total_bytes))
                .tick(done);
        },
    );
    if let Some(p) = &progress {
        p.end();
    }
    match result {
        Ok((m, report)) => {
            let secs = progress.as_ref().map_or(0, |p| p.elapsed().as_secs());
            eprintln!(
                "Imported {} ({} entries) into {} in {secs} s.",
                yulius_import::human_bytes(m.total_bytes),
                report.entries,
                m.data_dir
            );
            if report.foreign_owners > 0 {
                eprintln!(
                    "{} entries belonged to neither the service user nor its ids and now belong \
                     to the service user, without setuid or setgid.",
                    report.foreign_owners
                );
            }
            if report.skipped_xattrs > 0 {
                eprintln!(
                    "{} extended attributes were not restored (file capabilities, ACLs, others \
                     than user.* and trusted.overlay.*).",
                    report.skipped_xattrs
                );
            }
            let url = m
                .public_url
                .as_deref()
                .unwrap_or("https://yulius.example.com");
            eprintln!(
                "{}",
                next_steps(
                    url,
                    &m.service.unit,
                    lacks_namespaces(&m, opts.user.as_deref())
                )
            );
            ExitCode::SUCCESS
        }
        Err(Failure::Refused(why)) => refused(&why),
        Err(Failure::Failed(why)) => {
            eprintln!("yulius instance import: {why}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
