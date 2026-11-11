//! The new host's side of a move (SPEC-460). `yulius instance migrate-target` runs one
//! [`Request`] there, read as JSON from standard input, with the build the old host copied
//! over, and answers one [`Reply`]. Every step reads the facts, acts if needed and returns its
//! lines, so a step that was done before is `ok` when it runs again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_contracts::update::{HostLine, HostOutcome};

use super::profile::{self, Release, SOURCES_DIR, TextFile, UNITS_DIR};
use crate::host_setup::{self, Cmd, Host, Mode, Out};
use crate::instance::Manifest;
use crate::instance::import::INCOMPLETE;
use crate::instance::inventory::Inventory;

/// The flow's own directory on both hosts (root, 0700).
pub const STATE_DIR: &str = "/var/lib/yulius-migrate";
/// The build the old host copied to the new one.
pub const STAGED: &str = "/var/lib/yulius-migrate/yulius";
/// What the next copy is to be installed as ([`Expect`]).
pub const EXPECT_FILE: &str = "/var/lib/yulius-migrate/expect.json";
/// The reply's line on standard output; everything before it is a command's output.
pub const REPLY: &str = "YULIUS-REPLY ";
pub use yulius_app::host_move::{MOVE_PENDING, MOVED};
/// The file system label of a data disk a move made.
const DISK_LABEL: &str = "yulius-data";
/// The array a move makes of two disks.
const MIRROR: &str = "/dev/md/yulius";
/// How long a started instance has to report its build.
const HEALTH_WAIT: Duration = Duration::from_secs(120);
/// The programs that may hold the instance's ports before it starts.
const OWN_PROGRAMS: [&str; 2] = ["caddy", "yulius"];

/// What `host setup` needs to know of the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The service user.
    pub user: String,
    /// The server's unit.
    pub service: String,
    /// The unit's binary.
    pub exe: PathBuf,
}

/// An apt source for the new release, with the old host's text to fall back on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    /// The file, with the new release's codename.
    pub file: TextFile,
    /// Its text on the old host.
    pub fallback: String,
}

/// How the next copy is installed; kept in [`EXPECT_FILE`] so the receiving command needs no
/// argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expect {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The stream is compressed with zstd.
    pub zstd: bool,
}

/// One step for the new host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Request {
    /// What the host is ([`Facts`]); changes nothing.
    Facts {
        /// The data directory.
        data_dir: PathBuf,
        /// The server's unit.
        service: String,
        /// The ports the instance and its proxy need.
        ports: Vec<u16>,
    },
    /// Make the data disk of the named blank disks.
    Disk {
        /// The instance.
        setup: Setup,
        /// One disk, or two for a mirror.
        disks: Vec<String>,
    },
    /// Write third-party apt sources and read the package lists.
    Sources {
        /// The sources.
        files: Vec<SourceFile>,
    },
    /// Fit the old host's packages to this release and, with `install`, install them.
    Packages {
        /// `apt-mark showmanual` of the old host.
        names: Vec<String>,
        /// Install; without it only say what would be.
        install: bool,
    },
    /// Make coreutils the old host's flavour.
    Coreutils {
        /// `GNU` or `uutils`.
        flavour: String,
    },
    /// Install global npm packages at the old host's versions.
    Npm {
        /// `name@version`.
        packages: Vec<String>,
    },
    /// `host setup`'s host items.
    HostItems {
        /// The instance.
        setup: Setup,
    },
    /// Install the staged build at the unit's path.
    Binary {
        /// The instance.
        setup: Setup,
    },
    /// Make the data directory ready for a copy.
    DataDir {
        /// The instance.
        setup: Setup,
    },
    /// `host setup` up to the sandbox base, so the host is proven ready before the freeze.
    BeforeStart {
        /// The instance.
        setup: Setup,
    },
    /// Write text files under one checklist name.
    Texts {
        /// The line's name.
        name: String,
        /// The files.
        files: Vec<TextFile>,
    },
    /// Validate and start Caddy.
    Proxy {
        /// Its files changed in this run.
        changed: bool,
    },
    /// Say how the next copy is installed.
    Expect(Expect),
    /// Count the installed copy and name the sandboxes built on another release.
    Inventory {
        /// The copy's manifest (its ids).
        manifest: Manifest,
        /// The old host's inventory.
        old: Inventory,
        /// The old host's release.
        release: Release,
    },
    /// Mark the copy as moved.
    Mark {
        /// The instance.
        setup: Setup,
        /// The old public URL.
        from: Option<String>,
        /// The new one.
        to: String,
    },
    /// Mark a copy that was installed but never started as unfinished, so the next run removes
    /// it.
    Discard {
        /// The data directory.
        data_dir: PathBuf,
    },
    /// Finish `host setup`, start the instance and wait until it reports `build`.
    Start {
        /// The instance.
        setup: Setup,
        /// The commit it must report.
        build: Option<String>,
        /// The address of its internal listener (`127.0.0.1:8081`).
        health: String,
        /// The update helper's path unit to enable.
        helper: Option<String>,
    },
    /// Stop and disable the instance.
    Stop {
        /// The server's unit.
        service: String,
    },
    /// What the first start did with the tasks that were live.
    Tasks {
        /// The data directory.
        data_dir: PathBuf,
        /// How long to wait for it.
        wait_secs: u64,
    },
    /// Remove the one-time key and the flow's files; enable the firewall.
    Finish {
        /// The key's comment in `authorized_keys`.
        key: String,
        /// Enable ufw.
        firewall: bool,
    },
}

/// Whether the host holds an instance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Instance {
    /// Nothing there.
    #[default]
    None,
    /// Its unit is active.
    Running,
    /// Its data directory holds a database.
    Present,
    /// An import that did not finish.
    Incomplete,
}

/// A blank disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    /// Its device path.
    pub path: String,
    /// Its size.
    pub bytes: u64,
}

/// What the new host is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// Its release.
    pub release: Release,
    /// `uname -m`.
    pub arch: String,
    /// systemd's version.
    pub systemd: u32,
    /// Logical CPUs.
    pub cpus: u32,
    /// Memory in bytes.
    pub memory: u64,
    /// The address the old host reached it at.
    pub address: Option<String>,
    /// The staged build's commit.
    pub build: Option<String>,
    /// The staged build's version.
    pub version: String,
    /// Whether it holds an instance.
    pub instance: Instance,
    /// Free bytes where the data directory goes.
    pub free_bytes: u64,
    /// Free inodes there (0 when the filesystem counts none).
    pub free_inodes: u64,
    /// Blank disks.
    pub blank: Vec<Disk>,
    /// Needed ports another program holds (`80 (nginx)`).
    pub busy_ports: Vec<String>,
    /// `zstd` is installed.
    pub zstd: bool,
    /// The data directory is a mount point of its own.
    pub mounted: bool,
}

