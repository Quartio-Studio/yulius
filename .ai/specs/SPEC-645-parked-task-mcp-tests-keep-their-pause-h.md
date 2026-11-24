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
- [ ] Full integration binary and yulius-app tests under libtest threads
- [ ] ADR, architecture, changes and spec evidence
- [ ] Independent review approved

## Acceptance criteria

- The regression test fails on dev's `race.rs` with the neighbour having taken the hook, and
  passes with the fix.
- With a neighbour runtime reading its own parked `t-main` while the hook is armed, the hold
  case shows the dismissed hold's wait reason and the plan case shows 0 steps (and the owner's
  new decision).
- The `yulius::main` binary passes under libtest with several threads, and under nextest;
  `yulius-app`'s own tests pass.

## Test plan

- `race.rs` unit test: a hook registered in runtime A is not run by a point reached in runtime B
  with the same key, and is run by A; a hook registered outside a runtime runs anywhere; a
  hook whose runtime shut down runs on the next one.
- `agent_hooks.rs`: a sync test runs each parked MCP case on its own runtime with a neighbour
  fixture on a second runtime (own thread, as libtest runs another test). The neighbour does its
  parked read after the hook is armed and before the case's read starts; the case fails at once
  if its hook is reached by the neighbour, else goes on and asserts its MCP state.
- Libtest run of `agent_hooks::` and `needs_you_messages::` with `--test-threads=8`, the whole
  binary with nextest.

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
