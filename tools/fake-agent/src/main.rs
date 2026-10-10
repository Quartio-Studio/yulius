//! Deterministic fake coding-agent CLI used by Yulius supervisor, harness and server tests.
//! Owned by work package A.
//!
//! The binary is built at `target/<profile>/fake-agent`; tests in other crates locate it with
//! `yulius_ptyhost::test_support::locate_fake_agent` (feature `test-support`).
//!
//! ```text
//! fake-agent [--mode echo|flood|ignore-term|fork-child|exit-early|sleep]
//!            [--bytes N] [--code N] [--seconds N] [--ignore-term]
//!            [--session-id ID] [--transcript PATH [--transcript-kind claude]]
//!            [--limit-after N] [--report-env NAME]... [--tui]
//!            [--hooks YULIUS_EXE] [--goal-background SECONDS] [--goal-delay-ms MS]
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
//! (`--transcript-kind claude`, the only kind). `--limit-after N` prints
//! `LIMIT REACHED` and appends a usage-limit record after N turns, shaped like Claude Code
//! 2.1.292's (`apiError: "usage_limit_reached"`, `resetsAt` one hour ahead); later turns only repeat
//! `LIMIT REACHED`. `--report-env NAME` prints `ENV NAME=set len=<n>` or `ENV NAME=unset` (never
//! the value).
//!
//! Goal mode (TSK-010/TSK-014), like Claude Code 2.1.292: `/goal <condition>` prints `GOAL SET`,
//! appends the goal's sentinel record, then (after `--goal-delay-ms`, default 0) runs one turn and
//! records the goal as achieved (`goal_status` with `met: true`, after the turn's Stop hook) and
//! prints `✓ Goal achieved`. With `--goal-background N` the goal turn first starts a background
//! shell shaped like Claude Code's Bash tool (`/bin/sh -c "sleep N && pwd -P >| /dev/null"`, a
//! child of this process) and prints `BACKGROUND <pid>`; when it exits the agent is woken like
//! Claude Code wakes it (a `queued_command` task notification and a short turn with its Stop
//! hook) and prints `BACKGROUND DONE`.
//!
//! `--hooks YULIUS_EXE` runs Claude Code's hooks as configured by Yulius:
//! `YULIUS_EXE hook claude UserPromptSubmit` before and `… Stop` after each turn, with the hook
//! JSON on stdin (the hook client reads `YULIUS_URL` and `YULIUS_TASK_TOKEN` from the inherited
//! environment). A Stop answer that blocks prints `STOP BLOCKED`.
//!
//! `--tui` draws like a full-screen agent TUI (Claude Code 2.1.x): the alternate screen with a
//! header box, the transcript and an input box between two rules along the bottom, all as wide
//! as the terminal, redrawn in full whenever the terminal is resized (SIGWINCH). Printed lines
//! go into the transcript. Used to measure how the web terminal settles (F 2026-10-09).

use std::fs::OpenOptions;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const MAX_LINE: u64 = 64 * 1024;
const MAX_FLOOD_BYTES: u64 = 1 << 30;
const MAX_SLEEP_SECONDS: u64 = 3600;
const MAX_LATER_MS: u64 = 60_000;
const MAX_GOAL_DELAY_MS: u64 = 60_000;
const MAX_HOOK_OUTPUT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Echo,
    Flood,
    IgnoreTerm,
    ForkChild,
    ExitEarly,
    Sleep,
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
    limit_after: Option<u64>,
    report_env: Vec<String>,
    tui: bool,
    hooks: Option<PathBuf>,
    goal_background: Option<u64>,
    goal_delay_ms: u64,
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
        limit_after: None,
        report_env: Vec::new(),
        tui: false,
        hooks: None,
        goal_background: None,
        goal_delay_ms: 0,
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
            "--tui" => options.tui = true,
            "--session-id" => {
                options.session_id = Some(args.next().ok_or("--session-id needs a value")?);
            }
            "--transcript" => {
                options.transcript = Some(args.next().ok_or("--transcript needs a path")?.into());
            }
            "--transcript-kind" => {
                if args.next().as_deref() != Some("claude") {
                    return Err("--transcript-kind must be claude".into());
                }
            }
            "--report-env" => {
                options
                    .report_env
                    .push(args.next().ok_or("--report-env needs a name")?);
            }
            "--hooks" => {
                options.hooks = Some(args.next().ok_or("--hooks needs a path")?.into());
            }
            "--goal-background" => {
                let seconds: u64 = parse_number(&flag, args.next())?;
                options.goal_background = Some(seconds.min(MAX_SLEEP_SECONDS));
            }
            "--goal-delay-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.goal_delay_ms = ms.min(MAX_GOAL_DELAY_MS);
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
    if let Some(tui) = TUI.get() {
        return tui.say(line);
    }
    let mut out = io::stdout().lock();
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Most transcript lines kept by the TUI.
const TUI_KEEP: usize = 500;

