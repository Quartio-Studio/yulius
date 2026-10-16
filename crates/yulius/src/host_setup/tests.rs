//! `yulius host setup` and `check` against a fake host (SPEC-203 acceptance criteria).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use yulius_contracts::update::{HostLine, HostOutcome};
use yulius_sandbox::docker_status::{ENGINE_MODULES, MODULES_FILE, modules_file_content};

use super::{
    Cmd, DOCKER_CE, DOCKER_SOURCE, Host, Mode, NEEDRESTART_RULE, NEEDRESTART_RULE_FILE, Options,
    Out, Run, passed, summary,
};

/// A unit's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unit {
    enabled: bool,
    active: bool,
}

/// A simulated Ubuntu 24.04 host: files, dpkg, systemd units, kernel modules, users.
#[derive(Debug, Clone)]
struct Fake {
    files: BTreeMap<PathBuf, (String, u32, u32)>,
    packages: BTreeSet<String>,
    units: BTreeMap<String, Unit>,
    loaded: BTreeSet<String>,
    containers: usize,
    /// Commands run, as one string each.
    log: Vec<String>,
    /// `apt-get update` exits 100 (a broken third-party source).
    broken_source: bool,
    /// `modprobe` fails.
    modprobe_fails: bool,
}

impl Fake {
    /// A fresh Ubuntu 24.04 VM: needrestart, no Docker, nothing loaded.
    fn fresh() -> Self {
        let mut fake = Self {
            files: BTreeMap::new(),
            packages: BTreeSet::new(),
            units: BTreeMap::new(),
            loaded: BTreeSet::new(),
            containers: 0,
            log: Vec::new(),
            broken_source: false,
            modprobe_fails: false,
        };
        fake.file(
            "/etc/os-release",
            "PRETTY_NAME=\"Ubuntu 24.04.5 LTS\"\nID=ubuntu\nVERSION_CODENAME=noble\n",
        );
        fake.file("/etc/needrestart/needrestart.conf", "# defaults\n");
        fake.file("/proc/sys/kernel/osrelease", "6.8.0-139-generic\n");
        fake.file(
            "/etc/apt/sources.list.d/ubuntu.sources",
            "URIs: http://archive.ubuntu.com\n",
        );
        fake
    }

    /// The host after a setup: what [`Fake::fresh`] gets from it.
    fn set_up() -> Self {
        let mut fake = Self::fresh();
        let opts = options();
        Run::new(&mut fake, Mode::Setup, opts, &mut |_| {}).all();
        fake.log.clear();
        fake
    }

    fn file(&mut self, path: &str, content: &str) {
        self.files
            .insert(PathBuf::from(path), (content.to_owned(), 0, 0o100_644));
    }

    fn has(&self, path: &str) -> bool {
        self.files.contains_key(Path::new(path))
    }

    fn text(&self, path: &str) -> Option<&str> {
        self.files.get(Path::new(path)).map(|(t, _, _)| t.as_str())
    }

    fn install(&mut self, package: &str) {
        self.packages.insert(package.to_owned());
        match package {
            "docker-ce" => {
                self.file("/usr/bin/dockerd", "");
                for unit in ["docker.socket", "docker.service"] {
                    self.units.insert(
                        unit.into(),
                        Unit {
                            enabled: true,
                            active: true,
                        },
                    );
                }
            }
            "containerd.io" => {
                self.units.insert(
                    "containerd.service".into(),
                    Unit {
                        enabled: true,
                        active: true,
                    },
                );
            }
            "docker.io" => self.file("/usr/sbin/dockerd", ""),
            _ => {}
        }
    }

