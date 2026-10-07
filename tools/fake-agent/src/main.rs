//! Deterministic fake coding-agent CLI used by Yulius supervisor, harness and server tests.
//! Owned by work package A.
//!
//! The binary is built at `target/<profile>/fake-agent`; tests in other crates locate it with
//! `yulius_ptyhost::test_support::locate_fake_agent` (feature `test-support`).
//!
//! ```text
//! fake-agent [--mode echo|flood|ignore-term|fork-child|exit-early|sleep]
//!            [--bytes N] [--code N] [--seconds N] [--ignore-term]
//!            [--session-id ID] [--transcript PATH --transcript-kind claude|codex]
//!            [--limit-after N] [--report-env NAME]...
//! ```
//!
//! Modes:
//! - `echo` (default): prints `FAKE-AGENT READY session=<id>`, then answers each input line with
//!   `ECHO: <line>`. `/exit` exits 0, `/crash` exits 3, `/later <ms> <text>` prints `<text>` after
//!   a delay (≤ 60 s), `/size` prints `SIZE <rows> <cols>` of its terminal (via `stty size`),
//!   `/read <path>` prints `READ <file name> OK <n>` or `READ <file name> ERR <kind>` (containment
//!   probe; never the contents),
//!   `/flood <n>` floods like `--mode flood` on demand.
//! - `flood --bytes N`: writes N bytes to stdout and N bytes to stderr, prints `FLOOD DONE`, then
//!   behaves like `echo`.
//! - `ignore-term`: like `echo` but ignores SIGTERM, SIGINT and SIGHUP (also `--ignore-term`).
//! - `fork-child`: spawns a descendant `fake-agent --mode sleep` (same executable, inheriting
//!   `--ignore-term` and `--seconds`), prints `CHILD <pid>`, then behaves like `echo`.
//! - `exit-early --code N`: prints `EXITING code=N` and exits with N.
//! - `sleep [--seconds N]`: sleeps N seconds (default 600, bounded so a leaked fixture ends).
//!
//! With `--transcript`, each echoed turn appends JSONL records shaped like Claude Code
//! (`--transcript-kind claude`) or Codex rollouts (`codex`). `--limit-after N` prints
//! `LIMIT REACHED` and appends a usage-limit record after N turns; later turns only repeat
//! `LIMIT REACHED`. `--report-env NAME` prints `ENV NAME=set len=<n>` or `ENV NAME=unset` (never
//! the value).

use std::fs::OpenOptions;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const MAX_LINE: u64 = 64 * 1024;
const MAX_FLOOD_BYTES: u64 = 1 << 30;
const MAX_SLEEP_SECONDS: u64 = 3600;
const MAX_LATER_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Echo,
    Flood,
    IgnoreTerm,
    ForkChild,
    ExitEarly,
    Sleep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptKind {
    Claude,
    Codex,
}

#[derive(Debug)]
struct Options {
    mode: Mode,
    bytes: u64,
    code: i32,
    seconds: u64,
    ignore_term: bool,
    session_id: Option<String>,
    transcript: Option<PathBuf>,
    transcript_kind: TranscriptKind,
    limit_after: Option<u64>,
    report_env: Vec<String>,
}

fn parse_number<T: std::str::FromStr>(flag: &str, value: Option<String>) -> Result<T, String> {
    value
        .ok_or_else(|| format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag} needs a number"))
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        mode: Mode::Echo,
        bytes: 0,
        code: 0,
        seconds: 600,
        ignore_term: false,
        session_id: None,
        transcript: None,
        transcript_kind: TranscriptKind::Claude,
        limit_after: None,
        report_env: Vec::new(),
    };
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--mode" => {
                options.mode = match args.next().as_deref() {
                    Some("echo") => Mode::Echo,
                    Some("flood") => Mode::Flood,
                    Some("ignore-term") => Mode::IgnoreTerm,
                    Some("fork-child") => Mode::ForkChild,
                    Some("exit-early") => Mode::ExitEarly,
                    Some("sleep") => Mode::Sleep,
                    _ => return Err("unknown --mode".into()),
                }
            }
            "--bytes" => options.bytes = parse_number(&flag, args.next())?,
            "--code" => options.code = parse_number(&flag, args.next())?,
            "--seconds" => options.seconds = parse_number(&flag, args.next())?,
            "--limit-after" => options.limit_after = Some(parse_number(&flag, args.next())?),
            "--ignore-term" => options.ignore_term = true,
            "--session-id" => {
                options.session_id = Some(args.next().ok_or("--session-id needs a value")?);
            }
            "--transcript" => {
                options.transcript = Some(args.next().ok_or("--transcript needs a path")?.into());
            }
            "--transcript-kind" => {
                options.transcript_kind = match args.next().as_deref() {
                    Some("claude") => TranscriptKind::Claude,
                    Some("codex") => TranscriptKind::Codex,
                    _ => return Err("--transcript-kind must be claude or codex".into()),
                }
            }
            "--report-env" => {
                options
                    .report_env
                    .push(args.next().ok_or("--report-env needs a name")?);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if options.bytes > MAX_FLOOD_BYTES {
        return Err("--bytes exceeds 1 GiB".into());
    }
    options.seconds = options.seconds.min(MAX_SLEEP_SECONDS);
    if options.mode == Mode::IgnoreTerm {
        options.ignore_term = true;
    }
    Ok(options)
}