/// The full-screen drawing of `--tui`: the transcript and the session id.
struct Tui {
    lines: Mutex<Vec<String>>,
    session: String,
}

static TUI: OnceLock<Tui> = OnceLock::new();

impl Tui {
    fn say(&self, line: &str) -> io::Result<()> {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(line.to_owned());
            let excess = lines.len().saturating_sub(TUI_KEEP);
            lines.drain(..excess);
        }
        self.draw()
    }

    /// Redraws the whole screen at the terminal's current size.
    fn draw(&self) -> io::Result<()> {
        let (rows, cols) = terminal_rows_cols().unwrap_or((24, 80));
        let rows = rows.max(8);
        let cols = cols.max(20);
        let fit = |text: &str| -> String { text.chars().take(cols).collect() };
        let pad = |text: &str, width: usize| -> String {
            let mut out: String = text.chars().take(width).collect();
            let used = out.chars().count();
            out.extend(std::iter::repeat_n(' ', width - used));
            out
        };
        let rule = "─".repeat(cols);
        let mut screen = String::from("\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J");
        screen.push_str(&format!("╭{}╮\r\n", "─".repeat(cols - 2)));
        screen.push_str(&format!(
            "│{}│\r\n",
            pad(
                &format!(" ✻ Fake Claude Code · session {}", self.session),
                cols - 2
            )
        ));
        screen.push_str(&format!("╰{}╯", "─".repeat(cols - 2)));
        let body = rows - 3 - 4;
        if let Ok(lines) = self.lines.lock() {
            let start = lines.len().saturating_sub(body);
            for (i, line) in lines.iter().skip(start).enumerate() {
                screen.push_str(&format!("\x1b[{};1H{}", 4 + i, fit(line)));
            }
        }
        let status = format!(" → fixture git:(main) · {cols}×{rows}");
        screen.push_str(&format!(
            "\x1b[{};1H{rule}\x1b[{};1H❯ \x1b[{};1H{rule}\x1b[{};1H{}",
            rows - 3,
            rows - 2,
            rows - 1,
            rows,
            fit(&status)
        ));
        screen.push_str(&format!("\x1b[{};3H\x1b[?25h", rows - 2));
        let mut out = io::stdout().lock();
        out.write_all(screen.as_bytes())?;
        out.flush()
    }
}

/// Starts the TUI and its redraw-on-resize thread.
fn start_tui(session: &str) -> io::Result<()> {
    let resized = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&resized))?;
    let tui = TUI.get_or_init(|| Tui {
        lines: Mutex::new(Vec::new()),
        session: session.to_owned(),
    });
    tui.draw()?;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(20));
            if resized.swap(false, Ordering::SeqCst)
                && let Some(tui) = TUI.get()
            {
                let _ = tui.draw();
            }
        }
    });
    Ok(())
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
    session_id: String,
    cwd: String,
    turns: u64,
    last_uuid: Option<String>,
}

