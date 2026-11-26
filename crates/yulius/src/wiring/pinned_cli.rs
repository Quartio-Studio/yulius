//! The pinned agent CLIs Yulius provides to runs and sign-ins: Codex (SPEC-503 CLI-001, CDX-012,
//! ADR-0355) and Antigravity (SPEC-505 AGY-007, ADR-0361).
//!
//! **Install.** On the first need (a run or sign-in; for Codex also at start when Codex accounts
//! exist) the platform tarball of the pinned release is downloaded (Codex: npm's registry;
//! Antigravity: Google's release storage), its sha512 checked against the one pinned in the
//! source (not the server's word), and its release tree (Codex: `package/vendor/<triple>/`, the
//! native `bin/codex`, `codex-path/rg`, `codex-resources`, `codex-package.json`; Antigravity: the
//! single `antigravity`) unpacked into a temporary directory: regular files only, no links, no
//! path leaving it, bounded. The binary must be a 64-bit ELF for this platform. The tree then
//! moves to `<root>/<version>-<platform>` by an atomic rename; other releases there go once no
//! process runs them. Nothing runs `npm`, a package script, an installer or the binary on the
//! host.
//!
//! **Use.** Runs and sign-ins start the binary by its absolute path through
//! `HarnessPort::provide_codex` and `provide_antigravity`; a contained run binds the release's
//! directory read-only at the same path, so a sandbox without the CLI in its system layer still
//! runs it. Installs are one at a time: callers wait for the one under way.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt as _;
use yulius_app::ports::harness::ProvidedCli;
use yulius_domain::agent_cli::REGISTRY_HOST;
use yulius_domain::agent_cli::{antigravity, codex};

use crate::wiring::claude_cli::{
    REGISTRY, UpdateError, running_executables, sha512_of, unique_suffix,
};

/// The largest tarball downloaded (Codex 0.169.0's linux-x64 package is 167 MB packed,
/// Antigravity 1.5.10's 100 MB).
pub const MAX_TARBALL: u64 = 512 * 1024 * 1024;
/// The largest unpacked release (Codex 0.169.0's is 423 MB, Antigravity 1.5.10's 267 MB).
pub const MAX_UNPACKED: u64 = 1536 * 1024 * 1024;
/// Deadline of the download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Deadline of connecting to the registry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Prefix of temporary entries in the root, removed by the next install.
const STAGING: &str = ".staging-";
/// How long a failed install answers callers at once with its reason before one tries again:
/// every start would otherwise wait for its own failing download.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

fn fail<T>(why: impl Into<String>) -> Result<T, UpdateError> {
    Err(UpdateError::new(why.into()))
}

/// How a tarball's pinned sha512 is written in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Digest512 {
    /// npm's `integrity` (`sha512-<base64>`).
    Integrity(&'static str),
    /// Hexadecimal, as Google's release manifests give it.
    Hex(&'static str),
}

impl Digest512 {
    fn bytes(self) -> Result<Vec<u8>, UpdateError> {
        match self {
            Self::Integrity(integrity) => sha512_of(integrity),
            Self::Hex(hex) => (0..hex.len())
                .step_by(2)
                .map(|i| {
                    hex.get(i..i + 2)
                        .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                })
                .collect::<Option<Vec<u8>>>()
                .filter(|digest| digest.len() == 64)
                .ok_or_else(|| UpdateError::new("the pinned sha512 is not 128 hex digits")),
        }
    }
}

/// One platform's release of a pinned CLI: where its tarball lies, what it must hash to, and
/// what of it is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// How people name it ("Codex CLI").
    label: &'static str,
    /// The release's version.
    version: &'static str,
    /// The server named when it cannot be reached.
    host: &'static str,
    /// The tarball's path on the server.
    tarball_path: String,
    /// Its pinned sha512.
    digest: Digest512,
    /// The tarball's folder holding the release (`""`: its root).
    archive_dir: String,
    /// The binary below the installed release's directory.
    binary: &'static str,
    /// Name of the installed release's directory.
    install_name: String,
    /// ELF `e_machine` of the binary.
    machine: u16,
}

