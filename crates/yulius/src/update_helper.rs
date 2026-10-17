//! `yulius update-helper`: the root helper that updates the instance (SPEC-002 ADR-18, §8.4).
//!
//! The service runs as the unprivileged `yulius` user and cannot replace its own binary or
//! restart itself. A root-owned systemd path unit (`yulius-update.path`) starts this helper as a
//! oneshot (`yulius-update.service`) when the server writes `<data>/update/request.json`. The
//! request can only say **build** or **install** one commit; everything else comes from the
//! root-owned unit: the repository URL, the binary, the service, the health URL.
//!
//! - **build**: fetch `main` of the fixed repository (with the short-lived read token the request
//!   carries, as an HTTP header in the environment, never in argv or on disk), verify the commit
//!   is on `main` and not older than the commit this helper installed last, check it out exactly,
//!   `cargo build --release --locked -p yulius` with `YULIUS_BUILD_COMMIT` set, and stage the
//!   binary. Touches no service.
//! - **install**: the staged build of that commit only: stop the service (agent sessions keep
//!   running), back up the binary and the database, install, start, and wait until the new
//!   server reports that commit; otherwise restore the previous binary and database and start
//!   them again.
//!
//! Every step is reported in `<state>/status.json`. The request file is untrusted: a regular
//! file (not a link) owned by the data directory's owner, at most 16 KiB, the exact schema, a
//! fresh timestamp, and an id acted on once. No value from it reaches a shell.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use clap::Args;
use yulius_contracts::update::{
    HelperAction, HelperProgress, HelperRequest, HelperState, HelperStatus, HostCheckReport,
    LOG_TAIL_LINES, MAX_REQUEST_BYTES, REQUEST_TTL_MS, VERSION, VersionInfo,
};
use yulius_domain::update::{BRANCH, is_commit, short};

use crate::wiring::update_files::{REQUEST_FILE, STATUS_FILE, write_atomic};

/// The one repository updates come from (F 2026-10-10: "always will be").
pub const DEFAULT_REPO: &str = "https://github.com/Quartio-Studio/yulius.git";
/// Longest build.
const BUILD_TIMEOUT: Duration = Duration::from_secs(2 * 3600);
/// Longest Git command.
const GIT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Longest `systemctl` command.
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// Backups kept.
const KEEP_BACKUPS: usize = 3;
/// Requests handled in one run (a request written while the helper was busy is picked up).
const MAX_REQUESTS_PER_RUN: usize = 3;
/// Longest `yulius host check` after an install (its commands are bounded to 30 s each, or 2
/// minutes for stopping the host's Docker; tasks resume once it ends).
const HOST_CHECK_TIMEOUT: Duration = Duration::from_secs(4 * 60);
/// Where the last re-check is kept (the status is replaced by the next request).
pub const HOST_CHECK_FILE: &str = "host-check.json";
/// Lines of a re-check kept, and characters of each detail.
const HOST_CHECK_LINES: usize = 40;
const HOST_CHECK_DETAIL: usize = 1000;
const HOST_CHECK_NAME: usize = 100;
const HOST_CHECK_ERROR: usize = 2000;

/// Status writes while building, at most one per this.
const STATUS_EVERY: Duration = Duration::from_secs(1);

