//! In-sandbox redaction of agent secrets (SPEC-002 ADR-18, VLT-007).
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

use crate::hook_client::post_internal;

/// Environment variable naming the run's fingerprint file.
pub const SCRUB_FILE_ENV: &str = "YULIUS_SCRUB_FILE";
/// Largest tool result read by the `PostToolUse` hook.
pub const MAX_TOOL_RESULT_BYTES: usize = 32 * 1024 * 1024;
/// Quiet time after which held output is written.
const FLUSH_AFTER: Duration = Duration::from_millis(50);
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
    /// A Yulius run whose fingerprints cannot be loaded.
    Unavailable,
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

/// Loads the run's fingerprints: from Yulius, else from the run's file.
pub fn load_fingerprints(env: &dyn Fn(&str) -> Option<String>) -> Fingerprints {
    let file = env(SCRUB_FILE_ENV).filter(|f| !f.is_empty());
    let in_run = file.is_some() || env(crate::launch::TASK_TOKEN_ENV).is_some();
    if !in_run {
        return Fingerprints::NotARun;
    }
    if let Ok(body) = post_internal("/internal/scrub/fingerprints", b"{}", env, FETCH_TIMEOUT)
        && let Some(set) = serde_json::from_slice(&body).ok().and_then(valid)
    {
        return Fingerprints::Set(set);
    }
    if let Some(path) = file
        && let Some(set) = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .and_then(valid)
    {
        return Fingerprints::Set(set);
    }
    Fingerprints::Unavailable
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

fn command(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Option<Command> {
    match args {
        [] => None,
        [line] => {
            let mut command = Command::new(shell(env));
            command.arg("-c").arg(line);
            Some(command)
        }
        [program, rest @ ..] => {
            let mut command = Command::new(program);
            command.args(rest);
            Some(command)
        }
    }
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
        Fingerprints::Unavailable => {
            eprintln!(
                "yulius: the agent-secret fingerprints of this run cannot be loaded, so the command was not run (Yulius is restarting or the run ended); try again"
            );
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
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| redact_value(item, matcher, names) | changed),
        Value::Object(map) => map
            .values_mut()
            .fold(false, |changed, item| redact_value(item, matcher, names) | changed),
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
    let _ignored = serde_json::to_writer(&mut *output, &answer).and_then(|()| {
        output.flush().map_err(serde_json::Error::io)
    });
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
        Fingerprints::Unavailable => {
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
        assert!(content.starts_with("STRIPE=[secret:STRIPE_KEY]\nB64="), "{content}");
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
}
