//! How a move reaches the new host (SPEC-460 MIG-010): `ssh` with a one-time key and a handful
//! of constant remote commands. A remote command passes through the remote user's shell, so
//! nothing is ever put into one: requests travel as JSON, files and the copy as streams, all on
//! standard input.

use std::io::{self, BufRead as _, Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::target::{REPLY, Reply, Request};

/// Receives this build into the new host's state directory.
pub const PUT_BUILD: &str = "umask 077 && mkdir -p /var/lib/yulius-migrate && \
     cat > /var/lib/yulius-migrate/yulius.new && \
     chmod 0700 /var/lib/yulius-migrate/yulius.new && \
     mv -f /var/lib/yulius-migrate/yulius.new /var/lib/yulius-migrate/yulius";
/// Runs one step there.
pub const STEP: &str = "/var/lib/yulius-migrate/yulius instance migrate-target";
/// Installs the copy arriving on standard input.
pub const RECEIVE: &str = "/var/lib/yulius-migrate/yulius instance migrate-target --receive";
/// Extracts carried files at the root; owners are taken by name, so the service user's files
/// belong to the new host's service user.
pub const UNTAR: &str = "tar -C / -xpf -";
/// Adds the one-time key, with the operator's own sign-in. On a line of its own, also after a
/// file that ends without a newline.
pub const ADD_KEY: &str =
    "umask 077 && mkdir -p ~/.ssh && { echo; cat; } >> ~/.ssh/authorized_keys";
/// Swallows the link test's bytes.
pub const SINK: &str = "cat > /dev/null";
/// Answers a sign-in.
pub const HELLO: &str = "true";
/// In [`Remote::reachable`]'s error when this host holds no one-time key: none was made yet, or
/// a finish deleted it ([`Remote::forget`]).
pub const NO_KEY: &str = "there is no one-time key";
/// In [`Remote::reachable`]'s error when the new host refuses the key (`ssh`'s own words): it
/// was removed there.
pub const KEY_REFUSED: &str = "Permission denied";

/// The new host, as a move uses it.
pub trait Remote {
    /// Whether the one-time key signs in.
    ///
    /// # Errors
    /// Why not.
    fn reachable(&mut self) -> Result<(), String>;
    /// Makes the one-time key when there is none; its public line.
    ///
    /// # Errors
    /// `ssh-keygen` failed.
    fn key(&mut self) -> Result<String, String>;
    /// Adds the key to the new host's `authorized_keys` through the operator's own sign-in
    /// (a password or a second factor is asked on the terminal).
    ///
    /// # Errors
    /// The sign-in failed.
    fn install_key(&mut self) -> Result<(), String>;
    /// Copies the build at `exe` to the new host's state directory.
    ///
    /// # Errors
    /// The transfer failed.
    fn put_build(&mut self, exe: &Path) -> Result<(), String>;
    /// Runs one step on the new host.
    ///
    /// # Errors
    /// The step did not run or did not answer.
    fn call(&mut self, request: &Request) -> Result<Reply, String>;
    /// [`Remote::call`] for a step that may not be runnable yet (no build there): what it
    /// says, without its complaint.
    fn try_call(&mut self, request: &Request) -> Option<Reply> {
        self.call(request).ok()
    }
    /// Sends files and directories of this host as they are; they land at the same paths.
    ///
    /// # Errors
    /// The transfer failed.
    fn send_paths(&mut self, paths: &[PathBuf]) -> Result<(), String>;
    /// Streams a copy, written by `write`, into the new host's import.
    ///
    /// # Errors
    /// The stream failed and the import said nothing.
    fn send_copy(
        &mut self,
        zstd: bool,
        write: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> Result<Reply, String>;
    /// How long `bytes` take through the connection.
    ///
    /// # Errors
    /// The transfer failed.
    fn measure(&mut self, bytes: u64) -> Result<Duration, String>;
    /// Deletes the one-time key and what else the connection kept on this host.
    fn forget(&mut self);
    /// The new host's key as first seen (`SHA256:… (ED25519)`), for the person to compare.
    fn host_key(&mut self) -> Option<String> {
        None
    }
}

/// Whether `destination` is `[user@]host` and nothing `ssh` could read as an option.
#[must_use]
pub fn destination_ok(destination: &str) -> bool {
    let name = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-:[]".contains(c))
    };
    match destination.split_once('@') {
        Some((user, host)) => name(user) && name(host) && !user.contains([':', '[', ']']),
        None => name(destination),
    }
}

/// The host of a destination, without its user.
#[must_use]
pub fn host_part(destination: &str) -> &str {
    destination
        .split_once('@')
        .map_or(destination, |(_, host)| host)
}

/// The reply in a step's output: its last [`REPLY`] line. The lines before are what the step's
/// commands printed.
#[must_use]
pub fn parse_reply(output: &str) -> Option<Reply> {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(REPLY))
        .and_then(|json| serde_json::from_str(json).ok())
}

