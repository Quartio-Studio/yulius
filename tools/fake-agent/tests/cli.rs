//! Fake agent contracts over pipes: the behaviors supervisor and harness tests rely on.

use std::io::{BufRead, Write};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EXE: &str = env!("CARGO_BIN_EXE_fake-agent");

fn run(args: &[&str], input: &str) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(EXE)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }
    Ok(child.wait_with_output()?)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn echo_mode_answers_lines_and_exits_on_command() -> TestResult {
    let output = run(
        &["--session-id", "s-1"],
        "hello\nsecond line\n/exit\nignored\n",
    )?;
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert_eq!(
        text,
        "FAKE-AGENT READY session=s-1\nECHO: hello\nECHO: second line\nBYE\n"
    );
    Ok(())
}

#[test]
fn crash_and_exit_early_report_their_codes() -> TestResult {
    let crash = run(&[], "/crash\n")?;
    assert_eq!(crash.status.code(), Some(3));
    let early = run(&["--mode", "exit-early", "--code", "7"], "")?;
    assert_eq!(early.status.code(), Some(7));
    assert!(stdout(&early).contains("EXITING code=7"));
    Ok(())
}

#[test]
fn flood_writes_exact_byte_counts_to_both_streams() -> TestResult {
    let output = run(&["--mode", "flood", "--bytes", "200000"], "/exit\n")?;
    assert_eq!(output.status.code(), Some(0));
    let marker = "FLOOD DONE\nBYE\n";
    let text = stdout(&output);
    assert!(text.starts_with("FAKE-AGENT READY session="));
    assert!(text.ends_with(marker));
    let first_line = text.find('\n').ok_or("no first line")? + 1;
    assert_eq!(output.stdout.len() - first_line - marker.len(), 200_000);
    assert_eq!(output.stderr.len(), 200_000);
    Ok(())
}

#[test]
fn flood_command_floods_on_demand() -> TestResult {
    let output = run(&[], "/flood 1000\n/exit\n")?;
    assert_eq!(output.stderr.len(), 1000);
    assert!(stdout(&output).ends_with("FLOOD DONE\nBYE\n"));
    Ok(())
}

#[test]
fn report_env_never_prints_values() -> TestResult {
    let output = Command::new(EXE)
        .args([
            "--mode",
            "exit-early",
            "--report-env",
            "FAKE_SECRET",
            "--report-env",
            "FAKE_ABSENT",
        ])
        .env_clear()
        .env("FAKE_SECRET", "synthetic-value-123")
        .output()?;
    let text = stdout(&output);
    assert!(text.contains("ENV FAKE_SECRET=set len=19"));
    assert!(text.contains("ENV FAKE_ABSENT=unset"));
    assert!(!text.contains("synthetic-value-123"));
    Ok(())
}

fn read_jsonl(path: &std::path::Path) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let mut records = Vec::new();
    for line in text.lines() {
        records.push(serde_json::from_str(line)?);
    }
    Ok(records)
}

#[test]
fn claude_transcript_has_one_user_and_assistant_record_per_turn_and_a_limit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("projects/x/s-2.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let output = run(
        &[
            "--session-id",
            "s-2",
            "--transcript",
            &path_text,
            "--transcript-kind",
            "claude",
            "--limit-after",
            "2",
        ],
        "one\ntwo\nthree\n/exit\n",
    )?;
    let text = stdout(&output);
    assert!(text.contains("ECHO: two\nLIMIT REACHED\nLIMIT REACHED\nBYE"));
    assert!(!text.contains("ECHO: three"));
    let records = read_jsonl(&path)?;
    assert_eq!(records.len(), 5);
    assert_eq!(records[0]["type"], "user");
    assert_eq!(records[0]["message"]["content"], "one");
    assert_eq!(records[0]["sessionId"], "s-2");
    assert_eq!(records[1]["type"], "assistant");
    assert_eq!(records[1]["message"]["id"], "msg_fake_1");
    assert!(records[1]["message"]["usage"]["output_tokens"].is_u64());
    assert_eq!(records[1]["parentUuid"], records[0]["uuid"]);
    assert_eq!(records[4]["isApiErrorMessage"], true);
    assert_eq!(records[4]["apiError"], "usage_limit_reached");
    assert!(records[4]["apiErrorParams"]["rate_limit_info"]["resetsAt"].is_u64());
    let limit_text = records[4]["message"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(limit_text.starts_with("You've hit your session limit"));
    Ok(())
}

