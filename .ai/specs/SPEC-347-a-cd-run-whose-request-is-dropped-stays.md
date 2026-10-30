---
yulius-task: d5bd81f5-5c11-443e-b45d-ba7547ddc85c
status: done
---

# SPEC-347: A CD run whose request is dropped still gets its executor

Status: implemented
Date: 2026-10-30
Owner: Yulius task #331
Scope authorization: task #331, fix GitHub issue #948 (the CD run; the delivery environment run of
its second criterion is #952, a chained task).

## Goal

A CD run recorded as `running` always reaches its executor, or ends at once as `error`, whatever
the request that started it does. A client that goes away mid-start never leaves a `running` run
with nothing executing it, which would make an update wait its whole bound (SPEC-342) and refuse
the environment as "already running" until a restart.

## Context

- Symptom (#948): `App::cd_start` (`crates/yulius-app/src/ci/cd.rs`) inserts the run as `running`
  (`insert_cd_run`), then awaits its spans (`cd_start_spans`) and its audit entry, and only then
  registers its cancel channel in `ci.executing` and spawns `cd_execute`, all inside the request's
  future. Axum drops that future when its client disconnects (`POST .../cd/{env}/deploy`, and an
  agent's remote MCP `cd_deploy` through `cd_deploy_for_task`).
- Root cause: the section from the record to the spawn is not atomic with respect to
  cancellation. Every store call is a job on the database thread (`SqliteStore::call`), so a drop
  while awaiting the insert's answer already leaves the row written; moving only the spawn earlier
  would keep that gap.
- Only start-up recovery (`reconcile_cd_runs`) or a person's Cancel ends such a run. The update
  wait counts it (`listed_deploys` → `running_cd_runs`).
- Why tests missed it: every CD test awaits its request to the end; none drops it.
- The same pattern already exists for shell and Cockpit starts: `App::run_detached`
  (ADR-0053, SPEC-077). The deploy permit (SPEC-342 DPU-002) borrows the update state
  (`DeployPermit<'a>`), so it cannot move into a spawned task as it is.
- The same gap in `dlv_start_environment_run` (`delivery.rs`) is #952, a separate task; it can
  apply the same two pieces (owned permit, `run_detached` from the insert on).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DCR-001 | The section of a CD start from its record (`insert_cd_run`) to the spawn of its execution runs as a task spawned by the request and awaited by it (`App::run_detached`): a dropped request leaves the run executing, or ended as `error` when its audit entry could not be written, never `running` without an executor. | `cd.rs` `a_deploy_whose_request_is_dropped_after_its_record_still_runs` |
| DCR-002 | The deploy permit (SPEC-342 DPU-002) moves into that task and is held until the run is recorded and started or refused, so an update never misses a deploy whose request was dropped. `DeployPermit` owns its gate (no borrow). | same test (`App::deploys_starting` is 0 after the run ended); `deploy_gate.rs` `a_permit_moved_into_a_task_counts_until_the_task_drops_it` |
| DCR-003 | What a deploy answers, refuses and records is unchanged for a request that is not dropped. | the existing `cd.rs` and `self_update.rs` tests |

## Plan

- [x] Spec SPEC-347 written and validated
- [x] Regression test drops a deploy request after its run is recorded
- [x] CD start runs detached from its request with an owned deploy permit
- [x] Decision, architecture and changes documented
- [x] Gates green and independent review answered
- [ ] Pull request merged into dev on green

## Acceptance criteria

- A test polls a person's `POST /api/sandboxes/s1/cd/dev/deploy` by hand and drops it as soon as
  the run is in the store (the store answers in order on one thread, so the request has not
  passed its insert's answer yet). The run then ends `passed` with its deploy and verify run, and
  no deploy is left counted as starting. It fails on `dev` (the run stays `running`).
- Every existing CD and self-update test passes unchanged.

## Test plan

- Integration (`crates/yulius/tests/cd.rs`): the dropped-request test above, next to the restart
  reconcile test.
- Unit (`deploy_gate.rs`): an owned permit moved into a spawned task counts as starting until it
  is dropped there.
- Regression: the whole `cd::` and `self_update::` integration files, then the full gate.

## Decisions

- Detach from the insert on, not only the spawn (ADR-0233): a drop while awaiting the insert's
  answer already leaves the row, so "spawn right after the insert" keeps a gap. The checks,
  promotion and refusals stay in the request, as in ADR-0053's terminal start.
- An owned `DeployPermit` (an `Arc` of the gate) instead of releasing the permit in the request:
  releasing it before the insert would let an update close the gate, count nothing starting, read
  no run and restart over a deploy being recorded (SPEC-342 DPU-002).
- Rejected: a drop guard that ends the run as `error` (synchronous store write from `Drop`, and it
  throws away a deploy the person asked for); relying on `reconcile_cd_runs` (only at start-up).

## Reusing it for #952

`dlv_start_environment_run` takes the same permit; with it owned, its section from the run's
insert to its spawn can run in `App::run_detached` the same way.

## Review

Independent review in a fresh session of `5e91e3c0..c1310fe6`: APPROVE, no critical, high or
medium finding. Answers:

1. Low, own: the test's comment claimed a run seen in the store means the request had not passed
   its insert's answer; both go through `spawn_blocking`, so the request can be one store call
   further. Reworded: it is dropped at most one store call past its insert, with at least two
   (spans, audit) left before the spawn, so the test still always catches the gap. Fixed.
2. Low, own: `deploy_permit().is_ok()` did not prove the permit was let go. The test now asserts
   `App::deploys_starting() == 0` (new accessor for tests and diagnostics). Fixed.
3. Low, pre-existing: a Cancel between the run's insert and its cancel channel's registration
   records it `cancelled` while the start goes on to run it. The window is the same before and
   after this change; reported on #948 (this task creates no issues).
4. Low, pre-existing: a production promotion made in the request, then a dropped request, leaves
   a promotion with no run. Nothing stays `running`, so it is outside #948; reported on #948.
5. Nit, own: ADR-0233's wording of #952 and of a panic. Fixed.
6. Nit, own: a panic after the insert would leave the run `running` (as before; no such path
   panics). Stated in ADR-0233's consequences.

## Evidence

- Before the fix (`f52fd82a`, test only): `cargo nextest run -p yulius -E 'binary_id(yulius::main)
  & test(/^cd::a_deploy_whose_request/)'` failed after 61 s with "the CD run did not end",
  `"state":"running"`, empty output: the run had no executor.
- After the fix: the same test passes (about 1.2 s). A temporary print in its drop branch, run
  5 times and reverted, showed the drop branch taken 5 of 5 runs (not the answered branch).
- `deploy_gate.rs` unit tests: 3 passed (`cargo test -p yulius-app --lib deploy_gate`); the new
  one did not compile against the borrowing permit (E0597).
- `cd::` and `self_update::` integration files: 52 passed.
- `scripts/dev ci` at `a17d944f`: exit 0, nextest 2775 passed, 16 skipped; lint, docs, macOS
  targets and repository checks green.
- `yulius security-scan` at `a17d944f`: **not run** (exit 2): Vidoc refused the key (HTTP 401),
  4 code files listed as not scanned. Unrun, not passed; the CI `security` job reports its own.
- Independent review: round 1 at `c1310fe6` APPROVE (2 low and 2 nits own, fixed or stated; 2 low
  pre-existing, reported on #948); round 2 at `a17d944f` APPROVE, nothing new.
