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
//!            [--hooks YULIUS_EXE] [--goal-background SECONDS]
//!            [--goal-agent SECONDS [--goal-agent-interim SECONDS]]
//!            [--goal-monitor SECONDS [--goal-monitor-ends]] [--goal-verdict met|failed|none]
//!            [--goal-met-on-wake-ms MS [--wake-stop-lost]] [--wake-delay-ms MS]
//!            [--goal-continue-ms MS
//!            [--goal-continue-verdict] [--goal-continue-dies]] [--goal-delay-ms MS] [--drop-lines N]
//!            [--interrupts] [--rate-limits-file PATH] [--session-start] [--opening-turn-ms MS]
//!            [--echo-turn-ms MS] [--context-full-after N] [--compact-fails] [--compact-hangs]
//!            [--prompt TEXT] [--limit-continues-ms MS [--limit-continues-answer-only]]
//!            [--idle-compact-ms MS] [--trust-prompt]
//! ```
//!
//! Modes:
//! - `echo` (default): prints `FAKE-AGENT READY session=<id>`, then answers each input line with
//!   `ECHO: <line>`. `/exit` exits 0, `/crash` exits 3, `/later <ms> <text>` prints `<text>` after
//!   a delay (≤ 60 s), `/size` prints `SIZE <rows> <cols>` of its terminal (via `stty size`),
//!   `/read <path>` prints `READ <file name> OK <n>` or `READ <file name> ERR <kind>` (containment
//!   probe; never the contents), `/sh <command>` runs `/bin/sh -c <command>` on the terminal and
//!   prints `SH EXIT <code>` (like an agent's Bash tool),
//!   `/flood <n>` floods like `--mode flood` on demand. `/clear <id>` starts session `<id>` in the
//!   same process as Claude Code's `/clear` does: `SessionEnd` (`reason: clear`) for the old
//!   session, a transcript `<id>.jsonl` next to the old one, `SessionStart` (`source: clear`)
//!   with the new id, then `CLEARED session=<id>`.
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
//! `LIMIT REACHED`. With `--limit-continues-ms MS` the agent then continues the cut turn by itself
//! like Claude Code's "continuing automatically" at the reset (#276): MS later it appends a hidden
//! `isMeta` prompt and an assistant `tool_use` record and prints `CONTINUED`, and MS after that it
//! appends the answer ending the turn and prints `DONE continued`; with
//! `--limit-continues-answer-only` the continuation is only that answer (no tool call). With
//! `--hooks` the continued turn then runs its Stop hook and prints `CONTINUED STOP ANSWERED`.
//! `--report-env NAME` prints `ENV NAME=set len=<n>` or `ENV NAME=unset` (never
//! the value).
//!
//! Goal mode (TSK-010/TSK-014), like Claude Code 2.1.292: `/goal <condition>` prints `GOAL SET`,
//! appends the goal's sentinel record, then (after `--goal-delay-ms`, default 0) runs one turn and
//! records the goal as achieved (`goal_status` with `met: true`, after the turn's Stop hook) and
//! prints `✓ Goal achieved`. With `--goal-background N` the goal turn first starts a background
//! shell shaped like Claude Code's Bash tool (`/bin/sh -c "sleep N && pwd -P >| /dev/null"`, a
//! child of this process) and prints `BACKGROUND <pid>`; when it exits the agent is woken like
//! Claude Code wakes an idle session (a `<task-notification>` user prompt and a short turn with
//! its Stop hook) and prints `BACKGROUND DONE`. With `--goal-agent N` the goal turn launches a background
//! agent (its tool result `{"status":"async_launched","agentId":"agent-1"}` before the Stop hook)
//! that wakes the session after N seconds and prints `AGENT DONE`. With `--goal-agent-interim M`
//! too, the agent first stops with background work of its own still running: after N seconds its
//! notification says so in Claude Code's note (the result may be interim), wakes the session and
//! prints `AGENT INTERIM`; M seconds later its final notification prints `AGENT DONE`. With
//! `--goal-monitor N` it starts a Monitor stream (`{"taskId":"monitor-1","timeoutMs":N000}`) that never reports; with
//! `--goal-monitor-ends` too, the stream ends after N/2 seconds and wakes the session
//! (`MONITOR DONE`). `--goal-verdict failed` records the goal as failed instead of achieved, and
//! `--goal-verdict none` records no verdict (the turn closes without one). With
//! `--goal-met-on-wake-ms MS` and `--goal-background N`, the goal turn gets no verdict (Claude
//! Code's evaluator waits while background work runs) and the turn the shell's end wakes records
//! the goal as achieved MS after its Stop hook (the evaluator's own time), then closes; with
//! `--wake-stop-lost` too, that turn's Stop hook never reaches Yulius (the hook client fails open,
//! as on an error answer or an unreachable server). `--wake-delay-ms MS` wakes the session MS after
//! the background shell exits instead of at once, as Claude Code notices it only when it polls
//! its background tasks. With
//! `--idle-compact-ms MS` the idle session compacts itself MS after the goal turn, like Claude
//! Code 2.1.292 "before the prompt cache expired": the `compact_boundary` record, the summary
//! user record it continues from (`isCompactSummary`), the files it reads again, its
//! `SessionStart` hook (`source: compact`) and its notice, then it prints `COMPACTED WHILE IDLE`.
//! No turn runs.
//!
//! `--hooks YULIUS_EXE` runs Claude Code's hooks as configured by Yulius:
//! `YULIUS_EXE hook claude UserPromptSubmit` before and `… Stop` after each turn, with the hook
//! JSON on stdin (the hook client reads `YULIUS_URL` and `YULIUS_TASK_TOKEN` from the inherited
//! environment). A Stop answer that blocks prints `STOP BLOCKED`. A `UserPromptSubmit` answer
//! that blocks keeps the prompt from the agent like Claude Code: it prints
//! `PROMPT BLOCKED: <reason>` and runs no turn. `/exit` runs `SessionEnd` with
//! `reason: prompt_input_exit` first.
//!
//! `/work <ms> <text>` is a turn that takes `<ms>` (≤ 60 s): it prints `WORKING`, then `DONE
//! <text>` with its transcript records and Stop hook. With `--interrupts` the agent handles
//! Ctrl+C like Claude Code 2.1.292: during a turn it interrupts it (prints `INTERRUPTED`, writes
//! the user line "[Request interrupted by user]", no Stop hook); at the prompt it prints `Press
//! Ctrl-C again to exit`, and a second Ctrl+C within 2 s runs `SessionEnd`
//! (`prompt_input_exit`), prints `BYE` and exits 0; SIGTERM runs `SessionEnd` (`other`) and
//! exits 143.
//!
//! `--rate-limits-file PATH` runs Claude Code's status line command as Yulius configures it
//! (`YULIUS_EXE hook claude StatusLine`, with `--hooks`) at the start and after every turn, with
//! the `rate_limits` object read from PATH (when it exists) in its input, and prints
//! `STATUS <line>`.
//!
//! `--trust-prompt` opens like Claude Code 2.1.292 in a folder it does not trust (SPEC-106): its
//! "Quick safety check" dialog, then one line read as the answer, which prints `TRUST ANSWERED:
//! <line>` on a cleared screen; only then does the rest start (the `SessionStart` hook too).
//!
//! `--session-start` (with `--hooks`) runs Claude Code's `SessionStart` hook before anything else.
//! `--opening-turn-ms MS` stands for a first message given on the command line (Claude Code's
//! prompt argument): the session starts with a turn like `/work MS opening` (`WORKING`, then
//! `DONE opening` and its Stop hook) before it reads input. With the terminal's echo, a line typed
//! during that turn shows before `DONE opening`.
//!
//! `--echo-turn-ms MS` makes each echoed message a turn that takes MS (≤ 60 s) before its `ECHO:`
//! answer and Stop hook, silent meanwhile (a turn on a typed first message, SPEC-030).
//!
//! `--context-full-after N` fills the session's context after N echoed messages (0: from the
//! first), like Claude Code 2.1.292 when the API refuses a prompt as too long (#213): each later
//! message prints `PROMPT IS TOO LONG` and appends its user line and the API-error record
//! (`error: "invalid_request"`, text `Prompt is too long`), with no Stop hook. `/compact <focus>`
//! appends the command as typed (a user line with no `origin`), then, like Claude Code 2.1.292,
//! the `compact_boundary` record, the summary user record (`isCompactSummary`), the command's
//! caveat, echo and output ("Compacted"), prints `COMPACTED: <focus>` and empties the context for
//! good; it runs no hook and no turn. With `--compact-fails` it prints `COMPACTION FAILED`, appends the command's error output
//! ("Error during compaction") and the context stays full; with `--compact-hangs` it prints
//! `COMPACTING` and never finishes. A refusal takes `--echo-turn-ms` after its user line, like
//! the API's answer. `--prompt TEXT` is a first message
//! given on the command line (Claude Code's prompt argument), answered like a typed one before
//! input is read; a `/goal <condition>` one sets the goal and runs its turn, like Claude Code.
//!
//! Bracketed pastes (`ESC [200~ … ESC [201~`) are read like Claude Code 2.1.292 reads them: a
//! pasted path ending in `.png`, `.jpg`, `.jpeg`, `.gif` or `.webp` becomes `[Image #N] ` (N
//! counts the images of the message), other pasted text is kept as typed, newlines included; the
//! message is submitted by the Enter after it. Focus reports (`ESC [ I` / `ESC [ O`) and SGR or
//! X10 mouse reports are dropped from a message, as Claude Code consumes them.
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
const MAX_WORK_MS: u64 = 60_000;
/// Two Ctrl+C at the prompt within this time exit (Claude Code's "Press Ctrl-C again to exit").
const DOUBLE_CTRL_C: Duration = Duration::from_secs(2);
/// The user line Claude Code 2.1.292 writes when the person interrupts a turn.
const INTERRUPT_LINE: &str = "[Request interrupted by user]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Echo,
    Flood,
    IgnoreTerm,
    ForkChild,
    ExitEarly,
    Sleep,
}

