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

/// `/tool` is the agent going on with no prompt: one tool call in the transcript, no user record.
#[test]
fn the_tool_command_appends_one_tool_call() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("projects/x/s-8.jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let output = run(
        &["--session-id", "s-8", "--transcript", &path_text],
        "/tool\n/notify permission_prompt\n/exit\n",
    )?;
    let text = stdout(&output);
    assert!(text.contains("TOOL\nNOTIFIED\nBYE"), "{text}");
    let records = read_jsonl(&path)?;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["type"], "assistant");
    assert_eq!(records[0]["message"]["stop_reason"], "tool_use");
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

/// A stand-in for the Yulius executable the agent runs its hooks with (`--hooks`): it appends
/// `<event> <prompt>` to the log beside it and answers a Stop with a block while the file
/// `<log>.block` exists, taking the file away.
fn hooks_exe(dir: &std::path::Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    let exe = dir.join("hooks");
    let log = dir.join("hooks.log");
    let script = format!(
        "#!/bin/sh\nlog='{}'\ninput=\"$(cat)\"\n\
         prompt=\"$(printf '%s' \"$input\" | sed -n 's/.*\"prompt\":\"\\([^\"]*\\)\".*/\\1/p')\"\n\
         if [ \"$3\" = Stop ] && [ -e \"$log.block\" ]; then\n  rm -f \"$log.block\"\n\
         \x20 printf '%s\\n' \"Stop blocked\" >> \"$log\"\n\
         \x20 printf '{{\"decision\":\"block\",\"reason\":\"again\"}}\\n'\n  exit 0\nfi\n\
         printf '%s\\n' \"$3 $prompt\" >> \"$log\"\n",
        log.display()
    );
    std::fs::write(&exe, script)?;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
    Ok(exe)
}

