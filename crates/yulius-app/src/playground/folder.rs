//! A playground from a folder uploaded from the browser (SPEC-099 PLY-003, PLY-004).
//!
//! The browser packs the folder into a gzip tar: the working tree under `files/` (the layout of
//! `yulius onboard` snapshots, so [`extract_snapshot`] unpacks it with its bounds) and the
//! folder's `.git` under `git/`. Of `git/` only the history is taken ([`is_history`]): `HEAD`,
//! `packed-refs`, `shallow`, `objects/` and `refs/`. The server's Git runs outside containment,
//! and an uploaded `config` or hook could run commands. The history's branches and tags are then
//! copied into the new bare `origin` with the hardened Git. The working files land on top of the
//! branch `HEAD` names as one commit when they differ: uncommitted work included, ignored files
//! left out by the project's own `.gitignore`.
//!
//! Bounds: the body has at most [`MAX_FOLDER_UPLOAD_BYTES`] and goes to a temporary file under
//! `<data>/tmp`, removed whatever happens; the archive expands to at most the import bounds
//! ([`MAX_IMPORT_TOTAL_BYTES`], [`MAX_SNAPSHOT_ENTRIES`]); uploads share the upload slots.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::OwnedSemaphorePermit;
use yulius_contracts::api::sandboxes::Sandbox;
use yulius_contracts::import::MAX_IMPORT_TOTAL_BYTES;
use yulius_domain::delivery::DeliveryProfile;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::AccessLevel;

use super::{no_token, playground_name};
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::imports::{
    MAX_SNAPSHOT_ENTRIES, check_archive, extract_snapshot, prepare_target, safe_relative,
};
use crate::model::UserRecord;
use crate::projects::{blocking, identity, import_profile, remove_tree, write_scaffold};

/// Largest folder upload (the compressed archive).
pub const MAX_FOLDER_UPLOAD_BYTES: u64 = 1 << 30;
/// Where the archive's `.git` entries are.
const HISTORY_PREFIX: &[u8] = b"git/";
/// Most bytes an archive's entries may declare in all: the working tree and the history, each
/// within the import bound.
const MAX_ARCHIVE_EXPANSION: u64 = 2 * MAX_IMPORT_TOTAL_BYTES;
/// Longest `HEAD` file read.
const MAX_HEAD_BYTES: u64 = 1024;
/// Longest `yulius.toml` read for the delivery profile.
const MAX_YULIUS_TOML_BYTES: u64 = 256 * 1024;

/// A folder upload in progress: write the body with [`FolderUpload::write`], then hand it to
/// [`App::playground_from_folder`]. Dropping it removes its temporary file.
#[derive(Debug)]
pub struct FolderUpload {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    received: u64,
    _slot: OwnedSemaphorePermit,
}

impl FolderUpload {
    /// Appends a chunk of the body.
    ///
    /// # Errors
    /// `Invalid` beyond [`MAX_FOLDER_UPLOAD_BYTES`]; `Capacity` when the disk is full.
    pub async fn write(&mut self, chunk: &[u8]) -> AppResult<()> {
        let len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if self.received.saturating_add(len) > MAX_FOLDER_UPLOAD_BYTES {
            return Err(too_large());
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| AppError::internal("the upload was already finished"))?;
        file.write_all(chunk).await.map_err(|err| io_error(&err))?;
        self.received += len;
        Ok(())
    }
}

