//! Claude Code kept current in the data directory (SPEC-421, ADR-0266).
//!
//! **Check.** npm's dist-tag of the channel (`stable` or `latest`) is compared with the version
//! runs get: the newer of the host's npm install and Yulius's `current`.
//!
//! **Install.** A newer version's platform package is downloaded from the registry (the tarball
//! must be on it), its sha512 checked against the registry's `integrity`, and only its native
//! binary `package/claude` taken into a temporary directory, where `--version` must name the
//! version. The binary then moves to `<data>/claude-code/versions/<v>/bin/claude.exe` and
//! `current` is pointed at it by an atomic rename; the version before it stays, older ones go.
//! The host's install is never written, and nothing runs `npm` or a package script.
//!
//! **Use.** [`agent_cli_source`] says which directory a run binds at the host package's path:
//! chosen when the run starts, so a running process keeps its binary and the next start gets
//! the new one. Subscriptions are untouched (their tokens reach runs as
//! `CLAUDE_CODE_OAUTH_TOKEN`), so nobody signs in again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use yulius_app::ports::session::Bind;
use yulius_domain::agent_cli::{
    self as policy, Channel, PACKAGE, REGISTRY_HOST, Version, platform_package, should_install,
};

/// npm's registry.
pub const REGISTRY: &str = "https://registry.npmjs.org";
/// The largest tarball downloaded (a platform package is about 80 MB packed).
pub const MAX_TARBALL: u64 = 512 * 1024 * 1024;
/// The largest binary taken from it (about 300 MB unpacked).
pub const MAX_BINARY: u64 = 1024 * 1024 * 1024;
/// The binary's path inside a version's directory, as npm's wrapper package names it.
pub const BINARY: &str = "bin/claude.exe";
/// Deadline of a registry document.
const DOCUMENT_TIMEOUT: Duration = Duration::from_secs(15);
/// Deadline of a tarball download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Largest registry document read.
const MAX_DOCUMENT: usize = 1024 * 1024;
/// Deadline of the new binary's `--version`.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
/// Most of `--version`'s output read.
const MAX_VERSION_OUTPUT: u64 = 4096;
/// Names of temporary entries in the updater's directory, removed by the next check.
const TEMPORARY: [&str; 2] = [".staging-", ".current-"];

/// What a check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The channel is `off`.
    Off,
    /// No platform package for this host.
    Unsupported,
    /// No npm install of Claude Code on the host to bind over.
    NoHostInstall,
    /// Runs already get this version (or a newer one).
    UpToDate(Version),
    /// This version was installed and is now current.
    Installed(Version),
}

/// Why a check installed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateError(String);

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpdateError {}

fn fail<T>(why: impl Into<String>) -> Result<T, UpdateError> {
    Err(UpdateError(why.into()))
}

/// Checks the registry and installs newer Claude Code releases under `root`.
pub struct ClaudeUpdater {
    http: reqwest::Client,
    registry: String,
    root: PathBuf,
    host_dir: Option<PathBuf>,
    platform: Option<String>,
    max_tarball: u64,
}