/// The hooks the stand-in logged so far, one per line.
fn hooks_logged(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("hooks.log"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect()
}

/// Waits (bounded) until `done` holds, looking every few milliseconds.
fn until(what: &str, mut done: impl FnMut() -> bool) -> TestResult {
    let started = Instant::now();
    while !done() {
        if started.elapsed() > Duration::from_secs(30) {
            return Err(format!("no {what} within 30 s").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// The `type`, and what tells records of that type apart, of each transcript record.
fn shapes(path: &std::path::Path) -> Vec<String> {
    read_jsonl(path)
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let kind = r["type"].as_str().unwrap_or("?");
            match kind {
                "queue-operation" => format!(
                    "{} {}",
                    r["operation"].as_str().unwrap_or("?"),
                    r["content"].as_str().unwrap_or("-")
                ),
                "user" => format!(
                    "user {} {}",
                    r["promptSource"]
                        .as_str()
                        .unwrap_or(if r["isMeta"] == true { "meta" } else { "-" }),
                    r["message"]["content"]
                        .as_str()
                        .unwrap_or("?")
                        .lines()
                        .next()
                        .unwrap_or("")
                ),
                "system" => r["subtype"].as_str().unwrap_or("?").to_owned(),
                _ => kind.to_owned(),
            }
        })
        .collect()
}

/// A fake agent with a transcript, the stand-in hooks, a turn held until `hold` exists, and
/// `extra` arguments.
fn held_agent(
    dir: &std::path::Path,
    extra: &[&str],
) -> Result<std::process::Child, Box<dyn std::error::Error>> {
    let exe = hooks_exe(dir)?;
    Ok(Command::new(EXE)
        .args(["--session-id", "s-q", "--transcript"])
        .arg(dir.join("s-q.jsonl"))
        .arg("--hooks")
        .arg(exe)
        .arg("--turn-hold-file")
        .arg(dir.join("hold"))
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?)
}

/// SPEC-471 CCQ-005: with `--queue-prompts` the agent takes a prompt entered while a turn runs
/// as Claude Code 2.1.322 does: its prompt hook at the Enter and an `enqueue` record; a Stop
/// that is blocked releases nothing and the turn goes on to another Stop; after the Stop that
/// ends the turn all that wait are handed on (a `dequeue` each, before the ended turn's closing
/// records) and answered in one turn, with no further prompt hook.
#[test]
fn queued_prompts_get_their_hook_at_the_enter_and_one_turn_after_the_ending_stop() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &["--queue-prompts"])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\n")?;
    until("the first prompt's hook", || {
        hooks_logged(dir) == ["UserPromptSubmit first"]
    })?;
    // Entered while the turn runs: each gets its hook and its record now, not after the turn.
    stdin.write_all(b"second\n")?;
    until("the second prompt's hook", || hooks_logged(dir).len() == 2)?;
    stdin.write_all(b"third\n")?;
    until("the waiting prompts' records", || {
        shapes(&path)
            .iter()
            .filter(|s| s.starts_with("enqueue"))
            .count()
            == 2
    })?;
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "UserPromptSubmit second",
            "UserPromptSubmit third"
        ],
        "no Stop yet: the turn is held"
    );
    assert_eq!(
        shapes(&path),
        ["user - first", "enqueue second", "enqueue third"]
    );
    // The turn ends; its first Stop is blocked, the second lets it end.
    std::fs::write(dir.join("hooks.log.block"), "")?;
    std::fs::write(dir.join("hold"), "")?;
    // Its closing records are the last thing a turn writes: what is entered now is a command.
    until("the handed-on turn's end", || shapes(&path).len() == 15)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    let text = stdout(&output);
    assert_eq!(
        hooks_logged(dir)[3..],
        ["Stop blocked", "Stop", "Stop", "SessionEnd"],
        "no prompt hook at the hand-over"
    );
    assert_eq!(
        shapes(&path),
        [
            "user - first",
            "enqueue second",
            "enqueue third",
            "assistant",
            // The blocked Stop: the turn goes on, and nothing was handed on.
            "user meta Stop hook feedback:",
            "assistant",
            "dequeue -",
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user queued second",
            "user queued third",
            "assistant",
            "stop_hook_summary",
            "turn_duration",
        ]
    );
    let order = [
        "QUEUED: second",
        "QUEUED: third",
        "ECHO: first",
        "STOP BLOCKED",
        "ECHO: second",
        "ECHO: third",
        "BYE",
    ];
    let mut rest = text.as_str();
    for word in order {
        let at = rest
            .find(word)
            .ok_or_else(|| format!("no {word:?} in order: {text}"))?;
        rest = &rest[at + word.len()..];
    }
    Ok(())
}

/// SPEC-471 CCQ-003, CCQ-005: a waiting line that begins with `/scheduled ` stands for a prompt
/// Claude Code scheduled for itself, in the order 2.1.322 was seen to have: its fire and its
/// `enqueue` with no hook; at the hand-over the `dequeue`, then the prompt hook with the plain
/// text, then a prompt record that is `isMeta`; its turn starts with a tool call.
#[test]
fn a_scheduled_prompt_gets_its_hook_at_the_hand_over_and_a_meta_record() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &["--queue-prompts"])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\n")?;
    until("the first prompt's hook", || hooks_logged(dir).len() == 1)?;
    stdin.write_all(b"/scheduled check the build\n")?;
    until("the scheduled prompt's record", || shapes(&path).len() == 3)?;
    assert_eq!(
        hooks_logged(dir),
        ["UserPromptSubmit first"],
        "no hook when it fires: nobody pressed Enter"
    );
    std::fs::write(dir.join("hold"), "")?;
    until("the scheduled turn's end", || shapes(&path).len() == 12)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "Stop",
            "UserPromptSubmit check the build",
            "Stop",
            "SessionEnd"
        ],
        "its hook comes at the hand-over, with the plain text"
    );
    assert_eq!(
        shapes(&path),
        [
            "user - first",
            "scheduled_task_fire",
            "enqueue check the build",
            "assistant",
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user system check the build",
            "assistant",
            "assistant",
            "stop_hook_summary",
            "turn_duration",
        ]
    );
    let records = read_jsonl(&path)?;
    assert_eq!(records[7]["isMeta"], true, "read as no prompt by Yulius");
    assert_eq!(records[7]["queuePriority"], "later");
    assert_eq!(
        records[8]["message"]["stop_reason"], "tool_use",
        "its turn works before it answers"
    );
    assert!(stdout(&output).contains("ECHO: check the build"));
    Ok(())
}

