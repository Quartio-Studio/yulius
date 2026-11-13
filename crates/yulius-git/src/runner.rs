//! Bounded, argv-only execution of the Git executable.
//!
//! Every command: explicit argv (no shell), cleared environment plus an allowlist,
//! `GIT_TERMINAL_PROMPT=0`, fixed `-c` hardening options that never come from input,
//! bounded stdout/stderr capture and a deadline after which the whole process group is
//! killed and reaped. A command may also be bounded by what it writes to disk (a size watch).

use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::Git;
use crate::error::GitError;
use crate::redact::redact;

/// Bytes of standard error retained for diagnostics; the rest is drained and discarded.
const STDERR_LIMIT: usize = 64 * 1024;
/// Bytes of standard error copied into [`GitError::Failed`].
const STDERR_IN_ERROR: usize = 4 * 1024;
/// How long to wait for output pipes to close after the process exits.
const PIPE_GRACE: Duration = Duration::from_secs(2);
/// Upper bound for the exit polling interval.
const MAX_POLL: Duration = Duration::from_millis(20);
/// How often a size watch measures its path: walking a folder costs system calls per file.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);
/// Attempts of a fetch that met a worktree being added beside it (SPEC-368, ADR-0249).
const FETCH_ATTEMPTS: u32 = 5;
/// Wait before the first retry of such a fetch; it doubles before each later one (1.5 s in all).
const FETCH_RETRY_BACKOFF: Duration = Duration::from_millis(100);

/// Variables inherited from the parent environment; everything else is cleared.
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SYSTEMROOT",
    "XDG_CONFIG_HOME",
];

/// Hardening configuration applied to every command. Constant; never derived from input.
/// `protocol.ext.allow=never` blocks `ext::` transports that run arbitrary commands,
/// `core.fsmonitor=false` stops repository configuration from launching a monitor daemon, and
/// `core.hooksPath=/dev/null` keeps repository hooks from running: the sandbox repository's Git
/// directory is writable by its agents, and these commands run outside containment (#54). The
/// agents' own Git, inside containment, still runs hooks, so the pre-push guard applies to them.
///
/// The rest keeps Git from starting a configured word with arguments (ADR-0322): a confined
/// Git may execute the program loader, which runs any file it is given. The server uses
/// neither `ssh` nor `git` transports (their commands get the address's parts), signs nothing
/// and shows no signatures (the signing program gets files), and lists an alternate's refs
/// itself.
const FIXED_CONFIG: &[&str] = &[
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "protocol.ssh.allow=never",
    "-c",
    "protocol.git.allow=never",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.alternateRefsCommand=",
    "-c",
    "commit.gpgSign=false",
    "-c",
    "tag.gpgSign=false",
    "-c",
    "log.showSignature=false",
];

/// The commands that can make a diff: each gets [`NO_DIFF_PROGRAMS`], whatever its call site
/// wrote. An external diff and a text conversion are started with the file's path.
const DIFF_COMMANDS: &[&str] = &[
    "diff",
    "log",
    "show",
    "diff-tree",
    "diff-index",
    "diff-files",
];
const NO_DIFF_PROGRAMS: &[&str] = &["--no-ext-diff", "--no-textconv"];

/// Configuration of a command that transfers between two repositories of this machine
/// ([`Invocation::local`]), where a confined Git may start the shell: only the `file` transport
/// (a rewritten address cannot turn it into `ssh`, whose command the configuration names), and
/// no command for listing an alternate's refs.
const LOCAL_CONFIG: &[&str] = &[
    "-c",
    "protocol.allow=never",
    "-c",
    "protocol.file.allow=always",
    "-c",
    "protocol.http.allow=never",
    "-c",
    "protocol.https.allow=never",
];

/// Captured result of a finished command.
pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) status: ExitStatus,
}

impl Output {
    pub(crate) fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub(crate) fn stdout_trimmed(&self) -> String {
        self.stdout_text().trim_end_matches(['\n', '\r']).to_owned()
    }
}

