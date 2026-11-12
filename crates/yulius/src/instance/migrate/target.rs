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
use crate::instance::import::{INCOMPLETE, Mark};
use crate::instance::inventory::Inventory;

/// The flow's own directory on both hosts (root, 0700).
pub const STATE_DIR: &str = "/var/lib/yulius-migrate";
/// The build the old host copied to the new one.
pub const STAGED: &str = "/var/lib/yulius-migrate/yulius";
/// The finished checklist between the step that brings it and the Finish ([`Carried`]).
pub const CARRY_FILE: &str = "/var/lib/yulius-migrate/checklist.json";
/// The finished checklist as the Finish leaves it for the new instance's server
/// (`yulius_contracts::migrate::ARRIVED_FILE` in the state directory).
pub const ARRIVED: &str = "/var/lib/yulius-migrate/arrived.json";
/// What the next copy is to be installed as ([`Expect`]).
pub const EXPECT_FILE: &str = "/var/lib/yulius-migrate/expect.json";
/// Written by [`Request::Stop`]: this copy was stopped by the move. A Start that is still
/// running here (its connection or its caller died) finds it and never starts the unit.
pub const STOPPED: &str = "/var/lib/yulius-migrate/stopped";
/// The lock files of [`Hold`].
const HOLD_FILES: [&str; 3] = ["flight.lock", "switch.lock", "receive.lock"];
/// How long a look at the instance's health waits for a Start that is still in flight, and a
/// look at the staged copy's marker for a stream that is being closed.
const FLIGHT_WAIT: Duration = Duration::from_secs(30 * 60);
/// Written beside the lock of a stream being received: the old host went on without that
/// stream, so its receiver closes it instead of waiting on a link that may be gone.
const RECEIVE_STOP: &str = "receive.stop";
/// How long a stream waits for one before it to be closed.
const RECEIVE_WAIT: Duration = Duration::from_secs(120);
/// How often a receive that gets no bytes asks whether it was told to stop.
const RECEIVE_LOOK: Duration = Duration::from_secs(1);
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

