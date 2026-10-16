//! `yulius host setup` and `yulius host check` (SPEC-203): the host's prerequisites of Docker in
//! sandboxes, checked in one order and fixed where that is safe.
//!
//! Every item reads the host through [`Host`] (the real one is [`system::RealHost`]; the tests
//! use a fake) and gives one [`HostLine`]: `ok` (left alone), `changed`, `manual` (with the
//! reason it is not automatic) or `failed`. [`Mode::Setup`] fixes everything it safely can,
//! installs packages and builds the base; [`Mode::Check`] (the update helper's re-check, HST-012)
//! applies only the safe, idempotent fixes (the needrestart rule, the host's Docker units, the
//! modules file and loading, the binary's owner) and reports the rest; [`Mode::DryRun`] changes
//! nothing and says what setup would change. The last line is the "sandbox Docker" verdict
//! (`yulius_sandbox::docker_status`), the doctor's words.

pub mod system;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use yulius_contracts::update::{HostCheckReport, HostLine, HostOutcome};
use yulius_sandbox::base::BaseInfo;
use yulius_sandbox::docker_status::{
    BaseFacts, Facts, MODULES_FILE, NAME as DOCKER_NAME, exe_writable, missing_modules,
    modules_file_content, verdict,
};
use yulius_sandbox::manager::DOCKERD_PATHS;
use yulius_sandbox::subid::{REQUIRED_IDS, find_range};

/// What a run may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `yulius host setup`: everything safe, packages and the base included.
    Setup,
    /// `yulius host setup --dry-run`: nothing; says what setup would change.
    DryRun,
    /// `yulius host check`: only the safe, idempotent fixes (HST-012).
    Check,
}

/// One command for [`Host::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    /// Program and arguments (never through a shell).
    pub argv: Vec<String>,
    /// Extra environment.
    pub env: Vec<(String, String)>,
    /// Let its output through to the operator's terminal (apt, the base build, the smoke).
    pub show: bool,
    /// Bound.
    pub timeout: Duration,
}

impl Cmd {
    fn new(argv: &[&str]) -> Self {
        Self {
            argv: argv.iter().map(|&a| a.to_owned()).collect(),
            env: Vec::new(),
            show: false,
            timeout: Duration::from_secs(120),
        }
    }

    fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    fn shown(mut self, timeout: Duration) -> Self {
        self.show = true;
        self.timeout = timeout;
        self
    }

    /// Non-interactive apt with needrestart suspended (HST-003, HST-006).
    fn apt(args: &[&str]) -> Self {
        let mut cmd = Self::new(&["apt-get"]).shown(Duration::from_secs(30 * 60));
        cmd.argv.extend(args.iter().map(|&a| a.to_owned()));
        cmd.env = [
            ("DEBIAN_FRONTEND", "noninteractive"),
            ("NEEDRESTART_SUSPEND", "1"),
            ("NEEDRESTART_MODE", "l"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .to_vec();
        cmd
    }
}

/// What a command did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Out {
    /// Exit code (-1 when killed or not started).
    pub code: i32,
    /// Its standard output (empty when shown).
    pub stdout: String,
    /// Its standard error (empty when shown).
    pub stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == 0
    }

    /// The last non-empty line of its error, else of its output.
    fn why(&self) -> String {
        let last = |text: &str| {
            text.lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_owned)
        };
        last(&self.stderr)
            .or_else(|| last(&self.stdout))
            .unwrap_or_else(|| format!("exit status {}", self.code))
    }
}

/// The host, as the items see it.
pub trait Host {
    /// A file's text; `None` when it cannot be read.
    fn read(&self, path: &Path) -> Option<String>;
    /// Writes a file (creating its directory) owned by root with `mode`.
    ///
    /// # Errors
    /// Why it could not be written.
    fn write(&mut self, path: &Path, content: &str, mode: u32) -> Result<(), String>;
    /// Whether a regular file is there.
    fn is_file(&self, path: &Path) -> bool;
    /// Names in a directory (empty when unreadable).
    fn list(&self, dir: &Path) -> Vec<String>;
    /// A path's owner and mode bits, not following a final link.
    fn stat(&self, path: &Path) -> Option<(u32, u32)>;
    /// Makes `path` owned by root and not writable by group or others.
    ///
    /// # Errors
    /// Why not.
    fn own_by_root(&mut self, path: &Path) -> Result<(), String>;
    /// The host system's fingerprint, as sandbox bases record it (`base::fingerprint`).
    fn fingerprint(&self) -> String;
    /// Runs a command; `Err` when it could not be started.
    ///
    /// # Errors
    /// Why it did not start.
    fn run(&mut self, cmd: &Cmd) -> Result<Out, String>;
}

/// Options of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The service user when the data directory does not exist yet.
    pub user: String,
    /// The public URL for a `yulius.service` written by the setup.
    pub public_url: Option<String>,
    /// The `yulius` executable running the setup.
    pub exe: PathBuf,
    /// Run the Docker smoke test at the end of a setup.
    pub smoke: bool,
}

/// The packages of Docker's repository (HST-003).
pub const DOCKER_CE: [&str; 5] = [
    "docker-ce",
    "docker-ce-cli",
    "containerd.io",
    "docker-buildx-plugin",
    "docker-compose-plugin",
];
/// Ubuntu's plugins for its `docker.io` (SPEC-201).
pub const UBUNTU_PLUGINS: [&str; 2] = ["docker-buildx", "docker-compose-v2"];
/// The packages of sandbox isolation (`newuidmap`, egress, `bwrap`).
pub const ISOLATION_PACKAGES: [&str; 3] = ["uidmap", "slirp4netns", "bubblewrap"];
/// The host's own Docker units, which Yulius never needs (HST-004).
pub const HOST_DOCKER_UNITS: [&str; 3] = ["docker.socket", "docker.service", "containerd.service"];
/// Docker's apt source written by the setup.
pub const DOCKER_SOURCE: &str = "/etc/apt/sources.list.d/docker.sources";
/// Docker's signing key.
pub const DOCKER_KEY: &str = "/etc/apt/keyrings/docker.asc";
/// The needrestart rule (HST-006).
pub const NEEDRESTART_RULE_FILE: &str = "/etc/needrestart/conf.d/yulius.conf";
/// Its content.
pub const NEEDRESTART_RULE: &str = "# Written by `yulius host setup`: package installs never \
restart Yulius behind the operator's\n# back. Its update flow pauses tasks before a restart \
(docs/operations.md, \"Upgrades\").\n$nrconf{override_rc}{qr(^yulius\\.service$)} = 0;\n";

