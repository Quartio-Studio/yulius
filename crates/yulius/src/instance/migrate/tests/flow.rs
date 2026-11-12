//! Whole moves between two fake hosts, with the real export, extraction and inventory between
//! two temporary directories (SPEC-460 acceptance criteria).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yulius_contracts::update::{HostLine, HostOutcome};

use super::fake::{FakeHost, NEW_ADDRESS, OLD_HOST, STAGED_BUILD, new_host, old_host};
use super::github::facts;
use crate::host_setup::{Host as _, Out};
use crate::instance::Manifest;
use crate::instance::archive::{Item, Reader};
use crate::instance::extract::IdMap;
use crate::instance::freeze;
use crate::instance::import::{self, Failure, Ground, INCOMPLETE, Mark};
use crate::instance::inventory::of_copy;
use crate::instance::migrate::github::{Asked, line as line_of};
use crate::instance::migrate::remote::Remote;
use crate::instance::migrate::target::{self, Env, Part, Reply, Request};
use crate::instance::migrate::{Cutover, Move, Options, State, Status, World, profile, warm};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NEW_HOST: &str = "185-189-47-160.sslip.io";
const NEW_URL: &str = "https://185-189-47-160.sslip.io";

/// What happened on both hosts, in order.
type Events = Arc<Mutex<Vec<String>>>;

fn note(events: &Events, what: impl Into<String>) {
    if let Ok(mut events) = events.lock() {
        events.push(what.into());
    }
}

fn build() -> String {
    crate::serve::build_commit().unwrap_or_else(|| "0".repeat(40))
}

/// What happens to the old unit from outside.
type Outside = fn(&mut Systemd);

/// systemd on the old host, as the freeze sees it.
#[derive(Debug, Default)]
struct Systemd {
    frozen: bool,
    disabled: bool,
    /// Not running at all (the old host was restarted after the move).
    stopped: bool,
    /// Not running, and systemd says `failed` (its start limit was hit).
    failed: bool,
    /// `systemctl disable` fails.
    no_disable: bool,
    /// `systemctl enable` fails.
    no_enable: bool,
    /// systemd does not answer `show`.
    silent: bool,
    /// Looks (`show`) answered since the last `freeze`; `None` before one.
    looks: Option<u32>,
    /// What happens to the instance from outside once that many looks were answered since
    /// the last freeze (the freeze's own look, which finds its unit frozen, is the first).
    after_freeze: Option<(u32, Outside)>,
    /// `systemctl freeze` fails.
    no_freeze: bool,
    /// A freeze never completes (a process hangs in the kernel): the unit stays `freezing`.
    never_frozen: bool,
    /// On its way up or down (`activating`: a restart after a crash, a start in progress).
    /// systemd freezes no unit in that state.
    starting: bool,
    /// The unit has settled (it is `active`) by the time a freeze is asked for.
    settles: bool,
    /// The freezer is on its way.
    freezing: bool,
    /// The host's sessions unit (SPEC-458) and whether it is frozen; `None`: it has none.
    sessions: Option<bool>,
    /// systemd does not answer `show` for the sessions unit.
    sessions_silent: bool,
}

struct FakeSystemd(Arc<Mutex<Systemd>>, Events);

impl freeze::Host for FakeSystemd {
    fn systemctl(&mut self, args: &[&str]) -> Result<(i32, String), String> {
        let mut unit = self.0.lock().map_err(|e| e.to_string())?;
        let said = |text: String| Ok((0, text));
        let of_sessions = args
            .last()
            .is_some_and(|name| name.ends_with("-sandboxes.service"));
        if args[0] == "show"
            && let Some((after, change)) = unit.after_freeze
            && unit.looks.is_some_and(|looks| looks >= after)
        {
            unit.after_freeze = None;
            change(&mut unit);
        }
        match args[0] {
            "show" if unit.silent => Err("Failed to connect to bus".into()),
            "show" if of_sessions && unit.sessions_silent => Err("Failed to connect to bus".into()),
            "show" => {
                if let Some(looks) = unit.looks.as_mut() {
                    *looks += 1;
                }
                if of_sessions {
                    return said(match unit.sessions {
                        None => "LoadState=not-found\nActiveState=inactive\nFreezerState=running\n"
                            .into(),
                        Some(frozen) => format!(
                            "LoadState=loaded\nActiveState=active\nFreezerState={}\n\
                             ControlGroup=/yulius.slice/yulius-sandboxes.service\n",
                            if frozen { "frozen" } else { "running" }
                        ),
                    });
                }
                said(format!(
                    "LoadState=loaded\nActiveState={}\nFreezerState={}\n\
                     ControlGroup=/system.slice/yulius.service\n",
                    if unit.failed {
                        "failed"
                    } else if unit.stopped {
                        "inactive"
                    } else if unit.starting {
                        "activating"
                    } else {
                        "active"
                    },
                    if unit.frozen {
                        "frozen"
                    } else if unit.freezing {
                        "freezing"
                    } else {
                        "running"
                    }
                ))
            }
            verb @ ("freeze" | "thaw") if of_sessions => {
                if unit.sessions.is_none() {
                    return Ok((5, String::new()));
                }
                if verb == "freeze" {
                    if unit.no_freeze {
                        return Ok((1, String::new()));
                    }
                    unit.looks = Some(0);
                }
                unit.sessions = Some(verb == "freeze");
                note(&self.1, format!("old: systemctl {verb} sessions"));
                said(String::new())
            }
            "is-active" => Ok((3, "inactive\n".into())),
            verb => {
                if verb == "freeze" && unit.settles {
                    unit.starting = false;
                }
                match verb {
                    // systemd refuses to freeze or thaw a unit that does not run, and to
                    // freeze one that is on its way up or down.
                    "freeze" if unit.no_freeze || unit.stopped || unit.failed || unit.starting => {
                        return Ok((1, String::new()));
                    }
                    "freeze" => {
                        unit.frozen = !unit.never_frozen;
                        unit.freezing = unit.never_frozen;
                        unit.looks = Some(0);
                    }
                    "thaw" if unit.stopped || unit.failed || unit.starting => {
                        return Ok((1, String::new()));
                    }
                    "thaw" => {
                        unit.frozen = false;
                        unit.freezing = false;
                    }
                    "disable" if unit.no_disable => return Ok((1, String::new())),
                    "disable" => unit.disabled = true,
                    "enable" if unit.no_enable => return Ok((1, String::new())),
                    "enable" => {
                        unit.disabled = false;
                        if args.contains(&"--now") {
                            unit.stopped = false;
                        }
                    }
                    _ => {}
                }
                note(&self.1, format!("old: systemctl {verb}"));
                said(String::new())
            }
        }
    }

    fn read(&self, path: &Path) -> Option<String> {
        (path == Path::new("/proc/self/cgroup"))
            .then(|| "0::/user.slice/user-0.slice/session-7.scope\n".to_owned())
    }

    fn sleep(&mut self, _: Duration) {}
}

/// A step of the new host, and what happens to the old unit from outside while it runs.
type Meanwhile = (&'static str, fn(&mut Systemd), Arc<Mutex<Systemd>>);

/// The new host: its fake system, and a real directory the copy lands in.
struct FakeRemote {
    host: FakeHost,
    copy_dir: PathBuf,
    events: Events,
    key: bool,
    /// The one-time key's comment (the move's id), as the real connection is given it.
    comment: String,
    signs_in: bool,
    /// How often the operator's own sign-in was tried ([`Remote::install_key`]).
    sign_ins: u32,
    /// The new host cannot be reached at all.
    down: bool,
    /// How often the key was tried ([`Remote::reachable`]).
    asked: u32,
    /// Steps tried although the key's file is gone: `ssh` can only complain about it.
    keyless: u32,
    /// The key's file was deleted on the old host ([`Remote::forget`]).
    deleted: bool,
    healthy: bool,
    /// Changes the installed copy before it is counted.
    tamper: Option<fn(&Path)>,
    /// `host setup` cannot build the sandbox base there.
    no_base: bool,
    /// The new host says the copy is proven whatever it counted.
    lies: bool,
    /// What happens to the old unit from outside while this step runs.
    meanwhile: Option<Meanwhile>,
    /// The new host's key as first seen.
    host_key: Option<&'static str>,
    /// Steps the new host is not reached for, once each: nothing is done there.
    refuse: Vec<&'static str>,
    /// Steps whose answer is lost, once each: done there, unheard here.
    lose: Vec<&'static str>,
    sent: Vec<PathBuf>,
    /// The data directory, as both hosts name it.
    data: PathBuf,
    /// What happens to the old host's data directory after each stream, in order.
    churn: Vec<fn(&Path)>,
    /// The next stream is cut to this many bytes on its way.
    cut: Option<usize>,
    /// What each stream held: whether it was read live, and its entries' paths.
    streams: Vec<(bool, Vec<String>)>,
    /// The link is slow: every write of 64 KiB or more takes this long.
    slow: Option<Duration>,
    /// What the new instance says of its GitHub App.
    github: Asked,
}

/// The fake link: what is written arrives, a large write after a while when it is slow.
struct Link<'a> {
    out: &'a mut Vec<u8>,
    slow: Option<Duration>,
}

impl Write for Link<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(slow) = self.slow.filter(|_| bytes.len() >= 64 << 10) {
            // Not a wait for anything: the time is what the test is about.
            std::thread::sleep(slow);
        }
        self.out.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What `ssh` says of a host that cannot be reached, as the real connection reports it.
const DOWN: &str = "ssh ended with exit status: 255: ssh: connect to host 185.189.47.160 port \
                    22: Connection timed out";

