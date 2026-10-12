---
yulius-task: 9d013409-2398-4152-868a-a728a52539ab
status: done
---

# SPEC-010: a goal-mode task's `/goal` is always submitted

Status: implemented
Date: 2026-10-12
Owner: Yulius task #16
Scope authorization: user request: "when I sent a task, it was written as a goal but didnt
start. It happened twice but doesnt happen always. Fix." (screenshot: `/goal <prompt>` sitting
in Claude Code's prompt box, unsubmitted).

## Goal

A goal-mode task (TSK-010) starts every time. Today Yulius types `/goal <prompt>` and one
separate Enter once the TUI is quiet; when the TUI drops the text or the Enter, the goal stays in
the prompt box and the task never starts.

## Context

- `Supervisor::spawn_goal` (`crates/yulius-app/src/supervisor.rs`) types the command, waits 150 ms,
  sends `\r` once and never checks the outcome. Readiness is a heuristic (1.5 s of quiet output).
- Claude Code records the goal in its transcript (`goal_status` attachment, observed as
  `Observation::Goal`), and a submitted prompt is observed as `Observation::UserTurn`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GSB-001 | After typing the goal, Yulius confirms it was taken: a goal record observed for the attempt (a prompt alone is not proof: it may be another). | `goal_submitted` in `TaskRuntime` |
| GSB-002 | Without confirmation after 4 s, Yulius sends Enter again; then clears the prompt box (Ctrl-U) and types the whole command again; then Enter once more. A person typing meanwhile stops the retries. | `spawn_goal` |
| GSB-003 | The retries are bounded (3) and stop when the attempt is no longer live or the input fails. | `spawn_goal` |
| GSB-004 | A TUI that drops the first typed line still starts the goal. | `task_done.rs::a_goal_the_tui_dropped_is_submitted_again` |

## Design

Confirmation comes from the transcript Yulius already observes, not from screen scraping. Extra
Enter on an empty prompt box is harmless; the clear-and-retype only happens after two missed
confirmations (8 s), so a slow transcript cannot duplicate the text in practice.

## Plan

- [x] Confirm the submit from observations and retry it, bounded
- [x] Regression test with a fake agent that drops the first typed line

## Acceptance criteria

- With `--drop-lines 1` the goal task reaches Done; existing goal tests still pass.

## Test plan

`cargo test -p yulius --test task_done`.

## Decisions

- Retry in place rather than restarting the session: the session is healthy, only the input was lost.

## Evidence

- `scripts/dev check`: finished clean (all crates compile).
- `cargo test -p yulius --test task_done`: 5 passed, 0 failed (34.5 s; rerun after the review fixes below), including
  `a_goal_the_tui_dropped_is_submitted_again` (GSB-004) and the four existing goal tests.
- Not run: the full `scripts/dev ci` gate and the e2e suite (no web UI change).