/// SPEC-464: `--credits-out` writes the refusal of a model whose usage credits are used up as
/// Claude Code 2.1.311 does (no `rate_limit_info`), then goal mode's pause notice and the record
/// closing the turn; `--credits-reset-s` adds 2.1.322's `rate_limit_info` with the reset.
#[test]
fn a_usage_credits_refusal_is_written_as_either_claude_code_version_does() -> TestResult {
    let dir = tempfile::tempdir()?;
    let transcript =
        |name: &str, extra: &[&str]| -> Result<Vec<Value>, Box<dyn std::error::Error>> {
            let path = dir.path().join(format!("projects/x/{name}.jsonl"));
            let path_text = path.to_string_lossy().into_owned();
            let mut args = vec![
                "--session-id",
                name,
                "--transcript",
                &path_text,
                "--limit-after",
                "1",
                "--credits-out",
            ];
            args.extend_from_slice(extra);
            let output = run(&args, "one\n/exit\n")?;
            assert!(stdout(&output).contains("ECHO: one\nLIMIT REACHED"));
            read_jsonl(&path)
        };
    let old = transcript("s-old", &[])?;
    assert_eq!(old.len(), 5, "{old:?}");
    assert_eq!(old[2]["apiError"], "model_requires_usage_credits");
    assert_eq!(old[2]["error"], "rate_limit");
    assert_eq!(old[2]["apiErrorStatus"], 429);
    assert!(old[2].get("apiErrorParams").is_none());
    assert_eq!(
        old[2]["message"]["content"][0]["text"],
        "You're out of usage credits. /model to switch models."
    );
    assert_eq!(old[3]["subtype"], "informational");
    assert_eq!(
        old[3]["content"],
        "Goal paused \u{b7} the request was rate limited \u{b7} send a message to retry"
    );
    assert_eq!(old[4]["subtype"], "turn_duration");

    let new = transcript("s-new", &["--credits-reset-s", "3600"])?;
    let info = &new[2]["apiErrorParams"]["rate_limit_info"];
    assert_eq!(info["status"], "rejected");
    assert_eq!(info["rateLimitType"], "seven_day_overage_included");
    assert_eq!(info["overageDisabledReason"], "out_of_credits");
    assert!(info["resetsAt"].is_i64(), "{info}");

    // Every later message gets its own refusal: its prompt, the error, the notice, the turn's end.
    let path = dir.path().join("projects/x/s-again.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let output = run(
        &[
            "--session-id",
            "s-again",
            "--transcript",
            &path_text,
            "--limit-after",
            "1",
            "--credits-out",
        ],
        "one\ntwo\n/exit\n",
    )?;
    assert!(stdout(&output).contains("ECHO: one\nLIMIT REACHED\nLIMIT REACHED\nBYE"));
    let again = read_jsonl(&path)?;
    assert_eq!(again.len(), 9, "{again:?}");
    assert_eq!(again[5]["type"], "user");
    assert_eq!(again[5]["message"]["content"], "two");
    assert_eq!(again[6]["apiError"], "model_requires_usage_credits");
    assert_eq!(again[6]["parentUuid"], again[5]["uuid"]);
    assert_ne!(again[6]["uuid"], again[2]["uuid"]);
    assert_eq!(again[7]["subtype"], "informational");
    assert_eq!(again[8]["subtype"], "turn_duration");
    assert_ne!(again[8]["uuid"], again[4]["uuid"]);

    // `--limit-after 0`: out of credits from the first message on, which gets no answer.
    let path = dir.path().join("projects/x/s-start.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let output = run(
        &[
            "--session-id",
            "s-start",
            "--transcript",
            &path_text,
            "--limit-after",
            "0",
            "--credits-out",
        ],
        "one\n/exit\n",
    )?;
    let text = stdout(&output);
    assert!(
        text.contains("LIMIT REACHED") && !text.contains("ECHO: one"),
        "{text}"
    );
    let start = read_jsonl(&path)?;
    assert_eq!(start.len(), 4, "{start:?}");
    assert_eq!(start[0]["message"]["content"], "one");
    assert_eq!(start[1]["apiError"], "model_requires_usage_credits");
    assert_eq!(start[3]["subtype"], "turn_duration");
    Ok(())
}