/// The goal evaluator's last verdict in a goal turn (`--goal-verdict`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoalVerdict {
    Met,
    Failed,
    None,
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
    goal_agent: Option<u64>,
    goal_agent_interim: Option<u64>,
    goal_monitor: Option<u64>,
    goal_monitor_ends: bool,
    goal_verdict: GoalVerdict,
    goal_continue_ms: Option<u64>,
    goal_continue_verdict: bool,
    goal_continue_dies: bool,
    goal_delay_ms: u64,
    goal_met_on_wake_ms: Option<u64>,
    wake_stop_lost: bool,
    wake_delay_ms: u64,
    opening_turn_ms: Option<u64>,
    echo_turn_ms: u64,
    session_start: bool,
    drop_lines: u32,
    interrupts: bool,
    rate_limits_file: Option<PathBuf>,
    context_full_after: Option<u64>,
    compact_fails: bool,
    compact_hangs: bool,
    prompt: Option<String>,
    limit_continues_ms: Option<u64>,
    limit_continues_answer_only: bool,
    idle_compact_ms: Option<u64>,
    trust_prompt: bool,
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
        goal_agent: None,
        goal_agent_interim: None,
        goal_monitor: None,
        goal_monitor_ends: false,
        goal_verdict: GoalVerdict::Met,
        goal_continue_ms: None,
        goal_continue_verdict: false,
        goal_continue_dies: false,
        goal_delay_ms: 0,
        goal_met_on_wake_ms: None,
        wake_stop_lost: false,
        wake_delay_ms: 0,
        opening_turn_ms: None,
        echo_turn_ms: 0,
        session_start: false,
        drop_lines: 0,
        interrupts: false,
        rate_limits_file: None,
        context_full_after: None,
        compact_fails: false,
        compact_hangs: false,
        prompt: None,
        limit_continues_ms: None,
        limit_continues_answer_only: false,
        idle_compact_ms: None,
        trust_prompt: false,
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
            "--goal-agent" => {
                let seconds: u64 = parse_number(&flag, args.next())?;
                options.goal_agent = Some(seconds.min(MAX_SLEEP_SECONDS));
            }
            "--goal-agent-interim" => {
                let seconds: u64 = parse_number(&flag, args.next())?;
                options.goal_agent_interim = Some(seconds.min(MAX_SLEEP_SECONDS));
            }
            "--goal-monitor" => {
                let seconds: u64 = parse_number(&flag, args.next())?;
                options.goal_monitor = Some(seconds.min(MAX_SLEEP_SECONDS));
            }
            "--goal-monitor-ends" => options.goal_monitor_ends = true,
            "--goal-verdict" => {
                options.goal_verdict = match args.next().as_deref() {
                    Some("met") => GoalVerdict::Met,
                    Some("failed") => GoalVerdict::Failed,
                    Some("none") => GoalVerdict::None,
                    _ => return Err("--goal-verdict must be met, failed or none".into()),
                }
            }
            "--goal-continue-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.goal_continue_ms = Some(ms.min(MAX_GOAL_DELAY_MS));
            }
            "--goal-continue-verdict" => options.goal_continue_verdict = true,
            "--idle-compact-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.idle_compact_ms = Some(ms.min(MAX_GOAL_DELAY_MS));
            }
            "--goal-continue-dies" => options.goal_continue_dies = true,
            "--goal-met-on-wake-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.goal_met_on_wake_ms = Some(ms.min(MAX_GOAL_DELAY_MS));
            }
            "--wake-stop-lost" => options.wake_stop_lost = true,
            "--wake-delay-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.wake_delay_ms = ms.min(MAX_GOAL_DELAY_MS);
            }
            "--goal-delay-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.goal_delay_ms = ms.min(MAX_GOAL_DELAY_MS);
            }
            "--drop-lines" => options.drop_lines = parse_number(&flag, args.next())?,
            "--interrupts" => options.interrupts = true,
            "--session-start" => options.session_start = true,
            "--trust-prompt" => options.trust_prompt = true,
            "--echo-turn-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.echo_turn_ms = ms.min(MAX_WORK_MS);
            }
            "--opening-turn-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.opening_turn_ms = Some(ms.min(MAX_WORK_MS));
            }
            "--rate-limits-file" => {
                options.rate_limits_file =
                    Some(args.next().ok_or("--rate-limits-file needs a path")?.into());
            }
            "--context-full-after" => {
                options.context_full_after = Some(parse_number(&flag, args.next())?);
            }
            "--compact-fails" => options.compact_fails = true,
            "--compact-hangs" => options.compact_hangs = true,
            "--limit-continues-ms" => {
                let ms: u64 = parse_number(&flag, args.next())?;
                options.limit_continues_ms = Some(ms.min(MAX_WORK_MS));
            }
            "--limit-continues-answer-only" => options.limit_continues_answer_only = true,
            "--prompt" => options.prompt = Some(args.next().ok_or("--prompt needs a text")?),
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

    /// The user line of a turn that takes a while (`/work`).
    fn begin(&mut self, prompt: &str) -> io::Result<()> {
        self.turns += 1;
        let uuid = format!("fake-{}-{}-u", self.session_id, self.turns);
        let mut user = self.claude_envelope("user", &uuid, &rfc3339(now_ms()));
        user["message"] = json!({"role": "user", "content": prompt});
        self.last_uuid = Some(uuid);
        self.append(&[user])
    }

    /// The answer that ends a `/work` turn.
    fn answer(&mut self, prompt: &str, answer: &str) -> io::Result<()> {
        let n = self.turns;
        let uuid = format!("fake-{}-{n}-a", self.session_id);
        let mut assistant = self.claude_envelope("assistant", &uuid, &rfc3339(now_ms()));
        assistant["message"] = json!({
            "id": format!("msg_fake_{n}"),
            "type": "message",
            "role": "assistant",
            "model": "fake-model",
            "content": [{"type": "text", "text": answer}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10 + prompt.len() as u64,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                "output_tokens": 4 + answer.len() as u64}
        });
        self.last_uuid = Some(uuid);
        self.append(&[assistant])
    }

    /// Claude Code continued the turn a usage limit cut by itself (`--limit-continues-ms`): its
    /// hidden continuation prompt and, unless `answer_only`, the first answer, a tool call.
    fn continued(&mut self, answer_only: bool) -> io::Result<()> {
        let timestamp = rfc3339(now_ms());
        let meta_uuid = format!("fake-{}-continue-u", self.session_id);
        let mut meta = self.claude_envelope("user", &meta_uuid, &timestamp);
        meta["isMeta"] = json!(true);
        meta["message"] = json!({"role": "user", "content": "Continue from where you left off."});
        self.last_uuid = Some(meta_uuid);
        if answer_only {
            return self.append(&[meta]);
        }
        let uuid = format!("fake-{}-continue-a", self.session_id);
        let mut assistant = self.claude_envelope("assistant", &uuid, &timestamp);
        assistant["message"] = json!({
            "id": "msg_fake_continue_tool",
            "type": "message",
            "role": "assistant",
            "model": "fake-model",
            "content": [{"type": "tool_use", "id": "toolu_fake_continue", "name": "Bash",
                "input": {"command": "true"}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 12, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0, "output_tokens": 5}
        });
        self.last_uuid = Some(uuid);
        self.append(&[meta, assistant])
    }

    /// The answer that ends the turn Claude Code continued by itself.
    fn continued_answer(&mut self) -> io::Result<()> {
        let uuid = format!("fake-{}-continue-end", self.session_id);
        let mut assistant = self.claude_envelope("assistant", &uuid, &rfc3339(now_ms()));
        assistant["message"] = json!({
            "id": "msg_fake_continue_end",
            "type": "message",
            "role": "assistant",
            "model": "fake-model",
            "content": [{"type": "text", "text": "DONE continued"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 14, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0, "output_tokens": 4}
        });
        self.last_uuid = Some(uuid);
        self.append(&[assistant])
    }

    /// The person interrupted the turn: Claude Code's user line, shaped like 2.1.292's.
    fn interrupted(&mut self) -> io::Result<()> {
        let uuid = format!("fake-{}-{}-interrupt", self.session_id, self.turns);
        let mut user = self.claude_envelope("user", &uuid, &rfc3339(now_ms()));
        user["message"] = json!({"role": "user",
            "content": [{"type": "text", "text": INTERRUPT_LINE}]});
        self.last_uuid = Some(uuid);
        self.append(&[user])
    }

    /// An assistant line that is not a turn's end (a tool call), like Claude Code writes while
    /// the agent works.
    fn tool_use(&mut self) -> io::Result<()> {
        self.turns += 1;
        let n = self.turns;
        let uuid = format!("fake-{}-{n}-tool", self.session_id);
        let mut assistant = self.claude_envelope("assistant", &uuid, &rfc3339(now_ms()));
        assistant["message"] = json!({
            "id": format!("msg_fake_{n}"),
            "type": "message",
            "role": "assistant",
            "model": "fake-model",
            "content": [{"type": "tool_use", "id": format!("toolu_fake_{n}"),
                "name": "Bash", "input": {"command": "true"}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 8, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0, "output_tokens": 6}
        });
        self.last_uuid = Some(uuid);
        self.append(&[assistant])
    }

    /// A tool result that started background work, with Claude Code 2.1.292's `toolUseResult`.
    fn tool_result(&mut self, text: &str, result: Value) -> io::Result<()> {
        let uuid = format!("fake-{}-{}-result", self.session_id, self.turns);
        let mut user = self.claude_envelope("user", &uuid, &rfc3339(now_ms()));
        user["message"] = json!({"role": "user", "content": [{"type": "tool_result",
            "tool_use_id": format!("toolu_fake_{}", self.turns), "content": text}]});
        user["toolUseResult"] = result;
        self.last_uuid = Some(uuid);
        self.append(&[user])
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
    fn close_turn(&mut self, duration_ms: u64, pending_agents: u32) -> io::Result<()> {
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
        duration["pendingBackgroundAgentCount"] = json!(pending_agents);
        self.last_uuid = Some(duration_uuid);
        self.append(&[summary, duration])
    }

    /// The API's answer to a message too long for the context ([`Self::begin`] wrote its user
    /// line): Claude Code 2.1.292's API-error record (no Stop hook follows).
    fn prompt_too_long(&mut self) -> io::Result<()> {
        let n = self.turns;
        let uuid = format!("fake-{}-{n}-too-long", self.session_id);
        let mut record = self.claude_envelope("assistant", &uuid, &rfc3339(now_ms()));
        record["isApiErrorMessage"] = json!(true);
        record["error"] = json!("invalid_request");
        record["errorDetails"] = json!("prompt is too long: 213000 tokens > 200000 maximum");
        record["message"] = json!({
            "id": format!("msg_fake_too_long_{n}"),
            "type": "message",
            "role": "assistant",
            "model": "<synthetic>",
            "content": [{"type": "text", "text": "Prompt is too long"}],
            "stop_reason": "stop_sequence",
            "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0, "output_tokens": 0}
        });
        self.last_uuid = Some(uuid);
        self.append(&[record])
    }

    /// The line Claude Code 2.1.292 writes when `/compact` is typed, before it compacts: the
    /// command as typed, with no `origin` (a typed prompt has `origin: {kind: "human"}`).
    fn compact_typed(&mut self, line: &str) -> io::Result<()> {
        let uuid = format!("fake-{}-{}-compact-typed", self.session_id, self.turns);
        let mut record = self.claude_envelope("user", &uuid, &rfc3339(now_ms()));
        record["promptId"] = json!(format!("fake-{}-{}-compact", self.session_id, self.turns));
        record["message"] = json!({"role": "user", "content": line});
        self.last_uuid = Some(uuid);
        self.append(&[record])
    }

    /// The records Claude Code 2.1.292 writes when `/compact` succeeds, in its order: the
    /// boundary, the summary the session continues from, the command's caveat and echo (dated
    /// when it was typed) and its output.
    fn compacted(&mut self, line: &str) -> io::Result<()> {
        let at = rfc3339(now_ms());
        let prompt = format!("fake-{}-{}-compact", self.session_id, self.turns);
        let uuid = format!("fake-{}-{}-compact", self.session_id, self.turns);
        let mut boundary = self.claude_envelope("system", &uuid, &at);
        boundary["subtype"] = json!("compact_boundary");
        boundary["content"] = json!("Conversation compacted");
        boundary["isMeta"] = json!(false);
        boundary["level"] = json!("info");
        boundary["compactMetadata"] = json!({"trigger": "manual", "preTokens": 213_000});
        let mut summary = self.claude_envelope("user", &format!("{uuid}-summary"), &at);
        summary["promptId"] = json!(prompt);
        summary["message"] = json!({"role": "user", "content": "This session is being continued \
            from a previous conversation that ran out of context."});
        summary["isVisibleInTranscriptOnly"] = json!(true);
        summary["isCompactSummary"] = json!(true);
        let mut caveat = self.claude_envelope("user", &format!("{uuid}-caveat"), &at);
        caveat["promptId"] = json!(prompt);
        caveat["isMeta"] = json!(true);
        caveat["message"] = json!({"role": "user", "content": "<local-command-caveat>The command \
            below was run directly in Claude Code, not sent to you as a request.\
            </local-command-caveat>"});
        let args = line.get("/compact".len()..).unwrap_or_default().trim();
        let mut echo = self.claude_envelope("user", &format!("{uuid}-echo"), &at);
        echo["promptId"] = json!(prompt);
        echo["message"] = json!({"role": "user", "content": format!(
            "<command-name>/compact</command-name>\n            <command-message>compact\
            </command-message>\n            <command-args>{args}</command-args>")});
        let mut output = self.claude_envelope("user", &format!("{uuid}-output"), &at);
        output["promptId"] = json!(prompt);
        output["message"] = json!({"role": "user", "content": "<local-command-stdout>\u{1b}[2m\
            Compacted (ctrl+o to see full summary)\u{1b}[22m</local-command-stdout>"});
        self.last_uuid = Some(format!("{uuid}-output"));
        self.append(&[boundary, summary, caveat, echo, output])
    }

    /// The records of Claude Code 2.1.292's compaction of an idle session ("Compacted while idle,
    /// before the prompt cache expired"), in its order: the boundary, the summary the session
    /// continues from, the files it reads again and the `SessionStart:compact` hook's record.
    /// The notice ([`Self::idle_compaction_notice`]) follows the hook.
    fn idle_compaction(&mut self) -> io::Result<()> {
        let at = rfc3339(now_ms());
        let n = self.turns;
        let mut boundary = self.claude_envelope(
            "system",
            &format!("fake-{}-{n}-idle-compact", self.session_id),
            &at,
        );
        boundary["parentUuid"] = Value::Null;
        boundary["logicalParentUuid"] = json!(self.last_uuid);
        boundary["subtype"] = json!("compact_boundary");
        boundary["content"] = json!("Conversation compacted");
        boundary["isMeta"] = json!(false);
        boundary["level"] = json!("info");
        boundary["compactMetadata"] = json!({"trigger": "auto", "preTokens": 400_160, "durationMs": 47_527, "postTokens": 8_602});
        let summary_uuid = format!("fake-{}-{n}-summary", self.session_id);
        let mut summary = self.claude_envelope("user", &summary_uuid, &at);
        summary["parentUuid"] = boundary["uuid"].clone();
        summary["promptId"] = json!(format!("fake-{}-{n}-summary-prompt", self.session_id));
        summary["message"] = json!({"role": "user", "content": "This session is being continued \
            from a previous conversation that ran out of context. The summary below covers the \
            earlier portion of the conversation."});
        summary["isVisibleInTranscriptOnly"] = json!(true);
        summary["isCompactSummary"] = json!(true);
        summary["turnPosition"] = json!({"promptIndex": 1, "turnIndex": n});
        let mut file = self.claude_envelope("attachment", &format!("{summary_uuid}-file"), &at);
        file["parentUuid"] = json!(summary_uuid);
        file["attachment"] = json!({"type": "file", "filename": "/tmp/fake/tasks/b1.output",
            "content": {"type": "text", "file": {"filePath": "/tmp/fake/tasks/b1.output",
            "content": "done", "numLines": 1, "startLine": 1, "totalLines": 1}}});
        let mut reference =
            self.claude_envelope("attachment", &format!("{summary_uuid}-reference"), &at);
        reference["attachment"] = json!({"type": "compact_file_reference",
            "filename": format!("{}/src/lib.rs", self.cwd), "displayPath": "src/lib.rs"});
        let mut hook = self.claude_envelope("attachment", &format!("{summary_uuid}-hook"), &at);
        hook["attachment"] = json!({"type": "hook_success", "hookName": "SessionStart:compact",
            "hookEvent": "SessionStart", "content": "", "exitCode": 0});
        self.last_uuid = Some(format!("{summary_uuid}-hook"));
        self.append(&[boundary, summary, file, reference, hook])
    }

    /// The notice that ends an idle compaction.
    fn idle_compaction_notice(&mut self) -> io::Result<()> {
        let uuid = format!("fake-{}-{}-idle-notice", self.session_id, self.turns);
        let mut notice = self.claude_envelope("system", &uuid, &rfc3339(now_ms()));
        notice["subtype"] = json!("informational");
        notice["content"] = json!("Compacted while idle, before the prompt cache expired");
        notice["isMeta"] = json!(false);
        notice["level"] = json!("notice");
        self.last_uuid = Some(uuid);
        self.append(&[notice])
    }

    /// The error output of a `/compact` that failed.
    fn compact_failed(&mut self) -> io::Result<()> {
        let uuid = format!("fake-{}-{}-compact-failed", self.session_id, self.turns);
        let mut record = self.claude_envelope("system", &uuid, &rfc3339(now_ms()));
        record["subtype"] = json!("local_command");
        record["content"] = json!(
            "<local-command-stderr>Error: Error during compaction: Conversation too long. Press \
             esc twice to go up a few messages and try again.</local-command-stderr>"
        );
        record["level"] = json!("info");
        self.last_uuid = Some(uuid);
        self.append(&[record])
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
    /// `UserPromptSubmit`; `Some(reason)` when the hook blocks the prompt.
    fn prompt(&self, prompt: &str) -> Option<String> {
        let answer = run_hook(
            &self.exe,
            "UserPromptSubmit",
            &json!({"session_id": self.session_id, "hook_event_name": "UserPromptSubmit",
                "prompt": prompt}),
        );
        let value: Value = serde_json::from_str(&answer).ok()?;
        (value.get("decision").and_then(Value::as_str) == Some("block")).then(|| {
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
    }

    /// `SessionStart` (`source: startup`), as Claude Code runs it before its prompt box reads.
    fn session_start(&self) {
        self.session_start_from("startup");
    }

    /// `SessionStart` with `source` (`startup`, or `compact` after a compaction).
    fn session_start_from(&self, source: &str) {
        run_hook(
            &self.exe,
            "SessionStart",
            &json!({"session_id": self.session_id, "hook_event_name": "SessionStart",
                "source": source}),
        );
    }

    /// `SessionEnd` with `reason`.
    fn session_end(&self, reason: &str) {
        run_hook(
            &self.exe,
            "SessionEnd",
            &json!({"session_id": self.session_id, "hook_event_name": "SessionEnd",
                "reason": reason}),
        );
    }

    /// Claude Code's status line command with the rate limits in `file` (if it exists).
    fn status_line(&self, file: &Path) -> io::Result<()> {
        let mut input = json!({"session_id": self.session_id,
            "model": {"id": "fake-model", "display_name": "Fake"},
            "workspace": {"current_dir": std::env::current_dir()
                .map(|p| p.display().to_string()).unwrap_or_default()}});
        if let Some(limits) = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            input["rate_limits"] = limits;
        }
        let line = run_hook(&self.exe, "StatusLine", &input);
        say(&format!("STATUS {}", line.trim_end()))
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
/// the agent `wake_delay_ms` after it exits, like Claude Code does when it next polls.
fn start_background(
    seconds: u64,
    wake_delay_ms: u64,
    verdict: Option<WakeVerdict>,
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
        std::thread::sleep(Duration::from_millis(wake_delay_ms));
        let prompt = format!(
            "<task-notification>\n<task-id>bash-{pid}</task-id>\n<status>completed</status>\n</task-notification>"
        );
        wake_with(
            &prompt,
            "BACKGROUND DONE",
            verdict.as_ref(),
            transcript.as_ref(),
            hooks.as_ref(),
        );
    });
    Ok(pid)
}

/// Background work (a background agent, a Monitor stream) that ends after `ms`: the session is
/// woken by its `<task-notification>` prompt, answers, prints `done`, runs its Stop hook and closes
/// the turn with no agent pending.
fn wake_after(
    ms: u64,
    prompt: &'static str,
    done: &'static str,
    transcript: Option<Arc<Mutex<Transcript>>>,
    hooks: Option<Hooks>,
) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        wake(prompt, done, transcript.as_ref(), hooks.as_ref());
    });
}

/// The notes of Claude Code 2.1.292 on a background agent's notification: it stopped with
/// background work of its own still running (its result may be interim), or with none left.
const AGENT_INTERIM_NOTE: &str = "This agent stopped with background work of its own still running. It may resume on its own when that work completes or reports, and the same task-id notifies again if it does; the result below may be interim.";
const AGENT_FINAL_NOTE: &str = "A task-notification fires each time this agent stops with no live background children of its own. The user can send it another message and resume it, so the same task-id may notify more than once.";

/// The goal turn's background agent `agent-1` wakes the session after `seconds` (`AGENT DONE`).
/// With `interim` it first stops with background work of its own still running, like the review
/// agent of task #74: that notification wakes the session (`AGENT INTERIM`), and `interim`
/// seconds later the same task id's final one does.
fn agent_notifies(
    seconds: u64,
    interim: Option<u64>,
    transcript: Option<Arc<Mutex<Transcript>>>,
    hooks: Option<Hooks>,
) {
    let notification = |note: &str, result: &str| {
        format!(
            "<task-notification>\n<task-id>agent-1</task-id>\n<status>completed</status>\n<summary>Agent \"Review\" finished</summary>\n<note>{note}</note>\n<result>{result}</result>\n</task-notification>"
        )
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        if let Some(rest) = interim {
            let text = notification(AGENT_INTERIM_NOTE, "The tests still run; I report then.");
            wake(&text, "AGENT INTERIM", transcript.as_ref(), hooks.as_ref());
            std::thread::sleep(Duration::from_secs(rest));
        }
        let text = notification(AGENT_FINAL_NOTE, "Approve.");
        wake(&text, "AGENT DONE", transcript.as_ref(), hooks.as_ref());
    });
}

/// Claude Code wakes the idle session when background work ends: its `<task-notification>` is a
/// user prompt (after `UserPromptSubmit`), the agent answers and prints `done`, the Stop hook runs
/// and the turn closes with nothing pending.
fn wake(
    prompt: &str,
    done: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
) {
    wake_with(prompt, done, None, transcript, hooks);
}

/// The goal verdict that closes a woken turn (`--goal-met-on-wake-ms`).
struct WakeVerdict {
    condition: String,
    /// How long after the Stop hook the evaluator records the goal as achieved.
    after_ms: u64,
    /// The turn's Stop hook never reaches Yulius (`--wake-stop-lost`).
    stop_lost: bool,
}

/// [`wake`], with the goal `verdict` closing the woken turn.
fn wake_with(
    prompt: &str,
    done: &str,
    verdict: Option<&WakeVerdict>,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
) {
    if let Some(hooks) = hooks {
        let _ = hooks.prompt(prompt);
    }
    let write = |f: &dyn Fn(&mut Transcript) -> io::Result<()>| {
        if let Some(transcript) = transcript
            && let Ok(mut t) = transcript.lock()
        {
            let _ = f(&mut t);
        }
    };
    // The model takes a moment to answer, as a real one does: the prompt is read before the Stop.
    write(&|t| t.begin(prompt));
    std::thread::sleep(Duration::from_millis(1_500));
    write(&|t| t.answer(prompt, "The background work finished."));
    let _ = say(done);
    if let Some(hooks) = hooks
        && !verdict.is_some_and(|v| v.stop_lost)
    {
        let _ = hooks.stop();
    }
    if let Some(verdict) = verdict {
        std::thread::sleep(Duration::from_millis(verdict.after_ms));
        let condition = &verdict.condition;
        write(&|t| {
            t.goal_status(
                json!({"type": "goal_status", "met": true, "condition": condition,
                "reason": "Merged on green.", "iterations": 1, "durationMs": 51_000,
                "tokens": 7_100}),
            )
        });
        let _ = say("✓ Goal achieved");
    }
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        let _ = t.close_turn(2_000, 0);
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

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// Reads one submitted message: a line, or several when a bracketed paste holds newlines.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let Some(mut message) = read_line(input)? else {
        return Ok(None);
    };
    while message.matches(PASTE_START).count() > message.matches(PASTE_END).count() {
        let Some(next) = read_line(input)? else {
            break;
        };
        message.push('\n');
        message.push_str(&next);
    }
    Ok(Some(render_pastes(&drop_reports(&message))))
}