/// SPEC-479 SWQ-005: a waiting line that begins with `/stuck ` stands for an entry Claude Code
/// never takes out of its queue (some of its own notifications): it gets its `enqueue` record
/// and no prompt hook, the turn's end hands on what else waits and not it, and it is never
/// answered. A turn at whose end only such an entry waits closes like one with an empty queue.
#[test]
fn a_stuck_entry_is_recorded_and_never_handed_on() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &["--queue-prompts"])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\n")?;
    until("the first prompt's hook", || hooks_logged(dir).len() == 1)?;
    stdin.write_all(b"/stuck a notification\n")?;
    until("the stuck entry's record", || shapes(&path).len() == 2)?;
    stdin.write_all(b"second\n")?;
    until("the waiting prompt's record", || shapes(&path).len() == 3)?;
    std::fs::write(dir.join("hold"), "")?;
    until("the handed-on turn's end", || shapes(&path).len() == 11)?;
    // A turn of its own, at whose end nothing but the stuck entry waits.
    stdin.write_all(b"third\n")?;
    until("the third turn's end", || shapes(&path).len() == 15)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    let text = stdout(&output);
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "UserPromptSubmit second",
            "Stop",
            "Stop",
            "UserPromptSubmit third",
            "Stop",
            "SessionEnd"
        ],
        "nobody entered the stuck entry: no prompt hook"
    );
    assert_eq!(
        shapes(&path),
        [
            "user - first",
            "enqueue a notification",
            "enqueue second",
            "assistant",
            // One hand-over, of the prompt: the stuck entry stays.
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user queued second",
            "assistant",
            "stop_hook_summary",
            "turn_duration",
            "user - third",
            "assistant",
            "stop_hook_summary",
            "turn_duration",
        ]
    );
    assert!(text.contains("QUEUED: a notification"), "{text}");
    assert!(text.contains("ECHO: second"), "{text}");
    assert!(!text.contains("ECHO: a notification"), "{text}");
    assert!(!text.contains("ECHO: /stuck"), "{text}");
    Ok(())
}

/// SPEC-479 SWQ-005: Claude Code hands its queue on in the order of the entries, and only
/// prompts that follow one another leave together (seen with 2.1.322): a notification of its
/// own, a slash command or a `!` line leaves alone, and what waits behind it stays through
/// that turn's closing record until the next turn's end. A waiting line that begins with
/// `/alone ` stands for such an entry: no prompt hook when it is entered; at its hand-over
/// the `dequeue`, the ended turn's closing records, its prompt hook and a prompt record
/// (`promptSource: "system"`).
#[test]
fn an_entry_handed_on_alone_leaves_the_rest_for_the_next_turns_end() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &["--queue-prompts"])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\n")?;
    until("the first prompt's hook", || hooks_logged(dir).len() == 1)?;
    stdin.write_all(b"/alone note\n")?;
    until("the entry's record", || shapes(&path).len() == 2)?;
    assert_eq!(hooks_logged(dir).len(), 1, "no hook when it is entered");
    stdin.write_all(b"second\nthird\n")?;
    until("the waiting prompts' records", || shapes(&path).len() == 4)?;
    std::fs::write(dir.join("hold"), "")?;
    until("the last turn's end", || shapes(&path).len() == 19)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    let text = stdout(&output);
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "UserPromptSubmit second",
            "UserPromptSubmit third",
            "Stop",
            "UserPromptSubmit note",
            "Stop",
            "Stop",
            "SessionEnd"
        ],
        "its hook comes at its hand-over"
    );
    assert_eq!(
        shapes(&path),
        [
            "user - first",
            "enqueue note",
            "enqueue second",
            "enqueue third",
            "assistant",
            // The first turn's end hands the entry on alone: two prompts still wait when the
            // turn closes.
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user system note",
            "assistant",
            // Its turn's end hands on the prompts, together.
            "dequeue -",
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user queued second",
            "user queued third",
            "assistant",
            "stop_hook_summary",
            "turn_duration",
        ]
    );
    let order = ["ECHO: first", "ECHO: note", "ECHO: second", "ECHO: third"];
    let mut rest = text.as_str();
    for word in order {
        let at = rest
            .find(word)
            .ok_or_else(|| format!("no {word:?} in order: {text}"))?;
        rest = &rest[at + word.len()..];
    }
    Ok(())
}