/// The drop-in that gives `yulius.service` what sandboxes need (HST-008).
pub const UNIT_DROP_IN: &str = "/etc/systemd/system/yulius.service.d/10-yulius-sandboxes.conf";
/// Its content.
pub const UNIT_DROP_IN_TEXT: &str = "# Written by `yulius host setup`: per-sandbox cgroups (limits, \
Docker engines, the update's\n# freeze) and sessions that survive a restart (docs/operations.md \
§3).\n[Service]\nDelegate=yes\nDelegateSubgroup=server\nKillMode=process\n";
/// The drop-in that lets the update helper's re-check write its safe fixes (HST-009).
pub const HELPER_DROP_IN: &str =
    "/etc/systemd/system/yulius-update.service.d/10-yulius-host-check.conf";
/// Its content.
pub const HELPER_DROP_IN_TEXT: &str = "# Written by `yulius host setup`: the re-check after an \
update (`yulius host check`) may\n# write the kernel modules list and the needrestart rule.\n\
[Service]\nReadWritePaths=/etc/modules-load.d -/etc/needrestart/conf.d\n";
/// Where the setup writes a missing `yulius.service`.
pub const UNIT_FILE: &str = "/etc/systemd/system/yulius.service";
/// The first subordinate id the setup gives out.
const SUB_ID_FLOOR: u32 = 100_000;
/// Ids per range (65,536: root of the sandbox is the service user, the rest 1–65535).
const SUB_ID_RANGE: u32 = 65_536;

/// `yulius.service` as written for a fresh install (docs/operations.md §3), for `exe`, the
/// service `user`, the data directory and the public URL.
#[must_use]
pub fn unit_text(exe: &Path, user: &str, data_dir: &Path, public_url: &str) -> String {
    format!(
        "# Written by `yulius host setup` (docs/operations.md §3); edit it for this server.\n\
         [Unit]\nDescription=Yulius\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nUser={user}\nGroup={user}\nEnvironment=YULIUS_DATA={data}\n\
         Environment=YULIUS_PUBLIC_URL={public_url}\nEnvironment=YULIUS_BIND=127.0.0.1:8080\n\
         Environment=YULIUS_INTERNAL_BIND=127.0.0.1:8081\nEnvironment=YULIUS_CONTAINED=true\n\
         Environment=PATH=/usr/local/bin:/usr/bin:/bin\nExecStart={exe} serve\nKillMode=process\n\
         Delegate=yes\nDelegateSubgroup=server\nRestart=on-failure\nRestartSec=2\n\
         LimitNOFILE=65536\n\n[Install]\nWantedBy=multi-user.target\n",
        data = data_dir.display(),
        exe = exe.display(),
    )
}

/// Whether `value` is safe in a unit file line (no spaces, quotes, `%` specifiers or controls).
fn unit_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\'' | b'\\' | b'%' | b';'))
}

/// The service user.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Service {
    name: String,
    /// `None` until a dry run's user exists.
    uid: Option<u32>,
}

/// What `systemctl show yulius.service` said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UnitState {
    loaded: bool,
    delegated: bool,
    active: bool,
    /// The binary of `ExecStart`.
    exec: Option<PathBuf>,
    /// The setup wrote it: started at the end.
    written: bool,
}

/// The state of one run: the host, what it may change, the lines so far.
pub struct Run<'a> {
    host: &'a mut dyn Host,
    mode: Mode,
    opts: Options,
    lines: Vec<HostLine>,
    report: &'a mut dyn FnMut(&HostLine),
    apt_updated: bool,
    /// The service's `YULIUS_SANDBOX_DOCKER=off` (read from its unit): Docker items are skipped.
    docker_off: bool,
    service: Option<Service>,
    unit: UnitState,
}

impl<'a> Run<'a> {
    /// A run on `host`; `report` gets each line as it ends.
    pub fn new(
        host: &'a mut dyn Host,
        mode: Mode,
        opts: Options,
        report: &'a mut dyn FnMut(&HostLine),
    ) -> Self {
        Self {
            host,
            mode,
            opts,
            lines: Vec::new(),
            report,
            apt_updated: false,
            docker_off: false,
            service: None,
            unit: UnitState::default(),
        }
    }

    fn line(&mut self, name: &str, outcome: HostOutcome, detail: impl Into<String>) {
        let line = HostLine {
            name: name.to_owned(),
            outcome,
            detail: detail.into(),
        };
        (self.report)(&line);
        self.lines.push(line);
    }

    fn ok(&mut self, name: &str, detail: impl Into<String>) {
        self.line(name, HostOutcome::Ok, detail);
    }

    fn failed(&mut self, name: &str, detail: impl Into<String>) {
        self.line(name, HostOutcome::Failed, detail);
    }

    fn manual(&mut self, name: &str, detail: impl Into<String>) {
        self.line(name, HostOutcome::Manual, detail);
    }

    /// `changed` with what was done, or in a dry run what would be.
    fn changed(&mut self, name: &str, done: &str, would: &str) {
        let detail = if self.mode == Mode::DryRun {
            format!("would {would}")
        } else {
            done.to_owned()
        };
        self.line(name, HostOutcome::Changed, detail);
    }

    /// Whether this run applies a fix that is `safe` (also in [`Mode::Check`]).
    fn may(&self, safe: bool) -> bool {
        match self.mode {
            Mode::Setup => true,
            Mode::DryRun => false,
            Mode::Check => safe,
        }
    }

    /// What a check says about an unsafe item it leaves (HST-012).
    fn needs_setup(&mut self, name: &str, what: &str) {
        self.failed(name, format!("{what}; run `sudo yulius host setup`"));
    }

    fn run_cmd(&mut self, cmd: &Cmd) -> Out {
        self.host.run(cmd).unwrap_or_else(|why| Out {
            code: -1,
            stdout: String::new(),
            stderr: why,
        })
    }

    fn installed(&mut self, package: &str) -> bool {
        let out = self.run_cmd(&Cmd::new(&[
            "dpkg-query",
            "-W",
            "-f=${db:Status-Status}",
            package,
        ]));
        out.ok() && out.stdout.trim() == "installed"
    }