/// The message without the focus reports (`ESC [ I`, `ESC [ O`) and SGR or X10 mouse reports a
/// terminal sends by itself: Claude Code consumes them, they never reach its prompt box.
fn drop_reports(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(at) = rest.find("\x1b[") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let skip = if after.starts_with('I') || after.starts_with('O') {
            Some(1)
        } else if let Some(sgr) = after.strip_prefix('<') {
            sgr.find(['M', 'm'])
                .filter(|end| sgr[..*end].bytes().all(|b| b.is_ascii_digit() || b == b';'))
                .map(|end| end + 2)
        } else if after.starts_with('M') {
            Some(after.char_indices().nth(4).map_or(after.len(), |(k, _)| k))
        } else {
            None
        };
        match skip {
            Some(n) => rest = &after[n..],
            None => {
                out.push_str("\x1b[");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A message with its bracketed pastes read like Claude Code (see the module docs).
fn render_pastes(message: &str) -> String {
    let mut out = String::new();
    let mut images = 0;
    let mut rest = message;
    while let Some(start) = rest.find(PASTE_START) {
        out.push_str(&rest[..start]);
        let after = &rest[start + PASTE_START.len()..];
        let (pasted, next) = after.split_once(PASTE_END).unwrap_or((after, ""));
        let lower = pasted.to_ascii_lowercase();
        if pasted.starts_with('/')
            && [".png", ".jpg", ".jpeg", ".gif", ".webp"]
                .iter()
                .any(|ext| lower.ends_with(ext))
        {
            images += 1;
            out.push_str(&format!("[Image #{images}] "));
        } else {
            out.push_str(pasted);
        }
        rest = next;
    }
    out.push_str(rest);
    out
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
        let verdict = options.goal_met_on_wake_ms.map(|after_ms| WakeVerdict {
            condition: condition.clone(),
            after_ms,
            stop_lost: options.wake_stop_lost,
        });
        let pid = start_background(
            seconds,
            options.wake_delay_ms,
            verdict,
            transcript.cloned(),
            hooks.cloned(),
        )?;
        say(&format!("BACKGROUND {pid}"))?;
    }
    std::thread::sleep(Duration::from_millis(options.goal_delay_ms));
    let answer = format!("ECHO: {line}");
    say(&answer)?;
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.turn(line, &answer)?;
        // Background work the turn started, recorded before its Stop hook.
        if options.goal_agent.is_some() {
            t.tool_result(
                "Async agent launched successfully.",
                json!({"isAsync": true, "status": "async_launched",
                "agentId": "agent-1", "description": "Review"}),
            )?;
        }
        if let Some(seconds) = options.goal_monitor {
            t.tool_result(
                &format!("Monitor started (task monitor-1, expires in {seconds}s unless the source ends first)"),
                json!({"taskId": "monitor-1", "timeoutMs": seconds * 1000,
                "persistent": false}),
            )?;
        }
    }
    if let (Some(seconds), true) = (options.goal_monitor, options.goal_monitor_ends) {
        wake_after(
            seconds * 500,
            "<task-notification>\n<task-id>monitor-1</task-id>\n<status>completed</status>\n<summary>Monitor \"ci\" stream ended</summary>\n</task-notification>",
            "MONITOR DONE",
            transcript.cloned(),
            hooks.cloned(),
        );
    }
    // Claude Code's goal evaluator is a Stop hook: its record follows the turn's Stop hooks.
    if let Some(hooks) = hooks {
        hooks.stop()?;
    }
    // A turn the stop did not end: the evaluator says "not met" (`--goal-continue-verdict`) or
    // another Stop hook continues it, and the agent works on with no new prompt.
    if let Some(ms) = options.goal_continue_ms {
        let write = |f: &dyn Fn(&mut Transcript) -> io::Result<()>| -> io::Result<()> {
            if let Some(transcript) = transcript
                && let Ok(mut t) = transcript.lock()
            {
                f(&mut t)?;
            }
            Ok(())
        };
        if options.goal_continue_verdict {
            write(&|t| {
                t.goal_status(json!({"type": "goal_status", "met": false,
                    "condition": condition, "reason": "Not yet: the merge is pending."}))
            })?;
            say("GOAL NOT MET")?;
            std::thread::sleep(Duration::from_millis(ms));
            if options.goal_continue_dies {
                // An API error ended the continued turn: no Stop hook runs, the turn closes.
                write(&|t| t.close_turn(ms, 0))?;
                return say("CONTINUATION DIED");
            }
            write(&|t| t.tool_use())?;
        } else {
            say("STOP CONTINUED")?;
            std::thread::sleep(Duration::from_millis(1_000));
            write(&|t| t.tool_use())?;
            std::thread::sleep(Duration::from_millis(ms));
        }
        if let Some(hooks) = hooks {
            hooks.stop()?;
        }
    }
    // The evaluator gives no verdict while background work runs: the woken turn gets it.
    let deferred = options.goal_background.is_some() && options.goal_met_on_wake_ms.is_some();
    let verdict = if deferred {
        GoalVerdict::None
    } else {
        options.goal_verdict
    };
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        match verdict {
            GoalVerdict::Met => t.goal_status(
                json!({"type": "goal_status", "met": true, "condition": condition,
                "reason": "Done and verified.", "iterations": 1, "durationMs": 51_000,
                "tokens": 7_100}),
            )?,
            GoalVerdict::Failed => {
                t.goal_status(json!({"type": "goal_status", "met": false, "failed": true,
                    "condition": condition, "reason": "It cannot be done."}))?;
                // The record that closes the turn comes late: the verdict alone settles it.
                say("GOAL FAILED")?;
                std::thread::sleep(Duration::from_secs(6));
            }
            GoalVerdict::None => {}
        }
        t.close_turn(51_000, u32::from(options.goal_agent.is_some()))?;
    }
    if verdict != GoalVerdict::Met {
        return say("GOAL UNMET");
    }
    if let Some(seconds) = options.goal_agent {
        agent_notifies(
            seconds,
            options.goal_agent_interim,
            transcript.cloned(),
            hooks.cloned(),
        );
    }
    if let Some(ms) = options.idle_compact_ms {
        compact_when_idle(ms, transcript.cloned(), hooks.cloned());
    }
    say("✓ Goal achieved")
}