/// SPEC-479 SWQ-005 (review round 2): the other order. Prompts entered ahead of an `/alone `
/// line leave together at the turn's end, without it: the entry waits through that turn's
/// closing records and leaves, alone, at the next turn's end (as 2.1.322 does with a prompt
/// ahead of a notification or a slash command).
#[test]
fn the_lines_ahead_of_an_entry_handed_on_alone_leave_first() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &["--queue-prompts"])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\n")?;
    until("the first prompt's hook", || hooks_logged(dir).len() == 1)?;
    stdin.write_all(b"second\nthird\n/alone note\n")?;
    until("the waiting lines' records", || shapes(&path).len() == 4)?;
    std::fs::write(dir.join("hold"), "")?;
    until("the last turn's end", || shapes(&path).len() == 19)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    let text = stdout(&output);
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "UserPromptSubmit second",
            "UserPromptSubmit third",
            "Stop",
            "Stop",
            "UserPromptSubmit note",
            "Stop",
            "SessionEnd"
        ]
    );
    assert_eq!(
        shapes(&path),
        [
            "user - first",
            "enqueue second",
            "enqueue third",
            "enqueue note",
            "assistant",
            // The prompts leave together; the entry still waits when the turn closes.
            "dequeue -",
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user queued second",
            "user queued third",
            "assistant",
            // The prompts' turn ends: the entry leaves, alone.
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user system note",
            "assistant",
            "stop_hook_summary",
            "turn_duration",
        ]
    );
    let order = ["ECHO: first", "ECHO: second", "ECHO: third", "ECHO: note"];
    let mut rest = text.as_str();
    for word in order {
        let at = rest
            .find(word)
            .ok_or_else(|| format!("no {word:?} in order: {text}"))?;
        rest = &rest[at + word.len()..];
    }
    Ok(())
}

/// SPEC-479 SWQ-005: with `--queue-prompts`, `--limit-after N` cuts the handed-on turn that
/// would be the Nth: its prompt records are written, then the usage-limit record; it is not
/// answered and runs no Stop hook, as a turn a limit cuts in Claude Code.
#[test]
fn a_handed_on_turn_can_be_cut_by_the_limit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &["--queue-prompts", "--limit-after", "2"])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\n")?;
    until("the first prompt's hook", || hooks_logged(dir).len() == 1)?;
    stdin.write_all(b"second\n")?;
    until("the waiting prompt's record", || shapes(&path).len() == 2)?;
    std::fs::write(dir.join("hold"), "")?;
    until("the limit's record", || shapes(&path).len() == 8)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    let text = stdout(&output);
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "UserPromptSubmit second",
            "Stop",
            "SessionEnd"
        ],
        "the first turn's Stop alone: a turn the limit cuts runs none"
    );
    assert_eq!(
        shapes(&path),
        [
            "user - first",
            "enqueue second",
            "assistant",
            "dequeue -",
            "stop_hook_summary",
            "turn_duration",
            "user queued second",
            "assistant",
        ]
    );
    let records = read_jsonl(&path)?;
    assert_eq!(records[7]["apiError"], "usage_limit_reached");
    assert!(text.contains("ECHO: first"), "{text}");
    assert!(text.contains("LIMIT REACHED"), "{text}");
    assert!(!text.contains("ECHO: second"), "{text}");
    Ok(())
}

