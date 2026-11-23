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

The interval is narrower than "a pass while the status reads Running". A pass first probes the session's processes (`background_or_wait_now`) and refuses an agent that already exited. So the defect needs a pass that probed the live session, then waited for the launch lock while the first park ended the session, and checked the intent only after `finalize_attempt` took it. No race point marked "probe done, waiting for the launch lock". A pass paused at `RunEndTaken` alone is refused by the probe even with the `fb525a13c` guard.

The reproduction found a second defect in the same interval. After the reader took the park's intent, `parking()` is false while the task still reads Running. A message then went to `type_into_session` and failed with `Upstream("session input: ptyhost socket I/O: Broken pipe")`, a server error that tells the person nothing.

## Requirements

- **PSP-001** An idle-park pass while a run's end is being recorded (`terminal_cleanup` > 0, its intent already taken) parks nothing and leaves no end intent behind.
- **PSP-002** Once the task reads `Parked`, a message resumes it (`delivered: "resumed"`) in the same provider conversation, and the text is delivered once.
- **PSP-003** A message that comes while the parking run's end is being recorded is not typed into the ending session. It is refused as parking (409, "is parking its idle session") with advice to send it again. A task that reads `Parked` takes the message at once.

## Plan

- [x] Reproduce the second idle-park pass at the run's end with the RunEndTaken hook
- [x] Write the fix spec and the regression test, failing with #1662's guard reverted
- [x] Refuse a message in the run-end interval and deliver the next one once, test-first
- [x] Record the contract in the docs and changes, then pass the touched checks
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1** A test orders a second `park_idle_sessions_now()` pass: its probe sees the live session (`Point::ParkProbed`), and its claim (`Point::ParkPrepared`) comes after the first park's run end took the intent (`Point::RunEndTaken`). The pass returns 0. The same test fails with #1662's guard reverted to the `fb525a13c` condition.
- **AC-2** After the pause is released and the task reads `Parked`, `tool_task_message` returns `delivered: "resumed"`. The resumed run continues the first run's provider session, and the message reaches it once.
- **AC-3** A message sent during the pause is refused and is not typed into the ending session.

## Test plan

Add an integration test to `crates/yulius/tests/parking.rs` using the race hooks (`pause_watch_or_queue` at `ParkDecided`, `ParkPrepared` and `RunEndTaken`, and a notify at the new test-only `ParkProbed`). Revert #1662's guard by hand to show that the test fails, then restore it. Undo the message refusal to show that the early message fails. Stress the test, then run the whole `parking` and `task_message` files and the tests that park, message or notify.

## Decisions

- The product fix for the second pass is #1662's guard, already on dev. This change adds the deterministic reproduction that the issue asks for, and no second guard.
- A test-only race point `ParkProbed` (after the process probe, before the launch lock) is added to the existing race mechanism. Without it, "the second pass probed and now waits for the lock" could only be reached with a sleep. Production builds compile the point to nothing.
- The message refusal is checked in the typing path (`type_into_session_written`) as `parking() || park_end_recording()`. `park_end_recording` means terminal cleanup is under way and the park is committed. `parking()` itself is unchanged: `tool_task_message` also uses it before the Parked branch, so widening it would refuse messages to a task that already reads `Parked`. That was this issue's symptom. A message whose status read came just before `Parked` was written can still meet the ended session in the instant before terminal cleanup drops. That window is a read-then-act race that every live check has, and it is not the interval reported here.
- No ADR: this keeps ADR-0367's contract ("nothing is typed into a session that is parking") and states when a parked task is ready to wake. The contract is recorded in `docs/architecture.md` (Parked sessions) and `docs/api.md`.
- The fixture's `parked()` loop stays as it is. A pass that comes while the end is recorded is the case under test, and the product must refuse it. Removing the loop's repeat would hide the interval instead of holding it.

## Evidence

- A pass paused only at `RunEndTaken` parked nothing even with the `fb525a13c` guard. Instrumented gates showed the process probe (`background_or_wait_now`) refusing the already-exited agent. That is why the test orders the probe with `ParkProbed`.
- `parking::a_second_park_pass_at_the_runs_end_leaves_the_parked_task_open_to_a_message` with `park_now`'s guard reverted to the `fb525a13c` condition (`intent.is_some() || merge_is_armed`): `assertion left == right failed: a second pass parked the ending run again, left: 1, right: 0`. With the interval asserts skipped under the same revert, the message after `Parked` failed exactly as in #1605: `Conflict("task #1 is parking its idle session: send the message again in a few seconds and it resumes the task")`.
- Without the typing-path refusal (commit `a1a0a3597`), the message during the run's end failed with `Err(Upstream("session input: ptyhost socket I/O: Broken pipe (os error 32)"))`. With it (commit `258c41784`) the test passes.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^parking::a_second_park_pass/)' --stress-count 30`: 30 of 30 passed.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^parking::/) | test(/^task_message::/))'`: 227 passed.
- `scripts/cargo nextest run -p yulius -p yulius-app -E 'test(/park|message|notice|type_into|task_done::/)'`: 565 passed.