/// The idle session compacts itself after `ms`, like Claude Code 2.1.292 before its prompt cache
/// expires: no prompt, no turn, no Stop hook (`--idle-compact-ms`).
fn compact_when_idle(ms: u64, transcript: Option<Arc<Mutex<Transcript>>>, hooks: Option<Hooks>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        let write = |f: &dyn Fn(&mut Transcript) -> io::Result<()>| {
            if let Some(transcript) = &transcript
                && let Ok(mut t) = transcript.lock()
            {
                let _ = f(&mut t);
            }
        };
        write(&|t| t.idle_compaction());
        if let Some(hooks) = &hooks {
            hooks.session_start_from("compact");
        }
        write(&|t| t.idle_compaction_notice());
        let _ = say("COMPACTED WHILE IDLE");
    });
}

/// Claude Code 2.1.292's folder-trust dialog for a folder whose `.claude/settings.json`
/// pre-approves tools; the answer is the next line, then the screen is cleared of the dialog.
fn trust_prompt() -> io::Result<()> {
    say("Accessing workspace:")?;
    say("Quick safety check: Is this a project you created or one you trust?")?;
    say("This folder pre-approves 2 tool permissions in .claude/settings.json: Write, Edit")?;
    say("\u{276f} No, continue without these permissions")?;
    say("  Yes, I trust this folder")?;
    say("Enter to confirm \u{b7} Esc to cancel")?;
    let answer = read_message(&mut io::stdin().lock())?.unwrap_or_default();
    let mut out = io::stdout().lock();
    out.write_all(b"\x1b[2J\x1b[H")?;
    out.flush()?;
    drop(out);
    say(&format!("TRUST ANSWERED: {answer}"))
}

