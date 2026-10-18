---
yulius-task: b92cf9f2-c6db-47c1-b2b5-c4a2e97a917b
status: in-progress
---

# SPEC-223: The launch-goal restart test waits for a spawned run, and runs again

Status: in-progress
Date: 2026-10-18
Owner: task #209
Scope authorization: task #209: "Fix GitHub issue #514 … The product, or the test's wait, is fixed
so it passes repeatedly under the same conditions … No retries are added". Fixes #514. Restores the
CI coverage of SPEC-146 GWR-002.

## Goal

`first_message::a_launch_goal_unrecorded_across_a_restart_is_watched_again` (SPEC-146 GWR-001,
GWR-002) passes every time, at any host load, and is no longer quarantined, so CI again covers a
launch `/goal` watched again after a restart.

## Context

- **Symptom.** Under host load the test failed after 45 s with "timed out waiting for the flag"
  (CI runs c6fc158a and 9d09927c, task #146's local runs at load ~70). Task #146 quarantined it
  with `#[ignore]` (#514).
- **Root cause (a race in the test's setup, not in the product).** The first server's
  `wait_for("the run")` only needed `store.live_attempt(&id)`, which matches an attempt in
  `spawning` or `live` (`yulius-store` `tasks.rs`). The attempt is `spawning` from
  `store.insert_attempt` until `session_host.spawn` returns (`supervisor.rs`, launch), and the
  launch-goal marker is written in between, so the test's marker assertion passed too.
  `first.shutdown_timeout` then dropped the in-flight launch before the ptyhost existed. The second
  server's `reconcile` found the attempt `spawning` with no host in its run directory
  (`RunInspection::Missing`), so `attach_run` failed and it marked the run lost
  (`Stopped(ServerLost)`). The reattach branch, where the launch-goal watch is armed, was never
  reached, and `flag_needs_attention` needs a running task: no flag ever came, whatever the budget.
  Under load the spawn (writing launch files, starting the ptyhost, waiting until it is ready)
  takes long enough to lose that race; on a quiet host it usually finishes first.
- **Why other tests missed it.** The sibling `a_launch_goal_recorded_before_a_restart_is_not_watched_again`
  waits for "GOAL SET" on the screen, which only a spawned run shows.
- **The product behaviour is right.** A server stopped while it spawns a run leaves no process to
  reattach; the next server never respawns (SPEC-002 §7.1) and ends the run as lost. Nothing in
  the product changes.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| LGR-001 | Before stopping the first server, the test waits until the run's attempt is `Live` (its process host started and ready), not only recorded. | `first_message.rs` `a_launch_goal_unrecorded_across_a_restart_is_watched_again` |
| LGR-002 | The test runs in the default suite (no `#[ignore]`), with no retry and no larger budget. | same |

## Plan

- [x] Reproduce #514 with probe delays and name the race
- [x] Test waits for the spawned run; quarantine removed
- [ ] Spec SPEC-223 with root cause and evidence
- [ ] Loaded proof: repeated passes at a held 0.2 CPU share
- [ ] Gates green and changes entry
- [ ] Independent review of the exact head
- [ ] Merge into dev on green CI

## Acceptance criteria

- The failure is reproduced with probe delays and the race is named (Context, Evidence).
- With the old wait, the test fails under the probes; with the new one it passes repeatedly under
  the same probes and under a held CPU share.
- No retries, no larger timeouts; the `#[ignore]` and its quarantine note are gone.

## Test plan

- Temporary probe (marker `PROBE514`, removed before commit): a fixed 1.5 s sleep in
  `PtySessionHost::spawn` before `spawn_host`; then random sleeps there (0-1.5 s), before
  `PtyClient::connect` in `attach` (0-1.5 s), at the reattach branch of `reconcile` (0-1 s),
  before each replayed output in `attach_run`'s reader loop (0-300 ms), and at the start (0-1 s)
  and each step (0-300 ms) of `spawn_goal_watch`.
- Run the old and the new wait under the probes with `cargo nextest run --stress-count`.
- Without probes, run the test repeatedly while a controller holds a probe thread's CPU share at
  0.2 (the share that reproduced load failures in earlier tasks).
- `scripts/dev ci`.

## Decisions

- **Fix the test's wait, not the product.** The product does what SPEC-002 §7.1 asks of a run cut
  during its spawn (no respawn; lost). Making `reconcile` arm the watch for a run with no process
  would flag a task that is not running. No ADR: no lasting design decision changes.
- **Wait on `AttemptState::Live`** rather than on screen output: the agent here ignores its goal
  prompt and shows nothing specific, and `Live` is exactly "the host started and is ready", which
  survives the server's stop.