/// One command invocation.
pub(crate) struct Invocation<'a> {
    pub(crate) label: &'static str,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: Vec<(&'static str, String)>,
    pub(crate) timeout: Duration,
    /// Paths (folders or files) the command writes and the most bytes they may hold together.
    pub(crate) watch: Option<(Vec<&'a Path>, u64)>,
    /// What the command works on besides its folder (a clone's destination, a bundle's file,
    /// another repository): a confined Git reaches these too (ADR-0322).
    pub(crate) touches: Vec<std::path::PathBuf>,
    /// The command transfers to or from another repository of this machine: see
    /// [`Invocation::local`].
    pub(crate) local: bool,
}

impl<'a> Invocation<'a> {
    pub(crate) fn new(git: &Git, label: &'static str, cwd: Option<&'a Path>) -> Self {
        Self {
            label,
            cwd,
            args: Vec::new(),
            env: Vec::new(),
            timeout: git.timeout,
            watch: None,
            touches: Vec::new(),
            local: false,
        }
    }

    /// The command transfers to or from the repository at `path` on this machine (a fetch, a
    /// push, a clone or a listing by path). Git starts the other side through the system's
    /// shell, so a confined Git may start that shell here and nowhere else (ADR-0322). What the
    /// shell could be made to run by the configuration of a repository an agent writes is
    /// switched off for the command ([`LOCAL_CONFIG`]), and it gets none of the adapter's own
    /// configuration, which is where a credential travels.
    pub(crate) fn local(mut self, path: impl AsRef<Path>) -> Self {
        self.local = true;
        self.touching(path)
    }

    /// [`Self::local`] when `url` is a path of this machine, else unchanged.
    pub(crate) fn local_if_path(self, url: &str) -> Self {
        if Path::new(url).is_absolute() {
            self.local(url)
        } else {
            self
        }
    }

    /// Names a path outside the command's folder that it reads or writes.
    pub(crate) fn touching(mut self, path: impl AsRef<Path>) -> Self {
        self.touches.push(path.as_ref().to_path_buf());
        self
    }

    pub(crate) fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub(crate) fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub(crate) fn env(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.env.push((key, value.into()));
        self
    }

    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Bounds the command by what `paths` hold together: measured every [`WATCH_INTERVAL`] while
    /// it runs and once when it ends; past `limit` bytes the process group is killed and reaped,
    /// and the run fails with [`GitError::TooLarge`].
    pub(crate) fn watch_size(mut self, paths: &[&'a Path], limit: u64) -> Self {
        self.watch = Some((paths.to_vec(), limit));
        self
    }
}

enum Stream {
    Stdout(std::io::Result<Vec<u8>>),
    Stderr(std::io::Result<Vec<u8>>),
}

/// Changes to a repository's worktree registrations, and listings of them, run one at a time in
/// this process: a `git worktree prune` that runs while an add has created `.git/worktrees/<name>`
/// but not yet written it removes the directory, and the add fails ("failed to read
/// .git/worktrees/<name>/commondir"; two CI lanes creating checkouts at once, SPEC-021); a
/// `git worktree list` at that moment dies reading the half-written files (#666, SPEC-289).
static WORKTREE_CHANGES: Mutex<()> = Mutex::new(());

/// Holds [`WORKTREE_CHANGES`] for a registration change made without Git (a worktree's record
/// removed through handles, SPEC-401).
pub(crate) fn worktree_changes() -> std::sync::MutexGuard<'static, ()> {
    WORKTREE_CHANGES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Whether `args` add, move, remove, prune or list worktrees.
fn touches_worktree_registrations(args: &[OsString]) -> bool {
    args.first()
        .is_some_and(|a| a.as_os_str() == OsStr::new("worktree"))
        && args.get(1).is_some_and(|a| {
            ["add", "move", "remove", "prune", "list"]
                .iter()
                .any(|c| a.as_os_str() == OsStr::new(c))
        })
}

/// The Git subcommand `args` run, past leading `-c <name>=<value>` pairs (a CD mirror's fetch sets
/// its fsck options so).
fn subcommand(args: &[OsString]) -> Option<&OsStr> {
    subcommand_rest(args)
        .first()
        .map(OsString::as_os_str)
        .filter(|a| *a != OsStr::new("-c"))
}

/// `args` from the subcommand's name on (past leading `-c <name>=<value>` pairs).
fn subcommand_rest(args: &[OsString]) -> &[OsString] {
    let mut rest = args;
    while let [flag, _, tail @ ..] = rest
        && flag.as_os_str() == OsStr::new("-c")
    {
        rest = tail;
    }
    rest
}

/// Whether Git's `stderr` names a linked worktree's `HEAD` as a bad object: `git worktree add`
/// writes the new registration's `HEAD` as the null id before it sets the commit, and a fetch's
/// connectivity check, which reads every worktree's `HEAD`, dies meanwhile with "fatal: bad object
/// worktrees/<name>/HEAD" (#995, SPEC-368).
fn names_a_half_added_worktree(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).lines().any(|line| {
        line.split_once("bad object worktrees/")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .and_then(|path| path.strip_suffix("/HEAD"))
            .is_some_and(|name| !name.is_empty() && !name.contains('/'))
    })
}