impl ClaudeUpdater {
    /// The updater of `<data>/claude-code` over npm's registry, for this host's platform.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn new(root: PathBuf, host_dir: Option<PathBuf>) -> Result<Self, String> {
        let platform = platform_package(
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(target_env = "musl"),
        );
        Self::with_registry(root, host_dir, REGISTRY.to_owned(), platform, true)
    }

    /// The same over another registry (tests).
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn with_registry(
        root: PathBuf,
        host_dir: Option<PathBuf>,
        registry: String,
        platform: Option<String>,
        https_only: bool,
    ) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(DOCUMENT_TIMEOUT)
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("registry client setup failed: {e}"))?;
        Ok(Self {
            http,
            registry: registry.trim_end_matches('/').to_owned(),
            root,
            host_dir,
            platform,
            max_tarball: MAX_TARBALL,
        })
    }

    /// A smaller download bound (tests).
    #[must_use]
    pub fn with_max_tarball(mut self, bytes: u64) -> Self {
        self.max_tarball = bytes;
        self
    }

    /// The registry's base URL.
    #[must_use]
    pub fn registry(&self) -> &str {
        &self.registry
    }

    /// Yulius's current version, when one is installed.
    #[must_use]
    pub fn current(&self) -> Option<Version> {
        current_version(&self.root)
    }

    /// One check of `channel`: installs a newer release and makes it current.
    ///
    /// # Errors
    /// The registry could not be read or the release was refused; the current version is left
    /// as it was.
    pub async fn check(&self, channel: Channel) -> Result<Outcome, UpdateError> {
        let Some(tag) = channel.dist_tag() else {
            return Ok(Outcome::Off);
        };
        let Some(platform) = self.platform.as_deref() else {
            return Ok(Outcome::Unsupported);
        };
        let Some(host_dir) = self.host_dir.as_deref() else {
            return Ok(Outcome::NoHostInstall);
        };
        self.remove_temporary().await;
        let tags = self
            .document(&format!("/-/package/{}/dist-tags", encoded(PACKAGE)))
            .await?;
        let Some(offered) = tags
            .get(tag)
            .and_then(Value::as_str)
            .and_then(Version::parse)
        else {
            return fail(format!("the registry names no plain version as `{tag}`"));
        };
        let in_use = host_version(host_dir).max(self.current());
        if let Some(in_use) = in_use
            && !should_install(offered, Some(in_use))
        {
            return Ok(Outcome::UpToDate(in_use));
        }
        let staging = self
            .root
            .join(format!("{}{offered}-{}", TEMPORARY[0], unique_suffix()));
        let installed = self.install(platform, offered, &staging).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        installed?;
        self.remove_old(offered).await;
        Ok(Outcome::Installed(offered))
    }

    async fn install(
        &self,
        platform: &str,
        version: Version,
        staging: &Path,
    ) -> Result<(), UpdateError> {
        let document = self
            .document(&format!("/{}/{version}", encoded(platform)))
            .await?;
        if document.get("version").and_then(Value::as_str) != Some(&version.to_string()) {
            return fail(format!(
                "the registry's document of {platform} is not {version}"
            ));
        }
        let dist = document.get("dist");
        let field = |name: &str| dist.and_then(|d| d.get(name)).and_then(Value::as_str);
        let (Some(url), Some(integrity)) = (field("tarball"), field("integrity")) else {
            return fail("the registry gives no tarball or integrity");
        };
        if !url.starts_with(&format!("{}/", self.registry)) {
            return fail(format!("the tarball is not on {REGISTRY_HOST}"));
        }
        let expected = sha512_of(integrity)?;
        let package = staging.join("package");
        tokio::fs::create_dir_all(package.join("bin"))
            .await
            .map_err(|e| UpdateError(format!("cannot create {}: {e}", staging.display())))?;
        let tarball = staging.join("download.tgz");
        let digest = self.download(url, &tarball).await?;
        if digest != expected {
            return fail("the tarball's sha512 is not the registry's integrity");
        }
        let binary = package.join(BINARY);
        let (from, to) = (tarball.clone(), binary.clone());
        tokio::task::spawn_blocking(move || extract_binary(&from, &to))
            .await
            .map_err(|e| UpdateError(format!("extraction stopped: {e}")))??;
        let _ = tokio::fs::remove_file(&tarball).await;
        check_version(&binary, version, &staging.join("home")).await?;
        self.switch(&package, version).await
    }

    /// A registry document (JSON), bounded.
    async fn document(&self, path: &str) -> Result<Value, UpdateError> {
        let mut response = self
            .http
            .get(format!("{}{path}", self.registry))
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(DOCUMENT_TIMEOUT)
            .send()
            .await
            .map_err(|_| UpdateError(format!("{REGISTRY_HOST} is unreachable")))?;
        if !response.status().is_success() {
            return fail(format!(
                "{REGISTRY_HOST} answered HTTP {}",
                response.status().as_u16()
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| UpdateError(format!("{REGISTRY_HOST} stopped answering")))?
        {
            if body.len() + chunk.len() > MAX_DOCUMENT {
                return fail("the registry's document is too long");
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body)
            .map_err(|_| UpdateError("the registry's document is not JSON".into()))
    }

    /// Streams the tarball to `to`, bounded; its sha512.
    async fn download(&self, url: &str, to: &Path) -> Result<Vec<u8>, UpdateError> {
        let mut response = self
            .http
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|_| UpdateError("the tarball could not be downloaded".into()))?;
        if !response.status().is_success() {
            return fail(format!(
                "the tarball's download answered HTTP {}",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.max_tarball)
        {
            return fail("the tarball is larger than allowed");
        }
        let mut file = tokio::fs::File::create(to)
            .await
            .map_err(|e| UpdateError(format!("cannot write the tarball: {e}")))?;
        let mut hasher = Sha512::new();
        let mut size = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| UpdateError("the tarball's download stopped".into()))?
        {
            size += chunk.len() as u64;
            if size > self.max_tarball {
                return fail("the tarball is larger than allowed");
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| UpdateError(format!("cannot write the tarball: {e}")))?;
        }
        file.flush()
            .await
            .map_err(|e| UpdateError(format!("cannot write the tarball: {e}")))?;
        Ok(hasher.finalize().to_vec())
    }

    /// Moves the verified package to `versions/<v>` and points `current` at it atomically.
    async fn switch(&self, package: &Path, version: Version) -> Result<(), UpdateError> {
        let versions = self.root.join("versions");
        let target = versions.join(version.to_string());
        let io = |what: &str, e: std::io::Error| UpdateError(format!("cannot {what}: {e}"));
        tokio::fs::create_dir_all(&versions)
            .await
            .map_err(|e| io("create the versions directory", e))?;
        if tokio::fs::symlink_metadata(&target).await.is_ok() {
            tokio::fs::remove_dir_all(&target)
                .await
                .map_err(|e| io("replace an earlier copy of the version", e))?;
        }
        tokio::fs::rename(package, &target)
            .await
            .map_err(|e| io("move the version in place", e))?;
        let link = self
            .root
            .join(format!("{}{}", TEMPORARY[1], unique_suffix()));
        tokio::fs::symlink(Path::new("versions").join(version.to_string()), &link)
            .await
            .map_err(|e| io("link the version", e))?;
        if let Err(e) = tokio::fs::rename(&link, self.root.join("current")).await {
            let _ = tokio::fs::remove_file(&link).await;
            return Err(io("make the version current", e));
        }
        Ok(())
    }

    /// Removes versions older than the one before `current` (CCU-004).
    async fn remove_old(&self, current: Version) {
        let versions = self.root.join("versions");
        let Ok(mut entries) = tokio::fs::read_dir(&versions).await else {
            return;
        };
        let mut installed = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(version) = entry.file_name().to_str().and_then(Version::parse) {
                installed.push(version);
            }
        }
        for old in policy::to_remove(&installed, current) {
            if let Err(error) = tokio::fs::remove_dir_all(versions.join(old.to_string())).await {
                tracing::warn!(%old, %error, "cannot remove an old Claude Code version");
            }
        }
    }

    /// Removes what an interrupted install left.
    async fn remove_temporary(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !TEMPORARY.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            let path = entry.path();
            let removed = match tokio::fs::symlink_metadata(&path).await {
                Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(&path).await,
                _ => tokio::fs::remove_file(&path).await,
            };
            if let Err(error) = removed {
                tracing::warn!(path = %path.display(), %error, "cannot remove a leftover of a Claude Code install");
            }
        }
    }
}

