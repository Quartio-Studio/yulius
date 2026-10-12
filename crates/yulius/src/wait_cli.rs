//! `yulius wait`, run by a task's agent (SPEC-011): blocks until the Yulius CI checks at the
//! agent's head finish, its delivery moves (a merge Yulius made, a deploy) or a merge it armed is
//! decided, then says what happened in one line. Agents start it with `run_in_background` and end
//! their turn; Claude Code wakes them when it exits, so nobody polls.

use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

use yulius_app::agent_wait::WaitState;

/// The server endpoint (task token, loopback listener).
const PATH: &str = "/internal/agent/wait";
/// Server-side wait of one request (the server caps it at 55 s).
const REQUEST_WAIT_SECS: u64 = 50;
/// Consecutive failed requests before giving up.
const MAX_FAILURES: u32 = 5;

/// Runs `yulius wait` for at most `minutes`; returns the exit code (0 = something happened or
/// the time is up, 1 = the server cannot be reached).
pub fn run(out: &mut dyn Write, minutes: u64, env: &dyn Fn(&str) -> Option<String>) -> i32 {
    let head = git_head();
    let deadline = Instant::now() + Duration::from_secs(minutes.max(1) * 60);
    let Some(first) = ask(out, head.as_deref(), None, 0, env) else {
        return 1;
    };
    if let Some(line) = settled(&first, None) {
        let _ = writeln!(out, "{line}");
        return 0;
    }
    let _ = writeln!(out, "Waiting: {}", describe(&first));
    let _ = out.flush();
    let mut since = first;
    loop {
        if Instant::now() >= deadline {
            let _ = writeln!(
                out,
                "Still waiting after {minutes} min: {}. Start `yulius wait` again, or look at \
                 `ci_status`.",
                describe(&since)
            );
            return 0;
        }
        let Some(next) = ask(out, head.as_deref(), Some(&since), REQUEST_WAIT_SECS, env) else {
            return 1;
        };
        if next == since {
            continue;
        }
        if let Some(line) = settled(&next, Some(&since)) {
            let _ = writeln!(out, "{line}");
            return 0;
        }
        let _ = writeln!(out, "Waiting: {}", describe(&next));
        let _ = out.flush();
        since = next;
    }
}

/// The line to end with when the wait is over, or `None` to keep waiting.
fn settled(now: &WaitState, before: Option<&WaitState>) -> Option<String> {
    // How an armed merge ended (MSR-003): when it is decided during this wait, or already was
    // at this head when the wait starts.
    let decided = match before {
        // Arming clears the outcome, so a real decision always brings a new one.
        Some(before) => {
            before.merge_armed && !now.merge_armed && now.merge_outcome != before.merge_outcome
        }
        None => {
            !now.merge_armed
                && !now.head.is_empty()
                && now
                    .merge_outcome_head
                    .as_deref()
                    .is_some_and(|h| h.starts_with(&now.head) || now.head.starts_with(h))
        }
    };
    if decided && let Some(outcome) = &now.merge_outcome {
        return Some(outcome.clone());
    }
    if let Some(before) = before
        && now.stage != before.stage
    {
        return Some(format!(
            "Delivery moved: {} -> {}. Check `delivery_status` for what is left.",
            before.stage, now.stage
        ));
    }
    if now.checks_ended() && !now.merge_armed {
        let head = short(&now.head);
        return Some(match now.checks.as_str() {
            "passed" => format!(
                "The checks at {head} passed. Merge with `delivery_merge` (with the independent \
                 review of that head) if you have not."
            ),
            state => format!(
                "The checks at {head} ended {state}{}. Read why with `ci_status`; fix it \
                 test-first, or rerun with `ci_rerun` when the cause was not the code.",
                now.checks_detail
                    .as_deref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ),
        });
    }
    if before.is_none() && now.head.is_empty() && !now.merge_armed {
        return Some(
            "Nothing to wait for: this directory has no commit. Run `yulius wait` in the task's \
             worktree."
                .to_owned(),
        );
    }
    None
}

fn describe(state: &WaitState) -> String {
    let checks = if state.head.is_empty() {
        "no commit".to_owned()
    } else if state.checks == "none" {
        format!("no Yulius CI run at {} yet", short(&state.head))
    } else {
        format!("checks at {} {}", short(&state.head), state.checks)
    };
    let armed = if state.merge_armed {
        ", merge armed for when they pass"
    } else {
        ""
    };
    format!("{checks}{armed}; delivery {}", state.stage)
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}