impl Transcript {
    fn open(path: PathBuf, session_id: &str) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Ok(Self {
            path,
            session_id: session_id.to_owned(),
            cwd,
            turns: 0,
            last_uuid: None,
        })
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
        let timestamp = rfc3339(now_ms());
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
        self.append(&[user, assistant])
    }

    /// A `goal_status` attachment shaped like Claude Code 2.1.292's.
    fn goal_status(&mut self, goal: Value) -> io::Result<()> {
        let uuid = format!("fake-{}-goal-{}", self.session_id, self.turns);
        let mut record = self.claude_envelope("attachment", &uuid, &rfc3339(now_ms()));
        record["attachment"] = goal;
        self.last_uuid = Some(uuid);
        self.append(&[record])
    }

    /// The `system` records Claude Code writes when a turn closes after its Stop hooks.
    fn close_turn(&mut self, duration_ms: u64) -> io::Result<()> {
        let timestamp = rfc3339(now_ms());
        let summary_uuid = format!("fake-{}-{}-stop", self.session_id, self.turns);
        let mut summary = self.claude_envelope("system", &summary_uuid, &timestamp);
        summary["subtype"] = json!("stop_hook_summary");
        summary["hookCount"] = json!(2);
        summary["hookErrors"] = json!([]);
        summary["preventedContinuation"] = json!(false);
        let duration_uuid = format!("fake-{}-{}-duration", self.session_id, self.turns);
        let mut duration = self.claude_envelope("system", &duration_uuid, &timestamp);
        duration["subtype"] = json!("turn_duration");
        duration["durationMs"] = json!(duration_ms);
        self.last_uuid = Some(duration_uuid);
        self.append(&[summary, duration])
    }

    /// A background shell ended: Claude Code queues a task notification and wakes the agent.
    fn notification_turn(&mut self, pid: u32) -> io::Result<()> {
        self.turns += 1;
        let n = self.turns;
        let timestamp = rfc3339(now_ms());
        let queued_uuid = format!("fake-{}-{n}-q", self.session_id);
        let mut queued = self.claude_envelope("attachment", &queued_uuid, &timestamp);
        queued["attachment"] = json!({
            "type": "queued_command",
            "commandMode": "task-notification",
            "prompt": format!("<task-notification>\n<task-id>bash-{pid}</task-id>\n<status>completed</status>\n</task-notification>"),
            "origin": {"kind": "task-notification", "producer": "session-task"},
        });
        let assistant_uuid = format!("fake-{}-{n}-a", self.session_id);
        let mut assistant = self.claude_envelope("assistant", &assistant_uuid, &timestamp);
        assistant["message"] = json!({
            "id": format!("msg_fake_{n}"),
            "type": "message",
            "role": "assistant",
            "model": "fake-model",
            "content": [{"type": "text", "text": "The background command finished."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 8, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0, "output_tokens": 6}
        });
        self.last_uuid = Some(assistant_uuid);
        self.append(&[queued, assistant])
    }

    fn limit(&mut self) -> io::Result<()> {
        let timestamp = rfc3339(now_ms());
        let resets_at = now_ms() / 1000 + 3600;
        let uuid = format!("fake-{}-limit", self.session_id);
        let mut record = self.claude_envelope("assistant", &uuid, &timestamp);
        record["isApiErrorMessage"] = json!(true);
        record["error"] = json!("rate_limit");
        record["apiError"] = json!("usage_limit_reached");
        record["apiErrorStatus"] = json!(429);
        let info =
            json!({"status": "rejected", "resetsAt": resets_at, "rateLimitType": "five_hour"});
        record["apiErrorParams"] = json!({"rate_limit_info": info.clone()});
        record["quotaLimits"] = info;
        record["message"] = json!({
            "id": format!("msg_fake_limit_{}", self.turns),
            "type": "message",
            "role": "assistant",
            "model": "<synthetic>",
            "content": [{"type": "text",
                "text": "You've hit your session limit · resets in 1 hour"}],
            "stop_reason": "stop_sequence",
            "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0, "output_tokens": 0}
        });
        self.last_uuid = Some(uuid);
        self.append(&[record])
    }
}

/// Runs one Claude Code hook through Yulius' hook client; returns its output (empty when none).
fn run_hook(exe: &Path, event: &str, input: &Value) -> String {
    let child = Command::new(exe)
        .args(["hook", "claude", event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return String::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.to_string().as_bytes());
    }
    let mut output = String::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(MAX_HOOK_OUTPUT).read_to_string(&mut output);
    }
    let _ = child.wait();
    output
}

/// The hooks of one session, when `--hooks` is given.
#[derive(Clone)]
struct Hooks {
    exe: PathBuf,
    session_id: String,
}

impl Hooks {
    fn prompt(&self, prompt: &str) {
        run_hook(
            &self.exe,
            "UserPromptSubmit",
            &json!({"session_id": self.session_id, "hook_event_name": "UserPromptSubmit",
                "prompt": prompt}),
        );
    }

    fn stop(&self) -> io::Result<()> {
        let answer = run_hook(
            &self.exe,
            "Stop",
            &json!({"session_id": self.session_id, "hook_event_name": "Stop",
                "stop_hook_active": false}),
        );
        if answer.contains("\"block\"") {
            say("STOP BLOCKED")?;
        }
        Ok(())
    }
}

