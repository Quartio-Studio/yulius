---
yulius-task: a634a17b-e4fb-4cb9-af93-7abd463d08ce
status: done
---

# SPEC-005: `the_task_follows_its_session_however_it_stops` no longer flakes

Status: implemented
Date: 2026-10-11
Owner: Yulius task #5
Scope authorization: user request ("Fix the flaky test yulius::session_sync
the_task_follows_its_session_however_it_stops ... find the race test-first, and fix the cause
(test or product), not by retrying or sleeping")

## Goal

`crates/yulius/tests/session_sync.rs::the_task_follows_its_session_however_it_stops` passes on
every run, including on a loaded host. It failed once in `scripts/dev ci` (1083 of 1084 passed)
during onboarding and passed on the rerun. The fix removes the race; it does not retry the test
or add a sleep.

## Context

- The test's Ctrl+C-twice step waits for the task to be `closed` in the store (`has_status`),
  then drains a subscription to the app's event bus once with `try_recv` and asserts that the
  last `TaskUpdated` for the task says `closed` (HRN-002, TSK-014: viewers get each change).
- `App::publish_task` (`crates/yulius-app/src/tasks.rs`) runs after the store write: it reads the
  task, builds its summary (async), then publishes. The store is the source of truth and the
  event follows it, by design.
- Reproduced by running the test with about twice as many busy-loop processes as CPUs: 3 failures
  in 14 runs, always at the `seen.last()` assertion with `seen` empty (`left: None, right:
  Some("closed")`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FLK-001 | The test does not assume the `closed` event has been published when the store shows `closed`; it waits, on the condition, for the event. | `session_sync.rs`, the "closed event" wait |
| FLK-002 | No retry and no fixed sleep is added; the wait is bounded (10 s) and fails with a named message. | `wait_for` use |
| FLK-003 | Product behavior is unchanged: the store write still precedes the event, and viewers still receive the final `closed` update. | the test's unchanged final assertion |

## Design

The race is in the test, not the product: "store says closed" and "event published" are two
moments, and only the former is awaited. Making the product publish before the write would
break the source-of-truth ordering, so the test waits for the event instead. The subscription is
created before the Ctrl+C that closes the session, so no event is missed.

## Failure and compatibility

If the event never arrives, the test fails after 10 s with "timed out waiting for the closed
event", rather than a misleading empty-list assertion. No production code or contract changes.

## Acceptance criteria

- The test passes 10 of 10 runs under the CPU load that failed it 3 of 14 times before.
- `scripts/dev ci` passes.

## Test plan

- Load reproduction: busy loops at 2x CPU count, `cargo test -p yulius --test session_sync
  the_task_follows`, repeated. The race depends on timing, so this reproduction is the failing
  case; the corrected test is its regression guard.
- `scripts/dev ci` on a quiet host.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Decisions

- Fix the test, not the product: the event-after-store ordering is intended (see Design).
- No separate failing unit test: the race is timing-dependent and cannot be forced
  deterministically without changing production code; the load reproduction stands in.
- A different, load-sensitive test (`wiring::jev::tests::a_slow_answer_times_out_within_the_deadline`,
  a 2 s wall-clock bound) failed once in a gate run made while the host load average was 20 to 28;
  it passes alone and on rerun. It is out of scope here.
- Review (fresh session) found no blocking problems. Open, not traced: two concurrent
  `publish_task` calls for one task could interleave and deliver a stale event after `closed`.
  The test would still pass (it stops at the first `closed` last-seen), and none of the load runs
  hit it; revisit if the test flakes again.

## Evidence

- Load reproduction before the fix: 3 failures in 14 runs at the `seen.last()` assertion.
- After the fix: 10 of 10 passes under the same load.
- `scripts/dev ci` exit 0 on a quiet host: 1084 passed, 16 skipped; `session_sync` passed.