    /// A reboot: modules are loaded from the modules-load.d files, enabled units start, others
    /// stay stopped.
    fn reboot(&mut self) {
        self.loaded.clear();
        let listed: Vec<String> = self
            .files
            .iter()
            .filter(|(p, _)| p.starts_with("/etc/modules-load.d"))
            .flat_map(|(_, (text, _, _))| {
                text.lines()
                    .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        self.loaded.extend(listed);
        for unit in self.units.values_mut() {
            unit.active = unit.enabled;
        }
    }

    fn ran(&self, prefix: &str) -> bool {
        self.log.iter().any(|c| c.starts_with(prefix))
    }

    fn installable(&self, package: &str) -> bool {
        let docker_repo = self
            .files
            .iter()
            .any(|(p, (t, _, _))| p.starts_with("/etc/apt") && t.contains("download.docker.com"));
        !DOCKER_CE.contains(&package) || docker_repo
    }
}

impl Host for Fake {
    fn read(&self, path: &Path) -> Option<String> {
        if path == Path::new("/proc/modules") {
            return Some(
                self.loaded
                    .iter()
                    .map(|m| format!("{m} 16384 0 - Live 0x0\n"))
                    .collect(),
            );
        }
        self.files.get(path).map(|(t, _, _)| t.clone())
    }

    fn write(&mut self, path: &Path, content: &str, mode: u32) -> Result<(), String> {
        self.log.push(format!("write {}", path.display()));
        self.files.insert(
            path.to_path_buf(),
            (content.to_owned(), 0, 0o100_000 | mode),
        );
        Ok(())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        self.files
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
            .collect()
    }

    fn stat(&self, path: &Path) -> Option<(u32, u32)> {
        self.files.get(path).map(|(_, uid, mode)| (*uid, *mode))
    }

    fn own_by_root(&mut self, path: &Path) -> Result<(), String> {
        self.log.push(format!("own {}", path.display()));
        let entry = self.files.get_mut(path).ok_or("missing")?;
        entry.1 = 0;
        entry.2 &= !0o022;
        Ok(())
    }

    fn run(&mut self, cmd: &Cmd) -> Result<Out, String> {
        let line = cmd.argv.join(" ");
        self.log.push(line.clone());
        let argv: Vec<&str> = cmd.argv.iter().map(String::as_str).collect();
        let ok = |stdout: &str| Out {
            code: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        };
        let fail = |code: i32, why: &str| Out {
            code,
            stdout: String::new(),
            stderr: why.to_owned(),
        };
        Ok(match argv.as_slice() {
            ["dpkg-query", "-W", "-f=${db:Status-Status}", package] => {
                if self.packages.contains(*package) {
                    ok("installed")
                } else {
                    fail(1, "no packages found")
                }
            }
            ["dpkg-query", "-W", "-f=${Version}", _] => ok("5:29.9.1-1~ubuntu.24.04~noble"),
            ["dpkg", "--print-architecture"] => ok("amd64\n"),
            ["apt-get", "update"] => {
                if self.broken_source {
                    fail(
                        100,
                        "E: The repository 'https://dl.cloudsmith.io' is not signed.",
                    )
                } else {
                    ok("")
                }
            }
            [
                "apt-get",
                "install",
                "-y",
                "--no-install-recommends",
                packages @ ..,
            ] => {
                assert!(
                    cmd.env
                        .iter()
                        .any(|(k, v)| k == "NEEDRESTART_SUSPEND" && v == "1"),
                    "apt runs with needrestart suspended"
                );
                if let Some(missing) = packages.iter().find(|p| !self.installable(p)) {
                    fail(100, &format!("E: Unable to locate package {missing}"))
                } else {
                    for package in packages.iter() {
                        self.install(package);
                    }
                    ok("")
                }
            }
            [
                "systemctl",
                "show",
                "yulius.service",
                "--property=Environment",
            ] => ok(self.text("/fake/yulius.env").unwrap_or("Environment=")),
            [
                "systemctl",
                "show",
                unit,
                "--property=LoadState,UnitFileState,ActiveState",
            ] => match self.units.get(*unit) {
                Some(u) => ok(&format!(
                    "LoadState=loaded\nUnitFileState={}\nActiveState={}\n",
                    if u.enabled { "enabled" } else { "disabled" },
                    if u.active { "active" } else { "inactive" }
                )),
                None => ok("LoadState=not-found\nUnitFileState=\nActiveState=inactive\n"),
            },
            ["systemctl", "disable", "--now", units @ ..] => {
                for unit in units.iter() {
                    if let Some(u) = self.units.get_mut(*unit) {
                        *u = Unit {
                            enabled: false,
                            active: false,
                        };
                    }
                }
                ok("")
            }
            ["docker", "ps", "--quiet"] => ok(&"abc123\n".repeat(self.containers)),
            ["install", "-m", "0755", "-d", _] | ["chmod", "a+r", _] => ok(""),
            ["curl", .., url] => {
                assert_eq!(*url, "https://download.docker.com/linux/ubuntu/gpg");
                self.file("/etc/apt/keyrings/docker.asc", "KEY");
                ok("")
            }
            ["modprobe", "-a", modules @ ..] => {
                if self.modprobe_fails {
                    fail(1, "modprobe: FATAL: Module overlay not found")
                } else {
                    self.loaded.extend(modules.iter().map(|m| (*m).to_owned()));
                    ok("")
                }
            }
            _ => panic!("unexpected command: {line}"),
        })
    }
}

fn options() -> Options {
    Options {
        data_dir: PathBuf::from("/var/lib/yulius"),
        user: "yulius".into(),
        public_url: Some("https://yulius.example.com".into()),
        exe: PathBuf::from("/usr/local/bin/yulius"),
        smoke: false,
    }
}

fn run(fake: &mut Fake, mode: Mode) -> Vec<HostLine> {
    Run::new(fake, mode, options(), &mut |_| {}).all()
}

fn line<'a>(lines: &'a [HostLine], name: &str) -> &'a HostLine {
    lines
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no line {name}: {lines:?}"))
}

