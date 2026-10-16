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
use yulius_sandbox::docker_status::{
    MODULES_FILE, NAME as DOCKER_NAME, missing_modules, modules_file_content,
};

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
        if self.may(true) {
            if let Err(why) = self.host.write(path, NEEDRESTART_RULE, 0o644) {
                return self.failed(ITEM, format!("cannot write {NEEDRESTART_RULE_FILE}: {why}"));
            }
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
        if self.may(false) {
            if let Err(why) = self.apt_install(&missing) {
                return self.failed(ITEM, why);
            }
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
        if self.may(false) {
            if let Err(why) = self.add_docker_source(&id, &codename) {
                return self.failed(ITEM, why);
            }
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
            if self.may(true) {
                if let Err(why) = self.host.write(Path::new(MODULES_FILE), &want, 0o644) {
                    return self.failed(ITEM, format!("cannot write {MODULES_FILE}: {why}"));
                }
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
