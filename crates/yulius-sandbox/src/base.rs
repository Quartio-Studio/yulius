//! The base of every sandbox's system layer (SPEC-002 ADR-22 §3): a copy of the host system
//! owned by ids that exist in a sandbox's user namespace.
//!
//! `system/base-<n>/` holds `/usr`, `/etc`, `/opt` and dpkg/apt/debconf state from `/var`
//! (an allow-list; `/var/lib/yulius` and the rest of `/var` are never read). The build:
//! 1. copies as the service user, so every file becomes root's inside, skipping what it cannot
//!    read (shadow, sudoers, private keys) and recording the files host users other than root
//!    own;
//! 2. writes the sandbox-specific files ([`generate`]);
//! 3. inside a mapped user namespace, gives the recorded files their owners (`yulius
//!    sandbox-base chown`, [`apply_owners`]);
//! 4. renames the directory into place and points `system/current.json` at it.
//!
//! A build is redone when the host's system changes ([`fingerprint`]). Bases no live holder uses
//! are removed ([`collect`]), whoever runs the build.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::holder::{read_json, write_json};
use crate::userns::Mapper;

/// Format of the base; a change rebuilds every base.
pub const BASE_FORMAT: u32 = 2;
/// The system directories a run binds from the system layer.
pub const SYSTEM_DIRS: [&str; 9] = [
    "usr", "etc", "var", "opt", "root", "home", "srv", "mnt", "media",
];
/// Host directories copied whole.
const COPIED: [&str; 3] = ["usr", "etc", "opt"];
/// Parts of `/var` copied (package manager state).
const VAR_COPIED: [&str; 12] = [
    "lib/dpkg",
    "lib/apt",
    "lib/ucf",
    "lib/pam",
    "lib/systemd/deb-systemd-helper-enabled",
    "lib/systemd/deb-systemd-helper-masked",
    "lib/python",
    "lib/sgml-base",
    "lib/xml-core",
    "lib/misc",
    "cache/debconf",
    "cache/ldconfig",
];
/// Paths inside `/var` that must not be copied even below an allowed one.
const VAR_SKIPPED: [&str; 3] = [
    "lib/apt/lists/partial",
    "lib/apt/lists/lock",
    "lib/dpkg/lock",
];
/// Empty directories every base has (mode).
const EMPTY_DIRS: [(&str, u32); 16] = [
    ("root", 0o700),
    ("home", 0o755),
    ("srv", 0o755),
    ("mnt", 0o755),
    ("media", 0o755),
    ("tmp", 0o1777),
    ("run", 0o755),
    ("var/tmp", 0o1777),
    ("var/log", 0o755),
    ("var/log/apt", 0o755),
    ("var/mail", 0o2775),
    ("var/spool", 0o755),
    ("var/opt", 0o755),
    ("var/local", 0o755),
    ("var/backups", 0o755),
    ("var/cache/apt/archives", 0o755),
];
/// Builds may take this long in the namespace step.
const CHOWN_LIMIT: Duration = Duration::from_secs(30 * 60);
/// The resolver inside a sandbox (`slirp4netns`'s DNS).
pub const SANDBOX_RESOLV_CONF: &str = "nameserver 10.0.2.3\n";
/// No service starts from package scripts.
const POLICY_RC_D: &str = "#!/bin/sh\n# Yulius sandbox: services are started by hand.\nexit 101\n";

/// What `system/current.json` says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseInfo {
    /// `base-<n>`.
    pub id: String,
    /// [`fingerprint`] of the host when it was built.
    pub fingerprint: String,
    /// When it was finished (ms since the epoch).
    pub built_at_ms: i64,
    /// [`BASE_FORMAT`].
    pub format: u32,
}

/// One file whose owner differs from root inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerEntry {
    /// Path relative to the base.
    pub path: String,
    /// Owner inside the namespace.
    pub uid: u32,
    /// Group inside the namespace.
    pub gid: u32,
    /// Mode to restore after `chown` (it clears set-id bits); `None` for links.
    pub mode: Option<u32>,
}

