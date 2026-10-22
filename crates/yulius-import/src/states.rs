//! Agent states from a laptop (SPEC-271, ADR-0200): `yulius secrets import --state NAME=DIR`
//! packs a folder of operator state (a deploy tool's inventory, rollout folders, configs) into a
//! gzip tar and sends it to a project's vault, where a task restores it into the sandbox home;
//! `yulius secrets state-get` downloads the sandbox's newest saved copy.
//!
//! What is said names states, sources, file counts and sizes only, never content. Once a task
//! restores a state, the sandbox's copy is the one written to: the server refuses a second send
//! over it unless the person replaces it on purpose (`--replace-state`).

use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag};
use nix::sys::stat::{Mode, SFlag};
use serde::Deserialize;
use yulius_domain::vault::names::SecretName;

use crate::http::{ApiError, CHUNK_TIMEOUT, JSON_TIMEOUT};
use crate::secrets_import::{PreparedTarget, SecretsImportError, SecretsIo, VaultClient};
use crate::ui::human_bytes;

/// Largest packed state the server takes.
pub const MAX_STATE_BYTES: u64 = 64 << 20;
/// Most files and folders in a state.
const MAX_STATE_ENTRIES: u64 = 20_000;

/// A folder of operator state ready to send.
pub struct StateEntry {
    /// Its name in the vault (an agent secret name).
    pub name: String,
    /// Where it came from, as given.
    pub source: String,
    /// The packed state.
    pub archive: Vec<u8>,
    /// Files in it.
    pub files: u64,
}

/// How a run's states ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatesSummary {
    /// Stored (with the version they got).
    pub sent: Vec<(String, u32)>,
    /// Refused by the server (a restored copy without `--replace-state`, a bad archive, …).
    pub refused: Vec<String>,
}

#[derive(Deserialize)]
struct StateView {
    name: String,
    version: u32,
    #[serde(default)]
    restored_path: Option<String>,
}

/// The state `name` from `path`: a folder, packed here, or an existing `.tar.gz`/`.tgz`.
///
/// # Errors
/// What makes it unsendable: missing, a link or special file in the folder, too large, not a
/// gzip tar. Messages name paths, never content.
pub fn state_entry(name: &str, path: &Path) -> Result<StateEntry, String> {
    let name = state_name(name)?;
    let shown = path.display().to_string();
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let (archive, files) = if meta.is_dir() {
        pack_state_dir(path)?
    } else {
        let lower = shown.to_ascii_lowercase();
        if !(lower.ends_with(".tar.gz") || lower.ends_with(".tgz")) {
            return Err(format!(
                "{shown} is neither a folder nor a .tar.gz archive of one"
            ));
        }
        let mut archive = Vec::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(MAX_STATE_BYTES + 1).read_to_end(&mut archive))
            .map_err(|e| format!("cannot read {shown}: {e}"))?;
        if archive.len() as u64 > MAX_STATE_BYTES {
            return Err(too_large(&shown));
        }
        let files = count_files(&archive).map_err(|e| format!("{shown}: {e}"))?;
        (archive, files)
    };
    Ok(StateEntry {
        name,
        source: shown,
        archive,
        files,
    })
}

/// `name` as an agent secret name (`A-Z`, `0-9`, `_`), the only names a state takes: nothing
/// else ever reaches a request path.
fn state_name(name: &str) -> Result<String, String> {
    SecretName::parse(name.trim())
        .map(|n| n.as_str().to_owned())
        .map_err(|e| format!("{name:?} is not a state name: {e}"))
}

fn too_large(shown: &str) -> String {
    format!(
        "{shown} is larger than {} MiB packed, more than a state holds",
        MAX_STATE_BYTES >> 20
    )
}

/// The regular files of a packed state; a link or special file refuses it, as the server would.
fn count_files(archive: &[u8]) -> Result<u64, String> {
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    let mut files = 0;
    let entries = tar
        .entries()
        .map_err(|e| format!("not a readable gzip tar ({e})"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("not a readable gzip tar ({e})"))?;
        match entry.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => files += 1,
            tar::EntryType::Directory | tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => {
            }
            _ => {
                return Err(format!(
                    "holds {}, which is neither a file nor a folder: only files and folders move",
                    String::from_utf8_lossy(&entry.path_bytes())
                ));
            }
        }
    }
    Ok(files)
}