fn echo_loop(options: &Options, session_id: &str) -> io::Result<i32> {
    let transcript = match &options.transcript {
        Some(path) => Some(Arc::new(Mutex::new(Transcript::open(
            path.clone(),
            session_id,
        )?))),
        None => None,
    };
    let mut hooks = options.hooks.as_ref().map(|exe| Hooks {
        exe: exe.clone(),
        session_id: session_id.to_owned(),
    });
    if options.trust_prompt {
        trust_prompt()?;
    }
    let keys = Keys::start(options, hooks.clone())?;
    let keys = keys.as_ref();
    status_line(options, hooks.as_ref())?;
    let mut state = EchoState::default();
    if let (true, Some(hooks)) = (options.session_start, &hooks) {
        hooks.session_start();
    }
    if let Some(ms) = options.opening_turn_ms {
        state.turns += 1;
        work_turn(
            options,
            &format!("/work {ms} opening"),
            transcript.as_ref(),
            hooks.as_ref(),
            keys,
        )?;
    }
    if let Some(prompt) = &options.prompt {
        if prompt.starts_with("/goal ") && (transcript.is_some() || hooks.is_some()) {
            // Claude Code runs a slash command given as its prompt argument (SPEC-117).
            state.turns += 1;
            goal_turn(options, prompt, transcript.as_ref(), hooks.as_ref())?;
        } else {
            echo_message(
                options,
                prompt,
                transcript.as_ref(),
                hooks.as_ref(),
                &mut state,
            )?;
        }
    }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut to_drop = options.drop_lines;
    while let Some(line) = read_message(&mut input)? {
        // A submit the TUI lost (`--drop-lines`): the first typed lines never reach the agent.
        if to_drop > 0 && !line.is_empty() {
            to_drop -= 1;
            continue;
        }
        match line.as_str() {
            "/exit" => {
                if let Some(hooks) = &hooks {
                    hooks.session_end("prompt_input_exit");
                }
                say("BYE")?;
                return Ok(0);
            }
            "/crash" => {
                say("CRASHING")?;
                return Ok(3);
            }
            "/size" => say(&format!("SIZE {}", terminal_size()))?,
            _ if line.starts_with("/clear ") => {
                let id = line["/clear ".len()..].trim().to_owned();
                if let Some(hooks) = &hooks {
                    hooks.session_end("clear");
                }
                if let Some(transcript) = &transcript
                    && let Ok(mut t) = transcript.lock()
                {
                    let path = t.path.with_file_name(format!("{id}.jsonl"));
                    *t = Transcript::open(path, &id)?;
                }
                if let Some(hooks) = &mut hooks {
                    hooks.session_id.clone_from(&id);
                    hooks.session_start_from("clear");
                }
                say(&format!("CLEARED session={id}"))?;
            }
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
            _ if line.starts_with("/sh ") => {
                // Runs a shell command like an agent's Bash tool (containment tests): its output
                // goes to the terminal, then `SH EXIT <code>`.
                let command = line["/sh ".len()..].trim();
                let status = Command::new("/bin/sh")
                    .args(["-c", command])
                    .stdin(Stdio::null())
                    .status();
                let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
                say(&format!("SH EXIT {code}"))?;
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
            _ if line.starts_with("/compact") => {
                compact(options, &line, transcript.as_ref(), &mut state)?;
            }
            _ if state.limited => say("LIMIT REACHED")?,
            _ if line.starts_with("/goal ") && (transcript.is_some() || hooks.is_some()) => {
                state.turns += 1;
                goal_turn(options, &line, transcript.as_ref(), hooks.as_ref())?;
            }
            _ if line.starts_with("/work ") => {
                state.turns += 1;
                work_turn(options, &line, transcript.as_ref(), hooks.as_ref(), keys)?;
            }
            _ => echo_message(
                options,
                &line,
                transcript.as_ref(),
                hooks.as_ref(),
                &mut state,
            )?,
        }
    }
    Ok(0)
}

/// What the echo loop keeps between messages.
#[derive(Debug, Default)]
struct EchoState {
    /// Turns run (echoed messages, goal and work turns).
    turns: u64,
    /// A usage limit stopped the agent (`--limit-after`).
    limited: bool,
    /// Messages answered with an echo (`--context-full-after` counts them).
    echoed: u64,
    /// A `/compact` emptied the context: it never fills again.
    compacted: bool,
}

impl EchoState {
    /// Whether the context is full (`--context-full-after`) before the next message.
    fn context_full(&self, options: &Options) -> bool {
        !self.compacted && options.context_full_after.is_some_and(|n| self.echoed >= n)
    }
}

/// Answers one message with `ECHO: <line>` and its turn, or `PROMPT IS TOO LONG` while the
/// context is full.
fn echo_message(
    options: &Options,
    line: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
    state: &mut EchoState,
) -> io::Result<()> {
    if let Some(reason) = hooks.and_then(|h| h.prompt(line)) {
        return say(&format!("PROMPT BLOCKED: {reason}"));
    }
    if state.context_full(options) {
        // The API refused the prompt: no turn ran, so no Stop hook runs.
        if let Some(transcript) = transcript
            && let Ok(mut t) = transcript.lock()
        {
            t.begin(line)?;
        }
        std::thread::sleep(Duration::from_millis(options.echo_turn_ms));
        say("PROMPT IS TOO LONG")?;
        if let Some(transcript) = transcript
            && let Ok(mut t) = transcript.lock()
        {
            t.prompt_too_long()?;
        }
        return status_line(options, hooks);
    }
    std::thread::sleep(Duration::from_millis(options.echo_turn_ms));
    let answer = format!("ECHO: {line}");
    say(&answer)?;
    state.turns += 1;
    state.echoed += 1;
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.turn(line, &answer)?;
    }
    if options
        .limit_after
        .is_some_and(|limit| state.turns >= limit)
    {
        state.limited = true;
        say("LIMIT REACHED")?;
        if let Some(transcript) = transcript
            && let Ok(mut t) = transcript.lock()
        {
            t.limit()?;
        }
        if let (Some(ms), Some(transcript)) = (options.limit_continues_ms, transcript) {
            continue_after_limit(
                ms,
                options.limit_continues_answer_only,
                Arc::clone(transcript),
                hooks.cloned(),
            );
        }
    } else if let Some(hooks) = hooks {
        hooks.stop()?;
    }
    status_line(options, hooks)
}