/// Paths of the bases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bases {
    /// `<data>/system`.
    pub dir: PathBuf,
}

impl Bases {
    /// Under `<data>/system`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("system"),
        }
    }

    /// The current base, when one was built in this format.
    #[must_use]
    pub fn current(&self) -> Option<BaseInfo> {
        let info: BaseInfo = read_json(&self.dir.join("current.json")).ok()?;
        (info.format == BASE_FORMAT && self.path(&info.id).is_dir()).then_some(info)
    }

    /// The directory of base `id`.
    #[must_use]
    pub fn path(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    fn next_id(&self) -> String {
        let highest = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("base-"))
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0);
        format!("base-{}", highest + 1)
    }
}

/// What a build needs.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    /// Root of the host system to copy (`/`; a fixture in tests).
    pub host_root: PathBuf,
    /// The data directory (never copied).
    pub data_dir: PathBuf,
    /// Content of the `sudo` shim (`/usr/bin/sudo` in the base).
    pub sudo_shim: String,
    /// The namespace for the ownership step.
    pub mapper: Mapper,
    /// The `yulius` executable (`sandbox-base chown`).
    pub exe: PathBuf,
}

/// A fingerprint of the host system: dpkg's status, `/usr/local` and `/opt` entries (except
/// `yulius*`, which is bound live), the shim and [`BASE_FORMAT`].
#[must_use]
pub fn fingerprint(host_root: &Path, sudo_shim: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("format {BASE_FORMAT}\n"));
    hasher.update(sudo_shim.as_bytes());
    let stamp = |path: &Path| {
        std::fs::symlink_metadata(path)
            .map(|m| format!("{} {} {}", m.size(), m.mtime(), m.mtime_nsec()))
            .unwrap_or_else(|_| "-".into())
    };
    hasher.update(format!(
        "dpkg {}\n",
        stamp(&host_root.join("var/lib/dpkg/status"))
    ));
    let mut listed = Vec::new();
    for dir in ["usr/local/bin", "usr/local/lib/node_modules", "opt"] {
        let Ok(entries) = std::fs::read_dir(host_root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("yulius") {
                continue;
            }
            listed.push(format!("{dir}/{name} {}", stamp(&entry.path())));
            if name.starts_with('@')
                && let Ok(scoped) = std::fs::read_dir(entry.path())
            {
                for inner in scoped.flatten() {
                    listed.push(format!(
                        "{dir}/{name}/{} {}",
                        inner.file_name().to_string_lossy(),
                        stamp(&inner.path())
                    ));
                }
            }
        }
    }
    listed.sort();
    for line in listed {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Counters of a copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    /// Files, links and directories copied.
    pub copied: u64,
    /// Entries skipped (unreadable, special files).
    pub skipped: u64,
}

struct Copier<'a> {
    owners: Vec<OwnerEntry>,
    links: HashMap<(u64, u64), PathBuf>,
    stats: CopyStats,
    data_dir: &'a Path,
    destination_root: &'a Path,
}

/// Host id → id inside (host ids above the mapped range become `nobody`).
fn inside(id: u32) -> u32 {
    if id <= 65_535 { id } else { 65_534 }
}

