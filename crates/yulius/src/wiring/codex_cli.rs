//! The pinned Codex CLI provided to Codex runs and sign-ins (SPEC-451 CLI-001, CDX-012,
//! ADR-0290).
//!
//! **Install.** On the first need (a Codex run or sign-in, or at start when Codex accounts
//! exist) the platform package of [`codex::VERSION`] is downloaded from npm's registry, its
//! sha512 checked against the one pinned in [`codex::platform`] (not the registry's word), and
//! its `package/vendor/<triple>/` tree (the native `bin/codex`, `codex-path/rg`,
//! `codex-resources`, `codex-package.json`) unpacked into a temporary directory: regular files
//! only, no links, no path leaving it, bounded. `bin/codex` must be a 64-bit ELF for this
//! platform. The tree then moves to `<data>/codex-cli/<version>-<platform>` by an atomic rename;
//! other releases there go once no process runs them. Nothing runs `npm`, a package script or
//! the binary on the host.
//!
//! **Use.** Runs start `<dir>/bin/codex` by its absolute path; a contained run binds `<dir>`
//! read-only at the same path, so a sandbox without Codex in its system layer still runs it.
//! Installs are one at a time: callers wait for the one under way.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt as _;
use yulius_app::ports::harness::ProvidedCli;
use yulius_domain::agent_cli::REGISTRY_HOST;
use yulius_domain::agent_cli::codex::{self, Platform};

use crate::wiring::claude_cli::{
    REGISTRY, UpdateError, running_executables, sha512_of, unique_suffix,
};

/// The largest tarball downloaded (0.167.0's linux-x64 package is 174 MB packed).
pub const MAX_TARBALL: u64 = 512 * 1024 * 1024;
/// The largest unpacked release (0.167.0's is 459 MB).
pub const MAX_UNPACKED: u64 = 1536 * 1024 * 1024;
/// Deadline of the download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Deadline of connecting to the registry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Prefix of temporary entries in the root, removed by the next install.
const STAGING: &str = ".staging-";
/// How long a failed install answers callers at once with its reason before one tries again:
/// every Codex start would otherwise wait for its own failing download.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

fn fail<T>(why: impl Into<String>) -> Result<T, UpdateError> {
    Err(UpdateError(why.into()))
}

/// Installs and provides the pinned Codex CLI under `root`.
pub struct CodexCli {
    http: reqwest::Client,
    registry: String,
    root: PathBuf,
    platform: Option<Platform>,
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

impl CodexCli {
    /// The provider of `root` (`<data>/codex-cli`) over npm's registry, for this host's platform.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn new(root: PathBuf) -> Result<Self, String> {
        let platform = codex::platform(std::env::consts::OS, std::env::consts::ARCH);
        Self::with_registry(root, REGISTRY.to_owned(), platform, true)
    }

