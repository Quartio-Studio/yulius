//! `yulius migrate-helper`: the root helper that runs a move asked from Settings → Instance
//! (SPEC-470, ADR-0305).
//!
//! The server runs as the unprivileged service user and is frozen while its data is copied, so
//! it cannot move itself. A root-owned systemd path unit (`yulius-migrate.path`) starts this
//! helper as a oneshot (`yulius-migrate.service`) when the server writes
//! `<data>/migrate/request.json`. The request can only say **plan**, **move** or **abort** and
//! name the new host, its SSH port, a public URL and data disks; everything else comes from the
//! root-owned unit. The helper then runs this binary's `instance migrate` (SPEC-460), which keeps
//! its checklist in `<state>/status.json`, and reports in `<state>/helper.json`.
//!
//! The request file is untrusted: a regular file (not a link) owned by the data directory's
//! owner, at most 16 KiB, the exact schema, a fresh timestamp, an id acted on once, every value
//! checked again. No value from it reaches a shell: each is one argument of the command.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Args;
use yulius_app::migration::rules;
use yulius_contracts::migrate::{
    HELPER_STATUS_FILE, MAX_REQUEST_BYTES, MoveAction, MoveHelperState, MoveHelperStatus,
    MoveRequest, REQUEST_AHEAD_MS, REQUEST_DIR, REQUEST_FILE, REQUEST_TTL_MS, STATUS_FILE, VERSION,
    Values,
};

use crate::update_helper::read_owned_file;
use crate::wiring::update_files::write_atomic;

/// Requests handled in one run: a plan or a move, the abort that stopped it, one more.
const MAX_REQUESTS_PER_RUN: usize = 3;
/// What the command's own refusals begin with on standard error.
const COMMAND_SAYS: &str = "yulius instance migrate: ";
/// Lines of the command's error output kept, and characters of the one reported.
const KEPT_LINES: usize = 20;
const KEPT_CHARS: usize = 500;
/// How long the command's last words are waited for once it ended: a step of its own that
/// still runs may hold its error output open for much longer.
const LAST_WORDS: Duration = Duration::from_millis(300);
/// Longest `systemctl is-active`.
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(30);

/// Options from the root-owned unit (flags or environment); none comes from the request.
#[derive(Debug, Clone, Args)]
pub struct MoveHelperArgs {
    /// The service's data directory: the request is `<data>/migrate/request.json`, and its
    /// owner is the only one whose request is read.
    #[arg(
        long,
        visible_alias = "data",
        env = "YULIUS_DATA",
        default_value = "/var/lib/yulius"
    )]
    pub data_dir: PathBuf,
    /// Where the move keeps its key, state and checklist, and this helper its report.
    #[arg(
        long,
        env = "YULIUS_MIGRATE_STATE",
        default_value = "/var/lib/yulius-migrate"
    )]
    pub state_dir: PathBuf,
    /// The server's unit.
    #[arg(long, env = "YULIUS_MIGRATE_SERVICE", default_value = "yulius.service")]
    pub service: String,
    /// The update helper's unit: nothing is asked of a move while it runs.
    #[arg(
        long,
        env = "YULIUS_MIGRATE_UPDATE_SERVICE",
        default_value = "yulius-update.service"
    )]
    pub update_service: String,
    /// The only hosts a request may name, separated by commas; any host when unset or empty.
    #[arg(long, env = "YULIUS_MIGRATE_TARGETS", value_delimiter = ',')]
    pub targets: Vec<String>,
    /// systemctl.
    #[arg(long, env = "YULIUS_MIGRATE_SYSTEMCTL", default_value = "systemctl")]
    pub systemctl: PathBuf,
    /// The program whose `instance migrate` is run; this binary when unset.
    #[arg(long, env = "YULIUS_MIGRATE_PROGRAM", hide = true)]
    pub program: Option<PathBuf>,
}

/// How often the helper looks.
#[derive(Debug, Clone, Copy)]
struct Pace {
    /// At the command.
    poll: Duration,
    /// At the request file, for an abort.
    watch: Duration,
    /// Between two writes of the report while the command runs.
    heartbeat: Duration,
}

