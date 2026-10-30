//! In-sandbox redaction of agent secrets (SPEC-002 ADR-20, VLT-007).
//!
//! Two entry points run inside the agent's sandbox, as the agent's own user:
//! - `yulius --scrub-exec '<command>'` is Claude Code's `CLAUDE_CODE_SHELL_PREFIX`: every Bash
//!   command (and hook, and stdio MCP server) runs through it. It runs the command with the
//!   shell, streams stdout and stderr through a redactor and exits with the command's status.
//! - `yulius hook claude PostToolUse` replaces a tool result that holds a secret with its
//!   redacted copy (`hookSpecificOutput.updatedToolOutput`, Claude Code ≥ 2.1.292).
//!
//! Neither holds a value: they redact from **fingerprints** (rolling hash plus salted SHA-256
//! of each value and encoding), fetched from `POST /internal/scrub/fingerprints` with the task
//! token, or read from the run's `scrub.json` while Yulius restarts. With neither, a Yulius run
//! fails closed: the command does not run and a tool result is withheld.

use std::io::{self, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use yulius_contracts::scrub::{FINGERPRINT_VERSION, FingerprintSet, RedactionReport};
use yulius_domain::vault::redact::{Fingerprint, HashedMatcher, Matcher, StreamRedactor};

use crate::hook_client::{HookError, post_internal};

/// Environment variable naming the run's fingerprint file.
pub const SCRUB_FILE_ENV: &str = "YULIUS_SCRUB_FILE";
/// Largest tool result read by the `PostToolUse` hook.
pub const MAX_TOOL_RESULT_BYTES: usize = 32 * 1024 * 1024;
/// Quiet time after which held output (the end of an unfinished line that may begin a value) is
/// written. The model reads a command's output when it ends, so waiting costs only the live view.
const FLUSH_AFTER: Duration = Duration::from_secs(1);
/// Deadline of the fingerprint request.
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);
/// Deadline of a redaction report (best effort).
const REPORT_TIMEOUT: Duration = Duration::from_millis(500);
/// Exit status when a command is not run (fails closed).
const NOT_RUN: i32 = 126;

/// What the run's redaction knows.
pub enum Fingerprints {
    /// Not a Yulius agent run (no task token, no fingerprint file): nothing to redact.
    NotARun,
    /// The fingerprints.
    Set(FingerprintSet),
    /// A Yulius run whose fingerprints cannot be loaded, and why.
    Unavailable(Unavailable),
}

/// Why a run's fingerprints could not be loaded (SPEC-353 LAP-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    /// What Yulius answered.
    pub server: ServerAnswer,
    /// What became of the run's fingerprint file.
    pub file: FileState,
}

/// What Yulius answered the fingerprint request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerAnswer {
    /// It refused the token (a 4xx): the run ended, or the person may no longer work here.
    Refused(u16),
    /// It answered with another error status.
    Failed(u16),
    /// It could not be reached, or did not answer in time.
    Unreachable,
    /// Its answer, or this session's Yulius address, could not be used.
    Unusable,
}

/// The run's fingerprint file, the fallback while Yulius cannot answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// The session has none (`YULIUS_SCRUB_FILE` is not set).
    None,
    /// It cannot be read.
    Unreadable(String),
    /// It holds no fingerprint set this `yulius` can use.
    Invalid(String),
    /// Not read: Yulius refused the session, which no file overrides (SPEC-352 SRF-003).
    NotConsulted,
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn valid(set: FingerprintSet) -> Option<FingerprintSet> {
    (set.version == FINGERPRINT_VERSION).then_some(set)
}

