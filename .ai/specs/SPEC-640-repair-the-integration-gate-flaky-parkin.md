---
yulius-task: 45580b8c-2545-4998-ac4b-733917801244
status: in-progress
---

# SPEC-640: Repair the integration gate: flaky parking::candidate::a_live_owner_with_a_pending_candidate_is_shown_waiting_not_done

## Goal

Make `yulius::main parking::candidate::a_live_owner_with_a_pending_candidate_is_shown_waiting_not_done`
deterministic. The full integration gate on dev `fd8d889e1` (Yulius CI run
`d6505f62-7a12-4856-8ac5-0233ae1fbabb`, job `test (2/2)`) failed at its last assertion:
`assert_eq!(task_updates(&mut events), vec![true])`. The owner's events were `[true, true]`.
The parent `edcf33a21` has the same Rust code and passed the full gate, so the test is flaky.

## Context

- The test came with #1723 (`79e3b5bbe`, SPEC-621 FIN-004). It runs in `with_idle_candidate`:
  the owner is seeded in one runtime, then `restore_owner` builds a new app, which reattaches
  the owner's live PTY session (`reconcile`).
- Reattaching starts the session watcher (`App::start_watcher`). It resets the background
  counts to "unknown" (`background_shells = -1`, `background_waits = -1`). Its first round
  begins `WATCH_INTERVAL_MS` (1 s) later and does two things:
  - It probes the idle task's shells (`probe_idle_shells`). The count goes from -1 to 0, and
    the summary's `background_shells` from absent to `0`, so it calls `publish_task`.
  - It replays the reattached transcript to its end and settles the idle activity
    (`settle_idle`). A task whose goal is not recorded as achieved goes from `done` to
    `waiting`, and this publishes too.
- The test body runs within about that first second. Probes in the product (see Evidence)
  showed the watcher's first round starting just as the second counted watch pass ended. When
  the host is a little slower, the round falls inside the counted window. The window then
  holds the watch's own publish (`pr_watch.rs` `observe_candidate_pull`, once) plus the
  probe's publish: `[true, true]`.
- The first counted pass also types a notice into the owner ("Its head … passed its own Yulius
  CI run, but no merge is armed"). The fake agent answers it with a turn within milliseconds.
  The watcher reads that turn in its next round and shows the owner `working`, which is a
  second, independent publisher racing the later windows. Holding the pass for a watcher
  round made that race certain: `[false, false]`.

## Root cause

The test counts every `TaskUpdated` of the owner during one `watch_prs` pass. Its fixture does
not wait for the reattached session watcher's first round. That round publishes on its own
schedule, so an unrelated, correct publish can land in the counted window. The same holds
for the agent's answer to the notice that the first counted pass types: the watcher hears it
whenever its next round comes.

The product is right. The pass publishes once, when what the task shows changes (SPEC-621
FIN-004). The probe's publish reports a real change: the background-shell count was unknown
before the first probe. The replay's publish reports a real activity change too.

## Requirements

- FLK-001: `restore_owner` returns only after the reattached owner's transcript watcher has
  finished its first round (two rounds began: `transcript_rounds() >= 2`, as
  `pool_supervision` waits for "the transcript replay"), bounded by a timeout, with no sleep.
- FLK-002: The test checks that the watcher's rounds add nothing to the counted pass. After
  the first pass's notice, it waits until the watcher has read the agent's answer (the owner
  shows `working`, then two more rounds begin) and puts the owner back at rest (`done`). It holds
  the second pass at `CandidateObservationRead` until a full watcher round has run. It then
  expects the watch's one publish (`true`) first and no other `true`. That pass ends by typing
  the failure notice, so the agent's answer to it (`false`) may already be heard after it.
- FLK-003: A watcher shell-probe round that falls while the owner answers (`working`) forgets
  the count (`-1`), and the next probe round would publish it again inside a counted window
  (review round 1). After putting the owner back at rest, the test waits for a round that began
  since and probes the shells itself. Before each counted window it asserts a known count
  (`background_shells == Some(0)`).

## Plan

- [x] Reproduce the double publish and find its root cause
- [x] Regression check: a watcher round during the held pass adds no publish (fails before the fix)
- [x] Fix: the restored owner's watcher finishes its first round before the test body; parking tests in full and under stress
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The test passes repeatedly, including with a busy loop beside it under `--stress-count`.
- Without either half of the fix, the regression check fails every run.
- Every `parking::` test of the `yulius::main` binary passes.

## Test plan

- Hold the second pass at `CandidateObservationRead` while two further watcher rounds begin,
  then release it, and expect `[true]`. Run this without the fix (it must fail) and with it.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^parking::/)'`.
