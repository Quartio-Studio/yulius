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

/// Runs a goal session whose goal Claude Code pauses (`--goal-api-errors 4`) and that is then
/// woken (`--paused-wake-file`, with `extra`), until it prints `marker`; returns the transcript's
/// records after the pause notice. Bounded: a session that never prints it is killed.
fn records_after_the_goal_pause(
    extra: &[&str],
    marker: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("projects/x/s-5.jsonl");
    let wake = dir.path().join("wake");
    std::fs::write(&wake, "")?;
    let mut child = Command::new(EXE)
        .args(["--session-id", "s-5", "--transcript"])
        .arg(&path)
        .args(["--goal-api-errors", "4", "--goal-retry-ms", "1"])
        .arg("--paused-wake-file")
        .arg(&wake)
        .args(extra)
        .args(["--prompt", "/goal Merge on green"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let out = child.stdout.take().ok_or("no stdout")?;
    let (lines, read) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    while !seen.iter().any(|line: &String| line == marker) {
        let left = deadline.saturating_duration_since(Instant::now());
        match read.recv_timeout(left) {
            Ok(line) => seen.push(line),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no {marker:?} in {seen:?}").into());
            }
        }
    }
    // The woken turn's last record follows its marker: the session exits once it is written.
    let closed = || {
        read_jsonl(&path).is_ok_and(|records| {
            records
                .iter()
                .rposition(|r| r["type"] == "user")
                .is_some_and(|woke| {
                    records[woke..]
                        .iter()
                        .any(|r| r["subtype"] == "turn_duration")
                })
        })
    };
    while !closed() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the woken turn never closed".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(b"/exit\n")?;
    child.wait()?;
    assert!(
        seen.iter().any(|line| line == "GOAL PAUSED"),
        "the goal was paused first: {seen:?}"
    );
    let records = read_jsonl(&path)?;
    let pause = records
        .iter()
        .position(|r| {
            r["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("Goal paused after 3 automatic retries"))
        })
        .ok_or("no pause notice")?;
    Ok(records[pause + 1..].to_vec())
}

/// SPEC-469 GPW-005: Claude Code wakes a session whose goal it paused when background work ends,
/// and the woken turn works: the turn the pause closed, the notification as a prompt, a tool call
/// (the model's own line), the answer and the closed turn.
#[test]
fn a_session_woken_after_its_goal_pause_works() -> TestResult {
    let after = records_after_the_goal_pause(&[], "WOKEN DONE")?;
    let kinds: Vec<(&str, &str)> = after
        .iter()
        .map(|r| {
            let detail = match r["type"].as_str() {
                Some("system") => r["subtype"].as_str(),
                Some("assistant") => r["message"]["stop_reason"].as_str(),
                _ => None,
            };
            (r["type"].as_str().unwrap_or(""), detail.unwrap_or(""))
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("system", "turn_duration"),
            ("user", ""),
            ("assistant", "tool_use"),
            ("assistant", "end_turn"),
            ("system", "stop_hook_summary"),
            ("system", "turn_duration"),
        ],
        "{after:?}"
    );
    let prompt = after[1]["message"]["content"].as_str().unwrap_or("");
    assert!(
        prompt.starts_with("<task-notification>") && prompt.contains("<status>completed</status>"),
        "{prompt}"
    );
    assert_eq!(after[2]["message"]["model"], "fake-model");
    assert!(after[2]["isApiErrorMessage"].is_null());
    Ok(())
}

/// SPEC-469 GPW-005: the API refuses the woken turn again (`--paused-wake-dies`): the prompt, the
/// error record Claude Code writes itself and the closed turn; no line of the model's, no new
/// goal notice.
#[test]
fn a_woken_turn_the_api_refuses_again_brings_no_work() -> TestResult {
    let after = records_after_the_goal_pause(&["--paused-wake-dies"], "WAKE DIED")?;
    assert_eq!(after.len(), 4, "{after:?}");
    assert_eq!(after[0]["subtype"], "turn_duration");
    assert_eq!(after[1]["type"], "user");
    assert_eq!(after[2]["type"], "assistant");
    assert_eq!(after[2]["isApiErrorMessage"], true);
    assert_eq!(after[2]["message"]["model"], "<synthetic>");
    assert_eq!(after[3]["subtype"], "turn_duration");
    assert!(
        after.iter().all(|r| r["subtype"] != "informational"),
        "no new notice: {after:?}"
    );
    Ok(())
}

/// SPEC-469 GPW-005: the session goes on with no prompt anyone sees (`--paused-wake-hidden`): a
/// hidden `isMeta` record, then the same work.
#[test]
fn a_session_that_goes_on_by_itself_after_its_goal_pause_has_a_hidden_prompt() -> TestResult {
    let after = records_after_the_goal_pause(&["--paused-wake-hidden"], "WOKEN DONE")?;
    assert_eq!(after[0]["subtype"], "turn_duration");
    assert_eq!(after[1]["type"], "user");
    assert_eq!(after[1]["isMeta"], true);
    assert_eq!(after[2]["message"]["stop_reason"], "tool_use");
    assert_eq!(after[2]["message"]["model"], "fake-model");
    assert_eq!(after[3]["message"]["stop_reason"], "end_turn");
    assert_eq!(
        after.last().ok_or("no records")?["subtype"],
        "turn_duration"
    );
    Ok(())
}

/// SPEC-469: a resumed session starts with the line Claude Code writes itself (`--no-response-line`),
/// before its first turn: the `<synthetic>` model's "No response requested.".
#[test]
fn a_resumed_session_starts_with_claude_codes_own_line() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("projects/x/s-6.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let output = run(
        &[
            "--session-id",
            "s-6",
            "--transcript",
            &path_text,
            "--no-response-line",
        ],
        "one\n/exit\n",
    )?;
    assert!(stdout(&output).contains("ECHO: one"), "{}", stdout(&output));
    let records = read_jsonl(&path)?;
    assert_eq!(records[0]["type"], "assistant");
    assert_eq!(records[0]["message"]["model"], "<synthetic>");
    assert_eq!(records[0]["message"]["stop_reason"], "stop_sequence");
    assert_eq!(
        records[0]["message"]["content"][0]["text"],
        "No response requested."
    );
    assert_eq!(records[1]["type"], "user");
    Ok(())
}

/// SPEC-469: with `--pause-hold-file` the notice that pauses the goal is written only once the
/// file exists; until then the session says `PAUSE HELD` and the transcript ends in the last
/// failed turn's error.
#[test]
fn the_goal_pause_waits_for_its_hold_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("projects/x/s-7.jsonl");
    let hold = dir.path().join("hold-pause");
    let mut child = Command::new(EXE)
        .args(["--session-id", "s-7", "--transcript"])
        .arg(&path)
        .args(["--goal-api-errors", "4", "--goal-retry-ms", "1"])
        .arg("--pause-hold-file")
        .arg(&hold)
        .args(["--prompt", "/goal Merge on green"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut lines = std::io::BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let mut until = |marker: &str| -> TestResult {
        loop {
            let line = lines.next().ok_or_else(|| format!("no {marker:?}"))??;
            if line == marker {
                return Ok(());
            }
        }
    };
    until("PAUSE HELD")?;
    let paused = |records: &[Value]| {
        records.iter().any(|r| {
            r["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("Goal paused"))
        })
    };
    assert!(!paused(&read_jsonl(&path)?), "no pause while it is held");
    std::fs::write(&hold, "")?;
    until("GOAL PAUSED")?;
    assert!(paused(&read_jsonl(&path)?));
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(b"/exit\n")?;
    child.wait()?;
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