/// Options from the root-owned unit (flags or environment); none comes from the request.
#[derive(Debug, Clone, Args)]
pub struct HelperArgs {
    /// The service's data directory: the request is `<data>/update/request.json`, the database
    /// `<data>/yulius.db`, and its owner is the only one whose request is read.
    #[arg(
        long,
        visible_alias = "data",
        env = "YULIUS_DATA",
        default_value = "/var/lib/yulius"
    )]
    pub data_dir: PathBuf,
    /// The helper's own root-owned state: checkout, build, staged binary, backups, status.
    #[arg(
        long,
        env = "YULIUS_UPDATE_STATE",
        default_value = "/var/lib/yulius-update"
    )]
    pub state_dir: PathBuf,
    /// The repository to fetch `main` from.
    #[arg(long, env = "YULIUS_UPDATE_REPO", default_value = DEFAULT_REPO)]
    pub repo: String,
    /// The installed binary to replace.
    #[arg(
        long,
        env = "YULIUS_UPDATE_BINARY",
        default_value = "/usr/local/bin/yulius"
    )]
    pub binary: PathBuf,
    /// The service to stop and start.
    #[arg(long, env = "YULIUS_UPDATE_SERVICE", default_value = "yulius.service")]
    pub service: String,
    /// The server's version endpoint on its internal listener.
    #[arg(
        long,
        env = "YULIUS_UPDATE_HEALTH",
        default_value = "http://127.0.0.1:8081/internal/version"
    )]
    pub health_url: String,
    /// Seconds the new server has to report the installed commit.
    #[arg(long, env = "YULIUS_UPDATE_HEALTH_SECS", default_value_t = 120)]
    pub health_secs: u64,
    /// Cargo (rustup's proxy, so `rust-toolchain.toml` picks the compiler).
    #[arg(long, env = "YULIUS_UPDATE_CARGO", default_value = "cargo")]
    pub cargo: PathBuf,
    /// Git.
    #[arg(long, env = "YULIUS_UPDATE_GIT", default_value = "git")]
    pub git: PathBuf,
    /// systemctl.
    #[arg(long, env = "YULIUS_UPDATE_SYSTEMCTL", default_value = "systemctl")]
    pub systemctl: PathBuf,
    /// The program whose `host check --json` re-checks the host after an install (SPEC-203
    /// HST-012); the installed binary when unset.
    #[arg(long, env = "YULIUS_UPDATE_HOST_CHECK", hide = true)]
    pub host_check: Option<PathBuf>,
}

/// `yulius update-helper`: handles the pending request, if any. Exit 0 also when a request was
/// refused or failed (that is reported in the status file); 1 only when the helper itself
/// cannot work (its state directory).
pub fn main(args: &HelperArgs) -> ExitCode {
    let helper = Helper::new(args.clone());
    match helper.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("yulius update-helper: {err}");
            ExitCode::FAILURE
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Whether `id` is a request id: 8–64 of `[A-Za-z0-9-]`.
fn valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Longest request token: GitHub's stateless installation tokens (`ghs_<APPID>_<JWT>`, since
/// 2026-10-02) are about 520 bytes and vary with their claims.
const MAX_TOKEN_BYTES: usize = 4096;

/// Whether `token` can be sent in an HTTP header: 1–[`MAX_TOKEN_BYTES`] of `[A-Za-z0-9_.-]`.
fn valid_token(token: &str) -> bool {
    (1..=MAX_TOKEN_BYTES).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Checks a request's bytes: the exact schema, protocol version, field formats and age.
///
/// # Errors
/// Why it is refused (never echoing its content).
pub fn parse_request(bytes: &[u8], now: i64) -> Result<HelperRequest, String> {
    let request: HelperRequest = serde_json::from_slice(bytes)
        .map_err(|_| "the request is not a valid request".to_owned())?;
    if request.version != VERSION {
        return Err("the request has another protocol version".into());
    }
    if !valid_id(&request.id) {
        return Err("the request id is malformed".into());
    }
    if !is_commit(&request.commit) {
        return Err("the commit is not a full commit id".into());
    }
    if request.token.as_deref().is_some_and(|t| !valid_token(t)) {
        return Err("the token is malformed".into());
    }
    if request.requested_at < now - REQUEST_TTL_MS || request.requested_at > now + 60_000 {
        return Err("the request is stale".into());
    }
    Ok(request)
}

/// Opens a file that the unprivileged owner `owner` controls: a regular file (not a link) owned
/// by `owner`, the same file when opened as when checked (no FIFO, no swapped-in link).
///
/// # Errors
/// Why it is refused; `Ok(None)` when it does not exist.
pub fn open_owned_file(path: &Path, owner: u32) -> Result<Option<std::fs::File>, String> {
    let checked = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot inspect {}: {err}", path.display())),
    };
    if !checked.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if checked.uid() != owner {
        return Err(format!(
            "{} is not owned by the service user",
            path.display()
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK).bits())
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
    if opened.dev() != checked.dev() || opened.ino() != checked.ino() || !opened.is_file() {
        return Err(format!("{} changed while it was read", path.display()));
    }
    Ok(Some(file))
}

/// Reads at most `limit` bytes of [`open_owned_file`].
///
/// # Errors
/// Why it is refused (also when larger than `limit`); `Ok(None)` when it does not exist.
pub fn read_owned_file(path: &Path, owner: u32, limit: u64) -> Result<Option<Vec<u8>>, String> {
    let Some(file) = open_owned_file(path, owner)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!("{} is too large", path.display()));
    }
    Ok(Some(bytes))
}

