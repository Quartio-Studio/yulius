//! The old host's profile (SPEC-460 MIG-020..028): what a move carries besides the data
//! directory. It is read through [`Host`], so it is tested against a fake host, and fitted to
//! the new host by pure rewrites (release codename, public URL, host name). Files that are
//! carried as they are (keys, the proxy's directory, firewall rules) are only named here: their
//! content travels as a tar stream and never through a request, a line or the status.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::host_setup::{Cmd, Host, Out};

/// The limits an operator pins for one machine (SPEC-459): carried unchanged and named.
pub const LIMIT_VARS: [&str; 7] = [
    "YULIUS_MAX_SESSIONS",
    "YULIUS_CI_MAX_JOBS",
    "YULIUS_CI_LANES",
    "YULIUS_SANDBOX_MEMORY_GB",
    "YULIUS_SANDBOX_DISK_GB",
    "YULIUS_CD_RUN_DISK_GB",
    "YULIUS_SANDBOX_PIDS",
];
/// The distribution's own apt sources, never carried.
const DISTRO_SOURCES: [&str; 2] = ["ubuntu.sources", "debian.sources"];
/// Where apt reads sources from.
pub const SOURCES_DIR: &str = "/etc/apt/sources.list.d";
/// Where an operator's units live; nothing else is carried as a unit.
pub const UNITS_DIR: &str = "/etc/systemd/system";
/// Caddy's configuration.
pub const CADDYFILE: &str = "/etc/caddy/Caddyfile";
/// Other proxies a host may run: named, not carried.
const OTHER_PROXIES: [&str; 4] = ["nginx", "apache2", "haproxy", "traefik"];
/// ufw's rules and settings.
const UFW_FILES: [&str; 4] = [
    "/etc/ufw/ufw.conf",
    "/etc/ufw/user.rules",
    "/etc/ufw/user6.rules",
    "/etc/default/ufw",
];
/// Packages of the machine, not of the software on it: a move never installs them (prefixes).
const HOST_PACKAGES: [&str; 14] = [
    "linux-",
    "grub",
    "shim",
    "cloud-init",
    "cloud-initramfs",
    "netplan",
    "ifupdown",
    "network-manager",
    "systemd-boot",
    "intel-microcode",
    "amd64-microcode",
    "open-vm-tools",
    "qemu-guest-agent",
    "ubuntu-",
];

/// The packages that select the coreutils flavour (Ubuntu 25.10 and later). They conflict with
/// each other, so they are never part of the package transaction: the coreutils step selects.
pub const FLAVOUR_PACKAGES: [&str; 4] = [
    "coreutils-from-gnu",
    "coreutils-from-uutils",
    "gnu-coreutils",
    "rust-coreutils",
];

/// A distribution release, from `/etc/os-release`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// `ID` (`ubuntu`).
    pub id: String,
    /// `VERSION_ID` (`24.04`).
    pub version: String,
    /// `VERSION_CODENAME` (`noble`).
    pub codename: String,
}

impl Release {
    /// The release an `/etc/os-release` text names.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_owned())
                .unwrap_or_default()
        };
        Self {
            id: field("ID"),
            version: field("VERSION_ID"),
            codename: field("VERSION_CODENAME"),
        }
    }

    /// `ubuntu 24.04`.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{} {}", self.id, self.version).trim().to_owned()
    }
}

/// A text file written on the new host after its rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextFile {
    /// Where it is, on both hosts.
    pub path: PathBuf,
    /// Its permission bits.
    pub mode: u32,
    /// Its text.
    pub content: String,
}

/// A directory copied as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    /// Its path, on both hosts.
    pub path: PathBuf,
    /// Its size.
    pub bytes: u64,
}

/// Whether a part of the host (the proxy, the firewall) is carried.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Carried {
    /// The old host has none.
    #[default]
    None,
    /// The kind a move carries (Caddy, ufw).
    Yes,
    /// Another kind, named for the operator.
    Other(String),
}