fn say(line: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Formats Unix milliseconds as RFC 3339 UTC with millisecond precision.
fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

struct Transcript {
    path: PathBuf,
    kind: TranscriptKind,
    session_id: String,
    cwd: String,
    turns: u64,
    total_input: u64,
    total_output: u64,
    last_uuid: Option<String>,
}

impl Transcript {
    fn open(path: PathBuf, kind: TranscriptKind, session_id: &str) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let transcript = Self {
            path,
            kind,
            session_id: session_id.to_owned(),
            cwd,
            turns: 0,
            total_input: 0,
            total_output: 0,
            last_uuid: None,
        };
        let empty = std::fs::metadata(&transcript.path).map_or(true, |m| m.len() == 0);
        if kind == TranscriptKind::Codex && empty {
            let now = rfc3339(now_ms());
            transcript.append(&[json!({
                "timestamp": now,
                "type": "session_meta",
                "payload": {
                    "id": transcript.session_id,
                    "timestamp": now,
                    "cwd": transcript.cwd,
                    "originator": "fake-agent",
                    "cli_version": "0.0.0-fake",
                    "model_provider": "fake"
                }
            })])?;
        }
        Ok(transcript)
    }

    fn append(&self, records: &[Value]) -> io::Result<()> {
        let mut text = String::new();
        for record in records {
            text.push_str(&record.to_string());
            text.push('\n');
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(text.as_bytes())?;
        file.flush()
    }

    fn claude_envelope(&self, kind: &str, uuid: &str, timestamp: &str) -> Value {
        json!({
            "type": kind,
            "parentUuid": self.last_uuid,
            "isSidechain": false,
            "uuid": uuid,
            "timestamp": timestamp,
            "userType": "external",
            "entrypoint": "cli",
            "cwd": self.cwd,
            "sessionId": self.session_id,
            "version": "0.0.0-fake",
        })
    }

    fn turn(&mut self, prompt: &str, answer: &str) -> io::Result<()> {
        self.turns += 1;
        let n = self.turns;
        let input = 10 + prompt.len() as u64;
        let output = 4 + answer.len() as u64;
        self.total_input += input;
        self.total_output += output;
        let timestamp = rfc3339(now_ms());
        let records = match self.kind {
            TranscriptKind::Claude => {
                let user_uuid = format!("fake-{}-{n}-u", self.session_id);
                let mut user = self.claude_envelope("user", &user_uuid, &timestamp);
                user["message"] = json!({"role": "user", "content": prompt});
                self.last_uuid = Some(user_uuid);
                let assistant_uuid = format!("fake-{}-{n}-a", self.session_id);
                let mut assistant = self.claude_envelope("assistant", &assistant_uuid, &timestamp);
                assistant["requestId"] = json!(format!("req_fake_{n}"));
                assistant["message"] = json!({
                    "id": format!("msg_fake_{n}"),
                    "type": "message",
                    "role": "assistant",
                    "model": "fake-model",
                    "content": [{"type": "text", "text": answer}],
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": input,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "output_tokens": output
                    }
                });
                self.last_uuid = Some(assistant_uuid);
                vec![user, assistant]
            }
            TranscriptKind::Codex => {
                let used = (10.0 * n as f64).min(99.0);
                vec![
                    json!({"timestamp": timestamp, "type": "event_msg",
                        "payload": {"type": "task_started"}}),
                    json!({"timestamp": timestamp, "type": "response_item", "payload": {
                        "type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": prompt}]}}),
                    json!({"timestamp": timestamp, "type": "response_item", "payload": {
                        "type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": answer}]}}),
                    self.codex_token_count(&timestamp, input, output, used, None),
                    json!({"timestamp": timestamp, "type": "event_msg", "payload": {
                        "type": "task_complete", "last_agent_message": answer,
                        "duration_ms": 1}}),
                ]
            }
        };
        self.append(&records)
    }

    fn codex_token_count(
        &self,
        timestamp: &str,
        input: u64,
        output: u64,
        used_percent: f64,
        reached: Option<&str>,
    ) -> Value {
        let resets_at = now_ms() / 1000 + 3600;
        json!({"timestamp": timestamp, "type": "event_msg", "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": self.total_input, "cached_input_tokens": 0,
                    "output_tokens": self.total_output, "reasoning_output_tokens": 0,
                    "total_tokens": self.total_input + self.total_output
                },
                "last_token_usage": {
                    "input_tokens": input, "cached_input_tokens": 0,
                    "output_tokens": output, "reasoning_output_tokens": 0,
                    "total_tokens": input + output
                },
                "model_context_window": 200_000
            },
            "rate_limits": {
                "primary": {"used_percent": used_percent, "window_minutes": 300,
                    "resets_at": resets_at},
                "secondary": null,
                "plan_type": "fake",
                "rate_limit_reached_type": reached
            }
        }})
    }

    fn limit(&mut self) -> io::Result<()> {
        let timestamp = rfc3339(now_ms());
        let resets_at = now_ms() / 1000 + 3600;
        let record = match self.kind {
            TranscriptKind::Claude => {
                let uuid = format!("fake-{}-limit", self.session_id);
                let mut record = self.claude_envelope("assistant", &uuid, &timestamp);
                record["isApiErrorMessage"] = json!(true);
                record["error"] = json!("rate_limit");
                record["message"] = json!({
                    "id": format!("msg_fake_limit_{}", self.turns),
                    "type": "message",
                    "role": "assistant",
                    "model": "<synthetic>",
                    "content": [{"type": "text",
                        "text": format!("Claude AI usage limit reached|{resets_at}")}],
                    "stop_reason": "stop_sequence",
                    "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0, "output_tokens": 0}
                });
                self.last_uuid = Some(uuid);
                record
            }
            TranscriptKind::Codex => {
                self.codex_token_count(&timestamp, 0, 0, 100.0, Some("primary"))
            }
        };
        self.append(&[record])
    }
}