fn step_name(request: &Request) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|v| v.get("step")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl Remote for FakeRemote {
    fn reachable(&mut self) -> Result<(), String> {
        self.asked += 1;
        if self.deleted {
            // As the real connection: without its key it does not even try, whatever the
            // new host does.
            Err("there is no one-time key yet".into())
        } else if self.down {
            Err(DOWN.into())
        } else if self.key {
            Ok(())
        } else {
            Err("Permission denied (publickey)".into())
        }
    }

    fn key(&mut self) -> Result<String, String> {
        self.deleted = false;
        Ok(format!("ssh-ed25519 AAAAC3FAKE {}", self.comment))
    }

    fn install_key(&mut self) -> Result<(), String> {
        self.sign_ins += 1;
        if self.down {
            return Err("the sign-in to root@185.189.47.160 ended with exit status: 255".into());
        }
        if !self.signs_in {
            return Err("no terminal to sign in from".into());
        }
        self.key = true;
        let keys = format!(
            "ssh-ed25519 AAAA admin@laptop\nssh-ed25519 AAAAC3FAKE {}\n",
            self.comment
        );
        self.host
            .file_as("/root/.ssh/authorized_keys", &keys, 0, 0o600);
        Ok(())
    }

    fn put_build(&mut self, _: &Path) -> Result<(), String> {
        note(&self.events, "new: build copied");
        self.host.file_as(STAGED_BUILD, "build", 0, 0o700);
        Ok(())
    }

    fn call(&mut self, request: &Request) -> Result<Reply, String> {
        if self.deleted {
            self.keyless += 1;
        }
        if self.down {
            return Err(DOWN.into());
        }
        if !self.key {
            return Err("Permission denied (publickey)".into());
        }
        if !self.host.is_staged() {
            return Err("yulius: No such file or directory".into());
        }
        let step = step_name(request);
        if let Some(at) = self.refuse.iter().position(|s| *s == step) {
            self.refuse.remove(at);
            return Err("ssh: connect to host 185.189.47.160 port 22: Connection timed out".into());
        }
        note(&self.events, format!("new: {step}"));
        if let Some((at, change, systemd)) = &self.meanwhile
            && *at == step
            && let Ok(mut unit) = systemd.lock()
        {
            change(&mut unit);
        }
        let lost = self.lose.iter().position(|s| *s == step);
        if let Some(at) = lost {
            self.lose.remove(at);
        }
        // What the new instance says of its App: its loopback listener is not this machine's.
        if let Request::Callback { public_url, .. } = request {
            let (outcome, detail) = line_of(self.github.clone(), public_url);
            return Ok(Reply {
                lines: vec![HostLine {
                    name: "GitHub App".into(),
                    outcome,
                    detail,
                }],
                ..Reply::default()
            });
        }
        let Self {
            host,
            copy_dir,
            healthy,
            no_base,
            ..
        } = self;
        let mut env = Env {
            address: Some(NEW_ADDRESS.into()),
            ssh_port: Some(22),
            peer: Some("109.199.123.240".into()),
            hold: &mut |_| Box::new(()),
            health: &mut |_| healthy.then(build),
            sleep: &mut |_| {},
            count: &mut |manifest, old| {
                of_copy(copy_dir, &same_ids(manifest), old).map_err(|e| e.to_string())
            },
            setup: &mut |_, part, _| {
                vec![HostLine {
                    name: match part {
                        Part::Host => "host packages",
                        Part::BeforeStart => "sandbox base",
                        Part::AfterStart => "sandbox Docker",
                    }
                    .to_owned(),
                    outcome: if *no_base && part == Part::BeforeStart {
                        HostOutcome::Failed
                    } else {
                        HostOutcome::Ok
                    },
                    detail: "as host setup leaves it".into(),
                }]
            },
        };
        let mut reply = target::handle(host, &mut env, request);
        if self.lies && matches!(request, Request::Inventory { .. }) {
            for line in &mut reply.lines {
                line.outcome = HostOutcome::Ok;
                line.detail = "trust me\u{1b}[2J".into();
            }
        }
        // What a started server does with the mark (SPEC-460 MIG-050).
        if let Request::Start { setup, .. } = request
            && reply.started
        {
            let mark = setup.data_dir.join(target::MOVE_PENDING);
            host.files.remove(&mark);
            let report = setup.data_dir.join(target::MOVED).display().to_string();
            host.file(&report, r#"{"queued":2,"left":1,"failed":0}"#);
        }
        // What the step removed on the new host's disk is gone from the copy's directory.
        let marker = self.data.join(INCOMPLETE);
        if matches!(request, Request::DataDir { .. })
            && reply
                .lines
                .iter()
                .any(|l| l.detail.starts_with("removed an unfinished copy"))
        {
            let _ = fs::remove_dir_all(&self.copy_dir);
        } else if !self.host.is_file(&marker) {
            let _ = fs::remove_file(self.copy_dir.join(INCOMPLETE));
        }
        if lost.is_some() {
            return Err("ssh: the connection was reset".into());
        }
        Ok(reply)
    }

    fn send_paths(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        self.sent.extend(paths.iter().cloned());
        Ok(())
    }

    fn send_copy(
        &mut self,
        _: bool,
        write: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> Result<Reply, String> {
        let mut bytes = Vec::new();
        let wrote = write(&mut Link {
            out: &mut bytes,
            slow: self.slow,
        });
        if let Some(cut) = self.cut.take() {
            bytes.truncate(cut);
        }
        let (live, paths) = listed(&bytes);
        note(
            &self.events,
            if live {
                "new: copy warm"
            } else {
                "new: copy frozen"
            },
        );
        self.streams.push((live, paths));
        // The new host takes in what arrived, also of a stream that broke on the old host.
        let landed = land(&bytes, &self.copy_dir);
        if let Some(tamper) = self.tamper {
            tamper(&self.copy_dir);
        }
        // What the import leaves on the new host's disk: the copy, marked as a staged one.
        let data = self.data.display().to_string();
        self.host.file(&format!("{data}/yulius.db"), "the copy");
        if let Ok(marker) = fs::read_to_string(self.copy_dir.join(INCOMPLETE)) {
            self.host.file(&format!("{data}/{INCOMPLETE}"), &marker);
        }
        if !self.churn.is_empty() {
            self.churn.remove(0)(&self.data);
        }
        wrote.map_err(|e| format!("the copy's stream failed: {e}"))?;
        let mut reply = Reply::default();
        let (outcome, detail) = match landed {
            Ok(landing) => {
                let detail = target::landed(&landing);
                reply.stage = Some(landing.mark);
                (HostOutcome::Changed, detail)
            }
            Err(Failure::Refused(why) | Failure::Failed(why)) => (HostOutcome::Failed, why),
        };
        reply.lines = vec![HostLine {
            name: "copy".into(),
            outcome,
            detail,
        }];
        Ok(reply)
    }

    fn measure(&mut self, _: u64) -> Result<Duration, String> {
        Ok(Duration::from_millis(500))
    }

    fn host_key(&mut self) -> Option<String> {
        self.host_key.map(str::to_owned)
    }

    fn forget(&mut self) {
        note(&self.events, "old: key deleted");
        self.key = false;
        self.deleted = true;
    }
}

impl FakeHost {
    fn is_staged(&self) -> bool {
        self.text(STAGED_BUILD).is_some()
    }
}

fn same_ids(m: &Manifest) -> IdMap {
    IdMap {
        uid: (m.service.uid, m.service.uid),
        gid: (m.service.gid, m.service.gid),
        subuid: None,
        subgid: None,
    }
}

/// Takes a pass into the staged copy at `dir` as the new host does, without its checks of the
/// host (the copy's directory is not where the stream was read).
fn land(bytes: &[u8], dir: &Path) -> Result<import::Landing, Failure> {
    let mut r = Reader::new(bytes);
    let m = import::manifest(&mut r).map_err(Failure::Refused)?;
    import::take_in(
        r,
        &m,
        dir,
        &m.service.user,
        same_ids(&m),
        &mut Ground {
            chown: false,
            sync: &mut |_| Ok(()),
            tick: &mut |_, _| {},
        },
    )
}

/// Whether a stream was read live, and the paths of its `data` entries.
fn listed(bytes: &[u8]) -> (bool, Vec<String>) {
    let mut r = Reader::new(bytes);
    let live = import::manifest(&mut r)
        .ok()
        .and_then(|m| m.pass)
        .is_some_and(|pass| pass.live);
    let mut paths = Vec::new();
    while let Ok(Item::Entry(e)) = r.next_item() {
        if e.path.starts_with(b"data") {
            paths.push(String::from_utf8_lossy(&e.path).into_owned());
        }
    }
    (live, paths)
}

struct FakeWorld {
    systemd: Arc<Mutex<Systemd>>,
    events: Events,
    answer: Option<bool>,
    dns: BTreeMap<String, Vec<String>>,
    https: bool,
    shown: Vec<String>,
    /// The terminal goes away when a line with this text is shown.
    lost_at: Option<&'static str>,
    /// This build names no commit.
    no_commit: bool,
    /// How far ahead of the real clock a pass begins: far enough, and every entry a test just
    /// wrote is old enough to earn its receipt.
    ahead: i64,
    /// The data directory's filesystem cannot be synced.
    no_sync: bool,
    /// Syncs asked for.
    syncs: u32,
    /// Entries a second that a sample walk of the data directory measured.
    walk_rate: Option<f64>,
    /// The old instance is updated to this build after the plan looked at it.
    updated_to: Option<&'static str>,
    /// Looks at the old instance's build.
    looks: u32,
    /// The boot and the clock the receipts are believed by.
    era: crate::instance::receipts::Era,
    /// The clock is an hour behind once this event has happened so many times (`sync`: the
    /// data directory's filesystem was synced).
    set_back_at: Option<(&'static str, usize)>,
    /// What the old instance says of its GitHub App for a new URL, and the URLs it was asked.
    github: Asked,
    asked: Vec<String>,
}

impl World for FakeWorld {
    fn freezer(&mut self) -> Box<dyn freeze::Host> {
        Box::new(FakeSystemd(
            Arc::clone(&self.systemd),
            Arc::clone(&self.events),
        ))
    }

    fn arm(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn refused(&mut self) {}

    fn frozen(&mut self, _: Vec<String>) {}

    fn claim(&mut self) -> bool {
        note(&self.events, "old: no signal thaws from here");
        true
    }

    fn confirm(&mut self, _: &str) -> Option<bool> {
        self.answer
    }

    fn addresses(&mut self, host: &str) -> Vec<String> {
        self.dns.get(host).cloned().unwrap_or_default()
    }

    fn healthz(&mut self, _: &str, _: Option<&str>) -> Result<(), String> {
        if self.https {
            Ok(())
        } else {
            Err("connection refused".into())
        }
    }

    fn sleep(&mut self, _: Duration) {}

    fn now(&mut self) -> (i64, i64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        (
            i64::try_from(now.as_secs()).unwrap_or(0) + self.ahead,
            i64::from(now.subsec_nanos()),
        )
    }

    fn sync(&mut self, _: &Path) -> Result<(), String> {
        self.syncs += 1;
        if self.no_sync {
            Err("Input/output error (os error 5)".into())
        } else {
            Ok(())
        }
    }

    fn era(&mut self) -> crate::instance::receipts::Era {
        let mut era = self.era.clone();
        if let Some((event, times)) = self.set_back_at {
            let seen = if event == "sync" {
                self.syncs as usize
            } else {
                self.events.lock().map_or(0, |events| {
                    events.iter().filter(|seen| *seen == event).count()
                })
            };
            if seen >= times {
                era.ahead = era.ahead.map(|ahead| ahead - 3_600_000);
            }
        }
        era
    }

    fn walk_rate(&mut self, _: &Path) -> Option<f64> {
        self.walk_rate
    }

    fn running(&mut self, _: &str) -> Option<String> {
        self.looks += 1;
        match self.updated_to {
            Some(newer) if self.looks > 1 => Some(newer.to_owned()),
            _ => Some(build()),
        }
    }

    fn exe(&self) -> PathBuf {
        PathBuf::from("/usr/local/bin/yulius")
    }

    fn build(&self) -> Option<String> {
        (!self.no_commit).then(build)
    }

    fn github(&mut self, _: &Path, address: &str, public_url: &str) -> Asked {
        assert_eq!(
            address, "127.0.0.1:8081",
            "the old instance's loopback listener"
        );
        self.asked.push(public_url.to_owned());
        self.github.clone()
    }

    fn show(&mut self, text: &str) {
        assert!(
            !self.lost_at.is_some_and(|at| text.contains(at)),
            "the terminal went away"
        );
        self.shown.push(text.to_owned());
    }
}

/// The new host for a whole move: what apt and coreutils answer there.
fn move_script(host: &mut FakeHost, argv: &[String]) -> Option<Out> {
    let said = |stdout: &str| {
        Some(Out {
            code: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    };
    let gnu = Path::new("/gnu-selected");
    match argv[0].as_str() {
        "apt-get" if argv.iter().any(|a| a == "coreutils-from-gnu") => {
            host.file("/gnu-selected", "");
            None
        }
        "apt-get" if argv[1] == "install" && host.is_file(Path::new("/apt-broken")) => Some(Out {
            code: 100,
            stdout: String::new(),
            stderr: "E: Unable to correct problems, you have held broken packages.".into(),
        }),
        "apt-get" if argv[1] == "install" => {
            host.file("/packages-installed", "");
            None
        }
        "dpkg-query" if host.is_file(Path::new("/packages-installed")) => said(
            "caddy installed\ndocker-ce installed\ngh installed\nlibicu78 installed\n\
             ufw installed\nxvfb installed\nzstd installed\n",
        ),
        "ls" if host.is_file(gnu) => said("ls (GNU coreutils) 9.7\n"),
        // Docker's and GitHub's packages are known once their sources are there.
        "apt-cache" if argv[1] == "policy" => {
            let mut names = vec!["caddy", "ufw", "xvfb", "zstd", "coreutils-from-gnu"];
            if host.is_file(Path::new("/etc/apt/sources.list.d/docker.list")) {
                names.extend(["docker-ce", "gh"]);
            }
            let mut policy = String::from("libicu74:\n  Installed: (none)\n  Candidate: (none)\n");
            for name in names.iter().filter(|n| argv.iter().any(|a| a == *n)) {
                policy.push_str(&format!("{name}:\n  Installed: (none)\n  Candidate: 1.0\n"));
            }
            said(&policy)
        }
        _ => None,
    }
}

fn target_host() -> FakeHost {
    let mut host = new_host();
    host.files.remove(Path::new(STAGED_BUILD));
    host.script = Some(move_script);
    host.says(
        &["apt-cache", "--names-only", "search"],
        "libicu78 - International Components for Unicode\n",
    );
    host
}

/// A move between two temporary directories.
struct Setup {
    _tmp: tempfile::TempDir,
    opts: Options,
    local: FakeHost,
    remote: FakeRemote,
    world: FakeWorld,
}

fn tree(root: &Path) -> io::Result<()> {
    fs::create_dir_all(root.join("sandboxes/sb/home"))?;
    fs::create_dir_all(root.join("sysroots/sb/upper"))?;
    fs::write(root.join("yulius.db"), vec![7u8; 50_000])?;
    fs::write(root.join("master.key"), b"key")?;
    fs::write(root.join("sandboxes/sb/home/notes"), b"hello\n")?;
    fs::hard_link(
        root.join("sandboxes/sb/home/notes"),
        root.join("sandboxes/sb/home/notes-again"),
    )?;
    std::os::unix::fs::symlink("notes", root.join("sandboxes/sb/home/link"))?;
    fs::write(root.join("sysroots/sb/upper/tool"), vec![1u8; 1500])
}

fn setup() -> Result<Setup, Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let data_dir = tmp.path().join("old/data");
    fs::create_dir_all(&data_dir)?;
    tree(&data_dir)?;
    let state_dir = tmp.path().join("state");
    fs::create_dir_all(&state_dir)?;
    let events: Events = Arc::default();
    let mut local = old_host();
    local.says(
        &["systemctl", "show", "--property=ControlGroup"],
        "/system.slice/yulius.service\n",
    );
    local.file(
        "/proc/self/cgroup",
        "0::/user.slice/user-0.slice/session-7.scope\n",
    );
    let systemd: Arc<Mutex<Systemd>> = Arc::default();
    Ok(Setup {
        opts: Options {
            target: Some("root@185.189.47.160".into()),
            port: 22,
            public_url: None,
            data_disks: Vec::new(),
            yes: true,
            data_dir: data_dir.clone(),
            service: "yulius.service".into(),
            update_service: "yulius-update.service".into(),
            state_dir,
            max_downtime: None,
            no_sign_in: false,
        },
        local,
        remote: FakeRemote {
            host: target_host(),
            copy_dir: tmp.path().join("new/data"),
            events: Arc::clone(&events),
            key: false,
            comment: String::new(),
            signs_in: true,
            sign_ins: 0,
            down: false,
            asked: 0,
            keyless: 0,
            deleted: false,
            healthy: true,
            tamper: None,
            no_base: false,
            lies: false,
            meanwhile: None,
            host_key: Some("SHA256:abcDEF123 (ED25519)"),
            refuse: Vec::new(),
            lose: Vec::new(),
            sent: Vec::new(),
            data: data_dir,
            churn: Vec::new(),
            cut: None,
            streams: Vec::new(),
            slow: None,
            github: Ok(Some(facts("missing", NEW_URL))),
        },
        world: FakeWorld {
            systemd,
            events,
            answer: None,
            dns: BTreeMap::from([(NEW_HOST.to_owned(), vec![NEW_ADDRESS.to_owned()])]),
            https: true,
            shown: Vec::new(),
            lost_at: None,
            no_commit: false,
            ahead: 3600,
            no_sync: false,
            syncs: 0,
            walk_rate: Some(100_000.0),
            updated_to: None,
            looks: 0,
            era: crate::instance::receipts::Era {
                boot: "boot".into(),
                ahead: Some(0),
            },
            set_back_at: None,
            github: Ok(Some(facts("missing", NEW_URL))),
            asked: Vec::new(),
        },
        _tmp: tmp,
    })
}

impl Setup {
    fn state(&self) -> Result<State, String> {
        State::begin(&self.opts.state_dir, "root@185.189.47.160", 22)
    }

    /// Runs the move; its exit code.
    fn run(&mut self) -> Result<u8, String> {
        let state = self.state()?;
        self.remote.comment = state.key_comment();
        Ok(Move::new(
            &self.opts,
            state,
            &mut self.local,
            &mut self.remote,
            &mut self.world,
        )
        .run())
    }

    fn abort(&mut self) -> Result<u8, String> {
        let state = State::load(&self.opts.state_dir).ok_or("no move is recorded")?;
        self.remote.comment = state.key_comment();
        Ok(Move::new(
            &self.opts,
            state,
            &mut self.local,
            &mut self.remote,
            &mut self.world,
        )
        .abort())
    }

    fn status(&self) -> Result<Status, Box<dyn std::error::Error>> {
        let text = fs::read_to_string(self.opts.state_dir.join("status.json"))?;
        Ok(serde_json::from_str(&text)?)
    }

    fn events(&self) -> Vec<String> {
        self.world
            .events
            .lock()
            .map(|e| e.clone())
            .unwrap_or_default()
    }

    fn old_unit(&self) -> (bool, bool) {
        self.world
            .systemd
            .lock()
            .map(|u| (u.frozen, u.disabled))
            .unwrap_or_default()
    }

    /// The checklist as shown, without headings.
    fn shown(&self) -> String {
        self.world.shown.join("\n")
    }
}

fn line<'a>(status: &'a Status, name: &str) -> Option<&'a HostLine> {
    status.lines.iter().rev().find(|l| l.name == name)
}

fn outcome(status: &Status, name: &str) -> Option<HostOutcome> {
    line(status, name).map(|l| l.outcome)
}

#[test]
fn a_move_runs_from_the_plan_to_the_finish() -> TestResult {
    let mut s = setup()?;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(status.phase, "done");
    assert!(
        !status
            .lines
            .iter()
            .any(|l| l.outcome == HostOutcome::Failed),
        "{}",
        s.shown()
    );

    // The old host: frozen and disabled, redirecting, without the key.
    assert_eq!(s.old_unit(), (true, true));
    // Only what the site proxied to Yulius became the redirect: it keeps its name.
    let old_proxy = s.local.text(profile::CADDYFILE).unwrap_or_default();
    assert!(
        old_proxy.starts_with(&format!("{OLD_HOST} {{\n")),
        "{old_proxy}"
    );
    assert!(
        old_proxy.contains(&format!("\tredir {NEW_URL}{{uri}} 307\n")),
        "{old_proxy}"
    );
    assert!(old_proxy.contains(&format!("\theader X-Yulius-Moved-To {NEW_URL}\n")));
    assert!(!old_proxy.contains("reverse_proxy") && !old_proxy.contains("header_up"));
    assert!(s.local.ran("systemctl reload caddy"));
    let kept = fs::read_to_string(s.opts.state_dir.join("Caddyfile.before"))?;
    assert!(kept.contains("reverse_proxy 127.0.0.1:8080"));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(
        (state.cutover, state.finished, state.redirected),
        (Cutover::Moved, true, true)
    );
    assert_eq!(state.new_url.as_deref(), Some(NEW_URL));

    // The new host: the build, the unit for the new URL, sources, packages, coreutils, proxy,
    // firewall, and no key.
    let new = &s.remote.host;
    assert!(new.ran("mv -f -- /usr/local/bin/yulius.migrate-new /usr/local/bin/yulius"));
    let unit = new
        .text("/etc/systemd/system/yulius.service")
        .unwrap_or_default();
    assert!(
        unit.contains(&format!("YULIUS_PUBLIC_URL={NEW_URL}\n")),
        "{unit}"
    );
    assert!(unit.contains("UMask=0077") && unit.contains("WorkingDirectory=/var/lib/yulius"));
    let limits = new.text("/etc/systemd/system/yulius.service.d/20-limits.conf");
    assert!(limits.is_some_and(|t| t.contains("YULIUS_CI_JOB_TIMEOUT_MIN=60")));
    let docker = new
        .text("/etc/apt/sources.list.d/docker.list")
        .unwrap_or_default();
    assert!(docker.ends_with("ubuntu resolute stable\n"), "{docker}");
    assert!(
        new.runs("apt-get install")
            .iter()
            .any(|l| l.ends_with("-- caddy docker-ce gh libicu78 ufw xvfb zstd"))
    );
    assert!(new.ran("apt-get install -y --allow-remove-essential coreutils-from-gnu"));
    let caddy = new.text(profile::CADDYFILE).unwrap_or_default();
    assert!(caddy.starts_with(&format!("{NEW_HOST} {{")), "{caddy}");
    assert!(new.ran("systemctl reload-or-restart caddy"));
    assert!(new.ran("systemctl start yulius.service"));
    assert!(new.ran("systemctl enable --now yulius-update.path"));
    assert!(new.ran("ufw --force enable"));
    assert_eq!(
        new.text("/root/.ssh/authorized_keys"),
        Some("ssh-ed25519 AAAA admin@laptop\n")
    );
    assert!(!new.is_staged() && !s.remote.key);

    // Carried as they are: apt keys first, software, then key files, Caddy, ufw, the helper.
    let sent: Vec<&str> = s.remote.sent.iter().filter_map(|p| p.to_str()).collect();
    assert_eq!(
        &sent[..3],
        [
            "/etc/apt/keyrings/docker.asc",
            "/usr/share/keyrings/githubcli-archive-keyring.gpg",
            "/usr/local"
        ]
    );
    for path in [
        "/root/.rustup",
        "/etc/yulius/jev.key",
        "/etc/caddy",
        "/etc/ufw/user.rules",
        "/etc/systemd/system/yulius-update.path",
    ] {
        assert!(sent.contains(&path), "{path} was not sent: {sent:?}");
    }

    // The copy is the old data directory, proven before the start.
    assert_eq!(fs::read(s.remote.copy_dir.join("master.key"))?, b"key");
    assert_eq!(
        fs::read_link(s.remote.copy_dir.join("sandboxes/sb/home/link"))?,
        Path::new("notes")
    );
    let proof = line(&status, "copy proven").ok_or("no proof")?;
    assert!(
        proof
            .detail
            .starts_with("13 entries and 50.3 KiB in 5 folders"),
        "{}",
        proof.detail
    );

    // In this order: the old instance is frozen before the copy, the new one starts only
    // after the proof and the mark, and the old one is disabled only once the new one runs.
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    assert!(at("new: before_start")? < at("new: copy warm")?);
    assert!(at("new: copy warm")? < at("old: systemctl freeze")?);
    assert!(at("old: systemctl freeze")? < at("new: copy frozen")?);
    assert!(at("new: copy frozen")? < at("new: inventory")?);
    assert!(at("new: inventory")? < at("old: no signal thaws from here")?);
    assert!(at("old: no signal thaws from here")? < at("new: mark")?);
    assert!(at("new: mark")? < at("new: start")?);
    assert!(at("old: systemctl disable")? < at("new: mark")?);
    assert!(at("new: start")? < at("new: finish")?);
    assert!(!events.contains(&"old: systemctl thaw".to_owned()));

    // What is left for people is said: the App, laptops, the old disk.
    for name in ["GitHub App", "remote clients", "old host"] {
        assert_eq!(outcome(&status, name), Some(HostOutcome::Manual), "{name}");
    }
    let app = line(&status, "GitHub App").ok_or("no GitHub line")?;
    assert!(
        app.detail
            .contains(&format!("{NEW_URL}/api/auth/github/callback"))
    );
    // Laptops' CLI follows the old URL's redirect by itself; Claude Code's connection does not.
    let clients = line(&status, "remote clients").ok_or("no line for clients")?;
    assert!(
        clients.detail.contains("`yulius remote` follows")
            && clients.detail.contains("claude mcp add"),
        "{}",
        clients.detail
    );
    // With the App's own settings page, which only the running instance knows (SPEC-466).
    assert!(
        app.detail
            .contains("open https://github.com/organizations/acme/settings/apps/yulius-acme"),
        "{}",
        app.detail
    );
    assert_eq!(s.world.asked, [NEW_URL]);
    assert_eq!(outcome(&status, "tasks"), Some(HostOutcome::Ok));
    assert_eq!(outcome(&status, "HTTPS"), Some(HostOutcome::Ok));
    // No secret value is shown or kept.
    let kept = fs::read_to_string(s.opts.state_dir.join("status.json"))?;
    for secret in ["jev-secret-value", "token-value", "proxy-secret-value"] {
        assert!(!kept.contains(secret) && !s.shown().contains(secret));
    }

    // Again: finished, nothing done.
    let before = s.events().len();
    assert_eq!(s.run()?, 0);
    assert_eq!(s.events().len(), before);
    assert!(
        s.shown()
            .contains("finished: the instance runs on root@185.189.47.160")
    );
    Ok(())
}

#[test]
fn the_plan_alone_changes_nothing() -> TestResult {
    let mut s = setup()?;
    s.opts.yes = false;
    assert_eq!(s.run()?, 0);
    assert!(
        s.shown().contains("Run it with --yes to move"),
        "{}",
        s.shown()
    );
    // Everything a person has to do is said before anything changes.
    let status = s.status()?;
    assert_eq!(status.phase, "Plan");
    assert_eq!(outcome(&status, "GitHub App"), Some(HostOutcome::Manual));
    let packages = line(&status, "packages").ok_or("no packages line")?;
    assert!(
        packages.detail.contains("libicu74 → libicu78"),
        "{}",
        packages.detail
    );
    // Docker's and GitHub's packages are not asked for before their sources are there.
    assert_eq!(packages.outcome, HostOutcome::Ok, "{}", packages.detail);
    let sources = line(&status, "apt sources").ok_or("no sources line")?;
    assert!(
        sources.detail.starts_with("2 third-party sources")
            && sources.detail.contains("2 of the packages"),
        "{}",
        sources.detail
    );
    assert!(line(&status, "downtime").is_some() && line(&status, "settings").is_some());
    // On the new host: the flow's own files and questions only.
    let new = &s.remote.host;
    assert!(!new.ran("apt-get install") && !new.ran("systemctl enable") && !new.ran("install "));
    assert!(new.runs("write").is_empty(), "{:?}", new.runs("write"));
    assert_eq!(s.old_unit(), (false, false));
    assert!(s.remote.sent.is_empty());

    // A person who says no gets the same.
    s.world.answer = Some(false);
    assert_eq!(s.run()?, 0);
    assert!(s.shown().contains("Nothing was changed."));
    assert_eq!(s.old_unit(), (false, false));
    Ok(())
}

#[test]
fn a_failure_while_preparing_leaves_the_old_instance_running_and_a_rerun_completes() -> TestResult {
    let mut s = setup()?;
    s.remote.host.file("/apt-broken", "");
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(status.phase, "Prepare");
    assert_eq!(outcome(&status, "packages"), Some(HostOutcome::Failed));
    assert_eq!(s.old_unit(), (false, false));
    assert!(
        !s.events()
            .iter()
            .any(|e| e.starts_with("new: copy") || e == "old: systemctl freeze")
    );

    // Fixed: the same command goes on, and what was done is found done.
    s.remote.host.files.remove(Path::new("/apt-broken"));
    s.world.shown.clear();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(status.phase, "done");
    let shown = s.shown();
    assert!(shown.contains("ok       one-time key"), "{shown}");
    assert!(shown.contains("ok       yulius build           the new host holds this build"));
    assert!(shown.contains("ok       apt sources"), "{shown}");
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_copy_that_differs_thaws_the_old_instance_and_starts_nothing() -> TestResult {
    let mut s = setup()?;
    s.remote.tamper = Some(|copy| {
        let _ = fs::write(copy.join("sysroots/sb/upper/tool"), vec![1u8; 1499]);
    });
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let proof = line(&status, "copy proven").ok_or("no proof line")?;
    assert_eq!(proof.outcome, HostOutcome::Failed);
    assert!(
        proof
            .detail
            .contains("sysroots: 4 entries, 1500 bytes on the old host"),
        "{}",
        proof.detail
    );
    // Thawed and still enabled; the new instance was never started or marked.
    assert_eq!(s.old_unit(), (false, false));
    let events = s.events();
    assert!(events.contains(&"old: systemctl thaw".to_owned()));
    assert!(!events.iter().any(|e| e == "new: start" || e == "new: mark"));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::No);

    assert_eq!(
        s.local
            .text(profile::CADDYFILE)
            .map(|t| t.contains("reverse_proxy")),
        Some(true)
    );

    // A copy that is not the old host's data is not built on: the receipts are dropped with
    // it, the same command removes it, copies everything again and completes.
    assert!(
        s.shown()
            .contains("is dropped: the next run copies everything again"),
        "{}",
        s.shown()
    );
    assert!(!s.opts.state_dir.join("copy").exists());
    s.remote.tamper = None;
    s.remote.streams.clear();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(s.remote.host.ran("rm -rf --one-file-system -- "));
    assert!(
        s.remote.streams[0]
            .1
            .contains(&"data/sysroots/sb/upper/tool".to_owned())
    );
    assert_eq!(
        fs::read(s.remote.copy_dir.join("sysroots/sb/upper/tool"))?.len(),
        1500
    );
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

/// SPEC-458 beside SPEC-460: on a host whose sessions live in their own unit the cutover freezes
/// that unit with the server's, and a move that does not happen thaws both, the sessions unit
/// first. A sessions unit left frozen would leave every agent frozen beside a server that runs.
#[test]
fn a_host_with_a_sessions_unit_is_frozen_and_thawed_whole() -> TestResult {
    let mut s = setup()?;
    s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(false);
    s.remote.tamper = Some(|copy| {
        let _ = fs::write(copy.join("sysroots/sb/upper/tool"), vec![1u8; 1499]);
    });
    assert_eq!(s.run()?, 1);
    let events = s.events();
    let at = |event: &str| events.iter().position(|e| e == event);
    assert!(
        at("old: systemctl freeze").is_some() && at("old: systemctl freeze sessions").is_some(),
        "{events:#?}"
    );
    assert!(
        at("old: systemctl thaw sessions").is_some()
            && at("old: systemctl thaw sessions") < at("old: systemctl thaw"),
        "{events:#?}"
    );
    let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
    assert_eq!((unit.frozen, unit.sessions), (false, Some(false)));
    Ok(())
}

/// A sessions unit that is frozen while the server's is not (an earlier run that was killed
/// half-way) is a frozen instance: the move refuses as for a frozen server unit and names the
/// command that thaws both.
#[test]
fn a_frozen_sessions_unit_alone_is_a_frozen_instance() -> TestResult {
    let mut s = setup()?;
    s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(true);
    assert_eq!(s.run()?, 2, "{}", s.shown());
    assert!(
        s.shown()
            .contains("systemctl thaw yulius-sandboxes.service yulius.service"),
        "{}",
        s.shown()
    );
    assert!(!s.events().iter().any(|e| e.contains("freeze")));
    Ok(())
}

/// An abort brings the whole old instance back: the sessions unit is thawed with the server's.
#[test]
fn abort_thaws_the_sessions_unit_too() -> TestResult {
    let mut s = setup()?;
    {
        let mut unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
        unit.sessions = Some(false);
    }
    s.remote.healthy = false;
    let _ = s.run()?;
    {
        let mut unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
        unit.frozen = true;
        unit.sessions = Some(true);
    }
    let _ = s.abort()?;
    let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
    assert_eq!(
        (unit.frozen, unit.sessions),
        (false, Some(false)),
        "{:#?}",
        s.events()
    );
    Ok(())
}

/// Review of the merge (medium): "is the old instance still frozen" before the new one starts
/// means all of it. One unit thawed alone during the copy (by hand, or a restart of it) leaves
/// the other frozen, and "something is frozen" would let the new instance start beside it.
#[test]
fn one_unit_thawed_alone_during_the_copy_starts_nothing() -> TestResult {
    for change in [(|unit| unit.frozen = false) as fn(&mut Systemd), |unit| {
        unit.sessions = Some(false)
    }] {
        let mut s = meanwhile(change)?;
        s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(false);
        assert_eq!(s.run()?, 1);
        assert!(
            s.shown().contains("is no longer known to be frozen"),
            "{}",
            s.shown()
        );
        let events = s.events();
        assert!(
            !events.iter().any(|e| e == "new: mark" || e == "new: start"),
            "{events:?}"
        );
        // Nothing was started, so all of it runs again: none of it is left frozen.
        let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
        assert_eq!((unit.frozen, unit.sessions), (false, Some(false)));
    }
    Ok(())
}

/// Not knowing is not frozen, for the sessions unit as for the server's: when systemd stops
/// answering about it during the copy, nothing is started.
#[test]
fn a_sessions_unit_systemd_says_nothing_about_starts_nothing() -> TestResult {
    let mut s = meanwhile(|unit| unit.sessions_silent = true)?;
    s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(false);
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown().contains("is no longer known to be frozen"),
        "{}",
        s.shown()
    );
    let events = s.events();
    assert!(
        !events.iter().any(|e| e == "new: mark" || e == "new: start"),
        "{events:?}"
    );
    Ok(())
}

/// Review of the merge (medium): every command a person is told to stop the new instance with
/// names its sessions unit. A new instance that could not be stopped through the key:
#[test]
fn a_new_instance_that_cannot_be_stopped_names_both_units_to_stop() -> TestResult {
    let mut s = setup()?;
    s.remote.healthy = false;
    s.remote.refuse = vec!["stop"];
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown().contains(
            "the new instance could not be stopped. Stop it on the new host (`systemctl \
             disable --now yulius.service; systemctl stop yulius-sandboxes.service`)"
        ),
        "{}",
        s.shown()
    );
    // Frozen is safe: nothing of the old instance was thawed.
    assert!(s.world.systemd.lock().map_err(|e| e.to_string())?.frozen);
    Ok(())
}

/// Review of the merge (low): a server's unit that was stopped from outside during the copy is
/// not thawed (systemd thaws no unit that does not run) and does not "run as before": the line
/// says that it is down and how to start it.
#[test]
fn a_server_stopped_during_the_copy_is_not_reported_as_thawed() -> TestResult {
    // Without a sessions unit, and with one (not frozen before the move).
    for sessions in [None, Some(false)] {
        let mut s = meanwhile(|unit| {
            unit.frozen = false;
            unit.stopped = true;
        })?;
        s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = sessions;
        assert_eq!(s.run()?, 1);
        assert!(!s.shown().contains("it runs as before"), "{}", s.shown());
        assert!(
            s.shown().contains(
                "yulius.service is not running: start it (`systemctl start yulius.service`)"
            ),
            "{}",
            s.shown()
        );
        // What the move froze of it is thawed all the same.
        let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
        assert_eq!(unit.sessions, sessions);
        if sessions.is_some() {
            assert!(
                s.events()
                    .iter()
                    .any(|e| e == "old: systemctl thaw sessions"),
                "{:#?}",
                s.events()
            );
        }
    }
    Ok(())
}

/// Review of the merge (nit): with nothing of the old instance to thaw, the command a failed
/// abort names is one that runs, not `systemctl thaw` without a unit.
#[test]
fn an_abort_that_cannot_start_the_old_unit_names_a_command_that_runs() -> TestResult {
    let mut s = setup()?;
    assert_eq!(s.run()?, 0);
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.stopped = true;
        unit.frozen = false;
        unit.no_enable = true;
    }
    s.world.https = false;
    assert_eq!(s.abort()?, 1);
    assert!(
        s.shown().contains(
            "yulius.service could not be brought back: `systemctl enable --now yulius.service`"
        ),
        "{}",
        s.shown()
    );
    assert!(!s.shown().contains("systemctl thaw ;"), "{}", s.shown());
    Ok(())
}

/// What the cutover says when the instance is not frozen as a whole right after its freeze
/// depends on what systemd said (SPEC-467 LFT-001): stopped, running unfrozen, or no answer.
#[test]
fn the_cutover_says_why_the_instance_is_not_frozen_after_its_freeze() {
    use crate::instance::migrate::{Old, not_frozen_after_freeze};
    let old = |active, stopped| Old {
        active,
        stopped,
        sessions_run: false,
        any_frozen: active,
        all_frozen: false,
    };
    // Stopped, or on its way up or down: not running either way.
    for stopped in [true, false] {
        let said = not_frozen_after_freeze("yulius.service", Some(old(false, stopped)));
        assert!(
            said.contains("is not running any more: start it (`systemctl start yulius.service`)"),
            "{said}"
        );
    }
    // One of its units runs unfrozen.
    let thawed = not_frozen_after_freeze("yulius.service", Some(old(true, false)));
    assert!(
        thawed.contains("yulius.service runs and is not frozen as a whole"),
        "{thawed}"
    );
    // systemd did not answer.
    let unknown = not_frozen_after_freeze("yulius.service", None);
    assert!(
        unknown.contains("systemd does not say whether yulius.service is frozen as a whole"),
        "{unknown}"
    );
    for said in [&thawed, &unknown] {
        assert!(!said.contains("is not running any more"), "{said}");
        assert!(said.contains("nothing is copied"), "{said}");
    }
}

/// A sessions unit: the host has one, frozen or not.
fn sessions(s: &Setup, frozen: bool) -> Result<(), String> {
    s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(frozen);
    Ok(())
}

/// The old instance's two units as systemd holds them: the server's frozen, the sessions'.
fn units(s: &Setup) -> Result<(bool, Option<bool>), String> {
    let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
    Ok((unit.frozen, unit.sessions))
}

/// SPEC-467 LFT-002 beside SPEC-458: a recorded start goes on only with the whole old instance
/// frozen, and one that finds a unit of it running beside a new instance that reports the
/// build freezes that unit to stay. Each half alone, and both.
#[test]
fn a_recorded_start_freezes_whatever_unit_runs_beside_the_new_instance() -> TestResult {
    // (the server's unit frozen, the sessions unit frozen) as the run finds them, and what
    // the checklist names as having run.
    for (server, its_sessions, ran) in [
        (true, false, "yulius-sandboxes.service ran here"),
        (false, true, "yulius.service ran here"),
        (
            false,
            false,
            "yulius.service and yulius-sandboxes.service ran here",
        ),
    ] {
        let mut s = killed_while_starting()?;
        s.world.systemd.lock().map_err(|e| e.to_string())?.frozen = server;
        sessions(&s, its_sessions)?;
        assert_eq!(s.run()?, 0, "{}", s.shown());
        let events = s.events();
        // Only asked: nothing is started beside an old instance that runs.
        assert_eq!(events.first().map(String::as_str), Some("new: health"));
        assert!(
            !events
                .iter()
                .any(|e| e == "new: start" || e == "new: stop" || e.contains("thaw")),
            "{events:?}"
        );
        let shown = s.shown();
        assert!(
            shown.contains(&format!("{ran} beside the new instance")),
            "{shown}"
        );
        assert!(
            shown.contains("stays frozen and is disabled") && !shown.contains(MAKE_SURE_OF),
            "{shown}"
        );
        // All of it is frozen, the server's unit disabled, and the move goes on to its finish.
        assert_eq!(units(&s)?, (true, Some(true)), "{events:?}");
        assert_eq!(s.old_unit(), (true, true));
        // Only what ran was sent a freeze.
        assert_eq!(
            events.contains(&"old: systemctl freeze".to_owned()),
            !server
        );
        assert_eq!(
            events.contains(&"old: systemctl freeze sessions".to_owned()),
            !its_sessions
        );
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert_eq!((state.cutover, state.finished), (Cutover::Moved, true));
    }

    // The server's unit cannot be frozen (it is on its way up) while its sessions unit can:
    // the unit that was frozen is named all the same, beside the failure of the other.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.starting = true;
    }
    sessions(&s, false)?;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains(
            "yulius-sandboxes.service ran here beside the new instance (thawed or started \
             since the start was ordered): it is frozen again"
        ),
        "{shown}"
    );
    assert!(
        shown.contains("yulius.service may run beside the new instance and could not be frozen"),
        "{shown}"
    );
    assert!(shown.contains("must be frozen and disabled now"), "{shown}");
    assert!(!s.events().iter().any(|e| e.contains("thaw")));
    assert_eq!(units(&s)?, (false, Some(true)));

    // The sessions unit cannot be frozen: never exit 0, both units named in what to do, and
    // nothing thawed.
    let mut s = killed_while_starting()?;
    sessions(&s, false)?;
    s.world.systemd.lock().map_err(|e| e.to_string())?.no_freeze = true;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains(
            "runs beside the new instance and could not be frozen: `systemctl freeze \
             yulius-sandboxes.service` failed (exit 1)"
        ),
        "{shown}"
    );
    assert!(
        shown.contains(
            "stop it for good instead: `systemctl stop yulius.service \
             yulius-sandboxes.service; systemctl kill --kill-whom=all --signal=SIGKILL \
             yulius.service yulius-sandboxes.service`"
        ),
        "{shown}"
    );
    assert!(
        shown.contains(
            "must be frozen and disabled now that the new instance runs: `systemctl freeze \
             yulius.service yulius-sandboxes.service; systemctl disable yulius.service`"
        ),
        "{shown}"
    );
    assert!(!s.events().iter().any(|e| e.contains("thaw")));
    assert_eq!(units(&s)?, (true, Some(false)));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Moved);

    // systemd says nothing about the sessions unit: that is not "there is none". It is sent
    // the freeze, and the run fails saying that nobody knows whether it took.
    let mut s = killed_while_starting()?;
    sessions(&s, false)?;
    s.world
        .systemd
        .lock()
        .map_err(|e| e.to_string())?
        .sessions_silent = true;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("may run beside the new instance and could not be frozen")
            && shown.contains("yulius-sandboxes.service was sent the freeze"),
        "{shown}"
    );
    assert!(shown.contains("must be frozen and disabled now"), "{shown}");
    assert!(
        s.events()
            .contains(&"old: systemctl freeze sessions".to_owned())
    );
    assert!(!s.events().iter().any(|e| e.contains("thaw")));
    Ok(())
}