/// A step's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// The checklist lines.
    #[serde(default)]
    pub lines: Vec<HostLine>,
    /// [`Request::Facts`]'s answer.
    #[serde(default)]
    pub facts: Option<Facts>,
    /// [`Request::Inventory`]'s answer.
    #[serde(default)]
    pub inventory: Option<Inventory>,
}

/// A part of `host setup` ([`host_setup::Run`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Packages, Docker, kernel modules, the service user and its ids.
    Host,
    /// Everything up to the sandbox base.
    BeforeStart,
    /// The running service: verdict, contained doctor, Docker smoke.
    AfterStart,
}

/// What a step needs beside the host: this process and its clock.
pub struct Env<'a> {
    /// The address the caller connected to (`SSH_CONNECTION`).
    pub address: Option<String>,
    /// The commit the server at a loopback address reports (`/internal/version`); `None` while
    /// it does not answer.
    pub health: &'a mut dyn FnMut(&str) -> Option<String>,
    /// Waits.
    pub sleep: &'a mut dyn FnMut(Duration),
    /// The inventory of the copy at a data directory (the real one walks the disk).
    pub count: &'a mut dyn FnMut(&Manifest, &Inventory) -> Result<Inventory, String>,
    /// Runs a part of `host setup` and returns its lines.
    pub setup: &'a mut dyn FnMut(&mut dyn Host, Part, &Setup) -> Vec<HostLine>,
}

/// The lines of one step.
#[derive(Debug, Default)]
struct Lines(Vec<HostLine>);

impl Lines {
    fn push(&mut self, name: &str, outcome: HostOutcome, detail: impl Into<String>) {
        self.0.push(HostLine {
            name: name.to_owned(),
            outcome,
            detail: detail.into(),
        });
    }

    fn ok(&mut self, name: &str, detail: impl Into<String>) {
        self.push(name, HostOutcome::Ok, detail);
    }

    fn changed(&mut self, name: &str, detail: impl Into<String>) {
        self.push(name, HostOutcome::Changed, detail);
    }

    fn manual(&mut self, name: &str, detail: impl Into<String>) {
        self.push(name, HostOutcome::Manual, detail);
    }

    fn failed(&mut self, name: &str, detail: impl Into<String>) {
        self.push(name, HostOutcome::Failed, detail);
    }
}

fn run(host: &mut dyn Host, cmd: &Cmd) -> Out {
    host.run(cmd).unwrap_or_else(|why| Out {
        code: -1,
        stdout: String::new(),
        stderr: why,
    })
}

fn sh(host: &mut dyn Host, argv: &[&str]) -> Out {
    run(host, &Cmd::new(argv))
}

/// A command whose output is kept, with more time than the default.
fn long(argv: &[&str], secs: u64) -> Cmd {
    Cmd {
        timeout: Duration::from_secs(secs),
        ..Cmd::new(argv)
    }
}

fn is_dir(host: &dyn Host, path: &Path) -> bool {
    host.stat(path)
        .is_some_and(|(_, mode)| mode & 0o170_000 == 0o040_000)
}

/// A path a step may write or pass to a command: absolute, plain, no option look-alike.
fn plain(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|c| {
            matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

fn uid_of(host: &dyn Host, user: &str) -> Option<u32> {
    let passwd = host.read(Path::new("/etc/passwd")).unwrap_or_default();
    crate::instance::user_by_name(&passwd, user).map(|(uid, _)| uid)
}

/// The blank disks of `lsblk`'s JSON: whole disks with no partition table, filesystem, child
/// or mount, writable.
#[must_use]
pub fn blank_disks(lsblk: &str) -> Vec<Disk> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(lsblk) else {
        return Vec::new();
    };
    let devices = value.get("blockdevices").and_then(|d| d.as_array());
    devices
        .into_iter()
        .flatten()
        .filter(|d| {
            let null = |key: &str| d.get(key).is_none_or(serde_json::Value::is_null);
            let unmounted = d
                .get("mountpoints")
                .and_then(|m| m.as_array())
                .is_none_or(|m| m.iter().all(serde_json::Value::is_null));
            let whole = d
                .get("children")
                .and_then(|c| c.as_array())
                .is_none_or(Vec::is_empty);
            let writable = !d.get("ro").is_some_and(|ro| ro == true || ro == "1");
            d.get("type").is_some_and(|t| t == "disk")
                && null("fstype")
                && null("pttype")
                && unmounted
                && whole
                && writable
        })
        .filter_map(|d| {
            let bytes = d
                .get("size")
                .and_then(|s| s.as_u64().or_else(|| s.as_str()?.parse().ok()))?;
            Some(Disk {
                path: d.get("path")?.as_str()?.to_owned(),
                bytes,
            })
        })
        .filter(|d| d.bytes > 0)
        .collect()
}

fn list_blank(host: &mut dyn Host) -> Vec<Disk> {
    let out = sh(
        host,
        &[
            "lsblk",
            "--json",
            "--bytes",
            "--output",
            "NAME,PATH,TYPE,SIZE,FSTYPE,PTTYPE,MOUNTPOINTS,RO",
        ],
    );
    blank_disks(&out.stdout)
}

/// The needed ports of `ss -H -ltnp` that a program other than Yulius's own holds.
#[must_use]
pub fn busy_ports(ss: &str, ports: &[u16]) -> Vec<String> {
    let mut busy = Vec::new();
    for line in ss.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(port) = fields
            .get(3)
            .and_then(|local| local.rsplit(':').next()?.parse::<u16>().ok())
        else {
            continue;
        };
        let program = line
            .split_once("((\"")
            .and_then(|(_, rest)| rest.split('"').next())
            .unwrap_or("another program");
        let shown = format!("{port} ({program})");
        if ports.contains(&port) && !OWN_PROGRAMS.contains(&program) && !busy.contains(&shown) {
            busy.push(shown);
        }
    }
    busy
}

fn mounted(host: &mut dyn Host, data_dir: &Path) -> Option<String> {
    let shown = data_dir.display().to_string();
    let out = sh(
        host,
        &["findmnt", "-n", "-o", "SOURCE", "--mountpoint", &shown],
    );
    let source = out.stdout.trim();
    (out.ok() && !source.is_empty()).then(|| source.to_owned())
}