impl Copier<'_> {
    fn relative(&self, destination: &Path) -> String {
        destination
            .strip_prefix(self.destination_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn note_owner(&mut self, destination: &Path, meta: &std::fs::Metadata, link: bool) {
        let (uid, gid) = (inside(meta.uid()), inside(meta.gid()));
        if uid == 0 && gid == 0 {
            return;
        }
        let path = self.relative(destination);
        if path.contains('\n') {
            return;
        }
        self.owners.push(OwnerEntry {
            path,
            uid,
            gid,
            mode: (!link).then(|| meta.mode() & 0o7777),
        });
    }

    fn copy(&mut self, source: &Path, destination: &Path) {
        if source.starts_with(self.data_dir) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(source) else {
            self.stats.skipped += 1;
            return;
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            match std::fs::read_link(source)
                .and_then(|target| std::os::unix::fs::symlink(target, destination))
            {
                Ok(()) => {
                    self.stats.copied += 1;
                    self.note_owner(destination, &meta, true);
                }
                Err(_) => self.stats.skipped += 1,
            }
        } else if kind.is_dir() {
            let Ok(entries) = std::fs::read_dir(source) else {
                self.stats.skipped += 1;
                return;
            };
            if std::fs::create_dir(destination).is_err() {
                self.stats.skipped += 1;
                return;
            }
            // Writable for us while filling it; the mode is restored below.
            let _ = std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755));
            let mut names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
            names.sort();
            for name in names {
                self.copy(&source.join(&name), &destination.join(&name));
            }
            let _ = std::fs::set_permissions(
                destination,
                std::fs::Permissions::from_mode((meta.mode() & 0o7777) | 0o200),
            );
            self.stats.copied += 1;
            self.note_owner(destination, &meta, false);
        } else if kind.is_file() {
            let key = (meta.dev(), meta.ino());
            if meta.nlink() > 1
                && let Some(first) = self.links.get(&key)
                && std::fs::hard_link(first, destination).is_ok()
            {
                self.stats.copied += 1;
                return;
            }
            if std::fs::copy(source, destination).is_err() {
                let _ = std::fs::remove_file(destination);
                self.stats.skipped += 1;
                return;
            }
            let _ = std::fs::set_permissions(
                destination,
                std::fs::Permissions::from_mode(meta.mode() & 0o7777),
            );
            if let (Ok(file), Ok(modified)) = (
                std::fs::OpenOptions::new().write(true).open(destination),
                meta.modified(),
            ) {
                let _ = file.set_modified(modified);
            }
            if meta.nlink() > 1 {
                self.links.insert(key, destination.to_path_buf());
            }
            self.stats.copied += 1;
            self.note_owner(destination, &meta, false);
        } else {
            // Sockets, FIFOs and device nodes do not belong in a base.
            self.stats.skipped += 1;
        }
    }
}

/// Copies the host system under `host_root` into `destination` (see the module docs) and
/// returns the owners to restore.
///
/// # Errors
/// The destination cannot be created.
pub fn copy_host(
    host_root: &Path,
    data_dir: &Path,
    destination: &Path,
) -> io::Result<(Vec<OwnerEntry>, CopyStats)> {
    std::fs::create_dir_all(destination)?;
    std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755))?;
    let data_dir = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let mut copier = Copier {
        owners: Vec::new(),
        links: HashMap::new(),
        stats: CopyStats::default(),
        data_dir: &data_dir,
        destination_root: destination,
    };
    for dir in COPIED {
        copier.copy(&host_root.join(dir), &destination.join(dir));
    }
    for part in VAR_COPIED {
        let source = host_root.join("var").join(part);
        if !source.exists() {
            continue;
        }
        let target = destination.join("var").join(part);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copier.copy(&source, &target);
    }
    for skipped in VAR_SKIPPED {
        let path = destination.join("var").join(skipped);
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_file(&path);
    }
    Ok((copier.owners, copier.stats))
}

fn id_of(file: &str, name: &str) -> Option<u32> {
    file.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
}

fn replace_file(path: &Path, text: &str, mode: u32) -> io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// `shadow` (or `gshadow`) entries for every name of `passwd` (or `group`), with no password.
fn shadow_of(table: &str, group: bool) -> String {
    table
        .lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| !name.is_empty())
        .map(|name| {
            if group {
                format!("{name}:*::\n")
            } else {
                format!("{name}:*:19000:0:99999:7:::\n")
            }
        })
        .collect()
}

