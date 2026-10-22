//! Agent states (SPEC-271, ADR-0200): a project's folders of operator state (a deploy tool's
//! inventory, rollout folders, configs) moved from a laptop. A person sends a folder as one gzip
//! tar, sealed in the vault per name and version; a task restores it into the sandbox home and
//! saves it back after each rollout. Once restored, the sandbox's copy is the only writer: a new
//! import over it is refused unless the person asks to replace it.
//!
//! Yulius runs outside the agent's containment and with more rights than it, so the home is
//! reached as `secret_copy_to_env` reaches its target: from the canonical sandbox folder, one
//! folder at a time without following links (ADR-0174). No answer, message or audit detail holds
//! a state's content; file names are not content.

use std::cell::Cell;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use nix::errno::Errno;
use nix::fcntl::{OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fchmod, mkdirat};
use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::secrets::AgentStateView;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::vault::names::SecretName;

use super::create::{Root, source};
use super::invalid;
use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::files::{errno_error, io_error, kind_at, names_in, open_dir_at, remove_tree};
use crate::ports::Envelope;
use crate::ports::vault::{AgentScope, AgentStateRecord, StateRestore};

/// Largest packed state (the request body and the stored archive).
pub const MAX_STATE_BYTES: u64 = 64 << 20;
/// Most bytes a state's files may hold in all.
pub const MAX_STATE_EXPANDED: u64 = 512 << 20;
/// Most files and folders in a state.
pub const MAX_STATE_ENTRIES: u64 = 20_000;
/// Deepest nesting of a state's folders.
const MAX_STATE_DEPTH: usize = 64;
/// The permission bits kept; setuid, setgid and sticky are dropped.
const MODE_BITS: u32 = 0o777;
/// The permission bits a state keeps, as a tar header writes them and as `nix` takes them:
/// mapped bit by bit, since `mode_t` is `u16` on macOS and `u32` on Linux (ADR-0169).
const PERMISSIONS: [(u32, Mode); 9] = [
    (0o400, Mode::S_IRUSR),
    (0o200, Mode::S_IWUSR),
    (0o100, Mode::S_IXUSR),
    (0o040, Mode::S_IRGRP),
    (0o020, Mode::S_IWGRP),
    (0o010, Mode::S_IXGRP),
    (0o004, Mode::S_IROTH),
    (0o002, Mode::S_IWOTH),
    (0o001, Mode::S_IXOTH),
];

/// A header's mode as `nix` permission bits (setuid, setgid and sticky dropped).
fn to_mode(bits: u32) -> Mode {
    PERMISSIONS
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .fold(Mode::empty(), |mode, (_, flag)| mode | *flag)
}

/// A file's permission bits as a tar header writes them.
fn from_mode(mode: Mode) -> u32 {
    PERMISSIONS
        .iter()
        .filter(|(_, flag)| mode.contains(*flag))
        .fold(0, |bits, (bit, _)| bits | bit)
}

/// Longest description.
const MAX_DESCRIPTION_CHARS: usize = 500;

/// What a state holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Contents {
    /// Regular files.
    pub(crate) files: u64,
    /// Their bytes.
    pub(crate) bytes: u64,
}

/// One entry of a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Folder,
}

fn refused(why: impl std::fmt::Display) -> AppError {
    AppError::Invalid(format!("the state {why}; nothing was stored"))
}

/// `raw` as a path inside the state's folder: plain names only (`.` skipped), no NUL.
fn state_relative(raw: &[u8]) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains(&0) {
        return None;
    }
    let mut out = PathBuf::new();
    for component in Path::new(OsStr::from_bytes(raw)).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Most bytes the tar reader may read for one entry's header and its extensions (a long name, pax
/// records): far more than any real path needs, far less than memory.
const HEADER_ALLOWANCE: u64 = 64 << 10;

/// How far the tar reader may read into the unpacked stream (review of SPEC-271, B1). The `tar`
/// crate loads extension headers whole and takes an entry's size from its pax records, so the
/// bound is on what is read: [`HEADER_ALLOWANCE`] for each header, then the size of the entry it
/// yields, which counts toward [`MAX_STATE_EXPANDED`] before any of it is read.
#[derive(Default)]
struct Budget {
    read: Cell<u64>,
    limit: Cell<u64>,
    data_end: Cell<u64>,
    hit: Cell<bool>,
}

impl Budget {
    /// Before a header: up to [`HEADER_ALLOWANCE`] past the end of the last entry's data and its
    /// padding. Set, never added to, so a header's unused allowance is gone with it (review of
    /// SPEC-271, round 2).
    fn header(&self) {
        self.limit.set(
            self.data_end
                .get()
                .saturating_add(511)
                .saturating_add(HEADER_ALLOWANCE),
        );
    }

    /// After a header: its entry's `size` bytes of data, and nothing more until the next header.
    fn data(&self, size: u64) {
        let end = self.read.get().saturating_add(size);
        self.data_end.set(end);
        self.limit.set(end);
    }
}

/// The unpacked stream, cut where the [`Budget`] ends.
struct Capped<R> {
    inner: R,
    budget: Rc<Budget>,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.budget.read.get();
        let left = self.budget.limit.get().saturating_sub(read);
        if left == 0 && !buf.is_empty() {
            self.budget.hit.set(true);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the state reads past its bound",
            ));
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        self.budget
            .read
            .set(read + u64::try_from(n).unwrap_or(u64::MAX));
        Ok(n)
    }
}

/// Whether `raw` names the state's folder itself (`.`, `./`), as `tar -C dir -czf x.tgz .` writes.
fn is_top(raw: &[u8]) -> bool {
    !raw.is_empty()
        && Path::new(OsStr::from_bytes(raw))
            .components()
            .all(|c| c == Component::CurDir)
}