/// The first check after a start.
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(120);
/// Checks after it.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);

/// Checks `channel` [`FIRST_CHECK_AFTER`] a start and every [`CHECK_EVERY`] after; `None`
/// (no loop) when the channel is `off`.
#[must_use]
pub fn start_updates(
    updater: ClaudeUpdater,
    channel: Channel,
) -> Option<tokio::task::JoinHandle<()>> {
    start_updates_every(updater, channel, FIRST_CHECK_AFTER, CHECK_EVERY)
}

/// [`start_updates`] with other periods (tests).
#[must_use]
pub fn start_updates_every(
    updater: ClaudeUpdater,
    channel: Channel,
    first: Duration,
    every: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    if channel == Channel::Off {
        tracing::info!("Claude Code updates are off: runs use the host's Claude Code");
        return None;
    }
    Some(tokio::spawn(async move {
        tokio::time::sleep(first).await;
        loop {
            // Each request, the download and `--version` have their own deadline.
            match updater.check(channel).await {
                Ok(Outcome::Installed(version)) => {
                    tracing::info!(%version, "Claude Code updated: new runs use it");
                }
                Ok(outcome) => tracing::debug!(?outcome, "Claude Code update check"),
                Err(error) => tracing::warn!(%error, "Claude Code was not updated"),
            }
            tokio::time::sleep(every).await;
        }
    }))
}

/// The read-only bind of the agent CLI in a run: at the host package's path `host_dir`, from
/// [`agent_cli_source`] under `root` when updates are on, else from the host's package; none
/// without a host package (CCU-005).
#[must_use]
pub fn agent_cli_binds(host_dir: Option<&Path>, root: Option<&Path>) -> Vec<Bind> {
    let Some(host_dir) = host_dir else {
        return Vec::new();
    };
    let source = root.map_or_else(
        || host_dir.to_path_buf(),
        |root| agent_cli_source(root, host_dir),
    );
    vec![Bind {
        source,
        dest: host_dir.to_path_buf(),
        writable: false,
    }]
}