/// Claude Code continuing the turn a usage limit cut by itself, `ms` after the limit
/// (`--limit-continues-ms`).
fn continue_after_limit(
    ms: u64,
    answer_only: bool,
    transcript: Arc<Mutex<Transcript>>,
    hooks: Option<Hooks>,
) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        if let Ok(mut t) = transcript.lock() {
            let _ = t.continued(answer_only);
        }
        let _ = say("CONTINUED");
        std::thread::sleep(Duration::from_millis(ms));
        if let Ok(mut t) = transcript.lock() {
            let _ = t.continued_answer();
        }
        let _ = say("DONE continued");
        if let Some(hooks) = hooks
            && hooks.stop().is_ok()
        {
            let _ = say("CONTINUED STOP ANSWERED");
        }
    });
}

/// `/compact <focus>`: empties the context for good, or fails with `--compact-fails`.
fn compact(
    options: &Options,
    line: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    state: &mut EchoState,
) -> io::Result<()> {
    let Some(transcript) = transcript else {
        return say("COMPACTED: (no transcript)");
    };
    let Ok(mut t) = transcript.lock() else {
        return Ok(());
    };
    t.compact_typed(line)?;
    if options.compact_hangs {
        return say("COMPACTING");
    }
    if options.compact_fails {
        say("COMPACTION FAILED")?;
        return t.compact_failed();
    }
    state.compacted = true;
    t.compacted(line)?;
    say(&format!("COMPACTED: {}", line["/compact".len()..].trim()))
}