/// Reads a packed state, checking every entry (only files and folders, paths inside the folder,
/// the bounds), and hands each one to `visit` with its permission bits and, for a file, its
/// content.
fn walk_archive(
    archive: &[u8],
    mut visit: impl FnMut(&Path, Kind, u32, &mut dyn Read) -> AppResult<()>,
) -> AppResult<Contents> {
    let budget = Rc::new(Budget::default());
    let unreadable = |err: io::Error| {
        if budget.hit.get() {
            refused(format_args!(
                "holds a header larger than {} KiB",
                HEADER_ALLOWANCE >> 10
            ))
        } else {
            refused(format_args!("is not a readable gzip tar ({err})"))
        }
    };
    let mut tar = tar::Archive::new(Capped {
        inner: GzDecoder::new(archive),
        budget: Rc::clone(&budget),
    });
    let (mut entries, mut real, mut unpacked) = (0_u64, 0_u64, 0_u64);
    let mut contents = Contents::default();
    let mut all = tar.entries().map_err(unreadable)?;
    loop {
        // The next header, after whatever of the last entry's data was not read.
        budget.header();
        let Some(entry) = all.next() else { break };
        let mut entry = entry.map_err(unreadable)?;
        // Every entry counts, also those skipped below (global headers, `./`).
        entries += 1;
        if entries > MAX_STATE_ENTRIES {
            return Err(refused(format_args!(
                "holds more than {MAX_STATE_ENTRIES} files and folders"
            )));
        }
        // The size the reader will use: a pax record's, when there is one.
        let size = entry.size();
        unpacked = unpacked.saturating_add(size);
        if unpacked > MAX_STATE_EXPANDED {
            return Err(refused(format_args!(
                "holds more than {} MiB once unpacked",
                MAX_STATE_EXPANDED >> 20
            )));
        }
        budget.data(size);
        let header = entry.header();
        let kind = match header.entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => Kind::File,
            tar::EntryType::Directory => Kind::Folder,
            // Extended headers carry names and times, never entries of their own.
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => continue,
            tar::EntryType::Symlink | tar::EntryType::Link => {
                let shown = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
                return Err(refused(format_args!(
                    "holds a link at {shown}: only files and folders move"
                )));
            }
            _ => {
                let shown = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
                return Err(refused(format_args!(
                    "holds {shown}, which is neither a file nor a folder"
                )));
            }
        };
        let mode = header.mode().map_err(unreadable)? & MODE_BITS;
        let raw = entry.path_bytes().into_owned();
        if kind == Kind::Folder && is_top(&raw) {
            continue;
        }
        let shown = String::from_utf8_lossy(&raw).into_owned();
        let rel = state_relative(&raw)
            .ok_or_else(|| refused(format_args!("names {shown:?}, outside its folder")))?;
        real += 1;
        if rel.components().count() > MAX_STATE_DEPTH {
            return Err(refused(format_args!(
                "nests folders more than {MAX_STATE_DEPTH} deep"
            )));
        }
        if kind == Kind::File {
            contents.files += 1;
            contents.bytes = contents.bytes.saturating_add(size);
        }
        visit(&rel, kind, mode, &mut entry)?;
    }
    if real == 0 {
        return Err(refused("is empty"));
    }
    Ok(contents)
}

/// Checks a packed state without writing anything.
pub(crate) fn check_archive(archive: &[u8]) -> AppResult<Contents> {
    if u64::try_from(archive.len()).unwrap_or(u64::MAX) > MAX_STATE_BYTES {
        return Err(too_large());
    }
    walk_archive(archive, |_, _, _, reader| {
        io::copy(reader, &mut io::sink())
            .map(|_| ())
            .map_err(|err| refused(format_args!("is not a readable gzip tar ({err})")))
    })
}

fn too_large() -> AppError {
    refused(format_args!(
        "is larger than {} MiB packed",
        MAX_STATE_BYTES >> 20
    ))
}

/// The folder `parts` below `root`, each opened with `O_NOFOLLOW` (created when missing and
/// `create`).
fn open_path(root: &OwnedFd, parts: &[&OsStr], create: bool) -> Result<OwnedFd, Errno> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
    let mut dir = openat(root, ".", flags, Mode::empty())?;
    for part in parts {
        dir = open_dir_at(&dir, part, create)?;
    }
    Ok(dir)
}

/// Unpacks a checked state into the empty folder `root`, keeping permission bits. Folders get
/// theirs last, deepest first, so a read-only folder was still written into.
fn unpack(archive: &[u8], root: &OwnedFd) -> AppResult<Contents> {
    let mut folders: Vec<(PathBuf, u32)> = Vec::new();
    let contents = walk_archive(archive, |rel, kind, mode, reader| {
        let parts: Vec<&OsStr> = rel.iter().collect();
        let shown = rel.display();
        let on_the_way = |err: Errno| match err {
            Errno::ENOTDIR | Errno::ELOOP | Errno::EEXIST => {
                refused(format_args!("holds {shown} inside a file"))
            }
            other => errno_error(other),
        };
        match kind {
            Kind::Folder => {
                open_path(root, &parts, true).map_err(on_the_way)?;
                folders.push((rel.to_path_buf(), mode));
            }
            Kind::File => {
                let Some((name, dirs)) = parts.split_last() else {
                    return Err(refused("names an empty path"));
                };
                let dir = open_path(root, dirs, true).map_err(on_the_way)?;
                let flags = OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC;
                let fd =
                    openat(&dir, *name, flags, Mode::from_bits_truncate(0o600)).map_err(|err| {
                        match err {
                            Errno::EEXIST => refused(format_args!("holds {shown} twice")),
                            other => errno_error(other),
                        }
                    })?;
                let mut file = std::fs::File::from(fd);
                io::copy(reader, &mut file).map_err(|err| io_error(&err))?;
                fchmod(&file, to_mode(mode)).map_err(errno_error)?;
            }
        }
        Ok(())
    })?;
    folders.sort_by_key(|(rel, _)| std::cmp::Reverse(rel.components().count()));
    for (rel, mode) in folders {
        let parts: Vec<&OsStr> = rel.iter().collect();
        let dir = open_path(root, &parts, false).map_err(errno_error)?;
        fchmod(&dir, to_mode(mode)).map_err(errno_error)?;
    }
    Ok(contents)
}

/// A folder packed back into a state.
pub(crate) struct Packed {
    pub(crate) archive: Vec<u8>,
    pub(crate) contents: Contents,
    /// Links and special files left out, as paths inside the folder.
    pub(crate) skipped: Vec<String>,
}

/// Packs the folder `root` within the state bounds: files and folders with their permission bits,
/// read through handles without following links; links and special files are left out and named.
pub(crate) fn pack(root: &OwnedFd) -> AppResult<Packed> {
    let gz = GzEncoder::new(Capped64::default(), Compression::fast());
    let mut packer = Packer {
        builder: tar::Builder::new(gz),
        entries: 0,
        contents: Contents::default(),
        skipped: Vec::new(),
    };
    packer.add(root, Path::new(""), 0)?;
    let Packer {
        builder,
        contents,
        skipped,
        entries,
    } = packer;
    if entries == 0 {
        return Err(AppError::Invalid(
            "the folder is empty: there is nothing to save".into(),
        ));
    }
    let archive = builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .map_err(|err| packing_error(&err))?
        .0;
    Ok(Packed {
        archive,
        contents,
        skipped,
    })
}

/// The packed bytes of a save, refused past [`MAX_STATE_BYTES`] as they are written (review of
/// SPEC-271, M3), so a large file never grows memory beyond the bound.
#[derive(Default)]
struct Capped64(Vec<u8>);

