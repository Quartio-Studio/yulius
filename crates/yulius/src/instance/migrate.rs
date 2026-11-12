//! `yulius instance migrate` (SPEC-460, ADR-0297): one root command on the old host that moves
//! the whole instance to another host as one resumable checklist: plan, prepare, copy, cutover,
//! finish. Every step reads the facts and acts only if needed, so running the command again
//! continues a move; only its identity and how far the cutover got are stored.

pub mod github;
pub mod profile;
pub mod remote;
pub mod target;
pub mod warm;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use yulius_contracts::update::{HostLine, HostOutcome};

use self::profile::{Carried, Profile, TextFile};
use self::remote::Remote;
use self::target::{Facts, Instance, Reply, Request, Setup, SourceFile};
use super::REFUSED;
use super::freeze::{self, Freeze};
use super::receipts::Stage;
use crate::host_setup::{self, Cmd, Host, Mode};
use crate::wiring::update_files::write_atomic;

/// The bytes sent to measure the link.
const LINK_TEST: u64 = 64 << 20;
/// How long the new URL has to answer after the start.
const HTTPS_WAIT: Duration = Duration::from_secs(300);
/// How long the new instance has to say what it did with the tasks.
const TASKS_WAIT: u64 = 180;
/// How long the plan may read the data directory to measure how fast a pass looks at entries.
const SAMPLE: Duration = Duration::from_secs(2);
/// The old proxy's configuration before the redirect, in the state directory.
const PROXY_BACKUP: &str = "Caddyfile.before";

/// Options of `yulius instance migrate`.
#[derive(Debug, Clone)]
pub struct Options {
    /// `[user@]host` of the new host; absent with `--status` and `--abort`.
    pub target: Option<String>,
    /// Its SSH port.
    pub port: u16,
    /// The new public URL.
    pub public_url: Option<String>,
    /// Blank disks of the new host for the data directory: one, or two for a mirror.
    pub data_disks: Vec<String>,
    /// Do not ask "Proceed?".
    pub yes: bool,
    /// The data directory.
    pub data_dir: PathBuf,
    /// The server's unit.
    pub service: String,
    /// The update helper's unit.
    pub update_service: String,
    /// The flow's own directory.
    pub state_dir: PathBuf,
    /// The longest freeze the move may begin, in minutes: above it, it copies again.
    pub max_downtime: Option<u64>,
    /// Never sign in to the new host as root does (its own keys, its agent, its ssh
    /// configuration, a password): only the move's one-time key reaches it, and its line is
    /// shown for a person to add there. How the move helper runs the command (SPEC-470).
    pub no_sign_in: bool,
}

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move, or continue a move.
    Run,
    /// Print the recorded move.
    Status,
    /// Go back to the old host.
    Abort,
}

/// How far the cutover got.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cutover {
    /// The new instance was never started.
    #[default]
    No,
    /// The start was ordered and its end is not recorded.
    Starting,
    /// The new instance runs; the old one is frozen and disabled.
    Moved,
    /// The new instance did not come up and the old one was thawed; the copy there was started.
    RolledBack,
}

/// What a move remembers between runs (`<state>/state.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// 1.
    pub version: u32,
    /// The move's id, also the one-time key's comment.
    pub id: String,
    /// The new host.
    pub target: String,
    /// Its SSH port.
    pub port: u16,
    /// The new public URL, once decided.
    pub new_url: Option<String>,
    /// How far the cutover got.
    pub cutover: Cutover,
    /// The old proxy redirects to the new URL.
    pub redirected: bool,
    /// The move ended: the key is gone and nothing is left to do here.
    pub finished: bool,
    /// The instance the move is of, for `--abort` without options.
    #[serde(default)]
    pub instance: Option<Recorded>,
    /// A copy was begun on the new host: its data directory may hold the instance's files and
    /// secrets, whatever this host still knows of them (SPEC-465 WCP-015).
    #[serde(default)]
    pub copied: bool,
}

/// The instance a move is of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The server's unit.
    pub service: String,
    /// The update helper's unit.
    pub update_service: String,
}

/// What readers such as Settings → Instance see (`<state>/status.json`, no secret values).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// 1.
    pub version: u32,
    /// The move's id.
    pub id: String,
    /// The new host.
    pub target: String,
    /// `Plan`, `Prepare`, `Copy`, `Cutover`, `Finish`, `Abort`, or `done`.
    pub phase: String,
    /// The checklist so far.
    pub lines: Vec<HostLine>,
    /// Seconds since the epoch.
    pub updated_at: u64,
    /// The one-time key's public line, while a person has to add it to root's
    /// `authorized_keys` on the new host (SPEC-470 MVB-010).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// The new public URL, once the plan decided it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_url: Option<String>,
}

impl State {
    /// The one-time key's comment.
    #[must_use]
    pub fn key_comment(&self) -> String {
        format!("yulius-migrate-{}", self.id)
    }

    /// The recorded move, if any.
    #[must_use]
    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join("state.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The move to `target` recorded in `dir`, or a new one kept there.
    ///
    /// # Errors
    /// A move to another host is recorded, or the directory cannot be written.
    pub fn begin(dir: &Path, target: &str, port: u16) -> Result<Self, String> {
        if Self::load(dir).is_none() && dir.join("state.json").exists() {
            return Err(format!(
                "{} cannot be read as a move's state. Look at it; if the unit is frozen, \
                 `systemctl thaw` it; then remove the file to start over",
                dir.join("state.json").display()
            ));
        }
        match Self::load(dir) {
            Some(state) if state.target != target || state.port != port => Err(format!(
                "a move to {} is recorded here: finish it with the same command, or `yulius \
                 instance migrate --abort`",
                state.target
            )),
            Some(state) => Ok(state),
            None => {
                let seed = format!("{}{}{target}", now_secs(), std::process::id());
                let state = Self {
                    version: 1,
                    id: hex::encode(&Sha256::digest(seed.as_bytes())[..4]),
                    target: target.to_owned(),
                    port,
                    ..Self::default()
                };
                state.save(dir)?;
                Ok(state)
            }
        }
    }

    /// Binds the move to the instance `opts` names, once. An abort is of the recorded
    /// instance whatever options it is given; a run of another instance is refused.
    ///
    /// # Errors
    /// The options name another instance than the move is of.
    pub fn for_instance(&mut self, opts: &mut Options, abort: bool) -> Result<(), String> {
        let named = Recorded {
            data_dir: opts.data_dir.clone(),
            service: opts.service.clone(),
            update_service: opts.update_service.clone(),
        };
        match &self.instance {
            None => self.instance = Some(named),
            Some(of) if abort => {
                opts.data_dir.clone_from(&of.data_dir);
                opts.service.clone_from(&of.service);
                opts.update_service.clone_from(&of.update_service);
            }
            Some(of) if *of != named => {
                return Err(format!(
                    "the move recorded here is of {} in {}: give the same --service, \
                     --update-service and --data-dir, or `yulius instance migrate --abort`",
                    of.service,
                    of.data_dir.display()
                ));
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Keeps the move.
    ///
    /// # Errors
    /// The state directory cannot be written.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&dir.join("state.json"), &json, 0o600)
            .map_err(|e| format!("cannot write {}: {e}", dir.join("state.json").display()))
    }
}

/// What a move needs of the machine and the person beside the two hosts.
pub trait World {
    /// systemd, for the freeze.
    fn freezer(&mut self) -> Box<dyn freeze::Host>;
    /// Signals are answered from now on: a thaw when the move froze the unit.
    ///
    /// # Errors
    /// Signals cannot be watched: nothing may be frozen then.
    fn arm(&mut self, unit: &str) -> Result<(), String>;
    /// The freeze was refused.
    fn refused(&mut self);
    /// The instance is frozen: `units` are what a signal thaws from now on, in that order (the
    /// server's unit and, where the host has one, its sessions unit before it, SPEC-458).
    fn frozen(&mut self, units: Vec<String>);
    /// The move takes the freeze's end itself; `false` when a signal already took it.
    fn claim(&mut self) -> bool;
    /// Asks the person; `None` without a terminal.
    fn confirm(&mut self, question: &str) -> Option<bool>;
    /// The addresses a host name leads to.
    fn addresses(&mut self, host: &str) -> Vec<String>;
    /// Whether `url` answers `ok`, connecting to `address` instead of the name's own.
    ///
    /// # Errors
    /// Why not.
    fn healthz(&mut self, url: &str, address: Option<&str>) -> Result<(), String>;
    /// Waits.
    fn sleep(&mut self, d: Duration);
    /// The wall clock: seconds and nanoseconds since the epoch.
    fn now(&mut self) -> (i64, i64);
    /// Syncs the filesystem that holds `dir` ([`super::export::sync`]).
    ///
    /// # Errors
    /// Why not.
    fn sync(&mut self, dir: &Path) -> Result<(), String>;
    /// Where and when this host is, for the receipts ([`super::receipts::Era::here`]).
    fn era(&mut self) -> super::receipts::Era;
    /// How many entries of `dir` a second a pass looks at once they are cached
    /// ([`warm::sampled`]); `None` when it cannot be measured.
    fn walk_rate(&mut self, dir: &Path) -> Option<f64>;
    /// The commit the instance listening at the loopback `address` reports; `None` while it
    /// does not answer.
    fn running(&mut self, address: &str) -> Option<String>;
    /// This build's binary.
    fn exe(&self) -> PathBuf;
    /// This build's commit.
    fn build(&self) -> Option<String>;
    /// Asks the instance of `data_dir`, at its loopback listener `address`, whether GitHub
    /// accepts the sign-in callback of `public_url` (SPEC-466 MVC-021).
    fn github(&mut self, data_dir: &Path, address: &str, public_url: &str) -> github::Asked;
    /// Prints a line for the person.
    fn show(&mut self, text: &str);
}

/// The old instance as systemd shows it: the server's unit and, where the host has one that
/// runs, its sessions unit (SPEC-458).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Old {
    /// The server's unit is active.
    active: bool,
    /// The server's unit has stopped (`inactive` or `failed`): it is neither active nor on its
    /// way up or down (SPEC-467 LFT-002).
    stopped: bool,
    /// Its sessions unit runs and is not frozen.
    sessions_run: bool,
    /// Something of it is frozen: it is held, by this move, an earlier run or an export.
    any_frozen: bool,
    /// Nothing of it runs unfrozen: what the new instance's start waits for.
    all_frozen: bool,
}

impl Old {
    /// Nothing of it runs, as far as systemd says: the server's unit has stopped and its
    /// sessions unit, where it runs, is frozen. (What a unit stopped by hand left running is
    /// not looked at here: issue #1178.)
    fn down(self) -> bool {
        self.stopped && !self.sessions_run
    }
}

/// What the cutover says when the old instance is not frozen as a whole right after its
/// freeze: what systemd said, not more (SPEC-467 LFT-001).
fn not_frozen_after_freeze(unit: &str, old: Option<Old>) -> String {
    match old {
        // Stopped since the plan: found before the frozen pass, not after it.
        Some(old) if !old.active => format!(
            "{unit} is not running any more: start it (`systemctl start {unit}`), so the move \
             can freeze everything it runs"
        ),
        Some(_) => format!(
            "{unit} runs and is not frozen as a whole (a unit of it was thawed or started from \
             outside right after the freeze): nothing is copied frozen and the staged copy \
             stays. Run the command again"
        ),
        None => format!(
            "systemd does not say whether {unit} is frozen as a whole (`systemctl show` failed \
             right after the freeze): nothing is copied frozen and the staged copy stays. Run \
             the command again when it answers"
        ),
    }
}

/// Why a run ends before its last step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Refused before anything changed (exit 2).
    Refused,
    /// A step failed (exit 1).
    Failed,
    /// Nothing more to do now: the plan was shown, or a person has to act first (exit 0).
    Ended,
}

fn bytes(n: u64) -> String {
    yulius_import::human_bytes(n)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A line of the new host as the person's terminal may show it: no control characters.
fn shown(mut line: HostLine) -> HostLine {
    let plain = |text: &str| text.replace(|c: char| c.is_control(), " ");
    line.name = plain(&line.name);
    line.detail = plain(&line.detail);
    line
}

fn failed(lines: &[HostLine]) -> bool {
    !host_setup::passed(lines)
}

fn said(host: &mut dyn Host, argv: &[&str]) -> String {
    host.run(&Cmd::new(argv))
        .map(|out| out.stdout.trim().to_owned())
        .unwrap_or_default()
}

/// A new public URL as the person gave it: `https://host`, nothing after it (the proxy
/// serves it on 443; a port would not survive the host's rewrite in the carried files). The
/// rule the running instance applies to the URL it is asked about (SPEC-466 MVC-020).
fn url_ok(url: &str) -> bool {
    yulius_app::github_callback::public_url_ok(url)
}

/// One move, from the old host.
pub struct Move<'a> {
    opts: &'a Options,
    local: &'a mut dyn Host,
    remote: &'a mut dyn Remote,
    world: &'a mut dyn World,
    state: State,
    /// What the new host holds of the staged copy (SPEC-465).
    stage: Stage,
    /// The build the old instance reported when the plan looked.
    ran: Option<String>,
    phase: &'static str,
    lines: Vec<HostLine>,
    /// The one-time key's public line, while a person has to add it (SPEC-470 MVB-010).
    key_line: Option<String>,
}

impl<'a> Move<'a> {
    /// The move `state` with `opts`, over the old host `local` and the new host `remote`.
    pub fn new(
        opts: &'a Options,
        state: State,
        local: &'a mut dyn Host,
        remote: &'a mut dyn Remote,
        world: &'a mut dyn World,
    ) -> Self {
        Self {
            opts,
            local,
            remote,
            world,
            state,
            stage: Stage::open(&opts.state_dir.join("copy")),
            ran: None,
            phase: "Plan",
            lines: Vec::new(),
            key_line: None,
        }
    }