fn outcome(lines: &[HostLine], name: &str) -> HostOutcome {
    line(lines, name).outcome
}

/// A fresh Ubuntu 24.04 host gets every change, the needrestart rule before the first install,
/// Docker from Docker's repository with buildx and compose, the host's units stopped.
#[test]
fn a_fresh_host_gets_every_change_in_order() {
    let mut fake = Fake::fresh();
    let lines = run(&mut fake, Mode::Setup);
    assert!(passed(&lines), "{lines:#?}");
    for name in [
        "needrestart",
        "isolation packages",
        "Docker Engine",
        "host Docker units",
        "kernel modules",
    ] {
        assert_eq!(
            outcome(&lines, name),
            HostOutcome::Changed,
            "{name}: {lines:#?}"
        );
    }
    let rule = fake
        .log
        .iter()
        .position(|c| c == &format!("write {NEEDRESTART_RULE_FILE}"));
    let first_install = fake
        .log
        .iter()
        .position(|c| c.starts_with("apt-get install"));
    assert!(rule.is_some() && rule < first_install, "{:#?}", fake.log);
    assert_eq!(fake.text(NEEDRESTART_RULE_FILE), Some(NEEDRESTART_RULE));
    for package in DOCKER_CE {
        assert!(fake.packages.contains(package), "{package}");
    }
    assert!(
        fake.text(DOCKER_SOURCE)
            .is_some_and(|t| t.contains("Suites: noble")
                && t.contains("URIs: https://download.docker.com/linux/ubuntu")
                && t.contains("Architectures: amd64"))
    );
    for unit in fake.units.values() {
        assert_eq!(
            *unit,
            super::tests::Unit {
                enabled: false,
                active: false
            }
        );
    }
    assert_eq!(
        fake.text(MODULES_FILE),
        Some(modules_file_content().as_str())
    );
    assert_eq!(fake.loaded.len(), ENGINE_MODULES.len());
    assert!(
        fake.ran("docker ps"),
        "the daemon the install started is asked for containers"
    );
}