/// How a transfer with a clone's remote is made ([`Git::remote_route`]).
pub(crate) enum Route {
    /// With this folder of the machine, named by its path.
    Folder(std::path::PathBuf),
    /// Through a courier, with the remote's address.
    Courier(crate::courier::Courier, String),
    /// By the remote's name, in the repository itself.
    Named,
}

impl Git {
    /// The folder a confined transfer with `repo`'s `remote` goes to, when that remote is a
    /// folder of this machine Yulius may take from the clone's configuration
    /// ([`crate::Confinement::local_origin`]): the command then names the folder itself, not the
    /// remote, whose configuration (a program for the other side) an agent writes, and is a
    /// local transfer ([`Invocation::local`]). `None`: the remote is named as it is, and a
    /// confined Git starts no shell for it.
    pub(crate) fn local_remote(&self, repo: &Path, remote: &str) -> Option<std::path::PathBuf> {
        let confinement = self.confinement.as_ref()?;
        if Path::new(remote).is_absolute() {
            // No remote's name but a folder the caller names itself.
            return Some(std::path::PathBuf::from(remote));
        }
        confinement.local_origin(repo, &self.remote_url(repo, remote)?)
    }

    /// How a confined transfer with `repo`'s `remote` is made: with a folder of this machine
    /// ([`Self::local_remote`]), through a courier (the remote is elsewhere and `repo` is a
    /// sandbox's, [`crate::courier`]), or by the remote's name as it is.
    pub(crate) fn remote_route(&self, repo: &Path, remote: &str) -> Result<Route, GitError> {
        if self.confinement.is_none() {
            // Not confined (macOS, this crate's tests): as Git does it, nothing is asked first.
            return Ok(Route::Named);
        }
        if let Some(folder) = self.local_remote(repo, remote) {
            return Ok(Route::Folder(folder));
        }
        // Only an address on the web goes through a courier. A folder that is not the sandbox's
        // own origin (another sandbox's, the host's) is the clone's word alone: it is not
        // reached, by the remote's name as before.
        let Some(url) = self
            .remote_url(repo, remote)
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        else {
            return Ok(Route::Named);
        };
        Ok(match self.courier(repo)? {
            Some(courier) => Route::Courier(courier, url),
            None => Route::Named,
        })
    }