    /// The same over another registry and platform package (tests).
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn with_registry(
        root: PathBuf,
        registry: String,
        platform: Option<Platform>,
        https_only: bool,
    ) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("registry client setup failed: {e}"))?;
        Ok(Self {
            http,
            registry: registry.trim_end_matches('/').to_owned(),
            root,
            platform,
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
        let dir = self.root.join(self.platform?.install_name());
        let program = dir.join(codex::BINARY);
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
        if self.platform.is_none() {
            return Err(unsupported());
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
            .map_err(|e| UpdateError(format!("the Codex CLI install stopped: {e}")))?
    }

    /// Looks once per process for earlier releases to remove (one still running at the last
    /// install stays until then).
    fn prune_once(self: &Arc<Self>) {
        let Some(platform) = self.platform else {
            return;
        };
        if self.pruned.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move { me.remove_others(platform).await });
    }

    async fn ensure_now(&self) -> Result<ProvidedCli, UpdateError> {
        let Some(platform) = self.platform else {
            return Err(unsupported());
        };
        let _one = self.installing.lock().await;
        // Another caller may have installed it while this one waited, or just failed to.
        if let Some(provided) = self.ready()? {
            return Ok(provided);
        }
        let result = self.install_now(platform).await;
        if let Ok(mut failed) = self.failed.lock() {
            *failed = result
                .as_ref()
                .err()
                .map(|why| (std::time::Instant::now(), why.clone()));
        }
        result
    }

    async fn install_now(&self, platform: Platform) -> Result<ProvidedCli, UpdateError> {
        self.remove_temporary().await;
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| UpdateError(format!("cannot create {}: {e}", self.root.display())))?;
        set_mode(&self.root, 0o755).await?;
        let staging = self.root.join(format!("{STAGING}{}", unique_suffix()));
        let installed = self.install(platform, &staging).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        installed?;
        self.remove_others(platform).await;
        self.pruned.store(true, std::sync::atomic::Ordering::SeqCst);
        self.installed()
            .ok_or_else(|| UpdateError("the installed Codex CLI is not in place".into()))
    }

    async fn install(&self, platform: Platform, staging: &Path) -> Result<(), UpdateError> {
        let expected = sha512_of(platform.integrity)?;
        let release = staging.join("release");
        tokio::fs::create_dir_all(&release)
            .await
            .map_err(|e| UpdateError(format!("cannot create {}: {e}", staging.display())))?;
        set_mode(&release, 0o755).await?;
        let tarball = staging.join("download.tgz");
        let url = format!("{}{}", self.registry, platform.tarball_path());
        let digest = self.download(&url, &tarball).await?;
        if digest != expected {
            return fail("the Codex CLI tarball's sha512 is not the pinned one");
        }
        let (from, to, max) = (tarball.clone(), release.clone(), self.max_unpacked);
        tokio::task::spawn_blocking(move || unpack(&from, &to, &platform, max))
            .await
            .map_err(|e| UpdateError(format!("unpacking stopped: {e}")))??;
        let _ = tokio::fs::remove_file(&tarball).await;
        let target = self.root.join(platform.install_name());
        if tokio::fs::symlink_metadata(&target).await.is_ok() {
            tokio::fs::remove_dir_all(&target)
                .await
                .map_err(|e| UpdateError(format!("cannot replace a partial release: {e}")))?;
        }
        tokio::fs::rename(&release, &target)
            .await
            .map_err(|e| UpdateError(format!("cannot move the release in place: {e}")))?;
        if let Ok(dir) = tokio::fs::File::open(&self.root).await {
            let _ = dir.sync_all().await;
        }
        tracing::info!(version = codex::VERSION, dir = %target.display(), "Codex CLI installed");
        Ok(())
    }

    /// Streams the tarball to `to`, bounded; its sha512.
    async fn download(&self, url: &str, to: &Path) -> Result<Vec<u8>, UpdateError> {
        let mut response = self
            .http
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|_| UpdateError(format!("{REGISTRY_HOST} is unreachable")))?;
        if !response.status().is_success() {
            return fail(format!(
                "the Codex CLI download answered HTTP {}",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.max_tarball)
        {
            return fail("the Codex CLI tarball is larger than allowed");
        }
        let write = |e: std::io::Error| UpdateError(format!("cannot write the tarball: {e}"));
        let mut file = tokio::fs::File::create(to).await.map_err(write)?;
        let mut hasher = Sha512::new();
        let mut size = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| UpdateError("the Codex CLI download stopped".into()))?
        {
            size += chunk.len() as u64;
            if size > self.max_tarball {
                return fail("the Codex CLI tarball is larger than allowed");
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(write)?;
        }
        file.flush().await.map_err(write)?;
        Ok(hasher.finalize().to_vec())
    }

    /// Removes releases other than `keep`'s that no process runs (an earlier pin).
    async fn remove_others(&self, keep: Platform) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return;
        };
        let mut others = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name != keep.install_name() && !name.starts_with('.') {
                others.push(entry.path());
            }
        }
        let programs: Vec<PathBuf> = others.iter().map(|d| d.join(codex::BINARY)).collect();
        let running = tokio::task::spawn_blocking(move || {
            let paths: Vec<&Path> = programs.iter().map(PathBuf::as_path).collect();
            running_executables(&paths)
        })
        .await
        .unwrap_or_default();
        for (index, dir) in others.iter().enumerate() {
            if running.contains(&index) {
                tracing::info!(dir = %dir.display(), "an earlier Codex CLI still runs: kept for now");
                continue;
            }
            if let Err(error) = tokio::fs::remove_dir_all(dir).await {
                tracing::warn!(dir = %dir.display(), %error, "cannot remove an earlier Codex CLI");
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

/// Why this host gets no provided Codex CLI.
fn unsupported() -> UpdateError {
    UpdateError(format!(
        "Yulius provides no Codex CLI for {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

/// Sets `path`'s permission bits.
async fn set_mode(path: &Path, mode: u32) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .map_err(|e| UpdateError(format!("cannot set the mode of {}: {e}", path.display())))
}

/// Unpacks the tarball's `vendor/<triple>/` tree into `to`: regular files (0755 when executable,
/// else 0644) in directories of 0755, every path inside `to`, at most `max` bytes in all; and
/// checks its `bin/codex` is a 64-bit little-endian ELF for the platform's machine.
fn unpack(tarball: &Path, to: &Path, platform: &Platform, max: u64) -> Result<(), UpdateError> {
    use std::io::{Read as _, Write as _};
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    let io = |e: std::io::Error| UpdateError(format!("cannot read the tarball: {e}"));
    let write = |e: std::io::Error| UpdateError(format!("cannot unpack the Codex CLI: {e}"));
    let prefix = platform.vendor_dir();
    let file = std::fs::File::open(tarball).map_err(io)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut total = 0u64;
    for entry in archive.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        let path = entry.path().map_err(io)?.into_owned();
        let Some(relative) = path
            .to_str()
            .and_then(|p| p.strip_prefix(prefix.as_str()))
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
            return fail("the Codex CLI tarball names a path outside its release");
        }
        if kind != tar::EntryType::Regular {
            return fail("the Codex CLI tarball holds an entry that is not a regular file");
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
                return fail("the Codex CLI is larger than allowed");
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
    let program = to.join(codex::BINARY);
    let mut header = [0u8; 20];
    let read = std::fs::symlink_metadata(&program)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::File::open(&program).ok())
        .is_some_and(|mut f| f.read_exact(&mut header).is_ok());
    if !read {
        return fail(format!("the Codex CLI tarball holds no {}", codex::BINARY));
    }
    let elf = header[..4] == *b"\x7fELF"
        && header[4] == 2
        && header[5] == 1
        && u16::from_le_bytes([header[18], header[19]]) == platform.machine;
    if !elf {
        return fail(format!(
            "the Codex CLI is not a 64-bit ELF for machine {}",
            platform.machine
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