/// Loads the run's fingerprints: from Yulius, else from the run's file while Yulius cannot be
/// reached or is not ready. A Yulius that refuses the token (an ended run, a person who may no
/// longer work in the sandbox) is not a restart: the file does not stand in for it (SPEC-352
/// SRF-003).
pub fn load_fingerprints(env: &dyn Fn(&str) -> Option<String>) -> Fingerprints {
    let file = env(SCRUB_FILE_ENV).filter(|f| !f.is_empty());
    let in_run = file.is_some() || env(crate::launch::TASK_TOKEN_ENV).is_some();
    if !in_run {
        return Fingerprints::NotARun;
    }
    let server = match post_internal("/internal/scrub/fingerprints", b"{}", env, FETCH_TIMEOUT) {
        Ok(body) => match serde_json::from_slice(&body).ok().and_then(valid) {
            Some(set) => return Fingerprints::Set(set),
            None => ServerAnswer::Unusable,
        },
        // A refusal is final (SPEC-352 SRF-003): the file is not consulted.
        Err(HookError::Status(code)) if (400..500).contains(&code) => {
            return Fingerprints::Unavailable(Unavailable {
                server: ServerAnswer::Refused(code),
                file: FileState::NotConsulted,
            });
        }
        Err(HookError::Status(code)) => ServerAnswer::Failed(code),
        Err(HookError::Io) => ServerAnswer::Unreachable,
        Err(_) => ServerAnswer::Unusable,
    };
    let file = match file {
        None => FileState::None,
        Some(path) => match std::fs::read(&path) {
            Err(_) => FileState::Unreadable(path),
            Ok(bytes) => match serde_json::from_slice(&bytes).ok().and_then(valid) {
                Some(set) => return Fingerprints::Set(set),
                None => FileState::Invalid(path),
            },
        },
    };
    Fingerprints::Unavailable(Unavailable { server, file })
}

/// What `--scrub-exec` says when it does not run a command for want of fingerprints: the cause
/// and what to do (SPEC-353 LAP-001). A token Yulius refuses means the run ended, never a restart.
#[must_use]
pub fn unavailable_message(why: &Unavailable) -> String {
    let file = match &why.file {
        FileState::None => "This session has no fingerprint file to fall back on.".to_owned(),
        FileState::Unreadable(path) => format!("Its fingerprint file {path} cannot be read."),
        FileState::Invalid(path) => {
            format!("Its fingerprint file {path} holds no fingerprints this yulius can use.")
        }
        FileState::NotConsulted => String::new(),
    };
    let not_run = "yulius: the agent-secret fingerprints of this run cannot be loaded, so the command was not run";
    match why.server {
        ServerAnswer::Refused(code) => format!(
            "yulius: the command was not run: Yulius no longer accepts this session (HTTP {code}). \
             The run it belonged to has ended (the task was stopped, finished or continued in a \
             new session), or you may no longer work in this project. No command can run \
             in this session any more: close it and continue from Yulius (send the task a \
             message, or open the Superagent again)."
        ),
        ServerAnswer::Unreachable => format!(
            "{not_run}: Yulius could not be reached from this sandbox, or did not answer within \
             {} s. {file} Yulius may be restarting: try again in a minute. If it is running, this \
             sandbox's connections to it may be used up by leftover processes (see \"Leftover \
             agent processes\" in Yulius's operations guide).",
            FETCH_TIMEOUT.as_secs()
        ),
        ServerAnswer::Failed(code) => format!(
            "{not_run}: Yulius answered HTTP {code}. {file} Try again; if it keeps failing, \
             Yulius's log says why."
        ),
        ServerAnswer::Unusable => format!(
            "{not_run}: Yulius's answer, or this session's Yulius address (YULIUS_URL), could not \
             be used. {file} Continue the task from Yulius to start a fresh session."
        ),
    }
}

/// A matcher over a fingerprint set (`None` when it holds nothing to look for).
pub fn matcher(set: &FingerprintSet) -> Option<HashedMatcher<impl Fn(&[u8]) -> [u8; 32]>> {
    let salt = unhex(&set.salt)?;
    let fingerprints: Vec<Fingerprint> = set
        .needles
        .iter()
        .filter_map(|n| {
            let digest: [u8; 32] = unhex(&n.digest)?.try_into().ok()?;
            Some(Fingerprint {
                secret: n.secret,
                len: n.len,
                rolling: n.rolling,
                digest,
            })
        })
        .collect();
    if fingerprints.is_empty() {
        return None;
    }
    let verify = move |window: &[u8]| -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(&salt);
        hasher.update(window);
        hasher.finalize().into()
    };
    let matcher = HashedMatcher::new(set.base, set.names.clone(), fingerprints, verify);
    (!matcher.is_empty()).then_some(matcher)
}