/// Parses `    Building [====>   ] 120/341: a, b` into (done, total).
#[must_use]
pub fn parse_progress(line: &str) -> Option<(u32, u32)> {
    let rest = &line[line.find("Building [")?..];
    let rest = &rest[rest.find("] ")? + 2..];
    let counts = rest.split(':').next()?.trim();
    let (done, total) = counts.split_once('/')?;
    let (done, total) = (done.trim().parse().ok()?, total.trim().parse().ok()?);
    (total > 0 && done <= total).then_some((done, total))
}

/// The output of a bounded command.
struct Output {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Waits for `child` up to `timeout`, killing it after; `None` on timeout.
fn wait_bounded(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run_bounded(command: &mut Command, timeout: Duration, what: &str) -> Result<Output, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {what}: {e}"))?;
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.by_ref().take(1 << 20).read_to_string(&mut text);
            }
            text
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let status = wait_bounded(&mut child, timeout)
        .map_err(|e| format!("{what} failed: {e}"))?
        .ok_or_else(|| format!("{what} did not finish within {} s", timeout.as_secs()))?;
    Ok(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// The last non-empty line of a command's error output, for a status message.
fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no output")
        .chars()
        .take(300)
        .collect()
}

/// A plain HTTP/1.0 GET of a loopback URL (`http://host:port/path`), bounded; the body.
fn http_get(url: &str, timeout: Duration) -> Result<(u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "the health URL must be http://".to_owned())?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let address = authority
        .parse::<std::net::SocketAddr>()
        .map_err(|_| "the health URL needs an IP address and port".to_owned())?;
    let mut stream = std::net::TcpStream::connect_timeout(&address, timeout)
        .map_err(|e| format!("connect: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let request = format!("GET /{path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("send: {e}"))?;
    let mut response = Vec::new();
    stream
        .take(64 * 1024)
        .read_to_end(&mut response)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| "no HTTP status".to_owned())?;
    Ok((status, body.to_owned()))
}

/// The commit a server reports on its version endpoint.
fn reported_commit(url: &str) -> Result<Option<String>, String> {
    let (status, body) = http_get(url, Duration::from_secs(3))?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    let info: VersionInfo =
        serde_json::from_str(&body).map_err(|_| "not a version answer".to_owned())?;
    Ok(info.commit)
}

/// What a refused request said about itself, so its status reaches the server.
struct Refused {
    id: String,
    commit: String,
    install: bool,
}

struct Helper {
    args: HelperArgs,
    request: Option<HelperRequest>,
    status: Mutex<Option<HelperStatus>>,
    last_write: Mutex<Option<Instant>>,
}

impl Helper {
    fn new(args: HelperArgs) -> Self {
        Self {
            args,
            request: None,
            status: Mutex::new(None),
            last_write: Mutex::new(None),
        }
    }

    fn state(&self, name: &str) -> PathBuf {
        self.args.state_dir.join(name)
    }

    fn run(mut self) -> Result<(), String> {
        let state = &self.args.state_dir;
        std::fs::create_dir_all(state)
            .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot set the mode of {}: {e}", state.display()))?;
        for _ in 0..MAX_REQUESTS_PER_RUN {
            let request = match self.read_request() {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(()),
                Err((refused, reason)) => {
                    eprintln!("yulius update-helper: request refused: {reason}");
                    if let Some(refused) = refused
                        && !self.already_handled(&refused.id)
                    {
                        self.mark_handled(&refused.id);
                        self.write_refusal(&refused, &reason);
                    }
                    return Ok(());
                }
            };
            if self.already_handled(&request.id) {
                return Ok(());
            }
            self.mark_handled(&request.id);
            eprintln!(
                "yulius update-helper: {} {} ({})",
                request.action.as_str(),
                short(&request.commit),
                request.id
            );
            self.request = Some(request.clone());
            let result = match request.action {
                HelperAction::Build => self.build(&request),
                HelperAction::Install => self.install(&request),
            };
            if let Err(err) = result {
                eprintln!("yulius update-helper: failed: {err}");
                self.report(HelperState::Failed, "failed", |s| {
                    s.error = Some(err.clone())
                });
            }
            self.request = None;
        }
        Ok(())
    }