impl Drop for FolderUpload {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn too_large() -> AppError {
    AppError::Invalid(format!(
        "the folder is larger than the playground upload limit of {} GB once packed; leave \
         dependencies and build output out",
        MAX_FOLDER_UPLOAD_BYTES >> 30
    ))
}

fn io_error(err: &io::Error) -> AppError {
    if err.kind() == io::ErrorKind::StorageFull {
        AppError::Capacity("the server's disk is full".into())
    } else {
        AppError::internal(err)
    }
}

/// Whether `rel` (below `git/`) is part of a repository's history: `HEAD`, `packed-refs`,
/// `shallow`, or a file under `objects/` (but not `objects/info/`, the alternates) or `refs/`.
/// Configuration, hooks, `info/`, the index, logs and everything else are not.
fn is_history(rel: &Path) -> bool {
    let parts: Vec<&OsStr> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    match parts.first().map(|p| p.as_bytes()) {
        Some(b"HEAD" | b"packed-refs" | b"shallow") => parts.len() == 1,
        // `objects/info/` holds alternates, which would make Git read another repository on the
        // server; the objects themselves are everything else under `objects/`.
        Some(b"objects") => parts.len() > 1 && parts[1].as_bytes() != b"info",
        Some(b"refs") => parts.len() > 1,
        _ => false,
    }
}

/// Unpacks the history part of the archive's `git/` entries into `root` (a Git directory).
/// Returns the number of files written; links and every other entry are skipped.
///
/// # Errors
/// Unreadable archives and archives over the import bounds.
fn extract_history(archive: &Path, root: &Path) -> AppResult<u64> {
    check_archive(archive, MAX_ARCHIVE_EXPANSION)?;
    fs::create_dir_all(root).map_err(AppError::internal)?;
    let root = fs::canonicalize(root).map_err(AppError::internal)?;
    let file = File::open(archive).map_err(AppError::internal)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    let invalid = |err: io::Error| AppError::Invalid(format!("unreadable folder archive: {err}"));
    let (mut entries, mut bytes, mut files) = (0_u64, 0_u64, 0_u64);
    for entry in tar.entries().map_err(invalid)? {
        let mut entry = entry.map_err(invalid)?;
        entries += 1;
        if entries > MAX_SNAPSHOT_ENTRIES {
            return Err(AppError::Invalid(
                "the folder has too many files for a playground".into(),
            ));
        }
        let raw = entry.path_bytes().into_owned();
        let Some(rel) = raw.strip_prefix(HISTORY_PREFIX).and_then(safe_relative) else {
            continue;
        };
        if !entry.header().entry_type().is_file() || !is_history(&rel) {
            continue;
        }
        let size = entry.header().size().map_err(invalid)?;
        bytes = bytes.saturating_add(size);
        if bytes > MAX_IMPORT_TOTAL_BYTES {
            return Err(AppError::Invalid(
                "the folder's history expands beyond the import size limit".into(),
            ));
        }
        let Some(target) = prepare_target(&root, &rel)? else {
            continue;
        };
        let mut output = File::create(&target).map_err(AppError::internal)?;
        io::copy(&mut (&mut entry).take(size), &mut output).map_err(AppError::internal)?;
        files += 1;
    }
    // Git recognises a directory by `HEAD`, `objects/` and `refs/`; a packed repository may have
    // sent no file under `refs/` (its refs are in `packed-refs`), and a tar has no empty folders.
    if files > 0 {
        for dir in ["objects", "refs"] {
            fs::create_dir_all(root.join(dir)).map_err(AppError::internal)?;
        }
    }
    Ok(files)
}

/// The branch an uploaded `HEAD` names (`ref: refs/heads/<branch>`), if it is one.
fn head_branch(git_dir: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(git_dir.join("HEAD"))
        .ok()?
        .take(MAX_HEAD_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    text.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_owned)
}

/// A small text file of the working tree, if present.
fn read_small(path: &Path, max: u64) -> Option<String> {
    // Only a regular file: an uploaded link could point anywhere on the server.
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(max)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

impl App {
    /// `POST /api/playgrounds`: starts a folder upload for the signed-in caller; `declared` is
    /// the request's `Content-Length`, refused before any byte is read when over the limit.
    ///
    /// # Errors
    /// `Invalid` above the limit; `Capacity` beyond the concurrent uploads; I/O errors.
    pub async fn begin_folder_upload(
        &self,
        ctx: &AuthContext,
        declared: Option<u64>,
    ) -> AppResult<FolderUpload> {
        if declared.is_some_and(|len| len > MAX_FOLDER_UPLOAD_BYTES) {
            return Err(too_large());
        }
        self.playground_quota(&ctx.user).await?;
        let slot = self.upload_slot()?;
        let dir = self.config.data_dir.join("tmp");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| io_error(&err))?;
        let path = dir.join(format!("playground-{}.tgz", self.deps.crypto.new_id()));
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|err| io_error(&err))?;
        Ok(FolderUpload {
            path,
            file: Some(file),
            received: 0,
            _slot: slot,
        })
    }

    /// Makes a playground named `name` from a finished folder upload (PLY-001, PLY-003). Nothing
    /// of it stays behind when it fails.
    ///
    /// # Errors
    /// `Invalid` for an empty name, an unreadable, empty or over-limit archive; Git failures.
    pub async fn playground_from_folder(
        self: &Arc<Self>,
        ctx: &AuthContext,
        mut upload: FolderUpload,
        name: &str,
    ) -> AppResult<Sandbox> {
        let name = playground_name(name)?;
        if let Some(mut file) = upload.file.take() {
            file.flush().await.map_err(|err| io_error(&err))?;
            file.sync_all().await.map_err(|err| io_error(&err))?;
        }
        let id = SandboxId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        let staging = self.sandbox_dir(&id).join("upload");
        let seeded = self
            .seed_from_folder(&id, &upload.path, &staging, &ctx.user)
            .await;
        remove_tree(staging).await;
        drop(upload);
        let profile = match seeded {
            Ok(profile) => profile,
            Err(err) => {
                self.discard_playground(&id).await;
                return Err(err);
            }
        };
        let record = self
            .record_playground(id, &name, &profile, &ctx.user, "folder")
            .await?;
        // Its maker has admin on it (PLY-005).
        self.sandbox_view(&record, AccessLevel::Admin).await
    }