/// Reports redacted names to Yulius for the audit trail (best effort).
fn report(env: &dyn Fn(&str) -> Option<String>, channel: &str, names: Vec<String>) {
    if names.is_empty() {
        return;
    }
    let body = RedactionReport {
        channel: channel.into(),
        names,
    };
    if let Ok(body) = serde_json::to_vec(&body) {
        let _ignored = post_internal("/internal/scrub/report", &body, env, REPORT_TIMEOUT);
    }
}

/// The shell that runs a wrapped command: `$SHELL` when it is a known shell, else bash, else sh.
fn shell(env: &dyn Fn(&str) -> Option<String>) -> String {
    let known = ["bash", "zsh", "sh", "dash"];
    if let Some(shell) = env("SHELL")
        && shell.starts_with('/')
        && known.iter().any(|k| shell.ends_with(&format!("/{k}")))
        && std::path::Path::new(&shell).exists()
    {
        return shell;
    }
    if std::path::Path::new("/bin/bash").exists() {
        "/bin/bash".into()
    } else {
        "/bin/sh".into()
    }
}

/// `nice`, which runs agent commands at a lower CPU priority than the agents and the sandbox's
/// CI jobs: eight tasks building at once starved the CI gate every task waits for (load 50 on 12
/// CPUs, F 2026-10-12; at 5 levels, full CI builds still took past 30 minutes next to eight agent
/// builds, F 2026-10-13). It execs the command, so the status is the command's.
const NICE: &str = "/usr/bin/nice";
/// How much lower (relative to the agent).
const NICENESS: &str = "10";

fn command(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Option<Command> {
    let (program, rest): (String, Vec<&str>) = match args {
        [] => return None,
        [line] => (shell(env), vec!["-c", line.as_str()]),
        [program, rest @ ..] => (program.clone(), rest.iter().map(String::as_str).collect()),
    };
    let mut command = if std::path::Path::new(NICE).exists() {
        let mut nice = Command::new(NICE);
        nice.args(["-n", NICENESS, "--"]).arg(program);
        nice
    } else {
        Command::new(program)
    };
    command.args(rest);
    Some(command)
}

fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Which stream a chunk came from.
enum Chunk {
    Out(Vec<u8>),
    Err(Vec<u8>),
    Closed,
}

fn pump(mut source: impl Read + Send + 'static, tx: mpsc::Sender<Chunk>, out: bool) {
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buffer[..n].to_vec();
                    let sent = if out {
                        tx.send(Chunk::Out(chunk))
                    } else {
                        tx.send(Chunk::Err(chunk))
                    };
                    if sent.is_err() {
                        return;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ignored = tx.send(Chunk::Closed);
    });
}