- `--stress-count` on the test with CPU-bound busy loops beside it.

## Decisions

- Fix the test fixture, not the product. The pass already publishes once. Suppressing the
  first probe's publish would hide a real change (the count goes from unknown to known), and
  muting the replay's publish would hide a real activity change. Both are what the views
  must hear.
- Wait in `restore_owner` rather than in this one test. Every `with_idle_candidate` test sets
  the owner's activity and then reads it, and the watcher's replay could reset it in the same
  first second. Waiting once in the fixture removes that race for all of them.
- No ADR: this is a test-only change with no lasting product decision.

## Evidence

Run in this worktree on 2026-11-24 with `scripts/cargo nextest run -p yulius -E
'binary_id(yulius::main) & test(=parking::candidate::a_live_owner_with_a_pending_candidate_is_shown_waiting_not_done)'`
(logs in the ignored `target/probe/`).

- Probes (temporary `eprintln!` with backtraces in `publish_task`, `probe_idle_shells` and
  `start_watcher`, since removed) on the unchanged test. The pass published once
  (`pr_watch.rs:2008`, `observe_candidate_pull`). The extra `pending=true` publish came from
  `finish.rs` `probe_idle_shells` ← `supervisor.rs` `watch_loop`, `idle=true previous=-1 count=0`.
  That was the first round of the watcher that `start_watcher` restarted at reattach, about
  1.0 s after it (`supervisor.rs` reset at reattach, then the first round). The transcript replay
  (`settle_idle`, `done` → `waiting`) followed in the same round.
- Regression check alone (commit `9feda500a`, the pass held for two watcher rounds): `--stress-count 5`:
  5/5 FAIL, `left: [true, false, false]`.
- With only the fixture wait: `--stress-count 10`: 10/10 FAIL, `left: [false, false]` (the
  agent's answer to the first pass's notice, read during the held pass).
- With only the answer wait (fixture wait removed): `--stress-count 5`: 5/5 FAIL, "timed out
  waiting for the owner's answer to the notice" (the answer is folded into the replay, which
  never shows `working`).
- Fixed (commit `3444b475a`): `--stress-count 10`: 10/10 PASS. `--stress-count 20` with 128
  CPU-bound busy loops beside it (load average 38 → 154): 20/20 PASS.
- `parking::` tests of `yulius::main` in full: 165 passed, 0 failed.
- Review round 1 (Fable 5.2, head `73bc4df1d`) requested changes. Major: a probe round during
  the answer forgot the shell count, and the reviewer reproduced `[true, true]` with test-side
  delays. Regression check (`8996c0e5b`: the test runs that probe itself, then asserts a known
  count before the counted windows): 3/3 FAIL, `left: None, right: Some(0)`. Fix (`ba7b46c8f`,
  which also answers the minor and two nits): `--stress-count 10`: 10/10 PASS. The reviewer's
  delay probes (start 1200 ms + final 3500 ms, and 2200 ms + 2500 ms, temporary, since removed):
  3/3 PASS each. `parking::` in full: 165 passed. `--stress-count 20` beside 128 busy loops
  (load average 11 → 130): 20/20 PASS.
- Not reproduced: a shell probe that times out (`SHELL_PROBE_TIMEOUT`, 3 s) inside a counted
  window would publish a real change. This is a bounded-timeout assumption, like the tests'
  30 s waits.