/// `ssh` to one destination with a one-time key kept in `dir`.
#[derive(Debug, Clone)]
pub struct Ssh {
    destination: String,
    port: u16,
    dir: PathBuf,
    /// The key's comment: what [`Request::Finish`] removes from `authorized_keys`.
    comment: String,
}

impl Ssh {
    /// A connection to `destination` (already checked with [`destination_ok`]).
    #[must_use]
    pub fn new(destination: &str, port: u16, dir: &Path, comment: &str) -> Self {
        Self {
            destination: destination.to_owned(),
            port,
            dir: dir.to_path_buf(),
            comment: comment.to_owned(),
        }
    }

    fn key_file(&self) -> PathBuf {
        self.dir.join("key")
    }

    /// The arguments of an `ssh` that runs `command`; with `key` only the one-time key signs
    /// in and nothing is asked.
    #[must_use]
    pub fn args(&self, key: bool, command: &str) -> Vec<String> {
        let dir = self.dir.display();
        let mut args: Vec<String> = vec![
            "-p".into(),
            self.port.to_string(),
            "-o".into(),
            format!("UserKnownHostsFile={dir}/known_hosts"),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=8".into(),
            // Each call is its own connection, whatever root's ssh configuration says: no
            // shared master may outlive the key's removal.
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "ControlPath=none".into(),
        ];
        if key {
            for option in ["IdentitiesOnly=yes", "BatchMode=yes", "ConnectTimeout=20"] {
                args.push("-o".into());
                args.push(option.into());
            }
            args.push("-i".into());
            args.push(self.key_file().display().to_string());
        }
        args.push("--".into());
        args.push(self.destination.clone());
        args.push(command.to_owned());
        args
    }

    fn spawn(&self, command: &str, stdin: Stdio, stdout: Stdio) -> Result<Child, String> {
        self.spawn_with(command, stdin, stdout, Stdio::inherit())
    }

    fn spawn_with(
        &self,
        command: &str,
        stdin: Stdio,
        stdout: Stdio,
        stderr: Stdio,
    ) -> Result<Child, String> {
        use std::os::unix::process::CommandExt as _;
        Command::new("ssh")
            .args(self.args(true, command))
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            // Its own process group: a Ctrl-C on the terminal reaches the move, which decides
            // (thaw, or go on once the new instance starts), not the step in flight.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot run ssh: {e}"))
    }

    fn step(&self, request: &Request, stderr: Stdio) -> Result<Reply, String> {
        let json = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let mut child = self.spawn_with(STEP, Stdio::piped(), Stdio::piped(), stderr)?;
        let sent = child
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(&json));
        let reply = child.stdout.take().and_then(follow);
        let end = ended("the step on the new host", &mut child);
        match (reply, sent, end) {
            (Some(reply), _, _) => Ok(reply),
            (None, Err(e), _) => Err(format!("the new host did not take the request: {e}")),
            (None, _, Err(why)) => Err(why),
            (None, _, Ok(())) => Err("the new host answered nothing".into()),
        }
    }
}

fn ended(what: &str, child: &mut Child) -> Result<(), String> {
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{what} ended with {status}")),
        Err(e) => Err(format!("{what}: {e}")),
    }
}

/// Reads a step's output to its end, showing what its commands print as it comes.
fn follow(out: impl io::Read) -> Option<Reply> {
    let mut reply = None;
    let mut reader = io::BufReader::new(out);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        match text.strip_prefix(REPLY) {
            Some(json) => reply = serde_json::from_str(json.trim_end()).ok(),
            // A terminal that went away must not end the move.
            None => {
                let _ = write!(io::stderr(), "{text}");
            }
        }
    }
    reply
}