    fn save_status(&self) {
        let status = Status {
            version: 1,
            id: self.state.id.clone(),
            target: self.state.target.clone(),
            phase: self.phase.to_owned(),
            lines: self.lines.clone(),
            updated_at: now_secs(),
            public_key: self.key_line.clone(),
            new_url: self.state.new_url.clone(),
        };
        if let Ok(json) = serde_json::to_vec(&status) {
            let _ = write_atomic(&self.opts.state_dir.join("status.json"), &json, 0o644);
        }
    }

    fn phase(&mut self, name: &'static str) {
        self.phase = name;
        self.world.show(&format!("\n{name}"));
        self.save_status();
    }

    fn push(&mut self, line: HostLine) {
        self.world.show(&host_setup::format_line(&line));
        self.lines.push(line);
        self.save_status();
    }

    /// A line of the new host; `host setup` runs there in parts that repeat its first items,
    /// which are said once.
    fn push_once(&mut self, line: HostLine) {
        let repeated = matches!(line.outcome, HostOutcome::Ok | HostOutcome::Manual)
            && self.lines.contains(&line);
        if !repeated {
            self.push(line);
        }
    }

    fn line(&mut self, name: &str, outcome: HostOutcome, detail: impl Into<String>) {
        self.push(HostLine {
            name: name.to_owned(),
            outcome,
            detail: detail.into(),
        });
    }

    fn ok(&mut self, name: &str, detail: impl Into<String>) {
        self.line(name, HostOutcome::Ok, detail);
    }

    fn changed(&mut self, name: &str, detail: impl Into<String>) {
        self.line(name, HostOutcome::Changed, detail);
    }

    fn manual(&mut self, name: &str, detail: impl Into<String>) {
        self.line(name, HostOutcome::Manual, detail);
    }

    fn fail(&mut self, name: &str, detail: impl Into<String>) -> Stop {
        self.line(name, HostOutcome::Failed, detail);
        Stop::Failed
    }

    /// Runs a step on the new host and takes its lines; a failed line or no answer stops.
    fn call(&mut self, name: &str, request: &Request) -> Result<Reply, Stop> {
        let reply = match self.remote.call(request) {
            Ok(reply) => reply,
            Err(why) => return Err(self.fail(name, format!("the new host: {why}"))),
        };
        let stop = failed(&reply.lines);
        for line in reply.lines.clone() {
            self.push_once(shown(line));
        }
        if stop { Err(Stop::Failed) } else { Ok(reply) }
    }

    fn save_state(&mut self) -> Result<(), Stop> {
        self.state
            .save(&self.opts.state_dir)
            .map_err(|why| self.fail("state", why))
    }

    fn setup(&self, p: &Profile) -> Setup {
        Setup {
            data_dir: self.opts.data_dir.clone(),
            user: p.user.clone(),
            service: self.opts.service.clone(),
            exe: p.exe.clone(),
        }
    }

    fn target(&self) -> &str {
        &self.state.target
    }

    /// Runs the move; the exit code.
    pub fn run(mut self) -> u8 {
        let end = self.steps();
        let code = match end {
            Err(Stop::Refused) => REFUSED,
            Err(Stop::Failed) => 1,
            Ok(()) | Err(Stop::Ended) => u8::from(failed(&self.lines)),
        };
        if end.is_ok() {
            self.phase = "done";
        }
        let summary = host_setup::summary(&self.lines, Mode::Setup);
        self.world.show(&format!("\n{summary}"));
        self.save_status();
        code
    }

    fn steps(&mut self) -> Result<(), Stop> {
        self.world.show("Plan");
        let profile = self.old_host()?;
        if self.state.finished {
            self.ok(
                "move",
                format!(
                    "finished: the instance runs on {} at {}",
                    self.state.target,
                    self.state.new_url.as_deref().unwrap_or("its URL")
                ),
            );
            return Ok(());
        }
        match self.state.cutover {
            Cutover::Moved => {}
            // A run that ended while the new instance was starting: its state decides, and
            // nothing is planned or copied again.
            Cutover::Starting => {
                self.key_again()?;
                self.resume_start(&profile)?;
            }
            Cutover::No | Cutover::RolledBack => {
                let facts = self.plan(&profile)?;
                if failed(&self.lines) {
                    return Err(Stop::Failed);
                }
                self.confirm()?;
                self.prepare(&profile, &facts)?;
                self.warm(&profile, &facts)?;
                self.cutover(&profile, &facts)?;
            }
        }
        self.after(&profile)?;
        self.finish(&profile);
        Ok(())
    }

    /// The old host's profile and the recorded move; refuses what cannot be moved from here.
    fn old_host(&mut self) -> Result<Profile, Stop> {
        const ITEM: &str = "old host";
        let opts = self.opts;
        let refuse = |me: &mut Self, why: String| {
            me.line(ITEM, HostOutcome::Failed, why);
            Stop::Refused
        };
        let profile = Profile::read(
            self.local,
            &opts.data_dir,
            &opts.service,
            &opts.update_service,
        )
        .map_err(|why| refuse(self, why))?;
        let group = said(
            self.local,
            &[
                "systemctl",
                "show",
                "--property=ControlGroup",
                "--value",
                &opts.service,
            ],
        );
        let own = self
            .local
            .read(Path::new("/proc/self/cgroup"))
            .and_then(|text| {
                text.lines()
                    .find_map(|l| l.strip_prefix("0::").map(str::to_owned))
            });
        // The move freezes the sessions unit too, where the host has one (SPEC-458).
        let sessions_group = yulius_sandbox::units::sessions_unit(&opts.service)
            .map(|sessions| {
                said(
                    self.local,
                    &[
                        "systemctl",
                        "show",
                        "--property=ControlGroup",
                        "--value",
                        &sessions,
                    ],
                )
            })
            .unwrap_or_default();
        let inside = |group: &str| {
            !group.is_empty()
                && own
                    .as_deref()
                    .is_some_and(|own| own == group || own.starts_with(&format!("{group}/")))
        };
        if inside(&group) || inside(&sessions_group) {
            return Err(refuse(
                self,
                format!(
                    "this command runs inside {} (a Yulius terminal?) and would freeze itself; \
                     run it over SSH on the host",
                    opts.service
                ),
            ));
        }
        let before = matches!(self.state.cutover, Cutover::No | Cutover::RolledBack);
        match self.old_unit() {
            Some(old) if old.active => {}
            // What a stopped unit left running cannot be frozen, and a unit that is stopped
            // or stopping cannot be told from one stopped under the move.
            Some(_) if before => {
                return Err(refuse(
                    self,
                    format!(
                        "{0} is not running: start it (`systemctl start {0}`), so the move can \
                         freeze everything it runs. An instance that cannot start is copied \
                         with `yulius instance export` instead (docs/operations.md §7)",
                        opts.service
                    ),
                ));
            }
            None if before => {
                return Err(refuse(
                    self,
                    format!("systemd does not say what state {} is in", opts.service),
                ));
            }
            _ => {}
        }
        if before && self.old_held() {
            let thaw = self
                .thaw_command()
                .unwrap_or_else(|| format!("systemctl thaw {}", opts.service));
            return Err(refuse(
                self,
                format!(
                    "{} is frozen (an earlier run that was killed, or an export in progress). \
                     When nothing else holds it, `{thaw}`, then run the command again",
                    opts.service
                ),
            ));
        }
        if said(
            self.local,
            &["systemctl", "is-active", &opts.update_service],
        ) == "active"
        {
            return Err(refuse(
                self,
                format!(
                    "an update is running ({}); move once it has finished",
                    opts.update_service
                ),
            ));
        }
        if self.world.build().is_none() {
            return Err(refuse(
                self,
                "this build names no commit (a development build), so the new instance could \
                 not be told from another: move with a released build"
                    .to_owned(),
            ));
        }
        self.ok(
            ITEM,
            format!(
                "{}, {}, {} CPUs, {} of memory; {} runs as {} with {} used where its data is",
                profile.release.name(),
                profile.arch,
                profile.cpus,
                bytes(profile.memory),
                profile.unit,
                profile.user,
                bytes(profile.used_bytes)
            ),
        );
        Ok(profile)
    }

    /// The new host's key as first seen, for the person to compare with the host's own.
    fn host_key_note(&mut self) -> String {
        self.remote
            .host_key()
            .map(|key| format!(" (its host key: {key})"))
            .unwrap_or_default()
    }

    fn key(&mut self) -> Result<(), Stop> {
        match self.remote.reachable() {
            Ok(()) => {
                self.key_signs_in();
                Ok(())
            }
            Err(_) => self.add_key(),
        }
    }

