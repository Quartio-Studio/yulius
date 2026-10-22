//! Agent states from a laptop (SPEC-271, ADR-0200): `yulius secrets import --state NAME=DIR`
//! packs a folder of operator state (a deploy tool's inventory, rollout folders, configs) into a
//! gzip tar and sends it to a project's vault, where a task restores it into the sandbox home;
//! `yulius secrets state-get` downloads the sandbox's newest saved copy.
//!
//! What is said names states, sources, file counts and sizes only, never content. Once a task
//! restores a state, the sandbox's copy is the one written to: the server refuses a second send
//! over it unless the person replaces it on purpose (`--replace-state`).

use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Deserialize;

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
        name: name.trim().to_owned(),
        source: shown,
        archive,
        files,
    })
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
/// permission bits. A link or special file refuses it, so nothing is silently left behind.
///
/// # Errors
/// Unreadable, holding a link or special file, more than 20 000 entries, over 64 MiB packed.
pub fn pack_state_dir(dir: &Path) -> Result<(Vec<u8>, u64), String> {
    let shown = dir.display().to_string();
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut counts = (0_u64, 0_u64);
    add_folder(&mut builder, dir, Path::new(""), &shown, &mut counts)?;
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

/// Adds the entries of `dir` (at `rel` in the state); `counts` is (entries, files).
fn add_folder(
    builder: &mut tar::Builder<GzEncoder<Vec<u8>>>,
    dir: &Path,
    rel: &Path,
    shown: &str,
    counts: &mut (u64, u64),
) -> Result<(), String> {
    let unreadable = |e: io::Error| format!("cannot read {shown}: {e}");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .map_err(unreadable)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<Result<_, _>>()
        .map_err(unreadable)?;
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let inside: PathBuf = rel.join(&name);
        counts.0 += 1;
        if counts.0 > MAX_STATE_ENTRIES {
            return Err(format!(
                "{shown} holds more than {MAX_STATE_ENTRIES} files and folders"
            ));
        }
        let meta = std::fs::symlink_metadata(&path).map_err(unreadable)?;
        let mode = meta.permissions().mode() & 0o777;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let kind = meta.file_type();
        let mut header = tar::Header::new_gnu();
        header.set_mode(mode);
        header.set_mtime(mtime);
        if kind.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            builder
                .append_data(&mut header, &inside, io::empty())
                .map_err(unreadable)?;
            add_folder(builder, &path, &inside, shown, counts)?;
        } else if kind.is_file() {
            let file = std::fs::File::open(&path).map_err(unreadable)?;
            let size = file.metadata().map_err(unreadable)?.len();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(size);
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
            return Err(format!(
                "{shown} holds {}, a link or special file: only files and folders move; replace \
                 it with what it points to, or move it out of the folder",
                inside.display()
            ));
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
        let fake = dir.path().join("fake.tgz");
        std::fs::write(&fake, "not gzip")?;
        assert!(state_entry("OPS", &fake).is_err());
        assert!(state_entry("OPS", &dir.path().join("missing")).is_err());
        Ok(())
    }
}