/// Runs the status line command when `--rate-limits-file` is given (with `--hooks`).
fn status_line(options: &Options, hooks: Option<&Hooks>) -> io::Result<()> {
    match (hooks, &options.rate_limits_file) {
        (Some(hooks), Some(file)) => hooks.status_line(file),
        _ => Ok(()),
    }
}

/// Ctrl+C and SIGTERM as Claude Code handles them (`--interrupts`, see the module docs).
#[derive(Clone)]
struct Keys {
    /// A turn is running: Ctrl+C interrupts it.
    turn: Arc<AtomicBool>,
    /// Ctrl+C asked the running turn to stop.
    interrupt: Arc<AtomicBool>,
}

impl Keys {
    /// Watches SIGINT and SIGTERM; `None` without `--interrupts`.
    fn start(options: &Options, hooks: Option<Hooks>) -> io::Result<Option<Self>> {
        if !options.interrupts {
            return Ok(None);
        }
        let sigint = Arc::new(AtomicBool::new(false));
        let sigterm = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigint))?;
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&sigterm))?;
        let keys = Self {
            turn: Arc::new(AtomicBool::new(false)),
            interrupt: Arc::new(AtomicBool::new(false)),
        };
        let watch = keys.clone();
        std::thread::spawn(move || {
            let mut armed: Option<std::time::Instant> = None;
            loop {
                std::thread::sleep(Duration::from_millis(5));
                if sigterm.swap(false, Ordering::SeqCst) {
                    if let Some(hooks) = &hooks {
                        hooks.session_end("other");
                    }
                    std::process::exit(143);
                }
                if !sigint.swap(false, Ordering::SeqCst) {
                    continue;
                }
                if watch.turn.load(Ordering::SeqCst) {
                    watch.interrupt.store(true, Ordering::SeqCst);
                    armed = None;
                } else if armed.is_some_and(|at| at.elapsed() <= DOUBLE_CTRL_C) {
                    if let Some(hooks) = &hooks {
                        hooks.session_end("prompt_input_exit");
                    }
                    let _ = say("BYE");
                    std::process::exit(0);
                } else {
                    armed = Some(std::time::Instant::now());
                    let _ = say("Press Ctrl-C again to exit");
                }
            }
        });
        Ok(Some(keys))
    }
}