impl Remote for Ssh {
    fn reachable(&mut self) -> Result<(), String> {
        if !self.key_file().is_file() {
            return Err(format!("{NO_KEY} yet"));
        }
        // Its words are kept: [`KEY_REFUSED`] (the key was removed there) is told from a host
        // that cannot be reached.
        let out = Command::new("ssh")
            .args(self.args(true, HELLO))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run ssh: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&out.stderr);
        let last = said.lines().rev().find(|l| !l.trim().is_empty());
        Err(format!(
            "ssh ended with {}: {}",
            out.status,
            last.unwrap_or("it said nothing").trim()
        ))
    }

    fn key(&mut self) -> Result<String, String> {
        let key = self.key_file();
        if !key.is_file() {
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", &self.comment, "-f"])
                .arg(&key)
                .stdin(Stdio::null())
                .status()
                .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
            if !status.success() {
                return Err(format!("ssh-keygen ended with {status}"));
            }
        }
        std::fs::read_to_string(key.with_extension("pub"))
            .map(|line| line.trim().to_owned())
            .map_err(|e| format!("cannot read the one-time key: {e}"))
    }

    fn install_key(&mut self) -> Result<(), String> {
        let public = std::fs::File::open(self.key_file().with_extension("pub"))
            .map_err(|e| format!("cannot read the one-time key: {e}"))?;
        // The operator's own sign-in: ssh asks for a password or a second factor on the
        // terminal, never on standard input.
        let status = Command::new("ssh")
            .args(self.args(false, ADD_KEY))
            .stdin(public)
            .status()
            .map_err(|e| format!("cannot run ssh: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "the sign-in to {} ended with {status}",
                self.destination
            ))
        }
    }

    fn put_build(&mut self, exe: &Path) -> Result<(), String> {
        let build = std::fs::File::open(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        let mut child = self.spawn(PUT_BUILD, build.into(), Stdio::null())?;
        ended("copying the build", &mut child)
    }

    fn call(&mut self, request: &Request) -> Result<Reply, String> {
        self.step(request, Stdio::inherit())
    }

    fn try_call(&mut self, request: &Request) -> Option<Reply> {
        self.step(request, Stdio::null()).ok()
    }

    fn send_paths(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut tar = Command::new("tar");
        tar.args(["-C", "/", "-cf", "-", "--"]);
        for path in paths {
            let relative = path
                .strip_prefix("/")
                .map_err(|_| format!("{} is not absolute", path.display()))?;
            tar.arg(relative);
        }
        let mut tar = tar
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run tar: {e}"))?;
        let stream = tar.stdout.take().ok_or("tar has no output")?;
        let mut ssh = self.spawn(UNTAR, stream.into(), Stdio::null())?;
        let received = ended("extracting on the new host", &mut ssh);
        // 1: a file changed while it was read, on a host that is still running.
        let packed = match tar.wait() {
            Ok(status) if matches!(status.code(), Some(0 | 1)) => Ok(()),
            Ok(status) => Err(format!("tar ended with {status}")),
            Err(e) => Err(format!("tar: {e}")),
        };
        // The new host's own words first: tar's broken pipe is their consequence.
        received.and(packed)
    }

    fn send_copy(
        &mut self,
        zstd: bool,
        write: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>,
    ) -> Result<Reply, String> {
        let mut pack = None;
        let mut ssh = if zstd {
            let mut child = Command::new("zstd")
                .args(["-T0", "-3", "-q", "-c"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| format!("cannot run zstd: {e}"))?;
            let packed = child.stdout.take().ok_or("zstd has no output")?;
            pack = Some(child);
            self.spawn(RECEIVE, packed.into(), Stdio::piped())?
        } else {
            self.spawn(RECEIVE, Stdio::piped(), Stdio::piped())?
        };
        let answer = ssh.stdout.take();
        let reader = std::thread::spawn(move || answer.and_then(follow));
        let sink = match pack.as_mut() {
            Some(child) => child.stdin.take(),
            None => ssh.stdin.take(),
        };
        let sent = match sink {
            Some(sink) => {
                let mut sink = io::BufWriter::with_capacity(1 << 20, sink);
                write(&mut sink).and_then(|()| sink.flush())
            }
            None => Err(io::Error::other("no stream to the new host")),
        };
        if let Some(mut child) = pack {
            let _ = child.wait();
        }
        let end = ended("the import on the new host", &mut ssh);
        let reply = reader.join().ok().flatten();
        match (reply, sent, end) {
            // The import's own words win over a broken pipe they caused.
            (Some(reply), _, _) => Ok(reply),
            (None, Err(e), _) => Err(format!("the copy's stream failed: {e}")),
            (None, _, Err(why)) => Err(why),
            (None, _, Ok(())) => Err("the import on the new host answered nothing".into()),
        }
    }

    fn measure(&mut self, bytes: u64) -> Result<Duration, String> {
        let mut child = self.spawn(SINK, Stdio::piped(), Stdio::null())?;
        let start = Instant::now();
        let sent = child.stdin.take().map_or(Ok(0), |mut stdin| {
            io::copy(&mut io::repeat(0x5a).take(bytes), &mut stdin)
        });
        ended("the link test", &mut child)?;
        sent.map_err(|e| format!("the link test: {e}"))?;
        Ok(start.elapsed())
    }

    fn host_key(&mut self) -> Option<String> {
        let out = Command::new("ssh-keygen")
            .arg("-lf")
            .arg(self.dir.join("known_hosts"))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let fields: Vec<&str> = text.lines().next()?.split_whitespace().collect();
        Some(format!("{} {}", fields.get(1)?, fields.last()?))
    }

    fn forget(&mut self) {
        for name in ["key", "key.pub", "known_hosts", "known_hosts.old"] {
            let _ = std::fs::remove_file(self.dir.join(name));
        }
    }
}