impl io::Write for Capped64 {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if u64::try_from(self.0.len() + buf.len()).unwrap_or(u64::MAX) > MAX_STATE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "the packed state is over its bound",
            ));
        }
        let bound = usize::try_from(MAX_STATE_BYTES).unwrap_or(usize::MAX);
        if self.0.capacity() < self.0.len() + buf.len() {
            // Doubling, but never past the bound (review of SPEC-271, n4).
            let want = (self.0.capacity() * 2).clamp(self.0.len() + buf.len(), bound);
            self.0.reserve_exact(want - self.0.len());
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn packing_error(err: &io::Error) -> AppError {
    if err.kind() == io::ErrorKind::FileTooLarge {
        too_large()
    } else {
        io_error(err)
    }
}

struct Packer {
    builder: tar::Builder<GzEncoder<Capped64>>,
    entries: u64,
    contents: Contents,
    skipped: Vec<String>,
}

impl Packer {
    fn add(&mut self, dir: &OwnedFd, rel: &Path, depth: usize) -> AppResult<()> {
        if depth >= MAX_STATE_DEPTH {
            return Err(refused(format_args!(
                "nests folders more than {MAX_STATE_DEPTH} deep"
            )));
        }
        let limit = usize::try_from(MAX_STATE_ENTRIES).unwrap_or(usize::MAX) + 1;
        let mut names = names_in(dir, limit).map_err(errno_error)?;
        names.sort();
        for name in names {
            let path = rel.join(&name);
            self.entries += 1;
            if self.entries > MAX_STATE_ENTRIES {
                return Err(refused(format_args!(
                    "holds more than {MAX_STATE_ENTRIES} files and folders"
                )));
            }
            let (kind, stat) = match kind_at(dir, &name) {
                Ok(found) => found,
                // Gone since the listing: nothing to keep.
                Err(Errno::ENOENT) => continue,
                Err(err) => return Err(errno_error(err)),
            };
            let mode = from_mode(Mode::from_bits_truncate(stat.st_mode));
            let mtime = u64::try_from(stat.st_mtime).unwrap_or(0);
            match kind {
                SFlag::S_IFDIR => {
                    let child = match open_dir_at(dir, &name, false) {
                        Ok(child) => child,
                        Err(Errno::ELOOP | Errno::ENOTDIR) => {
                            self.skipped.push(path.display().to_string());
                            continue;
                        }
                        Err(err) => return Err(errno_error(err)),
                    };
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_mode(mode);
                    header.set_mtime(mtime);
                    header.set_size(0);
                    self.builder
                        .append_data(&mut header, &path, io::empty())
                        .map_err(|err| packing_error(&err))?;
                    self.add(&child, &path, depth + 1)?;
                }
                SFlag::S_IFREG => self.add_file(dir, &name, &path, mode, mtime)?,
                _ => self.skipped.push(path.display().to_string()),
            }
        }
        Ok(())
    }

    fn add_file(
        &mut self,
        dir: &OwnedFd,
        name: &OsStr,
        path: &Path,
        mode: u32,
        mtime: u64,
    ) -> AppResult<()> {
        let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
        let fd = match openat(dir, name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ELOOP) => {
                self.skipped.push(path.display().to_string());
                return Ok(());
            }
            Err(Errno::ENOENT) => return Ok(()),
            Err(err) => return Err(errno_error(err)),
        };
        let file = std::fs::File::from(fd);
        let meta = file.metadata().map_err(|err| io_error(&err))?;
        if !meta.is_file() {
            self.skipped.push(path.display().to_string());
            return Ok(());
        }
        let size = meta.len();
        self.contents.files += 1;
        self.contents.bytes = self.contents.bytes.saturating_add(size);
        if self.contents.bytes > MAX_STATE_EXPANDED {
            return Err(refused(format_args!(
                "holds more than {} MiB",
                MAX_STATE_EXPANDED >> 20
            )));
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(mode);
        header.set_mtime(mtime);
        header.set_size(size);
        // Exactly `size` bytes even when a writer changes the file meanwhile: a shorter read is
        // padded, a longer one cut, so the archive stays readable.
        let data = file.take(size).chain(io::repeat(0)).take(size);
        self.builder
            .append_data(&mut header, path, data)
            .map_err(|err| packing_error(&err))
    }
}

/// Associated data of a state's archive: its row and version, so an archive never opens as
/// another state's or an older version's.
fn aad(id: &str, version: u32) -> Vec<u8> {
    format!("{id}|state|{version}").into_bytes()
}

fn view(record: &AgentStateRecord) -> AgentStateView {
    AgentStateView {
        name: record.name.clone(),
        description: record.description.clone(),
        version: record.version,
        bytes: record.bytes,
        files: record.files,
        updated_by: record.updated_by.clone(),
        updated_at: record.updated_at.0,
        restored_path: record.restored.as_ref().map(|r| r.path.clone()),
        restored_version: record.restored.as_ref().map(|r| r.version),
        restored_at: record.restored.as_ref().map(|r| r.at.0),
    }
}

fn no_state(name: &str) -> AppError {
    AppError::Invalid(format!(
        "no agent state named {name} in this project (secrets_list names them)"
    ))
}

/// An import whose caller, name and slot are checked, before its body is read (review of
/// SPEC-271, M2).
pub struct StateImport {
    sid: SandboxId,
    name: String,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// Removes the folder `~/parts` a restore placed, through handles.
fn remove_placed(anchor: &Path, parts: &[String]) -> AppResult<()> {
    let (name, above) = parts
        .split_last()
        .ok_or_else(|| AppError::Invalid("no folder".into()))?;
    let above: Vec<&OsStr> = std::iter::once(OsStr::new("home"))
        .chain(above.iter().map(OsStr::new))
        .collect();
    let parent = crate::files::open_dirs_below(anchor, &above, false).map_err(errno_error)?;
    remove_tree(&parent, OsStr::new(name))
}

/// The answer to a second import over a restored copy (SPEC-271 OPS-003).
fn writer_elsewhere(name: &str, restored: &StateRestore) -> AppError {
    AppError::Conflict(format!(
        "{name} was restored into the sandbox at {path} (version {version}), and that copy is the \
         one written to now: sending this one would fork it. Get the sandbox's copy with \
         `yulius secrets state-get {name}`, or pass --replace-state (replace=true) to overwrite \
         it with this one on purpose",
        path = restored.path,
        version = restored.version,
    ))
}

/// Arguments of `secret_state_restore`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreArgs {
    name: String,
    path: String,
}

/// Arguments of `secret_state_save`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    name: String,
}

/// What to do with the folder in the way of a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// Missing or an empty folder: the state goes there.
    Fresh,
    /// The stale copy of an earlier version: replaced once the new one is in place.
    Replace,
}

/// How a restore of `record` to `path` goes, given whether the recorded copy and the target
/// exist (SPEC-271 OPS-005).
fn placement(
    record: &AgentStateRecord,
    path: &str,
    recorded_exists: bool,
    target_empty: Option<bool>,
) -> AppResult<Placement> {
    let name = &record.name;
    if let Some(restored) = record.restored.as_ref().filter(|_| recorded_exists) {
        if restored.path != path {
            return Err(AppError::Conflict(format!(
                "{name} is restored at {} already, and that copy is the one written to: use it \
                 there (a state has one copy)",
                restored.path
            )));
        }
        if restored.version >= record.version {
            return Err(AppError::Conflict(format!(
                "{name} is restored at {path} already (version {}), and that copy is the one \
                 written to: use it, and save it with secret_state_save after each change",
                restored.version
            )));
        }
        return Ok(Placement::Replace);
    }
    match target_empty {
        None | Some(true) => Ok(Placement::Fresh),
        Some(false) => Err(AppError::Conflict(format!(
            "{path} exists and is not an empty folder: restore {name} into a new folder"
        ))),
    }
}

