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
    let limit_text = records[4]["message"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(limit_text.starts_with("Claude AI usage limit reached|"));
    Ok(())
}

#[test]
fn codex_transcript_has_session_meta_and_cumulative_token_counts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("rollout.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    run(
        &[
            "--session-id",
            "s-3",
            "--transcript",
            &path_text,
            "--transcript-kind",
            "codex",
            "--limit-after",
            "2",
        ],
        "a\nbb\n/exit\n",
    )?;
    let records = read_jsonl(&path)?;
    assert_eq!(records[0]["type"], "session_meta");
    assert_eq!(records[0]["payload"]["id"], "s-3");
    let counts: Vec<&Value> = records
        .iter()
        .filter(|r| r["payload"]["type"] == "token_count")
        .collect();
    assert_eq!(counts.len(), 3);
    let total = |r: &Value| r["payload"]["info"]["total_token_usage"]["total_tokens"].as_u64();
    assert!(total(counts[1]) > total(counts[0]));
    assert_eq!(
        counts[2]["payload"]["rate_limits"]["rate_limit_reached_type"],
        "primary"
    );
    assert_eq!(
        counts[2]["payload"]["rate_limits"]["primary"]["used_percent"],
        100.0
    );
    let users: Vec<&Value> = records
        .iter()
        .filter(|r| r["type"] == "response_item" && r["payload"]["role"] == "user")
        .collect();
    assert_eq!(users[1]["payload"]["content"][0]["text"], "bb");
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