/// Packs the folder `dir` (its content, not the folder itself) with each file's and folder's
/// permission bits. A link or special file refuses it, so nothing is silently left behind. The
/// folder is walked through handles: each entry is opened from its folder's handle without
/// following a link, so one swapped for a link after the listing is refused, never read.
///
/// # Errors
/// Unreadable, holding a link or special file, more than 20 000 entries, over 64 MiB packed.
pub fn pack_state_dir(dir: &Path) -> Result<(Vec<u8>, u64), String> {
    let shown = dir.display().to_string();
    let root = open_folder(dir).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut counts = (0_u64, 0_u64);
    add_folder(&mut builder, &root, Path::new(""), &shown, &mut counts)?;
    if counts.0 == 0 {
        return Err(format!("{shown} is empty: there is nothing to send"));
    }
    let archive = builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .map_err(|e| format!("cannot pack {shown}: {e}"))?;
    if archive.len() as u64 > MAX_STATE_BYTES {
        return Err(too_large(&shown));
    }
    Ok((archive, counts.1))
}

/// The folder the person named (a link to it is theirs to follow).
fn open_folder(dir: &Path) -> Result<OwnedFd, String> {
    nix::fcntl::open(
        dir,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| e.desc().to_owned())
}

fn link_refused(err: Errno) -> String {
    match err {
        // `O_DIRECTORY | O_NOFOLLOW` on a link says ENOTDIR on Linux.
        Errno::ELOOP | Errno::ENOTDIR => "a link".to_owned(),
        other => other.desc().to_owned(),
    }
}

/// The folder `name` in `dir`, never through a link.
fn open_dir_at(dir: &OwnedFd, name: &OsStr) -> Result<OwnedFd, String> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    nix::fcntl::openat(dir, name, flags, Mode::empty()).map_err(link_refused)
}

/// The regular file `name` in `dir`, never through a link and without blocking on a FIFO.
fn open_file_at(dir: &OwnedFd, name: &OsStr) -> Result<std::fs::File, String> {
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = std::fs::File::from(
        nix::fcntl::openat(dir, name, flags, Mode::empty()).map_err(link_refused)?,
    );
    let regular = file.metadata().map_err(|e| e.to_string())?.is_file();
    if regular {
        Ok(file)
    } else {
        Err("a special file".to_owned())
    }
}

/// The names in the folder `dir`, sorted.
fn names_in(dir: &OwnedFd) -> Result<Vec<OsString>, String> {
    let listing = nix::dir::Dir::openat(
        dir,
        ".",
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| e.desc().to_owned())?;
    let mut names = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|e| e.desc().to_owned())?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(OsStr::from_bytes(name).to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// A header for an entry with the permission bits and time of `meta`, read from the opened handle.
fn header_of(meta: &std::fs::Metadata, kind: tar::EntryType, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(meta.permissions().mode() & 0o777);
    header.set_mtime(
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs()),
    );
    header.set_size(size);
    header
}