/// `yulius --scrub-exec <command>`: runs `command` (one string: through the shell; several:
/// as a program and its arguments) with its output redacted. Returns the exit status.
pub fn run_scrub_exec(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> i32 {
    let Some(mut child_command) = command(args, env) else {
        eprintln!("yulius --scrub-exec: no command");
        return 2;
    };
    let set = match load_fingerprints(env) {
        Fingerprints::Set(set) => Some(set),
        Fingerprints::NotARun => None,
        Fingerprints::Unavailable(why) => {
            eprintln!("{}", unavailable_message(&why));
            return NOT_RUN;
        }
    };
    let Some(matcher) = set.as_ref().and_then(matcher) else {
        // Nothing to redact: become the command.
        use std::os::unix::process::CommandExt;
        let err = child_command.exec();
        eprintln!("yulius --scrub-exec: {err}");
        return NOT_RUN;
    };
    let mut child = match child_command
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("yulius --scrub-exec: {err}");
            return NOT_RUN;
        }
    };
    let (tx, rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        pump(stdout, tx.clone(), true);
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, tx.clone(), false);
    }
    drop(tx);
    let matcher = std::sync::Arc::new(matcher);
    let mut out_redactor = StreamRedactor::new(std::sync::Arc::clone(&matcher), 1);
    let mut err_redactor = StreamRedactor::new(std::sync::Arc::clone(&matcher), 1);
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let mut open = 2;
    while open > 0 {
        match rx.recv_timeout(FLUSH_AFTER) {
            Ok(Chunk::Out(chunk)) => {
                let _ = stdout.write_all(&out_redactor.push(&chunk));
                let _ = stdout.flush();
            }
            Ok(Chunk::Err(chunk)) => {
                let _ = stderr.write_all(&err_redactor.push(&chunk));
                let _ = stderr.flush();
            }
            Ok(Chunk::Closed) => open -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if out_redactor.holding() {
                    let _ = stdout.write_all(&out_redactor.flush());
                    let _ = stdout.flush();
                }
                if err_redactor.holding() {
                    let _ = stderr.write_all(&err_redactor.flush());
                    let _ = stderr.flush();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = stdout.write_all(&out_redactor.flush());
    let _ = stdout.flush();
    let _ = stderr.write_all(&err_redactor.flush());
    let _ = stderr.flush();
    let status = child.wait();
    let mut names = out_redactor.take_used();
    names.extend(err_redactor.take_used());
    names.sort();
    names.dedup();
    report(env, "bash", names);
    match status {
        Ok(status) => exit_code(status),
        Err(_) => NOT_RUN,
    }
}

/// Redacts every string inside `value`; returns whether anything changed.
fn redact_value(value: &mut Value, matcher: &dyn Matcher, names: &mut Vec<String>) -> bool {
    match value {
        Value::String(text) => {
            let (redacted, found) =
                yulius_domain::vault::redact::redact_text(matcher, text.as_str());
            if found.is_empty() {
                false
            } else {
                *text = redacted;
                names.extend(found);
                true
            }
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            redact_value(item, matcher, names) | changed
        }),
        Value::Object(map) => map.values_mut().fold(false, |changed, item| {
            redact_value(item, matcher, names) | changed
        }),
        _ => false,
    }
}

fn replacement(output: &mut dyn Write, tool_output: &Value) -> i32 {
    let answer = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": tool_output,
        }
    });
    let _ignored = serde_json::to_writer(&mut *output, &answer)
        .and_then(|()| output.flush().map_err(serde_json::Error::io));
    0
}

