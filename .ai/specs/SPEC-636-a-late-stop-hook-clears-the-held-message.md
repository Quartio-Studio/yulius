---
yulius-task: ddb1cc17-40cd-47c9-8308-27b33f3f0d26
status: done
---

# SPEC-636 A late submit hook on a shifted test clock reopens a settled turn

## Goal

`task_message::a_draft_the_screen_shows_holds_until_it_is_cleared` passes whatever order the
opening prompt's submit hook and the turn's Stop reach Yulius in: a hook's send time, stamped by
its client on the wall clock, is read on Yulius's own clock, also when a test runs that clock
shifted. Fixes #1615.

## Context

The test failed once under host load (#1615, production instance at `52b59ecb3`): after Ctrl+U,
the watch half a minute later found the held-message flag gone (`Null`) although the keyboard
minute still held the message.

### Root cause (reproduced)

1. The test runs Yulius on a `ShiftedClock` 13 minutes behind the wall clock, and moves it
   forward later (`-2 min`, `-2 min + 30 s`, …).
2. The `yulius hook` client stamps each request with its wall-clock send time
   (`x-yulius-hook-sent-ms`). `App::hook_event` takes `sent = sent_ms.min(now)`: never later
   than now, since client and server share the host clock. On the shifted clock every stamp is in
   the future, so `sent` becomes the time Yulius *handles* the hook.
3. Under load the opening prompt's `UserPromptSubmit` reached `hook_event` more than
   `HOOK_TIMEOUT` (5 s) after it was sent. The client had failed open, so the fake agent echoed
   and sent its Stop, which settled the task to Your turn; the test went on, typed a draft, the
   watch raised the held-message flag, and the person pressed Ctrl+U.
4. The late submit then arrived. Its `sent` (the handling time) was later than the Stop's
   `turn_end_ms`, so the check "a prompt answered after its turn ended changes nothing"
   (`hook_prompt`, SPEC-346 LPH-002) did not hold: the prompt set Working, which replaced the
   flag. Within the keyboard minute nothing raises it again (`BoxHeld::AtKeyboard`), so the next
   check read `Null`.

Reproduced with a temporary probe holding the opening submit at the entry of `hook_event` until
the box was cleared: the trace showed `NeedsAttention` → `Working` at the release and the test
failed at the same assertion (`left: Null`). The same hold placed after `sent` is read passed:
with a send time from before the Stop the late prompt changes nothing.

Task #616's reading (a late *Stop* after the transcript record settled the turn) does not hold
for this test: its fake agent writes no transcript the watcher reads, and a held Stop leaves the
task Working, so `idle_full_screen` cannot return before the Stop.

### What else the cause can break

Production is not affected: client and server read the same wall clock, so `sent` is the send
time. Every test on a shifted clock whose hook reaches Yulius late is: such a hook is ordered by
its arrival, not its sending. Transcript record times are also wall-clock stamps compared with
the shifted clock (`turn_began_ms`, `last_stop_ms`); no test hit that, and it is left as it is
(said on #1615).

## Requirements

- LSK-001: A hook's send time is read on Yulius's clock (`Clock::of_wall`); the system clock reads
  it as it is, so production is unchanged.
- LSK-002: The test `ShiftedClock` maps a wall-clock stamp with the shift in force when it was
  made, so a hook that straddles a move of the clock keeps its order.
- LSK-003: The held-message flag survives a cleared box during the keyboard minute and ends once
  the box is seen empty after it (SPEC-572 AC2), also when the opening submit reaches Yulius after
  the turn's Stop.

## Plan

- [x] Spec with the root cause of the late Stop
- [x] Deterministic regression: an opening submit held past the held-message flag
- [x] Hook send times read on Yulius's clock
- [x] Docs, ADR and changes entry
- [x] Independent review in a fresh session

## Acceptance criteria

- The new integration test holds a prompt's submit at its arrival (`race::Point::HookArrived`)
  until the flag was raised and Ctrl+U sent; it fails (`Null`) without the fix and passes with it.
- `task_message` passes in full; the `ShiftedClock` mapping has a unit test.
- Yulius CI is green at the reviewed head.

## Test plan

- Integration (`crates/yulius/tests/task_message.rs`): the AC2 steps after a prompt the person
  sends whose submit is held at arrival until the box is cleared, its client failing open
  meanwhile; the existing test unchanged. The opening submit cannot be held deterministically:
  its session id is known only once the run started, and the fake agent's `--stuck-at-start-file`
  never resumes.
- Unit: `ShiftedClock::of_wall` across a shift.
- Mutation by hand: read the stamp as it is again, watch the new test fail.

## Decisions

- Fix the clock, not the test's waits: a wait in `idle_full_screen` for the opening submit's
  answer would still see the late prompt set Working (its `sent` is still its handling time) and
  leave the task stuck. Reading the stamp through the clock port keeps the product rule ("never
  later than now") and makes test clocks behave like the shared host clock.
- The port method has an identity default, so `SystemClock` and the fixed test clocks keep their
  behaviour (tests that send their own `sent` header on a fixed clock stay valid).
- A race point at the entry of `hook_event`, keyed by the hook's session id and event, before
  anything is read: the only place where a delay reproduces the load-induced order. It does
  nothing without the `test-support` feature.
- The spec keeps the task's original slug and first Plan title, written before the cause was
  reproduced; the title of the task names the late Stop for the same reason.

## Evidence

Against dev `b1b50218f`, on this host (128 CPUs).

- Stress without a probe: `taskset -c 0-3` with eight busy loops on the same CPUs, the original
  test `--stress-count 30`: 30 passed. Load alone did not reproduce it.
- Task #616's reading (late Stop after the record settled the turn): a hold of the opening Stop at
  `race::Point::StopTurnLock` left the task Working ("timed out waiting for the first turn's
  end"); a probe on `on_observation` showed no transcript observation at all in this test.
- Probe (temporary, not committed): the opening `UserPromptSubmit` held at the entry of
  `hook_event` until Ctrl+U, with an activity trace polled every millisecond:
  `NeedsAttention` (first watch) → `Working` at the release → `left: Null` at the flag check
  after the second watch, the #1615 failure. The same hold after `sent` is read: passed.
- Regression `task_message::a_submit_that_arrives_after_its_turn_keeps_the_held_message_flag`
  without the fix (`hooks.rs` at `5dc595ded`): FAIL, `left: Null` at the flag check
  (`task_message.rs:1540`); with the fix: PASS (6.3 s, the client's 5 s fail-open).
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^task_message::/) |
  test(/^first_message::/) | test(/^parking::armed_notice::/))'`: 104 passed (includes
  `first_message::a_shifted_clock_reads_a_stamp_with_the_offset_of_its_time`).
- `agent_hooks::`, `intake::`, `subscription_pool::` (hook ordering on fixed clocks): 189 passed.
- `scripts/cargo clippy -p yulius-app -p yulius --all-targets --features
  yulius-app/test-support -- -D warnings`: clean; `scripts/dev quick`: passed.
- Not tested here: the full gate (Yulius CI's), and a production instance under load (production
  reads one clock in client and server, so the defect cannot occur there).