impl App {
    /// The state `name` of `sandbox` with its archive, `Invalid` when absent.
    async fn state_of(
        &self,
        sandbox: &SandboxId,
        name: &str,
    ) -> AppResult<(AgentStateRecord, Envelope)> {
        self.deps
            .store
            .agent_state(sandbox, name)
            .await?
            .ok_or_else(|| no_state(name))
    }

    /// The record of the state `name` of `sandbox`, without its archive.
    async fn record_of(&self, sandbox: &SandboxId, name: &str) -> AppResult<AgentStateRecord> {
        self.deps
            .store
            .agent_states(sandbox)
            .await?
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| no_state(name))
    }

    /// Seals `archive` for `record`'s row and version, off the async executor.
    async fn seal_state(&self, record: &AgentStateRecord, archive: Vec<u8>) -> AppResult<Envelope> {
        let crypto = std::sync::Arc::clone(&self.deps.crypto);
        let aad = aad(&record.id, record.version);
        tokio::task::spawn_blocking(move || crypto.envelope_seal(&aad, &archive))
            .await
            .map_err(AppError::internal)?
    }

    /// Opens a state's archive, off the async executor; wiped when dropped.
    async fn open_state(
        &self,
        record: &AgentStateRecord,
        sealed: Envelope,
    ) -> AppResult<zeroize::Zeroizing<Vec<u8>>> {
        let crypto = std::sync::Arc::clone(&self.deps.crypto);
        let aad = aad(&record.id, record.version);
        tokio::task::spawn_blocking(move || crypto.envelope_open(&aad, &sealed))
            .await
            .map_err(AppError::internal)?
    }

    /// `GET /api/sandboxes/{sid}/agent-states` (SPEC-271 OPS-007): the states, never content.
    ///
    /// # Errors
    /// `NotFound` for a sandbox the caller cannot see.
    pub async fn agent_states(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<AgentStateView>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let records = self.deps.store.agent_states(&sid).await?;
        Ok(records.iter().map(view).collect())
    }

    /// The checks of `PUT /api/sandboxes/{sid}/agent-states/{name}` that come before its body is
    /// read (review of SPEC-271, M2): the caller manages the sandbox's agent secrets, the name is
    /// an agent secret name, and one of the state slots is free, held until the import ends.
    ///
    /// # Errors
    /// `NotFound`, `Forbidden`, `Invalid` for the name, `Capacity` when busy.
    pub async fn begin_state_import(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
    ) -> AppResult<StateImport> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        self.require_manage_agent(ctx, &AgentScope::Project(sid.clone()))
            .await?;
        let name = SecretName::parse(name.trim()).map_err(invalid)?;
        Ok(StateImport {
            sid,
            name: name.as_str().to_owned(),
            _slot: self.state_slot()?,
        })
    }

    /// `PUT /api/sandboxes/{sid}/agent-states/{name}` (SPEC-271 OPS-002, OPS-003): stores a
    /// packed state, after [`App::begin_state_import`], once it is checked. Over a restored copy
    /// only with `replace`.
    ///
    /// # Errors
    /// `Invalid` for a bad archive or description, `Conflict` over a restored copy without
    /// `replace` or when another writer won.
    pub async fn import_agent_state(
        &self,
        ctx: &AuthContext,
        begun: StateImport,
        description: Option<&str>,
        replace: bool,
        archive: Vec<u8>,
    ) -> AppResult<AgentStateView> {
        let StateImport { sid, name, _slot } = begun;
        let description = description.map(str::trim).unwrap_or_default();
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(AppError::Invalid(format!(
                "a description has at most {MAX_DESCRIPTION_CHARS} characters"
            )));
        }
        let (archive, contents) = tokio::task::spawn_blocking(move || {
            let contents = check_archive(&archive)?;
            Ok::<_, AppError>((archive, contents))
        })
        .await
        .map_err(AppError::internal)??;

        let existing = self
            .deps
            .store
            .agent_states(&sid)
            .await?
            .into_iter()
            .find(|r| r.name == name);
        let now = self.now();
        let actor = Self::vault_actor(ctx);
        let bytes = u64::try_from(archive.len()).unwrap_or(u64::MAX);
        let record = match &existing {
            Some(old) => {
                if let Some(restored) = old.restored.as_ref().filter(|_| !replace) {
                    return Err(writer_elsewhere(&name, restored));
                }
                AgentStateRecord {
                    version: old.version + 1,
                    description: if description.is_empty() {
                        old.description.clone()
                    } else {
                        description.to_owned()
                    },
                    bytes,
                    files: contents.files,
                    updated_by: actor.clone(),
                    updated_at: now,
                    ..old.clone()
                }
            }
            None => AgentStateRecord {
                id: self.deps.crypto.new_id(),
                sandbox_id: sid.clone(),
                name: name.clone(),
                description: description.to_owned(),
                version: 1,
                bytes,
                files: contents.files,
                created_by: actor.clone(),
                created_at: now,
                updated_by: actor.clone(),
                updated_at: now,
                restored: None,
            },
        };
        let sealed = self.seal_state(&record, archive).await?;
        if !self
            .deps
            .store
            .agent_state_put(&record, sealed, existing.as_ref())
            .await?
        {
            return Err(AppError::Conflict(format!(
                "{name} changed while it was sent (a task may have restored it): look at it with \
                 `yulius secrets list`, and send it again only if it was not restored"
            )));
        }
        let action = if record.restored.is_some() {
            "vault.state.replace"
        } else {
            "vault.state.import"
        };
        self.vault_audit(
            &actor,
            action,
            Some(sid.as_str()),
            &format!(
                "{} version {}: {} files, {} bytes packed",
                record.name, record.version, record.files, record.bytes
            ),
        )
        .await?;
        Ok(view(&record))
    }

    /// `GET /api/sandboxes/{sid}/agent-states/{name}/archive` (SPEC-271 OPS-007): the newest
    /// packed state, for a person who manages the sandbox's agent secrets, never through a
    /// read-only token (review of SPEC-271, M4); audited.
    ///
    /// # Errors
    /// `NotFound`, `Forbidden`, `Capacity` when busy; a sealed archive that does not open.
    pub async fn export_agent_state(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
    ) -> AppResult<Vec<u8>> {
        if ctx.read_only() {
            return Err(AppError::Forbidden(
                "a read-only token cannot download an agent state: its content is operator \
                 state, kept like agent secret values"
                    .into(),
            ));
        }
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        self.require_manage_agent(ctx, &AgentScope::Project(sid.clone()))
            .await?;
        let _slot = self.state_slot()?;
        let (record, sealed) = self
            .deps
            .store
            .agent_state(&sid, name.trim())
            .await?
            .ok_or(AppError::NotFound)?;
        let mut archive = self.open_state(&record, sealed).await?;
        self.vault_audit(
            &Self::vault_actor(ctx),
            "vault.state.export",
            Some(sid.as_str()),
            &format!("{} version {}", record.name, record.version),
        )
        .await?;
        // Moved out, not copied: the response body is the only copy left.
        Ok(std::mem::take(&mut *archive))
    }

    /// Holds every state slot until the permits are dropped (tests of SPEC-271 OPS-008).
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn occupy_state_slots(&self) -> Vec<tokio::sync::OwnedSemaphorePermit> {
        std::iter::from_fn(|| {
            std::sync::Arc::clone(&self.uploads.states)
                .try_acquire_owned()
                .ok()
        })
        .collect()
    }

    /// One of the few state operations that may hold an archive in memory at once.
    fn state_slot(&self) -> AppResult<tokio::sync::OwnedSemaphorePermit> {
        std::sync::Arc::clone(&self.uploads.states)
            .try_acquire_owned()
            .map_err(|_| {
                AppError::Capacity("too many agent states are moving at once; try again".into())
            })
    }

    /// The sandbox home's folders of a `~/…` path, refused otherwise.
    fn home_path(path: &str) -> AppResult<Vec<String>> {
        let (root, parts) = source(path)?;
        if root != Root::Home || parts.is_empty() {
            return Err(AppError::Invalid(
                "restore a state into a folder of your home, as ~/path (for example \
                 ~/.quartio-operator): the home is kept across tasks, a worktree is not"
                    .into(),
            ));
        }
        Ok(parts)
    }

    /// The canonical sandbox folder, the anchor no agent can replace.
    fn state_anchor(&self, sandbox: &SandboxId) -> AppResult<PathBuf> {
        std::fs::canonicalize(self.sandbox_dir(sandbox)).map_err(|_| {
            AppError::Invalid("the sandbox folder does not exist on this machine".into())
        })
    }

    /// `secret_state_restore` (SPEC-271 OPS-005): writes a state into a folder of the sandbox
    /// home without its content reaching the agent.
    pub(crate) async fn tool_secret_state_restore(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let args: RestoreArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let parts = Self::home_path(&args.path)?;
        let path = format!("~/{}", parts.join("/"));
        let sandbox = ctx.task.sandbox_id.clone();
        let _slot = self.state_slot()?;
        let (record, sealed) = self.state_of(&sandbox, args.name.trim()).await?;
        let archive = self.open_state(&record, sealed).await?;
        let anchor = self.state_anchor(&sandbox)?;
        let recorded = record
            .restored
            .as_ref()
            .map(|r| Self::home_path(&r.path))
            .transpose()?;
        let temporary = format!(".yulius-state-{}", self.deps.crypto.new_id());
        let job = {
            let (record, path, anchor, parts) =
                (record.clone(), path.clone(), anchor.clone(), parts.clone());
            move || {
                restore_blocking(
                    &anchor,
                    &parts,
                    recorded.as_deref(),
                    &record,
                    &path,
                    &archive,
                    &temporary,
                )
            }
        };
        let (contents, how) = tokio::task::spawn_blocking(job)
            .await
            .map_err(AppError::internal)??;

        let restore = StateRestore {
            path: path.clone(),
            version: record.version,
            by: ctx.actor(),
            at: self.now(),
        };
        if !self
            .deps
            .store
            .agent_state_restored(&record, &restore)
            .await?
        {
            // Another restore or a send won meanwhile (review of SPEC-271, M1): a copy placed
            // fresh here is nobody's writer, so it goes.
            if how == Placement::Fresh {
                let target = path.clone();
                let removed = tokio::task::spawn_blocking(move || remove_placed(&anchor, &parts))
                    .await
                    .map_err(AppError::internal)?;
                if let Err(err) = removed {
                    tracing::warn!(
                        path = target,
                        error = %err.public_message(),
                        "an agent state copy that lost its restore was not removed"
                    );
                }
            }
            return Err(AppError::Conflict(format!(
                "{} changed while it was restored (another task restored it, or the person sent \
                 a newer one): read secrets_list and use or restore what it names",
                record.name
            )));
        }
        self.vault_audit(
            &ctx.actor(),
            "vault.state.restore",
            Some(sandbox.as_str()),
            &format!("{} version {} into {path}", record.name, record.version),
        )
        .await?;
        Ok(json!({
            "restored": record.name,
            "path": path,
            "version": record.version,
            "files": contents.files,
            "bytes": contents.bytes,
            "next": format!(
                "{path} now holds {name}, and this copy is the one written to: run the project's \
                 tools against it there (it stays for later tasks), never print or copy its files \
                 into the conversation, and call secret_state_save {{\"name\": \"{name}\"}} after \
                 each change (a rollout) so the vault keeps a backup the person can download. \
                 Keys stay agent secrets: put the ones its configs name into files with \
                 secret_copy_to_env.",
                name = record.name
            ),
        }))
    }

    /// `secret_state_save` (SPEC-271 OPS-006): stores the restored copy as the next version.
    pub(crate) async fn tool_secret_state_save(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let args: SaveArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let sandbox = ctx.task.sandbox_id.clone();
        let record = self.record_of(&sandbox, args.name.trim()).await?;
        let name = record.name.clone();
        let Some(restored) = record.restored.clone() else {
            return Err(AppError::Invalid(format!(
                "{name} was never restored: restore it with secret_state_restore first"
            )));
        };
        if restored.version < record.version {
            return Err(AppError::Conflict(format!(
                "the person sent a newer {name} (version {}) to replace the copy at {}: restore it \
                 with secret_state_restore at that path first, which replaces the folder",
                record.version, restored.path
            )));
        }
        let parts = Self::home_path(&restored.path)?;
        let anchor = self.state_anchor(&sandbox)?;
        let _slot = self.state_slot()?;
        let shown = restored.path.clone();
        let packed = tokio::task::spawn_blocking(move || {
            let mut below: Vec<&OsStr> = vec![OsStr::new("home")];
            below.extend(parts.iter().map(OsStr::new));
            let folder =
                crate::files::open_dirs_below(&anchor, &below, false).map_err(|err| match err {
                    Errno::ENOENT => AppError::Invalid(format!(
                        "{shown} is gone: restore the state again with secret_state_restore"
                    )),
                    Errno::ELOOP | Errno::ENOTDIR => AppError::Forbidden(format!(
                        "a folder on the way to {shown} is a link or not a folder: Yulius \
                         follows no links"
                    )),
                    other => errno_error(other),
                })?;
            pack(&folder)
        })
        .await
        .map_err(AppError::internal)??;

        let now = self.now();
        let next = AgentStateRecord {
            version: record.version + 1,
            bytes: u64::try_from(packed.archive.len()).unwrap_or(u64::MAX),
            files: packed.contents.files,
            updated_by: ctx.actor(),
            updated_at: now,
            restored: Some(StateRestore {
                version: record.version + 1,
                by: ctx.actor(),
                at: now,
                ..restored
            }),
            ..record.clone()
        };
        let Packed {
            archive, skipped, ..
        } = packed;
        let sealed = self.seal_state(&next, archive).await?;
        if !self
            .deps
            .store
            .agent_state_put(&next, sealed, Some(&record))
            .await?
        {
            return Err(AppError::Conflict(format!(
                "{name} changed while it was saved (the person replaced it, or another task \
                 saved it): read secrets_list and try again"
            )));
        }
        self.vault_audit(
            &ctx.actor(),
            "vault.state.save",
            Some(sandbox.as_str()),
            &format!(
                "{name} version {}: {} files, {} bytes packed",
                next.version, next.files, next.bytes
            ),
        )
        .await?;
        let mut answer = json!({
            "saved": name,
            "version": next.version,
            "files": next.files,
            "bytes": next.bytes,
        });
        if !skipped.is_empty() {
            answer["skipped"] = json!(skipped);
            answer["note"] = Value::String(
                "links and special files are not kept in a state; the files named in skipped \
                 were left out"
                    .into(),
            );
        }
        Ok(answer)
    }

    /// States of a sandbox for `secrets_list` (SPEC-271 OPS-007).
    pub(crate) async fn states_for_list(&self, sandbox: &SandboxId) -> AppResult<Vec<Value>> {
        let records = self.deps.store.agent_states(sandbox).await?;
        Ok(records
            .iter()
            .map(|r| {
                let restored = r.restored.as_ref();
                json!({
                    "name": r.name,
                    "description": r.description,
                    "version": r.version,
                    "files": r.files,
                    "bytes": r.bytes,
                    "restored_path": restored.map(|s| s.path.clone()),
                    "restored_version": restored.map(|s| s.version),
                })
            })
            .collect())
    }
}

