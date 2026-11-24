---
yulius-task: 2c5f677e-38b8-42b9-8249-2f8cd86acdb1
status: in-progress
---

# SPEC-645: Parked-task MCP tests keep their pause hooks to themselves (#1644)

## Goal

`agent_hooks::parked_mcp_explains_a_hold_dismissed_during_the_read` and
`agent_hooks::parked_mcp_reads_classify_the_plan_they_return_and_honor_a_new_decision` pass
whatever other test of the `yulius::main` binary runs beside them in the same process (libtest
threads), and a pause hook a test registers is reached only by that test's server.

## Context

- Both tests pause the parked read with `support::hold_point(Point::ParkedStatusRead, f.task)`,
  which registers a hook through `yulius_app::race::on`. The point is reached in
  `parked_status_inner` (`crates/yulius-app/src/sandboxes.rs`), keyed by the task id.
- `race::HOOKS` is one list for the whole process. `reached()` takes the first hook whose point
  matches and whose key is a prefix of the reached path, and removes it.
- Every `agent_hooks.rs` `fixture()` names its main task `t-main`; other files reuse ids such as
  `t1` (`needs_you_messages.rs` holds `ParkedStatusRead` on `t1`). About 30 points are keyed by a
  task id and 38 files register hooks. Path-keyed points were safe: each test's paths are its
  own temporary directories.
- The integration tests are one binary (ADR-0028). nextest (the gate) runs each test in its own
  process, so the gate never met it; `cargo test` (libtest, threads in one process) does.

## Root cause

Under libtest two tests share `race::HOOKS`. While test A holds `ParkedStatusRead` on `t-main`,
test B's parked read of *its own* `t-main` reaches the point first, takes A's hook and is held.
A's `point_reached` returns (B sent it), and A's own read runs unpaused:

- hold test: A's read finishes before `resolve_hold`, so it shows the pending hold (or the
  pull-request verification state), not "A prompt was held for a decision when the task was
  checked.";
- plan test: A's read is not held at the point, so it reads after `tool_steps_set` and returns
  one step where 0 is expected.

The product read is not at fault: when the hook holds the right read, the reply is the one the
tests expect (step 4 proves both cases with a neighbour beside them).

## Requirements

- PAU-001: a hook registered inside a tokio runtime runs only when its point is reached on a
  thread of that same runtime (a worker or its blocking pool). Each `#[tokio::test]` has its own
  runtime, so tests of one process never take each other's hooks.
- PAU-002: a hook registered outside any runtime, or a point reached outside any runtime (a
  plain thread), matches by point and key as before, so sync tests and points reached on
  `std::thread`s keep working.
- PAU-004: a hook whose runtime shut down matches by point and key as before, so a test that
  restarts its server on a new runtime (`task_done.rs`) meets the hooks it registered before the
  restart.
- PAU-003: both parked MCP cases keep their expected MCP state while another runtime of the
  process does a parked read of its own `t-main` between the hook's registration and the read.

## Plan

- [ ] Spec and root cause of the stolen pause hook
- [ ] Regression test: a neighbour runtime's parked read of its own t-main
- [ ] Pause hooks scoped to the registering runtime
- [ ] Both parked MCP cases proven beside a neighbour read
- [ ] Full suite under nextest, touched modules under libtest threads
- [ ] ADR, architecture, changes and spec evidence
- [ ] Independent review approved

## Acceptance criteria

- The regression test fails on dev's `race.rs` with the neighbour having taken the hook, and
  passes with the fix.
- With a neighbour runtime reading its own parked `t-main` while the hook is armed, the hold
  case shows the dismissed hold's wait reason and the plan case shows 0 steps (and the owner's
  new decision).