impl Release {
    /// The Codex CLI's release for `platform` (SPEC-503).
    #[must_use]
    pub fn codex(platform: &codex::Platform) -> Self {
        Self {
            label: "Codex CLI",
            version: codex::VERSION,
            host: REGISTRY_HOST,
            tarball_path: platform.tarball_path(),
            digest: Digest512::Integrity(platform.integrity),
            archive_dir: platform.vendor_dir(),
            binary: codex::BINARY,
            install_name: platform.install_name(),
            machine: platform.machine,
        }
    }

    /// The Antigravity CLI's release for `platform` (SPEC-505 AGY-007).
    #[must_use]
    pub fn antigravity(platform: &antigravity::Platform) -> Self {
        Self {
            label: "Antigravity CLI",
            version: antigravity::VERSION,
            host: antigravity::HOST,
            tarball_path: platform.tarball_path(),
            digest: Digest512::Hex(platform.sha512),
            archive_dir: String::new(),
            binary: antigravity::BINARY,
            install_name: platform.install_name(),
            machine: platform.machine,
        }
    }
}

/// Where Google's Antigravity releases are downloaded from.
pub const ANTIGRAVITY_SERVER: &str = "https://storage.googleapis.com";

/// The Codex CLI's installer (SPEC-503): a [`PinnedCli`] made by [`PinnedCli::codex`].
pub type CodexCli = PinnedCli;

/// Installs and provides a pinned CLI under `root`.
pub struct PinnedCli {
    http: reqwest::Client,
    registry: String,
    root: PathBuf,
    release: Option<Release>,
    /// How people name it, also when this host gets no release.
    label: &'static str,
    max_tarball: u64,
    max_unpacked: u64,
    /// Held while one install runs; the others wait for it.
    installing: tokio::sync::Mutex<()>,
    /// The last failed install: when, and why.
    failed: std::sync::Mutex<Option<(std::time::Instant, UpdateError)>>,
    retry_after: Duration,
    /// Earlier releases were looked at by this process.
    pruned: std::sync::atomic::AtomicBool,
}