    fn version(&mut self, package: &str) -> String {
        let out = self.run_cmd(&Cmd::new(&["dpkg-query", "-W", "-f=${Version}", package]));
        out.stdout.trim().to_owned()
    }

    fn apt_update(&mut self) -> Option<String> {
        if self.apt_updated {
            return None;
        }
        self.apt_updated = true;
        let out = self.run_cmd(&Cmd::apt(&["update"]));
        (!out.ok()).then(|| format!("`apt-get update` failed ({}); installing anyway", out.why()))
    }

    /// Installs `packages`; `Err(why)` when apt failed.
    fn apt_install(&mut self, packages: &[&str]) -> Result<Option<String>, String> {
        let note = self.apt_update();
        let mut args = vec!["install", "-y", "--no-install-recommends"];
        args.extend(packages);
        let out = self.run_cmd(&Cmd::apt(&args));
        if out.ok() {
            Ok(note)
        } else {
            Err(format!(
                "`apt-get install {}` failed: {}",
                packages.join(" "),
                out.why()
            ))
        }
    }

    /// Every item in order; the verdict last. Returns the lines.
    pub fn all(mut self) -> Vec<HostLine> {
        self.docker_off = self.unit_env("YULIUS_SANDBOX_DOCKER").as_deref() == Some("off");
        self.needrestart();
        self.isolation_packages();
        self.docker_engine();
        self.host_docker_units();
        self.kernel_modules();
        self.service_user();
        self.subordinate_ids();
        self.service_unit();
        self.helper_unit();
        self.binary();
        self.base();
        self.start_service();
        let docker_ok = self.verdict();
        if self.mode == Mode::Setup {
            self.contained_doctor();
            self.smoke(docker_ok);
        }
        self.lines
    }

    /// The value of `name` in `yulius.service`'s environment.
    fn unit_env(&mut self, name: &str) -> Option<String> {
        let out = self.run_cmd(&Cmd::new(&[
            "systemctl",
            "show",
            "yulius.service",
            "--property=Environment",
        ]));
        let prefix = format!("{name}=");
        out.stdout
            .trim()
            .strip_prefix("Environment=")?
            .split_whitespace()
            .find_map(|pair| pair.strip_prefix(&prefix).map(str::to_owned))
    }

    /// HST-006: needrestart never restarts `yulius.service`; before any install.
    fn needrestart(&mut self) {
        const ITEM: &str = "needrestart";
        if !self
            .host
            .is_file(Path::new("/etc/needrestart/needrestart.conf"))
        {
            self.ok(ITEM, "not installed: package installs restart no service");
            return;
        }
        let path = Path::new(NEEDRESTART_RULE_FILE);
        if self.host.read(path).as_deref() == Some(NEEDRESTART_RULE) {
            self.ok(
                ITEM,
                format!("never restarts yulius.service ({NEEDRESTART_RULE_FILE})"),
            );
            return;
        }
        if self.may(true)
            && let Err(why) = self.host.write(path, NEEDRESTART_RULE, 0o644)
        {
            return self.failed(ITEM, format!("cannot write {NEEDRESTART_RULE_FILE}: {why}"));
        }
        self.changed(
            ITEM,
            &format!(
                "wrote {NEEDRESTART_RULE_FILE}: package installs never restart yulius.service"
            ),
            &format!(
                "write {NEEDRESTART_RULE_FILE} so package installs never restart yulius.service"
            ),
        );
    }

    /// The packages sandbox isolation needs.
    fn isolation_packages(&mut self) {
        const ITEM: &str = "isolation packages";
        let missing: Vec<&str> = ISOLATION_PACKAGES
            .into_iter()
            .filter(|p| !self.installed(p))
            .collect();
        if missing.is_empty() {
            return self.ok(ITEM, ISOLATION_PACKAGES.join(", "));
        }
        let list = missing.join(" ");
        if self.mode == Mode::Check {
            return self.needs_setup(ITEM, &format!("{list} not installed"));
        }
        if self.may(false)
            && let Err(why) = self.apt_install(&missing)
        {
            return self.failed(ITEM, why);
        }
        self.changed(
            ITEM,
            &format!("installed {list}"),
            &format!("install {list}"),
        );
    }

    /// HST-003: Docker Engine with buildx and compose.
    fn docker_engine(&mut self) {
        const ITEM: &str = "Docker Engine";
        let ce_missing: Vec<&str> = DOCKER_CE
            .into_iter()
            .filter(|p| !self.installed(p))
            .collect();
        if ce_missing.is_empty() {
            let version = self.version("docker-ce");
            return self.ok(
                ITEM,
                format!("docker-ce {version} from Docker's repository, with buildx and compose"),
            );
        }
        if self.installed("docker.io") {
            let missing: Vec<&str> = UBUNTU_PLUGINS
                .into_iter()
                .filter(|p| !self.installed(p))
                .collect();
            let version = self.version("docker.io");
            if missing.is_empty() {
                return self.ok(
                    ITEM,
                    format!("Ubuntu's docker.io {version} with docker-buildx and docker-compose-v2 (kept)"),
                );
            }
            return self.install_docker(ITEM, &missing, "to Ubuntu's docker.io");
        }
        if self.docker_off {
            return self.ok(
                ITEM,
                "not installed: Docker in sandboxes is off (YULIUS_SANDBOX_DOCKER=off)",
            );
        }
        if ce_missing.len() < DOCKER_CE.len() || self.docker_source() {
            return self.install_docker(ITEM, &ce_missing, "from Docker's repository");
        }
        let Some((id, codename)) = self.distribution() else {
            return self.manual(
                ITEM,
                "this distribution is not Ubuntu or Debian: install Docker Engine with the buildx \
                 and compose plugins (https://docs.docker.com/engine/install/), then run the setup \
                 again",
            );
        };
        if self.mode == Mode::Check {
            return self.needs_setup(
                ITEM,
                "no Docker on the host, so new sandbox bases have none",
            );
        }
        if self.may(false)
            && let Err(why) = self.add_docker_source(&id, &codename)
        {
            return self.failed(ITEM, why);
        }
        self.install_docker(ITEM, &DOCKER_CE, "from Docker's repository (source added)");
    }