/// SPEC-117: like Claude Code 2.1.292, a `/goal` prompt argument sets the goal and runs its turn
/// before input is read; other prompt arguments are echoed.
#[test]
fn a_goal_prompt_argument_sets_the_goal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("projects/x/s-3.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let output = run(
        &[
            "--session-id",
            "s-3",
            "--transcript",
            &path_text,
            "--prompt",
            "/goal Make it blue",
        ],
        "/exit\n",
    )?;
    let text = stdout(&output);
    assert!(
        text.starts_with("FAKE-AGENT READY session=s-3\nGOAL SET\nECHO: /goal Make it blue\n"),
        "{text}"
    );
    let records = read_jsonl(&path)?;
    assert_eq!(records[0]["attachment"]["type"], "goal_status");
    assert_eq!(records[0]["attachment"]["condition"], "Make it blue");
    let plain = run(&["--session-id", "s-4", "--prompt", "Hello"], "/exit\n")?;
    assert!(
        stdout(&plain).contains("ECHO: Hello\nBYE"),
        "{}",
        stdout(&plain)
    );
    Ok(())
}

#[test]
fn only_claude_transcripts_are_written() -> TestResult {
    let output = Command::new(EXE)
        .args([
            "--transcript",
            "/tmp/unused.jsonl",
            "--transcript-kind",
            "codex",
        ])
        .stdin(Stdio::null())
        .output()?;
    assert!(!output.status.success(), "other kinds are refused");
    Ok(())
}

#[test]
fn fork_child_spawns_a_live_descendant() -> TestResult {
    let mut child = Command::new(EXE)
        .args(["--mode", "fork-child", "--seconds", "30"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    // The descendant inherits stdout, so read lines instead of waiting for EOF.
    let mut lines = std::io::BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let pid: i32 = loop {
        let line = lines.next().ok_or("no CHILD line")??;
        if let Some(pid) = line.strip_prefix("CHILD ") {
            break pid.parse()?;
        }
    };
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(b"/exit\n")?;
    assert_eq!(child.wait()?.code(), Some(0));
    // The descendant outlives its parent until it is killed.
    let descendant = Pid::from_raw(pid);
    let alive = kill(descendant, None).is_ok();
    let _ = kill(descendant, Signal::SIGKILL);
    assert!(alive, "descendant exited with its parent");
    Ok(())
}

#[test]
fn ignore_term_survives_sigterm() -> TestResult {
    let mut child = Command::new(EXE)
        .args(["--mode", "ignore-term"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let pid = Pid::from_raw(i32::try_from(child.id())?);
    // Wait for the READY line so the handlers are installed before signalling.
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let mut first = [0u8; 18];
    std::io::Read::read_exact(&mut out, &mut first)?;
    assert_eq!(&first, b"FAKE-AGENT READY s");
    kill(pid, Signal::SIGTERM)?;
    kill(pid, Signal::SIGINT)?;
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        assert!(child.try_wait()?.is_none(), "fake agent died on SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    }
    kill(pid, Signal::SIGKILL)?;
    child.wait()?;
    Ok(())
}

#[test]
fn invalid_arguments_are_a_usage_error() -> TestResult {
    let output = Command::new(EXE).args(["--mode", "nope"]).output()?;
    assert_eq!(output.status.code(), Some(2));
    Ok(())
}