impl PinnedCli {
    /// The Codex CLI's provider of `root` (`<data>/codex-cli`) over npm's registry, for this
    /// host's platform.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn codex(root: PathBuf) -> Result<Self, String> {
        let platform = codex::platform(std::env::consts::OS, std::env::consts::ARCH);
        Self::codex_from(root, REGISTRY.to_owned(), platform, true)
    }

    /// The same over another registry and platform package (tests).
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn codex_from(
        root: PathBuf,
        registry: String,
        platform: Option<codex::Platform>,
        https_only: bool,
    ) -> Result<Self, String> {
        let release = platform.as_ref().map(Release::codex);
        Self::with_release(root, registry, release, "Codex CLI", https_only)
    }

    /// The Antigravity CLI's provider of `root` (`<data>/antigravity-cli`) over Google's release
    /// storage, for this host's platform (SPEC-505 AGY-007).
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn antigravity(root: PathBuf) -> Result<Self, String> {
        let platform = antigravity::platform(std::env::consts::OS, std::env::consts::ARCH);
        Self::antigravity_from(root, ANTIGRAVITY_SERVER.to_owned(), platform, true)
    }

    /// The same over another server and platform (tests).
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn antigravity_from(
        root: PathBuf,
        server: String,
        platform: Option<antigravity::Platform>,
        https_only: bool,
    ) -> Result<Self, String> {
        let release = platform.as_ref().map(Release::antigravity);
        Self::with_release(root, server, release, "Antigravity CLI", https_only)
    }

    fn with_release(
        root: PathBuf,
        registry: String,
        release: Option<Release>,
        label: &'static str,
        https_only: bool,
    ) -> Result<Self, String> {
        let http = super::http::client_builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .https_only(https_only)
            .build()
            .map_err(|e| format!("registry client setup failed: {e}"))?;
        Ok(Self {
            http,
            registry: registry.trim_end_matches('/').to_owned(),
            root,
            release,
            label,
            max_tarball: MAX_TARBALL,
            max_unpacked: MAX_UNPACKED,
            installing: tokio::sync::Mutex::new(()),
            failed: std::sync::Mutex::new(None),
            pruned: std::sync::atomic::AtomicBool::new(false),
            retry_after: RETRY_AFTER,
        })
    }

    /// A smaller download bound (tests).
    #[must_use]
    pub fn with_max_tarball(mut self, bytes: u64) -> Self {
        self.max_tarball = bytes;
        self
    }

    /// Another wait before a failed install is tried again (tests).
    #[must_use]
    pub fn with_retry_after(mut self, wait: Duration) -> Self {
        self.retry_after = wait;
        self
    }

    /// A smaller unpacked bound (tests).
    #[must_use]
    pub fn with_max_unpacked(mut self, bytes: u64) -> Self {
        self.max_unpacked = bytes;
        self
    }

    /// The installed pinned release, if it is there.
    #[must_use]
    pub fn installed(&self) -> Option<ProvidedCli> {
        let release = self.release.as_ref()?;
        let dir = self.root.join(&release.install_name);
        let program = dir.join(release.binary);
        std::fs::symlink_metadata(&program)
            .is_ok_and(|m| m.is_file())
            .then_some(ProvidedCli { program, dir })
    }

    /// What a caller that must not wait gets: the installed release, the last failed install's
    /// reason while it is recent, or `None` when no install was tried yet.
    ///
    /// # Errors
    /// The recent failure.
    pub fn ready(&self) -> Result<Option<ProvidedCli>, UpdateError> {
        if self.release.is_none() {
            return Err(unsupported(self.label));
        }
        if let Some(provided) = self.installed() {
            return Ok(Some(provided));
        }
        if let Ok(failed) = self.failed.lock()
            && let Some((at, why)) = failed.as_ref()
            && at.elapsed() < self.retry_after
        {
            return Err(why.clone());
        }
        Ok(None)
    }

    /// The pinned release, installed first when it is not there yet. The install runs on its
    /// own task: a caller that goes away (a closed request) does not cut it short.
    ///
    /// # Errors
    /// No package for this platform, or the download or its checks failed; nothing is left
    /// installed then.
    pub async fn ensure(self: &Arc<Self>) -> Result<ProvidedCli, UpdateError> {
        if let Some(provided) = self.installed() {
            self.prune_once();
            return Ok(provided);
        }
        let me = Arc::clone(self);
        tokio::spawn(async move { me.ensure_now().await })
            .await
            .map_err(|e| UpdateError::new(format!("the {} install stopped: {e}", self.label)))?
    }

    /// Looks once per process for earlier releases to remove (one still running at the last
    /// install stays until then).
    fn prune_once(self: &Arc<Self>) {
        let Some(release) = self.release.clone() else {
            return;
        };
        if self.pruned.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move { me.remove_others(&release).await });
    }

    async fn ensure_now(&self) -> Result<ProvidedCli, UpdateError> {
        let Some(release) = self.release.clone() else {
            return Err(unsupported(self.label));
        };
        let _one = self.installing.lock().await;
        // Another caller may have installed it while this one waited, or just failed to.
        if let Some(provided) = self.ready()? {
            return Ok(provided);
        }
        let result = self.install_now(&release).await;
        if let Ok(mut failed) = self.failed.lock() {
            *failed = result
                .as_ref()
                .err()
                .map(|why| (std::time::Instant::now(), why.clone()));
        }
        result
    }

    async fn install_now(&self, release: &Release) -> Result<ProvidedCli, UpdateError> {
        self.remove_temporary().await;
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| UpdateError::new(format!("cannot create {}: {e}", self.root.display())))?;
        set_mode(&self.root, 0o755).await?;
        let staging = self.root.join(format!("{STAGING}{}", unique_suffix()));
        let installed = self.install(release, &staging).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        installed?;
        self.remove_others(release).await;
        self.pruned.store(true, std::sync::atomic::Ordering::SeqCst);
        self.installed().ok_or_else(|| {
            UpdateError::new(format!("the installed {} is not in place", release.label))
        })
    }

    async fn install(&self, release: &Release, staging: &Path) -> Result<(), UpdateError> {
        let expected = release.digest.bytes()?;
        let release_dir = staging.join("release");
        let create = |e: std::io::Error| {
            UpdateError::new(format!("cannot create {}: {e}", staging.display()))
        };
        // Only Yulius enters the staging folder, from its creation and whatever the umask: nobody
        // swaps the hashed tarball before it is unpacked.
        tokio::fs::DirBuilder::new()
            .mode(0o700)
            .create(staging)
            .await
            .map_err(create)?;
        set_mode(staging, 0o700).await?;
        tokio::fs::create_dir(&release_dir).await.map_err(create)?;
        set_mode(&release_dir, 0o755).await?;
        let tarball = staging.join("download.tgz");
        let url = format!("{}{}", self.registry, release.tarball_path);
        let digest = self.download(release, &url, &tarball).await?;
        if digest != expected {
            return fail(format!(
                "the {} tarball's sha512 is not the pinned one",
                release.label
            ));
        }
        let (from, to, max, of) = (
            tarball.clone(),
            release_dir.clone(),
            self.max_unpacked,
            release.clone(),
        );
        tokio::task::spawn_blocking(move || unpack(&from, &to, &of, max))
            .await
            .map_err(|e| UpdateError::new(format!("unpacking stopped: {e}")))??;
        let _ = tokio::fs::remove_file(&tarball).await;
        let target = self.root.join(&release.install_name);
        if tokio::fs::symlink_metadata(&target).await.is_ok() {
            tokio::fs::remove_dir_all(&target)
                .await
                .map_err(|e| UpdateError::new(format!("cannot replace a partial release: {e}")))?;
        }
        tokio::fs::rename(&release_dir, &target)
            .await
            .map_err(|e| UpdateError::new(format!("cannot move the release in place: {e}")))?;
        if let Ok(dir) = tokio::fs::File::open(&self.root).await {
            let _ = dir.sync_all().await;
        }
        tracing::info!(
            cli = release.label,
            version = release.version,
            dir = %target.display(),
            "agent CLI installed"
        );
        Ok(())
    }

    /// Streams the tarball to `to`, bounded; its sha512.
    async fn download(
        &self,
        release: &Release,
        url: &str,
        to: &Path,
    ) -> Result<Vec<u8>, UpdateError> {
        let mut response = self
            .http
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|_| UpdateError::new(format!("{} is unreachable", release.host)))?;
        if !response.status().is_success() {
            return fail(format!(
                "the {} download answered HTTP {}",
                release.label,
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.max_tarball)
        {
            return fail(format!(
                "the {} tarball is larger than allowed",
                release.label
            ));
        }
        let write = |e: std::io::Error| UpdateError::new(format!("cannot write the tarball: {e}"));
        let mut file = tokio::fs::File::create(to).await.map_err(write)?;
        let mut hasher = Sha512::new();
        let mut size = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| UpdateError::new(format!("the {} download stopped", release.label)))?
        {
            size += chunk.len() as u64;
            if size > self.max_tarball {
                return fail(format!(
                    "the {} tarball is larger than allowed",
                    release.label
                ));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(write)?;
        }
        file.flush().await.map_err(write)?;
        Ok(hasher.finalize().to_vec())
    }

    /// Removes releases other than `keep`'s that no process runs (an earlier pin).
    async fn remove_others(&self, keep: &Release) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return;
        };
        let mut others = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name != keep.install_name && !name.starts_with('.') {
                others.push(entry.path());
            }
        }
        let programs: Vec<PathBuf> = others.iter().map(|d| d.join(keep.binary)).collect();
        let running = tokio::task::spawn_blocking(move || {
            let paths: Vec<&Path> = programs.iter().map(PathBuf::as_path).collect();
            running_executables(&paths)
        })
        .await
        .unwrap_or_default();
        for (index, dir) in others.iter().enumerate() {
            if running.contains(&index) {
                tracing::info!(dir = %dir.display(), cli = keep.label, "an earlier release still runs: kept for now");
                continue;
            }
            if let Err(error) = tokio::fs::remove_dir_all(dir).await {
                tracing::warn!(dir = %dir.display(), cli = keep.label, %error, "cannot remove an earlier release");
            }
        }
    }

    /// Removes what an interrupted install left.
    async fn remove_temporary(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(STAGING))
            {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
}