/// What the old host is, beside its data directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Its release.
    pub release: Release,
    /// `uname -m`.
    pub arch: String,
    /// `GNU`, `uutils` or empty when unknown.
    pub coreutils: String,
    /// Logical CPUs.
    pub cpus: u32,
    /// Memory in bytes.
    pub memory: u64,
    /// The server's unit.
    pub unit: String,
    /// The service user.
    pub user: String,
    /// The unit's `ExecStart` binary.
    pub exe: PathBuf,
    /// The data directory.
    pub data_dir: PathBuf,
    /// The unit's `YULIUS_PUBLIC_URL`.
    pub public_url: Option<String>,
    /// Bytes used on the data directory's filesystem (an upper bound of the copy).
    pub used_bytes: u64,
    /// Inodes used there.
    pub used_inodes: u64,
    /// The unit file and its drop-ins.
    pub units: Vec<TextFile>,
    /// Third-party apt sources.
    pub sources: Vec<TextFile>,
    /// Caddy's configuration.
    pub caddyfile: Option<TextFile>,
    /// The key files the apt sources name, carried as they are before the sources.
    pub keys: Vec<PathBuf>,
    /// Files and directories carried as they are, by path.
    pub files: Vec<PathBuf>,
    /// Software outside apt.
    pub trees: Vec<Tree>,
    /// `apt-mark showmanual`.
    pub packages: Vec<String>,
    /// Those of them that come from a carried third-party source: the new host knows them only
    /// once that source is added.
    pub third_party: Vec<String>,
    /// Global npm packages (`name@version`) outside the copied trees.
    pub npm: Vec<String>,
    /// Pinned limits, `NAME=value`.
    pub pins: Vec<String>,
    /// Carried files that name the old host: they travel as they are, so a person edits them.
    pub stale: Vec<PathBuf>,
    /// The update helper is installed.
    pub helper: bool,
    /// The move helper (SPEC-470): `None` without its units, else whether its path unit is
    /// enabled.
    pub move_helper: Option<bool>,
    /// The reverse proxy.
    pub proxy: Carried,
    /// The firewall.
    pub firewall: Carried,
    /// The ports Yulius binds.
    pub ports: Vec<u16>,
}

fn run(host: &mut dyn Host, argv: &[&str]) -> Out {
    host.run(&Cmd::new(argv)).unwrap_or_else(|why| Out {
        code: -1,
        stdout: String::new(),
        stderr: why,
    })
}

/// The text a command printed when it succeeded.
fn said(host: &mut dyn Host, argv: &[&str]) -> Option<String> {
    let out = run(host, argv);
    out.ok().then(|| out.stdout.trim().to_owned())
}

fn is_dir(host: &dyn Host, path: &Path) -> bool {
    host.stat(path)
        .is_some_and(|(_, mode)| mode & 0o170_000 == 0o040_000)
}

/// The properties `systemctl show` printed, in order (a property may repeat).
fn properties(text: &str) -> Vec<(&str, &str)> {
    text.lines().filter_map(|l| l.split_once('=')).collect()
}

fn property<'a>(props: &[(&'a str, &'a str)], key: &str) -> &'a str {
    props
        .iter()
        .find(|(k, _)| *k == key)
        .map_or("", |(_, v)| v.trim())
}

/// The assignments of an `Environment=` property.
fn assignments(value: &str) -> Vec<(String, String)> {
    value
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Whether a move leaves `package` to the new machine (its kernel, boot loader, network setup).
#[must_use]
pub fn host_package(package: &str) -> bool {
    HOST_PACKAGES.iter().any(|p| package.starts_with(p))
}

/// The key files a source's text names (`signed-by=` in a `.list`, `Signed-By:` in a
/// `.sources`).
#[must_use]
pub fn key_paths(source: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        let values = if let Some(rest) = line.strip_prefix("Signed-By:") {
            vec![rest]
        } else {
            line.split(['[', ']', ' '])
                .filter_map(|word| word.strip_prefix("signed-by="))
                .collect()
        };
        for value in values {
            for path in value.split(',').map(str::trim) {
                if path.starts_with('/') && !paths.iter().any(|p| p == Path::new(path)) {
                    paths.push(PathBuf::from(path));
                }
            }
        }
    }
    paths
}

/// The URLs an apt source's text names.
#[must_use]
pub fn source_urls(source: &str) -> Vec<&str> {
    source
        .split_whitespace()
        .filter(|word| word.starts_with("http://") || word.starts_with("https://"))
        .collect()
}