/// SPEC-467 LFT-002 beside SPEC-458: a server's unit that has stopped is not all of the old
/// instance. Its sessions unit that still runs is frozen to stay; one that is frozen already
/// is left as it is. Either way the server's unit, which systemd does not freeze, is named
/// with it for a person to end.
#[test]
fn a_recorded_start_with_a_stopped_server_freezes_its_sessions() -> TestResult {
    for frozen in [false, true] {
        let mut s = killed_while_starting()?;
        if let Ok(mut unit) = s.world.systemd.lock() {
            unit.frozen = false;
            unit.stopped = true;
        }
        sessions(&s, frozen)?;
        assert_eq!(s.run()?, 0, "{}", s.shown());
        let events = s.events();
        assert_eq!(events.first().map(String::as_str), Some("new: health"));
        // Its sessions are frozen (again), and nothing else of it is touched: no start, no
        // stop, no thaw, no disable of a unit a person is told to look at.
        assert_eq!(units(&s)?, (false, Some(true)));
        let touched: Vec<&String> = events
            .iter()
            .filter(|e| e.starts_with("old: systemctl") || e == &"new: start" || e == &"new: stop")
            .collect();
        if frozen {
            assert!(touched.is_empty(), "{touched:?}");
        } else {
            assert_eq!(touched, ["old: systemctl freeze sessions"]);
        }
        let shown = s.shown();
        assert_eq!(
            shown.contains("yulius-sandboxes.service ran here beside the new instance"),
            !frozen,
            "{shown}"
        );
        assert!(
            shown.contains(
                "yulius.service does not run here any more, and the new instance runs. Make \
                 sure nothing of the old one runs here: `systemctl stop yulius.service \
                 yulius-sandboxes.service; systemctl kill --kill-whom=all --signal=SIGKILL \
                 yulius.service yulius-sandboxes.service; systemctl disable yulius.service`"
            ),
            "{shown}"
        );
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert_eq!(state.cutover, Cutover::Moved);
    }

    // Its sessions cannot be frozen: they run beside the new instance, and the run says so
    // and does not end well.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.stopped = true;
        unit.no_freeze = true;
    }
    sessions(&s, false)?;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("could not be frozen: `systemctl freeze yulius-sandboxes.service` failed"),
        "{shown}"
    );
    assert!(!shown.contains(MAKE_SURE_OF), "{shown}");
    assert_eq!(units(&s)?, (false, Some(false)));
    Ok(())
}