/// `/work <ms> <text>`: a turn that takes a while and can be interrupted (see the module docs).
fn work_turn(
    options: &Options,
    line: &str,
    transcript: Option<&Arc<Mutex<Transcript>>>,
    hooks: Option<&Hooks>,
    keys: Option<&Keys>,
) -> io::Result<()> {
    let mut parts = line.splitn(3, ' ').skip(1);
    let ms = parts
        .next()
        .and_then(|ms| ms.parse::<u64>().ok())
        .unwrap_or(0)
        .min(MAX_WORK_MS);
    let text = parts.next().unwrap_or("work").to_owned();
    if let Some(reason) = hooks.and_then(|h| h.prompt(line)) {
        return say(&format!("PROMPT BLOCKED: {reason}"));
    }
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.begin(line)?;
    }
    if let Some(keys) = keys {
        keys.interrupt.store(false, Ordering::SeqCst);
        keys.turn.store(true, Ordering::SeqCst);
    }
    say("WORKING")?;
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    let mut interrupted = false;
    while std::time::Instant::now() < deadline {
        if keys.is_some_and(|k| k.interrupt.swap(false, Ordering::SeqCst)) {
            interrupted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(keys) = keys {
        keys.turn.store(false, Ordering::SeqCst);
    }
    if interrupted {
        if let Some(transcript) = transcript
            && let Ok(mut t) = transcript.lock()
        {
            t.interrupted()?;
        }
        // Claude Code runs no Stop hook for an interrupted turn.
        return say("INTERRUPTED");
    }
    let answer = format!("DONE {text}");
    say(&answer)?;
    if let Some(transcript) = transcript
        && let Ok(mut t) = transcript.lock()
    {
        t.answer(line, &answer)?;
    }
    if let Some(hooks) = hooks {
        hooks.stop()?;
    }
    status_line(options, hooks)
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
    fn focus_and_mouse_reports_never_reach_the_message() {
        assert_eq!(
            drop_reports("\x1b[I\x1b[<0;3;4Mdr\x1b[<0;3;4maft\x1b[M #%\x1b[O"),
            "draft"
        );
        assert_eq!(drop_reports("\x1b[A keeps keys"), "\x1b[A keeps keys");
    }

    #[test]
    fn pasted_image_paths_become_images_and_other_pastes_stay_text() {
        let message = "\x1b[200~/a/b.png\x1b[201~\x1b[200~/a/my shot.JPG\x1b[201~\x1b[200~Look\nhere\x1b[201~";
        assert_eq!(render_pastes(message), "[Image #1] [Image #2] Look\nhere");
        assert_eq!(render_pastes("/goal plain"), "/goal plain");
        assert_eq!(
            render_pastes("\x1b[200~/a/notes.txt\x1b[201~"),
            "/a/notes.txt"
        );
        let mut input = io::Cursor::new(b"\x1b[200~one\ntwo\x1b[201~\nnext\n".to_vec());
        assert_eq!(
            read_message(&mut input).ok().flatten().as_deref(),
            Some("one\ntwo")
        );
        assert_eq!(
            read_message(&mut input).ok().flatten().as_deref(),
            Some("next")
        );
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
        assert!(parse(&["--wake-delay-ms", "999999"]).is_ok_and(|o| o.wake_delay_ms == MAX_GOAL_DELAY_MS));
        assert!(parse(&["--seconds", "999999"]).is_ok_and(|o| o.seconds == MAX_SLEEP_SECONDS));
        assert!(parse(&["--hooks"]).is_err());
        assert!(parse(&["--interrupts"]).is_ok_and(|o| o.interrupts));
        assert!(parse(&["--rate-limits-file"]).is_err());
        assert!(
            parse(&["--goal-background", "3", "--goal-delay-ms", "999999"])
                .is_ok_and(|o| o.goal_background == Some(3) && o.goal_delay_ms == MAX_GOAL_DELAY_MS)
        );
    }
}
