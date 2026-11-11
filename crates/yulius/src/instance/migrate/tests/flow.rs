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
use crate::host_setup::{Host as _, Out};
use crate::instance::archive::{Item, Reader};
use crate::instance::extract::{Extractor, IdMap};
use crate::instance::freeze;
use crate::instance::inventory::of_copy;
use crate::instance::migrate::remote::Remote;
use crate::instance::migrate::target::{self, Env, Part, Reply, Request};
use crate::instance::migrate::{Cutover, Move, Options, State, Status, World, profile};
use crate::instance::{MANIFEST, Manifest};

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

/// systemd on the old host, as the freeze sees it.
#[derive(Debug, Default)]
struct Systemd {
    frozen: bool,
    disabled: bool,
}

struct FakeSystemd(Arc<Mutex<Systemd>>, Events);

impl freeze::Host for FakeSystemd {
    fn systemctl(&mut self, args: &[&str]) -> Result<(i32, String), String> {
        let mut unit = self.0.lock().map_err(|e| e.to_string())?;
        let said = |text: String| Ok((0, text));
        match args[0] {
            "show" => said(format!(
                "LoadState=loaded\nActiveState=active\nFreezerState={}\n\
                 ControlGroup=/system.slice/yulius.service\n",
                if unit.frozen { "frozen" } else { "running" }
            )),
            "is-active" => Ok((3, "inactive\n".into())),
            verb => {
                match verb {
                    "freeze" => unit.frozen = true,
                    "thaw" => unit.frozen = false,
                    "disable" => unit.disabled = true,
                    "enable" => unit.disabled = false,
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

/// The new host: its fake system, and a real directory the copy lands in.
struct FakeRemote {
    host: FakeHost,
    copy_dir: PathBuf,
    events: Events,
    key: bool,
    /// The one-time key's comment (the move's id), as the real connection is given it.
    comment: String,
    signs_in: bool,
    healthy: bool,
    /// Changes the installed copy before it is counted.
    tamper: Option<fn(&Path)>,
    /// `host setup` cannot build the sandbox base there.
    no_base: bool,
    sent: Vec<PathBuf>,
}

fn step_name(request: &Request) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|v| v.get("step")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl Remote for FakeRemote {
    fn reachable(&mut self) -> Result<(), String> {
        if self.key {
            Ok(())
        } else {
            Err("Permission denied (publickey)".into())
        }
    }

    fn key(&mut self) -> Result<String, String> {
        Ok(format!("ssh-ed25519 AAAAC3FAKE {}", self.comment))
    }

    fn install_key(&mut self) -> Result<(), String> {
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
        if !self.key {
            return Err("Permission denied (publickey)".into());
        }
        if !self.host.is_staged() {
            return Err("yulius: No such file or directory".into());
        }
        note(&self.events, format!("new: {}", step_name(request)));
        let Self {
            host,
            copy_dir,
            healthy,
            no_base,
            ..
        } = self;
        let mut env = Env {
            address: Some(NEW_ADDRESS.into()),
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
        let reply = target::handle(host, &mut env, request);
        // What a started server does with the mark (SPEC-460 MIG-050).
        if let Request::Start { setup, .. } = request
            && *healthy
        {
            let mark = setup.data_dir.join(target::MOVE_PENDING);
            host.files.remove(&mark);
            let report = setup.data_dir.join(target::MOVED).display().to_string();
            host.file(&report, r#"{"queued":2,"left":1,"failed":0}"#);
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
        note(&self.events, "new: copy");
        let mut bytes = Vec::new();
        write(&mut bytes).map_err(|e| e.to_string())?;
        let installed = install(&bytes, &self.copy_dir).map_err(|e| e.to_string());
        if let Some(tamper) = self.tamper {
            tamper(&self.copy_dir);
        }
        // What the import leaves on the new host's disk.
        let data = self.host.text(target::EXPECT_FILE).and_then(|text| {
            let expect: target::Expect = serde_json::from_str(text).ok()?;
            Some(expect.data_dir.display().to_string())
        });
        if let Some(data) = data {
            self.host.file(&format!("{data}/yulius.db"), "the copy");
        }
        let (outcome, detail) = match installed {
            Ok(entries) => (HostOutcome::Changed, format!("installed {entries} entries")),
            Err(why) => (HostOutcome::Failed, why),
        };
        Ok(Reply {
            lines: vec![HostLine {
                name: "copy".into(),
                outcome,
                detail,
            }],
            ..Reply::default()
        })
    }

    fn measure(&mut self, _: u64) -> Result<Duration, String> {
        Ok(Duration::from_millis(500))
    }

    fn forget(&mut self) {
        note(&self.events, "old: key deleted");
        self.key = false;
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

/// Installs an archive into `dir` as the import does, without its checks of the host.
fn install(bytes: &[u8], dir: &Path) -> io::Result<u64> {
    let mut r = Reader::new(bytes);
    let Item::Entry(first) = r.next_item()? else {
        return Err(io::Error::other("no manifest"));
    };
    assert_eq!(first.path, MANIFEST);
    let mut json = Vec::new();
    io::Read::read_to_end(&mut r.data_reader(), &mut json)?;
    let m: Manifest = serde_json::from_slice(&json).map_err(io::Error::other)?;
    fs::create_dir_all(dir)?;
    let mut ex = Extractor::new(dir, same_ids(&m), false)?;
    while let Item::Entry(e) = r.next_item()? {
        ex.put(&e, &mut r.data_reader())?;
    }
    Ok(ex.finish()?.entries)
}

struct FakeWorld {
    systemd: Arc<Mutex<Systemd>>,
    events: Events,
    answer: Option<bool>,
    dns: BTreeMap<String, Vec<String>>,
    https: bool,
    shown: Vec<String>,
}

impl World for FakeWorld {
    fn freezer(&mut self) -> Box<dyn freeze::Host> {
        Box::new(FakeSystemd(
            Arc::clone(&self.systemd),
            Arc::clone(&self.events),
        ))
    }

    fn arm(&mut self, _: &str) {}

    fn refused(&mut self) {}

    fn frozen(&mut self, _: bool) {}

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

    fn exe(&self) -> PathBuf {
        PathBuf::from("/usr/local/bin/yulius")
    }

    fn build(&self) -> Option<String> {
        Some(build())
    }

    fn show(&mut self, text: &str) {
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
            data_dir,
            service: "yulius.service".into(),
            update_service: "yulius-update.service".into(),
            state_dir,
        },
        local,
        remote: FakeRemote {
            host: target_host(),
            copy_dir: tmp.path().join("new/data"),
            events: Arc::clone(&events),
            key: false,
            comment: String::new(),
            signs_in: true,
            healthy: true,
            tamper: None,
            no_base: false,
            sent: Vec::new(),
        },
        world: FakeWorld {
            systemd,
            events,
            answer: None,
            dns: BTreeMap::from([(NEW_HOST.to_owned(), vec![NEW_ADDRESS.to_owned()])]),
            https: true,
            shown: Vec::new(),
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
    assert_eq!(
        s.local.text(profile::CADDYFILE),
        Some(profile::moved_site(OLD_HOST, NEW_URL).as_str())
    );
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
    assert!(at("new: before_start")? < at("old: systemctl freeze")?);
    assert!(at("old: systemctl freeze")? < at("new: copy")?);
    assert!(at("new: copy")? < at("new: inventory")?);
    assert!(at("new: inventory")? < at("new: mark")?);
    assert!(at("new: mark")? < at("old: no signal thaws from here")?);
    assert!(at("old: no signal thaws from here")? < at("new: start")?);
    assert!(at("new: start")? < at("old: systemctl disable")?);
    assert!(at("old: systemctl disable")? < at("new: finish")?);
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
    assert!(!new.ran("apt-get") && !new.ran("systemctl enable") && !new.ran("install "));
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
            .any(|e| e == "new: copy" || e == "old: systemctl freeze")
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

    // The copy that was not proven is marked unfinished: the same command removes it, copies
    // again and completes.
    assert!(events.contains(&"new: discard".to_owned()));
    s.remote.tamper = None;
    assert_eq!(s.run()?, 0, "{}", s.shown());
    assert!(s.remote.host.ran("rm -rf --one-file-system -- "));
    assert_eq!(s.old_unit(), (true, true));
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
    assert!(at("new: start")? < at("new: stop")?);
    assert!(at("new: stop")? < at("old: systemctl thaw")?);
    assert!(!events.contains(&"old: systemctl disable".to_owned()));
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
        .says(&["blkid"], "0a1b2c3d-1111-2222-3333-444455556666\n");
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
    assert!(
        s.events()
            .iter()
            .all(|e| e == "new: build copied" || e == "new: facts" || e == "new: packages")
    );
    // Another move is recorded.
    let s = setup()?;
    s.state()?;
    let other = State::begin(&s.opts.state_dir, "root@elsewhere", 22);
    assert!(other.is_err_and(|why| why.contains("a move to root@185.189.47.160 is recorded")));
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
    assert!(
        s.shown()
            .contains("it still answers at https://185-189-47-160.sslip.io")
    );
    assert!(State::load(&s.opts.state_dir).is_some());

    // Stopped there: the old instance comes back with its proxy configuration.
    s.world.https = false;
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
fn a_unit_an_earlier_run_left_frozen_is_thawed_when_the_cutover_fails() -> TestResult {
    let mut s = setup()?;
    if let Ok(mut unit) = s.world.systemd.lock() {
        unit.frozen = true;
    }
    s.remote.tamper = Some(|copy| {
        let _ = fs::remove_file(copy.join("master.key"));
    });
    assert_eq!(s.run()?, 1);
    assert_eq!(s.old_unit(), (false, false), "{}", s.shown());
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
                "new: copy",
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
            .any(|e| e.starts_with("old: systemctl") || e == "new: copy")
    );
    Ok(())
}