/// The finished checklist on its way to the new instance (SPEC-470 MVB-011); kept in
/// [`CARRY_FILE`] between the step that brings it and the Finish that completes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carried {
    /// The move and its lines up to the Finish step.
    pub checklist: yulius_contracts::migrate::MoveChecklist,
    /// The lines the old host adds after the Finish step.
    pub then: Vec<HostLine>,
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
    /// Make the data directory ready for a copy: empty, or holding the staged copy `stage`.
    DataDir {
        /// The instance.
        setup: Setup,
        /// The staged copy this move goes on with; any other unfinished copy is removed.
        #[serde(default)]
        stage: Option<String>,
    },
    /// What the staged copy's marker says ([`Reply::stage`]), once no stream is being received.
    Stage {
        /// The data directory.
        data_dir: PathBuf,
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
        manifest: Box<Manifest>,
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
    /// Whether the instance reports `build` right now ([`Reply::started`]).
    Health {
        /// The commit it must report.
        build: Option<String>,
        /// The address of its internal listener.
        health: String,
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
    /// Whether GitHub accepts the running instance's sign-in callback, as one "GitHub App" line
    /// (SPEC-466 MVC-022).
    Callback {
        /// The data directory, for the server's key.
        data_dir: PathBuf,
        /// The address of its internal listener.
        address: String,
        /// The instance's public URL, for the note when it cannot be asked.
        public_url: String,
    },
    /// Stage the finished checklist for the new instance's page; the Finish step completes it
    /// and leaves it in this directory as [`ARRIVED`] (SPEC-470 MVB-011).
    Carry(Carried),
    /// Enable a unit the old host had enabled, once the instance runs: the move helper's path
    /// unit (SPEC-470 MVB-008).
    Enable {
        /// The line's name.
        name: String,
        /// The unit.
        unit: String,
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
    /// [`Request::Start`] and [`Request::Health`]: the instance reports the build, so it has
    /// taken over.
    #[serde(default)]
    pub started: bool,
    /// [`Request::Stage`]'s answer: the staged copy's marker, `None` when there is none.
    #[serde(default)]
    pub stage: Option<Mark>,
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

/// What steps of one move exclude each other with on this host: a Start can outlive the
/// connection that asked for it, and must not start the unit after a Stop said it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// A Start, from its beginning to its end.
    Flight,
    /// The moment a Start starts the unit, and a whole Stop.
    Switch,
    /// Nothing held: waits (bounded) until no Start is in flight.
    Landed,
    /// Nothing held: tells a stream that is still being received ([`receiving`]) to close and
    /// waits (bounded) until it has, so the marker read next is what the last stream left.
    Received,
}

/// What a step needs beside the host: this process and its clock.
pub struct Env<'a> {
    /// The address the caller connected to (`SSH_CONNECTION`).
    pub address: Option<String>,
    /// The port it connected to.
    pub ssh_port: Option<u16>,
    /// The address it connected from (the old host).
    pub peer: Option<String>,
    /// Takes a [`Hold`]; it is released when the returned value is dropped.
    pub hold: &'a mut dyn FnMut(Hold) -> Box<dyn std::any::Any>,
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

/// The staged copy `data_dir` holds, by its marker.
fn staged(host: &dyn Host, data_dir: &Path) -> Option<Mark> {
    Mark::parse(&host.read(&data_dir.join(INCOMPLETE))?)
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
    if !(1..=2).contains(&disks.len()) || disks.first() == disks.get(1) {
        return l.failed(ITEM, "name one disk, or two different ones for a mirror");
    }
    // What an earlier run that was cut already made is taken up, not refused as "not blank":
    // the labelled filesystem, or the array without one.
    let labelled = sh(host, &["blkid", "-L", DISK_LABEL]);
    let labelled =
        Some(labelled.stdout.trim().to_owned()).filter(|d| labelled.ok() && plain(Path::new(d)));
    let array = if disks.len() == 2 {
        sh(host, &["mdadm", "--detail", MIRROR])
    } else {
        Out {
            code: 1,
            ..Out::default()
        }
    };
    let mirrored = array.ok();
    if mirrored && labelled.is_none() {
        // Only the array a cut run made of exactly these disks, with nothing on it yet
        // (`wipefs` lists every signature; it must run and find none).
        let members: Vec<&str> = array.stdout.split_whitespace().collect();
        let ours = disks.iter().all(|d| members.contains(&d.as_str()));
        let found = sh(host, &["wipefs", "--no-act", MIRROR]);
        if !ours || !found.ok() || !found.stdout.trim().is_empty() {
            return l.failed(
                ITEM,
                format!(
                    "{MIRROR} exists and is not an empty mirror of {} and {}; nothing was \
                     changed",
                    disks[0], disks[1]
                ),
            );
        }
    }
    let blank = list_blank(host);
    if labelled.is_none()
        && !mirrored
        && let Some(taken) = disks.iter().find(|d| !blank.iter().any(|b| &b.path == *d))
    {
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
        let device = if let Some(device) = labelled.clone() {
            device
        } else if mirrored {
            MIRROR.to_owned()
        } else if let [a, b] = disks {
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
        if labelled.is_none() {
            sh(host, &["udevadm", "settle"]);
            step(
                host,
                &long(&["mkfs.ext4", "-q", "-L", DISK_LABEL, &device], 30 * 60),
            )?;
        }
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
        let line = format!("UUID={uuid} {data} ext4 defaults,nofail 0 2");
        if !fstab.lines().any(|l| l.trim() == line) {
            if !fstab.is_empty() && !fstab.ends_with('\n') {
                fstab.push('\n');
            }
            fstab.push_str(&line);
            fstab.push('\n');
            host.write(Path::new("/etc/fstab"), &fstab, 0o644)
                .map_err(|why| format!("cannot write /etc/fstab: {why}"))?;
        }
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
        .filter(|n| package_name(n) && !profile::FLAVOUR_PACKAGES.contains(&n.as_str()))
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

fn data_dir(host: &mut dyn Host, setup: &Setup, stage: Option<&str>, l: &mut Lines) {
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
        // This move's own staged copy is what the next pass builds on.
        if let (Some(mark), Some(stage)) = (staged(host, dir), stage)
            && mark.stage == stage
        {
            return l.ok(
                ITEM,
                format!(
                    "{shown} holds this move's staged copy ({} passes acknowledged)",
                    mark.acked
                ),
            );
        }
        // The marker last: until everything else is gone, what is there stays an unfinished
        // copy for the next run to remove.
        let marker_last = names
            .iter()
            .filter(|name| *name != INCOMPLETE)
            .chain(std::iter::once(&INCOMPLETE.to_owned()))
            .cloned()
            .collect::<Vec<_>>();
        for name in &marker_last {
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
    // As the import: only a skeleton's dotfiles may be there.
    if let Some(name) = names
        .iter()
        .find(|name| !(name.starts_with('.') && host.is_file(&dir.join(name))))
    {
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
        if let Err(why) = profile::write_keeping_owner(host, &file.path, &file.content, file.mode) {
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
    let unproven = setup.data_dir.join(INCOMPLETE);
    if host.is_file(&path) && !host.is_file(&unproven) {
        return l.ok(ITEM, "the copy is marked as moved");
    }
    // An earlier move's report came with the copy: this move writes its own. And a copy that
    // is proven anew is not the one an earlier attempt stopped.
    let report = setup.data_dir.join(MOVED).display().to_string();
    sh(host, &["rm", "-f", "--", &report, STOPPED]);
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
    // Proven on the old host: the copy is no longer unfinished, nor a staged copy.
    let rewritten = setup.data_dir.join(format!(".{INCOMPLETE}.tmp"));
    let out = sh(
        host,
        &[
            "rm",
            "-f",
            "--",
            &unproven.display().to_string(),
            &rewritten.display().to_string(),
        ],
    );
    if !out.ok() || host.is_file(&unproven) {
        return l.failed(
            ITEM,
            format!("cannot remove {}: {}", unproven.display(), out.why()),
        );
    }
    l.changed(ITEM, "the first start brings back the tasks that were live");
}

/// Starts the instance; whether it reports the build (it has taken over). What fails after
/// that is said and changes nothing of that.
fn start(
    host: &mut dyn Host,
    env: &mut Env<'_>,
    setup: &Setup,
    build: Option<&str>,
    health: &str,
    helper: Option<&str>,
    l: &mut Lines,
) -> bool {
    const ITEM: &str = "new instance";
    let _flight = (env.hold)(Hold::Flight);
    if host.is_file(&setup.data_dir.join(INCOMPLETE)) {
        l.failed(
            ITEM,
            "not started: the copy is not proven (it is marked unfinished)",
        );
        return false;
    }
    let before = (env.setup)(host, Part::BeforeStart, setup);
    let ready = host_setup::passed(&before);
    l.0.extend(before);
    if !ready {
        l.failed(ITEM, "not started: the host is not ready (see above)");
        return false;
    }
    let service = setup.service.as_str();
    {
        // With a Stop excluded: either it came first and left its word, or it comes after and
        // stops what is started here.
        let _switch = (env.hold)(Hold::Switch);
        if host.is_file(Path::new(STOPPED)) {
            l.failed(ITEM, "not started: the move stopped this copy meanwhile");
            return false;
        }
        // Units carried as files (the helper's, a drop-in) are known only after a reload.
        for argv in [
            &["systemctl", "daemon-reload"][..],
            &["systemctl", "enable", service][..],
            &["systemctl", "start", service][..],
        ] {
            let out = sh(host, argv);
            if !out.ok() {
                l.failed(ITEM, format!("`{}` failed: {}", argv.join(" "), out.why()));
                return false;
            }
        }
    }
    // Bounded by the clock and by the number of looks, whichever ends first.
    let deadline = std::time::Instant::now() + HEALTH_WAIT;
    let mut looks = 0;
    let reported = loop {
        match (env.health)(health) {
            Some(commit) if build.is_none_or(|b| b == commit) => break Some(commit),
            _ if looks >= HEALTH_WAIT.as_secs() || std::time::Instant::now() >= deadline => {
                break None;
            }
            _ => {
                (env.sleep)(Duration::from_secs(1));
                looks += 1;
            }
        }
    };
    let Some(commit) = reported else {
        l.failed(
            ITEM,
            format!(
                "{service} did not report the old host's build within {} s (`journalctl -u \
                 {service}` says why)",
                HEALTH_WAIT.as_secs()
            ),
        );
        return false;
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
    true
}

fn stop(host: &mut dyn Host, env: &mut Env<'_>, service: &str, l: &mut Lines) {
    const ITEM: &str = "new instance stopped";
    let _switch = (env.hold)(Hold::Switch);
    // First: a Start still in flight here must find it before it starts the unit.
    if let Err(why) = host.write(Path::new(STOPPED), "", 0o600) {
        return l.failed(ITEM, format!("cannot write {STOPPED}: {why}"));
    }
    let out = sh(host, &["systemctl", "disable", "--now", service]);
    // KillMode=process leaves agents and terminals: nothing of the copy may keep running.
    // (`--kill-whom` is systemd 252 and later, `--kill-who` its older name.)
    let kill = |host: &mut dyn Host, unit: &str| {
        for whom in ["--kill-whom=all", "--kill-who=all"] {
            if sh(host, &["systemctl", "kill", whom, "--signal=SIGKILL", unit]).ok() {
                break;
            }
        }
    };
    kill(host, service);
    // Where the host keeps sessions in a unit of their own (SPEC-458, which this build's host
    // setup writes), a stop of the server's unit leaves every agent, terminal and engine
    // running: that unit ends with it. A systemd that does not say whether the unit is there is
    // not a host without one: it is ended as far as that goes, and the step fails.
    let mut unanswered = false;
    let sessions = yulius_sandbox::units::sessions_unit(service).filter(|sessions| {
        let shown = sh(
            host,
            &[
                "systemctl",
                "show",
                "--property=LoadState",
                "--value",
                sessions,
            ],
        );
        unanswered = !shown.ok();
        unanswered || shown.stdout.trim() == "loaded"
    });
    if let Some(sessions) = &sessions {
        kill(host, sessions);
        let _ = sh(host, &["systemctl", "stop", sessions]);
    }
    let group_of = |host: &mut dyn Host, unit: &str| {
        let shown = sh(
            host,
            &[
                "systemctl",
                "show",
                "--property=ControlGroup",
                "--value",
                unit,
            ],
        );
        let name = shown.stdout.trim().trim_start_matches('/').to_owned();
        (!name.is_empty()).then(|| Path::new("/sys/fs/cgroup").join(name))
    };
    let group = group_of(host, service);
    let sessions_group = sessions.as_deref().and_then(|unit| group_of(host, unit));
    // Whether a process lives in the group or below it. A delegated unit keeps its processes
    // in subgroups (`DelegateSubgroup=server`, the sandboxes'), so its own `cgroup.procs` is
    // empty by construction: `populated` covers them all.
    let populated = |host: &mut dyn Host, group: &Option<PathBuf>| {
        group.as_ref().is_some_and(|group| {
            host.read(&group.join("cgroup.events"))
                .is_some_and(|events| events.lines().any(|l| l.trim() == "populated 1"))
        })
    };
    // What still runs: the first unit of the two that is active or whose group holds a process.
    let running = |host: &mut dyn Host| -> Option<String> {
        let active = |host: &mut dyn Host, unit: &str| {
            sh(host, &["systemctl", "is-active", unit]).stdout.trim() == "active"
        };
        if active(host, service)
            || group.as_ref().is_some_and(|group| {
                host.read(&group.join("cgroup.procs"))
                    .is_some_and(|pids| !pids.trim().is_empty())
            })
            || populated(host, &group)
        {
            return Some(service.to_owned());
        }
        let sessions = sessions.as_deref()?;
        (active(host, sessions) || populated(host, &sessions_group)).then(|| sessions.to_owned())
    };
    let mut waited = 0;
    while running(host).is_some() && waited < 15 {
        (env.sleep)(Duration::from_secs(1));
        waited += 1;
    }
    if !out.ok() {
        l.failed(
            ITEM,
            format!("`systemctl disable --now {service}` failed: {}", out.why()),
        );
    } else if let Some(unit) = running(host) {
        l.failed(
            ITEM,
            format!("processes of {unit} still run on the new host after it was killed"),
        );
    } else if let (true, Some(sessions)) = (unanswered, &sessions) {
        l.failed(
            ITEM,
            format!(
                "systemd does not say whether {sessions} is there, so nobody knows that nothing \
                 of the new instance runs: stop it on the new host (`systemctl stop {sessions}`)"
            ),
        );
    } else {
        l.changed(
            ITEM,
            format!("{service} is stopped and disabled on the new host, nothing of it runs"),
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

/// Enables and starts the path unit `unit` (SPEC-470 MVB-008); nothing but a unit's name is
/// given to `systemctl`.
fn enable(host: &mut dyn Host, name: &str, unit: &str, l: &mut Lines) {
    let named = unit
        .strip_suffix(".path")
        .is_some_and(|stem| host_setup::service_name_ok(&format!("{stem}.service")));
    if !named {
        return l.failed(name, "not enabled: the old host named no path unit");
    }
    let says = |host: &mut dyn Host, verb: &str| {
        sh(host, &["systemctl", verb, unit])
            .stdout
            .trim()
            .to_owned()
    };
    if says(host, "is-enabled") == "enabled" && says(host, "is-active") == "active" {
        return l.ok(name, format!("{unit} is enabled"));
    }
    let out = sh(host, &["systemctl", "enable", "--now", unit]);
    if out.ok() {
        l.changed(name, format!("{unit} is enabled"));
    } else {
        l.failed(
            name,
            format!("`systemctl enable --now {unit}` failed: {}", out.why()),
        );
    }
}

fn finish(host: &mut dyn Host, env: &Env<'_>, key: &str, firewall: bool, l: &mut Lines) {
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
        // The old host's rules may not know how the move reaches this host: the old host, and
        // nobody else, keeps its way in.
        let port = env.ssh_port.zip(
            env.peer
                .as_deref()
                .filter(|peer| peer.parse::<std::net::IpAddr>().is_ok()),
        );
        let port = port.map(|(port, peer)| {
            let port = port.to_string();
            sh(
                host,
                &[
                    "ufw", "allow", "from", peer, "to", "any", "port", &port, "proto", "tcp",
                ],
            );
            format!("SSH from {peer} to port {port}")
        });
        let out = sh(host, &["ufw", "--force", "enable"]);
        if out.ok() {
            l.changed(
                "firewall",
                match port {
                    Some(way) => format!(
                        "ufw is enabled with the old host's rules, and {way} (the move's way \
                         in) allowed"
                    ),
                    None => "ufw is enabled with the old host's rules".to_owned(),
                },
            );
        } else {
            l.failed(
                "firewall",
                format!("`ufw --force enable` failed: {}", out.why()),
            );
        }
    }
    // Read before the flow's files go, left after: what an earlier move left goes with them.
    let staged = host.read(Path::new(CARRY_FILE));
    if host.is_file(Path::new(STOPPED)) {
        // A Start may still be in flight: what tells it not to start the unit stays.
        for name in host.list(Path::new(STATE_DIR)) {
            if name != "stopped" && !HOLD_FILES.contains(&name.as_str()) {
                let path = Path::new(STATE_DIR).join(name).display().to_string();
                sh(host, &["rm", "-rf", "--", &path]);
            }
        }
    } else {
        sh(host, &["rm", "-rf", "--", STATE_DIR]);
    }
    if let Some(staged) = staged {
        leave_checklist(host, &staged, l);
    }
}

/// Keeps the checklist the old host brings until the Finish (SPEC-470 MVB-011).
fn carry(host: &mut dyn Host, carried: &Carried, l: &mut Lines) {
    let kept = serde_json::to_string(carried)
        .map_err(|e| e.to_string())
        .and_then(|json| host.write(Path::new(CARRY_FILE), &json, 0o600));
    if let Err(why) = kept {
        l.manual("checklist", not_left(&why));
    }
}

fn not_left(why: &str) -> String {
    format!(
        "could not be left for Settings → Instance on the new host ({why}); `yulius instance \
         migrate --status` on the old host has it"
    )
}

/// Completes the staged checklist with this step's own lines and leaves it for the new
/// instance's server (SPEC-470 MVB-011), in root's own directory: the data directory belongs
/// to the user of an instance that already runs, and root writes nothing there on its word.
fn leave_checklist(host: &mut dyn Host, staged: &str, l: &mut Lines) {
    const ITEM: &str = "checklist";
    let Carried {
        mut checklist,
        then,
    } = match serde_json::from_str(staged) {
        Ok(carried) => carried,
        Err(e) => {
            return l.manual(
                ITEM,
                not_left(&format!("what the old host brought is not one: {e}")),
            );
        }
    };
    checklist
        .lines
        .extend(l.0.iter().cloned().map(super::shown));
    checklist.lines.extend(then);
    let left = serde_json::to_string(&checklist)
        .map_err(|e| e.to_string())
        .and_then(|json| host.write(Path::new(ARRIVED), &json, 0o644));
    if let Err(why) = left {
        l.manual(ITEM, not_left(&format!("cannot write {ARRIVED}: {why}")));
    }
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
        Request::DataDir { setup, stage } => data_dir(host, setup, stage.as_deref(), &mut l),
        Request::Stage { data_dir } => {
            // A stream cut on the old host may still be taken in here.
            drop((env.hold)(Hold::Received));
            reply.stage = staged(host, data_dir);
        }
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
        Request::Health { build, health } => {
            // A Start whose answer was lost may still be on its way to starting the unit.
            drop((env.hold)(Hold::Landed));
            reply.started = (env.health)(health)
                .is_some_and(|commit| build.as_ref().is_none_or(|b| *b == commit));
        }
        Request::Start {
            setup,
            build,
            health,
            helper,
        } => {
            reply.started = start(
                host,
                env,
                setup,
                build.as_deref(),
                health,
                helper.as_deref(),
                &mut l,
            );
        }
        Request::Stop { service } => stop(host, env, service, &mut l),
        Request::Tasks {
            data_dir,
            wait_secs,
        } => tasks(host, env, data_dir, *wait_secs, &mut l),
        Request::Callback {
            data_dir,
            address,
            public_url,
        } => {
            // The server of this host answers, over its loopback listener.
            let asked = super::github::asked_instance(data_dir, address, None);
            let (outcome, detail) = super::github::line(asked, public_url);
            l.push("GitHub App", outcome, detail);
        }
        Request::Carry(carried) => carry(host, carried, &mut l),
        Request::Enable { name, unit } => enable(host, name, unit, &mut l),
        Request::Finish { key, firewall } => finish(host, env, key, *firewall, &mut l),
    }
    reply.lines = l.0;
    reply
}

/// The real [`Env::hold`]: [`hold_in`] the state directory.
fn real_hold(hold: Hold) -> Box<dyn std::any::Any> {
    hold_in(Path::new(STATE_DIR), hold, FLIGHT_WAIT)
}

/// Takes `hold` with `flock` on files in `dir`; it is released when the returned value is
/// dropped. [`Hold::Landed`] and [`Hold::Received`] wait at most `landing` for a Start in
/// flight or a stream being received and hold nothing. A lock that cannot be taken (no such
/// directory) holds nothing: the steps then run as before.
pub fn hold_in(dir: &Path, hold: Hold, landing: Duration) -> Box<dyn std::any::Any> {
    use rustix::fs::{FlockOperation, flock};
    let open = |name: &str| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(name))
            .ok()
    };
    let (name, wait) = match hold {
        Hold::Flight => (HOLD_FILES[0], false),
        Hold::Switch => (HOLD_FILES[1], false),
        Hold::Landed => (HOLD_FILES[0], true),
        Hold::Received => (HOLD_FILES[2], true),
    };
    if wait {
        if let Some(file) = open(name) {
            let deadline = std::time::Instant::now() + landing;
            let stop = dir.join(RECEIVE_STOP);
            let mut told = false;
            let mut free = flock(&file, FlockOperation::NonBlockingLockShared).is_ok();
            while !free && std::time::Instant::now() < deadline {
                if hold == Hold::Received && !told {
                    // One run at a time on the old host: whoever asks, that stream's sender
                    // is gone.
                    told = std::fs::write(&stop, b"").is_ok();
                    eprintln!(
                        "yulius instance migrate-target: a stream is still being taken in \
                         here; it is closed first"
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
                free = flock(&file, FlockOperation::NonBlockingLockShared).is_ok();
            }
            // Heard: taken back. A receive that still runs stays told.
            if told && free {
                let _ = std::fs::remove_file(&stop);
            }
        }
        return Box::new(());
    }
    let file = open(name);
    if let Some(file) = &file {
        let _ = flock(file, FlockOperation::LockExclusive);
    }
    Box::new(file)
}

/// A stream being received, from its first byte until its marker is written.
#[derive(Debug)]
pub struct Receiving {
    _lock: std::fs::File,
    stop: PathBuf,
}

impl Receiving {
    /// Whether the old host went on without this stream ([`Hold::Received`]).
    #[must_use]
    pub fn told_to_stop(&self) -> bool {
        self.stop.exists()
    }

    /// The file whose presence says so.
    #[must_use]
    pub fn stop_file(&self) -> PathBuf {
        self.stop.clone()
    }
}

/// Begins to receive a stream in the state directory `dir`: one at a time, waiting at most
/// `wait` for a stream before it to be closed.
///
/// # Errors
/// Another stream is still being received, or the lock cannot be made.
pub fn receiving(dir: &Path, wait: Duration) -> Result<Receiving, String> {
    use rustix::fs::{FlockOperation, flock};
    let path = dir.join(HOLD_FILES[2]);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let deadline = std::time::Instant::now() + wait;
    while flock(&lock, FlockOperation::NonBlockingLockExclusive).is_err() {
        if std::time::Instant::now() >= deadline {
            return Err(
                "another stream is still being taken in on the new host: run the command again \
                 in a few minutes"
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let stop = dir.join(RECEIVE_STOP);
    // What an earlier look left (it was cut before it took its word back) is not for this
    // stream.
    let _ = std::fs::remove_file(&stop);
    Ok(Receiving { _lock: lock, stop })
}

/// A stream's bytes, read on a thread of their own, so a receive whose sender is gone can be
/// told to close the stream: a link that breaks leaves the new host's end open for hours, and
/// what arrived counts only once the stream is closed and marked (SPEC-465 WCP-011).
pub struct Watched {
    chunks: std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    at: usize,
    ended: bool,
    every: Duration,
    stopped: Box<dyn FnMut() -> bool + Send>,
}

impl std::fmt::Debug for Watched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watched")
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl Watched {
    /// Reads `input`; when no bytes come for `every`, asks `stopped` whether to give up.
    pub fn new(
        mut input: impl std::io::Read + Send + 'static,
        every: Duration,
        stopped: impl FnMut() -> bool + Send + 'static,
    ) -> Self {
        let (send, chunks) = std::sync::mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let read = match input.read(&mut buf) {
                    Ok(n) => Ok(buf[..n].to_vec()),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => Err(e),
                };
                let last = read.as_ref().map_or(true, Vec::is_empty);
                if send.send(read).is_err() || last {
                    break;
                }
            }
        });
        Self {
            chunks,
            chunk: Vec::new(),
            at: 0,
            ended: false,
            every,
            stopped: Box::new(stopped),
        }
    }
}

impl std::io::Read for Watched {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            if self.at < self.chunk.len() {
                let n = out.len().min(self.chunk.len() - self.at);
                out[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            if self.ended || out.is_empty() {
                return Ok(0);
            }
            match self.chunks.recv_timeout(self.every) {
                Ok(Ok(chunk)) => {
                    self.ended = chunk.is_empty();
                    self.chunk = chunk;
                    self.at = 0;
                }
                Ok(Err(e)) => {
                    self.ended = true;
                    return Err(e);
                }
                Err(RecvTimeoutError::Disconnected) => self.ended = true,
                Err(RecvTimeoutError::Timeout) => {
                    if (self.stopped)() {
                        self.ended = true;
                        return Err(std::io::Error::other(
                            "the old host went on without the rest of this stream",
                        ));
                    }
                }
            }
        }
    }
}

/// Lets this process hold as many open files as the host allows: removing a tree, and walking
/// one while the instance runs, holds one directory open for each level it is deep.
pub(super) fn raise_open_files() {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    if let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) {
        let wanted = hard.min(1 << 16);
        if soft < wanted {
            let _ = setrlimit(Resource::RLIMIT_NOFILE, wanted, hard);
        }
    }
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

/// Takes the pass arriving on standard input into the staged copy [`EXPECT_FILE`] names, with
/// the import's own checks, and reports it. The copy stays marked unfinished: its marker is the
/// stage, until the old host has proven the copy ([`Request::Mark`] removes it), so a run that
/// dies in between leaves nothing that looks like an instance.
#[must_use]
pub fn receive() -> Reply {
    use crate::instance::import::{Failure, Ground, land, prepare_host};
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
    // One stream at a time, and none unseen by a look at the marker.
    let receiving = match receiving(Path::new(STATE_DIR), RECEIVE_WAIT) {
        Ok(receiving) => receiving,
        Err(why) => {
            l.failed(ITEM, why);
            return Reply {
                lines: l.0,
                ..Reply::default()
            };
        }
    };
    raise_open_files();
    let stop = receiving.stop_file();
    let watched = |input: Box<dyn std::io::Read + Send>| {
        Watched::new(input, RECEIVE_LOOK, move || stop.exists())
    };
    let mut unpack = None;
    let input = if expect.zstd {
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
                    Some(out) => watched(Box::new(out)),
                    None => watched(Box::new(std::io::empty())),
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
        watched(Box::new(std::io::stdin()))
    };
    let mut progress: Option<crate::instance::Progress> = None;
    let result = land(
        input,
        &expect.data_dir,
        None,
        &mut |m, target| prepare_host(m, target, None, true),
        &mut Ground {
            chown: true,
            sync: &mut |dir| crate::instance::export::sync(dir),
            tick: &mut |m, done| {
                progress
                    .get_or_insert_with(|| {
                        crate::instance::Progress::new("Installed", m.total_bytes)
                    })
                    .tick(done);
            },
        },
    );
    if let Some(p) = &progress {
        p.end();
    }
    let unpacked = unpack.is_none_or(|mut child| {
        // A stream that did not end by itself leaves it waiting on the link.
        if result.is_err() {
            let _ = child.kill();
        }
        child.wait().is_ok_and(|s| s.success())
    });
    let mut reply = Reply::default();
    match result {
        Ok(landing) => {
            if unpacked {
                l.changed(ITEM, landed(&landing));
            } else {
                l.failed(ITEM, "zstd reported a damaged stream");
            }
            reply.stage = Some(landing.mark);
        }
        Err(Failure::Refused(why) | Failure::Failed(why)) => l.failed(ITEM, why),
    }
    reply.lines = l.0;
    reply
}

/// What a pass left, for the checklist.
#[must_use]
pub fn landed(landing: &crate::instance::import::Landing) -> String {
    let last = landing.mark.last.clone().unwrap_or_default();
    let waiting = last.deferred.len() as u64;
    let mut detail = if last.live && waiting > 0 {
        // What was taken in and left out is not among the installed.
        format!(
            "pass {}: {} of {} entries installed and synced",
            landing.mark.acked,
            last.entries.saturating_sub(waiting),
            last.entries
        )
    } else if last.live {
        format!(
            "pass {}: {} entries installed and synced",
            landing.mark.acked, last.entries
        )
    } else {
        format!(
            "{} changed entries installed, {} removed that the old host no longer holds; \
             checksum and length as announced",
            last.entries, landing.removed
        )
    };
    if !last.deferred.is_empty() {
        detail.push_str(&format!(
            "; {} wait for the freeze (a directory is in their way)",
            last.deferred.len()
        ));
    }
    if landing.report.foreign_owners > 0 {
        detail.push_str(&format!(
            "; {} entries of other owners now belong to the service user",
            landing.report.foreign_owners
        ));
    }
    if landing.report.skipped_xattrs > 0 {
        detail.push_str(&format!(
            "; {} extended attributes not restored",
            landing.report.skipped_xattrs
        ));
    }
    detail
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
            ssh_port: std::env::var("SSH_CONNECTION")
                .ok()
                .and_then(|c| c.split_whitespace().nth(3)?.parse().ok()),
            peer: std::env::var("SSH_CONNECTION")
                .ok()
                .and_then(|c| c.split_whitespace().next().map(str::to_owned)),
            hold: &mut real_hold,
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