    /// Runs and requires exit status 0.
    pub(crate) fn run_ok(&self, invocation: Invocation<'_>) -> Result<Output, GitError> {
        let label = invocation.label;
        let output = self.run(invocation)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(failure(label, &output))
        }
    }

    /// Runs and returns the output whatever the exit status. A fetch that failed on a worktree
    /// being added beside it runs again, up to [`FETCH_ATTEMPTS`] in all, after a doubling backoff
    /// and once this process's own registration change has ended (SPEC-368, ADR-0249); the last
    /// attempt's output is returned as it is.
    pub(crate) fn run(&self, invocation: Invocation<'_>) -> Result<Output, GitError> {
        let fetch = subcommand(&invocation.args) == Some(OsStr::new("fetch"));
        let mut backoff = FETCH_RETRY_BACKOFF;
        for _ in 1..FETCH_ATTEMPTS {
            let output = self.run_once(&invocation)?;
            if !fetch || output.status.success() || !names_a_half_added_worktree(&output.stderr) {
                return Ok(output);
            }
            thread::sleep(backoff);
            backoff *= 2;
            drop(
                WORKTREE_CHANGES
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
        self.run_once(&invocation)
    }

    /// The process for `invocation`: Git with the constant options, the cleared environment and
    /// the invocation's own; confined, the launcher with what the command may reach
    /// (ADR-0322).
    fn command(&self, invocation: &Invocation<'_>) -> Command {
        let mut command = match &self.confinement {
            // Through the launcher, which confines itself and then becomes Git (ADR-0322).
            Some(confinement) => {
                let mut command = Command::new(&confinement.launcher);
                command.arg("git-confined");
                let mut reach: Vec<std::path::PathBuf> = invocation
                    .cwd
                    .into_iter()
                    .chain(invocation.touches.iter().map(std::path::PathBuf::as_path))
                    .flat_map(|path| confinement.reach(path))
                    .chain(confinement.shared.iter().cloned())
                    .collect();
                reach.sort();
                reach.dedup();
                for path in reach {
                    command.arg("--rw").arg(path);
                }
                if invocation.local || confinement.shell {
                    command.arg("--shell");
                }
                command.arg("--").arg(&self.exe);
                command
            }
            None => Command::new(&self.exe),
        };
        command.args(FIXED_CONFIG);
        if invocation.local {
            command.args(LOCAL_CONFIG);
        }
        // Past the command's own `-c` pairs and its name: no program for a diff.
        let named = invocation.args.len() - subcommand_rest(&invocation.args).len();
        let diffs = subcommand(&invocation.args)
            .is_some_and(|name| DIFF_COMMANDS.iter().any(|diff| name == OsStr::new(diff)));
        for (at, arg) in invocation.args.iter().enumerate() {
            if diffs && NO_DIFF_PROGRAMS.iter().any(|flag| arg == OsStr::new(flag)) {
                continue;
            }
            command.arg(arg);
            if diffs && at == named {
                command.args(NO_DIFF_PROGRAMS);
            }
        }
        command
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in INHERITED_ENV {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        // The adapter's own configuration is where a token travels. A local transfer needs
        // none and may start the shell, and a command in a sandbox's folder reads that
        // folder's configuration, which its agents write and which shapes a connection
        // (ADR-0323): neither gets it. What needs the token there goes through a courier.
        let in_a_sandbox = match (&self.confinement, invocation.cwd) {
            (Some(confinement), Some(cwd)) => confinement.sandbox_of_path(cwd).is_some(),
            _ => false,
        };
        for (key, value) in &self.env {
            if (invocation.local || in_a_sandbox) && key.starts_with("GIT_CONFIG_") {
                continue;
            }
            command.env(key, value);
        }
        command
            // No editor is started (`:` is Git's word for none): it would get a file's path.
            .env("GIT_EDITOR", ":")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("LC_ALL", "C");
        for (key, value) in &invocation.env {
            command.env(key, value);
        }
        if let Some(cwd) = invocation.cwd {
            command.current_dir(cwd);
        }
        command
    }

    fn run_once(&self, invocation: &Invocation<'_>) -> Result<Output, GitError> {
        let _changes = touches_worktree_registrations(&invocation.args).then(|| {
            WORKTREE_CHANGES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        });
        let mut command = self.command(invocation);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(GitError::Spawn)?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            kill_tree(&mut child);
            return Err(GitError::Io {
                context: "capturing git output",
                source: std::io::Error::other("missing pipe"),
            });
        };
        let overflow = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        spawn_readers(stdout, stderr, self.max_output_bytes, &overflow, &tx);
        drop(tx);

        let deadline = Instant::now() + invocation.timeout;
        let mut poll = Duration::from_millis(1);
        let mut watched = Instant::now();
        let too_large = |limit| GitError::TooLarge {
            command: invocation.label,
            limit,
        };
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(source) => {
                    kill_tree(&mut child);
                    return Err(GitError::Io {
                        context: "waiting for git",
                        source,
                    });
                }
            }
            if overflow.load(Ordering::SeqCst) {
                kill_tree(&mut child);
                return Err(GitError::OutputTooLarge {
                    command: invocation.label,
                    limit: self.max_output_bytes,
                });
            }
            if let Some((paths, limit)) = &invocation.watch
                && watched.elapsed() >= WATCH_INTERVAL
            {
                watched = Instant::now();
                if size_of_all(paths) > *limit {
                    kill_tree(&mut child);
                    return Err(too_large(*limit));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                kill_tree(&mut child);
                return Err(GitError::Timeout {
                    command: invocation.label,
                    after: invocation.timeout,
                });
            }
            thread::sleep(poll.min(deadline - now));
            poll = (poll * 2).min(MAX_POLL);
        };

        let mut stdout = None;
        let mut stderr = None;
        let grace_deadline = Instant::now() + PIPE_GRACE;
        while stdout.is_none() || stderr.is_none() {
            let wait = grace_deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Stream::Stdout(result)) => stdout = Some(result),
                Ok(Stream::Stderr(result)) => stderr = Some(result),
                Err(_) => {
                    // A descendant escaped and holds a pipe open: kill the group again.
                    #[cfg(unix)]
                    kill_group(child.id());
                    return Err(GitError::Io {
                        context: "collecting git output",
                        source: std::io::Error::new(
                            ErrorKind::TimedOut,
                            "output pipe stayed open after git exited",
                        ),
                    });
                }
            }
        }
        if overflow.load(Ordering::SeqCst) {
            return Err(GitError::OutputTooLarge {
                command: invocation.label,
                limit: self.max_output_bytes,
            });
        }
        // A command that ended between two measures is measured once more.
        if let Some((paths, limit)) = &invocation.watch
            && size_of_all(paths) > *limit
        {
            return Err(too_large(*limit));
        }
        let io_error = |source| GitError::Io {
            context: "reading git output",
            source,
        };
        let stdout = stdout.unwrap_or_else(|| Ok(Vec::new())).map_err(io_error)?;
        let stderr = stderr.unwrap_or_else(|| Ok(Vec::new())).map_err(io_error)?;
        Ok(Output {
            stdout,
            stderr,
            status,
        })
    }
}