fn facts(
    host: &mut dyn Host,
    env: &Env<'_>,
    data_dir: &Path,
    service: &str,
    ports: &[u16],
) -> Facts {
    let mut f = Facts {
        release: Release::parse(&host.read(Path::new("/etc/os-release")).unwrap_or_default()),
        arch: sh(host, &["uname", "-m"]).stdout.trim().to_owned(),
        cpus: sh(host, &["nproc"]).stdout.trim().parse().unwrap_or(0),
        memory: profile::memory(host),
        address: env.address.clone(),
        build: crate::serve::build_commit(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        ..Facts::default()
    };
    f.systemd = sh(host, &["systemctl", "--version"])
        .stdout
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    f.instance = if sh(host, &["systemctl", "is-active", service]).stdout.trim() == "active" {
        Instance::Running
    } else if host.is_file(&data_dir.join(INCOMPLETE)) {
        Instance::Incomplete
    } else if host.is_file(&data_dir.join("yulius.db")) {
        Instance::Present
    } else {
        Instance::None
    };
    let mut at = Some(data_dir);
    while let Some(dir) = at {
        if host.stat(dir).is_some() {
            let shown = dir.display().to_string();
            let stat = sh(host, &["stat", "-f", "-c", "%S %a %d", &shown]).stdout;
            let n: Vec<u64> = stat
                .split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect();
            if let [block, available, inodes] = n[..] {
                f.free_bytes = block.saturating_mul(available);
                f.free_inodes = inodes;
            }
            break;
        }
        at = dir.parent();
    }
    f.blank = list_blank(host);
    f.busy_ports = busy_ports(&sh(host, &["ss", "-H", "-ltnp"]).stdout, ports);
    f.zstd = sh(host, &["zstd", "--version"]).ok();
    f.mounted = mounted(host, data_dir).is_some();
    f
}

/// The unit drop-in that makes the service wait for its data disk.
fn mount_drop_in(service: &str) -> PathBuf {
    Path::new(UNITS_DIR)
        .join(format!("{service}.d"))
        .join("10-yulius-data-mount.conf")
}

fn disk(host: &mut dyn Host, setup: &Setup, disks: &[String], l: &mut Lines) {
    const ITEM: &str = "data disk";
    let data_dir = &setup.data_dir;
    let data = data_dir.display().to_string();
    if !plain(data_dir) || !host_setup::service_name_ok(&setup.service) {
        return l.failed(ITEM, "the data directory or the unit's name is not plain");
    }
    let drop_in = mount_drop_in(&setup.service);
    let drop_in_text = format!(
        "# Written by `yulius instance migrate`: the data directory is a disk of its own.\n\
         [Unit]\nRequiresMountsFor={data}\n"
    );
    if let Some(source) = mounted(host, data_dir) {
        if host.read(&drop_in).as_deref() != Some(&drop_in_text)
            && let Err(why) = host.write(&drop_in, &drop_in_text, 0o644)
        {
            return l.failed(ITEM, format!("cannot write {}: {why}", drop_in.display()));
        }
        return l.ok(ITEM, format!("{source} is mounted at {data}"));
    }
    if !(1..=2).contains(&disks.len()) {
        return l.failed(ITEM, "name one disk, or two for a mirror");
    }
    let blank = list_blank(host);
    if let Some(taken) = disks.iter().find(|d| !blank.iter().any(|b| &b.path == *d)) {
        return l.failed(
            ITEM,
            format!(
                "{taken} is not a blank disk (it has a partition table, a filesystem, a holder \
                 or a mount, or does not exist); nothing was changed"
            ),
        );
    }
    if host.list(data_dir).iter().any(|name| name != "lost+found") {
        return l.failed(
            ITEM,
            format!("{data} holds files: a disk cannot be mounted over them"),
        );
    }
    let step = |host: &mut dyn Host, cmd: &Cmd| -> Result<Out, String> {
        let out = run(host, cmd);
        if out.ok() {
            Ok(out)
        } else {
            Err(format!("`{}` failed: {}", cmd.argv.join(" "), out.why()))
        }
    };
    let made = (|| -> Result<String, String> {
        let device = if let [a, b] = disks {
            if !sh(host, &["mdadm", "--version"]).ok() {
                step(host, &Cmd::apt(&["install", "-y", "mdadm"]))?;
            }
            step(
                host,
                &long(
                    &[
                        "mdadm",
                        "--create",
                        MIRROR,
                        "--run",
                        "--level=1",
                        "--raid-devices=2",
                        "--metadata=1.2",
                        "--name=yulius",
                        a,
                        b,
                    ],
                    120,
                ),
            )?;
            MIRROR.to_owned()
        } else {
            disks[0].clone()
        };
        step(
            host,
            &long(&["mkfs.ext4", "-q", "-L", DISK_LABEL, &device], 30 * 60),
        )?;
        let uuid = step(
            host,
            &Cmd::new(&["blkid", "-s", "UUID", "-o", "value", &device]),
        )?
        .stdout
        .trim()
        .to_owned();
        if uuid.is_empty() || !uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(format!("{device} reports no filesystem UUID"));
        }
        let mut fstab = host.read(Path::new("/etc/fstab")).unwrap_or_default();
        if !fstab.is_empty() && !fstab.ends_with('\n') {
            fstab.push('\n');
        }
        fstab.push_str(&format!("UUID={uuid} {data} ext4 defaults,nofail 0 2\n"));
        host.write(Path::new("/etc/fstab"), &fstab, 0o644)
            .map_err(|why| format!("cannot write /etc/fstab: {why}"))?;
        step(host, &Cmd::new(&["mkdir", "-p", "--", &data]))?;
        step(host, &Cmd::new(&["mount", "--", &data]))?;
        if disks.len() == 2 {
            let scan = step(host, &Cmd::new(&["mdadm", "--detail", "--scan"]))?.stdout;
            let conf = Path::new("/etc/mdadm/mdadm.conf");
            let mut text = host.read(conf).unwrap_or_default();
            for array in scan.lines().map(str::trim) {
                if array.is_empty() || text.contains(array) {
                    continue;
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(array);
                text.push('\n');
            }
            host.write(conf, &text, 0o644)
                .map_err(|why| format!("cannot write {}: {why}", conf.display()))?;
            step(host, &long(&["update-initramfs", "-u"], 10 * 60))?;
        }
        host.write(&drop_in, &drop_in_text, 0o644)
            .map_err(|why| format!("cannot write {}: {why}", drop_in.display()))?;
        // A new disk's top belongs to root: the service user gets it explicitly.
        if uid_of(host, &setup.user).is_none() {
            step(
                host,
                &Cmd::new(&[
                    "useradd",
                    "--system",
                    "--home-dir",
                    &data,
                    "--shell",
                    "/usr/sbin/nologin",
                    "--",
                    &setup.user,
                ]),
            )?;
        }
        let owner = format!("{0}:{0}", setup.user);
        step(host, &Cmd::new(&["chown", "--", &owner, &data]))?;
        step(host, &Cmd::new(&["chmod", "0700", "--", &data]))?;
        Ok(if disks.len() == 2 {
            format!(
                "made a mirror of {} and {} with ext4, mounted at {data} (fstab by UUID, \
                 mdadm.conf, initramfs, RequiresMountsFor), owned by {}",
                disks[0], disks[1], setup.user
            )
        } else {
            format!(
                "made ext4 on {}, mounted at {data} (fstab by UUID, RequiresMountsFor), owned \
                 by {}",
                disks[0], setup.user
            )
        })
    })();
    match made {
        Ok(what) => l.changed(ITEM, what),
        Err(why) => l.failed(ITEM, why),
    }
}

fn sources(host: &mut dyn Host, files: &[SourceFile], l: &mut Lines) {
    const ITEM: &str = "apt sources";
    let mut wrote = 0;
    for source in files {
        let path = &source.file.path;
        if path.parent() != Some(Path::new(SOURCES_DIR)) || !plain(path) {
            return l.failed(ITEM, format!("{} is not an apt source", path.display()));
        }
        if host.read(path).as_deref() == Some(&source.file.content) {
            continue;
        }
        if let Err(why) = host.write(path, &source.file.content, 0o644) {
            return l.failed(ITEM, format!("cannot write {}: {why}", path.display()));
        }
        wrote += 1;
    }
    let update = long(&["apt-get", "update"], 15 * 60);
    let failing = |out: &Out| -> Vec<&SourceFile> {
        let errors: Vec<&str> = out
            .stderr
            .lines()
            .chain(out.stdout.lines())
            .filter(|line| line.starts_with("E:") || line.starts_with("Err:"))
            .collect();
        files
            .iter()
            .filter(|s| {
                profile::source_urls(&s.file.content)
                    .iter()
                    .any(|url| errors.iter().any(|e| e.contains(url.trim_end_matches('/'))))
            })
            .collect()
    };
    let mut out = run(host, &update);
    let mut kept_old = Vec::new();
    let mut dropped = Vec::new();
    if !out.ok() {
        // A repository without this release's suite: the old release's is the nearest.
        for source in failing(&out) {
            if source.fallback != source.file.content
                && host
                    .write(&source.file.path, &source.fallback, 0o644)
                    .is_ok()
            {
                kept_old.push(source.file.path.display().to_string());
            }
        }
        out = run(host, &update);
    }
    if !out.ok() {
        for source in failing(&out) {
            let shown = source.file.path.display().to_string();
            sh(host, &["rm", "-f", "--", &shown]);
            kept_old.retain(|kept| kept != &shown);
            dropped.push(shown);
        }
        out = run(host, &update);
    }
    if !out.ok() {
        return l.failed(ITEM, format!("`apt-get update` failed: {}", out.why()));
    }
    let mut detail = format!("{} third-party sources", files.len() - dropped.len());
    if !kept_old.is_empty() {
        detail.push_str(&format!(
            "; kept the old release's suite (none for this one): {}",
            kept_old.join(", ")
        ));
    }
    if !dropped.is_empty() {
        return l.manual(
            ITEM,
            format!(
                "{detail}; not usable on this release and removed, add them by hand: {}",
                dropped.join(", ")
            ),
        );
    }
    if wrote > 0 || !kept_old.is_empty() {
        l.changed(ITEM, detail);
    } else {
        l.ok(ITEM, detail);
    }
}

/// The names `apt-cache policy` shows a candidate for.
fn candidates(policy: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut name = None;
    for line in policy.lines() {
        if !line.starts_with(' ') {
            name = line.strip_suffix(':');
        } else if let Some(version) = line.trim().strip_prefix("Candidate:")
            && version.trim() != "(none)"
            && let Some(name) = name.take()
        {
            found.push(name.to_owned());
        }
    }
    found
}

fn regex_escaped(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if r".+*?()[]{}|^$\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A package name as apt takes it (never an option).
fn package_name(name: &str) -> bool {
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.:".contains(c))
}

fn packages(host: &mut dyn Host, names: &[String], install: bool, l: &mut Lines) {
    const ITEM: &str = "packages";
    let (machine, wanted): (Vec<&String>, Vec<&String>) = names
        .iter()
        .filter(|n| package_name(n))
        .partition(|n| profile::host_package(n));
    // A fresh host knows only part of its release until its package lists are read.
    let lists = run(host, &long(&["apt-get", "update"], 15 * 60));
    let mut argv = vec!["apt-cache", "policy", "--"];
    argv.extend(wanted.iter().map(|n| n.as_str()));
    let policy = run(host, &long(&argv, 5 * 60));
    if !policy.ok() {
        return l.failed(ITEM, format!("`apt-cache policy` failed: {}", policy.why()));
    }
    let available = candidates(&policy.stdout);
    let mut to_install: Vec<String> = Vec::new();
    let mut renamed = Vec::new();
    let mut missing = Vec::new();
    for name in wanted {
        if available.contains(name) {
            to_install.push(name.clone());
            continue;
        }
        let other = profile::stem(name).and_then(|stem| {
            let pattern = format!("^{}[0-9.]+(t64)?$", regex_escaped(stem));
            let found = sh(
                host,
                &["apt-cache", "--names-only", "search", "--", &pattern],
            );
            let names: Vec<String> = found
                .stdout
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .map(str::to_owned)
                .collect();
            profile::renamed(name, &names)
        });
        match other {
            Some(other) => {
                renamed.push(format!("{name} → {other}"));
                to_install.push(other);
            }
            None => missing.push(name.clone()),
        }
    }
    let mut detail = format!("{} of the old host's packages", to_install.len());
    if !renamed.is_empty() {
        detail.push_str(&format!("; renamed: {}", renamed.join(", ")));
    }
    if !machine.is_empty() {
        detail.push_str(&format!(
            "; {} left to this machine (kernel, boot, network)",
            machine.len()
        ));
    }
    if !lists.ok() {
        detail.push_str(&format!(
            "; `apt-get update` failed ({}), so this is from older lists",
            lists.why()
        ));
    }
    let lacking = (!missing.is_empty()).then(|| {
        format!(
            "; not on this release, install what replaces them: {}",
            missing.join(", ")
        )
    });
    if !install {
        return match lacking {
            Some(lacking) => l.manual(ITEM, format!("would install {detail}{lacking}")),
            None => l.ok(ITEM, format!("would install {detail}")),
        };
    }
    let mut query = vec![
        "dpkg-query",
        "-W",
        "-f=${Package} ${db:Status-Status}\\n",
        "--",
    ];
    query.extend(to_install.iter().map(String::as_str));
    let have = run(host, &long(&query, 5 * 60)).stdout;
    let installed: Vec<&str> = have
        .lines()
        .filter_map(|line| line.strip_suffix(" installed"))
        .collect();
    let need: Vec<&str> = to_install
        .iter()
        .map(String::as_str)
        .filter(|name| !installed.contains(name))
        .collect();
    if !need.is_empty() {
        let mut args = vec![
            "install",
            "-y",
            "-o",
            "Dpkg::Options::=--force-confdef",
            "-o",
            "Dpkg::Options::=--force-confold",
            "--",
        ];
        args.extend(&need);
        let out = run(host, &Cmd::apt(&args));
        if !out.ok() {
            return l.failed(
                ITEM,
                format!(
                    "`apt-get install` of {} packages failed: {}",
                    need.len(),
                    out.why()
                ),
            );
        }
    }
    let detail = if need.is_empty() {
        format!("{detail} are installed")
    } else {
        format!("installed {} of {detail}", need.len())
    };
    match (lacking, need.is_empty()) {
        (Some(lacking), _) => l.manual(ITEM, format!("{detail}{lacking}")),
        (None, true) => l.ok(ITEM, detail),
        (None, false) => l.changed(ITEM, detail),
    }
}

fn coreutils(host: &mut dyn Host, wanted: &str, l: &mut Lines) {
    const ITEM: &str = "coreutils";
    let here = profile::coreutils(host);
    if wanted.is_empty() || here == wanted {
        return l.ok(
            ITEM,
            format!("{} coreutils, as on the old host", here_or_unknown(&here)),
        );
    }
    // Ubuntu's selector packages (25.10 and later); another release has one flavour only.
    let (select, drop) = match wanted {
        "GNU" => ("coreutils-from-gnu", "coreutils-from-uutils-"),
        "uutils" => ("coreutils-from-uutils", "coreutils-from-gnu-"),
        _ => {
            return l.manual(
                ITEM,
                format!("the old host has {wanted} coreutils, this one {here}"),
            );
        }
    };
    if candidates(&sh(host, &["apt-cache", "policy", "--", select]).stdout).is_empty() {
        return l.manual(
            ITEM,
            format!(
                "the old host has {wanted} coreutils, this one {} and no {select} package",
                here_or_unknown(&here)
            ),
        );
    }
    let out = run(
        host,
        &Cmd::apt(&["install", "-y", "--allow-remove-essential", select, drop]),
    );
    let now = profile::coreutils(host);
    if !out.ok() || now != wanted {
        return l.failed(
            ITEM,
            format!(
                "`apt-get install {select}` did not make coreutils {wanted}: {}",
                out.why()
            ),
        );
    }
    l.changed(
        ITEM,
        format!("installed {select}: {wanted} coreutils, as on the old host"),
    );
}

fn here_or_unknown(flavour: &str) -> &str {
    if flavour.is_empty() {
        "unknown"
    } else {
        flavour
    }
}

fn npm(host: &mut dyn Host, wanted: &[String], l: &mut Lines) {
    const ITEM: &str = "npm packages";
    if wanted.is_empty() {
        return;
    }
    if !sh(host, &["npm", "--version"]).ok() {
        return l.failed(ITEM, "npm is not installed on the new host");
    }
    let have = profile::npm_packages(&sh(host, &["npm", "ls", "-g", "--depth=0", "--json"]).stdout);
    let need: Vec<&str> = wanted
        .iter()
        .filter(|p| !have.contains(p))
        .map(String::as_str)
        .collect();
    if need.is_empty() {
        return l.ok(
            ITEM,
            format!(
                "{} global packages at the old host's versions",
                wanted.len()
            ),
        );
    }
    let mut argv = vec!["npm", "install", "-g", "--"];
    argv.extend(&need);
    let out = run(host, &Cmd::new(&argv).shown(Duration::from_secs(15 * 60)));
    if out.ok() {
        l.changed(ITEM, format!("installed {}", need.join(", ")));
    } else {
        l.failed(ITEM, format!("`npm install -g` failed: {}", out.why()));
    }
}

/// The real [`Env::setup`]: `host setup`'s own items on this host.
pub fn host_setup_part(host: &mut dyn Host, part: Part, setup: &Setup) -> Vec<HostLine> {
    let options = host_setup::Options {
        data_dir: setup.data_dir.clone(),
        user: setup.user.clone(),
        public_url: None,
        exe: setup.exe.clone(),
        smoke: part == Part::AfterStart,
        service: setup.service.clone(),
        update_state: None,
    };
    let mut quiet = |_: &HostLine| {};
    let run = host_setup::Run::new(host, Mode::Setup, options, &mut quiet);
    match part {
        Part::Host => run.host_items(),
        Part::BeforeStart => run.before_start(),
        Part::AfterStart => run.after_start(),
    }
}

fn binary(host: &mut dyn Host, setup: &Setup, l: &mut Lines) {
    const ITEM: &str = "yulius build";
    let exe = setup.exe.display().to_string();
    if !plain(&setup.exe) {
        return l.failed(ITEM, format!("{exe:?} is not a plain absolute path"));
    }
    if !host.is_file(Path::new(STAGED)) {
        return l.failed(
            ITEM,
            format!("{STAGED} is missing: the copied build was removed"),
        );
    }
    if sh(host, &["cmp", "-s", "--", STAGED, &exe]).ok() {
        return l.ok(ITEM, format!("{exe} is the old host's build"));
    }
    if sh(host, &["systemctl", "is-active", &setup.service])
        .stdout
        .trim()
        == "active"
    {
        return l.failed(ITEM, format!("{} runs another build", setup.service));
    }
    let new = format!("{exe}.migrate-new");
    let installed = sh(
        host,
        &[
            "install", "-D", "-m", "0755", "-o", "root", "-g", "root", "--", STAGED, &new,
        ],
    );
    let moved = installed
        .ok()
        .then(|| sh(host, &["mv", "-f", "--", &new, &exe]));
    match moved {
        Some(out) if out.ok() => {
            l.changed(ITEM, format!("installed the old host's build at {exe}"))
        }
        Some(out) => l.failed(ITEM, format!("cannot install {exe}: {}", out.why())),
        None => l.failed(ITEM, format!("cannot install {exe}: {}", installed.why())),
    }
}

fn data_dir(host: &mut dyn Host, setup: &Setup, l: &mut Lines) {
    const ITEM: &str = "data directory";
    let dir = &setup.data_dir;
    let shown = dir.display().to_string();
    if !plain(dir) {
        return l.failed(ITEM, format!("{shown:?} is not a plain absolute path"));
    }
    if !is_dir(host, dir) {
        return l.ok(ITEM, format!("{shown} will be made by the copy"));
    }
    // What a copy never holds is this host's own (the sandbox base built before the copy).
    let names: Vec<String> = host
        .list(dir)
        .into_iter()
        .filter(|name| !crate::instance::walk::LEFT_OUT.contains(&name.as_str()))
        .collect();
    if names.iter().any(|name| name == INCOMPLETE) {
        for name in &names {
            let path = dir.join(name).display().to_string();
            let out = run(
                host,
                &long(&["rm", "-rf", "--one-file-system", "--", &path], 60 * 60),
            );
            if !out.ok() {
                return l.failed(ITEM, format!("cannot remove {path}: {}", out.why()));
            }
        }
        return l.changed(ITEM, format!("removed an unfinished copy from {shown}"));
    }
    if let Some(name) = names.iter().find(|name| !name.starts_with('.')) {
        return l.failed(
            ITEM,
            format!(
                "{shown} holds {name:?}: a move goes only into a new or empty data directory \
                 (after a rolled-back move, remove the copy that was started there)"
            ),
        );
    }
    let owner = host.stat(dir).map(|(uid, _)| uid);
    if owner == Some(0)
        && let Some(_uid) = uid_of(host, &setup.user)
    {
        let to = format!("{0}:{0}", setup.user);
        let chown = sh(host, &["chown", "--", &to, &shown]);
        let chmod = sh(host, &["chmod", "0700", "--", &shown]);
        if !chown.ok() || !chmod.ok() {
            return l.failed(
                ITEM,
                format!("cannot give {shown} to {}: {}", setup.user, chown.why()),
            );
        }
        return l.changed(ITEM, format!("gave {shown} to {}", setup.user));
    }
    l.ok(ITEM, format!("{shown} is empty"));
}

fn texts(host: &mut dyn Host, name: &str, files: &[TextFile], l: &mut Lines) {
    let mut wrote = Vec::new();
    for file in files {
        if !plain(&file.path) {
            return l.failed(
                name,
                format!("{:?} is not a plain absolute path", file.path),
            );
        }
        if host.read(&file.path).as_deref() == Some(&file.content) {
            continue;
        }
        if let Err(why) = host.write(&file.path, &file.content, file.mode & 0o7777) {
            return l.failed(name, format!("cannot write {}: {why}", file.path.display()));
        }
        wrote.push(file.path.display().to_string());
    }
    if wrote.iter().any(|path| path.starts_with(UNITS_DIR)) {
        let out = sh(host, &["systemctl", "daemon-reload"]);
        if !out.ok() {
            return l.failed(
                name,
                format!("`systemctl daemon-reload` failed: {}", out.why()),
            );
        }
    }
    if wrote.is_empty() {
        l.ok(name, format!("{} files as on the old host", files.len()));
    } else {
        l.changed(name, format!("wrote {}", wrote.join(", ")));
    }
}

fn proxy(host: &mut dyn Host, changed: bool, l: &mut Lines) {
    const ITEM: &str = "reverse proxy";
    let valid = run(
        host,
        &long(
            &[
                "caddy",
                "validate",
                "--config",
                profile::CADDYFILE,
                "--adapter",
                "caddyfile",
            ],
            60,
        ),
    );
    if !valid.ok() {
        return l.failed(
            ITEM,
            format!("Caddy refuses its configuration: {}", valid.why()),
        );
    }
    let state = |host: &mut dyn Host, verb: &str| {
        sh(host, &["systemctl", verb, "caddy"])
            .stdout
            .trim()
            .to_owned()
    };
    let was = state(host, "is-active") == "active" && state(host, "is-enabled") == "enabled";
    if was && !changed {
        return l.ok(ITEM, "Caddy runs with the carried configuration");
    }
    // Its drop-ins were carried as files.
    sh(host, &["systemctl", "daemon-reload"]);
    let enabled = sh(host, &["systemctl", "enable", "caddy"]);
    let started = sh(host, &["systemctl", "reload-or-restart", "caddy"]);
    if !enabled.ok() || !started.ok() {
        let why = if enabled.ok() {
            started.why()
        } else {
            enabled.why()
        };
        return l.failed(ITEM, format!("Caddy did not start: {why}"));
    }
    l.changed(
        ITEM,
        "Caddy validated the carried configuration and runs with it",
    );
}

/// The sandboxes whose package layer exists (`sysroots/<id>/upper` holds a dpkg database).
fn layered(host: &dyn Host, data_dir: &Path) -> Vec<String> {
    let sysroots = data_dir.join("sysroots");
    host.list(&sysroots)
        .into_iter()
        .filter(|id| host.is_file(&sysroots.join(id).join("upper/var/lib/dpkg/status")))
        .collect()
}

fn inventory(
    host: &mut dyn Host,
    env: &mut Env<'_>,
    manifest: &Manifest,
    old: &Inventory,
    release: &Release,
    reply: &mut Reply,
    l: &mut Lines,
) {
    const ITEM: &str = "copy proven";
    match (env.count)(manifest, old) {
        Ok(new) => {
            match crate::instance::inventory::difference(old, &new) {
                None => l.ok(
                    ITEM,
                    format!(
                        "{} entries and {} in {} folders, the same on both hosts",
                        new.entries,
                        yulius_import::human_bytes(new.bytes),
                        new.folders.len()
                    ),
                ),
                Some(why) => l.failed(ITEM, format!("the copy differs from the old host: {why}")),
            }
            reply.inventory = Some(new);
        }
        Err(why) => l.failed(ITEM, format!("the copy cannot be counted: {why}")),
    }
    let here = Release::parse(&host.read(Path::new("/etc/os-release")).unwrap_or_default());
    let ids = layered(host, Path::new(&manifest.data_dir));
    if here != *release && !ids.is_empty() {
        let shown: Vec<&str> = ids.iter().take(8).map(String::as_str).collect();
        let more = ids.len().saturating_sub(shown.len());
        l.manual(
            "sandbox packages",
            format!(
                "{} sandboxes hold packages installed on {}; they keep their files on {}, \
                 reinstall in a sandbox what misbehaves: {}{}",
                ids.len(),
                release.name(),
                here.name(),
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ),
        );
    }
}

fn mark(host: &mut dyn Host, setup: &Setup, from: Option<&str>, to: &str, l: &mut Lines) {
    const ITEM: &str = "move mark";
    let path = setup.data_dir.join(MOVE_PENDING);
    if host.is_file(&setup.data_dir.join(MOVED)) || host.is_file(&path) {
        return l.ok(ITEM, "the copy is marked as moved");
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let text = serde_json::json!({"version": 1, "from": from, "to": to, "at": at}).to_string();
    if let Err(why) = host.write(&path, &text, 0o600) {
        return l.failed(ITEM, format!("cannot write {}: {why}", path.display()));
    }
    let owner = format!("{0}:{0}", setup.user);
    let out = sh(host, &["chown", "--", &owner, &path.display().to_string()]);
    if !out.ok() {
        return l.failed(
            ITEM,
            format!("cannot give the mark to {}: {}", setup.user, out.why()),
        );
    }
    l.changed(ITEM, "the first start brings back the tasks that were live");
}

fn start(
    host: &mut dyn Host,
    env: &mut Env<'_>,
    setup: &Setup,
    build: Option<&str>,
    health: &str,
    helper: Option<&str>,
    l: &mut Lines,
) {
    const ITEM: &str = "new instance";
    let before = (env.setup)(host, Part::BeforeStart, setup);
    let ready = host_setup::passed(&before);
    l.0.extend(before);
    if !ready {
        return l.failed(ITEM, "not started: the host is not ready (see above)");
    }
    let service = setup.service.as_str();
    // Units carried as files (the helper's, a drop-in) are known only after a reload.
    for argv in [
        &["systemctl", "daemon-reload"][..],
        &["systemctl", "enable", service][..],
        &["systemctl", "start", service][..],
    ] {
        let out = sh(host, argv);
        if !out.ok() {
            return l.failed(ITEM, format!("`{}` failed: {}", argv.join(" "), out.why()));
        }
    }
    let mut waited = Duration::ZERO;
    let reported = loop {
        match (env.health)(health) {
            Some(commit) if build.is_none_or(|b| b == commit) => break Some(commit),
            _ if waited >= HEALTH_WAIT => break None,
            _ => {
                (env.sleep)(Duration::from_secs(1));
                waited += Duration::from_secs(1);
            }
        }
    };
    let Some(commit) = reported else {
        return l.failed(
            ITEM,
            format!(
                "{service} did not report the old host's build within {} s (`journalctl -u \
                 {service}` says why)",
                HEALTH_WAIT.as_secs()
            ),
        );
    };
    l.changed(
        ITEM,
        format!(
            "{service} runs build {}",
            commit.get(..7).unwrap_or(&commit)
        ),
    );
    if let Some(unit) = helper {
        let out = sh(host, &["systemctl", "enable", "--now", unit]);
        if out.ok() {
            l.changed("update helper", format!("{unit} is enabled"));
        } else {
            l.failed(
                "update helper",
                format!("`systemctl enable --now {unit}` failed: {}", out.why()),
            );
        }
    }
    let after = (env.setup)(host, Part::AfterStart, setup);
    l.0.extend(after);
}

fn stop(host: &mut dyn Host, service: &str, l: &mut Lines) {
    const ITEM: &str = "new instance stopped";
    let out = sh(host, &["systemctl", "disable", "--now", service]);
    // KillMode=process leaves agents and terminals: nothing of the copy may keep running.
    sh(
        host,
        &[
            "systemctl",
            "kill",
            "--kill-whom=all",
            "--signal=SIGKILL",
            service,
        ],
    );
    if out.ok() {
        l.changed(
            ITEM,
            format!("{service} is stopped and disabled on the new host"),
        );
    } else {
        l.failed(
            ITEM,
            format!("`systemctl disable --now {service}` failed: {}", out.why()),
        );
    }
}

fn tasks(host: &mut dyn Host, env: &mut Env<'_>, data_dir: &Path, wait_secs: u64, l: &mut Lines) {
    const ITEM: &str = "tasks";
    let mut waited = 0;
    while host.is_file(&data_dir.join(MOVE_PENDING)) && waited < wait_secs {
        (env.sleep)(Duration::from_secs(1));
        waited += 1;
    }
    let Some(report) = host
        .read(&data_dir.join(MOVED))
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return l.manual(
            ITEM,
            "the new instance has not said yet what it did with the tasks that were live; \
             tasks left stopped are continued by their owners",
        );
    };
    let count = |key: &str| {
        report
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let (queued, left, failed) = (count("queued"), count("left"), count("failed"));
    let mut detail = format!("{queued} tasks that were live are back in the queue");
    if left > 0 {
        detail.push_str(&format!(
            "; {left} a person had paused or was stopping stay stopped"
        ));
    }
    if failed > 0 {
        l.manual(
            ITEM,
            format!(
                "{detail}; {failed} could not be queued and wait for their owners under Needs you"
            ),
        );
    } else {
        l.ok(ITEM, detail);
    }
}

fn finish(host: &mut dyn Host, key: &str, firewall: bool, l: &mut Lines) {
    let keys = Path::new("/root/.ssh/authorized_keys");
    let text = host.read(keys).unwrap_or_default();
    let ours = |line: &str| !key.is_empty() && line.trim_end().ends_with(key);
    let kept: String = text
        .lines()
        .filter(|line| !ours(line))
        .map(|line| format!("{line}\n"))
        .collect();
    if !text.lines().any(ours) {
        l.ok("one-time key", "not in the new host's authorized_keys");
    } else {
        match host.write(keys, &kept, 0o600) {
            Ok(()) => l.changed(
                "one-time key",
                "removed from the new host's authorized_keys",
            ),
            Err(why) => l.failed(
                "one-time key",
                format!("cannot write {}: {why}", keys.display()),
            ),
        }
    }
    if firewall {
        let out = sh(host, &["ufw", "--force", "enable"]);
        if out.ok() {
            l.changed("firewall", "ufw is enabled with the old host's rules");
        } else {
            l.failed(
                "firewall",
                format!("`ufw --force enable` failed: {}", out.why()),
            );
        }
    }
    sh(host, &["rm", "-rf", "--", STATE_DIR]);
}

/// Runs `request` on this host.
pub fn handle(host: &mut dyn Host, env: &mut Env<'_>, request: &Request) -> Reply {
    let mut reply = Reply::default();
    let mut l = Lines::default();
    match request {
        Request::Facts {
            data_dir,
            service,
            ports,
        } => reply.facts = Some(facts(host, env, data_dir, service, ports)),
        Request::Disk { setup, disks } => disk(host, setup, disks, &mut l),
        Request::Sources { files } => sources(host, files, &mut l),
        Request::Packages { names, install } => packages(host, names, *install, &mut l),
        Request::Coreutils { flavour } => coreutils(host, flavour, &mut l),
        Request::Npm { packages } => npm(host, packages, &mut l),
        Request::HostItems { setup } => l.0 = (env.setup)(host, Part::Host, setup),
        Request::Binary { setup } => binary(host, setup, &mut l),
        Request::DataDir { setup } => data_dir(host, setup, &mut l),
        Request::BeforeStart { setup } => l.0 = (env.setup)(host, Part::BeforeStart, setup),
        Request::Texts { name, files } => texts(host, name, files, &mut l),
        Request::Proxy { changed } => proxy(host, *changed, &mut l),
        Request::Expect(expect) => {
            let text = serde_json::to_string(expect).unwrap_or_default();
            if let Err(why) = host.write(Path::new(EXPECT_FILE), &text, 0o600) {
                l.failed("copy", format!("cannot write {EXPECT_FILE}: {why}"));
            }
        }
        Request::Inventory {
            manifest,
            old,
            release,
        } => inventory(host, env, manifest, old, release, &mut reply, &mut l),
        Request::Mark { setup, from, to } => mark(host, setup, from.as_deref(), to, &mut l),
        Request::Discard { data_dir } => {
            let marker = data_dir.join(INCOMPLETE);
            match host.write(&marker, "", 0o600) {
                Ok(()) => l.changed(
                    "copy",
                    "marked unfinished on the new host: the next run removes it and copies again",
                ),
                Err(why) => l.failed("copy", format!("cannot write {}: {why}", marker.display())),
            }
        }
        Request::Start {
            setup,
            build,
            health,
            helper,
        } => start(
            host,
            env,
            setup,
            build.as_deref(),
            health,
            helper.as_deref(),
            &mut l,
        ),
        Request::Stop { service } => stop(host, service, &mut l),
        Request::Tasks {
            data_dir,
            wait_secs,
        } => tasks(host, env, data_dir, *wait_secs, &mut l),
        Request::Finish { key, firewall } => finish(host, key, *firewall, &mut l),
    }
    reply.lines = l.0;
    reply
}

/// The commit the server listening at `address` reports on `/internal/version`.
#[must_use]
pub fn reported_commit(address: &str) -> Option<String> {
    use std::io::{Read as _, Write as _};
    let address: std::net::SocketAddr = address.parse().ok()?;
    let limit = Duration::from_secs(3);
    let mut stream = std::net::TcpStream::connect_timeout(&address, limit).ok()?;
    stream.set_read_timeout(Some(limit)).ok()?;
    stream.set_write_timeout(Some(limit)).ok()?;
    stream
        .write_all(b"GET /internal/version HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .ok()?;
    let mut answer = String::new();
    stream.take(64 * 1024).read_to_string(&mut answer).ok()?;
    let (head, body) = answer.split_once("\r\n\r\n")?;
    head.lines().next()?.contains(" 200 ").then_some(())?;
    let info: yulius_contracts::update::VersionInfo = serde_json::from_str(body.trim()).ok()?;
    info.commit
}

/// Installs the copy arriving on standard input as [`EXPECT_FILE`] says, with the import's own
/// checks, and reports it.
#[must_use]
pub fn receive() -> Reply {
    const ITEM: &str = "copy";
    let mut l = Lines::default();
    let expect = std::fs::read_to_string(EXPECT_FILE)
        .ok()
        .and_then(|text| serde_json::from_str::<Expect>(&text).ok());
    let Some(expect) = expect else {
        l.failed(
            ITEM,
            format!("{EXPECT_FILE} does not say where the copy goes"),
        );
        return Reply {
            lines: l.0,
            ..Reply::default()
        };
    };
    let mut unpack = None;
    let input: Box<dyn std::io::Read> = if expect.zstd {
        let child = std::process::Command::new("zstd")
            .args(["-d", "-q", "-c"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::piped())
            .spawn();
        match child {
            Ok(mut child) => {
                let out = child.stdout.take();
                unpack = Some(child);
                match out {
                    Some(out) => Box::new(out),
                    None => Box::new(std::io::empty()),
                }
            }
            Err(e) => {
                l.failed(ITEM, format!("cannot run zstd: {e}"));
                return Reply {
                    lines: l.0,
                    ..Reply::default()
                };
            }
        }
    } else {
        Box::new(std::io::stdin().lock())
    };
    let mut progress: Option<crate::instance::Progress> = None;
    let result = crate::instance::import::install(
        std::io::BufReader::with_capacity(1 << 20, input),
        &expect.data_dir,
        None,
        &mut |m, target| crate::instance::import::prepare_host(m, target, None),
        true,
        &mut |m, done| {
            progress
                .get_or_insert_with(|| crate::instance::Progress::new("Imported", m.total_bytes))
                .tick(done);
        },
    );
    if let Some(p) = &progress {
        p.end();
    }
    let unpacked = unpack.is_none_or(|mut child| child.wait().is_ok_and(|s| s.success()));
    match result {
        Ok(_) if !unpacked => l.failed(ITEM, "zstd reported a damaged stream"),
        Ok((m, report)) => {
            let mut detail = format!(
                "installed {} entries, {}, checksum and length as announced",
                report.entries,
                yulius_import::human_bytes(m.total_bytes)
            );
            if report.foreign_owners > 0 {
                detail.push_str(&format!(
                    "; {} entries of other owners now belong to the service user",
                    report.foreign_owners
                ));
            }
            if report.skipped_xattrs > 0 {
                detail.push_str(&format!(
                    "; {} extended attributes not restored",
                    report.skipped_xattrs
                ));
            }
            l.changed(ITEM, detail);
        }
        Err(
            crate::instance::import::Failure::Refused(why)
            | crate::instance::import::Failure::Failed(why),
        ) => l.failed(ITEM, why),
    }
    Reply {
        lines: l.0,
        ..Reply::default()
    }
}

/// `yulius instance migrate-target`: one request from standard input, or with `receive` the
/// copy; the reply is the last line of standard output.
#[must_use]
pub fn main(receive_copy: bool) -> std::process::ExitCode {
    use std::io::Read as _;
    if !crate::instance::is_root() {
        eprintln!("yulius instance migrate-target: a move's own command; it needs root");
        return std::process::ExitCode::from(crate::instance::REFUSED);
    }
    let reply = if receive_copy {
        receive()
    } else {
        let mut text = String::new();
        let read = std::io::stdin()
            .lock()
            .take(64 << 20)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())
            .and_then(|_| serde_json::from_str::<Request>(&text).map_err(|e| e.to_string()));
        let request = match read {
            Ok(request) => request,
            Err(why) => {
                eprintln!("yulius instance migrate-target: not a request: {why}");
                return std::process::ExitCode::from(crate::instance::REFUSED);
            }
        };
        let mut host = host_setup::system::RealHost { quiet: false };
        let mut env = Env {
            address: std::env::var("SSH_CONNECTION")
                .ok()
                .and_then(|c| c.split_whitespace().nth(2).map(str::to_owned)),
            health: &mut |address| reported_commit(address),
            sleep: &mut |d| std::thread::sleep(d),
            count: &mut |manifest, old| {
                let map = crate::instance::import::id_map(manifest, &manifest.service.user)?;
                crate::instance::inventory::of_copy(Path::new(&manifest.data_dir), &map, old)
                    .map_err(|e| e.to_string())
            },
            setup: &mut host_setup_part,
        };
        handle(&mut host, &mut env, &request)
    };
    match serde_json::to_string(&reply) {
        Ok(json) => {
            println!("{REPLY}{json}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("yulius instance migrate-target: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