/// Why this host gets no provided `label`.
fn unsupported(label: &str) -> UpdateError {
    UpdateError::new(format!(
        "Yulius provides no {label} for {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

/// Sets `path`'s permission bits.
async fn set_mode(path: &Path, mode: u32) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .map_err(|e| UpdateError::new(format!("cannot set the mode of {}: {e}", path.display())))
}

/// Unpacks the tarball's release tree (its `archive_dir`) into `to`: regular files (0755 when
/// executable, else 0644) in directories of 0755, every path inside `to`, at most `max` bytes in
/// all; and checks its binary is a 64-bit little-endian ELF for the platform's machine.
fn unpack(tarball: &Path, to: &Path, release: &Release, max: u64) -> Result<(), UpdateError> {
    use std::io::{Read as _, Write as _};
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    let io = |e: std::io::Error| UpdateError::new(format!("cannot read the tarball: {e}"));
    let label = release.label;
    let write = |e: std::io::Error| UpdateError::new(format!("cannot unpack the {label}: {e}"));
    let prefix = release.archive_dir.as_str();
    let file = std::fs::File::open(tarball).map_err(io)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut total = 0u64;
    for entry in archive.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        let path = entry.path().map_err(io)?.into_owned();
        let Some(relative) = path
            .to_str()
            .and_then(|p| p.strip_prefix(prefix))
            .map(PathBuf::from)
        else {
            continue;
        };
        let kind = entry.header().entry_type();
        if kind == tar::EntryType::Directory {
            continue;
        }
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return fail(format!(
                "the {label} tarball names a path outside its release"
            ));
        }
        if kind != tar::EntryType::Regular {
            return fail(format!(
                "the {label} tarball holds an entry that is not a regular file"
            ));
        }
        let target = to.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(parent)
                .map_err(write)?;
        }
        let executable = entry.header().mode().map_err(io)? & 0o111 != 0;
        let mode = if executable { 0o755 } else { 0o644 };
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&target)
            .map_err(write)?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max {
                return fail(format!("the {label} is larger than allowed"));
            }
            out.write_all(&buf[..n]).map_err(write)?;
        }
        // The open's mode passed through the umask; every sandbox user runs it.
        out.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(write)?;
        out.sync_all().map_err(write)?;
    }
    // The builder's mode passed through the umask; every sandbox user enters these.
    open_dirs(to).map_err(write)?;
    let program = to.join(release.binary);
    let mut header = [0u8; 20];
    let read = std::fs::symlink_metadata(&program)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::File::open(&program).ok())
        .is_some_and(|mut f| f.read_exact(&mut header).is_ok());
    if !read {
        return fail(format!("the {label} tarball holds no {}", release.binary));
    }
    let elf = header[..4] == *b"\x7fELF"
        && header[4] == 2
        && header[5] == 1
        && u16::from_le_bytes([header[18], header[19]]) == release.machine;
    if !elf {
        return fail(format!(
            "the {label} is not a 64-bit ELF for machine {}",
            release.machine
        ));
    }
    Ok(())
}

/// Sets every directory below `dir` (itself included) to 0755; links are not followed.
fn open_dirs(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            open_dirs(&entry.path())?;
        }
    }
    Ok(())
}