/// Starts a background shell shaped like Claude Code's Bash tool (`run_in_background`) and wakes
/// the agent when it exits, like Claude Code does.
fn start_background(
    seconds: u64,
    transcript: Option<Arc<Mutex<Transcript>>>,
    hooks: Option<Hooks>,
) -> io::Result<u32> {
    let mut child = Command::new("/bin/sh")
        .args(["-c", &format!("sleep {seconds} && pwd -P >| /dev/null")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
        if let Some(transcript) = &transcript
            && let Ok(mut transcript) = transcript.lock()
        {
            let _ = transcript.notification_turn(pid);
        }
        let _ = say("BACKGROUND DONE");
        if let Some(hooks) = &hooks {
            let _ = hooks.stop();
        }
    });
    Ok(pid)
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

/// The terminal's size as (rows, cols), from `stty size`.
fn terminal_rows_cols() -> Option<(usize, usize)> {
    let size = terminal_size();
    let mut parts = size.split_whitespace().map(str::parse::<usize>);
    match (parts.next(), parts.next()) {
        (Some(Ok(rows)), Some(Ok(cols))) => Some((rows, cols)),
        _ => None,
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

/// One `/goal` turn (see the module docs).
fn goal_turn(
    options: &Options,
    line: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
) -> io::Result<()> {
    let condition = line
        .get("/goal ".len()..)
        .unwrap_or_default()
        .trim()
        .to_owned();
    say("GOAL SET")?;
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.goal_status(
            json!({"type": "goal_status", "met": false, "sentinel": true,
            "condition": condition}),
        )?;
    }
    if let Some(hooks) = hooks {
        hooks.prompt(line);
    }
    if let Some(seconds) = options.goal_background {
        let pid = start_background(seconds, transcript.cloned(), hooks.cloned())?;
        say(&format!("BACKGROUND {pid}"))?;
    }
    std::thread::sleep(Duration::from_millis(options.goal_delay_ms));
    let answer = format!("ECHO: {line}");
    say(&answer)?;
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.turn(line, &answer)?;
    }
    // Claude Code's goal evaluator is a Stop hook: its record follows the turn's Stop hooks.
    if let Some(hooks) = hooks {
        hooks.stop()?;
    }
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.goal_status(
            json!({"type": "goal_status", "met": true, "condition": condition,
            "reason": "Done and verified.", "iterations": 1, "durationMs": 51_000,
            "tokens": 7_100}),
        )?;
        t.close_turn(51_000)?;
    }
    say("✓ Goal achieved")
}

fn echo_loop(options: &Options, session_id: &str) -> io::Result<i32> {
    let transcript = match &options.transcript {
        Some(path) => Some(Arc::new(Mutex::new(Transcript::open(
            path.clone(),
            session_id,
        )?))),
        None => None,
    };
    let hooks = options.hooks.as_ref().map(|exe| Hooks {
        exe: exe.clone(),
        session_id: session_id.to_owned(),
    });
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
            _ if line.starts_with("/goal ") && (transcript.is_some() || hooks.is_some()) => {
                turns += 1;
                goal_turn(options, &line, transcript.as_ref(), hooks.as_ref())?;
            }
            _ => {
                if let Some(hooks) = &hooks {
                    hooks.prompt(&line);
                }
                let answer = format!("ECHO: {line}");
                say(&answer)?;
                turns += 1;
                if let Some(transcript) = &transcript
                    && let Ok(mut t) = transcript.lock()
                {
                    t.turn(&line, &answer)?;
                }
                if options.limit_after.is_some_and(|limit| turns >= limit) {
                    limited = true;
                    say("LIMIT REACHED")?;
                    if let Some(transcript) = &transcript
                        && let Ok(mut t) = transcript.lock()
                    {
                        t.limit()?;
                    }
                } else if let Some(hooks) = &hooks {
                    hooks.stop()?;
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
    if options.tui {
        start_tui(&session_id)?;
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
        assert!(parse(&["--tui"]).is_ok_and(|o| o.tui));
        assert!(parse(&["--seconds", "999999"]).is_ok_and(|o| o.seconds == MAX_SLEEP_SECONDS));
        assert!(parse(&["--hooks"]).is_err());
        assert!(
            parse(&["--goal-background", "3", "--goal-delay-ms", "999999"])
                .is_ok_and(|o| o.goal_background == Some(3) && o.goal_delay_ms == MAX_GOAL_DELAY_MS)
        );
    }
}
