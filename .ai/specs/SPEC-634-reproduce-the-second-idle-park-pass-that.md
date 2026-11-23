---
yulius-task: ef2914ca-cd05-4224-a8ff-3b9f4bd95dbd
status: in-progress
---

# SPEC-634 A second idle-park pass at the run's end leaves the parked task open to messages

## Goal

Reproduce deterministically the interval of issue #1605. A second idle-park pass that comes after the parking run's end took the park's intent, while the task still reads Running, must park nothing. Once the task is Parked, the next message must resume it once, in the same conversation.

## Context

`parking::a_message_during_a_park_is_refused_and_resumes_the_task_once_parked` failed once at `fb525a13c` (nextest run `1a1d81f6-e9b1-42cd-84b8-25cfddc9a5a1`). It saw the intended refusal and waited for `Parked`, then got "is parking its idle session" again on the next message.

Cause at `fb525a13c`: `park_now` refused a park only while an end intent was set. `finalize_attempt` takes the intent (`take()`) before it records the attempt as ended and writes `Parked`. The fixture's `parked()` loop calls `park_idle_sessions_now()` while the status still reads Running. A pass in that interval saw a live attempt and no intent, and set `EndIntent::Park` again on the run that was already ending. Nothing took that intent later, so `parking()` stayed true after `Parked`. `tool_task_message` then refused every message.

On dev, #1662 (`8b38f0344`) refuses a park under the intent lock unless the session still holds this attempt's run and `terminal_cleanup` is 0. `finalize_attempt` starts `terminal_cleanup` before it takes the intent and releases it only after it settles the task. No test held that guard for a second idle-park pass. The only test of the message contract uses `Point::ParkDecided`.

The durable Parked state appears only after the in-memory park ends: the intent is taken first, and `Parked` is written afterwards in the same `finalize_attempt`. A task that reads `Parked` is therefore ready to wake, unless a later pass sets a new intent. #1662's guard rules that out.

## Requirements

- **PSP-001** An idle-park pass while a run's end is being recorded (`terminal_cleanup` > 0, its intent already taken) parks nothing and leaves no end intent behind.
- **PSP-002** Once the task reads `Parked`, a message resumes it (`delivered: "resumed"`) in the same provider conversation, and the text is delivered once.
- **PSP-003** A message that comes while the parking run's end is being recorded is not typed into the ending session. It is refused with advice to send it again.

## Plan

- [ ] Reproduce the second idle-park pass at the run's end with the RunEndTaken hook
- [ ] Write the fix spec and the regression test, failing with #1662's guard reverted
- [ ] Refuse a message in the run-end interval and deliver the next one once, test-first
- [ ] Record the contract in the docs and changes, then pass the touched checks
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1** A test pauses the parking run's end at `Point::RunEndTaken`. A second `park_idle_sessions_now()` there returns 0. The same test fails with #1662's guard reverted to the `fb525a13c` condition.
- **AC-2** After the pause is released and the task reads `Parked`, `tool_task_message` returns `delivered: "resumed"`. The resumed run continues the first run's provider session, and the message reaches it once.
- **AC-3** A message sent during the pause is refused and is not typed into the ending session.

## Test plan

Add an integration test to `crates/yulius/tests/parking.rs` with the existing `pause_watch_or_queue` hook at `Point::RunEndTaken`. Revert #1662's guard by hand to show that the test fails, then restore it. Run the whole `parking` file and the `task_message` tests.

## Decisions

- The product fix for the second pass is #1662's guard, already on dev. This change adds the deterministic reproduction that the issue asks for, and no second guard.
- The fixture's `parked()` loop stays as it is. A pass that comes while the end is recorded is the case under test, and the product must refuse it. Removing the loop's repeat would hide the interval instead of holding it.