fn spawn_readers(
    stdout: ChildStdout,
    stderr: ChildStderr,
    limit: usize,
    overflow: &Arc<AtomicBool>,
    tx: &mpsc::Sender<Stream>,
) {
    let out_tx = tx.clone();
    let out_overflow = Arc::clone(overflow);
    thread::spawn(move || {
        let result = read_bounded(stdout, limit, &out_overflow);
        let _ = out_tx.send(Stream::Stdout(result));
    });
    let err_tx = tx.clone();
    thread::spawn(move || {
        let result = read_capped(stderr, STDERR_LIMIT);
        let _ = err_tx.send(Stream::Stderr(result));
    });
}

/// Reads until EOF; on exceeding `limit` flags overflow and stops (closing the pipe).
fn read_bounded(
    mut pipe: impl Read,
    limit: usize,
    overflow: &AtomicBool,
) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(buffer);
        }
        if buffer.len().saturating_add(read) > limit {
            overflow.store(true, Ordering::SeqCst);
            return Ok(buffer);
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// Keeps the first `cap` bytes and drains the rest so the child never blocks on stderr.
fn read_capped(mut pipe: impl Read, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(buffer);
        }
        let room = cap.saturating_sub(buffer.len());
        buffer.extend_from_slice(&chunk[..read.min(room)]);
    }
}

/// [`size_on_disk`] of every path, added.
fn size_of_all(paths: &[&Path]) -> u64 {
    paths
        .iter()
        .map(|path| size_on_disk(path))
        .fold(0, u64::saturating_add)
}

/// Bytes the files at `path` take (a file, or a folder walked without following links); entries
/// that vanish while it is walked (Git renames its temporary packs) count as nothing, and folders'
/// own blocks are left out (a repository's 256 object folders take about 1 MB).
fn size_on_disk(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return taken(&meta);
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size_on_disk(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |m| taken(&m)),
            Err(_) => 0,
        })
        .fold(0, u64::saturating_add)
}