    fn install_docker(&mut self, item: &str, packages: &[&str], whence: &str) {
        let list = packages.join(" ");
        if self.mode == Mode::Check {
            return self.needs_setup(item, &format!("{list} not installed"));
        }
        let mut note = None;
        if self.may(false) {
            match self.apt_install(packages) {
                Ok(n) => note = n,
                Err(why) => return self.failed(item, why),
            }
        }
        let done = match note {
            Some(note) => format!("installed {list} {whence} ({note})"),
            None => format!("installed {list} {whence}"),
        };
        self.changed(item, &done, &format!("install {list} {whence}"));
    }

    /// Whether an apt source names Docker's repository.
    fn docker_source(&mut self) -> bool {
        let dir = Path::new("/etc/apt/sources.list.d");
        let mut files: Vec<PathBuf> = self
            .host
            .list(dir)
            .into_iter()
            .map(|name| dir.join(name))
            .collect();
        files.push(PathBuf::from("/etc/apt/sources.list"));
        files.iter().any(|file| {
            self.host
                .read(file)
                .is_some_and(|text| text.contains("download.docker.com"))
        })
    }

    /// `(ID, VERSION_CODENAME)` of Ubuntu or Debian, from `/etc/os-release`.
    fn distribution(&self) -> Option<(String, String)> {
        let text = self.host.read(Path::new("/etc/os-release"))?;
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{key}=")))
                .map(|v| v.trim().trim_matches('"').to_owned())
        };
        let id = field("ID")?;
        let codename = field("VERSION_CODENAME")?;
        let safe = |v: &str| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric());
        (matches!(id.as_str(), "ubuntu" | "debian") && safe(&codename)).then_some((id, codename))
    }

    fn add_docker_source(&mut self, id: &str, codename: &str) -> Result<(), String> {
        let steps = [
            Cmd::new(&["install", "-m", "0755", "-d", "/etc/apt/keyrings"]),
            Cmd::new(&["curl", "-fsSL", "--max-time", "60", "-o", DOCKER_KEY])
                .arg(format!("https://download.docker.com/linux/{id}/gpg")),
            Cmd::new(&["chmod", "a+r", DOCKER_KEY]),
        ];
        for cmd in &steps {
            let out = self.run_cmd(cmd);
            if !out.ok() {
                return Err(format!("`{}` failed: {}", cmd.argv.join(" "), out.why()));
            }
        }
        let arch = self.run_cmd(&Cmd::new(&["dpkg", "--print-architecture"]));
        let arch = arch.stdout.trim();
        if !arch.bytes().all(|b| b.is_ascii_alphanumeric()) || arch.is_empty() {
            return Err("`dpkg --print-architecture` gave no architecture".into());
        }
        let source = format!(
            "# Written by `yulius host setup`: Docker Engine for the sandboxes' bases.\n\
             Types: deb\nURIs: https://download.docker.com/linux/{id}\nSuites: {codename}\n\
             Components: stable\nArchitectures: {arch}\nSigned-By: {DOCKER_KEY}\n"
        );
        self.host.write(Path::new(DOCKER_SOURCE), &source, 0o644)
    }

    /// HST-004: the host's own Docker units are disabled and stopped, unless they run
    /// containers.
    fn host_docker_units(&mut self) {
        const ITEM: &str = "host Docker units";
        let mut present = Vec::new();
        let mut running = Vec::new();
        let mut on = Vec::new();
        for unit in HOST_DOCKER_UNITS {
            let out = self.run_cmd(&Cmd::new(&[
                "systemctl",
                "show",
                unit,
                "--property=LoadState,UnitFileState,ActiveState",
            ]));
            let prop = |key: &str| {
                out.stdout
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{key}=")))
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            if prop("LoadState") != "loaded" {
                continue;
            }
            present.push(unit);
            let enabled = prop("UnitFileState") == "enabled";
            let active = matches!(
                prop("ActiveState").as_str(),
                "active" | "activating" | "reloading"
            );
            if active {
                running.push(unit);
            }
            if enabled || active {
                on.push(unit);
            }
        }
        if present.is_empty() {
            return self.ok(ITEM, "none installed");
        }
        if on.is_empty() {
            return self.ok(
                ITEM,
                format!(
                    "{} disabled and stopped (sandboxes run their own engines)",
                    present.join(", ")
                ),
            );
        }
        // Asking an idle daemon would start it through its socket: only a running one is asked.
        if running.contains(&"docker.service") {
            let out = self.run_cmd(&Cmd::new(&["docker", "ps", "--quiet"]));
            let containers = out.stdout.lines().filter(|l| !l.trim().is_empty()).count();
            if containers > 0 {
                return self.manual(
                    ITEM,
                    format!(
                        "the host's Docker runs {containers} container(s), so it is left running; \
                         Yulius never uses it (each sandbox runs its own engine): stop them and run \
                         the setup again to disable it"
                    ),
                );
            }
        }
        let list = on.join(" ");
        if self.may(true) {
            let mut cmd = Cmd::new(&["systemctl", "disable", "--now"]);
            cmd.argv.extend(on.iter().map(|&u| u.to_owned()));
            let out = self.run_cmd(&cmd);
            if !out.ok() {
                return self.failed(
                    ITEM,
                    format!("`systemctl disable --now {list}` failed: {}", out.why()),
                );
            }
        }
        self.changed(
            ITEM,
            &format!("disabled and stopped {list}: Yulius never uses the host's Docker"),
            &format!("disable and stop {list}"),
        );
    }

    /// HST-005: the engine's modules are listed for every boot and loaded now.
    fn kernel_modules(&mut self) {
        const ITEM: &str = "kernel modules";
        let want = modules_file_content();
        let file_ok = self.host.read(Path::new(MODULES_FILE)).as_deref() == Some(want.as_str());
        let missing = self.missing_modules();
        if file_ok && missing.is_empty() {
            return self.ok(
                ITEM,
                format!("loaded, and listed in {MODULES_FILE} for every boot"),
            );
        }
        let mut done = Vec::new();
        let mut would = Vec::new();
        if !file_ok {
            if self.may(true)
                && let Err(why) = self.host.write(Path::new(MODULES_FILE), &want, 0o644)
            {
                return self.failed(ITEM, format!("cannot write {MODULES_FILE}: {why}"));
            }
            done.push(format!("wrote {MODULES_FILE}"));
            would.push(format!("write {MODULES_FILE}"));
        }
        if !missing.is_empty() {
            let list = missing.join(" ");
            if self.may(true) {
                let mut cmd = Cmd::new(&["modprobe", "-a"]);
                cmd.argv.extend(missing.iter().map(|&m| m.to_owned()));
                let out = self.run_cmd(&cmd);
                let still = self.missing_modules();
                if !out.ok() || !still.is_empty() {
                    return self.failed(
                        ITEM,
                        format!("cannot load {}: {}", still.join(" "), out.why()),
                    );
                }
            }
            done.push(format!("loaded {list}"));
            would.push(format!("load {list}"));
        }
        self.changed(ITEM, &done.join("; "), &would.join(" and "));
    }

    /// The data directory's owner, else `--user`: created when missing (HST-007).
    fn service_user(&mut self) {
        const ITEM: &str = "service user";
        let data = self.opts.data_dir.clone();
        let passwd = self.host.read(Path::new("/etc/passwd")).unwrap_or_default();
        let by_uid = |uid: u32| {
            passwd.lines().find_map(|l| {
                let mut f = l.split(':');
                let name = f.next()?;
                (f.nth(1)?.parse::<u32>().ok()? == uid).then(|| name.to_owned())
            })
        };
        let uid_of = |name: &str| {
            passwd.lines().find_map(|l| {
                let mut f = l.split(':');
                (f.next()? == name).then_some(())?;
                f.nth(1)?.parse::<u32>().ok()
            })
        };
        if let Some((owner, mode)) = self.host.stat(&data) {
            if owner == 0 {
                return self.failed(
                    ITEM,
                    format!(
                        "{} belongs to root: sandboxes run as a service user that owns it, with a \
                         subordinate id range; `chown -R` it to that user (docs/operations.md §2)",
                        data.display()
                    ),
                );
            }
            let name = by_uid(owner).unwrap_or_else(|| owner.to_string());
            self.service = Some(Service {
                name: name.clone(),
                uid: Some(owner),
            });
            if mode & 0o077 != 0 {
                return self.ok(
                    ITEM,
                    format!(
                        "{name} owns {} (mode {:o}; `yulius serve` restricts it to 0700)",
                        data.display(),
                        mode & 0o777
                    ),
                );
            }
            return self.ok(ITEM, format!("{name} owns {}", data.display()));
        }
        let user = self.opts.user.clone();
        if !unit_safe(&user) || user.starts_with('-') {
            return self.failed(ITEM, format!("{user:?} is not a user name"));
        }
        if self.mode == Mode::Check {
            return self.needs_setup(ITEM, &format!("{} does not exist", data.display()));
        }
        let existing = uid_of(&user);
        let data_text = data.display().to_string();
        if self.may(false) {
            let cmd = if existing.is_some() {
                Cmd::new(&[
                    "install", "-d", "-m", "0700", "-o", &user, "-g", &user, &data_text,
                ])
            } else {
                Cmd::new(&[
                    "useradd",
                    "--system",
                    "--create-home",
                    "--home-dir",
                    &data_text,
                    "--shell",
                    "/usr/sbin/nologin",
                    &user,
                ])
            };
            let out = self.run_cmd(&cmd);
            if !out.ok() {
                return self.failed(
                    ITEM,
                    format!("`{}` failed: {}", cmd.argv.join(" "), out.why()),
                );
            }
            let _ = self.run_cmd(&Cmd::new(&["chmod", "0700", &data_text]));
        }
        let uid = self.host.stat(&data).map(|(uid, _)| uid).or(existing);
        self.service = Some(Service {
            name: user.clone(),
            uid,
        });
        if existing.is_some() {
            self.changed(
                ITEM,
                &format!("created {data_text} for {user} (mode 0700)"),
                &format!("create {data_text} for {user}"),
            );
        } else {
            self.changed(
                ITEM,
                &format!("created the system user {user} with the home {data_text} (mode 0700)"),
                &format!("create the system user {user} with the home {data_text}"),
            );
        }
    }

    /// A subordinate uid and gid range of 65,536 ids for the service user (HST-007).
    fn subordinate_ids(&mut self) {
        const ITEM: &str = "subordinate ids";
        let Some(Service { name, uid }) = self.service.clone() else {
            return self.manual(ITEM, "no service user (see above)");
        };
        let uid = uid.unwrap_or(u32::MAX);
        let read = |host: &dyn Host, file: &str| host.read(Path::new(file)).unwrap_or_default();
        let (subuid, subgid) = (
            read(&*self.host, "/etc/subuid"),
            read(&*self.host, "/etc/subgid"),
        );
        let has_uid = find_range(&subuid, &name, uid);
        let has_gid = find_range(&subgid, &name, uid);
        if let (Some((us, uc)), Some((gs, gc))) = (has_uid, has_gid) {
            return self.ok(
                ITEM,
                format!(
                    "{name}: uids {us}–{}, gids {gs}–{}",
                    us + uc - 1,
                    gs + gc - 1
                ),
            );
        }
        if self.mode == Mode::Check {
            return self.needs_setup(
                ITEM,
                &format!("{name} has no range of {} ids", REQUIRED_IDS + 1),
            );
        }
        let Some(start) = free_sub_range(&[&subuid, &subgid]) else {
            return self.failed(
                ITEM,
                "no free range of 65,536 ids is left after those in use",
            );
        };
        let range = format!("{start}-{}", start + SUB_ID_RANGE - 1);
        let mut cmd = Cmd::new(&["usermod"]);
        if has_uid.is_none() {
            cmd = cmd.arg("--add-subuids").arg(range.clone());
        }
        if has_gid.is_none() {
            cmd = cmd.arg("--add-subgids").arg(range.clone());
        }
        cmd = cmd.arg(name.clone());
        if self.may(false) {
            let out = self.run_cmd(&cmd);
            if !out.ok() {
                return self.failed(
                    ITEM,
                    format!("`{}` failed: {}", cmd.argv.join(" "), out.why()),
                );
            }
        }
        self.changed(
            ITEM,
            &format!("gave {name} the ids {range}"),
            &format!("give {name} the ids {range}"),
        );
    }

    fn read_unit(&mut self) -> UnitState {
        let out = self.run_cmd(&Cmd::new(&[
            "systemctl",
            "show",
            "yulius.service",
            "--property=LoadState,Delegate,DelegateSubgroup,KillMode,ActiveState,ExecStart",
        ]));
        let prop = |key: &str| {
            out.stdout
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{key}=")))
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        UnitState {
            loaded: prop("LoadState") == "loaded",
            delegated: prop("Delegate") == "yes"
                && prop("DelegateSubgroup") == "server"
                && prop("KillMode") == "process",
            active: prop("ActiveState") == "active",
            exec: prop("ExecStart")
                .split_once("path=")
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .map(PathBuf::from),
            written: false,
        }
    }

    /// `yulius.service`: written when missing (with `--public-url`), else given the delegation
    /// by a drop-in (HST-008).
    fn service_unit(&mut self) {
        const ITEM: &str = "yulius.service";
        let state = self.read_unit();
        if !state.loaded {
            self.unit = state;
            if self.mode == Mode::Check {
                return self.needs_setup(ITEM, "not installed");
            }
            let Some(url) = self.opts.public_url.clone() else {
                return self.manual(
                    ITEM,
                    "not installed: run the setup again with --public-url https://<host> to write \
                     the unit of docs/operations.md §3, or write it yourself",
                );
            };
            let user = self
                .service
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default();
            let exe = self.opts.exe.clone();
            let data = self.opts.data_dir.clone();
            let exe_text = exe.display().to_string();
            let data_text = data.display().to_string();
            if ![
                url.as_str(),
                user.as_str(),
                exe_text.as_str(),
                data_text.as_str(),
            ]
            .iter()
            .all(|v| unit_safe(v))
            {
                return self.failed(
                    ITEM,
                    "the URL, user or paths have characters a unit file cannot hold",
                );
            }
            if self.may(false) {
                let text = unit_text(&exe, &user, &data, &url);
                if let Err(why) = self.host.write(Path::new(UNIT_FILE), &text, 0o644) {
                    return self.failed(ITEM, format!("cannot write {UNIT_FILE}: {why}"));
                }
                for cmd in [
                    Cmd::new(&["systemctl", "daemon-reload"]),
                    Cmd::new(&["systemctl", "enable", "yulius.service"]),
                ] {
                    let out = self.run_cmd(&cmd);
                    if !out.ok() {
                        return self.failed(
                            ITEM,
                            format!("`{}` failed: {}", cmd.argv.join(" "), out.why()),
                        );
                    }
                }
            }
            self.unit = UnitState {
                loaded: true,
                delegated: true,
                active: false,
                exec: Some(exe),
                written: true,
            };
            return self.changed(
                ITEM,
                &format!("wrote and enabled {UNIT_FILE} for {url} (started at the end)"),
                &format!("write and enable {UNIT_FILE} for {url}"),
            );
        }
        if state.delegated {
            self.unit = state;
            return self.ok(
                ITEM,
                "Delegate=yes, DelegateSubgroup=server, KillMode=process",
            );
        }
        if self.mode == Mode::Check {
            self.unit = state;
            return self.needs_setup(
                ITEM,
                "lacks Delegate=yes, DelegateSubgroup=server or KillMode=process",
            );
        }
        if self.may(false) {
            if let Err(why) = self
                .host
                .write(Path::new(UNIT_DROP_IN), UNIT_DROP_IN_TEXT, 0o644)
            {
                return self.failed(ITEM, format!("cannot write {UNIT_DROP_IN}: {why}"));
            }
            let out = self.run_cmd(&Cmd::new(&["systemctl", "daemon-reload"]));
            if !out.ok() {
                return self.failed(
                    ITEM,
                    format!("`systemctl daemon-reload` failed: {}", out.why()),
                );
            }
        }
        self.unit = UnitState {
            delegated: true,
            ..state
        };
        self.changed(
            ITEM,
            &format!(
                "wrote {UNIT_DROP_IN} (Delegate=yes, DelegateSubgroup=server, KillMode=process)"
            ),
            &format!(
                "write {UNIT_DROP_IN} (Delegate=yes, DelegateSubgroup=server, KillMode=process)"
            ),
        );
        if self.unit.active {
            self.manual(
                "yulius.service restart",
                "the drop-in applies at the next start: `sudo systemctl restart yulius` when it \
                 suits (sessions and terminals survive it with KillMode=process), or the next \
                 update restarts it",
            );
        }
    }

    /// The update helper may write what its re-check fixes (HST-009).
    fn helper_unit(&mut self) {
        const ITEM: &str = "update helper";
        let out = self.run_cmd(&Cmd::new(&[
            "systemctl",
            "show",
            "yulius-update.service",
            "--property=LoadState,ReadWritePaths",
        ]));
        let loaded = out.stdout.lines().any(|l| l.trim() == "LoadState=loaded");
        if !loaded {
            return self.manual(
                ITEM,
                "not installed: it needs root's Rust toolchain and this server's values \
                 (docs/operations.md §8); without it Settings → Instance offers no Update now",
            );
        }
        let writable = out
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("ReadWritePaths="))
            .is_some_and(|paths| {
                paths
                    .split_whitespace()
                    .any(|p| p.trim_start_matches('-') == "/etc/modules-load.d")
            });
        if writable {
            return self.ok(
                ITEM,
                "re-checks the host after each update (yulius host check)",
            );
        }
        if self.mode == Mode::Check {
            return self.needs_setup(ITEM, "cannot write /etc/modules-load.d after an update");
        }
        if self.may(false) {
            if let Err(why) = self
                .host
                .write(Path::new(HELPER_DROP_IN), HELPER_DROP_IN_TEXT, 0o644)
            {
                return self.failed(ITEM, format!("cannot write {HELPER_DROP_IN}: {why}"));
            }
            let out = self.run_cmd(&Cmd::new(&["systemctl", "daemon-reload"]));
            if !out.ok() {
                return self.failed(
                    ITEM,
                    format!("`systemctl daemon-reload` failed: {}", out.why()),
                );
            }
        }
        self.changed(
            ITEM,
            &format!("wrote {HELPER_DROP_IN}: its re-check may fix the modules list and the needrestart rule"),
            &format!("write {HELPER_DROP_IN}"),
        );
    }

    /// The binaries Yulius runs: owned by root, writable by root alone (HST-010).
    fn binary(&mut self) {
        const ITEM: &str = "yulius binary";
        let mut paths = vec![self.opts.exe.clone()];
        if let Some(exec) = self.unit.exec.clone()
            && !paths.contains(&exec)
        {
            paths.push(exec);
        }
        let service_uid = self
            .service
            .as_ref()
            .and_then(|s| s.uid)
            .unwrap_or(u32::MAX);
        let mut fixed = Vec::new();
        let mut fine = Vec::new();
        for path in paths {
            let Some((owner, mode)) = self.host.stat(&path) else {
                continue;
            };
            if owner == 0 && !exe_writable(owner, mode, service_uid) {
                fine.push(path.display().to_string());
                continue;
            }
            if self.may(true)
                && let Err(why) = self.host.own_by_root(&path)
            {
                return self.failed(ITEM, why);
            }
            fixed.push(path.display().to_string());
        }
        if fixed.is_empty() {
            return self.ok(
                ITEM,
                format!("{} owned by root, not writable by others", fine.join(", ")),
            );
        }
        let list = fixed.join(", ");
        self.changed(
            ITEM,
            &format!("made {list} owned by root and writable by root alone"),
            &format!("make {list} owned by root and writable by root alone"),
        );
    }

    fn current_base(&self) -> Option<BaseFacts> {
        let bases = self.opts.data_dir.join("system");
        let info: BaseInfo =
            serde_json::from_str(&self.host.read(&bases.join("current.json"))?).ok()?;
        let dir = bases.join(&info.id);
        Some(BaseFacts {
            dockerd: DOCKERD_PATHS
                .iter()
                .any(|p| self.host.is_file(&dir.join(p))),
            id: info.id,
        })
    }

    fn base_fingerprint(&self) -> Option<String> {
        let info: BaseInfo = serde_json::from_str(
            &self
                .host
                .read(&self.opts.data_dir.join("system").join("current.json"))?,
        )
        .ok()?;
        Some(info.fingerprint)
    }

    fn host_dockerd(&self) -> bool {
        DOCKERD_PATHS
            .iter()
            .any(|p| self.host.is_file(&Path::new("/").join(p)))
    }

    /// The base matches the host and has its `dockerd`: built now when not, keeping the bases of
    /// running sandboxes (HST-011).
    fn base(&mut self) {
        const ITEM: &str = "sandbox base";
        let base = self.current_base();
        let stale = self.base_fingerprint() != Some(self.host.fingerprint());
        let lacks =
            !self.docker_off && self.host_dockerd() && base.as_ref().is_some_and(|b| !b.dockerd);
        let why = match &base {
            None => "there is none yet".to_owned(),
            Some(b) if lacks => format!("{} has no `dockerd` though the host has one", b.id),
            Some(b) if stale => format!("{} no longer matches the host's system", b.id),
            Some(b) => {
                return self.ok(ITEM, format!("{} matches the host and has `dockerd`", b.id));
            }
        };
        if self.mode == Mode::Check {
            return self.ok(
                ITEM,
                format!("{why}: the server builds a new one once the host has not changed for 10 minutes"),
            );
        }
        let Some(Service { name, .. }) = self.service.clone() else {
            return self.manual(
                ITEM,
                format!("{why}, and there is no service user to build it"),
            );
        };
        if self.may(false) {
            let cmd = Cmd::new(&["runuser", "-u", &name, "--"])
                .arg(self.opts.exe.display().to_string())
                .arg("sandbox-base")
                .arg("build")
                .arg("--data-dir")
                .arg(self.opts.data_dir.display().to_string())
                .shown(Duration::from_secs(60 * 60));
            let out = self.run_cmd(&cmd);
            if !out.ok() {
                return self.failed(ITEM, format!("{why}; the build failed: {}", out.why()));
            }
        }
        let built = self
            .current_base()
            .map(|b| b.id)
            .unwrap_or_else(|| "a new base".into());
        self.changed(
            ITEM,
            &format!("{why}: built {built} (running sandboxes keep theirs until they restart)"),
            &format!("build a new base ({why})"),
        );
    }

    /// A unit the setup wrote is started once the base is there.
    fn start_service(&mut self) {
        const ITEM: &str = "yulius running";
        if !self.unit.loaded || self.mode == Mode::Check {
            return;
        }
        if self.unit.written {
            if self.may(false) {
                let out = self.run_cmd(&Cmd::new(&["systemctl", "start", "yulius.service"]));
                if !out.ok() {
                    return self.failed(
                        ITEM,
                        format!("`systemctl start yulius` failed: {}", out.why()),
                    );
                }
            }
            return self.changed(ITEM, "started yulius.service", "start yulius.service");
        }
        if self.unit.active {
            self.ok(ITEM, "yulius.service is active");
        } else {
            self.manual(
                ITEM,
                "yulius.service is not running: `sudo systemctl start yulius`",
            );
        }
    }

    /// The "sandbox Docker" verdict, the doctor's words (HST-013). Returns whether it holds.
    fn verdict(&mut self) -> bool {
        let service_uid = self
            .service
            .as_ref()
            .and_then(|s| s.uid)
            .unwrap_or(u32::MAX);
        let exe = self
            .unit
            .exec
            .clone()
            .unwrap_or_else(|| self.opts.exe.clone());
        let facts = Facts {
            setting: if self.docker_off {
                Err(yulius_sandbox::manager::DOCKER_OFF.to_owned())
            } else {
                Ok(())
            },
            delegated: self.unit.delegated,
            exe_writable: self
                .host
                .stat(&exe)
                .is_some_and(|(owner, mode)| exe_writable(owner, mode, service_uid)),
            missing_modules: self.missing_modules(),
            host_dockerd: self.host_dockerd(),
            base: self.current_base(),
            building: false,
        };
        let verdict = verdict(&facts);
        let outcome = if verdict.ok {
            HostOutcome::Ok
        } else {
            HostOutcome::Failed
        };
        let ok = verdict.ok;
        if self.mode == Mode::DryRun && !ok {
            self.manual(DOCKER_NAME, format!("now: {}", verdict.detail));
        } else {
            self.line(DOCKER_NAME, outcome, verdict.detail);
        }
        ok
    }

    fn as_service(&self, args: &[&str], timeout: Duration) -> Option<Cmd> {
        let name = self.service.as_ref()?.name.clone();
        let mut cmd = Cmd::new(&["runuser", "-u", &name, "--"])
            .arg(self.opts.exe.display().to_string())
            .shown(timeout);
        for arg in args {
            cmd = cmd.arg(*arg);
        }
        Some(
            cmd.arg("--data-dir")
                .arg(self.opts.data_dir.display().to_string()),
        )
    }

    /// `yulius doctor --contained` as the service user, its lines shown.
    fn contained_doctor(&mut self) {
        const ITEM: &str = "contained doctor";
        let Some(cmd) = self.as_service(&["doctor", "--contained"], Duration::from_secs(120))
        else {
            return self.manual(ITEM, "not run: no service user");
        };
        let out = self.run_cmd(&cmd);
        if out.ok() {
            self.ok(ITEM, "passed (its lines are above)");
        } else {
            self.failed(ITEM, format!("failed: {} (its lines are above)", out.why()));
        }
    }

    /// `yulius doctor --docker-smoke` (as root it runs as the service user), its lines shown.
    fn smoke(&mut self, docker_ok: bool) {
        const ITEM: &str = "Docker smoke test";
        if !self.opts.smoke {
            return self.manual(
                ITEM,
                "not run (--no-smoke): `sudo yulius doctor --docker-smoke`",
            );
        }
        if !docker_ok || self.docker_off {
            return self.manual(
                ITEM,
                "not run: sandboxes get no Docker (see \"sandbox Docker\")",
            );
        }
        let cmd = Cmd::new(&[])
            .arg(self.opts.exe.display().to_string())
            .arg("doctor")
            .arg("--docker-smoke")
            .arg("--data-dir")
            .arg(self.opts.data_dir.display().to_string())
            .shown(Duration::from_secs(45 * 60));
        let out = self.run_cmd(&cmd);
        if out.ok() {
            self.ok(
                ITEM,
                "sandboxes run hello-world, a buildx build and a two-service compose",
            );
        } else {
            self.failed(ITEM, format!("failed: {} (its lines are above)", out.why()));
        }
    }

    fn missing_modules(&self) -> Vec<&'static str> {
        let release = self
            .host
            .read(Path::new("/proc/sys/kernel/osrelease"))
            .unwrap_or_default();
        missing_modules(
            &self
                .host
                .read(Path::new("/proc/modules"))
                .unwrap_or_default(),
            &self
                .host
                .read(
                    &Path::new("/lib/modules")
                        .join(release.trim())
                        .join("modules.builtin"),
                )
                .unwrap_or_default(),
        )
    }
}