/// The directory a run binds at the host package's path `host_dir`: Yulius's current version
/// when it is newer than the host's, else the host's own (CCU-003, CCU-005).
#[must_use]
pub fn agent_cli_source(root: &Path, host_dir: &Path) -> PathBuf {
    let Some(ours) = current_version(root) else {
        return host_dir.to_path_buf();
    };
    if host_version(host_dir).is_some_and(|host| host >= ours) {
        return host_dir.to_path_buf();
    }
    root.join("versions")
        .join(ours.to_string())
        .canonicalize()
        .unwrap_or_else(|_| host_dir.to_path_buf())
}

/// The version `current` points at, when its binary is there.
fn current_version(root: &Path) -> Option<Version> {
    let link = std::fs::read_link(root.join("current")).ok()?;
    let version = Version::parse(link.file_name()?.to_str()?)?;
    root.join("versions")
        .join(version.to_string())
        .join(BINARY)
        .is_file()
        .then_some(version)
}

/// The version in the host package's `package.json`.
#[must_use]
pub fn host_version(host_dir: &Path) -> Option<Version> {
    let text = std::fs::read(host_dir.join("package.json")).ok()?;
    let manifest: Value = serde_json::from_slice(&text).ok()?;
    Version::parse(manifest.get("version")?.as_str()?)
}

/// `@scope/name` as one path segment.
fn encoded(package: &str) -> String {
    package.replace('/', "%2F")
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// The digest of an npm `sha512-<base64>` integrity.
fn sha512_of(integrity: &str) -> Result<Vec<u8>, UpdateError> {
    integrity
        .split_whitespace()
        .find_map(|hash| hash.strip_prefix("sha512-"))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .filter(|digest| digest.len() == 64)
        .ok_or_else(|| UpdateError("the registry gives no sha512 integrity".into()))
}

/// Takes the regular file `package/claude` from the gzip tarball into `to` (mode 0755), bounded.
fn extract_binary(tarball: &Path, to: &Path) -> Result<(), UpdateError> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let io = |e: std::io::Error| UpdateError(format!("cannot read the tarball: {e}"));
    let file = std::fs::File::open(tarball).map_err(io)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in archive.entries().map_err(io)? {
        let entry = entry.map_err(io)?;
        if entry.path().map_err(io)?.as_ref() != Path::new("package/claude") {
            continue;
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            return fail("the tarball's `claude` is not a regular file");
        }
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(to)
            .map_err(|e| UpdateError(format!("cannot write the binary: {e}")))?;
        let copied = std::io::copy(&mut entry.take(MAX_BINARY + 1), &mut out)
            .map_err(|e| UpdateError(format!("cannot write the binary: {e}")))?;
        if copied > MAX_BINARY {
            return fail("the binary is larger than allowed");
        }
        return out
            .sync_all()
            .map_err(|e| UpdateError(format!("cannot write the binary: {e}")));
    }
    fail("the tarball holds no `package/claude`")
}

/// Runs `binary --version` in a clean environment; it must name `version`.
async fn check_version(binary: &Path, version: Version, home: &Path) -> Result<(), UpdateError> {
    tokio::fs::create_dir_all(home)
        .await
        .map_err(|e| UpdateError(format!("cannot create {}: {e}", home.display())))?;
    let mut child = tokio::process::Command::new(binary)
        .arg("--version")
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("DISABLE_AUTOUPDATER", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| UpdateError(format!("the new binary does not start: {e}")))?;
    let stdout = child.stdout.take();
    let run = async {
        let mut output = Vec::new();
        if let Some(stdout) = stdout {
            let _ = stdout
                .take(MAX_VERSION_OUTPUT)
                .read_to_end(&mut output)
                .await;
        }
        (child.wait().await, output)
    };
    let Ok((status, output)) = tokio::time::timeout(VERSION_TIMEOUT, run).await else {
        // Killed and reaped before the version counts as failed.
        let _ = child.kill().await;
        return fail("the new binary's --version did not finish");
    };
    let printed = String::from_utf8_lossy(&output);
    let named = printed.split_whitespace().next() == Some(&version.to_string());
    if !status.is_ok_and(|s| s.success()) || !named {
        return fail(format!(
            "the new binary's --version does not name {version}: {:?}",
            printed.trim().chars().take(80).collect::<String>()
        ));
    }
    Ok(())
}
