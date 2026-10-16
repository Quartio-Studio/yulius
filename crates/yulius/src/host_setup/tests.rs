//! `yulius host setup` and `check` against a fake host (SPEC-203 acceptance criteria).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use yulius_contracts::update::{HostLine, HostOutcome};
use yulius_sandbox::docker_status::{ENGINE_MODULES, MODULES_FILE, modules_file_content};

use super::{
    Cmd, DOCKER_CE, DOCKER_SOURCE, HELPER_DROP_IN, Host, Mode, NEEDRESTART_RULE,
    NEEDRESTART_RULE_FILE, Options, Out, Run, UNIT_DROP_IN, UNIT_FILE, free_sub_range, passed,
    summary,
};

const DATA: &str = "/var/lib/yulius";
const EXE: &str = "/usr/local/bin/yulius";
const SERVICE_UID: u32 = 999;

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
    /// `yulius-update.service` is installed.
    helper: bool,
    /// `yulius.service` runs.
    service_active: bool,
    /// The smoke test fails.
    smoke_fails: bool,
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
            helper: true,
            service_active: false,
            smoke_fails: false,
        };
        fake.files
            .insert(PathBuf::from(EXE), (String::new(), 0, 0o100_755));
        fake.file(
            "/etc/passwd",
            "root:x:0:0:root:/root:/bin/bash\nubuntu:x:1000:1000::/home/ubuntu:/bin/bash\n",
        );
        fake.file("/etc/subuid", "ubuntu:100000:65536\n");
        fake.file("/etc/subgid", "ubuntu:100000:65536\n");
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

    fn fingerprint(&self) -> String {
        self.packages.iter().cloned().collect::<Vec<_>>().join(",")
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
            [
                "useradd",
                "--system",
                "--create-home",
                "--home-dir",
                home,
                "--shell",
                _,
                user,
            ] => {
                let passwd = self.text("/etc/passwd").unwrap_or_default().to_owned();
                self.file(
                    "/etc/passwd",
                    &format!(
                        "{passwd}{user}:x:{SERVICE_UID}:{SERVICE_UID}::{home}:/usr/sbin/nologin\n"
                    ),
                );
                self.files.insert(
                    PathBuf::from(*home),
                    (String::new(), SERVICE_UID, 0o040_755),
                );
                ok("")
            }
            ["install", "-d", "-m", "0700", "-o", _, "-g", _, dir] => {
                self.files
                    .insert(PathBuf::from(*dir), (String::new(), SERVICE_UID, 0o040_700));
                ok("")
            }
            ["chmod", "0700", dir] => {
                if let Some(entry) = self.files.get_mut(Path::new(dir)) {
                    entry.2 = 0o040_700;
                }
                ok("")
            }
            ["usermod", rest @ ..] => {
                let user = rest.last().copied().unwrap_or_default();
                for pair in rest.chunks(2).filter(|c| c.len() == 2) {
                    let file = match pair[0] {
                        "--add-subuids" => "/etc/subuid",
                        "--add-subgids" => "/etc/subgid",
                        _ => continue,
                    };
                    let (start, end) = pair[1].split_once('-').unwrap_or_default();
                    let count =
                        end.parse::<u32>().unwrap_or(0) - start.parse::<u32>().unwrap_or(0) + 1;
                    let text = self.text(file).unwrap_or_default().to_owned();
                    self.file(file, &format!("{text}{user}:{start}:{count}\n"));
                }
                ok("")
            }
            ["systemctl", "show", "yulius.service", props] if props.contains("Delegate") => {
                let unit = self
                    .text(UNIT_FILE)
                    .map(str::to_owned)
                    .or_else(|| self.text("/fake/unit").map(str::to_owned));
                match unit {
                    None => ok("LoadState=not-found\n"),
                    Some(text) => {
                        let delegated = text.contains("Delegate=yes") || self.has(UNIT_DROP_IN);
                        ok(&format!(
                            "LoadState=loaded\nDelegate={}\nDelegateSubgroup={}\nKillMode={}\nActiveState={}\nExecStart={{ path={EXE} ; argv[]={EXE} serve ; ignore_errors=no }}\n",
                            if delegated { "yes" } else { "no" },
                            if delegated { "server" } else { "" },
                            if delegated {
                                "process"
                            } else {
                                "control-group"
                            },
                            if self.service_active {
                                "active"
                            } else {
                                "inactive"
                            },
                        ))
                    }
                }
            }
            ["systemctl", "show", "yulius-update.service", _] => {
                if self.helper {
                    let extra = if self.has(HELPER_DROP_IN) {
                        " /etc/modules-load.d -/etc/needrestart/conf.d"
                    } else {
                        ""
                    };
                    ok(&format!(
                        "LoadState=loaded\nReadWritePaths=/var/lib/yulius-update /var/lib/yulius /usr/local/bin{extra}\n"
                    ))
                } else {
                    ok("LoadState=not-found\nReadWritePaths=\n")
                }
            }
            ["systemctl", "daemon-reload"] | ["systemctl", "enable", "yulius.service"] => ok(""),
            ["systemctl", "start", "yulius.service"] => {
                self.service_active = true;
                ok("")
            }
            [
                "runuser",
                "-u",
                user,
                "--",
                exe,
                "sandbox-base",
                "build",
                "--data-dir",
                data,
            ] => {
                assert_eq!((*user, *exe, *data), ("yulius", EXE, DATA));
                assert!(cmd.show, "the build's progress is shown");
                let n = self
                    .files
                    .keys()
                    .filter(|p| p.starts_with("/var/lib/yulius/system") && p.ends_with("marker"))
                    .count()
                    + 1;
                self.file(&format!("{DATA}/system/base-{n}/marker"), "");
                if self.has("/usr/bin/dockerd") {
                    self.file(&format!("{DATA}/system/base-{n}/usr/bin/dockerd"), "");
                }
                self.file(
                    &format!("{DATA}/system/current.json"),
                    &format!(
                        r#"{{"id":"base-{n}","fingerprint":"{}","built_at_ms":1,"format":{}}}"#,
                        self.fingerprint(),
                        yulius_sandbox::base::BASE_FORMAT
                    ),
                );
                ok("")
            }
            [
                "runuser",
                "-u",
                "yulius",
                "--",
                EXE,
                "doctor",
                "--contained",
                "--data-dir",
                DATA,
            ] => ok(""),
            [EXE, "doctor", "--docker-smoke", "--data-dir", DATA] => {
                if self.smoke_fails {
                    fail(1, "FAIL docker run --rm hello-world")
                } else {
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
        exe: PathBuf::from(EXE),
        smoke: true,
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

/// HST-007/HST-008/HST-011: a fresh host gets the service user, its ids, the unit (started after
/// the base is built as the service user), and the verdict, doctor and smoke pass.
#[test]
fn a_fresh_host_gets_the_service_and_a_base_with_docker() {
    let mut fake = Fake::fresh();
    let lines = run(&mut fake, Mode::Setup);
    for name in [
        "service user",
        "subordinate ids",
        "yulius.service",
        "update helper",
        "sandbox base",
        "yulius running",
    ] {
        assert_eq!(
            outcome(&lines, name),
            HostOutcome::Changed,
            "{name}: {lines:#?}"
        );
    }
    for name in [
        "sandbox Docker",
        "contained doctor",
        "Docker smoke test",
        "yulius binary",
    ] {
        assert_eq!(outcome(&lines, name), HostOutcome::Ok, "{name}: {lines:#?}");
    }
    assert!(
        fake.text("/etc/subuid")
            .is_some_and(|t| t.contains("yulius:165536:65536"))
    );
    let unit = fake.text(UNIT_FILE).unwrap_or_default();
    for needle in [
        "User=yulius",
        "YULIUS_PUBLIC_URL=https://yulius.example.com",
        "YULIUS_CONTAINED=true",
        "Delegate=yes",
        "DelegateSubgroup=server",
        "KillMode=process",
        "ExecStart=/usr/local/bin/yulius serve",
    ] {
        assert!(unit.contains(needle), "{needle}: {unit}");
    }
    let build = fake
        .log
        .iter()
        .position(|c| c.contains("sandbox-base build"));
    let docker = fake
        .log
        .iter()
        .position(|c| c.starts_with("apt-get install") && c.contains("docker-ce"));
    let start = fake
        .log
        .iter()
        .position(|c| c == "systemctl start yulius.service");
    assert!(docker < build && build < start, "{:#?}", fake.log);
    assert!(fake.has("/var/lib/yulius/system/base-1/usr/bin/dockerd"));
}

/// The reference VM (2026-10-16): Docker installed by hand from a one-line `docker.list`, the
/// host's units left running, no needrestart rule, an older update helper unit. Only those
/// change; nothing is installed and the base is not rebuilt.
#[test]
fn the_reference_vm_changes_only_what_was_left() {
    let mut fake = Fake::set_up();
    fake.files.remove(Path::new(NEEDRESTART_RULE_FILE));
    fake.files.remove(Path::new(DOCKER_SOURCE));
    fake.files.remove(Path::new(HELPER_DROP_IN));
    fake.file(
        "/etc/apt/sources.list.d/docker.list",
        "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu noble stable\n",
    );
    for unit in fake.units.values_mut() {
        *unit = Unit {
            enabled: true,
            active: true,
        };
    }
    let lines = run(&mut fake, Mode::Setup);
    let changed: Vec<&str> = lines
        .iter()
        .filter(|l| l.outcome != HostOutcome::Ok)
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(
        changed,
        ["needrestart", "host Docker units", "update helper"],
        "{lines:#?}"
    );
    assert!(
        !fake.ran("apt-get")
            && !fake.ran("runuser -u yulius -- /usr/local/bin/yulius sandbox-base")
    );
}

/// HST-010 (#5 of the task): a binary the service user owns or may write is fixed, also by a check.
#[test]
fn a_wrong_binary_owner_is_fixed() {
    for mode in [Mode::Setup, Mode::Check] {
        let mut fake = Fake::set_up();
        fake.files
            .insert(PathBuf::from(EXE), (String::new(), SERVICE_UID, 0o100_775));
        let lines = run(&mut fake, mode);
        assert_eq!(
            outcome(&lines, "yulius binary"),
            HostOutcome::Changed,
            "{lines:#?}"
        );
        assert_eq!(fake.stat(Path::new(EXE)), Some((0, 0o100_755)));
        assert_eq!(
            outcome(&lines, "sandbox Docker"),
            HostOutcome::Ok,
            "{lines:#?}"
        );
    }
}

/// HST-007: the free range starts past every range in use (the VM: 100000–231071 taken).
#[test]
fn subordinate_ids_take_the_first_free_range() {
    assert_eq!(free_sub_range(&["", ""]), Some(100_000));
    assert_eq!(
        free_sub_range(&["ubuntu:100000:65536\n", "ubuntu:100000:65536\n"]),
        Some(165_536)
    );
    assert_eq!(
        free_sub_range(&[
            "ubuntu:100000:65536\nlxd:165536:65536\n# x\n",
            "other:100000:131072\n"
        ]),
        Some(231_072)
    );
    assert_eq!(free_sub_range(&["a:300000:10\n"]), Some(300_010));
    assert_eq!(free_sub_range(&["a:4294900000:10\n"]), Some(4_294_900_010));
    assert_eq!(free_sub_range(&["a:4294960000:10\n"]), None);
}

/// HST-008: a running unit without the delegation gets a drop-in, and its restart is the
/// operator's (listed as manual); a missing unit without --public-url is manual.
#[test]
fn the_unit_gets_a_drop_in_and_its_restart_is_manual() {
    let mut fake = Fake::set_up();
    let text = fake
        .text(UNIT_FILE)
        .unwrap_or_default()
        .replace("Delegate=yes\n", "");
    fake.files.remove(Path::new(UNIT_FILE));
    fake.file("/fake/unit", &text);
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(
        outcome(&lines, "yulius.service"),
        HostOutcome::Changed,
        "{lines:#?}"
    );
    assert!(fake.has(UNIT_DROP_IN));
    let restart = line(&lines, "yulius.service restart");
    assert_eq!(restart.outcome, HostOutcome::Manual);
    assert!(
        restart.detail.contains("systemctl restart yulius"),
        "{}",
        restart.detail
    );

    let mut fake = Fake::fresh();
    let lines = Run::new(
        &mut fake,
        Mode::Setup,
        Options {
            public_url: None,
            ..options()
        },
        &mut |_| {},
    )
    .all();
    let unit = line(&lines, "yulius.service");
    assert_eq!(unit.outcome, HostOutcome::Manual);
    assert!(unit.detail.contains("--public-url"), "{}", unit.detail);
    assert!(!fake.has(UNIT_FILE));
}

/// A data directory root owns is refused: sandboxes need the service user's id range.
#[test]
fn a_root_owned_data_directory_fails() {
    let mut fake = Fake::fresh();
    fake.files
        .insert(PathBuf::from(DATA), (String::new(), 0, 0o040_700));
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(outcome(&lines, "service user"), HostOutcome::Failed);
    assert!(!fake.ran("useradd"));
}

/// HST-011 (#3 of the task): Docker installed on the host after the base was built: the setup
/// builds a new base at once; a check leaves it to the server and the verdict says why.
#[test]
fn a_base_without_the_hosts_dockerd_is_rebuilt() {
    let mut fake = Fake::set_up();
    let current = fake
        .text("/var/lib/yulius/system/current.json")
        .unwrap_or_default()
        .to_owned();
    fake.files
        .remove(Path::new("/var/lib/yulius/system/base-1/usr/bin/dockerd"));
    let lines = run(&mut fake, Mode::Check);
    assert_eq!(outcome(&lines, "sandbox base"), HostOutcome::Ok);
    let docker = line(&lines, "sandbox Docker");
    assert_eq!(docker.outcome, HostOutcome::Failed);
    assert!(docker.detail.contains("base-1"), "{}", docker.detail);
    assert!(!fake.ran("runuser"));

    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(outcome(&lines, "sandbox base"), HostOutcome::Changed);
    assert_ne!(
        fake.text("/var/lib/yulius/system/current.json")
            .unwrap_or_default(),
        current
    );
    assert_eq!(
        outcome(&lines, "sandbox Docker"),
        HostOutcome::Ok,
        "{lines:#?}"
    );
}

/// HST-012: a check never installs, adds ids or users, writes units or builds a base.
#[test]
fn a_check_never_installs_adds_or_builds() {
    let mut fake = Fake::fresh();
    let lines = run(&mut fake, Mode::Check);
    for forbidden in [
        "apt-get",
        "useradd",
        "usermod",
        "runuser",
        "systemctl start",
        "systemctl enable",
        "curl",
    ] {
        assert!(!fake.ran(forbidden), "{forbidden}: {:#?}", fake.log);
    }
    assert!(!fake.has(UNIT_FILE) && !fake.has(UNIT_DROP_IN) && !fake.has(HELPER_DROP_IN));
    assert!(!passed(&lines));
    assert!(
        !lines
            .iter()
            .any(|l| l.name == "Docker smoke test" || l.name == "contained doctor")
    );
}

/// The smoke's failure fails the setup; without the update helper its item is manual.
#[test]
fn a_failed_smoke_fails_the_setup_and_a_missing_helper_is_manual() {
    let mut fake = Fake::set_up();
    fake.smoke_fails = true;
    fake.helper = false;
    let lines = run(&mut fake, Mode::Setup);
    assert_eq!(outcome(&lines, "Docker smoke test"), HostOutcome::Failed);
    assert!(!passed(&lines));
    let helper = line(&lines, "update helper");
    assert_eq!(helper.outcome, HostOutcome::Manual);
    assert!(helper.detail.contains("§8"), "{}", helper.detail);
}