- The `yulius` and `yulius-app` tests pass under nextest (the gate's runner), and the touched
  modules (`agent_hooks::`, `review_rounds::`, `needs_you_messages::`, `pause_points::`) pass
  under libtest with 8 threads in one process.

## Test plan

- `pause_points.rs` (the integration binary, which has `test-support`): a hook registered in runtime A is not run by a point reached in runtime B
  with the same key, and is run by A; a hook registered outside a runtime runs anywhere; a
  hook whose runtime shut down (current-thread and multi-thread) runs on the next one.
- `agent_hooks.rs`: a sync test runs each parked MCP case on its own runtime with a neighbour
  fixture on a second runtime (own thread, as libtest runs another test). The neighbour does its
  parked read after the hook is armed and before the case's read starts; the case fails at once
  if its hook is reached by the neighbour, else goes on and asserts its MCP state.
- Libtest run of `agent_hooks::`, `review_rounds::`, `needs_you_messages::` and `pause_points::`
  with `--test-threads=8`, the whole binary with nextest.

## Decisions

- Scope hooks to the registering runtime (`tokio::runtime::Handle::id`, stable in tokio 1.54)
  rather than renaming fixture task ids: it fixes every task-id-keyed point of every test file
  at once, needs no change at the 30 reach sites, and keeps the ids tests assert on. Unique ids
  per fixture would fix only the files changed and leave the next fixture to repeat it.
  Recorded in ADR-0474.
- No runtime on either side keeps today's matching, so nothing that pauses a plain thread
  breaks.
- A shut-down runtime releases its hooks to key-only matching (found by the full run: the
  strict rule left `task_done::a_worker_asking_during_reattachment_keeps_its_flag`'s hook
  unrun after its restart). Liveness is a token held by a never-ending task on the runtime,
  dropped at its shutdown; no test keeps a hook's runtime alive while reaching it on another,
  except the neighbour tests on purpose.

## Evidence

All on `feat/629-parked-task-mcp-tests-keep-their-pause-h` from dev `90ec692d2`.

- Red, at `8814530f5` (new tests, dev's `race.rs`):
  `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^agent_hooks::(a_neighbours_parked_read|parked_mcp_explains_a_hold|parked_mcp_reads_classify)/)'`
  → both `a_neighbours_parked_read_leaves_the_{dismissed_hold,plan_and_decision}_case_its_pause_hook`
  FAIL with `Error: "another test's parked read of its own t-main took this test's pause hook"`;
  the two original tests pass alone. This is the #1644 ordering, controlled: the neighbour
  runtime's parked read of its own `t-main` reaches the point while the case's hook is armed.
- Red mechanism, `pause_points.rs` with dev's `race.rs` copied back:
  `a_hook_runs_only_for_a_point_reached_on_the_runtime_that_registered_it` FAIL
  (`another runtime's point took the hook`).
- Green, with the fix: the same selection plus `pause_points::` and
  `task_done::a_worker_asking_during_reattachment_keeps_its_flag`: 8 passed. The neighbour
  cases assert the dismissed hold's wait reason ("A prompt was held for a decision when the task
  was checked.") and 0 steps / `shows` / the new `needs_you` action for all six tool and decision
  combinations.
- Found by the full run of the first, strictly runtime-scoped fix (`09fb82f82`):
  `scripts/cargo nextest run -p yulius -p yulius-app` → 4212 passed, 1 failed
  (`task_done::a_worker_asking_during_reattachment_keeps_its_flag`, `Error: Elapsed(())`: its hook
  was registered before the restart and never run on the second runtime). Fixed by PAU-004
  (`fba1f612b`), with `pause_points::a_hook_whose_runtime_shut_down_runs_on_the_next_one`.
- Full run at `fba1f612b`: `scripts/cargo nextest run -p yulius -p yulius-app` → 4214 tests
  run: 4214 passed, 25 skipped.
- libtest, one process: `scripts/cargo test -p yulius --test main -- agent_hooks:: review_rounds::
  needs_you_messages:: pause_points:: --test-threads=8` → 210 passed, 0 failed.
- Audit: every test file that registers hooks and builds several runtimes shuts the first one down
  before the next (a script over `crates/yulius/tests` found no test building a second runtime
  while a hook's first runtime lives); the only hook-using tests with plain threads reach points
  outside a runtime (`projects.rs`) or are the neighbour tests.
- Not done: the whole binary under libtest (`cargo test` of all 2800 tests in one process) is not
  the gate's runner; Yulius CI runs nextest.