fn report_env(names: &[String]) -> io::Result<()> {
    for name in names {
        match std::env::var_os(name) {
            Some(value) => say(&format!("ENV {name}=set len={}", value.len()))?,
            None => say(&format!("ENV {name}=unset"))?,
        }
    }
    Ok(())
}

fn ignore_termination() -> io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&flag))?;
    }
    Ok(flag)
}

fn flood(bytes: u64) -> io::Result<()> {
    let mut line = Vec::with_capacity(64 * 1024);
    let mut counter = 0u64;
    while line.len() + 64 <= 64 * 1024 {
        let text = format!("FLOOD {counter:010} ");
        line.extend_from_slice(text.as_bytes());
        line.extend(std::iter::repeat_n(b'x', 63 - text.len()));
        line.push(b'\n');
        counter += 1;
    }
    for target in 0..2 {
        let mut remaining = bytes;
        while remaining > 0 {
            let n = usize::try_from(remaining.min(line.len() as u64)).unwrap_or(line.len());
            let chunk = line.get(..n).unwrap_or(&line);
            if target == 0 {
                io::stdout().lock().write_all(chunk)?;
            } else {
                io::stderr().lock().write_all(chunk)?;
            }
            remaining -= n as u64;
        }
        if target == 0 {
            io::stdout().flush()?;
        }
    }
    say("FLOOD DONE")
}

fn spawn_descendant(options: &Options) -> io::Result<u32> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command.args(["--mode", "sleep", "--seconds", &options.seconds.to_string()]);
    if options.ignore_term {
        command.arg("--ignore-term");
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    // Deliberately not waited for: the descendant must outlive this process for tests.
    Ok(child.id())
}

fn terminal_size() -> String {
    match Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => "unknown".to_owned(),
    }
}

