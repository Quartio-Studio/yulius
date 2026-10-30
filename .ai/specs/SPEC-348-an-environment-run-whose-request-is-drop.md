---
yulius-task: 46588f82-ada2-4714-a66b-bf9ac1ef6059
status: done
---

# SPEC-348: An environment run whose request is dropped still gets its executor

Status: implemented
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
- [x] Gates green and independent review answered
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

## Review

Independent review in a fresh session of `540450ed..fba8367a`: APPROVE, no critical, high or
medium finding. Answers:

1. Minor, own: `listed_deploys` is also read by the instance page's view of an update waiting for
   deploys, so that view may end a lost run too; ADR-0235 said only "the wait". ADR-0235 and the
   architecture now name the view. Fixed.
2. Nit, own: a run still alive past its lost-run limit (outside its job timeout) is ended by the
   wait, as any read already ended it. Stated in ADR-0235's consequences.
3. Nit, own: with the fix, the request can occasionally finish before the test reads the store,
   so that run takes the answered branch. The test still fails without the fix on every run; a
   temporary print showed the drop branch taken 5 of 5 runs. No change.

## Evidence

- Before the fix (`9654c7c4`, tests only): `cargo nextest run -p yulius -E 'binary_id(yulius::main)
  & (test(/an_environment_run_whose_request_is_dropped/) | test(/an_environment_run_past_its_lost_run_limit/))'`
  failed both: the dropped run "did not finish" after 22 s (`running`, no executor), and the update
  stayed `building` with `env-lost` listed past its lost-run limit.
- After the fix: both pass (0.5 s and 1.7 s). A temporary print in the drop branch, run 5 times
  and reverted, showed the drop branch taken 5 of 5 runs. The reviewer reran them 15 times: 15/15.
- `scripts/dev ci` at `fba8367a`: exit 0, nextest 2777 passed, 16 skipped; lint, docs, deny,
  macOS targets and repository checks green.
- `yulius security-scan` at `fba8367a`: **not run** (exit 2): Vidoc refused the key (HTTP 401),
  4 code files listed as not scanned. Unrun, not passed; the CI `security` job reports its own.