/// The start of a range of [`SUB_ID_RANGE`] ids past every range in the subordinate id files'
/// `texts`, at least [`SUB_ID_FLOOR`] (as `useradd` would give, and the reference VM's
/// 231072 after 100000–231071); `None` when no such range fits below 2³².
#[must_use]
pub fn free_sub_range(texts: &[&str]) -> Option<u32> {
    let end = texts
        .iter()
        .flat_map(|text| text.lines())
        .filter_map(|line| {
            let mut fields = line.trim().split(':');
            fields.next()?;
            let start: u64 = fields.next()?.trim().parse().ok()?;
            let count: u64 = fields.next()?.trim().parse().ok()?;
            Some(start + count)
        })
        .max()
        .unwrap_or(0)
        .max(u64::from(SUB_ID_FLOOR));
    u32::try_from(end)
        .ok()
        .filter(|start| start.checked_add(SUB_ID_RANGE).is_some())
}

/// Whether a run's lines hold: nothing failed.
#[must_use]
pub fn passed(lines: &[HostLine]) -> bool {
    lines.iter().all(|l| l.outcome != HostOutcome::Failed)
}

/// The report of `yulius host check --json`.
#[must_use]
pub fn report(lines: Vec<HostLine>) -> HostCheckReport {
    let docker_ok = lines
        .iter()
        .filter(|l| l.name == DOCKER_NAME)
        .all(|l| l.outcome == HostOutcome::Ok);
    HostCheckReport {
        ok: passed(&lines) && docker_ok,
        lines,
        error: None,
        commit: Some(env!("YULIUS_BUILD_COMMIT").to_owned()).filter(|c| !c.is_empty()),
        checked_at: 0,
    }
}

