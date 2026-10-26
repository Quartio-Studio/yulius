---
yulius-task: c2b9111b-bb6b-4716-bf0a-f9894beeab70
status: done
---

# SPEC-333: An API error Claude Code retries keeps the task Working; spent retries need you

Status: done
Date: 2026-10-26
Owner: task #317
Scope authorization: task #317 ("this api error should trigger retry, and then needs you status
if retry didnt work. Fix", with two screenshots of task #33).

## Goal

A goal task whose turn an API error ends ("API Error: Connection lost mid-response") reads
Working while Claude Code retries it, and Needs you once Claude Code gives the retries up. It
never reads "Your turn" (or "in review") in between, which made the person type `continue`
themselves.

## Context

- Symptom (task #33, 2026-10-26 15:25): the turn ended on "API Error: Connection lost
  mid-response…", Claude Code printed "Goal still active · the API was unavailable or the
  connection dropped · retrying in 1 min (1/3) · send a message to retry now", and the sidebar
  showed the task idle ("in review <1m", the Waiting state of a task with a pull request). The
  person typed `continue` a minute later.
- Claude Code 2.1.292 retries a goal session itself (read from its bundle and from the
  transcripts of tasks #4 and #116 on this host): an API error that ends a turn with an active
  goal is classified; for a transient one (`api_unavailable`, `unclassified`, `output_limit`, …)
  it writes the `system` notice `Goal still active · <cause> · retrying in <n> min (<k>/3) · send
  a message to retry now`, closes the turn (`turn_duration`, no Stop hook), and 1, 5 then 15
  minutes later (+0–20 %) delivers a `<task-notification>` prompt "Goal check-in: continuing
  after the last turn ended early". After three retries it writes `Goal paused after 3 automatic
  retries · <cause> · send a message to continue` instead and stops. Errors it does not retry
  pause the goal at once: `Goal paused · the API rejected the last request · …`,
  `Goal paused · the last turn could not finish · …`, `Goal paused · the request was rate limited
  · …`. Every Yulius task runs under `/goal`, so Claude Code's retry covers Yulius tasks.
- Root cause: Yulius reads neither notice. The error turn has no Stop hook, so its
  `turn_duration` record settles it as a clean Stop would (`turn_record_seen` →
  `settle_unanswered_turn`, SPEC-290 GDW-001): the task reads Waiting ("Your turn") although
  Claude Code continues it by itself. When the retries are spent, the task reads Waiting too,
  not Needs you, although nothing will go on without a person.
- Why the tests missed it: the fake agent's error turn (`--goal-continue-dies`) writes no API
  error record and no retry notice; SPEC-290's test expects the plain error turn to read Waiting,
  which stays right for an error Claude Code does not retry.
- What else the cause could break: a restart while a retry is armed replays the transcript to a
  closed turn and settles the task Waiting the same way (`TranscriptFacts`).

## Requirements

- AER-001: a turn that closes after Claude Code armed a goal retry leaves the running task
  Working until the retry's prompt (or a person's) starts the next turn; the transcript's replay
  after a restart does the same.
- AER-002: when Claude Code gives the retries up, or pauses the goal on an API error it does not
  retry, the task is in Needs you: activity NeedsAttention and an open request saying what
  happened (the cause in Claude Code's words) and what to do (send a message to continue). A
  prompt typed into the task ends the request, as every Needs you request ends.
- AER-003: Yulius types nothing itself: Claude Code owns the retry (no second prompt racing its
  check-in). A pause Yulius's own hooks cause (`a hook ended the turn`), a goal check timeout and
  a usage limit (its own pause and resume, SPEC-072) are not API errors and keep their paths.

## Plan

- [x] Root cause and spec of the error-ended goal turn
- [x] Fake agent replays the API error and Claude Code's goal retries
- [x] Failing regression tests: a retried turn reads Working, spent retries read Needs you
- [x] A turn whose retry Claude Code armed stays Working
- [x] Spent or refused retries put the task in Needs you, also after a restart
- [x] Docs, ADR and changes entry true to the fix
- [x] Independent review answered: round-1 findings fixed test-first, round 2 approved
- [x] Gates green and merged on green

## Acceptance criteria

- The two integration tests fail on `dev` (the task reads Waiting) and pass with the fix.
- Unit tests of the notice parser cover the armed, spent and refused notices and leave a hook
  pause, a usage-limit pause and other notices alone.
- `scripts/dev ci` passes.

## Test plan

- `yulius-harness` unit tests: `observe_line` of Claude Code 2.1.292's notices (copied from real
  transcripts) → `Observed::GoalRetry { armed, cause }`; sidechain and unrelated notices → none.
- `finish` unit tests: `TranscriptFacts` keeps an armed retry over the turn record and drops it
  at the next prompt or agent work; a spent retry is folded as pending Needs you.
- Integration tests in `crates/yulius/tests/task_done.rs` with the fake agent
  (`--goal-api-errors N`, `--goal-retry-ms MS`, `--goal-check-errors`, `--goal-cleared-on-retry`):
  - N = 2 fails the goal turn and its first retry, and the second retry achieves the goal. The
    task never reads Waiting from the error to Done.
  - N = 4 spends the three retries. The task reads NeedsAttention with an open request and keeps
    it once the turn's record is read. This is tested for API errors and for failed goal checks,
    whose Stop hook ran.
  - A goal cleared while a retry is armed leaves the task Waiting, not Working for good.
  - Yulius restarts while a retry is armed (the task stays Working, then Done) and after the
    retries were spent (Needs you, with Claude Code's words).

## Decisions

- A goal cleared, achieved or failed while a retry is armed ends it, and the closed turn settles.
  Claude Code drops its check-in silently (found in review round 1).
- A failed goal check ("the goal check could not complete") is retried by Claude Code the same
  way and handled the same: any goal notice ends the wait for a verdict.

- [ADR-0225](../../docs/decisions/0225-claude-code-owns-the-goal-retry-after-an-api-error.md):
  Yulius follows Claude Code's goal retry instead of retrying itself.

## Evidence

- **Reproduction.** The real records come from Claude Code 2.1.292's bundle and from transcripts on
  this host (task #116: error, `Goal still active … (1/3)`, `turn_duration`; task #4: the check-in
  prompt about 1 min later). On the tests-only commit `23690b3b` the fake-agent replay read
  `[Working, Waiting, Working, Waiting, Working, Done]` (expected `[Working, Done]`), and spent
  retries never read NeedsAttention. Issue #927.
- **Fix.** Both tests pass with the fix. The restart tests fail with the replay block removed
  ("seen [Working, Waiting, Working, Done]", "timed out waiting for Needs you") and pass with it.
  The review round-1 tests (goal cleared while armed, spent goal-check retries, the replay keeping
  Claude Code's words) failed before their fixes and pass at `e1c10632`.
- **Suites.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^task_done::/)'`:
  62 passed. Unit tests: `finish::` (20 passed) and
  `claude::tests::goal_retry_notices_are_observed` passed.
- **Gate.** `scripts/dev ci` at `e1c10632` (with this commit's docs edits in the tree) exited 0:
  2641 tests run, 2641 passed, 16 skipped. The first run, at `31b5db86`, failed on clippy
  (`if_same_then_else` in the parser), which was fixed in `e1c10632`.
- **Security scan.** `scripts/dev security` exited 2: not run, because Vidoc is out of quota for
  this key (HTTP 429, "This API key has used its 50,000,000 tokens"). That is unrun, not passed.
- **Independent review** (a fresh session, read only):
  - Round 1 approved `31b5db86` with four own-code minors and nits. Each was fixed test-first in
    `e1c10632`, except two nits it accepted: the fake's fixed "1 min" delay, and no test of a task
    an older binary had already settled.
  - Round 2 approved `e1c10632`. Its remaining docs nits went into `9519d05d`.
- **Not tested.**
  - A real Claude Code session losing its connection: the fake agent replays the recorded shapes.
  - The real 1, 5 and 15 minute delays: the tests use 1–15 s.
  - A goal cleared while a retry is armed, replayed after a restart: covered by the fold unit test
    only.