/// SPEC-467 LFT-001 beside SPEC-458: when the instance is not known to be frozen as a whole
/// right after its freeze, all of it is thawed again, the sessions unit first, and the line
/// says what systemd said.
#[test]
fn a_failed_look_right_after_the_freeze_thaws_the_sessions_unit_too() -> TestResult {
    // systemd stops answering.
    let mut s = right_after_the_freeze(|unit| unit.silent = true)?;
    sessions(&s, false)?;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("systemd does not say whether yulius.service is frozen as a whole"),
        "{shown}"
    );
    assert!(shown.contains("thawed: it runs as before"), "{shown}");
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    assert!(at("old: systemctl freeze sessions")? < at("old: no signal thaws from here")?);
    assert!(at("old: no signal thaws from here")? < at("old: systemctl thaw sessions")?);
    assert!(at("old: systemctl thaw sessions")? < at("old: systemctl thaw")?);
    assert_eq!(units(&s)?, (false, Some(false)));
    assert!(!events.iter().any(|e| e == "new: copy frozen"));

    // Its sessions unit alone thawed from outside at that moment, and the server's unit alone.
    for change in [(|unit| unit.sessions = Some(false)) as Outside, |unit| {
        unit.frozen = false
    }] {
        let mut s = right_after_the_freeze(change)?;
        sessions(&s, false)?;
        assert_eq!(s.run()?, 1);
        let shown = s.shown();
        assert!(
            shown.contains("yulius.service runs and is not frozen as a whole"),
            "{shown}"
        );
        // Nothing of it is left frozen behind that line.
        assert!(shown.contains("thawed: it runs as before"), "{shown}");
        assert_eq!(units(&s)?, (false, Some(false)), "{:?}", s.events());
        assert!(!s.events().iter().any(|e| e == "new: copy frozen"));
    }

    // The server's unit stopped at that moment (systemd thaws what it stops), or died and is
    // on its way up again: its sessions, which the freeze took, are thawed without a failed
    // thaw of a unit that does not run (systemd thaws none with a job pending either).
    for change in [
        (|unit| {
            unit.frozen = false;
            unit.starting = true;
        }) as Outside,
        |unit| {
            unit.frozen = false;
            unit.stopped = true;
        },
    ] {
        let mut s = right_after_the_freeze(change)?;
        sessions(&s, false)?;
        assert_eq!(s.run()?, 1);
        let shown = s.shown();
        assert!(
            shown.contains("is not running any more: start it"),
            "{shown}"
        );
        // No thaw is reported as failed, none is left for a person to run, and a server's
        // unit that does not run is not said to run as before.
        assert!(!shown.contains("systemctl thaw"), "{shown}");
        assert!(!shown.contains("it runs as before"), "{shown}");
        assert_eq!(units(&s)?, (false, Some(false)));
        assert!(
            s.events()
                .contains(&"old: systemctl thaw sessions".to_owned())
                && !s.events().contains(&"old: systemctl thaw".to_owned()),
            "{:?}",
            s.events()
        );
    }
    Ok(())
}

/// The server's unit stopped since the plan while its sessions unit runs: the freeze takes the
/// sessions unit alone, the move ends, and nothing of the old instance may stay frozen for it.
#[test]
fn a_server_stopped_since_the_plan_leaves_no_sessions_frozen() -> TestResult {
    let mut s = setup()?;
    s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(false);
    s.remote.meanwhile = Some((
        "before_start",
        |unit| unit.stopped = true,
        Arc::clone(&s.world.systemd),
    ));
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown().contains("is not running any more: start it"),
        "{}",
        s.shown()
    );
    let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
    assert_eq!(unit.sessions, Some(false), "{:#?}", s.events());
    Ok(())
}

/// An abort on a host whose server's unit no longer runs while its sessions unit is frozen:
/// the server is started and the sessions are thawed, not left frozen under "runs here again".
#[test]
fn abort_with_a_stopped_server_thaws_its_sessions_unit() -> TestResult {
    let mut s = setup()?;
    s.world.systemd.lock().map_err(|e| e.to_string())?.sessions = Some(false);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.stopped = true;
        unit.frozen = false;
        unit.sessions = Some(true);
    }
    s.world.https = false;
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
    assert!(!unit.stopped && !unit.disabled && !unit.frozen);
    assert_eq!(unit.sessions, Some(false));
    Ok(())
}

#[test]
fn a_new_instance_that_does_not_come_up_is_stopped_before_the_old_one_thaws() -> TestResult {
    let mut s = setup()?;
    s.remote.healthy = false;
    assert_eq!(s.run()?, 1);
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    // Disabled while nobody knows whether the new one runs, enabled again once it is stopped.
    assert!(at("old: systemctl disable")? < at("new: start")?);
    assert!(at("new: start")? < at("new: stop")?);
    assert!(at("new: stop")? < at("old: systemctl enable")?);
    assert!(at("old: systemctl enable")? < at("old: systemctl thaw")?);
    assert_eq!(s.old_unit(), (false, false));
    assert!(s.remote.host.ran("systemctl disable --now yulius.service"));
    let status = s.status()?;
    let note = line(&status, "new host").ok_or("no note")?;
    assert!(
        note.detail.contains("no longer a clean copy"),
        "{}",
        note.detail
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::RolledBack);

    // The started copy is still there: the next run refuses in its plan and freezes nothing.
    s.remote.healthy = true;
    let before = events.len();
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let refusal = line(&status, "new host").ok_or("no refusal")?;
    assert_eq!(refusal.outcome, HostOutcome::Failed);
    assert!(
        refusal.detail.contains("holds an instance"),
        "{}",
        refusal.detail
    );
    assert!(
        !s.events()[before..]
            .iter()
            .any(|e| e.starts_with("old: systemctl"))
    );
    Ok(())
}

/// SPEC-470 MVB-004: a run asked without a person at this host (the move helper's) never
/// signs in as root does, also when that sign-in would work: the key's line is shown, and the
/// move goes on only once a person added it on the new host.
#[test]
fn a_run_without_a_sign_in_never_uses_roots_own() -> TestResult {
    let mut s = setup()?;
    s.opts.no_sign_in = true;
    assert!(s.remote.signs_in, "root's own sign-in would work");
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.remote.sign_ins, 0, "root's own sign-in was used");
    let status = s.status()?;
    let key = line(&status, "one-time key").ok_or("no key line")?;
    assert_eq!(key.outcome, HostOutcome::Manual);
    assert!(
        key.detail.contains("never signs in as root")
            && key
                .detail
                .contains("ssh-ed25519 AAAAC3FAKE yulius-migrate-"),
        "{}",
        key.detail
    );
    assert_eq!(status.lines.len(), 2, "the plan went on: {}", s.shown());
    assert!(
        status
            .public_key
            .as_deref()
            .is_some_and(|line| line.starts_with("ssh-ed25519 AAAAC3FAKE"))
    );
    assert!(!s.shown().contains("Signing in to"), "{}", s.shown());
    // Nothing was done on the new host, and its authorized_keys were not written.
    assert!(s.events().is_empty() && s.remote.host.log.is_empty());

    // A person added the line there: the same command goes on, still without a sign-in.
    s.remote.key = true;
    s.opts.yes = true;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.remote.sign_ins, 0);
    let status = s.status()?;
    assert_eq!(status.phase, "done");
    assert_eq!(status.public_key, None);
    Ok(())
}

/// SPEC-470 MVB-004: the connection the command itself makes for such a run offers the
/// one-time key alone and reads none of root's ssh configuration; any other run's is as before.
#[test]
fn the_connection_of_a_run_without_a_sign_in_is_key_only() -> TestResult {
    let mut s = setup()?;
    let state = s.state()?;
    let args = |opts: &Options| super::super::connection(opts, &state).args(true, "true");
    let usual = args(&s.opts);
    assert!(!usual.iter().any(|a| a == "-F"), "{usual:?}");
    s.opts.no_sign_in = true;
    let strict = args(&s.opts);
    assert_eq!(strict[..2], ["-F", "none"], "{strict:?}");
    assert!(
        strict
            .iter()
            .any(|a| a == "PreferredAuthentications=publickey")
    );
    // The recorded host, its port, and the move's own key.
    assert!(strict.iter().any(|a| a == "root@185.189.47.160"));
    let key = strict
        .iter()
        .position(|a| a == "-i")
        .map(|i| strict[i + 1].clone());
    assert_eq!(
        key,
        Some(s.opts.state_dir.join("key").display().to_string())
    );
    Ok(())
}

#[test]
fn without_a_sign_in_the_key_is_shown_for_a_person_to_add() -> TestResult {
    let mut s = setup()?;
    s.remote.signs_in = false;
    assert_eq!(s.run()?, 0);
    let status = s.status()?;
    let key = line(&status, "one-time key").ok_or("no key line")?;
    assert_eq!(key.outcome, HostOutcome::Manual);
    assert!(
        key.detail
            .contains("ssh-ed25519 AAAAC3FAKE yulius-migrate-"),
        "{}",
        key.detail
    );
    assert!(s.events().is_empty() && s.remote.host.log.is_empty());
    Ok(())
}

#[test]
fn a_name_that_does_not_lead_to_the_new_host_is_asked_for_first_and_again_after() -> TestResult {
    let mut s = setup()?;
    s.opts.public_url = Some("https://yulius.example.com/".into());
    s.world
        .dns
        .insert("yulius.example.com".into(), vec!["109.199.123.240".into()]);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let asked = status
        .lines
        .iter()
        .find(|l| l.name == "public URL")
        .ok_or("no URL line")?;
    assert_eq!(asked.outcome, HostOutcome::Manual);
    assert!(
        asked
            .detail
            .contains("yulius.example.com leads to 109.199.123.240, not to 185.189.47.160")
    );
    let https = line(&status, "HTTPS").ok_or("no HTTPS line")?;
    assert_eq!(https.outcome, HostOutcome::Manual);
    assert!(
        https
            .detail
            .contains("change its DNS record to 185.189.47.160 now"),
        "{}",
        https.detail
    );
    let unit = s
        .remote
        .host
        .text("/etc/systemd/system/yulius.service")
        .unwrap_or_default();
    assert!(
        unit.contains("YULIUS_PUBLIC_URL=https://yulius.example.com\n"),
        "{unit}"
    );

    // Not `https://host`: refused in the plan.
    let mut s = setup()?;
    s.opts.public_url = Some("http://yulius.example.com/x y".into());
    assert_eq!(s.run()?, 1);
    assert_eq!(
        outcome(&s.status()?, "public URL"),
        Some(HostOutcome::Failed)
    );
    assert_eq!(s.old_unit(), (false, false));
    Ok(())
}

#[test]
fn a_named_disk_must_be_blank_and_too_little_space_names_the_blank_ones() -> TestResult {
    let mut s = setup()?;
    s.opts.data_disks = vec!["/dev/sda".into()];
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(status.phase, "Plan");
    let space = line(&status, "disk space").ok_or("no line")?;
    assert!(
        space
            .detail
            .contains("blank on the new host: /dev/nvme0n1 (3.5 TiB)"),
        "{}",
        space.detail
    );
    assert!(!s.remote.host.ran("mkfs"));

    let mut s = setup()?;
    s.remote
        .host
        .says(&["stat", "-f", "-c"], "4096 1000 30000000\n");
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let space = line(&status, "disk space").ok_or("no line")?;
    assert!(
        space
            .detail
            .contains("name one, or two for a mirror, with --data-disk"),
        "{}",
        space.detail
    );

    // Two blank disks: the mirror is made while preparing, before anything is copied.
    let mut s = setup()?;
    s.opts.data_disks = vec!["/dev/nvme0n1".into(), "/dev/nvme1n1".into()];
    s.remote
        .host
        .says(&["blkid", "-s"], "0a1b2c3d-1111-2222-3333-444455556666\n");
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let events = s.events();
    let at = |what: &str| events.iter().position(|e| e == what);
    assert!(at("new: disk") < at("new: sources") && at("new: disk").is_some());
    assert!(s.remote.host.ran("mdadm --create /dev/md/yulius"));
    Ok(())
}

#[test]
fn what_cannot_be_moved_from_here_is_refused() -> TestResult {
    // From inside the unit (a Yulius terminal).
    let mut s = setup()?;
    s.local.file(
        "/proc/self/cgroup",
        "0::/system.slice/yulius.service/sandboxes/x\n",
    );
    assert_eq!(s.run()?, 2);
    assert!(s.shown().contains("would freeze itself"));
    // From inside the sessions unit (SPEC-458): the move freezes that unit too.
    let mut s = setup()?;
    s.local.says(
        &[
            "systemctl",
            "show",
            "--property=ControlGroup",
            "--value",
            "yulius-sandboxes.service",
        ],
        "/yulius.slice/yulius-sandboxes.service\n",
    );
    s.local.file(
        "/proc/self/cgroup",
        "0::/yulius.slice/yulius-sandboxes.service/sandboxes/x/agents\n",
    );
    assert_eq!(s.run()?, 2);
    assert!(s.shown().contains("would freeze itself"));
    // While an update runs.
    let mut s = setup()?;
    s.local.says(
        &["systemctl", "is-active", "yulius-update.service"],
        "active\n",
    );
    assert_eq!(s.run()?, 2);
    // A new host that runs an instance, or whose ports are taken.
    let mut s = setup()?;
    s.remote
        .host
        .says(&["systemctl", "is-active", "yulius.service"], "active\n");
    s.remote.host.says(
        &["ss", "-H", "-ltnp"],
        "LISTEN 0 511 0.0.0.0:443 0.0.0.0:* users:((\"nginx\",pid=7,fd=6))\n",
    );
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(outcome(&status, "new host"), Some(HostOutcome::Failed));
    assert_eq!(outcome(&status, "ports"), Some(HostOutcome::Failed));
    assert!(s.events().iter().all(|e| {
        [
            "new: build copied",
            "new: facts",
            "new: stage",
            "new: packages",
        ]
        .contains(&e.as_str())
    }));
    // Another move is recorded.
    let s = setup()?;
    s.state()?;
    let other = State::begin(&s.opts.state_dir, "root@elsewhere", 22);
    assert!(other.is_err_and(|why| why.contains("a move to root@185.189.47.160 is recorded")));
    // A state that cannot be read is not "no move".
    fs::write(s.opts.state_dir.join("state.json"), "{ not json")?;
    let broken = State::begin(&s.opts.state_dir, "root@185.189.47.160", 22);
    assert!(broken.is_err_and(|why| why.contains("cannot be read as a move's state")));
    Ok(())
}

#[test]
fn abort_never_leaves_both_running() -> TestResult {
    let mut s = setup()?;
    assert_eq!(s.run()?, 0);
    assert_eq!(s.old_unit(), (true, true));

    // After the finish the key is gone: while the new URL answers, nothing is touched.
    assert_eq!(s.abort()?, 1);
    assert_eq!(s.old_unit(), (true, true));
    // The new host is set up by this build: its sessions live in their own unit, which a stop
    // of the server's unit leaves running (SPEC-458 MOV-004).
    assert!(
        s.shown().contains(
            "it still answers at https://185-189-47-160.sslip.io: stop it on the new host \
             (`systemctl disable --now yulius.service; systemctl stop yulius-sandboxes.service`)"
        ),
        "{}",
        s.shown()
    );
    assert!(State::load(&s.opts.state_dir).is_some());

    // Silence alone is not proof that it is stopped: a person has to say so.
    s.world.https = false;
    s.opts.yes = false;
    assert_eq!(s.abort()?, 1);
    assert_eq!(s.old_unit(), (true, true));
    assert!(
        s.shown().contains(
            "does not prove that it is stopped: stop it on the new host (`systemctl disable \
             --now yulius.service; systemctl stop yulius-sandboxes.service`)"
        ),
        "{}",
        s.shown()
    );
    s.world.answer = Some(false);
    assert_eq!(s.abort()?, 1);
    assert_eq!(s.old_unit(), (true, true));

    // Stopped there, and said so: the old instance comes back with its proxy configuration.
    s.world.answer = Some(true);
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (false, false));
    let caddy = s.local.text(profile::CADDYFILE).unwrap_or_default();
    assert!(caddy.contains("reverse_proxy 127.0.0.1:8080"), "{caddy}");
    assert!(State::load(&s.opts.state_dir).is_none());
    assert_eq!(s.status()?.phase, "aborted");
    Ok(())
}

#[test]
fn abort_during_a_move_stops_the_new_host_through_the_key() -> TestResult {
    let mut s = setup()?;
    s.remote.healthy = false;
    assert_eq!(s.run()?, 1);
    // Rolled back already; the key is still there, so the abort cleans up through it.
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (false, false));
    assert!(
        s.remote
            .host
            .runs("systemctl disable --now yulius.service")
            .len()
            >= 2
    );
    assert_eq!(
        s.remote.host.text("/root/.ssh/authorized_keys"),
        Some("ssh-ed25519 AAAA admin@laptop\n")
    );
    assert!(!s.remote.key);
    Ok(())
}

#[test]
fn a_unit_that_is_frozen_before_the_move_is_not_touched() -> TestResult {
    // An earlier run that was killed, or an export in progress: said, not guessed.
    let mut s = setup()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = true;
    }
    assert_eq!(s.run()?, 2);
    assert!(
        s.shown().contains("yulius.service is frozen"),
        "{}",
        s.shown()
    );
    assert_eq!(s.old_unit(), (true, false));
    assert!(s.events().is_empty());
    Ok(())
}

