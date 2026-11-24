---
yulius-task: ddb1cc17-40cd-47c9-8308-27b33f3f0d26
status: done
---

# SPEC-636 Hook and transcript times read on Yulius's clock

## Goal

`task_message::a_draft_the_screen_shows_holds_until_it_is_cleared` passes in whatever order a
prompt's submit hook, its transcript record and the turn's Stop reach Yulius. Times another
process of the host stamps on the wall clock (a hook's send time, a transcript line's time) are
read on Yulius's own clock, also when a test runs that clock shifted. Fixes #1615.

## Context

The test failed once under host load (#1615, production instance at `52b59ecb3`): after Ctrl+U,
the watch half a minute later found the held-message flag gone (`Null`) although the keyboard
minute still held the message.

### Root cause (reproduced, two paths)

The test runs Yulius on a `ShiftedClock` 13 minutes behind the wall clock and moves it forward
later (`-2 min`, `-2 min + 30 s`, …). Two orderings compare a wall-clock stamp with that clock,
and either one makes a prompt of a finished turn read as a new turn, which sets Working over the
held-message flag. Within the keyboard minute nothing raises the flag again
(`BoxHeld::AtKeyboard`), so the next check reads `Null`. The issue's log cannot tell which path
hit #1615.

1. **A late submit hook.** `yulius hook` stamps each request with its wall-clock send time
   (`x-yulius-hook-sent-ms`), and `App::hook_event` took `sent = sent_ms.min(now)`. On the
   shifted clock every stamp lies in the future, so `sent` became the time Yulius *handled* the
   hook. A `UserPromptSubmit` that reaches Yulius more than `HOOK_TIMEOUT` (5 s) late, after its
   client failed open and the agent's Stop settled the turn, then reads as sent after that Stop,
   and the check "a prompt answered after its turn ended changes nothing" (SPEC-346 LPH-002)
   does not hold.
2. **A late transcript read** (found in review round 1). The transcript watcher reads once a
   second (`WATCH_INTERVAL_MS`). It compares a prompt record's time (wall clock) with
   `last_stop_ms` (shifted clock), so a record read after the Stop reads as a new turn
   (`Observation::UserTurn`, ADR-0299). This needs less than a second of slowness between the
   first watch and the check.

Path 1 was reproduced first, with a temporary probe that held the opening submit at the entry of
`hook_event` until the box was cleared. The trace showed `NeedsAttention` → `Working` at the
release, and the test failed at the same assertion. The reviewer reproduced path 2 by pausing
the test thread 900 ms after the first watch, with the fix for path 1 in place. My early probe
on `on_observation` saw no transcript observation only because the unmodified test ends in
0.93 s, before the watcher's first read.

Task #616's reading (a late *Stop* after the transcript record settled the turn) does not hold:
the record that settles a turn without a Stop (`turn_duration`) is written after the Stop hooks,
and a held Stop leaves the task Working, so `idle_full_screen` cannot return before the Stop.

### What else the cause can break

Production is not affected: client, agent and server read one wall clock. Tests on a shifted
clock (`task_message`, `first_message`, `parking::armed_notice`) ordered late hooks and late
transcript lines by their arrival, not their writing. Times a line announces (a usage limit's
reset) are left as written. The submit-origin lookup compares a transcript's queue times with the
run's start on Yulius's clock; no shifted-clock test uses it (its fake agent sends no prompt id),
and it is left as it is.

## Requirements

- LSK-001: A hook's send time is read on Yulius's clock (`Clock::of_wall`); the system clock reads
  it as it is, so production is unchanged.
- LSK-002: The time a transcript line was recorded (each observation's `at`) is read on Yulius's
  clock where the watcher takes its observations in, for replayed and live lines alike.
- LSK-003: The test `ShiftedClock` maps a wall-clock stamp with the shift in force when it was
  made, so a stamp made before a move of the clock keeps its order.
- LSK-004: The held-message flag survives a cleared box during the keyboard minute and ends once
  the box is seen empty after it (SPEC-572 AC2), also when a prompt's submit or its record reaches
  Yulius after the turn's Stop.

## Plan

- [x] Spec with the root cause of the late Stop
- [x] Deterministic regression: an opening submit held past the held-message flag
- [x] Hook send times read on Yulius's clock
- [x] Docs, ADR and changes entry
- [x] Independent review in a fresh session
- [x] Transcript record times read on Yulius's clock, with a held-record regression
- [x] Second review round in a fresh session

## Acceptance criteria

- `task_message::a_submit_that_arrives_after_its_turn_keeps_the_held_message_flag` holds a
  prompt's submit at its arrival (`race::Point::HookArrived`) until the flag was raised and
  Ctrl+U sent; it fails (`Null`) without the hook fix and passes with it.
- `task_message::a_prompt_record_read_after_its_turn_keeps_the_held_message_flag` holds the
  watcher's read of a prompt record (`race::Point::UserTurnRead`) the same way; it fails
  (`Null`) without the record-time fix and passes with it.
- The whole `yulius::main` binary and the `yulius-app` tests pass; `ShiftedClock::of_wall` and
  `Observation::with_record_times` have unit tests.
- Yulius CI is green at the reviewed head.

## Test plan

- Integration (`crates/yulius/tests/task_message.rs`): the AC2 steps after a prompt the person
  sends, whose submit (or, in the second test, whose transcript record) is held until the box is
  cleared; each waits for its turn's Stop explicitly; the existing test unchanged. The opening
  submit cannot be held deterministically: its session id is known only once the run started,
  and the fake agent's `--stuck-at-start-file` never resumes.
- Unit: `ShiftedClock::of_wall` across a shift; `Observation::with_record_times` maps recorded
  times only.
- Mutation by hand: undo each fix in turn, watch its test fail.

## Decisions

- Fix the clock, not the test's waits: a wait in `idle_full_screen` for the opening submit's
  answer would still see the late prompt set Working (its `sent` is still its handling time), and
  no wait can order a watcher read that happens later. Reading wall-clock stamps through the
  clock port keeps the product rules and makes test clocks behave like the shared host clock.
- The port method has an identity default, so `SystemClock` and the fixed test clocks keep their
  behaviour (tests that send their own `sent` header on a fixed clock stay valid).
- Record times are mapped once, right after the watcher's poll, so the live path, the replay's
  facts and every observation agree. Announced times (`resets_at`) stay as written: they are not
  orderings with Yulius's events.
- Two test-support pause points, which do nothing without the `test-support` feature: one at the
  entry of `hook_event`, keyed by the hook's session id and event, before anything is read; one
  where the watcher takes a prompt record, keyed by the task id, before its last-Stop check.
- The spec keeps the task's original slug and first Plan title, written before the cause was
  reproduced; the task's title names the late Stop for the same reason.

## Evidence

Against dev `b1b50218f` (round 1) and `c4eca6b9f` (merged in), on this host (128 CPUs).

- Stress without a probe: `taskset -c 0-3` with eight busy loops on the same CPUs, the original
  test `--stress-count 30`: 30 passed. Load alone did not reproduce it.
- Task #616's reading: a hold of the opening Stop at `race::Point::StopTurnLock` left the task
  Working ("timed out waiting for the first turn's end").
- Probe (temporary, not committed): the opening `UserPromptSubmit` held at the entry of
  `hook_event` until Ctrl+U, with an activity trace polled every millisecond:
  `NeedsAttention` (first watch) → `Working` at the release → `left: Null` at the flag check
  after the second watch, the #1615 failure. The same hold after `sent` is read: passed.
- Review round 1 (head `1b7b723ae`): with the hook fix in place the original test still failed
  with `left: Null` when its thread paused 900 or 1200 ms after the first watch; the watcher's
  trace read `UserTurn at=…938969 last_stop_ms=…158996 => NEW TURN: sets Working`.
- `a_submit_that_arrives_after_its_turn_keeps_the_held_message_flag` without the hook fix
  (`hooks.rs` at `5dc595ded`): FAIL, `left: Null` at the flag check; with it: PASS (6 s, the
  client's 5 s fail-open).
- `a_prompt_record_read_after_its_turn_keeps_the_held_message_flag` at `99e46fde2` (no record
  mapping): FAIL, `left: Null` at the flag check (`task_message.rs:1581`); with the mapping:
  PASS (3.9 s).
- `yulius-app` `ports::harness::record_time_tests::only_the_times_lines_were_recorded_are_mapped`:
  passed.
- With both fixes: `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main)'`: 2758
  passed, 24 skipped; `scripts/cargo nextest run -p yulius-app --features test-support`: 725
  passed; `scripts/cargo clippy -p yulius-app -p yulius --all-targets --features
  yulius-app/test-support -- -D warnings`: clean.
- Not tested here: the full gate (Yulius CI's), and a production instance under load (production
  reads one clock in client, agent and server, so the defect cannot occur there).