/// `yulius migrate-helper`: handles the pending request, if any. Exit 0 also when a request was
/// refused or its command failed (that is reported in the status file); 1 only when the helper
/// itself cannot work (its state directory).
pub fn main(args: &MoveHelperArgs) -> ExitCode {
    let helper = Helper::new(args.clone());
    // A stop of the unit ends the command first (it thaws what it froze), then the helper.
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        if let Err(err) = signal_hook::flag::register(signal, Arc::clone(&helper.stop)) {
            eprintln!("yulius migrate-helper: cannot watch signals: {err}");
            return ExitCode::FAILURE;
        }
    }
    match helper.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yulius migrate-helper: {err}");
            ExitCode::FAILURE
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Checks a request's bytes: the exact schema, protocol version, id, age and field rules.
///
/// # Errors
/// Why it is refused (never echoing its content).
pub fn parse_request(bytes: &[u8], now: i64) -> Result<MoveRequest, String> {
    let request: MoveRequest = serde_json::from_slice(bytes)
        .map_err(|_| "the request is not a valid request".to_owned())?;
    if request.version != VERSION {
        return Err("the request has another protocol version".into());
    }
    if !rules::valid_id(&request.id) {
        return Err("the request id is malformed".into());
    }
    if request.requested_at < now - REQUEST_TTL_MS || request.requested_at > now + REQUEST_AHEAD_MS
    {
        return Err("the request is stale".into());
    }
    rules::check_request(&request)?;
    Ok(request)
}

/// The arguments of the command for `request`: the unit's own options, then each value of the
/// request as one argument, and the target after `--`, so nothing in it reads as an option.
#[must_use]
pub fn command_args(args: &MoveHelperArgs, request: &MoveRequest) -> Vec<String> {
    let mut argv = vec!["instance".to_owned(), "migrate".to_owned()];
    if request.action == MoveAction::Abort {
        // Never `--yes`: whether the new instance is stopped is a person's word.
        argv.push("--abort".into());
    }
    argv.extend([
        "--data-dir".to_owned(),
        args.data_dir.display().to_string(),
        "--service".to_owned(),
        args.service.clone(),
        "--update-service".to_owned(),
        args.update_service.clone(),
        "--state-dir".to_owned(),
        args.state_dir.display().to_string(),
    ]);
    if request.action == MoveAction::Abort {
        return argv;
    }
    if let Some(port) = request.ssh_port {
        argv.push(format!("--ssh-port={port}"));
    }
    if let Some(url) = &request.public_url {
        argv.push(format!("--public-url={url}"));
    }
    for disk in &request.data_disks {
        argv.push(format!("--data-disk={disk}"));
    }
    if request.action == MoveAction::Move {
        // The plan was shown and confirmed on the page; without a terminal nobody is asked.
        argv.push("--yes".into());
    }
    argv.push("--".into());
    argv.extend(request.target.clone());
    argv
}