/// `yulius hook claude PostToolUse`: prints a redacted `updatedToolOutput` when the tool result
/// holds an agent secret, nothing otherwise. Always exits 0.
pub fn post_tool_use(
    input: &mut dyn Read,
    output: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    let withheld = |output: &mut dyn Write, why: &str| {
        replacement(
            output,
            &Value::String(format!(
                "[Yulius withheld this tool result: {why}. Agent secrets are never shown.]"
            )),
        )
    };
    let mut body = Vec::new();
    if input
        .take(MAX_TOOL_RESULT_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .is_err()
    {
        return 0;
    }
    let set = match load_fingerprints(env) {
        Fingerprints::NotARun => return 0,
        Fingerprints::Set(set) => set,
        Fingerprints::Unavailable(_) => {
            return withheld(output, "it could not be checked for agent secrets");
        }
    };
    let Some(matcher) = matcher(&set) else {
        return 0;
    };
    if body.len() > MAX_TOOL_RESULT_BYTES {
        return withheld(output, "it is too large to check for agent secrets");
    }
    let Ok(mut payload) = serde_json::from_slice::<Value>(&body) else {
        return 0;
    };
    let Some(tool_output) = payload.get_mut("tool_response") else {
        return 0;
    };
    let mut names = Vec::new();
    if !redact_value(tool_output, &matcher, &mut names) {
        return 0;
    }
    let code = replacement(output, tool_output);
    names.sort();
    names.dedup();
    report(env, "tool", names);
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_contracts::scrub::Fingerprint as WireFingerprint;
    use yulius_domain::vault::encode::{base64, needles};
    use yulius_domain::vault::redact::rolling_hash;

    const VALUE: &str = "sk_test_4eC39HqLyjWDarjtT1zdp7dc";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A fingerprint set as the server makes it.
    fn set() -> FingerprintSet {
        let salt = b"0123456789abcdef";
        let base = 912_367;
        FingerprintSet {
            version: FINGERPRINT_VERSION,
            base,
            salt: hex(salt),
            names: vec!["STRIPE_KEY".into()],
            needles: needles(VALUE)
                .iter()
                .map(|n| {
                    let mut hasher = Sha256::new();
                    hasher.update(salt);
                    hasher.update(n);
                    WireFingerprint {
                        secret: 0,
                        len: n.len(),
                        rolling: rolling_hash(base, n),
                        digest: hex(&hasher.finalize()),
                    }
                })
                .collect(),
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn tool_results_are_redacted_recursively() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("scrub.json");
        std::fs::write(&file, serde_json::to_vec(&set())?)?;
        let file = file.to_string_lossy().into_owned();
        let env = move |k: &str| (k == SCRUB_FILE_ENV).then(|| file.clone());
        let encoded = base64(format!("key:{VALUE}").as_bytes(), false, true);
        let input = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_response": {"type": "text", "file": {"filePath": "/w/.env", "content": format!("STRIPE={VALUE}\nB64={encoded}\n")}}
        });
        let mut out = Vec::new();
        post_tool_use(&mut input.to_string().as_bytes(), &mut out, &env);
        let text = String::from_utf8(out)?;
        assert!(!text.contains(VALUE), "{text}");
        let answer: Value = serde_json::from_str(&text)?;
        let content = answer["hookSpecificOutput"]["updatedToolOutput"]["file"]["content"]
            .as_str()
            .ok_or("content")?;
        assert!(
            content.starts_with("STRIPE=[secret:STRIPE_KEY]\nB64="),
            "{content}"
        );
        assert!(content.contains("[secret:STRIPE_KEY]"));
        assert_eq!(answer["hookSpecificOutput"]["hookEventName"], "PostToolUse");

        // Nothing to redact: no output at all.
        let clean = serde_json::json!({"tool_response": {"stdout": "hello"}});
        let mut out = Vec::new();
        post_tool_use(&mut clean.to_string().as_bytes(), &mut out, &env);
        assert!(out.is_empty());
        Ok(())
    }

    #[test]
    fn runs_without_fingerprints_withhold_results() {
        let env = |k: &str| (k == SCRUB_FILE_ENV).then(|| "/nonexistent/scrub.json".to_owned());
        let input = serde_json::json!({"tool_response": {"stdout": VALUE}});
        let mut out = Vec::new();
        post_tool_use(&mut input.to_string().as_bytes(), &mut out, &env);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("withheld"), "{text}");
        assert!(!text.contains(VALUE));
        // Outside a Yulius run nothing happens.
        let mut out = Vec::new();
        post_tool_use(&mut input.to_string().as_bytes(), &mut out, &no_env);
        assert!(out.is_empty());
    }

    #[test]
    fn matchers_need_well_formed_sets() {
        let mut bad = set();
        bad.salt = "zz".into();
        assert!(matcher(&bad).is_none());
        let mut empty = set();
        empty.needles.clear();
        assert!(matcher(&empty).is_none());
        assert!(matcher(&set()).is_some());
    }

    /// A Yulius that answers one fingerprint request with `status`; its URL.
    fn answering(status: &'static str) -> Result<String, Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // The whole request (its body is `{}`), so the answer never cuts the client's write.
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !request.ends_with(b"{}") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buffer[..n]),
                }
            }
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        });
        Ok(url)
    }

    /// A URL nothing listens on.
    fn nobody() -> Result<String, Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        Ok(format!("http://{}", listener.local_addr()?))
    }

    fn run_env(url: String, file: Option<String>) -> impl Fn(&str) -> Option<String> {
        move |key: &str| match key {
            crate::launch::YULIUS_URL_ENV => Some(url.clone()),
            crate::launch::TASK_TOKEN_ENV => Some("synthetic-task-token".to_owned()),
            SCRUB_FILE_ENV => file.clone(),
            _ => None,
        }
    }

    fn unavailable(fingerprints: Fingerprints) -> Result<Unavailable, &'static str> {
        match fingerprints {
            Fingerprints::Unavailable(why) => Ok(why),
            Fingerprints::Set(_) => Err("fingerprints loaded"),
            Fingerprints::NotARun => Err("not a run"),
        }
    }

    /// SPEC-353 LAP-001: a token Yulius refuses means the run ended (or the person may no longer
    /// work in the project): the message says so and what to do, and blames no restart. On
    /// 2026-10-30 every command of a Superagent said "Yulius is restarting" while it ran for days.
    #[test]
    fn a_refused_token_says_the_run_ended_not_a_restart() -> Result<(), Box<dyn std::error::Error>>
    {
        // A valid fallback file is not consulted once Yulius refused (SPEC-352 SRF-003).
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("scrub.json");
        std::fs::write(&file, serde_json::to_vec(&set())?)?;
        let file = file.to_string_lossy().into_owned();
        let env = run_env(answering("401 Unauthorized")?, Some(file));
        let why = unavailable(load_fingerprints(&env))?;
        assert_eq!(
            why,
            Unavailable {
                server: ServerAnswer::Refused(401),
                file: FileState::NotConsulted,
            }
        );
        let text = unavailable_message(&why);
        assert!(text.contains("has ended"), "{text}");
        assert!(text.contains("continue from Yulius"), "{text}");
        assert!(!text.to_lowercase().contains("restart"), "{text}");
        // The command does not run.
        let marker = dir.path().join("ran");
        let env = run_env(answering("403 Forbidden")?, None);
        let command = format!("touch '{}'", marker.display());
        assert_eq!(run_scrub_exec(&[command], &env), NOT_RUN);
        assert!(!marker.exists());
        Ok(())
    }

    /// LAP-001: an unreachable Yulius may be restarting, or this sandbox's connections to it may
    /// be used up by leftover processes; the message says both and what is wrong with the file.
    #[test]
    fn an_unreachable_yulius_names_a_restart_and_used_up_connections()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let invalid = dir.path().join("scrub.json");
        std::fs::write(&invalid, b"{}")?;
        let invalid = invalid.to_string_lossy().into_owned();
        let why = unavailable(load_fingerprints(&run_env(
            nobody()?,
            Some(invalid.clone()),
        )))?;
        assert_eq!(
            why,
            Unavailable {
                server: ServerAnswer::Unreachable,
                file: FileState::Invalid(invalid.clone()),
            }
        );
        let text = unavailable_message(&why);
        for part in [
            "could not be reached",
            "restarting",
            "leftover processes",
            &invalid,
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
        let missing = dir.path().join("gone.json").to_string_lossy().into_owned();
        let why = unavailable(load_fingerprints(&run_env(
            nobody()?,
            Some(missing.clone()),
        )))?;
        assert_eq!(why.file, FileState::Unreadable(missing.clone()));
        let text = unavailable_message(&why);
        assert!(
            text.contains(&format!("{missing} cannot be read")),
            "{text}"
        );
        Ok(())
    }

    /// LAP-001: another error status is named as it is.
    #[test]
    fn another_error_status_is_named() -> Result<(), Box<dyn std::error::Error>> {
        let env = run_env(answering("500 Internal Server Error")?, None);
        let why = unavailable(load_fingerprints(&env))?;
        assert_eq!(why.server, ServerAnswer::Failed(500));
        let text = unavailable_message(&why);
        assert!(text.contains("HTTP 500"), "{text}");
        assert!(text.contains("no fingerprint file"), "{text}");
        Ok(())
    }

    /// A Yulius that accepts a connection and closes it before answering (a relay at its limit,
    /// SPEC-353 review): unreachable, not an unusable answer.
    fn closing() -> Result<String, Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        std::thread::spawn(move || {
            for stream in listener.incoming().take(4) {
                drop(stream);
            }
        });
        Ok(url)
    }

    /// LAP-001 (review round 1): a connection closed before any answer is an unreachable Yulius.
    #[test]
    fn a_connection_closed_before_an_answer_is_unreachable()
    -> Result<(), Box<dyn std::error::Error>> {
        let why = unavailable(load_fingerprints(&run_env(closing()?, None)))?;
        assert_eq!(why.server, ServerAnswer::Unreachable);
        let text = unavailable_message(&why);
        assert!(text.contains("leftover processes"), "{text}");
        Ok(())
    }

    /// LAP-001 (review round 1): a tool result withheld for want of fingerprints says why.
    #[test]
    fn a_withheld_tool_result_says_why() -> Result<(), Box<dyn std::error::Error>> {
        let env = run_env(answering("401 Unauthorized")?, None);
        let input = serde_json::json!({"tool_response": {"stdout": "hello"}});
        let mut out = Vec::new();
        post_tool_use(&mut input.to_string().as_bytes(), &mut out, &env);
        let text = String::from_utf8(out)?;
        assert!(text.contains("withheld"), "{text}");
        assert!(text.contains("has ended"), "{text}");
        assert!(!text.to_lowercase().contains("restart"), "{text}");
        Ok(())
    }
}