/// `text` with the word `old` replaced by `new` (a release codename in an apt source: a suite
/// such as `noble` or `noble-updates`, never part of a longer word).
#[must_use]
pub fn with_codename(text: &str, old: &str, new: &str) -> String {
    if old.is_empty() || old == new {
        return text.to_owned();
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(old) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + old.len()..].chars().next();
        out.push_str(&rest[..at]);
        out.push_str(if word(before) || word(after) {
            old
        } else {
            new
        });
        rest = &rest[at + old.len()..];
    }
    out.push_str(rest);
    out
}

/// `text` with the host name `old` replaced by `new`, also as the end of a longer name
/// (`*.old`), never inside another name (`not-old`, `old.example`).
#[must_use]
pub fn with_host(text: &str, old: &str, new: &str) -> String {
    if old.is_empty() || old == new {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(old) {
        let before = rest[..at].chars().next_back();
        let mut after = rest[at + old.len()..].chars();
        let inside = before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            || match after.next() {
                Some('.') => after.next().is_some_and(|c| c.is_ascii_alphanumeric()),
                Some(c) => c.is_ascii_alphanumeric() || c == '-',
                None => false,
            };
        out.push_str(&rest[..at]);
        out.push_str(if inside { old } else { new });
        rest = &rest[at + old.len()..];
    }
    out.push_str(rest);
    out
}

/// The host name of a URL (no scheme, port or path).
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', ':', '?', '#']).next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The URL of the same form for another address, when `old_url`'s host is made of an IPv4
/// address (`1-2-3-4.sslip.io`, `app.1.2.3.4.nip.io`); `None` for a name of its own.
#[must_use]
pub fn address_url(old_url: &str, address: &str) -> Option<String> {
    let new: Vec<&str> = address.split('.').collect();
    let octet = |s: &&str| s.parse::<u8>().is_ok();
    if new.len() != 4 || !new.iter().all(octet) {
        return None;
    }
    let host = host_of(old_url)?;
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    if n < 3 || !matches!(labels[n - 2..], ["sslip" | "nip", "io"]) {
        return None;
    }
    let made = if n >= 6 && labels[n - 6..n - 2].iter().all(octet) {
        let mut all = labels[..n - 6].to_vec();
        all.extend(&new);
        all.extend(&labels[n - 2..]);
        all.join(".")
    } else {
        let parts: Vec<&str> = labels[n - 3].split('-').collect();
        let p = parts.len();
        if p < 4 || !parts[p - 4..].iter().all(octet) {
            return None;
        }
        let mut label = parts[..p - 4].to_vec();
        label.extend(&new);
        let mut all = labels[..n - 3].to_vec();
        let label = label.join("-");
        all.push(&label);
        all.extend(&labels[n - 2..]);
        all.join(".")
    };
    Some(with_host(old_url, &host, &made))
}

/// The name without its version suffix, when it has one (`libicu74` and `libssl3t64` give
/// `libicu` and `libssl`): what a library is called across releases.
#[must_use]
pub fn stem(package: &str) -> Option<&str> {
    let name = package.strip_suffix("t64").unwrap_or(package);
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    (stem.len() < name.len() && stem.len() >= 3).then_some(stem)
}

/// The package that replaces `package` among `available` names with the same stem: the one
/// with the highest version suffix.
#[must_use]
pub fn renamed(package: &str, available: &[String]) -> Option<String> {
    let stem = stem(package)?;
    let version = |name: &str| -> Option<Vec<u32>> {
        let rest = name.strip_prefix(stem)?;
        let rest = rest.strip_suffix("t64").unwrap_or(rest);
        if rest.is_empty() {
            return None;
        }
        rest.split('.').map(|n| n.parse().ok()).collect()
    };
    available
        .iter()
        .filter(|name| *name != package)
        .filter_map(|name| Some((version(name)?, name)))
        .max()
        .map(|(_, name)| name.clone())
}

/// Writes `content` to `path` and gives the new file the owner the old one had (a proxy's
/// configuration that belongs to its group stays readable to it).
///
/// # Errors
/// The write or the `chown` failed.
pub fn write_keeping_owner(
    host: &mut dyn Host,
    path: &Path,
    content: &str,
    mode: u32,
) -> Result<(), String> {
    let owner = host.stat(path).map(|(uid, _)| uid).zip(host.gid(path));
    host.write(path, content, mode & 0o7777)?;
    if let Some((uid, gid)) = owner.filter(|owner| *owner != (0, 0)) {
        let out = run(
            host,
            &[
                "chown",
                "--",
                &format!("{uid}:{gid}"),
                &path.display().to_string(),
            ],
        );
        if !out.ok() {
            return Err(format!("chown: {}", out.why()));
        }
    }
    Ok(())
}

/// `caddyfile` with every `reverse_proxy` to the local port `port` replaced by a temporary
/// redirect to the same path at `new_url` (SPEC-460 MIG-041), so the site keeps its name, its
/// TLS settings and its neighbours; `None` when it proxies nothing there.
#[must_use]
pub fn redirected(caddyfile: &str, port: u16, new_url: &str) -> Option<String> {
    let new_url = new_url.trim_end_matches('/');
    let upstream = format!(":{port}");
    let mut out = String::new();
    let mut found = false;
    let mut skip_depth = 0usize;
    for line in caddyfile.lines() {
        let text = line.trim();
        if skip_depth > 0 {
            // Inside the directive's own block (its `header_up` lines): dropped with it.
            skip_depth += text.matches('{').count();
            skip_depth = skip_depth.saturating_sub(text.matches('}').count());
            continue;
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let proxies = words.first() == Some(&"reverse_proxy")
            && words[1..]
                .iter()
                .any(|w| w.trim_end_matches('{').ends_with(&upstream));
        if !proxies {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        found = true;
        let indent = &line[..line.len() - line.trim_start().len()];
        out.push_str(&format!(
            "{indent}# Written by `yulius instance migrate`: this instance moved; `--abort` \
             puts the earlier file back.\n{indent}header X-Yulius-Moved-To {new_url}\n\
             {indent}redir {new_url}{{uri}} 307\n"
        ));
        if text.ends_with('{') {
            skip_depth = 1;
        }
    }
    found.then_some(out)
}

/// A file's text and mode, when it is a readable regular file.
fn text_file(host: &dyn Host, path: &Path) -> Option<TextFile> {
    let content = host.read(path)?;
    let mode = host.stat(path).map_or(0o644, |(_, mode)| mode & 0o7777);
    Some(TextFile {
        path: path.to_path_buf(),
        mode,
        content,
    })
}

fn push_path(files: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if !files.contains(&path) {
        files.push(path);
    }
}

/// The unit files of `props` an operator wrote (under [`UNITS_DIR`]).
fn own_units(props: &[(&str, &str)]) -> Vec<PathBuf> {
    let mut paths = vec![property(props, "FragmentPath").to_owned()];
    paths.extend(
        property(props, "DropInPaths")
            .split_whitespace()
            .map(str::to_owned),
    );
    paths
        .into_iter()
        .filter(|p| p.starts_with(&format!("{UNITS_DIR}/")))
        .map(PathBuf::from)
        .collect()
}

impl Profile {
    /// Reads the old host: the unit `unit` of the instance at `data_dir` and what stands beside
    /// it.
    ///
    /// # Errors
    /// The unit is unknown, or its file is not an operator's (nothing to carry it from).
    pub fn read(
        host: &mut dyn Host,
        data_dir: &Path,
        unit: &str,
        update_unit: &str,
    ) -> Result<Self, String> {
        let mut p = Self {
            unit: unit.to_owned(),
            data_dir: data_dir.to_path_buf(),
            ..Self::default()
        };
        p.read_unit(host)?;
        p.read_helper(host, update_unit);
        p.read_move_helper(host);
        p.release = Release::parse(&host.read(Path::new("/etc/os-release")).unwrap_or_default());
        p.arch = said(host, &["uname", "-m"]).unwrap_or_default();
        p.coreutils = coreutils(host);
        p.cpus = said(host, &["nproc"])
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        p.memory = memory(host);
        let data = data_dir.display().to_string();
        if let Some(stat) = said(host, &["stat", "-f", "-c", "%S %b %f %c %d", &data]) {
            let n: Vec<u64> = stat
                .split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect();
            if let [block, blocks, free, inodes, inodes_free] = n[..] {
                p.used_bytes = block.saturating_mul(blocks.saturating_sub(free));
                p.used_inodes = inodes.saturating_sub(inodes_free);
            }
        }
        p.read_sources(host);
        p.packages = said(host, &["apt-mark", "showmanual"])
            .map(|text| text.lines().map(|l| l.trim().to_owned()).collect())
            .unwrap_or_default();
        p.packages.retain(|name| !name.is_empty());
        p.read_origins(host);
        p.read_trees(host);
        p.read_npm(host);
        p.read_proxy(host);
        p.read_firewall(host);
        p.files.sort();
        Ok(p)
    }

    fn read_unit(&mut self, host: &mut dyn Host) -> Result<(), String> {
        let unit = self.unit.clone();
        let out = run(
            host,
            &[
                "systemctl",
                "show",
                &unit,
                "--property=LoadState,FragmentPath,DropInPaths,User,ExecStart,Environment,\
                 EnvironmentFiles",
            ],
        );
        let props = properties(&out.stdout);
        if !out.ok() || property(&props, "LoadState") != "loaded" {
            return Err(format!(
                "there is no unit {unit}; name the server's unit with --service"
            ));
        }
        let fragment = property(&props, "FragmentPath");
        if !fragment.starts_with(&format!("{UNITS_DIR}/")) {
            return Err(format!(
                "{unit} is defined in {fragment:?}, not under {UNITS_DIR}: a move carries an \
                 operator's unit only"
            ));
        }
        self.user = property(&props, "User").to_owned();
        if self.user.is_empty() {
            return Err(format!("{unit} names no User="));
        }
        self.exe = property(&props, "ExecStart")
            .split_once("path=")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .map_or_else(|| PathBuf::from("/usr/local/bin/yulius"), PathBuf::from);
        let mut units = own_units(&props);
        // Sessions live in a unit of their own where the host has one (SPEC-458). Its file is
        // the new host's setup's to write; what an operator added to it (limits for all
        // sandboxes) travels like the server unit's drop-ins.
        if let Some(sessions) = yulius_sandbox::units::sessions_unit(&unit) {
            let out = run(
                host,
                &[
                    "systemctl",
                    "show",
                    &sessions,
                    "--property=LoadState,DropInPaths",
                ],
            );
            let props = properties(&out.stdout);
            if out.ok() && property(&props, "LoadState") == "loaded" {
                units.extend(
                    property(&props, "DropInPaths")
                        .split_whitespace()
                        .filter(|path| path.starts_with(&format!("{UNITS_DIR}/")))
                        .map(PathBuf::from),
                );
            }
        }
        for path in units {
            match text_file(host, &path) {
                Some(file) => self.units.push(file),
                None => return Err(format!("cannot read {}", path.display())),
            }
        }
        let env = assignments(property(&props, "Environment"));
        self.public_url = env
            .iter()
            .find(|(name, _)| name == "YULIUS_PUBLIC_URL")
            .map(|(_, url)| url.trim_end_matches('/').to_owned());
        self.ports = ["YULIUS_BIND", "YULIUS_INTERNAL_BIND"]
            .iter()
            .zip([8080u16, 8081])
            .map(|(name, default)| {
                env.iter()
                    .find(|(n, _)| n == name)
                    .and_then(|(_, bind)| bind.rsplit(':').next()?.parse().ok())
                    .unwrap_or(default)
            })
            .collect();
        for (name, value) in &env {
            if LIMIT_VARS.contains(&name.as_str()) {
                self.pins.push(format!("{name}={value}"));
            }
            let path = Path::new(value);
            if name.ends_with("_FILE")
                && path.is_absolute()
                && !path.starts_with(&self.data_dir)
                && host.is_file(path)
            {
                push_path(&mut self.files, path);
            }
        }
        let old_host = self.public_url.as_deref().and_then(host_of);
        for (_, value) in props.iter().filter(|(k, _)| *k == "EnvironmentFiles") {
            let path = value.split_whitespace().next().unwrap_or_default();
            let path = Path::new(path.trim_start_matches('-'));
            if path.is_absolute() && !path.starts_with(&self.data_dir) && host.is_file(path) {
                push_path(&mut self.files, path);
                // Read here only to say so: its text never leaves this host but as a stream.
                if let Some(old_host) = &old_host
                    && host
                        .read(path)
                        .is_some_and(|text| text.contains(old_host.as_str()))
                {
                    push_path(&mut self.stale, path);
                }
            }
        }
        Ok(())
    }

    fn read_helper(&mut self, host: &mut dyn Host, update_unit: &str) {
        let show = "--property=LoadState,FragmentPath,DropInPaths,Environment";
        let service = run(host, &["systemctl", "show", update_unit, show]);
        let props = properties(&service.stdout);
        if !service.ok() || property(&props, "LoadState") != "loaded" {
            return;
        }
        self.helper = true;
        let mut units = own_units(&props);
        let path_unit = update_unit.replace(".service", ".path");
        let watch = run(host, &["systemctl", "show", &path_unit, show]);
        units.extend(own_units(&properties(&watch.stdout)));
        for unit in units {
            push_path(&mut self.files, unit);
        }
        let state = assignments(property(&props, "Environment"))
            .into_iter()
            .find(|(name, _)| name == "YULIUS_UPDATE_STATE")
            .map_or_else(
                || PathBuf::from("/var/lib/yulius-update"),
                |(_, v)| v.into(),
            );
        let installed = state.join("installed-commit");
        if host.is_file(&installed) {
            push_path(&mut self.files, installed);
        }
    }

    /// The move helper's units (SPEC-470 MVB-008), named after the server's unit: carried when
    /// the host has them, with whether its path unit is enabled.
    fn read_move_helper(&mut self, host: &mut dyn Host) {
        let (Some(service), Some(path_unit)) = (
            yulius_sandbox::units::migrate_unit(&self.unit),
            yulius_sandbox::units::migrate_path_unit(&self.unit),
        ) else {
            return;
        };
        let show = "--property=LoadState,FragmentPath,DropInPaths";
        let shown = run(host, &["systemctl", "show", &service, show]);
        let props = properties(&shown.stdout);
        if !shown.ok() || property(&props, "LoadState") != "loaded" {
            return;
        }
        let mut units = own_units(&props);
        let watch = run(host, &["systemctl", "show", &path_unit, show]);
        units.extend(own_units(&properties(&watch.stdout)));
        for unit in units {
            push_path(&mut self.files, unit);
        }
        let enabled = run(host, &["systemctl", "is-enabled", &path_unit]);
        self.move_helper = Some(enabled.stdout.trim() == "enabled");
    }

    fn read_sources(&mut self, host: &mut dyn Host) {
        for name in host.list(Path::new(SOURCES_DIR)) {
            let carried = (name.ends_with(".list") || name.ends_with(".sources"))
                && !DISTRO_SOURCES.contains(&name.as_str());
            let Some(file) = carried
                .then(|| text_file(host, &Path::new(SOURCES_DIR).join(&name)))
                .flatten()
            else {
                continue;
            };
            for key in key_paths(&file.content) {
                if host.is_file(&key) {
                    push_path(&mut self.keys, key);
                }
            }
            self.sources.push(file);
        }
    }

    /// Which manual packages a carried source provides, from where apt has them here.
    fn read_origins(&mut self, host: &mut dyn Host) {
        let sources: Vec<String> = self
            .sources
            .iter()
            .flat_map(|file| source_urls(&file.content))
            .map(|url| url.trim_end_matches('/').to_owned())
            .collect();
        if sources.is_empty() || self.packages.is_empty() {
            return;
        }
        let mut argv = vec!["apt-cache", "policy", "--"];
        argv.extend(self.packages.iter().map(String::as_str));
        let cmd = Cmd {
            timeout: Duration::from_secs(5 * 60),
            ..Cmd::new(&argv)
        };
        let policy = host.run(&cmd).map(|out| out.stdout).unwrap_or_default();
        let mut name = None;
        for line in policy.lines() {
            if !line.starts_with(' ') {
                name = line.strip_suffix(':');
            } else if let Some(package) = name
                && sources.iter().any(|url| line.contains(url.as_str()))
            {
                self.third_party.push(package.to_owned());
                name = None;
            }
        }
    }

    fn read_trees(&mut self, host: &mut dyn Host) {
        let mut paths = vec![PathBuf::from("/usr/local")];
        for name in host.list(Path::new("/opt")) {
            let path = Path::new("/opt").join(&name);
            // A package's directory comes back with the package.
            if !run(host, &["dpkg-query", "-S", &path.display().to_string()]).ok() {
                paths.push(path);
            }
        }
        paths.extend(["/root/.rustup", "/root/.cargo"].map(PathBuf::from));
        for path in paths {
            // The data directory travels as the copy, never as software.
            if !is_dir(host, &path) || self.data_dir.starts_with(&path) {
                continue;
            }
            let shown = path.display().to_string();
            let cmd = Cmd {
                timeout: Duration::from_secs(10 * 60),
                ..Cmd::new(&["du", "-sxb", "--", &shown])
            };
            let bytes = host
                .run(&cmd)
                .ok()
                .filter(Out::ok)
                .and_then(|out| out.stdout.split_whitespace().next()?.parse().ok())
                .unwrap_or(0);
            // An empty directory (containerd's in /opt) is nothing to carry.
            if bytes > 0 {
                self.trees.push(Tree { path, bytes });
            }
        }
    }

    fn read_npm(&mut self, host: &mut dyn Host) {
        let Some(prefix) = said(host, &["npm", "prefix", "-g"]) else {
            return;
        };
        if self
            .trees
            .iter()
            .any(|t| Path::new(&prefix).starts_with(&t.path))
        {
            return;
        }
        // npm exits 1 for a package with unmet peers and still prints the list.
        let listed = run(host, &["npm", "ls", "-g", "--depth=0", "--json"]).stdout;
        self.npm = npm_packages(&listed);
    }

    fn read_proxy(&mut self, host: &mut dyn Host) {
        let active = |host: &mut dyn Host, unit: &str| {
            run(host, &["systemctl", "is-active", unit]).stdout.trim() == "active"
        };
        if active(host, "caddy")
            && let Some(file) = text_file(host, Path::new(CADDYFILE))
        {
            self.proxy = Carried::Yes;
            self.caddyfile = Some(file);
            push_path(&mut self.files, "/etc/caddy");
            let drop_ins = Path::new(UNITS_DIR).join("caddy.service.d");
            if is_dir(host, &drop_ins) {
                push_path(&mut self.files, drop_ins);
            }
            return;
        }
        if let Some(other) = OTHER_PROXIES.iter().find(|unit| active(host, unit)) {
            self.proxy = Carried::Other((*other).to_owned());
        }
    }

    fn read_firewall(&mut self, host: &mut dyn Host) {
        let enabled = host
            .read(Path::new("/etc/ufw/ufw.conf"))
            .is_some_and(|text| text.lines().any(|l| l.trim() == "ENABLED=yes"));
        if enabled {
            self.firewall = Carried::Yes;
            for file in UFW_FILES {
                if host.is_file(Path::new(file)) {
                    push_path(&mut self.files, file);
                }
            }
        } else if run(host, &["systemctl", "is-enabled", "nftables"])
            .stdout
            .trim()
            == "enabled"
        {
            self.firewall = Carried::Other("nftables".to_owned());
        } else if host.is_file(Path::new("/etc/iptables/rules.v4")) {
            self.firewall = Carried::Other("iptables-persistent".to_owned());
        }
    }
}

/// `GNU` or `uutils`, from `ls --version`; empty when it says neither.
pub fn coreutils(host: &mut dyn Host) -> String {
    let said = run(host, &["ls", "--version"]).stdout;
    let first = said.lines().next().unwrap_or_default();
    ["GNU", "uutils"]
        .iter()
        .find(|flavour| first.contains(*flavour))
        .map(|flavour| (*flavour).to_owned())
        .unwrap_or_default()
}

/// The host's memory in bytes, from `/proc/meminfo`.
pub fn memory(host: &dyn Host) -> u64 {
    host.read(Path::new("/proc/meminfo"))
        .and_then(|text| {
            let line = text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?;
            line.split_whitespace().next()?.parse::<u64>().ok()
        })
        .map_or(0, |kib| kib.saturating_mul(1024))
}

/// `name@version` of every package in `npm ls -g --depth=0 --json`, npm's own left out.
#[must_use]
pub fn npm_packages(json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(deps) = value.get("dependencies").and_then(|d| d.as_object()) else {
        return Vec::new();
    };
    deps.iter()
        .filter(|(name, _)| !matches!(name.as_str(), "npm" | "corepack"))
        .filter_map(|(name, dep)| {
            let version = dep.get("version")?.as_str()?;
            let plain = |s: &str| {
                !s.is_empty()
                    && !s.starts_with('-')
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "@/._-+".contains(c))
            };
            (plain(name) && plain(version)).then(|| format!("{name}@{version}"))
        })
        .collect()
}
