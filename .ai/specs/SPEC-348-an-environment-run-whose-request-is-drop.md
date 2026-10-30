---
yulius-task: 46588f82-ada2-4714-a66b-bf9ac1ef6059
status: in-progress
---

# SPEC-348: An environment run whose request is dropped still gets its executor

Status: in progress
Date: 2026-10-30
Owner: Yulius task #332
Scope authorization: task #332, fix GitHub issue #952 (the delivery environment run; the CD run
was #948, SPEC-347).

## Goal

A delivery environment run recorded as `running` always reaches its executor, or ends at once as
`error`, whatever the request that started it does. An update (SPEC-342) never waits for an
environment run past its lost-run limit.

## Context

- Symptom (#952): `App::dlv_start_environment_run` (`crates/yulius-app/src/delivery.rs`) inserts
  the run as `running` (`insert_environment_run`), awaits its audit entry, and only then spawns
  `dlv_execute_environment_run`, all inside the request's future. Axum drops that future when its
  client disconnects (`POST .../delivery/environments/{env}/runs`), and so does the remote MCP
  `delivery_deploy` (`agent_delivery.rs`). Dropped in between, the run stays `running` with no
  executor and its deploy permit is let go.
- Root cause: the section from the record to the spawn is not atomic with respect to
  cancellation; every store call is a job on the database thread, so a drop while the insert's
  answer is awaited already leaves the row. It is the gap SPEC-347 closed for CD runs.
- An environment run has no Cancel. Only the lazy lost-run expiry `dlv_expire_lost_run` (job
  timeout + 35 min + 5 min, applied only by the reads `environment_run` and `environment_runs`)
  or start-up recovery (`reconcile_environment_runs`) ends it.
- The update's wait counts it: `listed_deploys` (`updates.rs`) reads `running_environment_runs()`
  straight from the store without the lost-run limit, so an orphan keeps an update `building`
  until `deploy_wait_max_ms` and then cancels it (recorded as known in SPEC-342, review round 2).
- Why tests missed it: every environment-run test awaits its request to the end, and the update
  tests finish their running environment run by hand.
- What exists to reuse: `App::run_detached` (ADR-0053) and the owned `DeployPermit` (SPEC-347,
  ADR-0233).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ERD-001 | The section of an environment-run start from its record (`insert_environment_run`) to the spawn of its execution runs in `App::run_detached`: a dropped request leaves the run executing, or ended as `error` when its audit entry could not be written, never `running` without an executor. | `delivery.rs` `an_environment_run_whose_request_is_dropped_after_its_record_still_runs` |
| ERD-002 | The deploy permit (SPEC-342 DPU-002) moves into that task and is held until the run is recorded and started or refused. | same test (`App::deploys_starting` is 0 after the run ended) |
| ERD-003 | The update's wait passes each running environment run through the lost-run expiry before counting it: a run past its limit is recorded as `error` (as a read would) and is not waited for. | `self_update.rs` `an_environment_run_past_its_lost_run_limit_is_not_waited_for` |
| ERD-004 | What a start answers, refuses and records, and the wait for a run within its limit, are unchanged. | the existing `delivery.rs` and `self_update.rs` tests |

## Plan

- [x] Spec SPEC-348 written and validated
- [x] Regression tests drop an environment run's request and outlive its lost-run limit
- [x] Environment run starts detached from its request
- [x] Update wait ends environment runs past their lost-run limit
- [x] Decision, architecture and changes documented
- [ ] Gates green and independent review answered
- [ ] Pull request merged into dev on green

## Acceptance criteria

- A test polls a person's `POST /api/sandboxes/s1/delivery/environments/dev-vm/runs` by hand and
  drops it as soon as the run is in the store. The run then ends `succeeded` with its command's
  output, and no deploy is left counted as starting. It fails on `dev` (the run stays `running`).
- A test records a running environment run, lets the update reach its wait, and advances the clock
  past the run's lost-run limit but not past the wait's bound: the update goes on to `pausing` and
  the run reads `error`. It fails on `dev` (the update stays `building`).
- Every existing delivery and self-update test passes unchanged.

## Test plan

- Integration (`crates/yulius/tests/delivery.rs`): the dropped-request test, the pattern of
  SPEC-347's `cd.rs` test.
- Integration (`crates/yulius/tests/self_update.rs`): the lost-run-limit test next to
  `a_running_environment_run_is_waited_for`.
- Regression: the whole `delivery::` and `self_update::` integration files, then the full gate.

## Decisions

- Reuse ADR-0233 as it is: `run_detached` from the insert on, the owned permit moved in. A drop
  guard and relying on recovery were rejected there for the same reasons.
- No Cancel for environment runs (the issue's fallback): with the start detached no orphan is
  left by a dropped request, and a Cancel would need a way to stop a command run on a host, a
  feature of its own.
- The wait applies the existing expiry (`dlv_expire_lost_run`) rather than skipping runs past the
  limit: the row is then ended for every reader too (the "already in progress" refusal goes with
  it), and the limit lives in one place. A store failure while expiring is a failed read, which
  the wait already treats as one deploy still running. ADR-0235.