/// A move whose earlier run ended (was killed) after it ordered the new instance's start.
fn killed_while_starting() -> Result<Setup, Box<dyn std::error::Error>> {
    let mut s = setup()?;
    let mut state = s.state()?;
    state.cutover = Cutover::Starting;
    state.new_url = Some(NEW_URL.into());
    state.save(&s.opts.state_dir)?;
    s.remote.comment = state.key_comment();
    s.remote.install_key()?;
    s.remote.put_build(Path::new("/usr/local/bin/yulius"))?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = true;
    }
    if let Ok(mut events) = s.world.events.lock() {
        events.clear();
    }
    Ok(s)
}

#[test]
fn a_run_that_ended_while_the_new_instance_started_goes_on_from_its_state() -> TestResult {
    // The new instance came up: the old one is disabled and the move finishes. Nothing is
    // planned, prepared or copied again.
    let mut s = killed_while_starting()?;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));
    let events = s.events();
    assert!(events.contains(&"new: start".to_owned()), "{events:?}");
    assert!(
        !events.iter().any(|e| {
            [
                "new: copy warm",
                "new: copy frozen",
                "new: packages",
                "new: sources",
                "old: systemctl freeze",
            ]
            .contains(&e.as_str())
        }),
        "{events:?}"
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!((state.cutover, state.finished), (Cutover::Moved, true));

    // It did not: it is stopped there, then the old one is thawed.
    let mut s = killed_while_starting()?;
    s.remote.healthy = false;
    assert_eq!(s.run()?, 1);
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    assert!(at("new: stop")? < at("old: systemctl thaw")?);
    assert_eq!(s.old_unit(), (false, false));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::RolledBack);
    Ok(())
}

#[test]
fn a_firewall_that_cannot_be_enabled_fails_the_run_but_the_move_is_finished() -> TestResult {
    let mut s = setup()?;
    s.remote.host.fails(
        &["ufw", "--force", "enable"],
        "ERROR: problem running ufw-init",
    );
    assert_eq!(s.run()?, 1);
    assert_eq!(outcome(&s.status()?, "firewall"), Some(HostOutcome::Failed));
    // The key is gone on both hosts: the same command has nothing more to do.
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert!(state.finished && !s.remote.key);
    assert_eq!(s.run()?, 0);
    Ok(())
}

#[test]
fn a_new_host_that_is_not_ready_is_found_before_anything_freezes() -> TestResult {
    let mut s = setup()?;
    s.remote.no_base = true;
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(status.phase, "Prepare");
    assert_eq!(outcome(&status, "sandbox base"), Some(HostOutcome::Failed));
    assert_eq!(s.old_unit(), (false, false));
    assert!(
        !s.events()
            .iter()
            .any(|e| e.starts_with("old: systemctl") || e.starts_with("new: copy"))
    );
    Ok(())
}

#[test]
fn the_new_hosts_word_is_not_the_proof() -> TestResult {
    let mut s = setup()?;
    s.remote.lies = true;
    s.remote.tamper = Some(|copy| {
        let _ = fs::write(copy.join("master.key"), b"another key");
    });
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let proof = line(&status, "copy proven").ok_or("no proof line")?;
    assert_eq!(proof.outcome, HostOutcome::Failed);
    assert!(proof.detail.starts_with("master.key: "), "{}", proof.detail);
    assert_eq!(s.old_unit(), (false, false));
    assert!(!s.events().contains(&"new: start".to_owned()));
    // What the new host says is shown without its control characters.
    assert!(s.shown().contains("trust me [2J") && !s.shown().contains('\u{1b}'));
    Ok(())
}

#[test]
fn a_terminal_lost_after_the_start_is_ordered_thaws_nothing() -> TestResult {
    let mut s = setup()?;
    // The line that reports the new instance cannot be shown any more.
    s.world.lost_at = Some("runs build");
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.run()));
    assert!(ended.is_err(), "the fake terminal did not go away");
    // The new instance runs: whatever ended this process, the old one stays frozen.
    assert!(s.events().contains(&"new: start".to_owned()));
    assert!(!s.events().contains(&"old: systemctl thaw".to_owned()));
    assert!(
        s.old_unit().0,
        "the old unit was thawed beside the new instance"
    );
    // The same command goes on from the recorded state.
    s.world.lost_at = None;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_new_instance_that_runs_is_never_rolled_back() -> TestResult {
    // It reports the build; enabling the helper fails after that.
    let mut s = setup()?;
    s.remote.host.fails(
        &["systemctl", "enable", "--now", "yulius-update.path"],
        "Unit yulius-update.path not found",
    );
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(outcome(&status, "update helper"), Some(HostOutcome::Failed));
    assert!(
        !s.events()
            .iter()
            .any(|e| e == "new: stop" || e == "old: systemctl thaw")
    );
    assert_eq!(s.old_unit(), (true, true));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!((state.cutover, state.finished), (Cutover::Moved, true));
    Ok(())
}

#[test]
fn a_lost_answer_to_the_start_is_decided_by_asking_the_new_host() -> TestResult {
    // The start happened, its answer did not arrive: the new host says that it runs.
    let mut s = setup()?;
    s.remote.lose = vec!["start"];
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(s.events().contains(&"new: health".to_owned()));
    assert!(!s.events().contains(&"new: stop".to_owned()));
    assert_eq!(s.old_unit(), (true, true));

    // Nobody answers: nothing is decided. The old instance stays frozen, and disabled, so a
    // restart of its host cannot start it beside a new one that may run.
    let mut s = setup()?;
    s.remote.refuse = vec!["start", "health"];
    assert_eq!(s.run()?, 1);
    assert_eq!(s.old_unit(), (true, true));
    assert!(!s.events().contains(&"old: systemctl thaw".to_owned()));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Starting);
    assert!(
        s.shown().contains("nobody knows whether it runs"),
        "{}",
        s.shown()
    );
    // The new host answers again: the same command starts it and finishes.
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_finish_that_did_not_reach_the_new_host_is_tried_again() -> TestResult {
    let mut s = setup()?;
    s.remote.refuse = vec!["finish"];
    assert_eq!(s.run()?, 1);
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!((state.cutover, state.finished), (Cutover::Moved, false));
    // The key is still on both hosts, and nothing claims the firewall is on.
    assert!(s.remote.key && !s.remote.host.ran("ufw --force enable"));
    assert!(
        s.shown().contains("the firewall there are still to do"),
        "{}",
        s.shown()
    );

    assert_eq!(s.run()?, 0, "{}", s.shown());
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert!(state.finished && !s.remote.key);
    assert!(s.remote.host.ran("ufw --force enable"));
    Ok(())
}

#[test]
fn a_run_that_ended_before_the_proof_goes_on_from_the_staged_copy() -> TestResult {
    // The frozen pass was installed; the old host never heard the copy's count (a cut link).
    let mut s = setup()?;
    s.remote.refuse = vec!["inventory"];
    assert_eq!(s.run()?, 1);
    assert_eq!(s.old_unit(), (false, false));
    assert!(!s.events().contains(&"new: start".to_owned()));
    // Not counted is not disproven: the staged copy stays, and the same command sends only
    // what the frozen pass sent (it earned no receipt) before it proves and starts.
    s.remote.streams.clear();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(!s.remote.host.ran("rm -rf --one-file-system -- "));
    assert!(
        s.remote
            .streams
            .iter()
            .all(|(_, paths)| !paths.contains(&"data/yulius.db".to_owned())),
        "{:?}",
        s.remote.streams
    );
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn abort_starts_an_old_unit_that_no_longer_runs() -> TestResult {
    let mut s = setup()?;
    assert_eq!(s.run()?, 0);
    // The old host was restarted since: its unit is disabled and does not run, so there is
    // nothing to thaw.
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.stopped = true;
        unit.frozen = false;
    }
    s.world.https = false;
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    let unit = s.world.systemd.lock().map_err(|e| e.to_string())?;
    assert!(!unit.stopped && !unit.disabled && !unit.frozen);
    Ok(())
}

#[test]
fn an_old_instance_that_does_not_run_is_refused_before_anything() -> TestResult {
    // What a stopped unit left running could not be frozen.
    let mut s = setup()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.stopped = true;
    }
    assert_eq!(s.run()?, 2);
    assert!(
        s.shown()
            .contains("yulius.service is not running: start it"),
        "{}",
        s.shown()
    );
    assert!(s.events().is_empty() && s.remote.host.log.is_empty());
    // And when systemd does not say what state it is in.
    let mut s = setup()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.silent = true;
    }
    assert_eq!(s.run()?, 2);
    assert!(s.shown().contains("systemd does not say"), "{}", s.shown());
    // And a build that names no commit could not be told from another on the new host.
    let mut s = setup()?;
    s.world.no_commit = true;
    assert_eq!(s.run()?, 2);
    assert!(s.shown().contains("names no commit"), "{}", s.shown());
    assert!(s.events().is_empty());
    Ok(())
}

/// The old unit is frozen by the move; then, during the proof, something else happens to it.
fn meanwhile(change: fn(&mut Systemd)) -> Result<Setup, Box<dyn std::error::Error>> {
    let mut s = setup()?;
    s.remote.meanwhile = Some(("inventory", change, Arc::clone(&s.world.systemd)));
    Ok(s)
}

#[test]
fn an_old_unit_not_known_to_be_frozen_starts_nothing() -> TestResult {
    // Stopped from outside: systemd thaws what it stops, and with KillMode=process its agents
    // run on. Or systemd no longer answers. Neither is "held".
    for change in [
        (|unit| {
            unit.frozen = false;
            unit.stopped = true;
        }) as fn(&mut Systemd),
        |unit| unit.silent = true,
    ] {
        let mut s = meanwhile(change)?;
        assert_eq!(s.run()?, 1);
        assert!(
            s.shown().contains("is no longer known to be frozen"),
            "{}",
            s.shown()
        );
        let events = s.events();
        assert!(
            !events.iter().any(|e| {
                ["new: mark", "new: start", "old: systemctl disable"].contains(&e.as_str())
            }),
            "{events:?}"
        );
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert_eq!(state.cutover, Cutover::No);
        // Nothing was started, so the old unit is not left frozen without a word.
        assert!(!s.old_unit().0, "{}", s.shown());
    }
    Ok(())
}

#[test]
fn a_unit_that_stopped_since_the_plan_is_found_before_anything_is_copied_frozen() -> TestResult {
    let mut s = setup()?;
    // The last step before the cutover: the unit is stopped meanwhile.
    s.remote.meanwhile = Some((
        "before_start",
        |unit| unit.stopped = true,
        Arc::clone(&s.world.systemd),
    ));
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown().contains("is not running any more: start it"),
        "{}",
        s.shown()
    );
    // The copy went on warm meanwhile, which harms nothing; nothing is copied frozen and
    // nothing is thawed.
    assert!(
        !s.events()
            .iter()
            .any(|e| e == "new: copy frozen" || e == "old: systemctl thaw")
    );
    Ok(())
}

/// The move freezes the old unit; at its own look right after, something has happened to it.
fn right_after_the_freeze(change: fn(&mut Systemd)) -> Result<Setup, Box<dyn std::error::Error>> {
    let s = setup()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.after_freeze = Some((1, change));
    }
    Ok(s)
}

#[test]
fn a_look_that_fails_right_after_the_freeze_thaws_and_says_what_is_known() -> TestResult {
    // systemd stops answering: the unit is frozen, and nobody can tell.
    let mut s = right_after_the_freeze(|unit| unit.silent = true)?;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("systemd does not say whether yulius.service is frozen"),
        "{shown}"
    );
    assert!(!shown.contains("is not running any more"), "{shown}");
    // Thawed once the move took the freeze's end, and still enabled; nothing was copied
    // frozen.
    assert_eq!(s.old_unit(), (false, false), "{shown}");
    // What the freeze covered is thawed as the freeze knew it, without another question to a
    // systemd that does not answer: no thaw of a sessions unit this host does not have, and
    // no failure that did not happen.
    assert!(shown.contains("thawed: it runs as before"), "{shown}");
    assert!(
        !shown.contains("run it by hand") && !shown.contains("yulius-sandboxes.service"),
        "{shown}"
    );
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    assert!(at("old: systemctl freeze")? < at("old: no signal thaws from here")?);
    assert!(at("old: no signal thaws from here")? < at("old: systemctl thaw")?);
    assert!(
        !events.iter().any(|e| e == "new: copy frozen"),
        "{events:?}"
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::No);
    // systemd answers again: the same command starts over and completes.
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.silent = false;
    }
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));

    // Thawed from outside at that moment: it runs, and the line says that.
    let mut s = right_after_the_freeze(|unit| unit.frozen = false)?;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains(
            "yulius.service runs and is not frozen as a whole (a unit of it was thawed or \
             started from outside right after the freeze)"
        ),
        "{shown}"
    );
    assert!(!shown.contains("is not running any more"), "{shown}");
    assert!(
        !shown.contains("`systemctl thaw yulius.service` failed"),
        "{shown}"
    );
    assert_eq!(s.old_unit(), (false, false));
    assert!(!s.events().iter().any(|e| e == "new: copy frozen"));

    // Stopped from outside at that moment (systemd thaws what it stops): nothing is frozen, so
    // nothing is thawed, and the line says what to do.
    let mut s = right_after_the_freeze(|unit| {
        unit.frozen = false;
        unit.stopped = true;
    })?;
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("is not running any more: start it"),
        "{shown}"
    );
    assert!(
        !shown.contains("`systemctl thaw yulius.service` failed"),
        "{shown}"
    );
    assert!(!s.events().contains(&"old: systemctl thaw".to_owned()));
    Ok(())
}

#[test]
fn a_unit_thawed_from_outside_during_the_copy_starts_nothing() -> TestResult {
    let mut s = meanwhile(|unit| unit.frozen = false)?;
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown().contains("is no longer known to be frozen"),
        "{}",
        s.shown()
    );
    let events = s.events();
    assert!(
        !events.iter().any(|e| e == "new: mark" || e == "new: start"),
        "{events:?}"
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::No);
    // The copy there is still unfinished: the same command copies again and completes.
    s.remote.meanwhile = None;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    Ok(())
}

#[test]
fn an_old_unit_that_cannot_be_disabled_is_thawed_and_nothing_starts() -> TestResult {
    let mut s = setup()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.no_disable = true;
    }
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown()
            .contains("`systemctl disable yulius.service` failed"),
        "{}",
        s.shown()
    );
    assert_eq!(s.old_unit(), (false, false));
    assert!(!s.events().contains(&"new: start".to_owned()));

    // It fails only once the new instance runs (a run that goes on from a recorded start):
    // said as what a person must do now, and the move is recorded as done.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.no_disable = true;
    }
    assert_eq!(s.run()?, 1);
    assert!(
        s.shown().contains("must be frozen and disabled now"),
        "{}",
        s.shown()
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Moved);
    assert!(s.old_unit().0);
    Ok(())
}

#[test]
fn one_run_at_a_time_holds_the_state_directory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let held = crate::instance::migrate::lock(dir.path())?;
    let second = crate::instance::migrate::lock(dir.path());
    assert!(second.is_err_and(|why| why.contains("another `yulius instance migrate` runs")));
    drop(held);
    // Free once its last descriptor is closed. A test beside this one that starts a process
    // holds a copy between its fork and its exec, so the lock is waited for, with a bound.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match crate::instance::migrate::lock(dir.path()) {
            Ok(_) => break,
            Err(why) if std::time::Instant::now() >= deadline => return Err(why.into()),
            Err(_) => std::thread::yield_now(),
        }
    }
    Ok(())
}

#[test]
fn a_finish_whose_answer_was_lost_is_recorded_when_the_key_no_longer_signs_in() -> TestResult {
    let mut s = setup()?;
    s.remote.lose = vec!["finish"];
    assert_eq!(s.run()?, 1);
    // Done there (the key is gone from the new host), unheard here.
    assert!(s.remote.host.ran("ufw --force enable"));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert!(!state.finished);
    // The key does not sign in any more and the new instance answers: finished.
    s.remote.key = false;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(
        s.shown().contains("the finish there was done"),
        "{}",
        s.shown()
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert!(state.finished);
    Ok(())
}

/// SPEC-467 LFT-005: the old host still holds the limits mark of the import it once came from.
/// The import on the new host writes its own, so the mark is no part of the copy or its proof.
#[test]
fn a_limits_mark_left_on_the_old_host_does_not_fail_the_proof() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;
    const MARK: &str = yulius_app::limits::ADAPT_MARK;
    let mut s = setup()?;
    fs::write(s.opts.data_dir.join(MARK), b"")?;
    fs::File::options()
        .write(true)
        .open(s.opts.data_dir.join(MARK))?
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000))?;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let proof = line(&status, "copy proven").ok_or("no proof")?;
    assert_eq!(proof.outcome, HostOutcome::Ok, "{}", proof.detail);
    assert!(
        proof
            .detail
            .starts_with("13 entries and 50.3 KiB in 5 folders"),
        "{}",
        proof.detail
    );
    // The new host's mark is the import's own, of now.
    let there = fs::symlink_metadata(s.remote.copy_dir.join(MARK))?;
    assert!(there.is_file() && there.mtime() != 1_700_000_000);
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_finish_that_was_done_but_not_recorded_is_found_done() -> TestResult {
    // Whether the new URL answers from here or not (a DNS record still to change), and whether
    // the new host can be reached at all: the key's absence is this host's own record.
    for (https, down) in [(true, false), (false, false), (false, true)] {
        let mut s = setup()?;
        assert_eq!(s.run()?, 0, "{}", s.shown());
        // The finish ran on both hosts; its record did not survive (a full disk, a kill).
        let mut state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert!(state.finished && !s.remote.key);
        state.finished = false;
        state.save(&s.opts.state_dir)?;
        s.world.shown.clear();
        s.world.https = https;
        s.remote.down = down;
        // The key is deleted here, which only a finish does: found done, not waited for.
        assert_eq!(s.run()?, 0, "{}", s.shown());
        let shown = s.shown();
        assert!(
            shown.contains("the finish on the new host was done"),
            "{shown}"
        );
        assert!(!shown.contains("Run the command again"), "{shown}");
        // Nothing is deleted a second time, and nothing is tried over a key that is gone
        // (`ssh` would only complain about it before the line that says all is done).
        assert!(!shown.contains("one-time key (old host)"), "{shown}");
        assert_eq!(s.remote.keyless, 0);
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert!(state.finished);
        assert_eq!(s.old_unit(), (true, true));
        // And the next run has nothing to do.
        s.world.shown.clear();
        assert_eq!(s.run()?, 0);
        assert!(s.shown().contains("finished: the instance runs on"));
    }
    Ok(())
}