/// SPEC-471 CCQ-005: without the option the agent is as before: a line entered while a turn
/// runs is read after that turn, gets its prompt hook then, and leaves no queue record.
#[test]
fn without_the_option_a_line_entered_during_a_turn_is_read_after_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let mut child = held_agent(dir, &[])?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    stdin.write_all(b"first\nsecond\n")?;
    until("the first prompt's hook", || !hooks_logged(dir).is_empty())?;
    std::fs::write(dir.join("hold"), "")?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    child.wait()?;
    assert_eq!(
        hooks_logged(dir),
        [
            "UserPromptSubmit first",
            "Stop",
            "UserPromptSubmit second",
            "Stop",
            "SessionEnd"
        ]
    );
    assert_eq!(
        shapes(&path),
        ["user - first", "assistant", "user - second", "assistant"]
    );
    Ok(())
}

/// SPEC-471 CCQ-005: a test says when a waiting line's record is written
/// (`--queue-record-file`: Claude Code writes it a moment after the hook), when the queue is
/// handed on (`--queue-handover-file`) and when the handed-on turn starts
/// (`--queue-turn-file`); the agent says when it waits for the last two.
#[test]
fn the_record_the_hand_over_and_the_handed_on_turn_can_be_held() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (dir, path) = (dir.path(), dir.path().join("s-q.jsonl"));
    let (handover, turn, written) = (dir.join("handover"), dir.join("turn"), dir.join("written"));
    let mut child = Command::new(EXE)
        .args(["--session-id", "s-q", "--transcript"])
        .arg(&path)
        .arg("--hooks")
        .arg(hooks_exe(dir)?)
        .arg("--turn-hold-file")
        .arg(dir.join("hold"))
        .arg("--queue-prompts")
        .arg("--queue-record-file")
        .arg(&written)
        .arg("--queue-handover-file")
        .arg(&handover)
        .arg("--queue-turn-file")
        .arg(&turn)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut lines = std::io::BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let mut said = |marker: &str| -> TestResult {
        loop {
            let line = lines.next().ok_or_else(|| format!("no {marker:?}"))??;
            if line == marker {
                return Ok(());
            }
        }
    };
    stdin.write_all(b"first\n")?;
    said("TURN HELD")?;
    stdin.write_all(b"second\n")?;
    // Said once its hook returned: its record comes only when the test says so.
    said("QUEUED: second")?;
    assert_eq!(hooks_logged(dir).len(), 2);
    assert_eq!(shapes(&path), ["user - first"], "its record is held");
    std::fs::write(&written, "")?;
    until("the waiting prompt's record", || shapes(&path).len() == 2)?;
    // The turn ends: nothing is handed on until the test says so.
    std::fs::write(dir.join("hold"), "")?;
    said("QUEUE HELD")?;
    assert_eq!(hooks_logged(dir)[2..], ["Stop"]);
    assert_eq!(
        shapes(&path),
        ["user - first", "enqueue second", "assistant"],
        "the queue is held"
    );
    std::fs::write(&handover, "")?;
    said("QUEUED TURN HELD")?;
    assert_eq!(
        shapes(&path)[3..],
        ["dequeue -", "stop_hook_summary", "turn_duration"],
        "the handed-on turn is held"
    );
    std::fs::write(&turn, "")?;
    said("ECHO: second")?;
    until("the handed-on turn's end", || shapes(&path).len() == 10)?;
    stdin.write_all(b"/exit\n")?;
    drop(stdin);
    said("BYE")?;
    child.wait()?;
    assert_eq!(
        shapes(&path)[6..],
        [
            "user queued second",
            "assistant",
            "stop_hook_summary",
            "turn_duration"
        ]
    );
    Ok(())
}