/// One line as the terminal shows it.
#[must_use]
pub fn format_line(line: &HostLine) -> String {
    let tag = match line.outcome {
        HostOutcome::Ok => "ok     ",
        HostOutcome::Changed => "changed",
        HostOutcome::Manual => "manual ",
        HostOutcome::Failed => "FAIL   ",
    };
    format!("{tag}  {:<22} {}", line.name, line.detail)
}

/// The closing summary.
#[must_use]
pub fn summary(lines: &[HostLine], mode: Mode) -> String {
    let count = |o: HostOutcome| lines.iter().filter(|l| l.outcome == o).count();
    let (changed, manual, failed) = (
        count(HostOutcome::Changed),
        count(HostOutcome::Manual),
        count(HostOutcome::Failed),
    );
    let ok = count(HostOutcome::Ok);
    if changed == 0 && manual == 0 && failed == 0 {
        return format!("nothing to change ({ok} items hold)");
    }
    let verb = if mode == Mode::DryRun {
        "would change"
    } else {
        "changed"
    };
    format!("{verb} {changed}, left alone {ok}, manual {manual}, failed {failed}")
}

/// `yulius host setup` / `check`: refuses non-root, runs the items on this host, prints them
/// (or one JSON report with `json`).
#[must_use]
pub fn main(mode: Mode, opts: Options, json: bool) -> ExitCode {
    if !nix::unistd::geteuid().is_root() {
        eprintln!(
            "yulius: `yulius host` changes the host's packages and services: run it as root (sudo)"
        );
        return ExitCode::from(2);
    }
    let mut host = system::RealHost { quiet: json };
    let mut print = |line: &HostLine| {
        if !json {
            println!("{}", format_line(line));
        }
    };
    let lines = Run::new(&mut host, mode, opts, &mut print).all();
    let ok = passed(&lines);
    if json {
        let mut report = report(lines);
        report.checked_at = system::now_ms();
        match serde_json::to_string(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("yulius: {error}"),
        }
    } else {
        println!("{}", summary(&lines, mode));
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