/// The way out a failed freeze beside a running new instance names. SIGKILL: a process that
/// ignores SIGTERM (an interactive shell) would be left.
const STOP_FOR_GOOD: &str = "stop it for good instead: `systemctl stop yulius.service; systemctl \
                             kill --kill-whom=all --signal=SIGKILL yulius.service`";
/// How the line for an old instance of which nothing runs begins.
const MAKE_SURE_OF: &str = "Make sure nothing of the old one runs here";
/// What that line tells to run on a host without a sessions unit.
const MAKE_SURE: &str = "Make sure nothing of the old one runs here: `systemctl stop \
                         yulius.service; systemctl kill --kill-whom=all --signal=SIGKILL \
                         yulius.service; systemctl disable yulius.service`";

#[test]
fn a_recorded_start_never_ends_with_both_instances_running() -> TestResult {
    // Thawed by hand since, and the new instance reports the build: it has taken over, so the
    // old one is frozen again here and now, then disabled as after any start.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
    }
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    assert_eq!(at("new: health")?, 0, "{events:?}");
    assert!(at("new: health")? < at("old: systemctl freeze")?);
    assert!(at("old: systemctl freeze")? < at("old: systemctl disable")?);
    assert!(
        !events
            .iter()
            .any(|e| e == "new: start" || e == "new: stop" || e == "old: systemctl thaw"),
        "{events:?}"
    );
    assert_eq!(s.old_unit(), (true, true));
    let shown = s.shown();
    assert!(
        shown.contains("yulius.service ran here beside the new instance"),
        "{shown}"
    );
    assert!(
        !shown.contains("Make sure nothing of the old one runs here"),
        "{shown}"
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!((state.cutover, state.finished), (Cutover::Moved, true));

    // It cannot be frozen: a failure with the reason and the commands, never exit 0; the move
    // is recorded all the same, for the new instance has taken over. And nothing is thawed:
    // once a start was ordered no failure thaws, the freeze's own included.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.no_freeze = true;
    }
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("runs beside the new instance and could not be frozen"),
        "{shown}"
    );
    assert!(shown.contains("must be frozen and disabled now"), "{shown}");
    // And what to do when a freeze keeps failing: systemd freezes only a unit that runs.
    assert!(shown.contains(STOP_FOR_GOOD), "{shown}");
    assert!(
        !shown.contains("Make sure nothing of the old one runs here"),
        "{shown}"
    );
    assert!(!s.events().contains(&"old: systemctl thaw".to_owned()));
    assert_eq!(s.old_unit(), (false, true));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Moved);

    // A freeze that does not complete (a process hangs in the kernel) is left on its way: the
    // more of the old instance is frozen the better, so the export's way out, a thaw, is not
    // taken beside a new instance that runs.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.never_frozen = true;
    }
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(shown.contains("the freeze is left in place"), "{shown}");
    assert!(shown.contains("must be frozen and disabled now"), "{shown}");
    assert!(shown.contains(STOP_FOR_GOOD), "{shown}");
    let events = s.events();
    assert!(
        events.contains(&"old: systemctl freeze".to_owned()),
        "{events:?}"
    );
    assert!(
        !events.contains(&"old: systemctl thaw".to_owned()),
        "{events:?}"
    );
    assert!(s.world.systemd.lock().is_ok_and(|unit| unit.freezing));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Moved);

    // On its way up or down (a restart after a crash, a start or a stop in progress): that
    // is not "it does not run" either. systemd freezes no unit in that state, so the run
    // fails with the commands instead of ending well on a guess.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.starting = true;
    }
    assert_eq!(s.run()?, 1, "{}", s.shown());
    let shown = s.shown();
    assert!(
        shown.contains("may run beside the new instance and could not be frozen"),
        "{shown}"
    );
    assert!(shown.contains("must be frozen and disabled now"), "{shown}");
    // A unit on its way up or down is not frozen by systemd whatever is tried: the line says
    // how to end it.
    assert!(shown.contains(STOP_FOR_GOOD), "{shown}");
    assert!(
        !shown.contains("does not run here any more")
            && !shown.contains("Make sure nothing of the old one runs here"),
        "{shown}"
    );
    assert!(!s.events().contains(&"old: systemctl thaw".to_owned()));
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Moved);

    // The same, settled by the time the freeze is asked for: frozen, and the line does not
    // claim to know that it ran.
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.starting = true;
        unit.settles = true;
    }
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(
        s.shown()
            .contains("yulius.service may have run here beside the new instance"),
        "{}",
        s.shown()
    );
    assert_eq!(s.old_unit(), (true, true));

    // systemd does not say what the old unit does: that is not "it does not run".
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.silent = true;
    }
    assert_eq!(s.run()?, 1);
    let shown = s.shown();
    assert!(
        shown.contains("may run beside the new instance and could not be frozen"),
        "{shown}"
    );
    assert!(shown.contains("must be frozen and disabled now"), "{shown}");
    assert!(
        !shown.contains("Make sure nothing of the old one runs here"),
        "{shown}"
    );
    Ok(())
}

#[test]
fn a_recorded_start_waits_for_a_new_host_that_cannot_be_reached() -> TestResult {
    // The run that ordered the start had added the key: a new host that does not answer now is
    // not a key to add again. Nothing is decided and nothing is touched, whatever the old unit
    // does, and the run does not end as if all were well.
    for frozen in [true, false] {
        let mut s = killed_while_starting()?;
        if let Ok(mut unit) = s.world.systemd.lock() {
            unit.frozen = frozen;
        }
        s.remote.down = true;
        assert_eq!(s.run()?, 1, "{}", s.shown());
        let shown = s.shown();
        assert!(shown.contains("cannot be reached"), "{shown}");
        assert!(shown.contains("Connection timed out"), "{shown}");
        assert!(
            shown.contains("run the command again when the new host answers"),
            "{shown}"
        );
        assert!(
            !shown.contains("add this line") && !shown.contains("Signing in"),
            "{shown}"
        );
        assert!(!shown.contains("--abort` goes back"), "{shown}");
        assert!(s.events().is_empty(), "{:?}", s.events());
        assert_eq!(s.old_unit(), (frozen, false));
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert_eq!(state.cutover, Cutover::Starting);
        // It answers again: the same command decides from the new instance's state.
        s.remote.down = false;
        assert_eq!(s.run()?, 0, "{}", s.shown());
        assert_eq!(s.old_unit(), (true, true));
    }

    // A key the new host refuses is another matter: it is added again through a sign-in, as
    // in a plan, and the run goes on.
    let mut s = killed_while_starting()?;
    s.remote.key = false;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(s.shown().contains("made and added to"), "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_recorded_start_goes_on_only_with_the_old_unit_frozen() -> TestResult {
    // The old host was restarted meanwhile (its disabled unit does not run) and the new instance
    // reports the build: it is only asked, the move is recorded, and a person is told what to
    // make sure of.
    let stopped = || -> Result<Setup, Box<dyn std::error::Error>> {
        let s = killed_while_starting()?;
        if let Ok(mut unit) = s.world.systemd.lock() {
            unit.frozen = false;
            unit.stopped = true;
        }
        Ok(s)
    };
    let untouched = |s: &Setup| {
        !s.events()
            .iter()
            .any(|e| e == "new: start" || e == "new: stop" || e.starts_with("old: systemctl"))
    };
    let mut s = stopped()?;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.events().first().map(String::as_str), Some("new: health"));
    assert!(untouched(&s), "{:?}", s.events());
    assert!(s.shown().contains(MAKE_SURE), "{}", s.shown());
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Moved);

    // The same for a unit that systemd reports failed (its start limit was hit).
    let mut s = killed_while_starting()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.failed = true;
    }
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(untouched(&s), "{:?}", s.events());
    assert!(s.shown().contains(MAKE_SURE), "{}", s.shown());

    // The new instance does not run either: nothing is started; the abort is the way back.
    let mut s = stopped()?;
    s.remote.healthy = false;
    assert_eq!(s.run()?, 1);
    assert_eq!(s.events(), ["new: health"]);
    assert!(
        s.shown()
            .contains("`yulius instance migrate --abort` goes back"),
        "{}",
        s.shown()
    );
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!(state.cutover, Cutover::Starting);

    // The new host does not answer the question: that is not "the new instance does not run".
    // Whatever the old unit does, nothing is touched, and the same command asks again.
    for runs in [false, true] {
        let mut s = stopped()?;
        if let Ok(mut unit) = s.world.systemd.lock() {
            unit.stopped = !runs;
        }
        s.remote.refuse = vec!["health"];
        let asked = s.remote.asked;
        assert_eq!(s.run()?, 1);
        // The key is tried once: one answer decides whether it signs in, is to be added again
        // or the host is to be waited for.
        assert_eq!(s.remote.asked - asked, 1);
        let shown = s.shown();
        assert!(
            shown.contains("run the command again when the new host answers"),
            "{shown}"
        );
        assert!(shown.contains("Connection timed out"), "{shown}");
        assert!(!shown.contains("does not say that it runs"), "{shown}");
        assert!(!shown.contains("--abort` goes back"), "{shown}");
        assert!(untouched(&s), "{:?}", s.events());
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert_eq!(state.cutover, Cutover::Starting);
        // It answers again: the same command decides from the new instance's state.
        assert_eq!(s.run()?, 0, "{}", s.shown());
        let state = State::load(&s.opts.state_dir).ok_or("no state")?;
        assert_eq!(state.cutover, Cutover::Moved);
    }
    Ok(())
}

#[test]
fn the_plan_shows_the_host_key_and_names_files_that_mention_the_old_host() -> TestResult {
    let mut s = setup()?;
    s.opts.yes = false;
    s.local.file_as(
        "/etc/yulius/env",
        &format!("YULIUS_ALLOWED_ORIGINS=https://{OLD_HOST}\n"),
        0,
        0o600,
    );
    assert_eq!(s.run()?, 0);
    let status = s.status()?;
    let key = line(&status, "one-time key").ok_or("no key line")?;
    assert!(
        key.detail
            .contains("its host key: SHA256:abcDEF123 (ED25519)"),
        "{}",
        key.detail
    );
    let stale = status
        .lines
        .iter()
        .find(|l| l.name == "settings" && l.outcome == HostOutcome::Manual)
        .ok_or("no line for the stale file")?;
    assert!(stale.detail.contains("/etc/yulius/env"), "{}", stale.detail);
    // The reminder comes again at the end of a move, not only in its plan.
    let mut s = setup()?;
    assert_eq!(s.run()?, 0);
    let reminders = s
        .status()?
        .lines
        .iter()
        .filter(|l| l.name == "GitHub App")
        .count();
    assert_eq!(reminders, 2);
    Ok(())
}

#[test]
fn a_proxy_file_with_nothing_to_rewrite_is_left_alone() -> TestResult {
    let mut s = setup()?;
    let own = format!("{OLD_HOST} {{\n\treverse_proxy unix//run/yulius.sock\n}}\n");
    s.local.file("/etc/caddy/Caddyfile", &own);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let old_url = line(&status, "old URL").ok_or("no line")?;
    assert_eq!(old_url.outcome, HostOutcome::Manual);
    // Without the redirect nothing leads a laptop's CLI to the new URL.
    let clients = line(&status, "remote clients").ok_or("no line for clients")?;
    assert!(
        clients
            .detail
            .contains(&format!("`yulius remote login {NEW_URL}`")),
        "{}",
        clients.detail
    );
    assert!(
        old_url
            .detail
            .contains("has no `reverse_proxy` to port 8080"),
        "{}",
        old_url.detail
    );
    assert_eq!(s.local.text(profile::CADDYFILE), Some(own.as_str()));
    assert!(!s.opts.state_dir.join("Caddyfile.before").exists());
    Ok(())
}

#[test]
fn a_move_is_of_one_instance() -> TestResult {
    let s = setup()?;
    let mut state = s.state()?;
    let mut opts = s.opts.clone();
    opts.service = "yulius-prod.service".into();
    state.for_instance(&mut opts, false)?;
    // A run that names another instance is refused, and changes nothing of the record.
    let mut other = s.opts.clone();
    let refused = state.for_instance(&mut other, false);
    assert!(refused.is_err_and(|why| why.contains("is of yulius-prod.service")));
    // The same again is fine; an abort is of the recorded instance whatever it is given.
    state.for_instance(&mut opts.clone(), false)?;
    let mut abort = s.opts.clone();
    state.for_instance(&mut abort, true)?;
    assert_eq!(abort.service, "yulius-prod.service");
    Ok(())
}

#[test]
fn an_abort_that_stopped_the_new_instance_is_no_longer_a_finished_move() -> TestResult {
    // Through the key (the finish did not run), and then bringing the old unit back fails.
    let mut s = setup()?;
    s.remote.refuse = vec!["finish"];
    assert_eq!(s.run()?, 1);
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.silent = true;
    }
    assert_eq!(s.abort()?, 1);
    assert!(s.events().contains(&"new: stop".to_owned()));
    let state = State::load(&s.opts.state_dir).ok_or("the state is gone")?;
    assert_eq!(state.cutover, Cutover::RolledBack);
    // The same abort again, with systemd answering, brings the old instance back.
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.silent = false;
    }
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (false, false));
    Ok(())
}

// The warm copy (SPEC-465).

/// What a pass must follow: a rewrite in place, a deletion, a file that becomes a directory
/// and a directory that becomes a file, a broken hard link, new files.
fn churn(root: &Path) {
    let home = root.join("sandboxes/sb/home");
    let _ = fs::write(root.join("master.key"), b"KEY");
    let _ = fs::remove_dir_all(root.join("sysroots/sb"));
    let _ = fs::remove_file(home.join("link"));
    let _ = fs::create_dir(home.join("link"));
    let _ = fs::write(home.join("link/inside"), b"in");
    let _ = fs::remove_file(home.join("notes-again"));
    let _ = fs::write(home.join("notes-again"), b"own\n");
    let _ = fs::write(home.join("fresh"), b"fresh");
    let _ = fs::hard_link(home.join("fresh"), root.join("fresh-link"));
    let _ = fs::remove_dir_all(root.join("sandboxes/sb/work"));
    let _ = fs::write(root.join("sandboxes/sb/work"), b"a file now");
}

/// What changes in the last moment before the freeze.
fn last_changes(root: &Path) {
    let _ = fs::write(root.join("sandboxes/sb/home/fresh"), b"FRESH");
    let _ = fs::remove_file(root.join("sandboxes/sb/home/notes"));
}

/// Every file below `root` with its content, links as their text.
fn content(root: &Path) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let name = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            // The new host's own marks are not the copy.
            if name.to_string_lossy().starts_with(".yulius") {
                continue;
            }
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                found.push((name, b"<dir>".to_vec()));
                dirs.push(path);
            } else if meta.is_symlink() {
                let target = fs::read_link(&path)?;
                found.push((name, target.to_string_lossy().as_bytes().to_vec()));
            } else {
                found.push((name, fs::read(&path)?));
            }
        }
    }
    found.sort();
    Ok(found)
}