/// SPEC-486 WFL-007: a workflow as Claude Code 2.1.323 records one. The launch and the turn's
/// end are in the session's transcript; the usage limit that refuses the run's second agent is
/// in that agent's file only. A resumed process names the unfinished run for the transcript
/// only, and a prompt naming the run and `resumeFromRunId` resumes it to its end.
#[test]
fn a_workflow_is_launched_paused_by_a_limit_and_resumed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let transcript = dir.path().join("s-wf.jsonl");
    let path = transcript.to_str().ok_or("a path")?;
    let args = [
        "--session-id",
        "s-wf",
        "--transcript",
        path,
        "--transcript-kind",
        "claude",
    ];
    let first = run(
        &[&args[..], &["--workflow-limit"]].concat(),
        "/workflow\n/exit\n",
    )?;
    let text = stdout(&first);
    let run_id = text
        .lines()
        .find_map(|line| line.strip_prefix("WORKFLOW ")?.split(' ').next())
        .ok_or("the run's id")?
        .to_owned();
    assert!(
        text.contains(&format!("WORKFLOW {run_id} PAUSED")),
        "{text}"
    );
    let records = read_jsonl(&transcript)?;
    let launch = records
        .iter()
        .find_map(|r| r.get("toolUseResult"))
        .ok_or("the launch's tool result")?;
    assert_eq!(launch["status"], "async_launched");
    assert_eq!(launch["taskType"], "local_workflow");
    assert_eq!(launch["runId"], run_id.as_str());
    let task = launch["taskId"].as_str().ok_or("a task id")?.to_owned();
    let last = records.last().ok_or("records")?;
    assert_eq!(last["subtype"], "turn_duration");
    assert_eq!(last["pendingWorkflowCount"], 1);
    let own = std::fs::read_to_string(&transcript)?;
    assert!(!own.contains("usage_limit_reached"), "not in the session's");
    let run_dir = dir.path().join("s-wf/subagents/workflows").join(&run_id);
    let mut agents = Vec::new();
    for entry in std::fs::read_dir(&run_dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with("agent-") && name.ends_with(".jsonl") {
            agents.extend(read_jsonl(&path)?);
        }
    }
    assert_eq!(agents.len(), 2, "a finished agent and a refused one");
    assert!(agents.iter().all(|r| r["isSidechain"] == true));
    // Each has its meta file, which names no model: they run the session's.
    let metas: Vec<Value> = std::fs::read_dir(&run_dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with(".meta.json"))
        .filter_map(|path| serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok())
        .collect();
    assert_eq!(metas.len(), 2);
    assert!(
        metas.iter().all(|meta| meta.get("model").is_none()),
        "{metas:?}"
    );
    let refused: Vec<&Value> = agents
        .iter()
        .filter(|r| r["apiError"] == "usage_limit_reached")
        .collect();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["isApiErrorMessage"], true);
    assert_eq!(
        refused[0]["apiErrorParams"]["rate_limit_info"]["status"],
        "rejected"
    );
    assert!(refused[0]["apiErrorParams"]["rate_limit_info"]["resetsAt"].is_i64());

    // A resumed process with nothing to say: the run is named, and stays unfinished.
    let idle = run(&args, "/exit\n")?;
    assert!(
        stdout(&idle).contains(&format!("WORKFLOW {run_id} ORPHANED")),
        "{}",
        stdout(&idle)
    );
    let notice = read_jsonl(&transcript)?;
    // Handed on from the queue like a prompt, then written for the transcript only.
    let handed: Vec<&str> = notice[notice.len().saturating_sub(3)..]
        .iter()
        .map(|r| r["operation"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(handed, ["enqueue", "dequeue", "-"]);
    let notice = notice.last().ok_or("records")?;
    assert_eq!(notice["queueTranscriptOnly"], true);
    assert_eq!(notice["promptSource"], "system");
    let said = notice["message"]["content"].as_str().unwrap_or_default();
    assert!(
        said.contains(&format!("<task-id>{task}</task-id>")),
        "{said}"
    );
    assert!(said.contains("<status>stopped</status>"), "{said}");

    // A prompt that names another run is a message like any other; one that names this run
    // resumes it.
    let other = run(
        &[
            &args[..],
            &["--prompt", "use resumeFromRunId for run wf_none"],
        ]
        .concat(),
        "/exit\n",
    )?;
    assert!(stdout(&other).contains("ECHO: use resumeFromRunId"));
    let prompt = format!("Resume (run {run_id}) with resumeFromRunId.");
    let resumed = run(&[&args[..], &["--prompt", &prompt]].concat(), "/exit\n")?;
    let text = stdout(&resumed);
    assert!(
        text.contains(&format!("WORKFLOW {run_id} RESUMED")),
        "{text}"
    );
    assert!(
        text.contains(&format!("WORKFLOW {run_id} COMPLETED")),
        "{text}"
    );
    assert!(!text.contains("ECHO: Resume"), "{text}");
    let records = read_jsonl(&transcript)?;
    let relaunch = records
        .iter()
        .rev()
        .find_map(|r| r.get("toolUseResult"))
        .ok_or("the relaunch")?;
    assert_eq!(relaunch["runId"], run_id.as_str());
    let new_task = relaunch["taskId"].as_str().ok_or("a task id")?;
    assert_ne!(new_task, task, "a task of its own");
    let wake = records
        .iter()
        .rev()
        .find(|r| r["origin"]["kind"] == "task-notification")
        .ok_or("the wake-up")?;
    assert!(wake.get("queueTranscriptOnly").is_none());
    let said = wake["message"]["content"].as_str().unwrap_or_default();
    assert!(
        said.contains(&format!("<task-id>{new_task}</task-id>")),
        "{said}"
    );
    assert!(said.contains("<status>completed</status>"), "{said}");
    assert!(
        records
            .last()
            .ok_or("records")?
            .get("pendingWorkflowCount")
            .is_none()
    );
    assert!(
        dir.path()
            .join(format!("s-wf/workflows/{run_id}.json"))
            .exists()
    );
    // Nothing is unfinished any more.
    let after = run(&args, "/exit\n")?;
    assert!(!stdout(&after).contains("ORPHANED"), "{}", stdout(&after));
    Ok(())
}

/// SPEC-486 WFL-007: with `--workflow-limit-model` the refused agent's record is the refusal of
/// a model's usage credits, which names no family; its meta file names the model the agent
/// runs, or none when that is the session's.
#[test]
fn a_workflow_agent_s_model_refusal_names_its_model_in_the_meta_file() -> TestResult {
    for (model, named) in [
        ("claude-opus-5-5", Some("claude-opus-5-5")),
        ("session", None),
    ] {
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("s-wf.jsonl");
        let path = transcript.to_str().ok_or("a path")?;
        let output = run(
            &[
                "--session-id",
                "s-wf",
                "--transcript",
                path,
                "--transcript-kind",
                "claude",
                "--workflow-limit",
                "--workflow-limit-model",
                model,
            ],
            "/workflow\n/exit\n",
        )?;
        assert!(stdout(&output).contains("PAUSED"), "{}", stdout(&output));
        let runs = dir.path().join("s-wf/subagents/workflows");
        let run_dir = std::fs::read_dir(&runs)?.next().ok_or("a run")??.path();
        let mut refused = None;
        for entry in std::fs::read_dir(&run_dir)? {
            let path = entry?.path();
            let name = path.to_string_lossy().into_owned();
            if name.ends_with(".jsonl")
                && name.contains("agent-")
                && std::fs::read_to_string(&path)?.contains("model_requires_usage_credits")
            {
                refused = Some(path);
            }
        }
        let refused = refused.ok_or("the refused agent's file")?;
        let record = &read_jsonl(&refused)?[0];
        assert_eq!(record["isApiErrorMessage"], true);
        assert!(
            record.get("apiErrorParams").is_none(),
            "no family, no reset"
        );
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(
            refused.with_extension("meta.json"),
        )?)?;
        assert_eq!(meta.get("model").and_then(Value::as_str), named, "{model}");
    }
    Ok(())
}