/// A file's length, or the blocks it takes when more (a loose ref of 41 bytes takes a 4 KiB
/// block: 100 000 of them take 400 MB).
fn taken(meta: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.len().max(meta.blocks().saturating_mul(512))
    }
    #[cfg(not(unix))]
    {
        meta.len()
    }
}

/// Kills the child's process group (Unix) and the child itself, then reaps it.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    if let Ok(raw) = i32::try_from(pid) {
        let _ = killpg(Pid::from_raw(raw), Signal::SIGKILL);
    }
}

/// Builds a [`GitError::Failed`] with redacted, truncated stderr.
pub(crate) fn failure(label: &'static str, output: &Output) -> GitError {
    let raw = String::from_utf8_lossy(&output.stderr);
    let mut text = redact(raw.trim());
    if text.len() > STDERR_IN_ERROR {
        let mut end = STDERR_IN_ERROR;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    GitError::Failed {
        command: label,
        code: output.status.code(),
        stderr: text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confined(data: &Path) -> Git {
        Git::new("git")
            .with_env("GIT_CONFIG_COUNT", "1")
            .with_env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
            .with_env("GIT_CONFIG_VALUE_0", "AUTHORIZATION: basic dG9rZW4=")
            .with_env("OTHER", "kept")
            .confined(std::sync::Arc::new(crate::Confinement {
                launcher: "/opt/yulius".into(),
                data: data.to_path_buf(),
                data_depth: 0,
                shared: Vec::new(),
                shell: false,
            }))
    }

    fn words(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn sets(command: &Command, name: &str) -> bool {
        command
            .get_envs()
            .any(|(key, value)| key == OsStr::new(name) && value.is_some())
    }

    /// ADR-0322: a confined command is the launcher, the folders it may touch (its sandbox's
    /// and what it names besides), then Git with the constant options; no shell, and the
    /// adapter's own configuration (a token's header) in its environment.
    #[test]
    fn a_confined_command_is_the_launcher_with_its_sandboxes_folders() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let data = dir.path().canonicalize()?;
        let repo = data.join("sandboxes/s1/repo");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(data.join("out"))?;
        let git = confined(&data);
        let invocation = Invocation::new(&git, "status", Some(&repo))
            .args(["status", "--porcelain"])
            .touching(data.join("out/x.bundle"));
        let command = git.command(&invocation);
        assert_eq!(command.get_program(), OsStr::new("/opt/yulius"));
        let mut expected = vec!["git-confined".to_owned()];
        for folder in [data.join("out"), data.join("sandboxes/s1")] {
            expected.extend(["--rw".to_owned(), folder.to_string_lossy().into_owned()]);
        }
        expected.extend(["--", "git"].map(str::to_owned));
        expected.extend(FIXED_CONFIG.iter().map(|word| (*word).to_owned()));
        expected.extend(["status", "--porcelain"].map(str::to_owned));
        assert_eq!(words(&command), expected);
        // In a sandbox's folder the clone's configuration shapes a connection: the adapter's
        // own (a token's header) is not given to a command there (ADR-0323). Elsewhere, and
        // for a command in no folder (a clone), it is.
        assert!(!sets(&command, "GIT_CONFIG_VALUE_0") && !sets(&command, "GIT_CONFIG_COUNT"));
        assert!(sets(&command, "OTHER"));
        let own = data.join("git-couriers/x.git");
        std::fs::create_dir_all(&own)?;
        for cwd in [Some(own.as_path()), None] {
            let command = git.command(&Invocation::new(&git, "fetch", cwd).arg("fetch"));
            assert!(sets(&command, "GIT_CONFIG_VALUE_0"), "{cwd:?}");
        }

        // Unconfined (this crate's tests, macOS): Git itself.
        let plain = Git::new("git");
        let command = plain.command(&Invocation::new(&plain, "status", Some(&repo)).arg("status"));
        assert_eq!(command.get_program(), OsStr::new("git"));
        assert!(!words(&command).iter().any(|word| word == "git-confined"));
        Ok(())
    }

    /// The loader, which a confined Git must be able to execute, runs any file it can read:
    /// no configured word is started with an argument an agent chooses. Every command that can
    /// make a diff gets `--no-ext-diff --no-textconv` (an external diff and a text conversion
    /// are started with the file's path), whatever its call site wrote; no command signs,
    /// shows signatures, edits a message or uses `ssh` or `git` transports.
    #[test]
    fn nothing_is_started_with_an_argument_of_the_clones() {
        let git = Git::new("git");
        let said = |args: &[&str]| {
            words(&git.command(&Invocation::new(&git, "x", None).args(args.iter().copied())))
        };
        for family in [
            "diff",
            "log",
            "show",
            "diff-tree",
            "diff-index",
            "diff-files",
        ] {
            let words = said(&[family, "--stat", "HEAD"]);
            let at = words.iter().position(|word| word == family).unwrap_or(0);
            assert_eq!(
                words[at..],
                [family, "--no-ext-diff", "--no-textconv", "--stat", "HEAD"].map(str::to_owned),
                "{family}"
            );
        }
        // Past a command's own `-c` pairs; not twice; not for a command that makes no diff.
        let fsck = said(&["-c", "fetch.fsckObjects=true", "log", "--no-textconv", "-1"]);
        let at = fsck.iter().position(|word| word == "log").unwrap_or(0);
        assert_eq!(
            fsck[at..],
            ["log", "--no-ext-diff", "--no-textconv", "-1"].map(str::to_owned)
        );
        for other in ["status", "merge", "fetch", "rev-parse"] {
            let words = said(&[other]);
            assert!(
                !words.iter().any(|word| word == "--no-textconv"),
                "{other}: {words:?}"
            );
        }
        let constant = said(&["status"]);
        for pinned in [
            "protocol.ext.allow=never",
            "protocol.ssh.allow=never",
            "protocol.git.allow=never",
            "core.alternateRefsCommand=",
            "commit.gpgSign=false",
            "tag.gpgSign=false",
            "log.showSignature=false",
            "core.fsmonitor=false",
            "core.hooksPath=/dev/null",
        ] {
            assert!(
                constant.iter().any(|word| word == pinned),
                "{pinned}: {constant:?}"
            );
        }
        let command = git.command(&Invocation::new(&git, "x", None).arg("commit"));
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == OsStr::new("GIT_EDITOR")
                    && value == Some(OsStr::new(":"))),
            "no editor is started"
        );
    }

    /// A transfer with a folder Yulius names: the other folder is reached, the shell is
    /// allowed, the transports are pinned to `file` with no alternate-refs command, and none
    /// of the adapter's configuration (where a token travels) is in the environment.
    #[test]
    fn a_local_transfer_gets_the_shell_and_no_credential() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let data = dir.path().canonicalize()?;
        let repo = data.join("sandboxes/s1/repo");
        let origin = data.join("playgrounds/s1.git");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(&origin)?;
        let git = confined(&data);
        let command = git.command(
            &Invocation::new(&git, "push", Some(&repo))
                .args(["push", "--"])
                .arg(&origin)
                .local(&origin),
        );
        let said = words(&command);
        let split = said.iter().position(|word| word == "--").unwrap_or(0);
        let (launcher, git_words) = said.split_at(split);
        assert!(launcher.iter().any(|word| word == "--shell"), "{said:?}");
        for folder in [&origin, &data.join("sandboxes/s1")] {
            assert!(
                launcher.contains(&folder.to_string_lossy().into_owned()),
                "{said:?}"
            );
        }
        for pinned in [
            "protocol.allow=never",
            "protocol.file.allow=always",
            "protocol.ssh.allow=never",
            "protocol.https.allow=never",
            "core.alternateRefsCommand=",
        ] {
            assert!(
                git_words.iter().any(|word| word == pinned),
                "{pinned}: {said:?}"
            );
        }
        assert!(
            !sets(&command, "GIT_CONFIG_COUNT")
                && !sets(&command, "GIT_CONFIG_KEY_0")
                && !sets(&command, "GIT_CONFIG_VALUE_0"),
            "no token near the shell"
        );
        assert!(sets(&command, "OTHER"));

        // No local transfer: no shell and no pins.
        let command =
            git.command(&Invocation::new(&git, "fetch", Some(&repo)).args(["fetch", "origin"]));
        let said = words(&command);
        assert!(!said.iter().any(|word| word == "--shell"), "{said:?}");
        assert!(
            !said.iter().any(|word| word.starts_with("protocol.file")),
            "{said:?}"
        );
        // An address that is no folder of this machine is no local transfer.
        let https =
            Invocation::new(&git, "fetch", Some(&repo)).local_if_path("https://github.com/a/b.git");
        assert!(!https.local);
        assert!(
            Invocation::new(&git, "fetch", Some(&repo))
                .local_if_path("/srv/x.git")
                .local
        );
        Ok(())
    }

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_size_on_disk_sums_a_folder_without_following_links() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("objects/pack"))?;
        let (head, pack) = (dir.path().join("HEAD"), dir.path().join("objects/pack/p"));
        for (path, len) in [(&head, 10), (&pack, 100)] {
            let file = std::fs::File::create(path)?;
            std::io::Write::write_all(&mut &file, &vec![0; len])?;
            // Settled blocks: a filesystem may allocate them when it commits the write.
            file.sync_all()?;
        }
        let each = |path: &Path| std::fs::symlink_metadata(path).map(|m| taken(&m));
        let files = each(&head)? + each(&pack)?;
        assert!(files >= 110, "{files}");
        assert_eq!(size_on_disk(dir.path()), files);
        assert_eq!(size_on_disk(&head), each(&head)?);
        assert_eq!(size_on_disk(&dir.path().join("missing")), 0);
        assert_eq!(size_of_all(&[&head, &pack, &dir.path().join("x")]), files);
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir()?;
            std::fs::write(outside.path().join("big"), [0; 1000])?;
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(outside.path(), &link)?;
            // The link counts as itself (its target's path), never as what it points to.
            assert_eq!(size_on_disk(dir.path()), files + each(&link)?);
        }
        Ok(())
    }

    #[test]
    fn worktree_listings_and_changes_take_the_registration_lock() {
        for command in ["add", "move", "remove", "prune", "list"] {
            assert!(
                touches_worktree_registrations(&args(&["worktree", command, "--porcelain"])),
                "{command}"
            );
        }
        assert!(!touches_worktree_registrations(&args(&[
            "worktree", "lock", "x"
        ])));
        assert!(!touches_worktree_registrations(&args(&[
            "rev-parse",
            "list"
        ])));
        assert!(!touches_worktree_registrations(&args(&[])));
    }

    #[test]
    fn the_subcommand_comes_after_the_configuration_pairs() {
        let fetch = Some(OsStr::new("fetch"));
        assert_eq!(
            subcommand(&args(&["fetch", "--quiet", "--", "origin"])),
            fetch
        );
        assert_eq!(
            subcommand(&args(&[
                "-c",
                "transfer.fsckObjects=true",
                "-c",
                "fetch.fsckObjects=true",
                "fetch",
                "--quiet"
            ])),
            fetch
        );
        assert_eq!(
            subcommand(&args(&["worktree", "list", "fetch"])),
            Some(OsStr::new("worktree"))
        );
        assert_eq!(subcommand(&args(&["-c"])), None);
        assert_eq!(subcommand(&args(&["-c", "a=b"])), None);
        assert_eq!(subcommand(&args(&[])), None);
    }

    #[test]
    fn only_a_worktree_head_named_bad_is_a_half_added_worktree() {
        let fetch = b"fatal: bad object worktrees/build@1/HEAD\nerror: /tmp/o.git did not send all necessary objects\n";
        assert!(names_a_half_added_worktree(fetch));
        assert!(names_a_half_added_worktree(
            b"fatal: bad object worktrees/t1/HEAD"
        ));
        for other in [
            &b"fatal: bad object HEAD"[..],
            b"fatal: bad object refs/heads/x",
            b"fatal: bad object worktrees//HEAD",
            b"fatal: bad object worktrees/a/b/HEAD",
            b"fatal: bad object worktrees/a/index",
            b"fatal: could not read from remote repository",
            b"",
        ] {
            assert!(
                !names_a_half_added_worktree(other),
                "{}",
                String::from_utf8_lossy(other)
            );
        }
    }
}