#[test]
fn the_copy_runs_warm_and_the_freeze_sends_only_the_last_changes() -> TestResult {
    let mut s = setup()?;
    fs::create_dir_all(s.opts.data_dir.join("sandboxes/sb/work"))?;
    fs::write(s.opts.data_dir.join("sandboxes/sb/work/below"), b"b")?;
    // After the first pass the instance has been busy; after the second, a little more.
    s.remote.churn = vec![churn, last_changes];
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(status.phase, "done");

    // Two passes while the instance ran, one frozen.
    let streams = &s.remote.streams;
    assert_eq!(
        streams.iter().map(|(live, _)| *live).collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(streams[0].1.len(), 15, "{:?}", streams[0].1);
    // The second holds what changed and nothing that did not.
    let second = &streams[1].1;
    for path in [
        "data/master.key",
        "data/sandboxes/sb/home/link",
        "data/sandboxes/sb/home/link/inside",
        "data/sandboxes/sb/home/notes-again",
        "data/fresh-link",
        "data/sandboxes/sb/work",
    ] {
        assert!(second.contains(&path.to_owned()), "{path}: {second:?}");
    }
    assert!(!second.contains(&"data/yulius.db".to_owned()), "{second:?}");
    // The frozen one: the last changes, and what waited for it (a directory that became a
    // file). Not the database, not the key.
    let frozen = &streams[2].1;
    assert!(frozen.contains(&"data/fresh-link".to_owned()), "{frozen:?}");
    assert!(
        frozen.contains(&"data/sandboxes/sb/work".to_owned()),
        "{frozen:?}"
    );
    assert!(
        !frozen.contains(&"data/yulius.db".to_owned())
            && !frozen.contains(&"data/master.key".to_owned()),
        "{frozen:?}"
    );

    // The new host holds the old host's tree: content, deletions, links.
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    assert!(!s.remote.copy_dir.join("sysroots/sb").exists());
    assert_eq!(
        fs::read(s.remote.copy_dir.join("sandboxes/sb/work"))?,
        b"a file now"
    );
    use std::os::unix::fs::MetadataExt as _;
    assert_eq!(
        fs::metadata(s.remote.copy_dir.join("fresh-link"))?.nlink(),
        2
    );
    assert_eq!(outcome(&status, "copy proven"), Some(HostOutcome::Ok));

    // In this order: the base before the first pass, the estimate before the freeze, the
    // freeze before the last pass, the proof before the mark and the start.
    let events = s.events();
    let at = |what: &str| {
        events
            .iter()
            .position(|e| e == what)
            .ok_or(format!("no {what}: {events:?}"))
    };
    assert!(at("new: before_start")? < at("new: copy warm")?);
    assert!(at("new: copy warm")? < at("old: systemctl freeze")?);
    assert!(at("old: systemctl freeze")? < at("new: copy frozen")?);
    assert!(at("new: copy frozen")? < at("new: inventory")?);
    assert!(at("new: inventory")? < at("new: mark")?);
    assert!(at("new: mark")? < at("new: start")?);
    let shown = s.shown();
    let said = |what: &str| shown.find(what).ok_or(format!("no {what:?} in {shown}"));
    assert!(said("\nCopy\n")? < said("ok       downtime               about ")?);
    assert!(said("ok       downtime               about ")? < said("\nCutover\n")?);
    let downtime = line(&status, "downtime").ok_or("no downtime line")?;
    assert!(
        downtime
            .detail
            .starts_with("about 30 s frozen: the entries are counted four times"),
        "{}",
        downtime.detail
    );
    // Each pass began with a sync of the data directory's filesystem; the frozen one needs
    // none.
    assert_eq!(s.world.syncs, 2);
    // Nothing is kept of the staged copy once the new instance runs.
    assert!(!s.opts.state_dir.join("copy").exists());
    assert!(!s.remote.copy_dir.join(INCOMPLETE).exists());
    Ok(())
}

#[test]
fn a_freeze_above_the_bound_is_never_begun() -> TestResult {
    let mut s = setup()?;
    // The start alone is allowed half a minute in the estimate: no bound below that is met.
    s.opts.max_downtime = Some(0);
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(status.phase, "Copy");
    let downtime = line(&status, "downtime").ok_or("no downtime line")?;
    assert_eq!(downtime.outcome, HostOutcome::Failed);
    assert!(
        downtime
            .detail
            .contains("above --max-downtime 0 after 5 passes")
            && downtime.detail.contains("Nothing was frozen"),
        "{}",
        downtime.detail
    );
    // It copied again instead of freezing: a full pass and five that sent only changes.
    assert_eq!(s.remote.streams.len(), 1 + warm::MAX_DELTA_PASSES as usize);
    assert!(s.remote.streams.iter().all(|(live, _)| *live));
    assert_eq!(
        s.shown().matches("so the move copies again").count(),
        warm::MAX_DELTA_PASSES as usize - 1,
        "{}",
        s.shown()
    );
    assert_eq!(s.old_unit(), (false, false));
    assert!(!s.events().iter().any(|e| e.starts_with("old: systemctl")));
    // The plan said so before anything was copied.
    let plan = status
        .lines
        .iter()
        .find(|l| l.name == "downtime")
        .ok_or("no plan line")?;
    assert!(
        plan.detail
            .contains("with --max-downtime 0 it begins only when that is at most 0 min"),
        "{}",
        plan.detail
    );

    // The staged copy stays: with a bound that can be met, the same command sends only what
    // changed and moves.
    assert!(Mark::read(&s.remote.copy_dir).is_some());
    s.opts.max_downtime = Some(10);
    s.remote.streams.clear();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(
        s.remote
            .streams
            .iter()
            .map(|(live, paths)| (*live, paths.len()))
            .collect::<Vec<_>>(),
        [(true, 0), (false, 0)]
    );
    assert!(
        s.shown()
            .contains("the new host holds a staged copy (13 entries acknowledged)"),
        "{}",
        s.shown()
    );
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_pass_that_was_cut_is_continued_by_the_same_command() -> TestResult {
    let mut s = setup()?;
    // The link breaks inside the third entry's header.
    s.remote.cut = Some(10_500);
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    assert_eq!(status.phase, "Copy");
    let copy = line(&status, "copy").ok_or("no copy line")?;
    assert_eq!(copy.outcome, HostOutcome::Failed);
    assert!(
        copy.detail
            .contains("keeps the 2 entries of this pass that arrived"),
        "{}",
        copy.detail
    );
    assert_eq!(s.old_unit(), (false, false));

    // Again: what was acknowledged is not sent again, the rest is, then the move goes on.
    s.remote.streams.clear();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let first = &s.remote.streams[0].1;
    assert!(
        !first.contains(&"data".to_owned()) && !first.contains(&"data/master.key".to_owned()),
        "{first:?}"
    );
    assert_eq!(first.len(), 11, "{first:?}");
    // A pass that only caught up is followed by one that finds changes only.
    assert_eq!(
        s.remote
            .streams
            .iter()
            .map(|(live, paths)| (*live, paths.len()))
            .collect::<Vec<_>>(),
        [(true, 11), (true, 0), (false, 0)]
    );
    assert!(!s.remote.host.ran("rm -rf --one-file-system -- "));
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}

#[test]
fn a_staged_copy_that_is_gone_or_another_one_makes_the_next_pass_a_full_one() -> TestResult {
    let bounded = || -> Result<Setup, Box<dyn std::error::Error>> {
        let mut s = setup()?;
        s.opts.max_downtime = Some(0);
        assert_eq!(s.run()?, 1);
        assert!(Mark::read(&s.remote.copy_dir).is_some());
        s.opts.max_downtime = None;
        s.remote.streams.clear();
        s.world.shown.clear();
        Ok(s)
    };
    // Removed on the new host.
    let mut s = bounded()?;
    fs::remove_dir_all(&s.remote.copy_dir)?;
    let data = s.remote.data.clone();
    s.remote
        .host
        .files
        .retain(|path, _| !path.starts_with(&data));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(
        s.shown()
            .contains("the staged copy this host knew is not on the new host"),
        "{}",
        s.shown()
    );
    assert_eq!(s.remote.streams[0].1.len(), 13);

    // Another stage there (another host's move, a copy put back): removed, never built on.
    let mut s = bounded()?;
    let mut mark = Mark::read(&s.remote.copy_dir).ok_or("no stage")?;
    mark.stage = "another".into();
    let text = serde_json::to_string(&mark)?;
    fs::write(s.remote.copy_dir.join(INCOMPLETE), &text)?;
    let marker = s.remote.data.join(INCOMPLETE).display().to_string();
    s.remote.host.file(&marker, &text);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(s.remote.host.ran("rm -rf --one-file-system -- "));
    assert_eq!(s.remote.streams[0].1.len(), 13);

    // The old host lost its receipts (its state directory was cleared): the same.
    let mut s = bounded()?;
    fs::remove_dir_all(s.opts.state_dir.join("copy"))?;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(s.remote.host.ran("rm -rf --one-file-system -- "));
    assert_eq!(s.remote.streams[0].1.len(), 13);
    Ok(())
}

#[test]
fn entries_too_fresh_for_a_receipt_are_sent_again_and_the_move_still_ends_right() -> TestResult {
    let mut s = setup()?;
    // The pass begins before the tree was written: nothing in it is old enough, however long
    // the test takes.
    s.world.ahead = -3600;
    s.remote.churn = vec![churn];
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let sent: Vec<usize> = s
        .remote
        .streams
        .iter()
        .map(|(_, paths)| paths.len())
        .collect();
    assert_eq!(sent.len(), 3);
    // Nothing earned a receipt, so the frozen pass sent the whole tree.
    assert_eq!(sent[2], 14, "{:?}", s.remote.streams[2].1);
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}

#[test]
fn a_data_directory_that_cannot_be_synced_is_not_copied_from() -> TestResult {
    let mut s = setup()?;
    s.world.no_sync = true;
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let copy = line(&status, "copy").ok_or("no copy line")?;
    assert!(copy.detail.contains("cannot sync"), "{}", copy.detail);
    assert!(s.remote.streams.is_empty());
    assert_eq!(s.old_unit(), (false, false));
    Ok(())
}

#[test]
fn a_rolled_back_move_leaves_no_staged_copy_to_go_on_with() -> TestResult {
    let mut s = setup()?;
    s.remote.healthy = false;
    assert_eq!(s.run()?, 1);
    // The copy was proven and started: its marker is gone, and so is what this host knew.
    assert!(!s.remote.copy_dir.join(INCOMPLETE).exists());
    s.remote.healthy = true;
    s.remote.streams.clear();
    assert_eq!(s.run()?, 1);
    assert!(s.remote.streams.is_empty());
    assert!(!s.opts.state_dir.join("copy").exists());
    let status = s.status()?;
    let refusal = line(&status, "new host").ok_or("no refusal")?;
    assert!(
        refusal.detail.contains("holds an instance"),
        "{}",
        refusal.detail
    );
    Ok(())
}

#[test]
fn a_rerun_does_not_ask_again_for_the_room_the_staged_copy_already_takes() -> TestResult {
    let mut s = setup()?;
    s.opts.max_downtime = Some(0);
    assert_eq!(s.run()?, 1);
    // The staged copy fills most of the new disk: less is free than the data takes.
    s.remote
        .host
        .says(&["stat", "-f", "-c"], "4096 1000 30000000\n");
    s.opts.max_downtime = None;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let space = line(&status, "disk space").ok_or("no line")?;
    assert_eq!(space.outcome, HostOutcome::Ok);
    assert!(
        space.detail.contains("the staged copy is there already"),
        "{}",
        space.detail
    );

    // Without a staged copy the same disk is too small, as before.
    let mut s = setup()?;
    s.remote
        .host
        .says(&["stat", "-f", "-c"], "4096 1000 30000000\n");
    assert_eq!(s.run()?, 1);
    assert_eq!(
        outcome(&s.status()?, "disk space"),
        Some(HostOutcome::Failed)
    );
    Ok(())
}

#[test]
fn an_abort_before_the_cutover_names_the_staged_copy_left_on_the_new_host() -> TestResult {
    let mut s = setup()?;
    s.opts.max_downtime = Some(0);
    assert_eq!(s.run()?, 1);
    assert!(s.opts.state_dir.join("copy/stage.json").exists());
    let looks = |s: &Setup| s.events().iter().filter(|e| *e == "new: stage").count();
    let before = looks(&s);
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    let status = s.status()?;
    let left = line(&status, "new host").ok_or("no line of the new host")?;
    assert_eq!(left.outcome, HostOutcome::Manual);
    assert!(
        left.detail
            .contains("still holds master.key and every secret"),
        "{}",
        left.detail
    );
    // What this host knew of it goes with the move.
    assert!(!s.opts.state_dir.join("copy").exists());
    assert_eq!(s.old_unit(), (false, false));
    // A stream the new host may still be taking in is closed before the flow's files go.
    let events = s.events();
    let last = |what: &str| events.iter().rposition(|e| e == what);
    assert!(last("new: stage").is_some() && last("new: stage") < last("new: finish"));
    assert_eq!(looks(&s), before + 1);
    Ok(())
}

#[test]
fn an_instance_updated_during_the_copy_is_not_frozen() -> TestResult {
    let mut s = setup()?;
    s.world.updated_to = Some("another-build");
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let old = line(&status, "old instance").ok_or("no line")?;
    assert_eq!(old.outcome, HostOutcome::Failed);
    assert!(
        old.detail.contains("reports build another-build now")
            && old.detail.contains("Nothing is frozen"),
        "{}",
        old.detail
    );
    assert_eq!(s.old_unit(), (false, false));
    assert!(
        !s.events()
            .iter()
            .any(|e| e.starts_with("old: systemctl") || e == "new: copy frozen")
    );
    // The staged copy stays for the run with the new build.
    assert!(Mark::read(&s.remote.copy_dir).is_some());
    s.world.updated_to = None;
    s.world.looks = 0;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    Ok(())
}

#[test]
fn any_bound_is_a_number_of_minutes() -> TestResult {
    let mut s = setup()?;
    s.opts.max_downtime = Some(u64::MAX);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_data_directory_that_holds_the_copy_s_marker_is_refused_in_the_plan() -> TestResult {
    for name in import::MARKERS {
        let mut s = setup()?;
        fs::write(s.opts.data_dir.join(name), b"")?;
        assert_eq!(s.run()?, 1);
        let status = s.status()?;
        assert_eq!(status.phase, "Plan", "{name}");
        let old = line(&status, "old host").ok_or("no line")?;
        assert_eq!(old.outcome, HostOutcome::Failed);
        assert!(
            old.detail.contains(&format!("holds {name}")),
            "{}",
            old.detail
        );
        assert!(s.remote.streams.is_empty() && s.remote.sent.is_empty());
        assert_eq!(s.old_unit(), (false, false));
    }
    Ok(())
}

#[test]
fn a_finished_move_keeps_no_receipts_however_it_got_there() -> TestResult {
    // A run that ended while the new instance started; this host was restarted since (its
    // disabled unit does not run) and the new instance runs. What this host knew of the
    // staged copy is of no use any more.
    let mut s = killed_while_starting()?;
    let copy = s.opts.state_dir.join("copy");
    fs::create_dir_all(&copy)?;
    fs::write(copy.join("receipts-0"), [0u8; 64])?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = false;
        unit.stopped = true;
    }
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let state = State::load(&s.opts.state_dir).ok_or("no state")?;
    assert_eq!((state.cutover, state.finished), (Cutover::Moved, true));
    assert!(!copy.exists(), "the receipts are still there");
    Ok(())
}

#[test]
fn a_bound_of_a_minute_can_be_met() -> TestResult {
    // The estimate allows the start half a minute: a quiet instance moves within one minute.
    let mut s = setup()?;
    s.opts.max_downtime = Some(1);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(s.old_unit(), (true, true));
    assert!(
        !s.shown().contains("so the move copies again"),
        "{}",
        s.shown()
    );
    Ok(())
}

#[test]
fn the_plan_gives_a_first_estimate_of_the_freeze_from_a_sample_of_the_tree() -> TestResult {
    // 6.4 million inodes are used where the data is; a pass looks at 100,000 entries a second.
    let mut s = setup()?;
    s.opts.yes = false;
    assert_eq!(s.run()?, 0);
    let status = s.status()?;
    let downtime = line(&status, "downtime").ok_or("no downtime line")?;
    assert_eq!(downtime.outcome, HostOutcome::Ok);
    // Four counts of 64 s and half a minute for the start.
    assert!(
        downtime.detail.contains(
            "then it is frozen for about 5 min: up to 6400000 entries are counted four times \
             (about 100000 a second here, twice on each host)"
        ) && downtime
            .detail
            .contains("It is measured again before the freeze"),
        "{}",
        downtime.detail
    );
    assert!(
        downtime
            .detail
            .starts_with("the instance runs during the copy (about "),
        "{}",
        downtime.detail
    );

    // A tree that cannot be sampled: no number, and the estimate still comes before the freeze.
    let mut s = setup()?;
    s.opts.yes = false;
    s.world.walk_rate = None;
    assert_eq!(s.run()?, 0);
    let status = s.status()?;
    let downtime = line(&status, "downtime").ok_or("no downtime line")?;
    assert!(
        !downtime.detail.contains("frozen for about")
            && downtime
                .detail
                .contains("The estimate is shown before the freeze"),
        "{}",
        downtime.detail
    );
    Ok(())
}

/// A move that copied and did not freeze (its bound cannot be met): the staged copy and its
/// receipts are there for the next run.
fn copied_and_not_frozen() -> Result<Setup, Box<dyn std::error::Error>> {
    let mut s = setup()?;
    s.opts.max_downtime = Some(0);
    assert_eq!(s.run()?, 1);
    assert!(Mark::read(&s.remote.copy_dir).is_some());
    s.opts.max_downtime = None;
    s.remote.streams.clear();
    s.world.shown.clear();
    Ok(s)
}

fn passes(s: &Setup) -> Vec<(bool, usize)> {
    s.remote
        .streams
        .iter()
        .map(|(live, paths)| (*live, paths.len()))
        .collect()
}

#[test]
fn after_a_restart_of_the_old_host_everything_is_sent_into_the_staged_copy_again() -> TestResult {
    // After a crash a file may hold newer content than the change time its receipt was earned
    // with: the receipts of the boot before are not believed.
    let mut s = copied_and_not_frozen()?;
    s.world.era.boot = "the boot after".into();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(
        s.shown()
            .contains("this host was restarted since the last pass")
            && s.shown()
                .contains("everything is sent into the staged copy again"),
        "{}",
        s.shown()
    );
    // Into the staged copy that is there: nothing is removed first.
    assert!(!s.remote.host.ran("rm -rf --one-file-system -- "));
    assert_eq!(passes(&s), [(true, 13), (true, 0), (false, 0)]);
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

#[test]
fn a_clock_set_back_during_a_pass_makes_the_next_one_send_everything_again() -> TestResult {
    let mut s = setup()?;
    // While the first pass is on its way.
    s.world.set_back_at = Some(("new: copy warm", 1));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(
        s.shown()
            .matches("this host's clock was set back since a pass began")
            .count(),
        1,
        "{}",
        s.shown()
    );
    // What the first pass sent earned nothing; the passes after it are a move like any other.
    assert_eq!(passes(&s), [(true, 13), (true, 13), (true, 0), (false, 0)]);
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}

#[test]
fn a_clock_set_back_before_the_freeze_copies_nothing_while_frozen() -> TestResult {
    let mut s = setup()?;
    // After the last warm pass was reconciled: the frozen pass would be planned on receipts
    // that are not believed any more.
    s.world.set_back_at = Some(("old: systemctl freeze", 1));
    assert_eq!(s.run()?, 1);
    let status = s.status()?;
    let copy = line(&status, "copy").ok_or("no copy line")?;
    assert_eq!(copy.outcome, HostOutcome::Failed);
    assert!(
        copy.detail
            .contains("clock was set back since the last pass: nothing is copied while frozen"),
        "{}",
        copy.detail
    );
    assert!(!s.remote.streams.iter().any(|(live, _)| !*live));
    assert!(s.events().contains(&"old: systemctl thaw".to_owned()));
    assert_eq!(s.old_unit(), (false, false));

    // The same command again: everything goes into the staged copy once more, warm, and the
    // move ends as it should.
    s.remote.streams.clear();
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(passes(&s), [(true, 13), (true, 0), (false, 0)]);
    assert!(!s.remote.host.ran("rm -rf --one-file-system -- "));
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}

#[test]
fn an_abort_names_the_copy_on_the_new_host_also_when_this_host_dropped_what_it_knew_of_it()
-> TestResult {
    // The proof differed: this host forgot the staged copy, which is on the new host still,
    // with master.key and every secret, until a next run removes it. There is none.
    let mut s = setup()?;
    s.remote.tamper = Some(|copy| {
        let _ = fs::write(copy.join("sysroots/sb/upper/tool"), vec![1u8; 1499]);
    });
    assert_eq!(s.run()?, 1);
    assert!(!s.opts.state_dir.join("copy").exists());
    assert!(Mark::read(&s.remote.copy_dir).is_some());
    let looks = |s: &Setup| s.events().iter().filter(|e| *e == "new: stage").count();
    let before = looks(&s);
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    let status = s.status()?;
    let left = line(&status, "new host").ok_or("no line of the new host")?;
    assert_eq!(left.outcome, HostOutcome::Manual);
    assert!(
        left.detail
            .contains("still holds master.key and every secret"),
        "{}",
        left.detail
    );
    // A stream the new host may still be taking in is closed before the key goes.
    assert_eq!(looks(&s), before + 1);

    // A move that never copied anything leaves nothing to name.
    let mut s = setup()?;
    s.remote.no_base = true;
    assert_eq!(s.run()?, 1);
    assert_eq!(s.status()?.phase, "Prepare");
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert!(line(&s.status()?, "new host").is_none());
    Ok(())
}

#[test]
fn a_clock_set_back_while_a_pass_syncs_is_found_when_the_pass_ends() -> TestResult {
    // The pass's start is read before the sync. A clock that goes back during the sync makes
    // what changes after that start look older than it: the lead the pass is measured against
    // is the one of its start, not one read after the sync.
    let mut s = setup()?;
    s.world.set_back_at = Some(("sync", 1));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(
        s.shown()
            .matches("this host's clock was set back since a pass began")
            .count(),
        1,
        "{}",
        s.shown()
    );
    assert_eq!(passes(&s), [(true, 13), (true, 13), (true, 0), (false, 0)]);
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}

#[test]
fn a_pass_of_changes_that_lost_its_receipts_is_not_followed_by_a_freeze() -> TestResult {
    // The clock goes back while the first pass of changes is on its way: its receipts and all
    // before them are dropped when it ends. The move copies again; it does not freeze the
    // instance for a pass that could not be sent.
    let mut s = setup()?;
    s.world.set_back_at = Some(("new: copy warm", 2));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert_eq!(
        passes(&s),
        [(true, 13), (true, 0), (true, 13), (true, 0), (false, 0)]
    );
    let events = s.events();
    assert_eq!(
        events
            .iter()
            .filter(|e| *e == "old: systemctl freeze")
            .count(),
        1
    );
    assert!(!events.contains(&"old: systemctl thaw".to_owned()));
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}

#[test]
fn a_clock_set_back_during_the_frozen_pass_does_not_announce_another_copy() -> TestResult {
    // Nothing changes while the instance is frozen, so the move goes on to the proof; what it
    // drops is only what a later run would have built on.
    let mut s = setup()?;
    s.world.set_back_at = Some(("new: copy frozen", 1));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(
        !s.shown()
            .contains("everything is sent into the staged copy again"),
        "{}",
        s.shown()
    );
    assert!(
        s.shown()
            .contains("this host's clock was set back during the frozen pass"),
        "{}",
        s.shown()
    );
    assert_eq!(passes(&s), [(true, 13), (true, 0), (false, 0)]);
    assert_eq!(s.old_unit(), (true, true));
    Ok(())
}

fn github_lines(status: &Status) -> Vec<(HostOutcome, &str)> {
    status
        .lines
        .iter()
        .filter(|l| l.name == "GitHub App")
        .map(|l| (l.outcome, l.detail.as_str()))
        .collect()
}

/// SPEC-466 MVC-021, MVC-022: the plan asks the old instance about the new URL's callback and the
/// finish asks the new instance: registered is `ok`, in both.
#[test]
fn a_callback_github_accepts_is_ok_in_the_plan_and_at_the_finish() -> TestResult {
    let mut s = setup()?;
    s.world.github = Ok(Some(facts("registered", NEW_URL)));
    s.remote.github = Ok(Some(facts("registered", NEW_URL)));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let said = github_lines(&status);
    let accepted = format!("GitHub accepts {NEW_URL}/api/auth/github/callback for sign-in");
    assert_eq!(
        said,
        [
            (HostOutcome::Ok, accepted.as_str()),
            (HostOutcome::Ok, accepted.as_str())
        ]
    );
    assert_eq!(s.world.asked, [NEW_URL]);
    // The new instance is asked while the one-time key still signs in.
    let events = s.events();
    let at = |what: &str| events.iter().position(|e| e == what);
    assert!(at("new: callback").is_some(), "{events:?}");
    assert!(at("new: callback") < at("new: finish"), "{events:?}");
    assert!(at("new: start") < at("new: callback"), "{events:?}");
    Ok(())
}

/// The person adds the callback while the move runs: the plan said what to do, the finish says ok.
#[test]
fn a_callback_registered_during_the_move_ends_ok() -> TestResult {
    let mut s = setup()?;
    s.remote.github = Ok(Some(facts("registered", NEW_URL)));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let said = github_lines(&status);
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(said[0].0, HostOutcome::Manual);
    assert!(
        said[0]
            .1
            .contains("open https://github.com/organizations/acme/settings/apps/yulius-acme"),
        "{}",
        said[0].1
    );
    assert_eq!(said[1].0, HostOutcome::Ok);
    Ok(())
}

/// An instance that cannot be asked (an older server, a lost key, the new host not reached for
/// that step) leaves the general note; the move goes on.
#[test]
fn an_instance_that_cannot_be_asked_leaves_the_general_note() -> TestResult {
    let mut s = setup()?;
    s.world.github = Err("127.0.0.1:8081: connection refused".into());
    s.remote.refuse = vec!["callback"];
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(status.phase, "done");
    let said = github_lines(&status);
    assert_eq!(said.len(), 2, "{said:?}");
    for (outcome, detail) in &said {
        assert_eq!(*outcome, HostOutcome::Manual);
        assert!(detail.contains("settings/apps/<app>"), "{detail}");
        assert!(
            detail.contains(&format!("{NEW_URL}/api/auth/github/callback")),
            "{detail}"
        );
    }
    assert!(said[0].1.contains("connection refused"), "{}", said[0].1);
    Ok(())
}

/// Without a GitHub App nothing is to register, and a move that keeps its URL asks nobody.
#[test]
fn no_app_or_no_new_url_leaves_nothing_to_register() -> TestResult {
    let mut s = setup()?;
    s.world.github = Ok(None);
    s.remote.github = Ok(None);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let said = github_lines(&status);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said.iter().all(
            |(outcome, detail)| *outcome == HostOutcome::Ok && detail.contains("no GitHub App")
        ),
        "{said:?}"
    );

    let mut s = setup()?;
    s.opts.public_url = Some(super::fake::OLD_URL.to_owned());
    s.world
        .dns
        .insert(OLD_HOST.to_owned(), vec![NEW_ADDRESS.to_owned()]);
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(github_lines(&s.status()?).is_empty());
    assert!(s.world.asked.is_empty());
    assert!(!s.events().contains(&"new: callback".to_owned()));
    Ok(())
}

/// The old host has the move helper's units (SPEC-470), with its path unit `enabled` or not.
fn with_move_helper(host: &mut FakeHost, enabled: bool) {
    host.says(
        &["systemctl", "show", "yulius-migrate.service"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-migrate.service\nDropInPaths=\n",
    );
    host.says(
        &["systemctl", "show", "yulius-migrate.path"],
        "LoadState=loaded\nFragmentPath=/etc/systemd/system/yulius-migrate.path\nDropInPaths=\n",
    );
    host.says(
        &["systemctl", "is-enabled", "yulius-migrate.path"],
        if enabled { "enabled\n" } else { "disabled\n" },
    );
}

/// SPEC-470 MVB-008: the move helper's units travel with a move, and its path unit is enabled
/// on the new host exactly when it was on the old one; the checklist says which.
#[test]
fn the_move_helper_travels_and_is_enabled_as_on_the_old_host() -> TestResult {
    let said = |s: &Setup| -> Result<Vec<(HostOutcome, String)>, Box<dyn std::error::Error>> {
        Ok(s.status()?
            .lines
            .into_iter()
            .filter(|l| l.name == "move helper")
            .map(|l| (l.outcome, l.detail))
            .collect())
    };
    let sent = |s: &Setup| {
        s.remote
            .sent
            .iter()
            .filter_map(|p| p.to_str())
            .filter(|p| p.contains("migrate"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    const ENABLE: &str = "systemctl enable --now yulius-migrate.path";

    // None on the old host: said in the plan, nothing sent, nothing enabled.
    let mut s = setup()?;
    assert_eq!(s.run()?, 0);
    assert_eq!(
        said(&s)?,
        [(
            HostOutcome::Ok,
            "none on the old host: the new one offers no move from Settings → Instance either"
                .to_owned()
        )]
    );
    assert!(sent(&s).is_empty() && !s.remote.host.ran(ENABLE));

    // Enabled here: carried, and enabled there once the new instance runs.
    let mut s = setup()?;
    with_move_helper(&mut s.local, true);
    assert_eq!(s.run()?, 0);
    assert_eq!(
        said(&s)?,
        [
            (
                HostOutcome::Ok,
                "its units are carried, and its path unit is enabled on the new host as it is \
                 here"
                    .to_owned()
            ),
            (
                HostOutcome::Changed,
                "yulius-migrate.path is enabled".to_owned()
            ),
        ]
    );
    assert_eq!(
        sent(&s),
        [
            "/etc/systemd/system/yulius-migrate.path",
            "/etc/systemd/system/yulius-migrate.service",
        ]
    );
    let events = s.world.events.lock().map_err(|e| e.to_string())?.clone();
    let at = |what: &str| events.iter().position(|e| e == what);
    assert!(at("new: start") < at("new: enable"), "{events:?}");
    assert!(at("new: enable") < at("new: finish"), "{events:?}");

    // Switched off here: carried, and off there.
    let mut s = setup()?;
    with_move_helper(&mut s.local, false);
    assert_eq!(s.run()?, 0);
    assert_eq!(
        said(&s)?,
        [(
            HostOutcome::Ok,
            "its units are carried; its path unit is not enabled here, so it is not enabled on \
             the new host"
                .to_owned()
        )]
    );
    assert_eq!(sent(&s).len(), 2);
    assert!(!s.remote.host.ran(ENABLE));

    // What fails there after the start is a line, never the end of the move.
    let mut s = setup()?;
    with_move_helper(&mut s.local, true);
    s.remote.host.fails(
        &["systemctl", "enable", "--now", "yulius-migrate.path"],
        "Unit yulius-migrate.path not found",
    );
    assert_eq!(s.run()?, 1);
    let lines = said(&s)?;
    assert_eq!(lines.last().map(|l| l.0), Some(HostOutcome::Failed));
    assert_eq!(s.state()?.cutover, Cutover::Moved);
    assert!(s.state()?.finished);
    Ok(())
}

/// SPEC-470 MVB-010: `status.json` gives its readers the key line while a person has to add it,
/// and the new URL once the plan decided it.
#[test]
fn the_status_names_the_key_to_add_and_the_new_url() -> TestResult {
    let mut s = setup()?;
    s.remote.signs_in = false;
    s.opts.yes = false;
    assert_eq!(s.run()?, 0);
    let status = s.status()?;
    let key = status.public_key.clone().ok_or("no key in the status")?;
    assert_eq!(
        key,
        format!("ssh-ed25519 AAAAC3FAKE yulius-migrate-{}", status.id)
    );
    assert!(line(&status, "one-time key").is_some_and(|l| l.detail.ends_with(&key)));
    assert_eq!(
        status.new_url, None,
        "the plan ended before it decided the URL"
    );
    let written = fs::read_to_string(s.opts.state_dir.join("status.json"))?;
    assert!(
        written.contains(r#""public_key":"ssh-ed25519 "#),
        "{written}"
    );

    // Added by hand on the new host: the same plan goes on and asks for no key.
    s.remote.key = true;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(status.phase, "Plan");
    assert_eq!(status.public_key, None);
    assert_eq!(status.new_url.as_deref(), Some(NEW_URL));
    let written = fs::read_to_string(s.opts.state_dir.join("status.json"))?;
    assert!(!written.contains("public_key"), "{written}");
    Ok(())
}

/// SPEC-470 MVB-011: the finished checklist travels to the new host with the Finish step, for
/// the new instance's page: the same lines, in the same order, as the old host's own.
#[test]
fn the_finished_checklist_travels_to_the_new_host() -> TestResult {
    use yulius_contracts::migrate::MoveChecklist;
    let carried = |s: &Setup| -> Result<MoveChecklist, Box<dyn std::error::Error>> {
        let (text, _, mode) = s
            .remote
            .host
            .files
            .get(Path::new(target::ARRIVED))
            .ok_or("no checklist on the new host")?;
        assert_eq!(mode & 0o7777, 0o644);
        Ok(serde_json::from_str(text)?)
    };
    let mut s = setup()?;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(status.phase, "done");
    let list = carried(&s)?;
    assert_eq!(list.lines, status.lines);
    // The lines of the Finish itself and what the old host said after it are there.
    let names: Vec<&str> = list.lines.iter().map(|l| l.name.as_str()).collect();
    for name in [
        "one-time key",
        "one-time key (old host)",
        "GitHub App",
        "remote clients",
        "old host",
    ] {
        assert!(names.contains(&name), "{name} is missing: {names:?}");
    }
    assert_eq!(names.last(), Some(&"old host"));
    assert_eq!(
        (list.version, list.id.as_str(), list.target.as_str()),
        (1, status.id.as_str(), "root@185.189.47.160")
    );
    assert_eq!(list.from.as_deref(), Some(super::fake::OLD_URL));
    assert_eq!(list.to.as_deref(), Some(NEW_URL));
    assert!(list.finished_at > 0);
    // Nothing else of the flow stays there, and the data directory got nothing from it.
    let left: Vec<&PathBuf> = s
        .remote
        .host
        .files
        .keys()
        .filter(|path| path.starts_with(target::STATE_DIR))
        .collect();
    assert_eq!(left, [Path::new(target::ARRIVED)]);
    // Brought while the key still signed in, before the step that removes it.
    let events = s.events();
    let at = |what: &str| events.iter().position(|e| e == what);
    assert!(at("new: carry") < at("new: finish"), "{events:?}");

    // A Finish that did not reach the new host leaves no checklist; the run that finishes does,
    // with its own lines.
    let mut s = setup()?;
    s.remote.refuse = vec!["finish"];
    assert_eq!(s.run()?, 1);
    assert!(carried(&s).is_err());
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    assert_eq!(carried(&s)?.lines, status.lines);

    // A move whose Finish did not reach the new host and that is then aborted leaves none:
    // the instance did not move there.
    let mut s = setup()?;
    s.remote.refuse = vec!["finish"];
    assert_eq!(s.run()?, 1);
    assert!(
        s.remote
            .host
            .files
            .contains_key(Path::new(target::CARRY_FILE)),
        "nothing was staged"
    );
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert!(
        carried(&s).is_err(),
        "an aborted move left a finished checklist"
    );

    // A new host that cannot keep it says so, and the move ends all the same.
    let mut s = setup()?;
    s.remote.host.read_only.insert(target::ARRIVED.into());
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let status = s.status()?;
    let said = line(&status, "checklist").ok_or("nothing said")?;
    assert_eq!(said.outcome, HostOutcome::Manual);
    assert!(said.detail.contains("--status"), "{}", said.detail);
    assert!(s.state()?.finished);
    Ok(())
}

/// Found by a real move asked from Settings → Instance (SPEC-470): a plan that ends with the key
/// to add never brings this build to the new host. When a person then adds the key and aborts,
/// the step that removes the key there is run by that build, which is not there: the abort
/// failed with the key left behind. It brings the build first.
#[test]
fn an_abort_after_a_plan_that_only_asked_for_the_key_removes_the_key_there() -> TestResult {
    let mut s = setup()?;
    s.remote.signs_in = false;
    s.opts.yes = false;
    assert_eq!(s.run()?, 0);
    assert!(
        !s.remote.host.is_staged(),
        "the plan ended before the build"
    );
    // The person adds the key on the new host, then thinks better of the move.
    let key = format!("ssh-ed25519 AAAAC3FAKE {}", s.remote.comment);
    s.remote.key = true;
    s.remote.host.file_as(
        "/root/.ssh/authorized_keys",
        &format!("ssh-ed25519 AAAA admin@laptop\n{key}\n"),
        0,
        0o600,
    );
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert_eq!(
        s.remote.host.text("/root/.ssh/authorized_keys"),
        Some("ssh-ed25519 AAAA admin@laptop\n"),
        "the move's key stayed on the new host"
    );
    // The build went there for that step only, and the step removed it with the flow's files.
    assert!(s.events().contains(&"new: build copied".to_owned()));
    assert!(!s.remote.host.is_staged());
    assert!(State::load(&s.opts.state_dir).is_none());
    let status = s.status()?;
    assert_eq!(status.phase, "aborted");
    assert!(
        status
            .lines
            .iter()
            .all(|l| l.outcome != HostOutcome::Failed),
        "{:?}",
        status.lines
    );
    assert!(!s.remote.key, "the key is deleted here");

    // A new host that had this build already gets nothing copied again.
    let mut s = setup()?;
    s.opts.yes = false;
    assert_eq!(s.run()?, 0);
    assert!(s.remote.host.is_staged());
    let copies = |s: &Setup| {
        s.events()
            .iter()
            .filter(|e| *e == "new: build copied")
            .count()
    };
    let before = copies(&s);
    assert_eq!(s.abort()?, 0, "{}", s.shown());
    assert_eq!(copies(&s), before);
    Ok(())
}

#[test]
fn the_estimate_counts_in_full_what_waits_for_the_freeze_behind_a_directory() -> TestResult {
    // After the first pass a directory becomes a file of half a megabyte. A warm pass removes
    // no directory, so the new host leaves the file out and every pass sends it again, the
    // frozen one too: its time is not a change that may not come again.
    let mut s = setup()?;
    s.remote.churn = vec![|root| {
        let _ = fs::remove_dir_all(root.join("sysroots/sb"));
        let _ = fs::write(root.join("sysroots/sb"), vec![7u8; 512 << 10]);
    }];
    // Two writes of the file's content, 0.6 s in all.
    s.remote.slow = Some(Duration::from_millis(300));
    assert_eq!(s.run()?, 0, "{}", s.shown());
    let shown = s.shown();
    assert!(
        shown.contains("1 wait for the freeze (a directory is in their way)"),
        "{shown}"
    );
    assert!(
        shown.contains(
            "what changes meanwhile takes about 1 s to send (1 s of it for entries that every \
             pass sends: they change all the time, lie on another filesystem, or wait for the \
             freeze behind a directory)"
        ),
        "{shown}"
    );
    assert_eq!(content(&s.remote.copy_dir)?, content(&s.opts.data_dir)?);
    Ok(())
}