    /// The pending request: `Ok(None)` without one; `Err((who, reason))` when refused.
    fn read_request(&self) -> Result<Option<HelperRequest>, (Option<Refused>, String)> {
        let data = &self.args.data_dir;
        let owner = std::fs::metadata(data)
            .map_err(|e| (None, format!("cannot inspect {}: {e}", data.display())))?;
        if owner.uid() == 0 {
            return Err((
                None,
                "the data directory must belong to the service user".into(),
            ));
        }
        let path = data.join("update").join(REQUEST_FILE);
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
            let refused = field("id").filter(|id| valid_id(id)).map(|id| Refused {
                id,
                commit: field("commit").filter(|c| is_commit(c)).unwrap_or_default(),
                install: field("action").as_deref() == Some("install"),
            });
            (refused, reason)
        })
    }

    fn already_handled(&self, id: &str) -> bool {
        std::fs::read_to_string(self.state("last-request")).is_ok_and(|last| last.trim() == id)
    }

    fn mark_handled(&self, id: &str) {
        if let Err(err) = write_atomic(&self.state("last-request"), id.as_bytes(), 0o600) {
            eprintln!("yulius update-helper: cannot record the request: {err}");
        }
    }

    fn write_refusal(&self, refused: &Refused, reason: &str) {
        let status = HelperStatus {
            version: VERSION,
            id: refused.id.clone(),
            action: if refused.install {
                HelperAction::Install
            } else {
                HelperAction::Build
            },
            commit: refused.commit.clone(),
            state: HelperState::Failed,
            step: "failed".into(),
            progress: None,
            error: Some(format!("The request was refused: {reason}.")),
            previous: None,
            log_tail: Vec::new(),
            host_check: None,
            updated_at: now_ms(),
        };
        self.write_status(&status);
    }

    fn write_status(&self, status: &HelperStatus) {
        match serde_json::to_vec_pretty(status) {
            Ok(bytes) => {
                if let Err(err) = write_atomic(&self.state(STATUS_FILE), &bytes, 0o644) {
                    eprintln!("yulius update-helper: cannot write the status: {err}");
                }
            }
            Err(err) => eprintln!("yulius update-helper: cannot encode the status: {err}"),
        }
        if let Ok(mut last) = self.last_write.lock() {
            *last = Some(Instant::now());
        }
    }

    /// Updates and writes the status of the current request.
    fn report(&self, state: HelperState, step: &str, change: impl FnOnce(&mut HelperStatus)) {
        let Some(request) = &self.request else {
            return;
        };
        let Ok(mut slot) = self.status.lock() else {
            return;
        };
        let status = slot.get_or_insert_with(|| HelperStatus {
            version: VERSION,
            id: request.id.clone(),
            action: request.action,
            commit: request.commit.clone(),
            state,
            step: step.to_owned(),
            progress: None,
            error: None,
            previous: None,
            log_tail: Vec::new(),
            host_check: None,
            updated_at: 0,
        });
        if status.id != request.id {
            *status = HelperStatus {
                id: request.id.clone(),
                action: request.action,
                commit: request.commit.clone(),
                progress: None,
                error: None,
                previous: None,
                log_tail: Vec::new(),
                host_check: None,
                ..status.clone()
            };
        }
        status.state = state;
        status.step = step.to_owned();
        change(status);
        status.updated_at = now_ms();
        let copy = status.clone();
        drop(slot);
        self.write_status(&copy);
    }

    fn step(&self, step: &str) {
        eprintln!("yulius update-helper: {step}");
        self.report(HelperState::Running, step, |_| {});
    }

    fn src(&self) -> PathBuf {
        self.state("src")
    }

    /// Git in the helper's checkout, with no user or system configuration and no prompts.
    fn git(&self, args: &[&str], token: Option<&str>) -> Result<Output, String> {
        let mut command = Command::new(&self.args.git);
        command
            .arg("-C")
            .arg(self.src())
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE");
        if let Some(token) = token {
            let basic =
                base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraHeader")
                .env(
                    "GIT_CONFIG_VALUE_0",
                    format!("Authorization: Basic {basic}"),
                );
        }
        run_bounded(&mut command, GIT_TIMEOUT, "git")
    }

    fn git_ok(&self, args: &[&str], token: Option<&str>) -> Result<String, String> {
        let output = self.git(args, token)?;
        if output.status.success() {
            Ok(output.stdout.trim().to_owned())
        } else {
            Err(format!(
                "git {} failed: {}",
                args[0],
                last_line(&output.stderr)
            ))
        }
    }

    /// Whether `ancestor` is reachable from `descendant` (`git merge-base --is-ancestor`).
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let output = self.git(&["merge-base", "--is-ancestor", ancestor, descendant], None)?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(format!(
                "git merge-base failed: {}",
                last_line(&output.stderr)
            )),
        }
    }

    fn has_commit(&self, commit: &str) -> bool {
        self.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")], None)
            .is_ok_and(|o| o.status.success())
    }

    fn installed_commit(&self) -> Option<String> {
        std::fs::read_to_string(self.state("installed-commit"))
            .ok()
            .map(|c| c.trim().to_owned())
            .filter(|c| is_commit(c))
    }

    fn build(&self, request: &HelperRequest) -> Result<(), String> {
        let commit = request.commit.as_str();
        self.step("fetching");
        let src = self.src();
        if !src.join(".git").is_dir() {
            std::fs::create_dir_all(&src)
                .map_err(|e| format!("cannot create the checkout: {e}"))?;
            std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            self.git_ok(&["init", "--quiet"], None)?;
        }
        self.git_ok(&["config", "remote.origin.url", &self.args.repo], None)?;
        let refspec = format!("+refs/heads/{BRANCH}:refs/remotes/origin/{BRANCH}");
        self.git_ok(
            &["fetch", "--quiet", "--no-tags", "origin", &refspec],
            request.token.as_deref(),
        )?;
        self.step("verifying");
        if !self.has_commit(commit) {
            return Err(format!("{} was not found on {BRANCH}.", short(commit)));
        }
        if !self.is_ancestor(commit, &format!("refs/remotes/origin/{BRANCH}"))? {
            return Err(format!("{} is not on {BRANCH}.", short(commit)));
        }
        if let Some(installed) = self.installed_commit() {
            if installed == commit {
                return Err(format!("{} is already installed.", short(commit)));
            }
            if self.has_commit(&installed) && self.is_ancestor(commit, &installed)? {
                return Err(format!(
                    "{} is older than the installed {}; updates never go back.",
                    short(commit),
                    short(&installed)
                ));
            }
        }
        self.git_ok(
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "--quiet",
                "--force",
                "--detach",
                commit,
            ],
            None,
        )?;
        self.git_ok(&["clean", "-ffdxq"], None)?;
        if self.git_ok(&["rev-parse", "HEAD"], None)? != commit {
            return Err("the checkout does not match the commit".into());
        }
        self.step("compiling");
        self.compile(commit)?;
        self.stage(commit)?;
        self.report(HelperState::Done, "built", |s| s.progress = None);
        Ok(())
    }

    fn compile(&self, commit: &str) -> Result<(), String> {
        let target = self.state("target");
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("cannot create {}: {e}", target.display()))?;
        let mut child = Command::new(&self.args.cargo)
            .args(["build", "--release", "--locked", "-p", "yulius"])
            .current_dir(self.src())
            .env("CARGO_TARGET_DIR", &target)
            .env("YULIUS_BUILD_COMMIT", commit)
            .env("CARGO_TERM_PROGRESS_WHEN", "always")
            .env("CARGO_TERM_PROGRESS_WIDTH", "120")
            .env("CARGO_TERM_COLOR", "never")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        let tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take();
        let reader = std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                let Some(stderr) = stderr else { return };
                let mut reader = BufReader::new(stderr);
                let mut chunk = Vec::new();
                // Progress lines end in '\r', messages in '\n'.
                loop {
                    chunk.clear();
                    match reader.read_until(b'\r', &mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let text = String::from_utf8_lossy(&chunk).to_string();
                    for line in text.split('\n') {
                        let line = line.trim_matches(['\r', ' ']);
                        if line.is_empty() {
                            continue;
                        }
                        if let Some((done, total)) = parse_progress(line) {
                            self.progress(done, total);
                        } else if let Ok(mut tail) = tail.lock() {
                            tail.push(line.chars().take(300).collect());
                            let excess = tail.len().saturating_sub(LOG_TAIL_LINES);
                            tail.drain(..excess);
                        }
                    }
                }
            });
            let status = wait_bounded(&mut child, BUILD_TIMEOUT);
            let _ = handle.join();
            status
        });
        let lines = tail.lock().map(|t| t.clone()).unwrap_or_default();
        match reader {
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(_)) => {
                self.report(HelperState::Running, "compiling", |s| {
                    s.log_tail = lines.clone()
                });
                Err(format!(
                    "The build failed: {}",
                    lines
                        .iter()
                        .rev()
                        .find(|l| l.starts_with("error"))
                        .map_or("see the log", String::as_str)
                ))
            }
            Ok(None) => Err(format!(
                "The build did not finish within {} hours.",
                BUILD_TIMEOUT.as_secs() / 3600
            )),
            Err(err) => Err(format!("The build failed: {err}")),
        }
    }

    fn progress(&self, done: u32, total: u32) {
        let due = self
            .last_write
            .lock()
            .map(|last| last.is_none_or(|at| at.elapsed() >= STATUS_EVERY))
            .unwrap_or(true);
        if due || done == total {
            self.report(HelperState::Running, "compiling", |s| {
                s.progress = Some(HelperProgress { done, total });
            });
        }
    }

    fn staged(&self) -> PathBuf {
        self.state("staged")
    }

    fn stage(&self, commit: &str) -> Result<(), String> {
        let built = self.state("target").join("release").join("yulius");
        let staged = self.staged();
        std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(staged.join("commit"));
        copy_file(&built, &staged.join("yulius"), 0o755, None)?;
        write_atomic(&staged.join("commit"), commit.as_bytes(), 0o600).map_err(|e| e.to_string())
    }

    fn systemctl(&self, args: &[&str]) -> Result<Output, String> {
        run_bounded(
            Command::new(&self.args.systemctl)
                .args(args)
                .arg(&self.args.service),
            SYSTEMCTL_TIMEOUT,
            "systemctl",
        )
    }

    fn service_active(&self) -> bool {
        self.systemctl(&["is-active", "--quiet"])
            .is_ok_and(|o| o.status.success())
    }

    fn stop_service(&self) -> Result<(), String> {
        let output = self.systemctl(&["stop"])?;
        if !output.status.success() || self.service_active() {
            return Err(format!(
                "Could not stop {}: {}",
                self.args.service,
                last_line(&output.stderr)
            ));
        }
        Ok(())
    }

    fn start_service(&self) -> Result<(), String> {
        let output = self.systemctl(&["start"])?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Could not start {}: {}",
                self.args.service,
                last_line(&output.stderr)
            ))
        }
    }

    /// Waits until the server reports `expected` (any commit when `None`) and still runs a moment
    /// later.
    fn wait_healthy(&self, expected: Option<&str>) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(self.args.health_secs);
        let mut last = String::from("no answer");
        while Instant::now() < deadline {
            if expected.is_none() {
                // The previous build is unknown (it may predate the version endpoint): it is
                // healthy once its listener answers and the service stays up.
                if http_get(&self.args.health_url, Duration::from_secs(3)).is_ok() {
                    std::thread::sleep(Duration::from_secs(2));
                    if self.service_active() {
                        return Ok(());
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            match reported_commit(&self.args.health_url) {
                Ok(reported) if reported.as_deref() == expected => {
                    std::thread::sleep(Duration::from_secs(2));
                    if self.service_active() && reported_commit(&self.args.health_url).is_ok() {
                        return Ok(());
                    }
                    last = "the server stopped right after starting".into();
                }
                Ok(reported) => {
                    last = format!(
                        "the server reports {}",
                        reported.as_deref().map_or("an unknown build", short)
                    );
                }
                Err(err) => last = err,
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!(
            "The new server did not become healthy within {} s ({last}).",
            self.args.health_secs
        ))
    }

    /// Runs the installed version's `host check --json` (bounded) and reads its report; a
    /// version without it, or one that fails to answer, is "not checked".
    fn host_check(&self, commit: &str) -> HostCheckReport {
        let program = self
            .args
            .host_check
            .clone()
            .unwrap_or_else(|| self.args.binary.clone());
        let not_checked = |why: String| HostCheckReport {
            ok: false,
            lines: Vec::new(),
            error: Some(format!(
                "The host was not re-checked ({why}); run `sudo yulius host check` on the server."
            )),
            commit: None,
            checked_at: 0,
        };
        let output = run_bounded(
            Command::new(&program)
                .args(["host", "check", "--json", "--data-dir"])
                .arg(&self.args.data_dir)
                .arg("--service")
                .arg(&self.args.service)
                .arg("--update-state")
                .arg(&self.args.state_dir),
            HOST_CHECK_TIMEOUT,
            "yulius host check",
        );
        let mut report = match output {
            Ok(output) => output
                .stdout
                .lines()
                .rev()
                .find(|l| l.trim_start().starts_with('{'))
                .and_then(|l| serde_json::from_str::<HostCheckReport>(l).ok())
                .unwrap_or_else(|| {
                    not_checked(format!(
                        "the new version gave no report: {}",
                        last_line(&output.stderr)
                    ))
                }),
            Err(err) => not_checked(err),
        };
        // The new version is untrusted here: every field is bounded, so the status stays under
        // what the server reads (`MAX_STATUS_BYTES`).
        // Control characters would serialize as six bytes each: replaced, so the bounds hold
        // in bytes too.
        let cut = |text: &mut String, max: usize| {
            *text = text
                .chars()
                .take(max)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
        };
        report.lines.truncate(HOST_CHECK_LINES);
        for line in &mut report.lines {
            cut(&mut line.name, HOST_CHECK_NAME);
            cut(&mut line.detail, HOST_CHECK_DETAIL);
        }
        if let Some(error) = report.error.as_mut() {
            cut(error, HOST_CHECK_ERROR);
        }
        report.commit = Some(commit.to_owned());
        report.checked_at = now_ms();
        report
    }

    fn data_owner(&self) -> Result<(u32, u32), String> {
        let meta = std::fs::metadata(&self.args.data_dir).map_err(|e| e.to_string())?;
        Ok((meta.uid(), meta.gid()))
    }

    /// Copies the binary and the database (with its WAL) to `<state>/backups/<id>/`.
    fn backup(&self, id: &str) -> Result<PathBuf, String> {
        let root = self.state("backups");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let dir = root.join(id);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        copy_file(&self.args.binary, &dir.join("yulius"), 0o755, None)?;
        let (owner, _) = self.data_owner()?;
        for name in ["yulius.db", "yulius.db-wal"] {
            let from = self.args.data_dir.join(name);
            match open_owned_file(&from, owner)? {
                Some(mut file) => write_new(&dir.join(name), &mut file, 0o600, None)?,
                None if name == "yulius.db-wal" => {}
                None => return Err(format!("{} does not exist", from.display())),
            }
        }
        prune_backups(&root, KEEP_BACKUPS);
        Ok(dir)
    }

    /// Puts the backed-up binary and database back (the service is stopped).
    fn restore(&self, backup: &Path) -> Result<(), String> {
        copy_file(&backup.join("yulius"), &self.args.binary, 0o755, None)?;
        let (uid, gid) = self.data_owner()?;
        for name in ["yulius.db", "yulius.db-wal", "yulius.db-shm"] {
            let path = self.args.data_dir.join(name);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(format!("cannot remove {}: {err}", path.display())),
            }
            let saved = backup.join(name);
            if saved.exists() {
                let mut file = std::fs::File::open(&saved).map_err(|e| e.to_string())?;
                write_new(&path, &mut file, 0o600, Some((uid, gid)))?;
            }
        }
        Ok(())
    }

    fn install(&self, request: &HelperRequest) -> Result<(), String> {
        let commit = request.commit.as_str();
        let staged = self.staged();
        let staged_commit = std::fs::read_to_string(staged.join("commit")).unwrap_or_default();
        if staged_commit.trim() != commit || !staged.join("yulius").is_file() {
            return Err(format!(
                "No build of {} is ready; build it first.",
                short(commit)
            ));
        }
        let previous = reported_commit(&self.args.health_url).ok().flatten();
        self.report(HelperState::Running, "stopping", |s| {
            s.previous.clone_from(&previous)
        });
        eprintln!("yulius update-helper: stopping");
        self.stop_service()?;
        self.step("backing_up");
        let backup = match self.backup(&request.id) {
            Ok(dir) => dir,
            Err(err) => {
                let restarted = self.start_service();
                return Err(format!(
                    "Could not back up ({err}); nothing was installed{}.",
                    restarted
                        .err()
                        .map_or(String::new(), |e| format!(", and {e}"))
                ));
            }
        };
        self.step("installing");
        if let Err(err) = copy_file(&staged.join("yulius"), &self.args.binary, 0o755, None) {
            let restarted = self.start_service();
            return Err(format!(
                "Could not install ({err}); the previous version is unchanged{}.",
                restarted
                    .err()
                    .map_or(String::new(), |e| format!(", but {e}"))
            ));
        }
        self.step("starting");
        let started = self.start_service();
        self.step("checking");
        let healthy = started.and_then(|()| self.wait_healthy(Some(commit)));
        match healthy {
            Ok(()) => {
                if let Err(err) =
                    write_atomic(&self.state("installed-commit"), commit.as_bytes(), 0o644)
                {
                    eprintln!("yulius update-helper: cannot record the installed commit: {err}");
                }
                // The new version knows what it needs: its `host check` re-checks the host and
                // applies only safe fixes (SPEC-203 HST-012). A regression is reported, never
                // rolled back: the server itself is healthy.
                self.step("checking_host");
                let host_check = self.host_check(commit);
                if !host_check.ok {
                    eprintln!(
                        "yulius update-helper: sandboxes' Docker after the update: {}",
                        host_check.problems().join("; ")
                    );
                }
                match serde_json::to_vec_pretty(&host_check) {
                    Ok(bytes) => {
                        if let Err(err) = write_atomic(&self.state(HOST_CHECK_FILE), &bytes, 0o644)
                        {
                            eprintln!("yulius update-helper: cannot keep the host check: {err}");
                        }
                    }
                    Err(err) => eprintln!("yulius update-helper: {err}"),
                }
                self.report(HelperState::Done, "succeeded", |s| {
                    s.host_check = Some(host_check);
                });
                eprintln!("yulius update-helper: {} installed", short(commit));
                Ok(())
            }
            Err(reason) => {
                eprintln!("yulius update-helper: {reason} Rolling back.");
                self.step("rolling_back");
                let rolled = self
                    .stop_service()
                    .and_then(|()| self.restore(&backup))
                    .and_then(|()| self.start_service())
                    .and_then(|()| self.wait_healthy(previous.as_deref()));
                match rolled {
                    Ok(()) => {
                        self.report(HelperState::RolledBack, "rolled_back", |s| {
                            s.error = Some(format!(
                                "{reason} The previous version and its database were restored."
                            ));
                        });
                        Ok(())
                    }
                    Err(err) => Err(format!(
                        "{reason} The rollback failed too: {err} Restore by hand from {}.",
                        backup.display()
                    )),
                }
            }
        }
    }
}

/// Writes a new file from `source` (failing when the path exists, also as a link) with `mode`
/// and owner.
fn write_new(
    path: &Path,
    source: &mut dyn std::io::Read,
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    if let Some((uid, gid)) = owner {
        std::os::unix::fs::fchown(&file, Some(uid), Some(gid))
            .map_err(|e| format!("cannot hand {} to the service user: {e}", path.display()))?;
    }
    std::io::copy(source, &mut file)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(|e| e.to_string())
}

/// Copies `from` over `to` through a new file beside it and a rename (atomic for running
/// programs: the old binary stays mapped until they exit).
fn copy_file(from: &Path, to: &Path, mode: u32, owner: Option<(u32, u32)>) -> Result<(), String> {
    let mut source =
        std::fs::File::open(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?;
    let tmp = to.with_file_name(format!(
        ".{}.yulius-update",
        to.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let _ = std::fs::remove_file(&tmp);
    write_new(&tmp, &mut source, mode, owner)?;
    std::fs::rename(&tmp, to).map_err(|e| format!("cannot replace {}: {e}", to.display()))
}

/// Keeps the newest `keep` backup directories.
fn prune_backups(root: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_dir()
                .then(|| (meta.modified().unwrap_or(UNIX_EPOCH), e.path()))
        })
        .collect();
    dirs.sort();
    let excess = dirs.len().saturating_sub(keep);
    for (_, dir) in dirs.into_iter().take(excess) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests;