/// The restore's file work (SPEC-271 OPS-005), off the async executor: opens the target's
/// parent from `anchor/home` without links (created when missing), decides with
/// [`placement`], unpacks into the fresh sibling `temporary` and renames it into place; a replaced
/// copy is removed after. Nothing is left of `temporary` when anything fails.
fn restore_blocking(
    anchor: &Path,
    parts: &[String],
    recorded: Option<&[String]>,
    record: &AgentStateRecord,
    path: &str,
    archive: &[u8],
    temporary: &str,
) -> AppResult<(Contents, Placement)> {
    let on_the_way = |err: Errno| match err {
        Errno::ELOOP | Errno::ENOTDIR => AppError::Forbidden(format!(
            "a folder on the way to {path} is a link or not a folder: Yulius follows no links"
        )),
        other => errno_error(other),
    };
    let home_and = |folders: &[String]| -> Vec<std::ffi::OsString> {
        std::iter::once("home".into())
            .chain(folders.iter().map(Into::into))
            .collect()
    };
    let exists = |folders: &[String]| -> AppResult<bool> {
        let Some((name, above)) = folders.split_last() else {
            return Ok(false);
        };
        let above = home_and(above);
        let above: Vec<&OsStr> = above.iter().map(std::ffi::OsString::as_os_str).collect();
        match crate::files::open_dirs_below(anchor, &above, false) {
            Ok(dir) => match kind_at(&dir, OsStr::new(name)) {
                Ok(_) => Ok(true),
                Err(Errno::ENOENT) => Ok(false),
                Err(err) => Err(errno_error(err)),
            },
            Err(Errno::ENOENT) => Ok(false),
            Err(err) => Err(on_the_way(err)),
        }
    };
    let recorded_exists = match recorded {
        Some(folders) => exists(folders)?,
        None => false,
    };
    let (name, above) = parts
        .split_last()
        .ok_or_else(|| AppError::Invalid("name the folder to restore into".into()))?;
    let above = home_and(above);
    let above: Vec<&OsStr> = above.iter().map(std::ffi::OsString::as_os_str).collect();
    let parent = crate::files::open_dirs_below(anchor, &above, true).map_err(on_the_way)?;
    let target = OsStr::new(name.as_str());
    let target_empty = match kind_at(&parent, target) {
        Err(Errno::ENOENT) => None,
        Ok((SFlag::S_IFDIR, _)) => {
            let dir = open_dir_at(&parent, target, false).map_err(on_the_way)?;
            Some(names_in(&dir, 1).map_err(errno_error)?.is_empty())
        }
        Ok(_) => Some(false),
        Err(err) => return Err(errno_error(err)),
    };
    let how = placement(record, path, recorded_exists, target_empty)?;

    mkdirat(&parent, temporary, Mode::from_bits_truncate(0o700)).map_err(errno_error)?;
    let placed = (|| {
        let fresh = open_dir_at(&parent, OsStr::new(temporary), false).map_err(errno_error)?;
        let contents = unpack(archive, &fresh)?;
        match how {
            Placement::Fresh => {
                // Over a missing name or an empty folder; anything else in the way meanwhile
                // fails the rename.
                renameat(&parent, temporary, &parent, target).map_err(|err| match err {
                    Errno::ENOTEMPTY | Errno::EEXIST | Errno::ENOTDIR | Errno::EISDIR => {
                        AppError::Conflict(format!(
                            "{path} changed while the state was restored: restore it into a new \
                             folder"
                        ))
                    }
                    other => errno_error(other),
                })?;
            }
            Placement::Replace => {
                let old = format!("{temporary}-old");
                renameat(&parent, target, &parent, old.as_str()).map_err(errno_error)?;
                if let Err(err) = renameat(&parent, temporary, &parent, target) {
                    // Put the old copy back rather than leave nothing at the path.
                    let _ = renameat(&parent, old.as_str(), &parent, target);
                    return Err(errno_error(err));
                }
                // The new copy is in place: a leftover old one is logged, not a failure
                // (review of SPEC-271, m1).
                if let Err(err) = remove_tree(&parent, OsStr::new(&old)) {
                    tracing::warn!(
                        path,
                        error = %err.public_message(),
                        "the replaced agent state copy was not removed"
                    );
                }
            }
        }
        Ok((contents, how))
    })();
    if placed.is_err() {
        let _ = remove_tree(&parent, OsStr::new(temporary));
    }
    placed
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use yulius_domain::time::Timestamp;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A gzip tar of `(path, kind, mode, content)` entries; kind `l` is a link to `content`.
    fn archive(entries: &[(&str, char, u32, &str)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, kind, mode, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(*mode);
            match kind {
                'd' => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    builder.append_data(&mut header, path, io::empty())?;
                }
                'l' => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    builder.append_link(&mut header, path, content)?;
                }
                'c' => {
                    header.set_entry_type(tar::EntryType::Char);
                    header.set_size(0);
                    builder.append_data(&mut header, path, io::empty())?;
                }
                _ => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_size(content.len() as u64);
                    builder.append_data(&mut header, path, content.as_bytes())?;
                }
            }
        }
        Ok(builder.into_inner()?.finish()?)
    }

    /// A raw tar entry whose name the `tar` builder would refuse (`..`, absolute).
    fn raw_archive(name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut header = tar::Header::new_old();
        header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_mode(0o644);
        header.set_size(1);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        let mut tar = header.as_bytes().to_vec();
        tar.extend_from_slice(b"x");
        tar.resize(512 * 2, 0);
        tar.resize(512 * 4, 0);
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&tar)?;
        Ok(gz.finish()?)
    }

    /// One raw tar entry: a header of `kind` declaring `size`, then `data` padded to a block.
    fn raw_entry(kind: tar::EntryType, name: &str, size: u64, data: &[u8]) -> Vec<u8> {
        let mut header = tar::Header::new_ustar();
        if let Some(ustar) = header.as_ustar_mut() {
            ustar.name[..name.len()].copy_from_slice(name.as_bytes());
        }
        header.set_mode(0o644);
        header.set_size(size);
        header.set_entry_type(kind);
        header.set_cksum();
        let mut out = header.as_bytes().to_vec();
        out.extend_from_slice(data);
        let padded = out.len().div_ceil(512) * 512;
        out.resize(padded, 0);
        out
    }

    fn gzip(tar: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(tar)?;
        gz.write_all(&[0; 1024])?;
        Ok(gz.finish()?)
    }

    /// Review of SPEC-271, B1: what bounds a state is what the reader reads, not a header's own
    /// size field. A pax `size` overriding the header, data hidden under a folder entry and an
    /// extension header far larger than any name are refused before they are read whole.
    #[test]
    fn archives_are_bounded_by_what_is_read() -> TestResult {
        // A pax record saying 600 MB, over a header saying 0.
        let record = "18 size=600000000\n";
        let mut tar = raw_entry(
            tar::EntryType::XHeader,
            "pax",
            record.len() as u64,
            record.as_bytes(),
        );
        tar.extend(raw_entry(tar::EntryType::Regular, "big", 0, &[]));
        let err = check_archive(&gzip(&tar)?).err().ok_or("pax size passed")?;
        assert!(format!("{err:?}").contains("512 MiB"), "{err:?}");

        // 600 MB of data under a folder entry.
        let tar = raw_entry(tar::EntryType::Directory, "dir/", 600 << 20, &[]);
        let err = check_archive(&gzip(&tar)?)
            .err()
            .ok_or("folder data passed")?;
        assert!(format!("{err:?}").contains("512 MiB"), "{err:?}");

        // A 64 MiB long name, which the tar reader would load whole.
        let name = vec![b'a'; 64 << 20];
        let tar = raw_entry(
            tar::EntryType::GNULongName,
            "././@LongLink",
            name.len() as u64,
            &name,
        );
        let err = check_archive(&gzip(&tar)?)
            .err()
            .ok_or("long name passed")?;
        assert!(format!("{err:?}").contains("header larger than"), "{err:?}");

        // Review round 2: unused header allowance never adds up. 100 empty global headers, then
        // a 1 MiB long name: refused as a header, not read whole on the slack.
        let mut tar = Vec::new();
        for _ in 0..100 {
            tar.extend(raw_entry(tar::EntryType::XGlobalHeader, "g", 0, &[]));
        }
        let name = vec![b'a'; 1 << 20];
        tar.extend(raw_entry(
            tar::EntryType::GNULongName,
            "././@LongLink",
            name.len() as u64,
            &name,
        ));
        let err = check_archive(&gzip(&tar)?)
            .err()
            .ok_or("long name on slack passed")?;
        assert!(format!("{err:?}").contains("header larger than"), "{err:?}");

        // Every entry counts toward the bound, global headers and `./` too.
        let mut tar = Vec::new();
        for _ in 0..=MAX_STATE_ENTRIES {
            tar.extend(raw_entry(tar::EntryType::Directory, "./", 0, &[]));
        }
        let err = check_archive(&gzip(&tar)?)
            .err()
            .ok_or("endless entries passed")?;
        assert!(format!("{err:?}").contains("more than 20000"), "{err:?}");

        // Review m2: `tar -C dir -czf x.tgz .` names the folder itself `./`.
        let mut tar = raw_entry(tar::EntryType::Directory, "./", 0, &[]);
        tar.extend(raw_entry(tar::EntryType::Regular, "./a", 1, b"x"));
        assert_eq!(check_archive(&gzip(&tar)?)?.files, 1);
        Ok(())
    }

    fn dir_fd(path: &Path) -> Result<OwnedFd, Box<dyn std::error::Error>> {
        Ok(nix::fcntl::open(
            path,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?)
    }

    fn mode_of(path: &Path) -> Result<u32, Box<dyn std::error::Error>> {
        Ok(std::fs::symlink_metadata(path)?.permissions().mode() & 0o7777)
    }

    fn record(version: u32, restored: Option<(&str, u32)>) -> AgentStateRecord {
        AgentStateRecord {
            id: "st".into(),
            sandbox_id: SandboxId::parse("s1").unwrap_or_else(|_| unreachable!()),
            name: "OPS".into(),
            description: String::new(),
            version,
            bytes: 1,
            files: 1,
            created_by: "ann".into(),
            created_at: Timestamp(1),
            updated_by: "ann".into(),
            updated_at: Timestamp(1),
            restored: restored.map(|(path, version)| StateRestore {
                path: path.into(),
                version,
                by: "task:1".into(),
                at: Timestamp(2),
            }),
        }
    }

    /// SPEC-271 OPS-002 (AC5): only files and folders inside the state's folder are taken.
    #[test]
    fn archives_hold_only_files_and_folders_inside() -> TestResult {
        let good = archive(&[
            ("inventory.sqlite", 'f', 0o600, "db"),
            ("rollouts", 'd', 0o755, ""),
            ("rollouts/a/bundle.json", 'f', 0o644, "{}"),
        ])?;
        assert_eq!(check_archive(&good)?, Contents { files: 2, bytes: 4 });
        let cases: [(Vec<u8>, &str); 6] = [
            (
                archive(&[("link", 'l', 0o777, "/etc/passwd")])?,
                "holds a link at link",
            ),
            (
                archive(&[("dev", 'c', 0o600, "")])?,
                "neither a file nor a folder",
            ),
            (raw_archive("../escape")?, "outside its folder"),
            (raw_archive("/etc/cron.d/x")?, "outside its folder"),
            (b"not gzip".to_vec(), "is not a readable gzip tar"),
            (archive(&[])?, "is empty"),
        ];
        for (bytes, why) in cases {
            let err = check_archive(&bytes).err().ok_or(why)?;
            assert!(
                matches!(&err, AppError::Invalid(m) if m.contains(why)),
                "{why}: {err:?}"
            );
        }
        let big = vec![0_u8; usize::try_from(MAX_STATE_BYTES)? + 1];
        assert!(check_archive(&big).is_err());
        Ok(())
    }

    /// SPEC-271 OPS-005, OPS-006: a state unpacks with its permission bits (setuid and sticky
    /// dropped) and packs back to the same files, leaving links out and naming them.
    #[test]
    fn unpacking_keeps_modes_and_packing_leaves_links_out() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("state");
        std::fs::create_dir(&root)?;
        let bytes = archive(&[
            (
                "config.json",
                'f',
                0o644,
                "{\"key_file\": \"/Users/x/.ssh/k\"}",
            ),
            ("bin", 'd', 0o1755, ""),
            ("bin/rollout", 'f', 0o4755, "#!/bin/sh\n"),
            ("locks/rollout.lock", 'f', 0o600, ""),
        ])?;
        let contents = unpack(&bytes, &dir_fd(&root)?)?;
        assert_eq!(contents.files, 3);
        assert_eq!(mode_of(&root.join("config.json"))?, 0o644);
        assert_eq!(mode_of(&root.join("bin"))?, 0o755);
        assert_eq!(mode_of(&root.join("bin/rollout"))?, 0o755);
        assert_eq!(mode_of(&root.join("locks/rollout.lock"))?, 0o600);
        assert_eq!(
            std::fs::read_to_string(root.join("bin/rollout"))?,
            "#!/bin/sh\n"
        );
        // The same path twice is refused, never overwritten.
        let twice = archive(&[("a", 'f', 0o600, "1"), ("a", 'f', 0o600, "2")])?;
        let other = dir.path().join("other");
        std::fs::create_dir(&other)?;
        assert!(unpack(&twice, &dir_fd(&other)?).is_err());

        std::fs::write(root.join("bin/rollout"), "#!/bin/sh\necho changed\n")?;
        std::os::unix::fs::symlink("/etc/passwd", root.join("passwd"))?;
        let packed = pack(&dir_fd(&root)?)?;
        assert_eq!(packed.skipped, vec!["passwd".to_owned()]);
        assert_eq!(packed.contents.files, 3);
        let again = dir.path().join("again");
        std::fs::create_dir(&again)?;
        unpack(&packed.archive, &dir_fd(&again)?)?;
        assert_eq!(
            std::fs::read_to_string(again.join("bin/rollout"))?,
            "#!/bin/sh\necho changed\n"
        );
        assert_eq!(mode_of(&again.join("locks/rollout.lock"))?, 0o600);
        assert!(!again.join("passwd").exists());
        Ok(())
    }

    /// Review of SPEC-271, M3: a save whose packed state passes 64 MiB is refused as it is
    /// written, never held whole.
    #[test]
    fn a_save_over_the_bound_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut noise = Vec::with_capacity(70 << 20);
        while noise.len() < 70 << 20 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            noise.extend_from_slice(&x.to_le_bytes());
        }
        std::fs::write(dir.path().join("big.bin"), &noise)?;
        let err = pack(&dir_fd(dir.path())?).err().ok_or("packed")?;
        assert!(format!("{err:?}").contains("larger than 64 MiB"), "{err:?}");
        Ok(())
    }

    /// SPEC-271 OPS-005 (AC2, AC3): one copy is the writer; only a newer version replaces it.
    #[test]
    fn a_restored_copy_is_replaced_only_by_a_newer_version() {
        let fresh = record(1, None);
        assert_eq!(
            placement(&fresh, "~/.ops", false, None).ok(),
            Some(Placement::Fresh)
        );
        assert_eq!(
            placement(&fresh, "~/.ops", false, Some(true)).ok(),
            Some(Placement::Fresh)
        );
        assert!(placement(&fresh, "~/.ops", false, Some(false)).is_err());

        let current = record(2, Some(("~/.ops", 2)));
        assert!(matches!(
            placement(&current, "~/.ops", true, Some(false)),
            Err(AppError::Conflict(m)) if m.contains("use it")
        ));
        assert!(matches!(
            placement(&current, "~/elsewhere", true, None),
            Err(AppError::Conflict(m)) if m.contains("restored at ~/.ops already")
        ));
        // The recorded copy is gone: restored again, anywhere empty.
        assert_eq!(
            placement(&current, "~/elsewhere", false, None).ok(),
            Some(Placement::Fresh)
        );

        let replaced = record(3, Some(("~/.ops", 2)));
        assert_eq!(
            placement(&replaced, "~/.ops", true, Some(false)).ok(),
            Some(Placement::Replace)
        );
    }

    /// SPEC-271 OPS-005 (AC6): the restore never follows a link the agent planted on the way or
    /// in place of the target, and leaves no temporary folder behind.
    #[test]
    fn restores_follow_no_links() -> TestResult {
        let dir = tempfile::tempdir()?;
        let anchor = std::fs::canonicalize(dir.path())?;
        let outside = anchor.join("outside");
        std::fs::create_dir_all(anchor.join("home"))?;
        std::fs::create_dir(&outside)?;
        std::os::unix::fs::symlink(&outside, anchor.join("home/linked"))?;
        let bytes = archive(&[("inventory.sqlite", 'f', 0o600, "db")])?;
        let state = record(1, None);
        let parts = |p: &[&str]| p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();

        let err = restore_blocking(
            &anchor,
            &parts(&["linked", "ops"]),
            None,
            &state,
            "~/linked/ops",
            &bytes,
            ".t1",
        )
        .err()
        .ok_or("followed a link on the way")?;
        assert!(matches!(err, AppError::Forbidden(_)), "{err:?}");
        let err = restore_blocking(
            &anchor,
            &parts(&["linked"]),
            None,
            &state,
            "~/linked",
            &bytes,
            ".t2",
        )
        .err()
        .ok_or("restored over a link")?;
        assert!(matches!(err, AppError::Conflict(_)), "{err:?}");
        assert_eq!(std::fs::read_dir(&outside)?.count(), 0);

        let (contents, _) = restore_blocking(
            &anchor,
            &parts(&[".local", "share", "ops"]),
            None,
            &state,
            "~/.local/share/ops",
            &bytes,
            ".t3",
        )?;
        assert_eq!(contents.files, 1);
        assert_eq!(
            std::fs::read_to_string(anchor.join("home/.local/share/ops/inventory.sqlite"))?,
            "db"
        );
        // A newer version replaces the restored copy in place.
        let newer = archive(&[("inventory.sqlite", 'f', 0o600, "db2")])?;
        restore_blocking(
            &anchor,
            &parts(&[".local", "share", "ops"]),
            Some(&parts(&[".local", "share", "ops"])),
            &record(2, Some(("~/.local/share/ops", 1))),
            "~/.local/share/ops",
            &newer,
            ".t4",
        )?;
        assert_eq!(
            std::fs::read_to_string(anchor.join("home/.local/share/ops/inventory.sqlite"))?,
            "db2"
        );
        let left: Vec<_> = std::fs::read_dir(anchor.join("home/.local/share"))?
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("ops")]);
        Ok(())
    }
}