/// HST-002: the second run changes nothing and says so.
#[test]
fn a_set_up_host_changes_nothing() {
    let mut fake = Fake::set_up();
    let lines = run(&mut fake, Mode::Setup);
    assert!(
        lines.iter().all(|l| l.outcome == HostOutcome::Ok),
        "{lines:#?}"
    );
    assert!(summary(&lines, Mode::Setup).starts_with("nothing to change"));
    assert!(!fake.log.iter().any(|c| c.starts_with("write")
        || c.starts_with("apt-get")
        || c.starts_with("systemctl disable")
        || c.starts_with("modprobe")));
}

/// A partly set-up host (the reference VM: Docker installed from Docker's repository by hand
/// with a one-line `docker.list`, the host units left running) changes only what is missing.
#[test]
fn a_partly_set_up_host_changes_only_what_is_missing() {
    let mut fake = Fake::fresh();
    fake.file(
        "/etc/apt/sources.list.d/docker.list",
        "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu noble stable\n",
    );
    for package in DOCKER_CE
        .iter()
        .chain(["uidmap", "slirp4netns", "bubblewrap"].iter())
    {
        fake.install(package);
    }
    fake.file(MODULES_FILE, &modules_file_content());
    fake.loaded
        .extend(ENGINE_MODULES.iter().map(|m| (*m).to_owned()));
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(outcome(&lines, "Docker Engine"), HostOutcome::Ok);
    assert_eq!(outcome(&lines, "kernel modules"), HostOutcome::Ok);
    assert_eq!(outcome(&lines, "isolation packages"), HostOutcome::Ok);
    assert_eq!(outcome(&lines, "needrestart"), HostOutcome::Changed);
    assert_eq!(outcome(&lines, "host Docker units"), HostOutcome::Changed);
    assert!(!fake.has(DOCKER_SOURCE), "no second Docker source");
    assert!(!fake.ran("apt-get"), "{:#?}", fake.log);
    assert!(fake.ran("systemctl disable --now docker.socket docker.service containerd.service"));
    assert!(
        fake.ran("docker ps"),
        "a running host Docker is asked for containers"
    );
}

/// HST-004: a host Docker that runs containers is someone's: left running, said plainly.
#[test]
fn a_host_docker_with_containers_is_left_alone() {
    let mut fake = Fake::set_up();
    for unit in fake.units.values_mut() {
        *unit = Unit {
            enabled: true,
            active: true,
        };
    }
    fake.containers = 2;
    let lines = run(&mut fake, Mode::Setup);
    let units = line(&lines, "host Docker units");
    assert_eq!(units.outcome, HostOutcome::Manual);
    assert!(units.detail.contains("2 container(s)"), "{}", units.detail);
    assert!(!fake.ran("systemctl disable"));
    assert!(passed(&lines), "manual is not a failure");

    // Only the socket listens: asking would start the daemon, so it is disabled unasked.
    fake.units.insert(
        "docker.service".into(),
        Unit {
            enabled: true,
            active: false,
        },
    );
    fake.log.clear();
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(outcome(&lines, "host Docker units"), HostOutcome::Changed);
    assert!(!fake.ran("docker ps"), "{:#?}", fake.log);
}

/// HST-005: missing modules are loaded; a modules file that differs is rewritten; a failed
/// modprobe fails the item.
#[test]
fn missing_modules_are_written_and_loaded() {
    let mut fake = Fake::set_up();
    fake.loaded.remove("veth");
    fake.file(MODULES_FILE, "overlay\n");
    let lines = run(&mut fake, Mode::Setup);
    let modules = line(&lines, "kernel modules");
    assert_eq!(modules.outcome, HostOutcome::Changed);
    assert!(modules.detail.contains("loaded veth"), "{}", modules.detail);
    assert!(modules.detail.contains("wrote"), "{}", modules.detail);
    assert!(fake.ran("modprobe -a veth"));
    assert_eq!(
        fake.text(MODULES_FILE),
        Some(modules_file_content().as_str())
    );

    fake.loaded.remove("overlay");
    fake.modprobe_fails = true;
    let lines = run(&mut fake, Mode::Setup);
    let modules = line(&lines, "kernel modules");
    assert_eq!(modules.outcome, HostOutcome::Failed);
    assert!(modules.detail.contains("overlay"), "{}", modules.detail);
    assert!(!passed(&lines));
}

