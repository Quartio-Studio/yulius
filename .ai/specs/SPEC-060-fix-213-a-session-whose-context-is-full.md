---
yulius-task: 9aa5e52a-abd3-4351-9483-68ca197f1484
status: done
---

# SPEC-060: a session whose context is full is compacted or restarted, not left working

Status: done
Date: 2026-10-13
Scope authorization: task #51, part of task #47 ("fix all open actionable GitHub issues"): fix
GitHub issue #213, "A session whose context is full stays 'working' for hours and never recovers".

## Goal

When a task's Claude Code session can no longer take a prompt because its context is full, Yulius
notices it from the transcript and recovers the session without a person. It first compacts the
session with a focus line built from the task. If that fails, it continues the task in a fresh
session with a handoff summary. Each step shows in the task's terminal and in the log. The task
is never left `working` while nothing runs. If recovery does not help, the task needs a person and
says why.

## Context

**Symptom (issue #213).** On production, task #8 ran out of context at 01:21 UTC on 2026-10-12. From
then on every prompt was answered with "Prompt is too long": the goal checker's Stop-hook feedback,
Yulius's wake-ups and typed messages. For three hours the task showed `working`. Its PR #205 sat
unmerged, and nothing reported the problem until a person read the transcript and typed `/compact`.
The transcript of task #8 (session `593b396e`, lines 5861, 5872 and 5877) shows the three
assistant lines "Prompt is too long" and then the person's `/compact`.

**What Claude Code writes** (2.1.292, read from the installed CLI). A request the API rejects as
too long becomes an assistant record built by its API-error helper:
`isApiErrorMessage: true`, `error: "invalid_request"`, `message.model: "<synthetic>"`, one text
block `Prompt is too long` (Claude Code's `PROMPT_TOO_LONG` constant, which its own check
`startsWith`), and `errorDetails` with the API's text (`prompt is too long: 213000 tokens > 200000
maximum`). The turn ends without a Stop hook (an API error runs `StopFailure`, not `Stop`).
A compaction that succeeds writes a `system` record with `subtype: "compact_boundary"` and
`compactMetadata.trigger` (`manual` or `auto`). A failed `/compact` shows "Error during compaction:
…" as the command's error output.

**Root cause.** `observe_assistant` (`yulius-harness/src/claude.rs`) reads an error record only as
a usage limit. Any other error record yields no observation, so the error ends no turn. The last
prompt had set the task `working` (`UserTurn`), no Stop hook came, and nothing took it back. Nothing
else watched for the condition either. The `autoCompactEnabled: true` run setting (F 2026-10-12)
lowers the odds, but auto-compaction can itself give up: the 2.1.29x changelog lists two such cases.

**What else the cause could break.** Any other API error that ends a turn without a Stop hook
(an image the API refuses, an overloaded API after its retries) also leaves the task `working`
until the next prompt. Those are out of scope; the new detection covers only a full context.

**Related work.** SPEC-032 derives the activity from the transcript (`Observed`, `TranscriptFacts`,
the replay after a restart). SPEC-034 recovers from a usage limit by resuming the session. A full
context cannot be resumed the same way: the resumed session would be just as full.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CTX-001 | A non-sidechain Claude Code error record that says the prompt is too long (text starting `Prompt is too long`, or `errorDetails`/`apiError` naming `prompt is too long`/`prompt_too_long`) is observed as `ContextFull`. A non-sidechain `compact_boundary` record is observed as `Compacted`. A non-sidechain user or system record whose text holds "Error during compaction" is observed as `CompactFailed`. A usage-limit record is still only a limit. | `yulius-harness` unit tests |
| CTX-002 | `ContextFull` ends the turn: the task stops reading `working` (it reads Your turn, `waiting`) and its active time stops. The terminal and the log (audit `task.context_full` and a warning) say the context is full and what Yulius does about it. | `context_full` integration tests |
| CTX-003 | The first `ContextFull` of an episode makes Yulius type `/compact <focus>` into the live session once. The focus is one line built from the task: its number and title, the goal (shortened), the branch, the pull request when there is one, and the steps not yet done. When `Compacted` follows, Yulius types one prompt that tells the agent its context was compacted and to carry on with the task. That prompt reaches the agent. | domain unit tests (focus); integration test with the fake agent |
| CTX-004 | A compaction that fails (`CompactFailed`, `ContextFull` again after the compaction, or no `Compacted` within 10 minutes of the `/compact`) continues the task in a fresh Claude Code session on the same subscription, through the switch path. Its first message is a handoff summary built from the task (goal, branch, pull request, steps left, and how to pick up). The terminal says the task continues in a fresh session. A `ContextFull` that arrives while the compaction is in flight is not a failure (a prompt queued before it). A fresh session that cannot start (the switch is refused or fails) makes the task `needs_attention` with that reason, and the terminal does not speak of subscriptions. | integration tests |
| CTX-005 | A `ContextFull` after the fresh session started, before any successful reply, makes the task `needs_attention`. The terminal and the log give the reason ("the context is still full after compacting and a fresh session") once per episode; later refusals flag the task again silently. Yulius types nothing more. | integration test |
| CTX-006 | An episode ends when the agent replies successfully (a turn completes or the agent works) after a recovery, when the session changes inside the TUI (`/clear`, `/resume`), or when the compaction is cut short (its run ended, or it was never typed). A refusal from another attempt while a compaction is pending starts a compaction of that attempt. A later full context starts a new episode at compaction. A person's prompt is not touched: Yulius types only into the attempt that reported the full context, only while the episode still asks for it, never while a person keeps typing (it gives up after 30 seconds), and stops once that attempt is no longer live. | integration tests; code review |
| CTX-007 | After a Yulius restart, a reattached run whose replayed transcript ends in `ContextFull` (no successful reply or compaction after it) starts recovery the same way, unless the task already needs a person. | `finish.rs` unit test; integration test |
| CTX-008 | Found on the way (pre-existing): a stop's escalation, about 20 s later, terminates only the run that stop ended, not a run of the task continued meanwhile; a transcript watcher ends when a newer one starts for the task, so it never reads the next run's lines as its own run's. | integration test (continued session); debugging evidence below |

Ownership and bounds: the episode state lives in the task's runtime (`TaskRuntime`), in memory, one
per task, and outlives a run so a fresh session counts as the second stage. Each stage types at
most once. The compaction wait is bounded (10 minutes) and cancelled by the attempt ending. Nothing
retries without bound: the last stage is a person.

## Plan

- [x] Spec with root cause written and validated
- [x] Claude Code's full-context, compaction and compaction-failure records are observed
- [x] The fake agent fills its context, compacts and fails compaction on demand
- [x] A full session is compacted with a focus line and the next prompt reaches the agent
- [x] A failed compaction continues the task in a fresh session with a handoff summary
- [x] A second failure flags the task with the reason, also after a restart
- [x] Independent review, docs, ADR and every gate green

## Acceptance criteria

- With a fake agent that answers "Prompt is too long", the task is compacted once and the next
  prompt reaches the agent. Its activity is not left `working`, and the terminal shows the notice.
- With a fake agent whose compaction fails, the task continues in a fresh session (a second
  attempt, a new session) whose first message is the handoff summary.
- With a fake agent whose fresh session is full too, the task is `needs_attention` with the
  reason in the terminal and the audit log, and no third run starts.
- `scripts/dev ci` passes.

## Test plan

- Unit (`yulius-harness`): the error record of Claude Code 2.1.292, the `errorDetails`-only shape,
  a sidechain error (ignored), a usage limit (still a limit), `compact_boundary`, a failed
  compaction in a system record and in a user record.
- Unit (`yulius-domain`): the focus line and the handoff (one line, control characters removed,
  bounded, done steps left out, no pull request).
- Unit (`yulius-app` `finish.rs`): a replay ending in `ContextFull` reads as pending recovery; one
  followed by a reply or a compaction does not.
- Integration (`crates/yulius/tests/context_full.rs`, real ptyhost and the fake agent): compaction
  recovers and the next prompt is echoed; a failed compaction starts a fresh session with the
  handoff; a full fresh session flags the task.

## Decisions

- **Compact first, then a fresh session, then a person** (ADR-0041). `/compact` keeps the most
  work and is what the person did by hand in #213. A fresh session loses the conversation but not
  the work on disk, so it is the fallback. A third automatic try would loop on a task whose
  briefing alone overflows the context.
- **Type one prompt after a compaction.** After `/compact` the session is idle: no turn ran, so no
  Stop hook or goal check wakes it. The prompts lost to the full context (goal feedback, wake-ups)
  are gone. The single typed line says what happened, so it cannot be taken for a person's message.
  SPEC-034's "type nothing" applies to usage limits, where the transcript can be stepped back; here
  it cannot be.
- **Detect a failed compaction three ways** (the error text, another full-context error, a 10-minute
  bound). The CLI's exact record for a failed manual `/compact` is not documented, so the bound
  keeps an unknown shape from waiting forever.
- **Fix the two races found on the way in this change** (CTX-008) rather than file them: the task
  forbids new issues, both are a few lines, and the continued-session test cannot pass without
  them.
- **The reason is said, not stored.** As for a goal that was never taken, the terminal notice and
  the audit entry carry the reason. No new column is added for a flag a person clears by acting.

## Evidence

- Record shapes read from the installed Claude Code 2.1.292: its API-error helper writes
  `error: "invalid_request"` with the `PROMPT_TOO_LONG` text "Prompt is too long" and
  `errorDetails`; a compaction writes `system`/`compact_boundary`. The production transcript of
  task #8 (session `593b396e`, lines 5861–5881) shows the three refusals and the person's `/compact`.
- Red first: the harness tests did not compile before the new `Observed` variants. With the
  supervisor's dispatch of the new observations removed, all four first integration tests failed
  (`timed out waiting for the task not working`, `…the compaction`, `…the fresh session`,
  `…needs attention`). With the replay hook removed, the restart test failed (`timed out waiting
  for the compaction after the restart`).
- Review round 1 (fresh session, om-code-review): request changes, 3 major and 5 minor. Each was
  fixed test-first. `a_continued_session_is_compacted_after_an_unfinished_compaction` fails with
  the M1 fix removed. `a_fresh_session_that_cannot_start_needs_a_person` failed before the M2 fix.
  `a_refused_prompt_does_not_leave_the_task_working` now fails with `idle_after_full_context`
  removed. The harness tests for m5 failed first.
- CTX-008, found while making the continued-session test pass: a plain stop and continue had its
  continued run killed by SIGKILL about 20 s later (`exited code=None signal=Some(9)`, then
  `lost`), from the stop's escalation. The old attempt's transcript watcher also kept running
  after a quick continue: the new run's lines were observed under the old attempt id. Both are
  fixed. The fake harness now resumes the same session id, as `claude --resume` does.
- Review round 2: approve. Its required docs fix (two other features' ADR numbers in
  `docs/changes.md`) and its cheap minors and nits are applied: no typing after a person typed
  since the refusal; a paused fresh session is said; stop's store error; a deleted task's runtime;
  quiet give-ups while stopping; a log on a session change mid-recovery.
- `cargo test -p yulius --test main context_full::`: 7 passed, four runs in a row.
  `scripts/dev ci` on the merged head before the round-2 fixes: 1280 of 1281 passed. The one
  failure was a notice race in a new test (the notice arrives after the echo), fixed by waiting
  for notices. The final gate run is recorded in the pull request.
- Not tested here: a real Claude Code session (the fake agent stands in), the exact transcript
  shape of a failed manual `/compact` in the real CLI (the 10-minute bound covers an unknown
  shape), and whether a real `/compact` keeps the session id (believed yes; a log says so if not).
  `close_task_now` has the same escalation shape as the old stop; it is left as it was, because
  only a done task is closed.