/// Writes the sandbox-specific files into the copied base at `root` and returns their owners.
///
/// # Errors
/// A file cannot be written.
pub fn generate(root: &Path, sudo_shim: &str) -> io::Result<Vec<OwnerEntry>> {
    let mut owners = Vec::new();
    for (dir, mode) in EMPTY_DIRS {
        let path = root.join(dir);
        std::fs::create_dir_all(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    for (link, target) in [("var/run", "/run"), ("var/lock", "/run/lock")] {
        let path = root.join(link);
        if std::fs::symlink_metadata(&path).is_err() {
            std::os::unix::fs::symlink(target, &path)?;
        }
    }
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
    let group = std::fs::read_to_string(root.join("etc/group")).unwrap_or_default();
    let shadow_gid = id_of(&group, "shadow").unwrap_or(0);
    let apt_uid = id_of(&passwd, "_apt").unwrap_or(42);
    replace_file(&root.join("etc/resolv.conf"), SANDBOX_RESOLV_CONF, 0o644)?;
    replace_file(&root.join("etc/hostname"), "sandbox\n", 0o644)?;
    replace_file(
        &root.join("etc/hosts"),
        &crate::holder::hosts_file("sandbox"),
        0o644,
    )?;
    replace_file(&root.join("usr/sbin/policy-rc.d"), POLICY_RC_D, 0o755)?;
    for (name, text) in [
        ("etc/shadow", shadow_of(&passwd, false)),
        ("etc/gshadow", shadow_of(&group, true)),
    ] {
        replace_file(&root.join(name), &text, 0o640)?;
        owners.push(OwnerEntry {
            path: name.into(),
            uid: 0,
            gid: shadow_gid,
            mode: Some(0o640),
        });
    }
    // sudo: the shim replaces the host's set-uid binary (mounts are nosuid anyway); a local
    // dpkg diversion keeps `apt upgrade` of `sudo` from overwriting it.
    let sudo = root.join("usr/bin/sudo");
    if sudo.exists() {
        let distrib = root.join("usr/bin/sudo.distrib");
        if !distrib.exists() {
            std::fs::rename(&sudo, &distrib)?;
        }
        let diversions = root.join("var/lib/dpkg/diversions");
        let current = std::fs::read_to_string(&diversions).unwrap_or_default();
        if !current.lines().any(|l| l == "/usr/bin/sudo") {
            let mut text = current;
            text.push_str("/usr/bin/sudo\n/usr/bin/sudo.distrib\n:\n");
            std::fs::write(&diversions, text)?;
        }
    }
    replace_file(&sudo, sudo_shim, 0o755)?;
    for dir in [
        "var/cache/apt/archives/partial",
        "var/lib/apt/lists/partial",
    ] {
        let path = root.join(dir);
        std::fs::create_dir_all(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        owners.push(OwnerEntry {
            path: dir.into(),
            uid: apt_uid,
            gid: 0,
            mode: Some(0o700),
        });
    }
    Ok(owners)
}

/// Gives every recorded file its owner (run as root of a mapped namespace) and restores its
/// mode. Returns the entries that failed.
#[must_use]
pub fn apply_owners(root: &Path, owners: &[OwnerEntry]) -> u64 {
    let mut failed = 0;
    for entry in owners {
        let path = root.join(&entry.path);
        if !path.starts_with(root) || entry.path.split('/').any(|c| c == "..") {
            failed += 1;
            continue;
        }
        let owned = std::os::unix::fs::lchown(&path, Some(entry.uid), Some(entry.gid));
        let moded = match entry.mode {
            Some(mode) => std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)),
            None => Ok(()),
        };
        if owned.is_err() || moded.is_err() {
            failed += 1;
        }
    }
    failed
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Builds a new base (see the module docs) and makes it current.
///
/// # Errors
/// A message for the operator or the server's log.
pub fn build(config: &BuildConfig, bases: &Bases) -> Result<BaseInfo, String> {
    std::fs::create_dir_all(&bases.dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&bases.dir, std::fs::Permissions::from_mode(0o700));
    for entry in std::fs::read_dir(&bases.dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".building-")
        {
            let _ = remove_tree(&config.mapper, &config.exe, &entry.path());
        }
    }
    let fingerprint = fingerprint(&config.host_root, &config.sudo_shim);
    let id = bases.next_id();
    let partial = bases.dir.join(format!(".building-{id}"));
    let (mut owners, stats) =
        copy_host(&config.host_root, &config.data_dir, &partial).map_err(|e| e.to_string())?;
    let generated = generate(&partial, &config.sudo_shim).map_err(|e| format!("generate: {e}"))?;
    owners.extend(generated);
    let manifest = bases.dir.join(format!(".owners-{id}.json"));
    write_json(&manifest, &owners).map_err(|e| e.to_string())?;
    let args: Vec<OsString> = [
        "--dev-bind",
        "/",
        "/",
        "--cap-add",
        "CAP_CHOWN",
        "--cap-add",
        "CAP_FOWNER",
        "--cap-add",
        "CAP_FSETID",
        "--cap-add",
        "CAP_DAC_OVERRIDE",
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([
        config.exe.as_os_str().to_owned(),
        "sandbox-base".into(),
        "chown".into(),
        "--root".into(),
        partial.as_os_str().to_owned(),
        "--manifest".into(),
        manifest.as_os_str().to_owned(),
    ])
    .collect();
    let owned = config.mapper.run(&args, CHOWN_LIMIT);
    let _ = std::fs::remove_file(&manifest);
    owned.map_err(|e| format!("owners: {e}"))?;
    let final_path = bases.path(&id);
    std::fs::rename(&partial, &final_path).map_err(|e| e.to_string())?;
    let info = BaseInfo {
        id,
        fingerprint,
        built_at_ms: now_ms(),
        format: BASE_FORMAT,
    };
    write_json(&bases.dir.join("current.json"), &info).map_err(|e| e.to_string())?;
    tracing::info!(
        base = %info.id,
        copied = stats.copied,
        skipped = stats.skipped,
        owners = owners.len(),
        "sandbox base built"
    );
    Ok(info)
}

/// Removes `path`; entries the service user cannot remove (owned by mapped ids) are removed as
/// root of a mapped namespace (`yulius sandbox-host remove-tree`).
///
/// # Errors
/// Both attempts failed.
pub fn remove_tree(mapper: &Mapper, exe: &Path, path: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    make_writable(path);
    if std::fs::remove_dir_all(path).is_ok() {
        return Ok(());
    }
    let args: Vec<OsString> = [
        "--dev-bind",
        "/",
        "/",
        "--cap-add",
        "CAP_DAC_OVERRIDE",
        "--cap-add",
        "CAP_FOWNER",
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([
        exe.as_os_str().to_owned(),
        "sandbox-host".into(),
        "remove-tree".into(),
        path.as_os_str().to_owned(),
    ])
    .collect();
    mapper
        .run(&args, CHOWN_LIMIT)
        .map_err(|e| format!("remove {}: {e}", path.display()))
}

/// Makes every directory under `path` that the caller owns writable and searchable (so its
/// entries can be removed).
pub fn make_writable(path: &Path) {
    let mut stack = vec![path.to_path_buf()];
    let mut visited = 0u64;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > 10_000_000 {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let _ = std::fs::set_permissions(
            &dir,
            std::fs::Permissions::from_mode((meta.mode() & 0o7777) | 0o700),
        );
        if let Ok(entries) = std::fs::read_dir(&dir) {
            stack.extend(entries.flatten().map(|e| e.path()));
        }
    }
}

/// Holder state directories read at most when looking for live holders (as many as adoption).
const MAX_HOLDERS: usize = 10_000;

/// The bases of the live holders recorded under `<data>/runtime/sandboxes/*/`, judged as the
/// server judges them ([`crate::holder::alive`]).
fn live_holder_bases(data_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(data_dir.join("runtime").join("sandboxes")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .take(MAX_HOLDERS)
        .filter_map(|entry| {
            read_json::<yulius_contracts::sandbox::HolderRecord>(
                &entry
                    .path()
                    .join(yulius_contracts::sandbox::HOLDER_RECORD_FILE),
            )
            .ok()
        })
        .filter(crate::holder::alive)
        .map(|record| record.base_id)
        .collect()
}

/// Removes bases that are neither current, in `in_use` nor mounted by a live holder (SPEC-202
/// RSB-002, ADR-0161): whoever builds, a hand-run `yulius sandbox-base build` without `--keep`
/// included, a running sandbox keeps its lower layer.
pub fn collect(bases: &Bases, in_use: &[String], mapper: &Mapper, exe: &Path) {
    let current = bases.current().map(|b| b.id);
    let live = bases
        .dir
        .parent()
        .map(live_holder_bases)
        .unwrap_or_default();
    for entry in std::fs::read_dir(&bases.dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("base-")
            || current.as_deref() == Some(name.as_str())
            || in_use.iter().chain(&live).any(|id| id == &name)
        {
            continue;
        }
        match remove_tree(mapper, exe, &entry.path()) {
            Ok(()) => tracing::info!(base = %name, "removed an unused sandbox base"),
            Err(error) => tracing::warn!(base = %name, %error, "cannot remove an unused base"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn host() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let host = tempfile::tempdir()?;
        let root = host.path();
        for dir in [
            "usr/bin",
            "usr/lib/x86_64-linux-gnu",
            "usr/local/bin",
            "etc/ssl/private",
            "opt/tool",
            "var/lib/dpkg/info",
            "var/lib/apt/lists/partial",
            "var/lib/yulius",
            "var/log",
            "var/cache/debconf",
        ] {
            std::fs::create_dir_all(root.join(dir))?;
        }
        std::fs::write(root.join("usr/bin/tool"), "#!/bin/sh\necho tool\n")?;
        std::fs::hard_link(root.join("usr/bin/tool"), root.join("usr/bin/tool2"))?;
        std::os::unix::fs::symlink("tool", root.join("usr/bin/link"))?;
        std::fs::write(root.join("usr/bin/sudo"), "real sudo")?;
        std::fs::write(
            root.join("etc/passwd"),
            "root:x:0:0::/root:/bin/sh\n_apt:x:42:65534::/nonexistent:/usr/sbin/nologin\n",
        )?;
        std::fs::write(root.join("etc/group"), "root:x:0:\nshadow:x:42:\n")?;
        std::fs::write(
            root.join("etc/nsswitch.conf"),
            "passwd:         files systemd\ngroup:          files systemd\nhosts:          files dns\n",
        )?;
        std::fs::write(root.join("etc/ssl/private/key.pem"), "secret")?;
        std::fs::set_permissions(
            root.join("etc/ssl/private/key.pem"),
            std::fs::Permissions::from_mode(0o000),
        )?;
        std::os::unix::fs::symlink(
            "/run/systemd/resolve/stub-resolv.conf",
            root.join("etc/resolv.conf"),
        )?;
        std::fs::write(root.join("var/lib/dpkg/status"), "Package: tool\n")?;
        std::fs::write(root.join("var/lib/yulius/master.key"), "never")?;
        std::fs::write(root.join("var/log/syslog"), "never")?;
        std::fs::write(root.join("opt/tool/run"), "x")?;
        Ok(host)
    }

    #[test]
    fn the_copy_keeps_the_system_and_leaves_out_private_data() -> TestResult {
        let host = host()?;
        let data = host.path().join("var/lib/yulius");
        let out = tempfile::tempdir()?;
        let base = out.path().join("base");
        let (owners, stats) = copy_host(host.path(), &data, &base)?;
        assert!(base.join("usr/bin/tool").is_file());
        assert_eq!(
            std::fs::metadata(base.join("usr/bin/tool"))?.ino(),
            std::fs::metadata(base.join("usr/bin/tool2"))?.ino(),
            "hard links stay links"
        );
        assert_eq!(
            std::fs::read_link(base.join("usr/bin/link"))?,
            PathBuf::from("tool")
        );
        assert!(base.join("opt/tool/run").is_file());
        assert!(base.join("var/lib/dpkg/status").is_file());
        assert!(
            !base.join("var/lib/yulius").exists(),
            "the data directory is never copied"
        );
        assert!(
            !base.join("var/log/syslog").exists(),
            "only allowed parts of /var"
        );
        assert!(!base.join("var/lib/apt/lists/partial").exists());
        // Unreadable files are skipped (unless the tests run as root).
        if !nix::unistd::Uid::effective().is_root() {
            assert!(!base.join("etc/ssl/private/key.pem").exists());
            assert!(stats.skipped >= 1);
        }
        // Everything here is owned by the test user: nothing to restore.
        assert!(owners.iter().all(|o| o.uid != 0 || o.gid != 0));
        Ok(())
    }

    #[test]
    fn generated_files_make_the_base_a_sandbox_system() -> TestResult {
        let host = host()?;
        let out = tempfile::tempdir()?;
        let base = out.path().join("base");
        copy_host(host.path(), &host.path().join("var/lib/yulius"), &base)?;
        let owners = generate(&base, "#!/bin/sh\nexec \"$@\"\n")?;
        assert_eq!(
            std::fs::read_to_string(base.join("etc/resolv.conf"))?,
            SANDBOX_RESOLV_CONF
        );
        assert_eq!(
            std::fs::read_to_string(base.join("usr/bin/sudo"))?,
            "#!/bin/sh\nexec \"$@\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(base.join("usr/bin/sudo.distrib"))?,
            "real sudo"
        );
        assert!(
            std::fs::read_to_string(base.join("var/lib/dpkg/diversions"))?
                .contains("/usr/bin/sudo\n/usr/bin/sudo.distrib\n:\n")
        );
        assert!(std::fs::read_to_string(base.join("etc/shadow"))?.contains("_apt:*:"));
        assert!(owners.contains(&OwnerEntry {
            path: "var/cache/apt/archives/partial".into(),
            uid: 42,
            gid: 0,
            mode: Some(0o700),
        }));
        assert!(owners.iter().any(|o| o.path == "etc/shadow" && o.gid == 42));
        assert_eq!(
            std::fs::metadata(base.join("var/tmp"))?.mode() & 0o7777,
            0o1777
        );
        // Generating twice keeps one diversion and the real sudo.
        generate(&base, "#!/bin/sh\nexec \"$@\"\n")?;
        assert_eq!(
            std::fs::read_to_string(base.join("var/lib/dpkg/diversions"))?
                .matches("/usr/bin/sudo.distrib")
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(base.join("usr/bin/sudo.distrib"))?,
            "real sudo"
        );
        Ok(())
    }

    #[test]
    fn the_fingerprint_follows_the_package_state_but_not_yulius() -> TestResult {
        let host = host()?;
        let first = fingerprint(host.path(), "shim");
        std::fs::write(host.path().join("usr/local/bin/yulius"), "new build")?;
        assert_eq!(
            fingerprint(host.path(), "shim"),
            first,
            "deploys do not rebuild"
        );
        std::fs::write(host.path().join("usr/local/bin/claude"), "cli")?;
        let second = fingerprint(host.path(), "shim");
        assert_ne!(second, first);
        std::fs::write(
            host.path().join("var/lib/dpkg/status"),
            "Package: tool\n\nPackage: more\n",
        )?;
        assert_ne!(fingerprint(host.path(), "shim"), second);
        assert_ne!(
            fingerprint(host.path(), "other shim"),
            fingerprint(host.path(), "shim")
        );
        Ok(())
    }

    #[test]
    fn owners_are_applied_inside_the_base_only() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("f"), "x")?;
        let me = nix::unistd::Uid::effective().as_raw();
        let group = nix::unistd::Gid::effective().as_raw();
        let entries = vec![
            OwnerEntry {
                path: "f".into(),
                uid: me,
                gid: group,
                mode: Some(0o640),
            },
            OwnerEntry {
                path: "../escape".into(),
                uid: me,
                gid: group,
                mode: None,
            },
        ];
        assert_eq!(apply_owners(dir.path(), &entries), 1);
        assert_eq!(
            std::fs::metadata(dir.path().join("f"))?.mode() & 0o777,
            0o640
        );
        Ok(())
    }

    #[test]
    fn bases_are_numbered_and_found() -> TestResult {
        let data = tempfile::tempdir()?;
        let bases = Bases::new(data.path());
        std::fs::create_dir_all(bases.path("base-3"))?;
        std::fs::create_dir_all(bases.path("base-1"))?;
        assert_eq!(bases.next_id(), "base-4");
        assert!(bases.current().is_none());
        write_json(
            &bases.dir.join("current.json"),
            &BaseInfo {
                id: "base-3".into(),
                fingerprint: "f".into(),
                built_at_ms: 1,
                format: BASE_FORMAT,
            },
        )?;
        assert_eq!(bases.current().map(|b| b.id).as_deref(), Some("base-3"));
        Ok(())
    }

    /// A holder record under `<data>/runtime/sandboxes/<sandbox>/` on `base_id`, alive when it
    /// names this test process's own boot and namespaces.
    fn holder_record(data: &Path, sandbox: &str, base_id: &str, live: bool) -> TestResult {
        let pid = std::process::id();
        let ns = |kind| crate::holder::ns_inode(pid, kind).unwrap_or(0);
        let record = yulius_contracts::sandbox::HolderRecord {
            version: yulius_contracts::sandbox::HOLDER_RECORD_VERSION,
            sandbox: sandbox.into(),
            base_id: base_id.into(),
            inner_pid: pid,
            bwrap_pid: pid,
            slirp_pid: None,
            user_ns: ns("user"),
            net_ns: ns("net"),
            mnt_ns: ns("mnt"),
            boot_id: if live {
                crate::holder::boot_id()
            } else {
                "an-earlier-boot".into()
            },
            started_at_ms: 1,
            system_root: "/run/yulius/system".into(),
            engine: false,
        };
        let dir = data.join("runtime").join("sandboxes").join(sandbox);
        std::fs::create_dir_all(&dir)?;
        write_json(
            &dir.join(yulius_contracts::sandbox::HOLDER_RECORD_FILE),
            &record,
        )?;
        Ok(())
    }

    /// SPEC-202 RSB-002 (#618): a build run by hand (no `--keep`) keeps the base a live holder
    /// has mounted, and still removes the bases nobody uses, a dead holder's included.
    #[test]
    fn collection_keeps_the_bases_of_live_holders_without_keep() -> TestResult {
        let data = tempfile::tempdir()?;
        let bases = Bases::new(data.path());
        for id in ["base-1", "base-2", "base-3", "base-4", "base-5"] {
            std::fs::create_dir_all(bases.path(id).join("usr/lib"))?;
        }
        write_json(
            &bases.dir.join("current.json"),
            &BaseInfo {
                id: "base-5".into(),
                fingerprint: "f".into(),
                built_at_ms: 1,
                format: BASE_FORMAT,
            },
        )?;
        holder_record(data.path(), "s1", "base-2", true)?;
        holder_record(data.path(), "s2", "base-3", false)?;
        // Not a holder's state directory: ignored.
        std::fs::create_dir_all(data.path().join("runtime/sandboxes/s3"))?;
        let mapper = Mapper {
            bwrap: "/nonexistent/bwrap".into(),
            newuidmap: "/nonexistent/newuidmap".into(),
            newgidmap: "/nonexistent/newgidmap".into(),
            uid: yulius_contracts::sandbox::IdMap {
                host: 1,
                sub_start: 100_000,
                count: 65_536,
            },
            gid: yulius_contracts::sandbox::IdMap {
                host: 1,
                sub_start: 100_000,
                count: 65_536,
            },
        };
        collect(&bases, &["base-4".into()], &mapper, Path::new("/nonexistent/yulius"));
        let left: Vec<bool> = ["base-1", "base-2", "base-3", "base-4", "base-5"]
            .iter()
            .map(|id| bases.path(id).is_dir())
            .collect();
        assert_eq!(left, [false, true, false, true, true]);
        Ok(())
    }
}