/// HST-003: Ubuntu's docker.io stays; only its missing plugins are installed. A broken apt source
/// does not stop the install.
#[test]
fn ubuntus_docker_io_is_kept_with_its_plugins() {
    let mut fake = Fake::fresh();
    for package in ["docker.io", "uidmap", "slirp4netns", "bubblewrap"] {
        fake.install(package);
    }
    fake.broken_source = true;
    let lines = run(&mut fake, Mode::Setup);
    let docker = line(&lines, "Docker Engine");
    assert_eq!(docker.outcome, HostOutcome::Changed, "{lines:#?}");
    assert!(
        docker.detail.contains("docker-buildx docker-compose-v2"),
        "{}",
        docker.detail
    );
    assert!(
        docker.detail.contains("apt-get update` failed"),
        "{}",
        docker.detail
    );
    assert!(!fake.packages.contains("docker-ce"));
    let again = run(&mut fake, Mode::Setup);
    assert!(
        line(&again, "Docker Engine")
            .detail
            .contains("Ubuntu's docker.io")
    );
}

/// Docker turned off for sandboxes installs no Docker.
#[test]
fn docker_off_installs_no_docker() {
    let mut fake = Fake::fresh();
    fake.file(
        "/fake/yulius.env",
        "Environment=YULIUS_DATA=/var/lib/yulius YULIUS_SANDBOX_DOCKER=off\n",
    );
    let lines = run(&mut fake, Mode::Setup);
    assert!(
        line(&lines, "Docker Engine")
            .detail
            .contains("YULIUS_SANDBOX_DOCKER=off")
    );
    assert!(!fake.packages.contains("docker-ce"));
}

/// HST-012: a check applies only the safe fixes and reports the rest as needing setup.
#[test]
fn a_check_applies_only_safe_fixes() {
    let mut fake = Fake::fresh();
    let lines = run(&mut fake, Mode::Check);
    assert!(!fake.ran("apt-get"), "{:#?}", fake.log);
    assert_eq!(outcome(&lines, "needrestart"), HostOutcome::Changed);
    assert_eq!(outcome(&lines, "kernel modules"), HostOutcome::Changed);
    for name in ["isolation packages", "Docker Engine"] {
        let l = line(&lines, name);
        assert_eq!(l.outcome, HostOutcome::Failed, "{name}");
        assert!(l.detail.contains("sudo yulius host setup"), "{}", l.detail);
    }

    // A Docker upgrade re-enabled the host's units: the check disables them again.
    let mut fake = Fake::set_up();
    fake.units.insert(
        "docker.service".into(),
        Unit {
            enabled: true,
            active: true,
        },
    );
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(outcome(&lines, "host Docker units"), HostOutcome::Changed);
    assert!(passed(&lines), "{lines:#?}");
}

/// A dry run changes nothing and says what it would.
#[test]
fn a_dry_run_changes_nothing() {
    let mut fake = Fake::fresh();
    let before = fake.files.clone();
    let lines = run(&mut fake, Mode::DryRun);
    assert_eq!(fake.files, before);
    assert!(!fake.ran("apt-get") && !fake.ran("modprobe") && !fake.ran("systemctl disable"));
    assert!(
        line(&lines, "Docker Engine")
            .detail
            .starts_with("would install")
    );
    assert!(summary(&lines, Mode::DryRun).starts_with("would change"));
}

/// HST-018: after a reboot (modules loaded from the file, enabled units started) a setup changes
/// nothing: everything a reboot needs persisted.
#[test]
fn a_rebooted_host_changes_nothing() {
    let mut fake = Fake::set_up();
    fake.reboot();
    assert_eq!(
        fake.loaded.len(),
        ENGINE_MODULES.len(),
        "the modules file loads them all"
    );
    let lines = run(&mut fake, Mode::Setup);
    assert!(
        lines.iter().all(|l| l.outcome == HostOutcome::Ok),
        "{lines:#?}"
    );
}