fn read_line(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = input.take(MAX_LINE).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&buf);
    Ok(Some(text.trim_end_matches(['\n', '\r']).to_owned()))
}

fn echo_loop(options: &Options, session_id: &str) -> io::Result<i32> {
    let mut transcript = match &options.transcript {
        Some(path) => Some(Transcript::open(
            path.clone(),
            options.transcript_kind,
            session_id,
        )?),
        None => None,
    };
    let mut turns = 0u64;
    let mut limited = false;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    while let Some(line) = read_line(&mut input)? {
        match line.as_str() {
            "/exit" => {
                say("BYE")?;
                return Ok(0);
            }
            "/crash" => {
                say("CRASHING")?;
                return Ok(3);
            }
            "/size" => say(&format!("SIZE {}", terminal_size()))?,
            _ if line.starts_with("/flood ") => {
                let bytes = line
                    .get("/flood ".len()..)
                    .and_then(|n| n.trim().parse::<u64>().ok())
                    .unwrap_or(0)
                    .min(MAX_FLOOD_BYTES);
                flood(bytes)?;
            }
            _ if line.starts_with("/read ") => {
                // Containment probe: reports whether a path is readable, never its contents.
                let path = line["/read ".len()..].trim();
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match std::fs::read(path) {
                    Ok(bytes) => say(&format!("READ {name} OK {}", bytes.len()))?,
                    Err(err) => say(&format!("READ {name} ERR {:?}", err.kind()))?,
                }
            }
            _ if line.starts_with("/later ") => {
                let mut parts = line.splitn(3, ' ').skip(1);
                let delay = parts
                    .next()
                    .and_then(|ms| ms.parse::<u64>().ok())
                    .unwrap_or(0)
                    .min(MAX_LATER_MS);
                let text = parts.next().unwrap_or("").to_owned();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(delay));
                    let _ = say(&text);
                });
            }
            _ if limited => say("LIMIT REACHED")?,
            _ => {
                let answer = format!("ECHO: {line}");
                say(&answer)?;
                turns += 1;
                if let Some(transcript) = transcript.as_mut() {
                    transcript.turn(&line, &answer)?;
                }
                if options.limit_after.is_some_and(|limit| turns >= limit) {
                    limited = true;
                    say("LIMIT REACHED")?;
                    if let Some(transcript) = transcript.as_mut() {
                        transcript.limit()?;
                    }
                }
            }
        }
    }
    Ok(0)
}

fn run(options: &Options) -> io::Result<i32> {
    let _ignored = if options.ignore_term {
        Some(ignore_termination()?)
    } else {
        None
    };
    let session_id = options
        .session_id
        .clone()
        .unwrap_or_else(|| format!("fake-{}", std::process::id()));
    match options.mode {
        Mode::Sleep => {
            say("SLEEPING")?;
            let deadline = std::time::Instant::now() + Duration::from_secs(options.seconds);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(200));
            }
            return Ok(0);
        }
        Mode::ExitEarly => {
            report_env(&options.report_env)?;
            say(&format!("EXITING code={}", options.code))?;
            return Ok(options.code);
        }
        _ => {}
    }
    say(&format!("FAKE-AGENT READY session={session_id}"))?;
    report_env(&options.report_env)?;
    if options.mode == Mode::ForkChild {
        let pid = spawn_descendant(options)?;
        say(&format!("CHILD {pid}"))?;
    }
    if options.mode == Mode::Flood {
        flood(options.bytes)?;
    }
    echo_loop(options, &session_id)
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("fake-agent: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(code) => ExitCode::from(u8::try_from(code.rem_euclid(256)).unwrap_or(1)),
        Err(error) => {
            eprintln!("fake-agent: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(951_782_400_123), "2000-02-29T00:00:00.123Z");
        assert_eq!(rfc3339(1_791_368_133_456), "2026-10-07T10:15:33.456Z");
    }

    #[test]
    fn arguments_are_validated() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|s| (*s).to_owned()));
        assert!(parse(&["--mode", "bogus"]).is_err());
        assert!(parse(&["--bytes"]).is_err());
        assert!(parse(&["--bytes", "x"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
        assert!(parse(&["--mode", "ignore-term"]).is_ok_and(|o| o.ignore_term));
        assert!(parse(&["--seconds", "999999"]).is_ok_and(|o| o.seconds == MAX_SLEEP_SECONDS));
    }
}