/// A line of the command as a report may hold it: no control characters, bounded.
fn plain(line: &str) -> String {
    line.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(KEPT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// What a refused request said about itself, so its report reaches the server.
struct Refused {
    id: String,
    action: MoveAction,
}

struct Helper {
    args: MoveHelperArgs,
    pace: Pace,
    /// More environment for the command (tests).
    env: Vec<(OsString, OsString)>,
    /// Set when the helper is told to stop: the command is told, and no request follows.
    stop: Arc<AtomicBool>,
}

impl Helper {
    fn new(args: MoveHelperArgs) -> Self {
        Self {
            args,
            pace: Pace {
                poll: Duration::from_millis(100),
                watch: Duration::from_millis(500),
                heartbeat: Duration::from_millis(
                    u64::try_from(yulius_contracts::migrate::HEARTBEAT_MS).unwrap_or(5_000),
                ),
            },
            env: Vec::new(),
            stop: Arc::default(),
        }
    }

    fn state(&self, name: &str) -> PathBuf {
        self.args.state_dir.join(name)
    }

    fn run(self) -> Result<(), String> {
        let state = &self.args.state_dir;
        std::fs::create_dir_all(state)
            .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
        // As the command keeps it: its key and state are 0600, the reports are for the server.
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o711))
            .map_err(|e| format!("cannot set the mode of {}: {e}", state.display()))?;
        for _ in 0..MAX_REQUESTS_PER_RUN {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let request = match self.read_request() {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(()),
                Err((refused, reason)) => {
                    eprintln!("yulius migrate-helper: request refused: {reason}");
                    if let Some(refused) = refused
                        && !self.already_handled(&refused.id)
                    {
                        // A refusal runs nothing: it is said even when it cannot be recorded.
                        if let Err(err) = self.mark_handled(&refused.id) {
                            eprintln!("yulius migrate-helper: {err}");
                        }
                        self.write_refusal(&refused, &reason);
                    }
                    return Ok(());
                }
            };
            if self.already_handled(&request.id) {
                return Ok(());
            }
            // One request once: a request that cannot be recorded as taken could run again
            // on the next change of its file, so it does not run at all.
            self.mark_handled(&request.id)
                .map_err(|err| format!("{err}: nothing was run"))?;
            if let Err(reason) = self.allowed(&request) {
                eprintln!("yulius migrate-helper: request refused: {reason}");
                let refused = Refused {
                    id: request.id.clone(),
                    action: request.action,
                };
                self.write_refusal(&refused, &reason);
                return Ok(());
            }
            eprintln!(
                "yulius migrate-helper: {} {} ({})",
                request.action.as_str(),
                request.target.as_deref().unwrap_or("the recorded move"),
                request.id
            );
            self.run_command(&request);
        }
        Ok(())
    }

    /// The pending request: `Ok(None)` without one; `Err((who, reason))` when refused.
    fn read_request(&self) -> Result<Option<MoveRequest>, (Option<Refused>, String)> {
        let data = &self.args.data_dir;
        let owner = std::fs::metadata(data)
            .map_err(|e| (None, format!("cannot inspect {}: {e}", data.display())))?;
        if owner.uid() == 0 {
            return Err((
                None,
                "the data directory must belong to the service user".into(),
            ));
        }
        let path = data.join(REQUEST_DIR).join(REQUEST_FILE);
        let Some(bytes) = read_owned_file(&path, owner.uid(), MAX_REQUEST_BYTES)
            .map_err(|reason| (None, reason))?
        else {
            return Ok(None);
        };
        parse_request(&bytes, now_ms()).map(Some).map_err(|reason| {
            let loose = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
            let field = |name: &str| {
                loose
                    .as_ref()
                    .and_then(|v| v.get(name))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            };
            let refused = field("id")
                .filter(|id| rules::valid_id(id))
                .map(|id| Refused {
                    id,
                    action: match field("action").as_deref() {
                        Some("move") => MoveAction::Move,
                        Some("abort") => MoveAction::Abort,
                        _ => MoveAction::Plan,
                    },
                });
            (refused, reason)
        })
    }

    fn already_handled(&self, id: &str) -> bool {
        std::fs::read_to_string(self.state("last-request")).is_ok_and(|last| last.trim() == id)
    }

    fn mark_handled(&self, id: &str) -> Result<(), String> {
        let path = self.state("last-request");
        write_atomic(&path, id.as_bytes(), 0o600)
            .map_err(|err| format!("cannot record the request in {}: {err}", path.display()))
    }

    /// What the root-owned unit and the host say about an accepted request.
    fn allowed(&self, request: &MoveRequest) -> Result<(), String> {
        // Never while an update runs: it stops and starts the unit a move freezes. A oneshot
        // unit whose command runs is `activating`.
        let unit = &self.args.update_service;
        let state = self.update_state()?;
        if matches!(state.as_str(), "active" | "activating" | "reloading") {
            return Err(format!(
                "an update is running ({unit}): ask again once it has finished"
            ));
        }
        let listed: Vec<&str> = self
            .args
            .targets
            .iter()
            .map(|host| host.trim())
            .filter(|host| !host.is_empty())
            .collect();
        if let Some(target) = request.target.as_deref()
            && !listed.is_empty()
            && !listed.contains(&rules::host_part(target))
        {
            return Err(
                "this unit allows a move only to the hosts of YULIUS_MIGRATE_TARGETS, and the \
                 new host is not one of them"
                    .into(),
            );
        }
        Ok(())
    }

    /// `systemctl is-active` of the update helper's unit: its one word, whatever the exit code.
    fn update_state(&self) -> Result<String, String> {
        let cannot = |why: String| format!("cannot ask systemd whether an update runs: {why}");
        let mut child = Command::new(&self.args.systemctl)
            .args(["is-active", &self.args.update_service])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| cannot(e.to_string()))?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > SYSTEMCTL_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(cannot("systemctl did not answer".into()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(cannot(e.to_string())),
            }
        }
        let mut said = String::new();
        if let Some(mut out) = child.stdout.take() {
            use std::io::Read as _;
            let _ = out.by_ref().take(4096).read_to_string(&mut said);
        }
        Ok(said.trim().to_owned())
    }

    fn write_status(&self, status: &MoveHelperStatus) {
        match serde_json::to_vec_pretty(status) {
            Ok(bytes) => {
                if let Err(err) = write_atomic(&self.state(HELPER_STATUS_FILE), &bytes, 0o644) {
                    eprintln!("yulius migrate-helper: cannot write the status: {err}");
                }
            }
            Err(err) => eprintln!("yulius migrate-helper: cannot encode the status: {err}"),
        }
    }

    fn write_refusal(&self, refused: &Refused, reason: &str) {
        let now = now_ms();
        self.write_status(&MoveHelperStatus {
            version: VERSION,
            id: refused.id.clone(),
            action: refused.action,
            // Nothing of a request that was not accepted is repeated.
            values: Values::default(),
            state: MoveHelperState::Refused,
            exit_code: None,
            error: Some(format!("The request was refused: {reason}.")),
            started_at: now,
            updated_at: now,
            ended_at: Some(now),
        });
    }

    /// Whether a new, valid abort waits in the request file. The request that runs is the one
    /// recorded as taken, so an abort that runs does not stop itself.
    fn abort_waits(&self) -> bool {
        matches!(
            self.read_request(),
            Ok(Some(request))
                if request.action == MoveAction::Abort && !self.already_handled(&request.id)
        )
    }

    /// Runs the command for `request` and reports until it ended.
    fn run_command(&self, request: &MoveRequest) {
        let mut status = MoveHelperStatus {
            version: VERSION,
            id: request.id.clone(),
            action: request.action,
            values: request.values(),
            state: MoveHelperState::Running,
            exit_code: None,
            error: None,
            started_at: now_ms(),
            updated_at: now_ms(),
            ended_at: None,
        };
        // The checklist is the command's; one of an earlier run would read as this run's until
        // its first line.
        let _ = std::fs::remove_file(self.state(STATUS_FILE));
        self.write_status(&status);
        let program = match &self.args.program {
            Some(program) => Ok(program.clone()),
            None => std::env::current_exe(),
        };
        let spawned = program.and_then(|program| {
            Command::new(program)
                .args(command_args(&self.args, request))
                .envs(self.env.iter().map(|(name, value)| (name, value)))
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::piped())
                // Its own group: a stop reaches the command, which decides what it means.
                .process_group(0)
                .spawn()
        });
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                eprintln!("yulius migrate-helper: cannot run the command: {err}");
                status.state = MoveHelperState::Ended;
                status.error = Some(format!("the command could not be started: {err}"));
                status.updated_at = now_ms();
                status.ended_at = Some(status.updated_at);
                return self.write_status(&status);
            }
        };
        // The command's error output goes on to the journal; its last lines are kept. Not
        // joined: a step that outlives the command may hold the pipe open.
        let words: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let drained = Arc::new(AtomicBool::new(false));
        if let Some(err) = child.stderr.take() {
            let (words, drained) = (Arc::clone(&words), Arc::clone(&drained));
            std::thread::spawn(move || {
                let mut reader = BufReader::new(err);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let text = String::from_utf8_lossy(&line);
                    let _ = write!(std::io::stderr(), "{text}");
                    if let Ok(mut kept) = words.lock() {
                        if kept.len() == KEPT_LINES {
                            kept.pop_front();
                        }
                        kept.push_back(plain(&text));
                    }
                }
                drained.store(true, Ordering::SeqCst);
            });
        } else {
            drained.store(true, Ordering::SeqCst);
        }
        let mut group = i32::try_from(child.id())
            .ok()
            .map(nix::unistd::Pid::from_raw);
        let mut told = false;
        let (mut watched, mut written) = (Instant::now(), Instant::now());
        let ended = loop {
            match child.try_wait() {
                Ok(Some(ended)) => break Some(ended),
                Ok(None) => {}
                Err(err) => {
                    eprintln!("yulius migrate-helper: cannot wait for the command: {err}");
                    break None;
                }
            }
            if !told && watched.elapsed() >= self.pace.watch {
                watched = Instant::now();
                if self.abort_waits() {
                    eprintln!("yulius migrate-helper: an abort arrived: stopping the command");
                    told = true;
                }
            }
            if !told && self.stop.load(Ordering::SeqCst) {
                eprintln!("yulius migrate-helper: stopped: stopping the command");
                told = true;
            }
            if told && let Some(group) = group.take() {
                // Once: the command thaws what it froze and ends, or, once the new instance's
                // start is ordered, goes on to its own end.
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGTERM);
            }
            if written.elapsed() >= self.pace.heartbeat {
                written = Instant::now();
                status.updated_at = now_ms();
                self.write_status(&status);
            }
            std::thread::sleep(self.pace.poll);
        };
        let code = ended.and_then(|ended| ended.code());
        // Its own refusal, before any checklist, is the last thing it said.
        if code == Some(2) {
            let waited = Instant::now();
            while !drained.load(Ordering::SeqCst) && waited.elapsed() < LAST_WORDS {
                std::thread::sleep(Duration::from_millis(10));
            }
            status.error = words.lock().ok().and_then(|kept| {
                kept.iter()
                    .rev()
                    .find_map(|line| line.strip_prefix(COMMAND_SAYS).map(str::to_owned))
            });
        }
        eprintln!(
            "yulius migrate-helper: the command ended ({})",
            code.map_or_else(|| "by a signal".to_owned(), |code| format!("exit {code}"))
        );
        status.state = MoveHelperState::Ended;
        status.exit_code = code;
        status.updated_at = now_ms();
        status.ended_at = Some(status.updated_at);
        self.write_status(&status);
    }
}

#[cfg(test)]
mod tests;