/// One request; retries a few times on transport errors, reporting the last one.
fn ask(
    out: &mut dyn Write,
    head: Option<&str>,
    since: Option<&WaitState>,
    wait_secs: u64,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<WaitState> {
    let body = serde_json::json!({
        "head": head,
        "since": since,
        "timeout_secs": wait_secs,
    })
    .to_string();
    let mut failures = 0;
    loop {
        let timeout = Duration::from_secs(wait_secs + 15);
        let error = match yulius_git::credential::agent_post(PATH, &body, env, timeout) {
            Ok((200, raw)) => match serde_json::from_slice::<WaitState>(&raw) {
                Ok(state) => return Some(state),
                Err(err) => format!("unreadable answer: {err}"),
            },
            Ok((401 | 403 | 404, _)) => {
                let _ = writeln!(
                    out,
                    "yulius wait works only inside a Yulius task's session (the task token was \
                     refused)."
                );
                return None;
            }
            Ok((status, _)) => format!("HTTP {status}"),
            Err(err) => err.to_string(),
        };
        failures += 1;
        if failures >= MAX_FAILURES {
            let _ = writeln!(out, "yulius wait: Yulius cannot be reached ({error}).");
            return None;
        }
        std::thread::sleep(Duration::from_secs(5 * u64::from(failures)));
    }
}

/// The commit checked out where `yulius wait` runs (the task's worktree).
fn git_head() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(checks: &str) -> WaitState {
        WaitState {
            head: "0123456789abcdef".into(),
            checks: checks.into(),
            stage: "pr_open".into(),
            ..WaitState::default()
        }
    }

    #[test]
    fn ended_checks_settle_the_wait_at_once() {
        assert!(settled(&state("running"), None).is_none());
        assert!(settled(&state("queued"), None).is_none());
        let passed = settled(&state("passed"), None).unwrap_or_default();
        assert!(
            passed.starts_with("The checks at 012345678 passed."),
            "{passed}"
        );
        let failed = WaitState {
            checks_detail: Some("quality exited 101".into()),
            ..state("failed")
        };
        let line = settled(&failed, None).unwrap_or_default();
        assert!(line.contains("ended failed (quality exited 101)"), "{line}");
    }

    #[test]
    fn an_armed_merge_waits_past_green_checks_until_it_is_decided() {
        let armed = WaitState {
            merge_armed: true,
            ..state("passed")
        };
        assert!(settled(&armed, None).is_none());
        let merged = WaitState {
            stage: "merged_integration".into(),
            ..state("passed")
        };
        let line = settled(&merged, Some(&armed)).unwrap_or_default();
        assert!(
            line.starts_with("Delivery moved: pr_open -> merged_integration."),
            "{line}"
        );
    }

    #[test]
    fn a_decided_armed_merge_ends_the_wait_with_how() {
        let armed = WaitState {
            merge_armed: true,
            ..state("running")
        };
        let dropped = WaitState {
            merge_outcome: Some("The merge you armed was dropped: the branch moved on.".into()),
            merge_outcome_head: Some("0123456789abcdef".into()),
            ..state("running")
        };
        let line = settled(&dropped, Some(&armed)).unwrap_or_default();
        assert_eq!(
            line,
            "The merge you armed was dropped: the branch moved on."
        );
        // Merged: the outcome is said rather than the bare stage change.
        let merged = WaitState {
            stage: "merged_integration".into(),
            merge_outcome: Some("Yulius merged pull request #1 into `dev`.".into()),
            merge_outcome_head: Some("0123456789abcdef".into()),
            ..state("passed")
        };
        let line = settled(&merged, Some(&armed)).unwrap_or_default();
        assert!(line.starts_with("Yulius merged pull request #1"), "{line}");
        // A wait started after the decision, at the decided head, is told at once.
        let line = settled(&merged, None).unwrap_or_default();
        assert!(line.starts_with("Yulius merged pull request #1"), "{line}");
        // Unarmed with no outcome (taken back by an immediate merge): not a decision to report.
        assert!(settled(&state("running"), Some(&armed)).is_none());
        // An outcome already there while it read as armed is old news, not this decision.
        let stale_armed = WaitState {
            merge_armed: true,
            ..dropped.clone()
        };
        assert!(settled(&dropped, Some(&stale_armed)).is_none());
        // At another head, an old outcome is not news.
        let elsewhere = WaitState {
            merge_outcome_head: Some("fedcba987".into()),
            ..dropped
        };
        assert!(settled(&elsewhere, None).is_none());
    }

    #[test]
    fn the_state_reads_in_one_line() {
        let armed = WaitState {
            merge_armed: true,
            ..state("running")
        };
        assert_eq!(
            describe(&armed),
            "checks at 012345678 running, merge armed for when they pass; delivery pr_open"
        );
        assert_eq!(
            describe(&state("none")),
            "no Yulius CI run at 012345678 yet; delivery pr_open"
        );
    }
}