    fn key_signs_in(&mut self) {
        let seen = self.host_key_note();
        self.ok(
            "one-time key",
            format!("signs in to {}{seen}", self.target()),
        );
    }

    /// Makes the one-time key when there is none and adds it to the new host through the
    /// person's own sign-in; without one, the line to add is shown and the run ends.
    fn add_key(&mut self) -> Result<(), Stop> {
        const ITEM: &str = "one-time key";
        let public = self.remote.key().map_err(|why| self.fail(ITEM, why))?;
        if self.opts.no_sign_in {
            // Asked without a person at this host (SPEC-470): one look with the key itself,
            // which also records the new host's key for the person to compare, and never
            // root's own sign-in, which would reach every host that trusts root here.
            return match self.remote.reachable() {
                Ok(()) => {
                    self.key_signs_in();
                    Ok(())
                }
                Err(_) => {
                    let seen = self.host_key_note();
                    self.key_line = Some(public.clone());
                    self.manual(
                        ITEM,
                        format!(
                            "is not on the new host yet{seen}. This run never signs in as root \
                             does (root's keys, its ssh configuration, a password): add this \
                             line to root's ~/.ssh/authorized_keys on the new host, then ask \
                             again: {public}"
                        ),
                    );
                    Err(Stop::Ended)
                }
            };
        }
        self.world.show(&format!(
            "Signing in to {} once to add the move's key (a password or a second factor may be \
             asked):",
            self.target()
        ));
        let added = self
            .remote
            .install_key()
            .and_then(|()| self.remote.reachable());
        match added {
            Ok(()) => {
                let seen = self.host_key_note();
                self.changed(
                    ITEM,
                    format!(
                        "made and added to {} through your sign-in{seen}",
                        self.target()
                    ),
                );
                Ok(())
            }
            Err(why) => {
                let seen = self.host_key_note();
                // For readers of the status (Settings → Instance shows it with a copy button).
                self.key_line = Some(public.clone());
                self.manual(
                    ITEM,
                    format!(
                        "could not be added ({why}){seen}: add this line to root's \
                         ~/.ssh/authorized_keys on the new host, then run the command again: \
                         {public}"
                    ),
                );
                Err(Stop::Ended)
            }
        }
    }

    /// The one-time key in a run that goes on from a recorded start. The run that ordered the
    /// start had added it, so a new host that does not answer is not a key to add again: it is
    /// waited for, and nothing is decided or touched meanwhile. A key that is missing here or
    /// refused there is made and added as in a plan. One answer decides which it is.
    fn key_again(&mut self) -> Result<(), Stop> {
        match self.remote.reachable() {
            Ok(()) => {
                self.key_signs_in();
                Ok(())
            }
            Err(why) if why.contains(remote::KEY_REFUSED) || why.contains(remote::NO_KEY) => {
                self.add_key()
            }
            Err(why) => Err(self.fail(
                "new host",
                format!(
                    "cannot be reached ({why}), so nobody knows whether the new instance runs. \
                     Nothing was started, stopped, frozen or thawed: run the command again \
                     when the new host answers (it decides from the new instance's state); \
                     `yulius instance migrate --abort` only once the new instance is known to \
                     be stopped"
                ),
            )),
        }
    }

    /// The new host's facts, with this build staged there.
    fn build(&mut self, p: &Profile) -> Result<Facts, Stop> {
        const ITEM: &str = "yulius build";
        let request = Request::Facts {
            data_dir: self.opts.data_dir.clone(),
            service: self.opts.service.clone(),
            ports: {
                let mut ports = p.ports.clone();
                if p.proxy == Carried::Yes {
                    ports.extend([80, 443]);
                }
                ports
            },
        };
        let same = |facts: &Facts, build: &Option<String>| {
            facts.build == *build && facts.version == env!("CARGO_PKG_VERSION")
        };
        let build = self.world.build();
        let staged = self.remote.try_call(&request).and_then(|r| r.facts);
        if let Some(facts) = staged.filter(|facts| same(facts, &build)) {
            self.ok(ITEM, "the new host holds this build");
            return Ok(facts);
        }
        let exe = self.world.exe();
        self.remote
            .put_build(&exe)
            .map_err(|why| self.fail(ITEM, format!("cannot copy {}: {why}", exe.display())))?;
        let facts = match self.remote.call(&request) {
            Ok(Reply {
                facts: Some(facts), ..
            }) if same(&facts, &build) => facts,
            Ok(_) => {
                return Err(self.fail(ITEM, "the copied build reports another commit there"));
            }
            Err(why) => {
                return Err(self.fail(
                    ITEM,
                    format!(
                        "the copied build does not run on the new host (another architecture, \
                         an older libc?): {why}"
                    ),
                ));
            }
        };
        self.changed(ITEM, format!("copied this build to {}", target::STAGED));
        Ok(facts)
    }

    fn plan(&mut self, p: &Profile) -> Result<Facts, Stop> {
        let opts = self.opts;
        self.key()?;
        let facts = self.build(p)?;
        let address = facts.address.clone().unwrap_or_default();
        let described = format!(
            "{}, {}, systemd {}, {} CPUs, {} of memory, at {address}",
            facts.release.name(),
            facts.arch,
            facts.systemd,
            facts.cpus,
            bytes(facts.memory)
        );
        let data = opts.data_dir.display();
        match facts.instance {
            Instance::Running => {
                self.fail(
                    "new host",
                    format!("{described}: it runs a Yulius instance"),
                );
            }
            Instance::Present => {
                self.fail(
                    "new host",
                    format!(
                        "{described}: {data} there holds an instance (after a move that was \
                         rolled back: the copy that was started). Remove it there, then run the \
                         command again"
                    ),
                );
            }
            Instance::None | Instance::Incomplete => {
                if self.state.cutover == Cutover::RolledBack {
                    self.state.cutover = Cutover::No;
                    self.save_state()?;
                }
                self.ok("new host", described);
            }
        }
        if !facts.busy_ports.is_empty() {
            self.fail(
                "ports",
                format!(
                    "another program holds {} on the new host",
                    facts.busy_ports.join(", ")
                ),
            );
        }
        // What an earlier run staged there is found before the plan speaks of room and time.
        self.agree()?;
        // The staged copy keeps its marker under names that no entry of the copy may have.
        for name in super::import::MARKERS {
            if std::fs::symlink_metadata(opts.data_dir.join(name)).is_ok() {
                self.fail(
                    "old host",
                    format!(
                        "{data} holds {name}, the mark of a copy that was never finished. \
                         Remove that file when this instance is whole, then run the command again"
                    ),
                );
            }
        }
        self.ran = self.world.running(&self.health_address(p));
        self.space(p, &facts);
        match self.remote.measure(LINK_TEST) {
            Ok(took) => {
                let rate = (LINK_TEST as f64 / took.as_secs_f64().max(0.001)) as u64;
                self.ok("link", format!("{}/s to the new host", bytes(rate)));
                self.plan_downtime(p, rate);
            }
            Err(why) => {
                self.fail("link", why);
            }
        }
        self.url(p, &address)?;
        // What a third-party source provides is known there only once the source is added.
        let names: Vec<String> = p
            .packages
            .iter()
            .filter(|name| !p.third_party.contains(name))
            .cloned()
            .collect();
        self.call(
            "packages",
            &Request::Packages {
                names,
                install: false,
            },
        )?;
        if !p.sources.is_empty() {
            self.ok(
                "apt sources",
                format!(
                    "{} third-party sources are added while preparing; {} of the packages come \
                     from them",
                    p.sources.len(),
                    p.third_party.len()
                ),
            );
        }
        self.carried(p);
        Ok(facts)
    }