/// Adds the entries of the folder `dir` (at `rel` in the state); `counts` is (entries, files).
fn add_folder(
    builder: &mut tar::Builder<GzEncoder<Vec<u8>>>,
    dir: &OwnedFd,
    rel: &Path,
    shown: &str,
    counts: &mut (u64, u64),
) -> Result<(), String> {
    let unreadable = |e: io::Error| format!("cannot read {shown}: {e}");
    let refused = |inside: &Path, what: &str| {
        format!(
            "{shown} holds {}, {what}: only files and folders move; replace it with what it \
             points to, or move it out of the folder",
            inside.display()
        )
    };
    for name in names_in(dir).map_err(|e| format!("cannot read {shown}: {e}"))? {
        let inside: PathBuf = rel.join(&name);
        counts.0 += 1;
        if counts.0 > MAX_STATE_ENTRIES {
            return Err(format!(
                "{shown} holds more than {MAX_STATE_ENTRIES} files and folders"
            ));
        }
        let stat = nix::sys::stat::fstatat(dir, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
            .map_err(|e| format!("cannot read {shown}: {}", e.desc()))?;
        let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
        if kind == SFlag::S_IFDIR {
            let child = open_dir_at(dir, &name).map_err(|what| refused(&inside, &what))?;
            let meta = std::fs::File::from(child.try_clone().map_err(unreadable)?)
                .metadata()
                .map_err(unreadable)?;
            let mut header = header_of(&meta, tar::EntryType::Directory, 0);
            builder
                .append_data(&mut header, &inside, io::empty())
                .map_err(unreadable)?;
            add_folder(builder, &child, &inside, shown, counts)?;
        } else if kind == SFlag::S_IFREG {
            let file = open_file_at(dir, &name).map_err(|what| refused(&inside, &what))?;
            let meta = file.metadata().map_err(unreadable)?;
            let size = meta.len();
            let mut header = header_of(&meta, tar::EntryType::Regular, size);
            // Exactly `size` bytes, even if a tool writes the file meanwhile.
            let data = file.take(size).chain(io::repeat(0)).take(size);
            builder
                .append_data(&mut header, &inside, data)
                .map_err(unreadable)?;
            counts.1 += 1;
            if builder.get_ref().get_ref().len() as u64 > MAX_STATE_BYTES {
                return Err(too_large(shown));
            }
        } else {
            return Err(refused(&inside, "a link or special file"));
        }
    }
    Ok(())
}

/// Sends `states` to the project of `target`: shows them (names, files, sizes, and whether a
/// restored copy is in the way), asks for `yes` unless unattended, and sends each once.
///
/// # Errors
/// [`SecretsImportError::Cancelled`] without `yes`; `Sandbox` for a target that is not one
/// project's agent secrets; API failures before anything was sent.
pub fn import_states(
    states: Vec<StateEntry>,
    target: &PreparedTarget,
    replace: bool,
    client: &VaultClient,
    io: &mut dyn SecretsIo,
) -> Result<StatesSummary, SecretsImportError> {
    let Some(sandbox) = target.project_sandbox() else {
        return Err(SecretsImportError::Sandbox(
            "agent states belong to one project: --to agent with --sandbox, without --instance"
                .into(),
        ));
    };
    let api = client.api();
    let url = api.endpoint(&["api", "sandboxes", sandbox, "agent-states"])?;
    let existing: Vec<StateView> = api.execute(&|c| c.get(url.clone()).timeout(JSON_TIMEOUT))?;
    io.say(&format!(
        "Send agent states (folders of operator state) to {} on {}:",
        target.heading(),
        client.server()
    ));
    for state in &states {
        let found = existing.iter().find(|s| s.name == state.name);
        let what = match found {
            None => "new".to_owned(),
            Some(StateView {
                restored_path: Some(path),
                ..
            }) if replace => format!("restored at {path}: --replace-state overwrites that copy"),
            Some(StateView {
                restored_path: Some(path),
                ..
            }) => format!(
                "restored at {path}, the copy written to now: refused without --replace-state"
            ),
            Some(found) => format!(
                "replaces version {} (not restored yet: this computer is still its writer)",
                found.version
            ),
        };
        io.say(&format!(
            "  + {}  {} files, {} packed, from {}: {what}",
            state.name,
            state.files,
            human_bytes(state.archive.len() as u64),
            state.source
        ));
    }
    if target.is_unattended() {
        io.say("Sending them without asking: --yes with the operate token made for your agent.");
    } else {
        let answer = io
            .ask("Type yes to send them: ")
            .map_err(|_| SecretsImportError::Cancelled)?;
        if !answer.trim().eq_ignore_ascii_case("yes") {
            return Err(SecretsImportError::Cancelled);
        }
    }
    let mut summary = StatesSummary::default();
    for state in states {
        let sent = send_state(client, sandbox, &state, replace);
        match sent {
            Ok(version) => {
                io.say(&format!("  sent {} (version {version})", state.name));
                summary.sent.push((state.name, version));
            }
            Err(err) => {
                io.say(&format!("  refused {}: {err}", state.name));
                summary.refused.push(state.name);
            }
        }
    }
    if !summary.sent.is_empty() {
        let sources: Vec<&str> = summary.sent.iter().map(|(n, _)| n.as_str()).collect();
        io.say(&format!(
            "Once a task restores {} (secret_state_restore), the sandbox's copy is the one written \
             to: stop using the folder on this computer for rollouts. Get the sandbox's copy back \
             with `yulius secrets state-get NAME --out FILE`.",
            sources.join(", ")
        ));
    }
    Ok(summary)
}

fn send_state(
    client: &VaultClient,
    sandbox: &str,
    state: &StateEntry,
    replace: bool,
) -> Result<u32, ApiError> {
    #[derive(Deserialize)]
    struct Stored {
        version: u32,
    }
    let api = client.api();
    let mut url = api.endpoint(&["api", "sandboxes", sandbox, "agent-states", &state.name])?;
    url.query_pairs_mut()
        .append_pair("description", &format!("From the folder {}", state.source));
    if replace {
        url.query_pairs_mut().append_pair("replace", "true");
    }
    let stored: Stored = api.execute(&|c| {
        c.put(url.clone())
            .timeout(CHUNK_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/gzip")
            .body(state.archive.clone())
    })?;
    Ok(stored.version)
}

/// `yulius secrets state-get NAME --out FILE`: writes the project's newest saved copy of the
/// state to a new file (mode 0600), never over an existing one.
///
/// # Errors
/// The file exists or cannot be written; API failures.
pub fn download_state(
    client: &VaultClient,
    target: &PreparedTarget,
    name: &str,
    out: &Path,
) -> Result<u64, String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let name = state_name(name)?;
    let name = name.as_str();
    let sandbox = target
        .project_sandbox()
        .ok_or("agent states belong to one project: pass --sandbox")?;
    let api = client.api();
    let url = api
        .endpoint(&["api", "sandboxes", sandbox, "agent-states", name, "archive"])
        .map_err(|e| e.to_string())?;
    let response = api
        .execute_raw(&|c| c.get(url.clone()).timeout(CHUNK_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let mut archive = Vec::new();
    response
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut archive)
        .map_err(|e| format!("reading the state failed: {e}"))?;
    if archive.len() as u64 > MAX_STATE_BYTES {
        return Err("the server sent more than a state holds".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out)
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    file.write_all(&archive)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(archive.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// SPEC-271 OPS-004: a folder packs with its modes and nested files; the archive round-trips.
    #[test]
    fn a_folder_packs_with_its_modes() -> TestResult {
        let dir = tempfile::tempdir()?;
        let state = dir.path().join("ops");
        std::fs::create_dir_all(state.join("rollouts/r1"))?;
        std::fs::write(state.join("inventory.sqlite"), "SQLite format 3\0")?;
        std::fs::set_permissions(
            state.join("inventory.sqlite"),
            std::fs::Permissions::from_mode(0o600),
        )?;
        std::fs::write(state.join("rollouts/r1/bundle.json"), "{}")?;
        let entry = state_entry("OPS", &state)?;
        assert_eq!(entry.files, 2);
        let mut tar = tar::Archive::new(GzDecoder::new(&entry.archive[..]));
        let mut seen = Vec::new();
        for item in tar.entries()? {
            let item = item?;
            seen.push((
                item.path()?.display().to_string(),
                item.header().mode()? & 0o777,
            ));
        }
        assert!(
            seen.contains(&("inventory.sqlite".to_owned(), 0o600)),
            "{seen:?}"
        );
        assert!(
            seen.iter().any(|(p, _)| p == "rollouts/r1/bundle.json"),
            "{seen:?}"
        );

        // An existing archive goes as it is.
        let packed = dir.path().join("ops.tar.gz");
        std::fs::write(&packed, &entry.archive)?;
        let again = state_entry("OPS", &packed)?;
        assert_eq!(again.archive, entry.archive);
        assert_eq!(again.files, 2);
        Ok(())
    }

    /// Vidoc, SPEC-271: entries are opened through the folder's handle without following a
    /// link, so a file swapped for a link after the listing is refused, never read.
    #[test]
    fn entries_are_opened_without_following_links() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("secret"), "key")?;
        std::os::unix::fs::symlink(dir.path().join("secret"), dir.path().join("swapped"))?;
        std::fs::create_dir(dir.path().join("sub"))?;
        std::os::unix::fs::symlink(dir.path().join("sub"), dir.path().join("swapped_dir"))?;
        let folder = open_folder(dir.path())?;
        let err = open_file_at(&folder, std::ffi::OsStr::new("swapped"))
            .err()
            .ok_or("followed a link to a file")?;
        assert!(err.contains("a link"), "{err}");
        let err = open_dir_at(&folder, std::ffi::OsStr::new("swapped_dir"))
            .err()
            .ok_or("followed a link to a folder")?;
        assert!(err.contains("a link"), "{err}");
        assert!(open_file_at(&folder, std::ffi::OsStr::new("secret")).is_ok());
        Ok(())
    }

    /// SPEC-271 OPS-004: a link, an empty folder, another file and a non-archive are refused
    /// before anything is sent.
    #[test]
    fn unsendable_folders_are_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let state = dir.path().join("ops");
        std::fs::create_dir(&state)?;
        let err = state_entry("OPS", &state).err().ok_or("empty sent")?;
        assert!(err.contains("is empty"), "{err}");
        std::fs::write(state.join("a"), "1")?;
        std::os::unix::fs::symlink("/etc/passwd", state.join("passwd"))?;
        let err = state_entry("OPS", &state).err().ok_or("link sent")?;
        assert!(err.contains("holds passwd, a link"), "{err}");
        let other = dir.path().join("notes.txt");
        std::fs::write(&other, "x")?;
        let err = state_entry("OPS", &other).err().ok_or("text sent")?;
        assert!(err.contains("neither a folder nor"), "{err}");
        // Vidoc, SPEC-271: a name is an agent secret name before it goes anywhere near a URL.
        for bad in ["../OTHER/archive", "ops?replace=true", "ops#x", "a/b", ""] {
            let err = state_entry(bad, &state).err().ok_or(bad)?;
            assert!(err.contains("is not a state name"), "{bad}: {err}");
        }
        let fake = dir.path().join("fake.tgz");
        std::fs::write(&fake, "not gzip")?;
        assert!(state_entry("OPS", &fake).is_err());
        assert!(state_entry("OPS", &dir.path().join("missing")).is_err());
        Ok(())
    }
}