    /// The bare `origin` from the archive (history, then the working files as one commit), its
    /// production and integration branches, and the sandbox clone. Returns the profile.
    async fn seed_from_folder(
        &self,
        id: &SandboxId,
        archive: &Path,
        staging: &Path,
        user: &UserRecord,
    ) -> AppResult<DeliveryProfile> {
        let history =
            std::path::absolute(staging.join("history.git")).map_err(AppError::internal)?;
        let work = staging.join("work");
        let (from, into) = (archive.to_path_buf(), history.clone());
        let history_files = blocking(move || extract_history(&from, &into)).await?;
        let has_history = history_files > 0 && history.join("HEAD").is_file();
        let git = &self.deps.git;
        let branch = has_history
            .then(|| head_branch(&history))
            .flatten()
            .filter(|b| git.valid_branch(b))
            .unwrap_or_else(|| "main".to_owned());
        let origin = self.playground_origin(id);
        git.init_bare(&origin, &branch).await?;
        if has_history {
            git.copy_refs(&origin, &history).await?;
        }
        git.sync_clone(&origin.to_string_lossy(), &work, &no_token())
            .await?;
        if !git.branch_exists(&work, &branch).await? {
            let root = work.clone();
            let unborn = branch.clone();
            blocking(move || write_scaffold(&root, &unborn, &[])).await?;
        }
        let (from, into) = (archive.to_path_buf(), work.clone());
        let stats = blocking(move || extract_snapshot(&from, &into)).await?;
        if stats.files + stats.links == 0 && !has_history {
            return Err(AppError::Invalid(
                "the folder holds no files to start a playground with".into(),
            ));
        }
        if has_history {
            // The browser sends no file modes and follows links: keep the repository's.
            git.keep_modes_and_links(&work).await?;
        }
        let (author, email) = identity(user);
        let message = if has_history {
            "chore: the folder's uncommitted work, as uploaded to a Yulius playground"
        } else {
            "chore: the folder as uploaded to a Yulius playground"
        };
        git.commit_all(&work, message, &author, &email).await?;
        if !git.branch_exists(&work, &branch).await? {
            return Err(AppError::Invalid(
                "the folder's branch has no commit and the folder no file to commit".into(),
            ));
        }
        git.push(&work, &branch, &branch, &no_token()).await?;
        let toml = {
            let path = work.join("yulius.toml");
            blocking(move || Ok(read_small(&path, MAX_YULIUS_TOML_BYTES))).await?
        };
        let mut profile = import_profile("", "", toml.as_deref());
        if toml.is_none_or(|text| !text.contains("[delivery]")) {
            profile.production_branch.clone_from(&branch);
        }
        // A folder whose own branch is the integration branch promotes to `main`.
        if profile.production_branch == profile.integration_branch {
            profile.production_branch = "main".to_owned();
        }
        // A playground has no deploy: commands from an uploaded `yulius.toml` never run on the
        // server, whoever made the playground (SPEC-099).
        profile.environments.clear();
        profile.auto_promote = false;
        for target in [&profile.production_branch, &profile.integration_branch] {
            if *target != branch && !git.branch_exists(&work, target).await? {
                git.push(&work, &branch, target, &no_token()).await?;
            }
        }
        self.clone_playground(id, &origin).await?;
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_history_of_a_git_directory_is_taken() {
        for taken in [
            "HEAD",
            "packed-refs",
            "shallow",
            "objects/ab/cdef",
            "objects/pack/p.pack",
            "refs/heads/main",
            "refs/tags/v1",
        ] {
            assert!(is_history(Path::new(taken)), "{taken}");
        }
        for refused in [
            "config",
            "hooks/pre-commit",
            "info/attributes",
            "index",
            "logs/HEAD",
            "objects",
            "refs",
            "HEAD/x",
            "modules/a/config",
            "FETCH_HEAD",
            // Alternates would make Git read another repository on the server.
            "objects/info/alternates",
            "objects/info/http-alternates",
            "description",
        ] {
            assert!(!is_history(Path::new(refused)), "{refused}");
        }
    }

    #[test]
    fn head_names_a_branch_or_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("HEAD"), "ref: refs/heads/trunk\n")?;
        assert_eq!(head_branch(dir.path()).as_deref(), Some("trunk"));
        fs::write(
            dir.path().join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )?;
        assert_eq!(
            head_branch(dir.path()),
            None,
            "a detached HEAD names no branch"
        );
        Ok(())
    }

    /// A gzip tar of `entries` (path, declared size, contents); a declared size larger than the
    /// contents writes only the header, as a crafted archive would.
    fn archive(
        dir: &Path,
        entries: &[(&str, u64, &[u8])],
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = dir.join("a.tgz");
        let gz = flate2::write::GzEncoder::new(File::create(&path)?, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (name, size, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_path(name)?;
            header.set_size(*size);
            header.set_mode(0o644);
            header.set_cksum();
            if *size == data.len() as u64 {
                tar.append(&header, *data)?;
            } else {
                std::io::Write::write_all(tar.get_mut(), header.as_bytes())?;
            }
        }
        tar.into_inner()?.finish()?;
        Ok(path)
    }

    #[test]
    fn only_the_history_of_an_uploaded_git_directory_is_written()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let entries: Vec<(&str, u64, &[u8])> = [
            "git/HEAD",
            "git/config",
            "git/hooks/post-checkout",
            "git/info/attributes",
            "git/objects/ab/cdef",
            "git/objects/info/alternates",
            "git/refs/heads/main",
            "files/README.md",
        ]
        .into_iter()
        .map(|name| (name, 2, &b"x\n"[..]))
        .collect();
        let archive = archive(dir.path(), &entries)?;
        let root = dir.path().join("history.git");
        assert_eq!(extract_history(&archive, &root)?, 3);
        for kept in ["HEAD", "objects/ab/cdef", "refs/heads/main"] {
            assert!(root.join(kept).is_file(), "{kept}");
        }
        for refused in [
            "config",
            "hooks/post-checkout",
            "info/attributes",
            "objects/info/alternates",
            "README.md",
        ] {
            assert!(!root.join(refused).exists(), "{refused}");
        }
        Ok(())
    }

    #[test]
    fn an_archive_declaring_more_than_the_bound_is_refused_unread()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        // Entries the history pass skips still count: their bytes would be decompressed.
        let archive = archive(dir.path(), &[("files/huge.bin", 5 << 30, &b""[..])])?;
        let refused = extract_history(&archive, &dir.path().join("history.git"));
        assert!(
            matches!(&refused, Err(AppError::Invalid(m)) if m.contains("size limit")),
            "refused by its declared size, before reading it: {refused:?}"
        );
        Ok(())
    }

    /// A gzip tar holding one raw `kind` entry of `data` (a GNU long name, a PAX record) before a
    /// small file.
    fn with_meta(
        dir: &Path,
        kind: tar::EntryType,
        data: &[u8],
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = dir.join("m.tgz");
        let gz = flate2::write::GzEncoder::new(File::create(&path)?, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut meta = tar::Header::new_gnu();
        meta.set_path("././@LongLink")?;
        meta.set_entry_type(kind);
        meta.set_size(data.len() as u64);
        meta.set_cksum();
        tar.append(&meta, data)?;
        let mut file = tar::Header::new_gnu();
        file.set_path("git/HEAD")?;
        file.set_size(2);
        file.set_mode(0o644);
        file.set_cksum();
        tar.append(&file, &b"x\n"[..])?;
        tar.into_inner()?.finish()?;
        Ok(path)
    }

    #[test]
    fn oversized_names_and_size_overrides_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        // The tar reader reads a long name whole, in memory, before any loop sees it.
        let long = with_meta(
            dir.path(),
            tar::EntryType::GNULongName,
            &vec![b'a'; 1 << 20],
        )?;
        let refused =
            |r: AppResult<u64>| matches!(r, Err(AppError::Invalid(m)) if m.contains("metadata"));
        assert!(refused(extract_history(&long, &dir.path().join("h1"))));
        assert!(refused(
            extract_snapshot(&long, dir.path()).map(|s| s.files)
        ));
        // A PAX `size` would make the next entry larger than its header says.
        let pax = with_meta(dir.path(), tar::EntryType::XHeader, b"19 size=9999999999\n")?;
        assert!(refused(extract_history(&pax, &dir.path().join("h2"))));
        Ok(())
    }
}