    fn space(&mut self, p: &Profile, facts: &Facts) {
        const ITEM: &str = "disk space";
        let disks = &self.opts.data_disks;
        let blank = || {
            facts
                .blank
                .iter()
                .map(|d| format!("{} ({})", d.path, bytes(d.bytes)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if !disks.is_empty() && !facts.mounted {
            let sizes: Vec<Option<u64>> = disks
                .iter()
                .map(|d| facts.blank.iter().find(|b| &b.path == d).map(|b| b.bytes))
                .collect();
            let smallest = sizes.iter().flatten().min().copied().unwrap_or(0);
            if disks.len() > 2 || sizes.iter().any(Option::is_none) {
                self.fail(
                    ITEM,
                    format!(
                        "--data-disk takes one blank disk, or two for a mirror; blank on the new \
                         host: {}",
                        if facts.blank.is_empty() {
                            "none".to_owned()
                        } else {
                            blank()
                        }
                    ),
                );
            } else if smallest < p.used_bytes {
                self.fail(
                    ITEM,
                    format!(
                        "{} is smaller than the {} to copy",
                        bytes(smallest),
                        bytes(p.used_bytes)
                    ),
                );
            } else {
                self.ok(
                    ITEM,
                    format!(
                        "will make {} of {} for the data directory ({} to copy)",
                        if disks.len() == 2 { "a mirror" } else { "ext4" },
                        disks.join(" and "),
                        bytes(p.used_bytes)
                    ),
                );
            }
            return;
        }
        if self.stage.id().is_some() {
            // Its room was found when the copy began; what is free now is for what changed.
            return self.ok(
                ITEM,
                format!(
                    "the staged copy is there already; {} more is free where the data \
                     directory goes",
                    bytes(facts.free_bytes)
                ),
            );
        }
        let inodes = facts.free_inodes == 0 || facts.free_inodes >= p.used_inodes;
        if facts.free_bytes >= p.used_bytes && inodes {
            return self.ok(
                ITEM,
                format!(
                    "{} free where the data directory goes, at most {} to copy",
                    bytes(facts.free_bytes),
                    bytes(p.used_bytes)
                ),
            );
        }
        let mut why = format!(
            "{} and {} inodes free where the data directory goes; the copy needs up to {} and \
             {} inodes",
            bytes(facts.free_bytes),
            facts.free_inodes,
            bytes(p.used_bytes),
            p.used_inodes
        );
        if !facts.blank.is_empty() {
            why.push_str(&format!(
                ". Blank disks there: {}: name one, or two for a mirror, with --data-disk",
                blank()
            ));
        }
        self.fail(ITEM, why);
    }

    /// Decides the new public URL and says what a person has to register for it.
    fn url(&mut self, p: &Profile, address: &str) -> Result<(), Stop> {
        const ITEM: &str = "public URL";
        let old = p.public_url.clone();
        let new = match (&self.opts.public_url, &old) {
            (Some(url), _) if url_ok(url.trim_end_matches('/')) => {
                url.trim_end_matches('/').to_owned()
            }
            (Some(url), _) => {
                self.fail(ITEM, format!("{url:?} is not `https://host`"));
                return Ok(());
            }
            (None, Some(old)) => profile::address_url(old, address).unwrap_or_else(|| old.clone()),
            (None, None) => {
                self.fail(
                    ITEM,
                    "the unit names no YULIUS_PUBLIC_URL: give the new one with --public-url",
                );
                return Ok(());
            }
        };
        let host = profile::host_of(&new).unwrap_or_default();
        let leads = self.world.addresses(&host);
        let was = old
            .as_ref()
            .filter(|old| **old != new)
            .map(|old| format!(" (was {old})"))
            .unwrap_or_default();
        if leads.iter().any(|a| a == address) {
            self.ok(ITEM, format!("{new}{was}: {host} leads to the new host"));
        } else {
            self.manual(
                ITEM,
                format!(
                    "{new}{was}: {host} leads to {}, not to {address}. Change its DNS record \
                     when the checklist says the new instance runs (lower its TTL now)",
                    if leads.is_empty() {
                        "nowhere".to_owned()
                    } else {
                        leads.join(", ")
                    }
                ),
            );
        }
        if old.as_ref().is_some_and(|old| *old != new) {
            // The old instance runs: it knows its App and asks GitHub about the new callback,
            // so what a person has to register is exact, and said before anything freezes.
            let address = self.health_address(p);
            let asked = self.world.github(&self.opts.data_dir, &address, &new);
            let (outcome, detail) = github::line(asked, &new);
            self.line("GitHub App", outcome, detail);
        }
        if self.state.new_url.as_ref() != Some(&new) {
            self.state.new_url = Some(new);
            self.save_state()?;
        }
        Ok(())
    }

    /// What travels beside the data, said before anything is changed.
    fn carried(&mut self, p: &Profile) {
        if !p.trees.is_empty() {
            let trees: Vec<String> = p
                .trees
                .iter()
                .map(|t| format!("{} ({})", t.path.display(), bytes(t.bytes)))
                .collect();
            self.ok(
                "software",
                format!("copies {} as they are", trees.join(", ")),
            );
        }
        let mut settings = format!(
            "carries the unit with {} drop-ins and {} key, environment and other files",
            p.units.len().saturating_sub(1),
            p.files.len()
        );
        if !p.pins.is_empty() {
            settings.push_str(&format!(
                "; limits pinned for this machine travel and are re-sized by the new instance: {}",
                p.pins.join(", ")
            ));
        }
        self.ok("settings", settings);
        if !p.stale.is_empty() {
            let files: Vec<String> = p.stale.iter().map(|f| f.display().to_string()).collect();
            self.manual(
                "settings",
                format!(
                    "carried as they are although they name the old host: {}. Edit them on \
                     the new host after the move and restart the service there",
                    files.join(", ")
                ),
            );
        }
        if p.helper {
            self.ok(
                "update helper",
                "its units and the installed commit are carried",
            );
        } else {
            self.ok(
                "update helper",
                "none on the old host: the new one offers no Update now either",
            );
        }
        // The helper of Settings → Instance's "Move to another host" (SPEC-470 MVB-008).
        self.ok(
            "move helper",
            match p.move_helper {
                Some(true) => {
                    "its units are carried, and its path unit is enabled on the new host as it \
                     is here"
                }
                Some(false) => {
                    "its units are carried; its path unit is not enabled here, so it is not \
                     enabled on the new host"
                }
                None => {
                    "none on the old host: the new one offers no move from Settings → Instance \
                     either"
                }
            },
        );
        let host = self
            .state
            .new_url
            .as_deref()
            .and_then(profile::host_of)
            .unwrap_or_default();
        match &p.proxy {
            Carried::Yes => self.ok(
                "reverse proxy",
                format!("Caddy's configuration is carried for {host}"),
            ),
            Carried::Other(name) => self.manual(
                "reverse proxy",
                format!(
                    "{name} is not carried: set up a proxy for {host} on the new host before \
                     going on (docs/operations.md §4)"
                ),
            ),
            Carried::None => self.ok("reverse proxy", "none on the old host"),
        }
        match &p.firewall {
            Carried::Yes => self.ok("firewall", "ufw's rules are carried and enabled at the end"),
            Carried::Other(name) => self.manual(
                "firewall",
                format!("{name} is not carried: set up the new host's firewall by hand"),
            ),
            Carried::None => self.ok("firewall", "none on the old host"),
        }
    }

    fn confirm(&mut self) -> Result<(), Stop> {
        if self.opts.yes {
            return Ok(());
        }
        let question = format!(
            "Move this instance to {}? The steps marked manual above are yours.",
            self.target()
        );
        match self.world.confirm(&question) {
            Some(true) => Ok(()),
            Some(false) => {
                self.world.show("Nothing was changed.");
                Err(Stop::Ended)
            }
            None => {
                self.world
                    .show("This was the plan; nothing was changed. Run it with --yes to move.");
                Err(Stop::Ended)
            }
        }
    }

    fn send(&mut self, name: &str, paths: &[PathBuf], what: &str) -> Result<(), Stop> {
        if paths.is_empty() {
            return Ok(());
        }
        match self.remote.send_paths(paths) {
            Ok(()) => {
                self.changed(name, format!("copied {what}"));
                Ok(())
            }
            Err(why) => Err(self.fail(name, why)),
        }
    }

    fn prepare(&mut self, p: &Profile, facts: &Facts) -> Result<(), Stop> {
        self.phase("Prepare");
        let setup = self.setup(p);
        if !self.opts.data_disks.is_empty() {
            self.call(
                "data disk",
                &Request::Disk {
                    setup: setup.clone(),
                    disks: self.opts.data_disks.clone(),
                },
            )?;
        }
        if let Err(why) = self.remote.send_paths(&p.keys) {
            return Err(self.fail("apt sources", format!("cannot copy their keys: {why}")));
        }
        let (old, new) = (&p.release.codename, &facts.release.codename);
        let same_distribution = p.release.id == facts.release.id;
        let files = p
            .sources
            .iter()
            .map(|file| SourceFile {
                file: TextFile {
                    content: if same_distribution {
                        profile::with_codename(&file.content, old, new)
                    } else {
                        file.content.clone()
                    },
                    ..file.clone()
                },
                fallback: file.content.clone(),
            })
            .collect();
        self.call("apt sources", &Request::Sources { files })?;
        self.call(
            "packages",
            &Request::Packages {
                names: p.packages.clone(),
                install: true,
            },
        )?;
        self.call(
            "coreutils",
            &Request::Coreutils {
                flavour: p.coreutils.clone(),
            },
        )?;
        let trees: Vec<PathBuf> = p.trees.iter().map(|t| t.path.clone()).collect();
        let total: u64 = p.trees.iter().map(|t| t.bytes).sum();
        self.send("software", &trees, &format!("{} outside apt", bytes(total)))?;
        self.call(
            "npm packages",
            &Request::Npm {
                packages: p.npm.clone(),
            },
        )?;
        self.call(
            "host setup",
            &Request::HostItems {
                setup: setup.clone(),
            },
        )?;
        self.call(
            "data directory",
            &Request::DataDir {
                setup: setup.clone(),
                stage: self.stage.id().map(str::to_owned),
            },
        )?;
        self.call("yulius build", &Request::Binary { setup })?;
        self.send(
            "carried files",
            &p.files,
            &format!("{} files and directories as they are", p.files.len()),
        )?;
        let old_host = p
            .public_url
            .as_deref()
            .and_then(profile::host_of)
            .unwrap_or_default();
        let new_host = self
            .state
            .new_url
            .as_deref()
            .and_then(profile::host_of)
            .unwrap_or_default();
        let moved = |file: &TextFile| TextFile {
            content: profile::with_host(&file.content, &old_host, &new_host),
            ..file.clone()
        };
        self.call(
            "unit",
            &Request::Texts {
                name: "unit".into(),
                files: p.units.iter().map(moved).collect(),
            },
        )?;
        if let Some(caddyfile) = &p.caddyfile {
            let wrote = self.call(
                "reverse proxy",
                &Request::Texts {
                    name: "reverse proxy files".into(),
                    files: vec![moved(caddyfile)],
                },
            )?;
            let changed = wrote
                .lines
                .iter()
                .any(|l| l.outcome == HostOutcome::Changed);
            self.call("reverse proxy", &Request::Proxy { changed })?;
        }
        // The unit's drop-in, the binary's owner and the sandbox base: the host is proven ready
        // while the old instance still runs, not found wanting after the copy.
        self.call(
            "host setup",
            &Request::BeforeStart {
                setup: self.setup(p),
            },
        )?;
        // And nothing it made stands in the import's way.
        self.call(
            "data directory",
            &Request::DataDir {
                setup: self.setup(p),
                stage: self.stage.id().map(str::to_owned),
            },
        )?;
        Ok(())
    }

    /// The frozen pass and the proof of the whole copy; `Err` says why the old instance is to
    /// be thawed. The copy stays marked unfinished on the new host until [`Request::Mark`].
    fn copy(&mut self, p: &Profile, facts: &Facts, held: &mut Freeze) -> Result<(), Stop> {
        const ITEM: &str = "copy proven";
        let (old, manifest) = self.frozen_pass(facts, held)?;
        let request = Request::Inventory {
            manifest: Box::new(manifest),
            old: old.clone(),
            release: p.release.clone(),
        };
        let reply = match self.remote.call(&request) {
            Ok(reply) => reply,
            // Not counted is not disproven: the staged copy stays for the next run.
            Err(why) => return Err(self.fail(ITEM, format!("the new host: {why}"))),
        };
        let mut differs = failed(&reply.lines);
        for line in reply.lines {
            self.push_once(shown(line));
        }
        // The new host's word is not the proof: its count is compared here.
        match reply.inventory.as_ref() {
            Some(new) if *new == old => {}
            Some(new) if !differs => {
                differs = true;
                self.fail(
                    ITEM,
                    super::inventory::difference(&old, new)
                        .unwrap_or_else(|| "the new host's count is not this host's".into()),
                );
            }
            None if !differs => {
                differs = true;
                self.fail(ITEM, "the new host sent no count of the copy");
            }
            _ => {}
        }
        if differs {
            // Whatever made it differ, nothing more is built on that copy or on what this
            // host believed of it.
            self.stage.forget();
            self.changed(
                "copy",
                "the staged copy on the new host is not this host's data and is dropped: the \
                 next run copies everything again",
            );
            return Err(Stop::Failed);
        }
        Ok(())
    }

    /// What systemd says of the old instance; `None` when it does not say.
    fn old_unit(&mut self) -> Option<Old> {
        let unit = self.opts.service.clone();
        let (code, said) = self
            .world
            .freezer()
            .systemctl(&["show", "--property=ActiveState,FreezerState", &unit])
            .ok()?;
        let value = |key: &str| {
            said.lines()
                .find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
                .map(str::to_owned)
        };
        let (active, freezer) = (value("ActiveState")?, value("FreezerState")?);
        // Its sessions live in a unit of their own where the host has one (SPEC-458). Not
        // knowing its state is not knowing the instance's.
        let sessions = freeze::sessions_state(&mut *self.world.freezer(), &unit).ok()?;
        let frozen = freezer == "frozen";
        (code == 0).then_some(Old {
            active: active == "active",
            stopped: matches!(active.as_str(), "inactive" | "failed"),
            sessions_run: sessions == Some(false),
            any_frozen: frozen || sessions == Some(true),
            all_frozen: frozen && sessions != Some(false),
        })
    }

    /// The command that thaws the old instance: its sessions unit, where the host has one, and
    /// the server's unit; `None` when nothing of it can be thawed (a stopped server without a
    /// sessions unit).
    fn thaw_command(&mut self) -> Option<String> {
        let unit = self.opts.service.clone();
        let units = freeze::thaw_units(&mut *self.world.freezer(), &unit);
        (!units.is_empty()).then(|| format!("systemctl thaw {}", units.join(" ")))
    }

    /// The units of the old instance, for a command a person runs by hand: the server's and,
    /// where the host has one that runs, its sessions unit.
    fn old_units(&mut self) -> String {
        let unit = self.opts.service.clone();
        match yulius_sandbox::units::sessions_unit(&unit) {
            // Named also when systemd does not say whether it runs.
            Some(sessions)
                if !matches!(
                    freeze::sessions_state(&mut *self.world.freezer(), &unit),
                    Ok(None)
                ) =>
            {
                format!("{unit} {sessions}")
            }
            _ => unit,
        }
    }

    /// The command that stops the instance on the new host by hand. The new host is set up
    /// by this build, so its sessions live in their own unit, which a stop of the server's
    /// unit leaves running (SPEC-458).
    fn new_stop_command(&self) -> String {
        let unit = &self.opts.service;
        match yulius_sandbox::units::sessions_unit(unit) {
            Some(sessions) => {
                format!("systemctl disable --now {unit}; systemctl stop {sessions}")
            }
            None => format!("systemctl disable --now {unit}"),
        }
    }

    /// Whether the whole old instance is known to be frozen: the server's unit and, where it
    /// runs, its sessions unit. Not knowing is not frozen: nothing is started on a guess.
    fn old_frozen(&mut self) -> bool {
        self.old_unit().is_some_and(|old| old.all_frozen)
    }

    /// Whether something of the old instance is known to be frozen.
    fn old_held(&mut self) -> bool {
        self.old_unit().is_some_and(|old| old.any_frozen)
    }

    /// Thaws the old instance, whoever froze it: after a cutover that did not happen it runs.
    /// A server's unit that does not run (stopped from outside meanwhile) cannot be thawed and
    /// does not run as before: what is frozen of the instance is thawed, and the line says that
    /// the unit is down and how to start it.
    fn thaw(&mut self) {
        let unit = self.opts.service.clone();
        let state = self.old_unit();
        if state.is_some_and(|old| old.active && !old.any_frozen) {
            return self.ok("old instance", format!("{unit} runs; nothing to thaw"));
        }
        // Frozen, or systemd did not say: thawing a unit that runs changes nothing. Its
        // sessions unit thaws with it (SPEC-458): agents left frozen beside a server that runs
        // again would be worse than no thaw.
        let thawed = freeze::thaw_instance(&mut *self.world.freezer(), &unit);
        match (thawed, state) {
            (Err(_), _) => {
                let command = self
                    .thaw_command()
                    .unwrap_or_else(|| format!("systemctl thaw {unit}"));
                self.fail(
                    "old instance",
                    format!("`{command}` failed: run it by hand"),
                );
            }
            (Ok(()), Some(old)) if !old.active => {
                let sessions = if old.any_frozen {
                    " (its sessions were thawed)"
                } else {
                    ""
                };
                self.fail(
                    "old instance",
                    format!("{unit} is not running: start it (`systemctl start {unit}`){sessions}"),
                );
            }
            (Ok(()), _) => self.changed("old instance", "thawed: it runs as before"),
        }
    }

    /// Freezes the old unit for the cutover; a signal thaws it until the move claims its end.
    fn freeze(&mut self) -> Result<Freeze, Stop> {
        let opts = self.opts;
        if let Err(why) = self.world.arm(&opts.service) {
            return Err(self.fail("old instance", why));
        }
        match freeze::freeze(self.world.freezer(), &opts.service, &opts.update_service) {
            Ok(held) => {
                // Also when it was frozen already: the move answers for the freeze from here.
                self.world
                    .frozen(held.covered().iter().rev().cloned().collect());
                Ok(held)
            }
            Err(why) => {
                self.world.refused();
                Err(self.fail("old instance", why))
            }
        }
    }

    /// The move takes the freeze's end: no signal thaws from here, and no guard does when this
    /// process dies or loses its terminal, so the old instance never thaws beside a new one.
    fn claim(&mut self, held: Freeze) {
        if !self.world.claim() {
            // A signal took it first: it thaws and exits, nothing here may go on.
            loop {
                std::thread::park();
            }
        }
        held.leave_frozen();
    }

    fn start_request(&self, p: &Profile) -> Request {
        Request::Start {
            setup: self.setup(p),
            build: self.world.build(),
            health: self.health_address(p),
            helper: p
                .helper
                .then(|| self.opts.update_service.replace(".service", ".path")),
        }
    }

    fn health_address(&self, p: &Profile) -> String {
        format!("127.0.0.1:{}", p.ports.get(1).copied().unwrap_or(8081))
    }

    fn cutover(&mut self, p: &Profile, facts: &Facts) -> Result<(), Stop> {
        self.phase("Cutover");
        let unit = self.opts.service.clone();
        // The copy took a while. An instance that was updated meanwhile holds data the build
        // staged on the new host may not read.
        let runs = self.world.running(&self.health_address(p));
        if let (Some(ran), Some(runs)) = (&self.ran, &runs)
            && ran != runs
        {
            return Err(self.fail(
                "old instance",
                format!(
                    "{unit} reported build {ran} when the copy began and reports build {runs} \
                     now (an update during the copy). Nothing is frozen; the staged copy \
                     stays: run the command again with the build the instance runs"
                ),
            ));
        }
        let mut held = self.freeze()?;
        let state = self.old_unit();
        if !state.is_some_and(|old| old.all_frozen) {
            // Found before the frozen pass, not after it. The move takes the freeze's end
            // first, so what is said and thawed here is the only answer to it.
            let covered: Vec<String> = held.covered().iter().rev().cloned().collect();
            self.claim(held);
            let why = not_frozen_after_freeze(&unit, state);
            self.fail("old instance", why);
            match state {
                // A unit of it runs unfrozen, as systemd says: thawed by what systemd says.
                Some(old) if old.active => self.thaw(),
                // The server's unit does not run, as the line says, or systemd does not say
                // what runs. What the freeze covered is thawed as the freeze knew it, in the
                // order a thaw takes it and without another question. A server's unit that
                // does not run is left out: systemd thaws none (stopped, or with a job
                // pending), nothing of it is frozen, and only a failed thaw is said then.
                _ => {
                    let down = state.is_some();
                    let units: Vec<String> = covered
                        .into_iter()
                        .filter(|name| !(down && *name == unit))
                        .collect();
                    match freeze::thaw_each(&mut *self.world.freezer(), &units) {
                        Ok(()) if down || units.is_empty() => {}
                        Ok(()) => self.changed("old instance", "thawed: it runs as before"),
                        Err(why) => {
                            self.fail("old instance", why);
                        }
                    }
                }
            }
            return Err(Stop::Failed);
        }
        self.changed(
            "old instance",
            format!("{unit} is frozen: every Yulius process is paused until the new one runs"),
        );
        let copied = self.copy(p, facts, &mut held);
        self.claim(held);
        if let Err(stop) = copied {
            self.thaw();
            return Err(stop);
        }
        if !self.old_frozen() {
            self.fail(
                "old instance",
                format!(
                    "{unit} is no longer known to be frozen (thawed or stopped from outside \
                     during the copy, or systemd does not answer): nothing is started"
                ),
            );
            // Nothing was started, so it may run: not left frozen without a word.
            self.thaw();
            return Err(Stop::Failed);
        }
        // Recorded first: a run that dies from here on is taken up from its state, with the
        // copy still unfinished on the new host until it is marked.
        self.state.cutover = Cutover::Starting;
        if let Err(stop) = self.save_state() {
            self.thaw();
            return Err(stop);
        }
        // Disabled before the start is ordered: while nobody knows whether the new instance
        // runs, a restart of this host must not start the old one beside it.
        let marked = if matches!(
            self.world.freezer().systemctl(&["disable", &unit]),
            Ok((0, _))
        ) {
            self.call(
                "move mark",
                &Request::Mark {
                    setup: self.setup(p),
                    from: p.public_url.clone(),
                    to: self.state.new_url.clone().unwrap_or_default(),
                },
            )
            .map(|_| ())
        } else {
            Err(self.fail(
                "old instance",
                format!("`systemctl disable {unit}` failed: nothing is started"),
            ))
        };
        if let Err(stop) = marked {
            self.state.cutover = Cutover::No;
            let _ = self.save_state();
            self.enable_old();
            self.thaw();
            return Err(stop);
        }
        self.start(p)
    }

    /// The old unit starts again with its host: the move did not happen.
    fn enable_old(&mut self) {
        let unit = self.opts.service.clone();
        if !matches!(
            self.world.freezer().systemctl(&["enable", &unit]),
            Ok((0, _))
        ) {
            self.fail(
                "old instance",
                format!("`systemctl enable {unit}` failed: run it by hand"),
            );
        }
    }

    /// Orders the new instance's start and decides the move by whether it runs: once it
    /// reports the build it has taken over (its first start queued the tasks), so whatever
    /// fails after that is a line, never a rollback.
    fn start(&mut self, p: &Profile) -> Result<(), Stop> {
        let started = match self.remote.call(&self.start_request(p)) {
            Ok(reply) => {
                for line in reply.lines {
                    self.push_once(shown(line));
                }
                reply.started
            }
            Err(why) => {
                // The answer was lost, not necessarily the start: the new host says.
                let asked = self.remote.call(&Request::Health {
                    build: self.world.build(),
                    health: self.health_address(p),
                });
                match asked {
                    Ok(reply) => reply.started,
                    Err(again) => {
                        let unit = self.opts.service.clone();
                        self.fail(
                            "new instance",
                            format!(
                                "the new host stopped answering ({why}; {again}), so nobody \
                                 knows whether it runs. {unit} stays frozen here: run the \
                                 command again when the new host answers (it decides from the \
                                 new instance's state), or `yulius instance migrate --abort`"
                            ),
                        );
                        return Err(Stop::Failed);
                    }
                }
            }
        };
        if started {
            self.moved()
        } else {
            self.roll_back()
        }
    }

    /// The new instance runs: the old one stays frozen and is disabled.
    fn moved(&mut self) -> Result<(), Stop> {
        let unit = self.opts.service.clone();
        // Again, for a run that goes on from a recorded start.
        let disabled = matches!(
            self.world.freezer().systemctl(&["disable", &unit]),
            Ok((0, _))
        );
        if disabled && self.old_frozen() {
            self.changed(
                "old instance",
                format!("{unit} stays frozen and is disabled: it never runs beside the new one"),
            );
        } else {
            let units = self.old_units();
            self.fail(
                "old instance",
                format!(
                    "{unit} must be frozen and disabled now that the new instance runs: \
                     `systemctl freeze {units}; systemctl disable {unit}`"
                ),
            );
        }
        self.state.cutover = Cutover::Moved;
        // The copy there is an instance now: nothing more is sent into it.
        self.stage.forget();
        self.save_state()
    }

    /// The new instance did not come up: it is stopped there before the old one thaws.
    fn roll_back(&mut self) -> Result<(), Stop> {
        let unit = self.opts.service.clone();
        let stopped = self.call(
            "new instance stopped",
            &Request::Stop {
                service: unit.clone(),
            },
        );
        if stopped.is_err() {
            // Frozen is safe; thawing beside a copy that may run is not.
            let stop = self.new_stop_command();
            self.manual(
                "old instance",
                format!(
                    "{unit} stays frozen: the new instance could not be stopped. Stop it on the \
                     new host (`{stop}`), then `yulius instance migrate --abort` here"
                ),
            );
            return Err(Stop::Failed);
        }
        self.enable_old();
        self.thaw();
        self.state.cutover = Cutover::RolledBack;
        self.save_state()?;
        self.manual(
            "new host",
            format!(
                "the copy in {} there was started and is no longer a clean copy: remove it \
                 there before running the move again",
                self.opts.data_dir.display()
            ),
        );
        Err(Stop::Failed)
    }

    /// A run that ended after it ordered the start: the new instance's state decides.
    fn resume_start(&mut self, p: &Profile) -> Result<(), Stop> {
        self.phase("Cutover");
        // The start was ordered by an earlier run: nothing here may thaw, and nothing is started
        // beside an old instance that is not frozen as a whole.
        let state = self.old_unit();
        if state.is_some_and(|old| old.all_frozen) {
            return self.start(p);
        }
        // Not frozen as a whole any more (this host was restarted, so its disabled unit does
        // not run; or someone thawed a unit of it): nothing is started on that. The new
        // instance is only asked.
        let unit = self.opts.service.clone();
        let not_frozen = if state.is_some() {
            "is not frozen as a whole"
        } else {
            "is not known to be frozen"
        };
        let asked = self.remote.call(&Request::Health {
            build: self.world.build(),
            health: self.health_address(p),
        });
        match asked {
            Ok(reply) if reply.started => {
                let mut old = state;
                if !old.is_some_and(Old::down) {
                    // Something of it runs beside the new instance, which has taken over; or
                    // a unit of it is on its way up or down, or systemd does not say, and "it
                    // does not run" would be a guess. Frozen here and now, to stay.
                    self.freeze_beside_new(old);
                    old = self.old_unit();
                }
                if old.is_some_and(Old::down) {
                    // The server's unit has stopped, which systemd does not freeze, and
                    // nothing of its sessions runs unfrozen (any more).
                    let units = self.old_units();
                    self.manual(
                        "old instance",
                        format!(
                            "{unit} does not run here any more, and the new instance runs. Make \
                             sure nothing of the old one runs here: `systemctl stop {units}; \
                             systemctl kill --kill-whom=all --signal=SIGKILL {units}; systemctl \
                             disable {unit}`"
                        ),
                    );
                    self.state.cutover = Cutover::Moved;
                    return self.save_state();
                }
                // Disabled and checked as after any start.
                self.moved()
            }
            Ok(_) => {
                self.fail(
                    "old instance",
                    format!(
                        "{unit} {not_frozen} although the new instance's start was ordered, \
                         and the new instance does not say that it runs. Nothing was started \
                         or stopped: `yulius instance migrate --abort` goes back"
                    ),
                );
                Err(Stop::Failed)
            }
            // Silence is not "does not run": the abort would bring the old instance back
            // beside a new one that may run.
            Err(why) => {
                self.fail(
                    "new instance",
                    format!(
                        "the new host does not answer ({why}), so nobody knows whether the new \
                         instance runs, and {unit} {not_frozen} here. Nothing was started or \
                         stopped: run the command again when the new host answers (it decides \
                         from the new instance's state); `yulius instance migrate --abort` only \
                         once the new instance is known to be stopped"
                    ),
                );
                Err(Stop::Failed)
            }
        }
    }

    /// Freezes what runs of an old instance beside a new one that has taken over (a recorded
    /// start, a unit of it thawed or started since), to stay: the start was ordered, so nothing
    /// here thaws, also not when a freeze fails ([`freeze::freeze_to_stay`], not the export's
    /// freeze, which undoes itself). Every unit that was frozen is named, and every one that
    /// could not be; the caller looks again and says whether the instance is held. `before` is
    /// what systemd said of the instance before.
    fn freeze_beside_new(&mut self, before: Option<Old>) {
        let unit = self.opts.service.clone();
        // Whether systemd had said that this unit of it ran unfrozen (a unit that is sent a
        // freeze was not frozen): the server's unit when it was active, its sessions unit when
        // it ran. A unit on its way up or down, or an unanswered question, is not that.
        let ran = |name: &String| {
            before.is_some_and(|old| {
                if *name == unit {
                    old.active
                } else {
                    old.sessions_run
                }
            })
        };
        let did = freeze::freeze_to_stay(&mut *self.world.freezer(), &unit);
        let are = |names: &[String]| {
            if names.len() == 1 {
                "it is"
            } else {
                "they are"
            }
        };
        let (known, unknown): (Vec<String>, Vec<String>) = did.frozen.into_iter().partition(ran);
        if !known.is_empty() {
            self.changed(
                "old instance",
                format!(
                    "{} ran here beside the new instance (thawed or started since the start \
                     was ordered): {} frozen again. What the old instance's agents did \
                     meanwhile is on this host only",
                    known.join(" and "),
                    are(&known)
                ),
            );
        }
        if !unknown.is_empty() {
            self.changed(
                "old instance",
                format!(
                    "{} may have run here beside the new instance (systemd did not say that \
                     {} frozen, or it was on its way up or down): {} frozen now",
                    unknown.join(" and "),
                    if unknown.len() == 1 {
                        "it was"
                    } else {
                        "they were"
                    },
                    are(&unknown)
                ),
            );
        }
        if !did.failed.is_empty() {
            let what: Vec<String> = did
                .failed
                .iter()
                .map(|(name, _)| format!("{name} {}", if ran(name) { "runs" } else { "may run" }))
                .collect();
            let why: Vec<&str> = did.failed.iter().map(|(_, why)| why.as_str()).collect();
            let units = self.old_units();
            // The freeze `moved` asks for may fail again: systemd freezes only a unit that
            // runs, not one on its way up or down or with a process stuck in the kernel.
            // Stopping it (so it is not restarted) and killing what is left, with SIGKILL as
            // the new host's own stop does, ends the old instance in every state.
            self.fail(
                "old instance",
                format!(
                    "{} beside the new instance and could not be frozen: {}. If a freeze keeps \
                     failing (systemd freezes only a unit that runs), stop it for good instead: \
                     `systemctl stop {units}; systemctl kill --kill-whom=all --signal=SIGKILL \
                     {units}`",
                    what.join(" and "),
                    why.join("; ")
                ),
            );
        }
    }

    /// What follows a start: the public URL, the tasks, the old URL.
    fn after(&mut self, p: &Profile) -> Result<(), Stop> {
        let Some(new) = self.state.new_url.clone() else {
            return Ok(());
        };
        let host = profile::host_of(&new).unwrap_or_default();
        // Asked first: over a key that is gone (an earlier run's finish removed it there and
        // deleted it here) nothing is tried, `ssh` could only complain about it.
        let key_gone = self
            .remote
            .reachable()
            .is_err_and(|why| why.contains(remote::KEY_REFUSED) || why.contains(remote::NO_KEY));
        let address = if key_gone {
            String::new()
        } else {
            self.remote
                .call(&Request::Facts {
                    data_dir: self.opts.data_dir.clone(),
                    service: self.opts.service.clone(),
                    ports: Vec::new(),
                })
                .ok()
                .and_then(|r| r.facts)
                .and_then(|f| f.address)
                .unwrap_or_default()
        };
        let leads = self.world.addresses(&host);
        let direct = !address.is_empty() && !leads.contains(&address);
        let url = format!("{new}/healthz");
        // Bounded by the clock and by the number of looks, whichever ends first.
        let deadline = std::time::Instant::now() + HTTPS_WAIT;
        let mut waited = Duration::ZERO;
        let answered = loop {
            match self.world.healthz(&url, direct.then_some(address.as_str())) {
                Ok(()) => break Ok(()),
                Err(why) if waited >= HTTPS_WAIT || std::time::Instant::now() >= deadline => {
                    break Err(why);
                }
                Err(_) => {
                    self.world.sleep(Duration::from_secs(5));
                    waited += Duration::from_secs(5);
                }
            }
        };
        match answered {
            Ok(()) if direct => self.manual(
                "HTTPS",
                format!(
                    "{new} answers from the new host, but {host} still leads to {}: change its \
                     DNS record to {address} now",
                    if leads.is_empty() {
                        "nowhere".to_owned()
                    } else {
                        leads.join(", ")
                    }
                ),
            ),
            Ok(()) => self.ok("HTTPS", format!("{new} answers from the new host")),
            Err(why) => self.manual(
                "HTTPS",
                format!(
                    "{new} does not answer yet ({why}); the instance itself runs: check the \
                     proxy's certificate and the DNS record"
                ),
            ),
        }
        // Skipped only when the key is gone: any other failure to reach the new host is said
        // by the step itself.
        if !key_gone {
            self.call(
                "tasks",
                &Request::Tasks {
                    data_dir: self.opts.data_dir.clone(),
                    wait_secs: TASKS_WAIT,
                },
            )?;
        }
        if !key_gone
            && p.move_helper == Some(true)
            && let Some(unit) = yulius_sandbox::units::migrate_path_unit(&self.opts.service)
        {
            // What fails here is a line: the new instance has taken over.
            let _ = self.call(
                "move helper",
                &Request::Enable {
                    name: "move helper".into(),
                    unit,
                },
            );
        }
        self.redirect(p, &new);
        Ok(())
    }

    /// The old URL redirects to the new one (MIG-041).
    fn redirect(&mut self, p: &Profile, new: &str) {
        const ITEM: &str = "old URL";
        let Some(old) = p.public_url.as_deref().filter(|old| *old != new) else {
            return;
        };
        let old_host = profile::host_of(old).unwrap_or_default();
        let Some(caddyfile) = p.caddyfile.as_ref().filter(|_| p.proxy == Carried::Yes) else {
            return self.manual(
                ITEM,
                format!("{old} is not redirected (no Caddy here): redirect it to {new} if people still use it"),
            );
        };
        if self.state.redirected || caddyfile.content.contains("X-Yulius-Moved-To") {
            return self.ok(ITEM, format!("{old} redirects to {new}"));
        }
        // The site keeps its name, its TLS settings and its neighbours; only what it proxied
        // to Yulius becomes the redirect.
        let port = p.ports.first().copied().unwrap_or(8080);
        let Some(site) = profile::redirected(&caddyfile.content, port, new) else {
            return self.manual(
                ITEM,
                format!(
                    "{old} is not redirected: {} has no `reverse_proxy` to port {port} to \
                     replace. In the site of {old_host}: `redir {new}{{uri}} 307`",
                    profile::CADDYFILE
                ),
            );
        };
        let backup = self.opts.state_dir.join(PROXY_BACKUP);
        if !backup.exists()
            && let Err(e) = write_atomic(&backup, caddyfile.content.as_bytes(), 0o600)
        {
            self.fail(ITEM, format!("cannot keep the proxy's configuration: {e}"));
            return;
        }
        let path = Path::new(profile::CADDYFILE);
        let reloaded = profile::write_keeping_owner(self.local, path, &site, caddyfile.mode)
            .and_then(|()| reload_caddy(self.local));
        match reloaded {
            Ok(()) => {
                self.state.redirected = true;
                let _ = self.save_state();
                self.changed(
                    ITEM,
                    format!("{old} answers with a temporary redirect to {new} (abort undoes it)"),
                );
            }
            Err(why) => {
                let _ = profile::write_keeping_owner(
                    self.local,
                    path,
                    &caddyfile.content,
                    caddyfile.mode,
                );
                let _ = reload_caddy(self.local);
                self.fail(
                    ITEM,
                    format!(
                        "the old proxy did not take the redirect ({why}); its configuration is back"
                    ),
                );
            }
        }
    }

    fn finish(&mut self, p: &Profile) {
        self.phase("Finish");
        let new = self.state.new_url.clone().unwrap_or_default();
        let moved_url = p.public_url.as_deref().is_some_and(|old| old != new);
        // Asked before any step: over a key that does not sign in nothing is tried.
        let key = self.remote.reachable().err();
        // Deleted here, which only a finish does, once the new host's finish answered: that
        // run's record did not survive. Nothing is sent over a key that is gone, and nothing
        // is waited for.
        let deleted_before = key
            .as_deref()
            .is_some_and(|why| why.contains(remote::NO_KEY));
        // Asked while the key still signs in: the new instance knows its App, and whether the
        // callback was registered meanwhile (SPEC-466 MVC-022).
        let github = (moved_url && key.is_none())
            .then(|| {
                self.remote.call(&Request::Callback {
                    data_dir: self.opts.data_dir.clone(),
                    address: self.health_address(p),
                    public_url: new.clone(),
                })
            })
            .and_then(Result::ok)
            .map(|reply| reply.lines)
            .filter(|lines| !lines.is_empty());
        // What this host says after the new host's Finish is known now, so the whole checklist
        // can travel with that step, while the key still signs in (SPEC-470 MVB-011).
        let mut then = self.after_finish(&new, moved_url, github);
        let finish = Request::Finish {
            key: self.state.key_comment(),
            firewall: p.firewall == Carried::Yes,
            arrived: true,
        };
        if deleted_before {
            self.manual(
                "one-time key",
                format!(
                    "is gone from this host: an earlier run deleted it once the finish on the \
                     new host was done, and only its record was not kept. Look at the new \
                     host's firewall once (`ufw status`); if the key was removed here by hand \
                     instead, also remove its line ({}) from root's authorized_keys there, and \
                     {}",
                    self.state.key_comment(),
                    target::STATE_DIR
                ),
            );
        } else {
            self.carry_checklist(p, &new, &then);
            match self.remote.call(&finish) {
                Ok(reply) => {
                    for line in reply.lines {
                        self.push(shown(line));
                    }
                }
                // The key no longer signs in and the new instance answers: an earlier run's
                // finish was done there and only its answer was lost.
                Err(_)
                    if self
                        .remote
                        .reachable()
                        .is_err_and(|why| why.contains(remote::KEY_REFUSED))
                        && self.state.new_url.clone().is_some_and(|url| {
                            self.world.healthz(&format!("{url}/healthz"), None).is_ok()
                        }) =>
                {
                    self.manual(
                        "one-time key",
                        "no longer signs in to the new host and the new instance answers: the \
                         finish there was done. Look at its firewall once (`ufw status`)",
                    );
                }
                Err(why) => {
                    // Not done there: the key stays on both hosts, and the same command tries
                    // again.
                    self.fail(
                        "one-time key",
                        format!(
                            "the new host did not answer ({why}): its key, the flow's files and \
                             the firewall there are still to do. Run the command again"
                        ),
                    );
                    return;
                }
            }
        }
        self.remote.forget();
        // The key's line first (an earlier run that deleted it has said so), the rest once the
        // move is recorded as finished.
        let rest = then.split_off(1);
        if !deleted_before {
            then.into_iter().for_each(|line| self.push(line));
        }
        // Without the key nothing more can be done from here: what failed above is a person's.
        self.state.finished = true;
        let _ = self.save_state();
        // The copy there is the instance: what this host knew of it while it was staged goes.
        self.stage.forget();
        rest.into_iter().for_each(|line| self.push(line));
    }

    /// What this host says once the new host's Finish is done: its key deleted, then what is
    /// left for people. Known before that step, so it can travel with it.
    fn after_finish(
        &self,
        new: &str,
        moved_url: bool,
        github: Option<Vec<HostLine>>,
    ) -> Vec<HostLine> {
        let line = |name: &str, outcome, detail: String| HostLine {
            name: name.to_owned(),
            outcome,
            detail,
        };
        let mut then = vec![line(
            "one-time key (old host)",
            HostOutcome::Changed,
            "deleted".into(),
        )];
        if moved_url {
            match github {
                Some(lines) => then.extend(lines.into_iter().map(shown)),
                None => then.push(line("GitHub App", HostOutcome::Manual, github_note(new))),
            }
            // The old URL's redirect leads `yulius remote` to the new one (SPEC-466 MVC-030).
            let cli = if self.state.redirected {
                "`yulius remote` follows the old URL's redirect by itself while the old host \
                 answers"
                    .to_owned()
            } else {
                format!("`yulius remote login {new}` keeps the token")
            };
            then.push(line(
                "remote clients",
                HostOutcome::Manual,
                format!(
                    "laptops point at the old URL: {cli}; in Claude Code `claude mcp remove \
                     yulius`, then `claude mcp add --transport http yulius {new}/mcp --header \
                     \"Authorization: Bearer <token>\"`"
                ),
            ));
        }
        then.push(line(
            "old host",
            HostOutcome::Manual,
            format!(
                "it keeps a frozen copy with master.key and every secret. `sudo yulius instance \
                 migrate --abort` here goes back, once the new instance is stopped on the new \
                 host. When the new instance has proven itself, wipe {} or retire the disk",
                self.opts.data_dir.display()
            ),
        ));
        then
    }

    /// Brings the new host the checklist so far and the lines that follow its Finish, which
    /// leaves the whole for the new instance's page (SPEC-470 MVB-011). A new host that does
    /// not take it is said by the Finish that follows.
    fn carry_checklist(&mut self, p: &Profile, new: &str, then: &[HostLine]) {
        let carried = target::Carried {
            checklist: yulius_contracts::migrate::MoveChecklist {
                version: yulius_contracts::migrate::VERSION,
                id: self.state.id.clone(),
                target: self.state.target.clone(),
                from: p.public_url.clone(),
                to: Some(new.to_owned()).filter(|url| !url.is_empty()),
                lines: self.lines.clone(),
                finished_at: now_secs(),
            },
            then: then.to_vec(),
        };
        if let Ok(reply) = self.remote.call(&Request::Carry(carried)) {
            for line in reply.lines {
                self.push(shown(line));
            }
        }
    }

    /// `--abort`: back to the old host, never with both instances running.
    pub fn abort(mut self) -> u8 {
        self.phase("Abort");
        let unit = self.opts.service.clone();
        let started = self.state.cutover != Cutover::No;
        let reachable = self.remote.reachable().is_ok();
        if started && reachable {
            if self
                .call(
                    "new instance stopped",
                    &Request::Stop {
                        service: unit.clone(),
                    },
                )
                .is_err()
            {
                return self.end_abort(false);
            }
            // Stopped there: whatever fails from here on, the move is no longer "moved".
            self.state.cutover = Cutover::RolledBack;
            let _ = self.save_state();
        } else if started {
            let url = self.state.new_url.clone().unwrap_or_default();
            if self.world.healthz(&format!("{url}/healthz"), None).is_ok() {
                let stop = self.new_stop_command();
                self.fail(
                    "new instance",
                    format!(
                        "it still answers at {url}: stop it on the new host (`{stop}`), then run \
                         --abort again. Nothing was changed here"
                    ),
                );
                return self.end_abort(false);
            }
            // Silence is not proof (a DNS record, a certificate, a network): the key is gone,
            // so a person says that the new instance is stopped.
            let stopped = self.opts.yes
                || self.world.confirm(&format!(
                    "{url} does not answer. If the new instance still runs on {}, both would \
                     run the same agents on the same repositories. Is it stopped and disabled \
                     there?",
                    self.state.target
                )) == Some(true);
            if !stopped {
                let stop = self.new_stop_command();
                self.fail(
                    "new instance",
                    format!(
                        "{url} does not answer, which does not prove that it is stopped: stop \
                         it on the new host (`{stop}`), then run `--abort --yes`. Nothing was \
                         changed here"
                    ),
                );
                return self.end_abort(false);
            }
            self.ok(
                "new instance",
                format!("{url} does not answer and is confirmed stopped"),
            );
        }
        let backup = self.opts.state_dir.join(PROXY_BACKUP);
        if let Ok(before) = std::fs::read_to_string(&backup) {
            let path = Path::new(profile::CADDYFILE);
            let mode = self
                .local
                .stat(path)
                .map_or(0o644, |(_, mode)| mode & 0o7777);
            let restored = profile::write_keeping_owner(self.local, path, &before, mode)
                .and_then(|()| reload_caddy(self.local));
            match restored {
                Ok(()) => self.changed("old URL", "the proxy's earlier configuration is back"),
                Err(why) => {
                    self.fail(
                        "old URL",
                        format!("cannot put the proxy's configuration back: {why}"),
                    );
                }
            }
        }
        // Frozen: thawed. Stopped (the old host was restarted since): started.
        let state = self.old_unit();
        let mut systemd = self.world.freezer();
        let did = |out: Result<(i32, String), String>| matches!(out, Ok((0, _)));
        let runs = match state {
            Some(old) if old.active => {
                (!old.any_frozen || freeze::thaw_instance(&mut *systemd, &unit).is_ok())
                    && did(systemd.systemctl(&["enable", &unit]))
            }
            // The server's unit does not run; its sessions unit may still be frozen.
            Some(old) => {
                (!old.any_frozen || freeze::thaw_instance(&mut *systemd, &unit).is_ok())
                    && did(systemd.systemctl(&["enable", "--now", &unit]))
            }
            None => false,
        };
        if runs {
            self.changed(
                "old instance",
                format!("{unit} runs here again and is enabled"),
            );
        } else {
            let thaw = self
                .thaw_command()
                .map_or_else(String::new, |thaw| format!("{thaw}; "));
            self.fail(
                "old instance",
                format!("{unit} could not be brought back: `{thaw}systemctl enable --now {unit}`"),
            );
        }
        if failed(&self.lines) {
            // Not back yet: the key stays, so the same abort can go on through it.
            return self.end_abort(false);
        }
        let copied = self.state.copied || self.stage.id().is_some();
        if reachable {
            // The step that removes the key there is run by this build. A plan that ended with
            // the key to add never brought it (a person added the key and aborts): it goes
            // there for this step, which removes it again with the flow's other files.
            let there = Request::Facts {
                data_dir: self.opts.data_dir.clone(),
                service: unit.clone(),
                ports: Vec::new(),
            };
            if self.remote.try_call(&there).is_none() {
                let exe = self.world.exe();
                // A copy that fails is said by the step that then cannot run.
                let _ = self.remote.put_build(&exe);
            }
            if copied {
                // A stream the new host is still taking in is closed before its files go.
                let _ = self.remote.call(&Request::Stage {
                    data_dir: self.opts.data_dir.clone(),
                });
            }
            let _ = self.call(
                "one-time key",
                &Request::Finish {
                    key: self.state.key_comment(),
                    firewall: false,
                    // Going back: no checklist of a finished move stays on the new host.
                    arrived: false,
                },
            );
        }
        if started || copied {
            self.manual(
                "new host",
                format!(
                    "the copy in {} there still holds master.key and every secret: remove it \
                     (and {} with it)",
                    self.opts.data_dir.display(),
                    target::STATE_DIR
                ),
            );
        }
        self.remote.forget();
        let done = !failed(&self.lines);
        self.end_abort(done)
    }

    fn end_abort(&mut self, done: bool) -> u8 {
        if done {
            for name in ["state.json", PROXY_BACKUP] {
                let _ = std::fs::remove_file(self.opts.state_dir.join(name));
            }
            self.stage.forget();
            self.phase = "aborted";
        }
        let summary = host_setup::summary(&self.lines, Mode::Setup);
        self.world.show(&format!("\n{summary}"));
        self.save_status();
        u8::from(!done)
    }
}

/// The connection of a run to the recorded host with the move's key. A run that is nobody's
/// sign-in (`--no-sign-in`, the move helper's) gets one that offers that key alone and reads
/// nothing of root's.
fn connection(opts: &Options, state: &State) -> remote::Ssh {
    let ssh = remote::Ssh::new(
        &state.target,
        state.port,
        &opts.state_dir,
        &state.key_comment(),
    );
    if opts.no_sign_in {
        ssh.without_sign_in()
    } else {
        ssh
    }
}

/// What a person registers in the GitHub App for a new URL (SPEC-460 MIG-042): GitHub has no
/// API for it, and Yulius never uses a person's GitHub session.
#[must_use]
pub fn github_note(new: &str) -> String {
    format!(
        "GitHub sign-in needs the new URL in the App: open its settings (the owner's Settings → \
         Developer settings → GitHub Apps → the App; for an organization \
         https://github.com/organizations/<owner>/settings/apps/<app>) and add the callback URL \
         {new}/api/auth/github/callback and the homepage {new}. Keep the old callback until the \
         old host is retired; password sign-in works meanwhile"
    )
}

fn reload_caddy(host: &mut dyn Host) -> Result<(), String> {
    for argv in [
        &[
            "caddy",
            "validate",
            "--config",
            profile::CADDYFILE,
            "--adapter",
            "caddyfile",
        ][..],
        &["systemctl", "reload", "caddy"][..],
    ] {
        let out = host.run(&Cmd::new(argv))?;
        if !out.ok() {
            return Err(format!("`{}` failed: {}", argv.join(" "), out.why()));
        }
    }
    Ok(())
}

/// The real machine and the person at its terminal.
struct RealWorld {
    /// What a signal does to the freeze ([`freeze::on_signal`]).
    signal: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// The units a signal thaws ([`World::frozen`]).
    units: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    watched: bool,
}

impl World for RealWorld {
    fn freezer(&mut self) -> Box<dyn freeze::Host> {
        Box::new(freeze::System)
    }

    fn arm(&mut self, _unit: &str) -> Result<(), String> {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        use std::io::Write as _;
        use std::sync::atomic::Ordering;
        self.signal.store(freeze::ARMING, Ordering::SeqCst);
        if self.watched {
            return Ok(());
        }
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|e| format!("cannot watch signals, so nothing is frozen: {e}"))?;
        self.watched = true;
        let (state, units) = (
            std::sync::Arc::clone(&self.signal),
            std::sync::Arc::clone(&self.units),
        );
        // On its own thread, so a thaw happens even while the copy waits on a stalled link.
        std::thread::spawn(move || {
            for signal in signals.forever() {
                let mut err = std::io::stderr();
                match freeze::on_signal(&state, &mut freeze::System, &units) {
                    freeze::Signal::LeaveToMain => {
                        let _ = writeln!(
                            err,
                            "\nyulius instance migrate: signal {signal} ignored: the move \
                             has taken its end (the new instance is being started, or the \
                             old one thawed) and ends by itself"
                        );
                    }
                    freeze::Signal::Exit(why) => {
                        let _ = writeln!(
                            err,
                            "\nyulius instance migrate: stopped by signal {signal}; the old \
                             instance runs on, run the command again to continue"
                        );
                        if let Some(why) = why {
                            let _ = writeln!(err, "yulius instance migrate: {why}");
                        }
                        std::process::exit(1);
                    }
                }
            }
        });
        Ok(())
    }

    fn refused(&mut self) {
        self.signal
            .store(freeze::REFUSED, std::sync::atomic::Ordering::SeqCst);
    }

    fn frozen(&mut self, units: Vec<String>) {
        let state = if units.is_empty() {
            freeze::LEAVE
        } else {
            freeze::THAW
        };
        // Before the state leaves ARMING: a signal reads the units once it sees THAW.
        if let Ok(mut held) = self.units.lock() {
            *held = units;
        }
        self.signal
            .store(state, std::sync::atomic::Ordering::SeqCst);
    }

    fn claim(&mut self) -> bool {
        freeze::claim_end(&self.signal)
    }

    fn confirm(&mut self, question: &str) -> Option<bool> {
        use std::io::{BufRead as _, IsTerminal as _, Write as _};
        if !std::io::stdin().is_terminal() {
            return None;
        }
        print!("\n{question} [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).ok()?;
        Some(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }

    fn addresses(&mut self, host: &str) -> Vec<String> {
        use std::net::ToSocketAddrs as _;
        let mut found: Vec<String> = (host, 443)
            .to_socket_addrs()
            .map(|all| all.map(|a| a.ip().to_string()).collect())
            .unwrap_or_default();
        found.sort();
        found.dedup();
        found
    }

    fn healthz(&mut self, url: &str, address: Option<&str>) -> Result<(), String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(address) = address {
            let ip: std::net::IpAddr = address.parse().map_err(|_| format!("{address:?}"))?;
            let host = profile::host_of(url).unwrap_or_default();
            client = client.resolve(&host, std::net::SocketAddr::new(ip, 443));
        }
        let client = client.build().map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime.block_on(async {
            let answer = client.get(url).send().await.map_err(|e| e.to_string())?;
            let status = answer.status();
            let body = answer.text().await.map_err(|e| e.to_string())?;
            if status.is_success() && body.trim() == "ok" {
                Ok(())
            } else {
                Err(format!("it answers {status}"))
            }
        })
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }

    fn now(&mut self) -> (i64, i64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        (
            i64::try_from(now.as_secs()).unwrap_or(i64::MAX),
            i64::from(now.subsec_nanos()),
        )
    }

    fn sync(&mut self, dir: &Path) -> Result<(), String> {
        super::export::sync(dir).map_err(|e| e.to_string())
    }

    fn era(&mut self) -> super::receipts::Era {
        super::receipts::Era::here()
    }

    fn walk_rate(&mut self, dir: &Path) -> Option<f64> {
        warm::sampled(dir, SAMPLE).map(|(_, per_sec)| per_sec)
    }

    fn running(&mut self, address: &str) -> Option<String> {
        target::reported_commit(address)
    }

    fn exe(&self) -> PathBuf {
        std::env::current_exe().unwrap_or_default()
    }

    fn build(&self) -> Option<String> {
        crate::serve::build_commit()
    }

    fn github(&mut self, data_dir: &Path, address: &str, public_url: &str) -> github::Asked {
        github::asked_instance(data_dir, address, Some(public_url))
    }

    fn show(&mut self, text: &str) {
        use std::io::Write as _;
        // A terminal that went away must not end the move (a panic would): the status file
        // keeps every line.
        let _ = writeln!(std::io::stdout(), "{text}");
    }
}

/// `--status`: the recorded move's phase and checklist.
fn status(dir: &Path) -> ExitCode {
    let status = std::fs::read_to_string(dir.join("status.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Status>(&text).ok());
    let Some(status) = status else {
        println!("No move is recorded in {}.", dir.display());
        return ExitCode::SUCCESS;
    };
    println!("Move {} to {}: {}", status.id, status.target, status.phase);
    for line in &status.lines {
        println!("{}", host_setup::format_line(line));
    }
    println!("{}", host_setup::summary(&status.lines, Mode::Setup));
    ExitCode::SUCCESS
}

/// One run or abort at a time in a state directory: a second one would thaw or remove what
/// the first relies on. The lock is held while the returned file lives.
///
/// # Errors
/// Another run holds it, or the file cannot be made.
pub fn lock(dir: &Path) -> Result<std::fs::File, String> {
    let path = dir.join("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| "another `yulius instance migrate` runs here".to_owned())?;
    Ok(file)
}

/// `yulius instance migrate`.
#[must_use]
pub fn main(opts: &Options, action: Action) -> ExitCode {
    use std::os::unix::fs::PermissionsExt as _;
    let refused = |why: &str| {
        eprintln!("yulius instance migrate: {why}");
        ExitCode::from(REFUSED)
    };
    if action == Action::Status {
        return status(&opts.state_dir);
    }
    // What cannot be a run is said before anything else, whoever asks.
    for unit in [&opts.service, &opts.update_service] {
        if !host_setup::service_name_ok(unit) {
            return refused(&format!("{unit:?} is not a unit name"));
        }
    }
    match (action, &opts.target) {
        (Action::Run, None) => {
            return refused("name the new host: `yulius instance migrate root@new-host`");
        }
        (Action::Run, Some(target)) if !remote::destination_ok(target) => {
            return refused(&format!("{target:?} is not `[user@]host`"));
        }
        (Action::Run, Some(target))
            if target
                .split_once('@')
                .is_some_and(|(user, _)| user != "root") =>
        {
            return refused("the new host is reached as root: `root@new-host`");
        }
        _ => {}
    }
    if !super::is_root() {
        return refused(
            "run it as root (sudo): it freezes the service, reads every sandbox's files and \
             sets up the new host",
        );
    }
    // Its files keep their own modes (the key and the state 0600); the status is for readers
    // such as the server, so the directory can be passed through.
    let made = std::fs::create_dir_all(&opts.state_dir).and_then(|()| {
        std::fs::set_permissions(&opts.state_dir, std::fs::Permissions::from_mode(0o711))
    });
    if let Err(e) = made {
        return refused(&format!("{}: {e}", opts.state_dir.display()));
    }
    let _lock = match lock(&opts.state_dir) {
        Ok(file) => file,
        Err(why) => return refused(&why),
    };
    let mut state = match (action, &opts.target) {
        (Action::Abort, _) => match State::load(&opts.state_dir) {
            Some(state) => state,
            None => return refused("no move is recorded here; nothing to abort"),
        },
        (_, Some(target)) => match State::begin(&opts.state_dir, target, opts.port) {
            Ok(state) => state,
            Err(why) => return refused(&why),
        },
        (_, None) => return refused("name the new host"),
    };
    let mut opts = opts.clone();
    if let Err(why) = state.for_instance(&mut opts, action == Action::Abort) {
        return refused(&why);
    }
    if let Err(why) = state.save(&opts.state_dir) {
        return refused(&why);
    }
    let opts = &opts;
    // The copy walks the data directory while the instance runs, with every directory it is in
    // held open.
    target::raise_open_files();
    let mut ssh = connection(opts, &state);
    let mut host = host_setup::system::RealHost { quiet: false };
    let mut world = RealWorld {
        signal: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(freeze::LEAVE)),
        units: std::sync::Arc::default(),
        watched: false,
    };
    let the_move = Move::new(opts, state, &mut host, &mut ssh, &mut world);
    ExitCode::from(match action {
        Action::Abort => the_move.abort(),
        _ => the_move.run(),
    })
}

#[cfg(test)]
mod tests;
