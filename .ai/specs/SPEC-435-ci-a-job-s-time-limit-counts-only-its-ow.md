---
yulius-task: 63e456b0-2c64-46f6-ad01-4b16759aab37
status: in-progress
---

# SPEC-435: A CI job's time limit counts only its own run, and a job can set its own limit

Status: in-progress
Date: 2026-11-06
Owner: Yulius task #417
Scope authorization: task #417, asked by the project's owner via quartio-v2's main session
("a job's time limit counts only its own run, and the limit can be set per job").

## Goal

A CI job that waits for another run of the same job never loses its own time to that wait: its
time limit starts when it starts running, a project can give a long job a longer limit, and the
run's status says how long each job waited and ran.

## Context

- Symptom (quartio-v2, 2026-11-06): PR #688's `websites` job timed out at 1 h without a failing
  test. It waited 39 min behind another PR's `websites` job, then had 21 min left for a ~42-minute
  run. Whenever two runs overlap, the second one's long job times out: a full rerun, a blocked merge.
- Root cause: the wait is not Yulius's. quartio-v2's `scripts/gate` takes an exclusive `flock` on
  `$HOME/.cache/quartio-gate/ci-<step>.lock` inside the job's own process, because the two CI lanes
  of a sandbox share the CI `$HOME`. Yulius starts a job's clock when its process starts
  (`ci_watch`, `crates/yulius-app/src/ci/exec.rs`), so everything the process does, waiting on its
  own lock included, counts. Yulius cannot see a lock taken inside the job.
- What Yulius already does right: the wait for a job slot and its CPU and memory budget happens
  before the process starts, is not counted, and is a `wait` stage of the run (SPEC-385).
- The limit is one instance setting, `YULIUS_CI_JOB_TIMEOUT_MIN` (`CiSettings::job_timeout`,
  default 30 min, 1 h on the instance quartio-v2 runs on); no job can ask for more.
- A job's record keeps `started_at` and `ended_at` only; how long it waited is in its `wait` span,
  which `ci_status` does not read.

## Requirements

- JTL-001: a `[[ci.jobs]]` entry may say `exclusive = true`: one run of that job (each matrix value
  apart) runs at a time in the sandbox. A later run's job waits, before it takes a job slot or its
  budget, until the earlier one ended, in the order the runs' jobs ask for the turn (after each
  run's checkout is prepared, so roughly the queue's); the wait is part of its `wait` stage, can be
  cancelled like any queued job, and holds no slot.
- JTL-002: a job's time limit starts when its process starts, after every wait (JTL-001's and the
  slot's). A job that waited longer than its limit and then runs within it passes.
- JTL-003: a `[[ci.jobs]]` entry may say `timeout_minutes = N` (1-360): that job's limit. Without
  it a job keeps the instance's limit (`YULIUS_CI_JOB_TIMEOUT_MIN`). A job whose own run exceeds its
  limit is still killed and errors.
- JTL-004: Yulius records how long each job waited before it started (`waited_ms`). The run's API
  form gives it per job; for a job that waited at least a second, `ci_status` says "websites
  passed, waited 39m 0s, ran 44m 2s" and the run page ends its output with "[waited 39 min, ran
  44 min]" (otherwise just its run time).
- JTL-005: a job that times out says it in plain words with how long it ran and waited:
  "timed out: ran 1 h 0 min, its time limit; it waited 39 min 12 s before it started".
- JTL-006: the jobs in force (`ci_status`, the CI page's API) show a job's own limit and
  `exclusive`, so a project can check its settings took effect. The contract template
  (`docs/api.md`, the `yulius.toml` format) documents both keys.

## Plan

- [x] Spec, ADR and the failing tests for both keys and the wait
- [x] `timeout_minutes` and `exclusive` parse, render and reach each job's record
- [x] A job's limit is its own and starts after every wait
- [x] Exclusive jobs wait in queue order before their slot, and each job records its wait
- [x] ci_status and the run page say how long a job waited and ran; a timeout says both
- [ ] Docs, changes, gate green and an independent review in a fresh session

## Acceptance criteria

- With a 5 s instance limit, three runs of an `exclusive` job that each run ~3.5 s never overlap,
  all pass, and the last one waited longer than 5 s (`waited_ms` > 5000).
- A job waiting for its turn holds no slot: with two slots, both runs' other jobs run while one
  run's exclusive job waits; a job cancelled while it waits ends cancelled without starting, and a
  later run still gets the turn.
- With a 2 s instance limit, a job with `timeout_minutes = 1` running 3 s passes, and a job without
  it running 3 s errors "timed out: ran 2 s, its time limit".
- `timeout_minutes = 0`, `361`, `"2"` and `exclusive = "yes"` are rejected with the key named;
  rendering a configuration keeps both keys.
- A job that waited reads "<name> passed, waited 39m 0s, ran 44m 2s" in `ci_status`.

## Test plan

- Domain (`yulius-domain` `spec_format::config` tests): parsing, bounds, rendering and expansion of
  `timeout_minutes` and `exclusive` onto each matrix job.
- Store (`yulius-store` `ci` tests): `timeout_minutes`, `exclusive` and `waited_ms` round-trip;
  a requeued run clears `waited_ms`.
- Integration (`crates/yulius/tests/local_ci.rs`):
  `an_exclusive_job_waits_its_turn_outside_its_time_limit` (JTL-001/002/004),
  `a_jobs_own_time_limit_replaces_the_instance_one` (JTL-003/005),
  `a_job_waiting_for_its_turn_holds_no_slot` and
  `a_job_cancelled_while_waiting_for_its_turn_never_starts` (JTL-001). All fail before the change
  (`exclusive` and `timeout_minutes` are unknown keys: no CI in force); the slot test also fails
  when the slot is taken before the turn (mutation check).
- `ci::exec` unit test `a_timeout_says_how_long_the_job_ran_and_waited` (JTL-005) and `ci` unit
  test `an_idle_turn_is_forgotten_and_a_held_one_kept`.
- e2e `ci.e2e.ts` "an ended job says how long it waited and ran" (JTL-004, the run page).
- `mcp_text` unit test: the waited/ran wording (JTL-004).
- Gate: `scripts/dev ci`.

## Decisions

- [ADR-0279](../../docs/decisions/0279-a-ci-job-s-limit-is-its-own-and-yulius-holds-its-one-at-a-time-lock.md):
  Yulius holds a job's one-run-at-a-time lock (`exclusive`) and each job may set its own limit
  (`timeout_minutes`), rather than reading a "waiting" marker from the job's output or never
  limiting a job.
- `waited_ms` is a column of the job (migration 0048) rather than read from the `wait` span: the
  run's API form and `ci_status` read job records, and spans are pruned on their own schedule.
- The lock is process-local: a restart requeues every running run (their jobs start over), so a
  lost lock holds nothing.
- `timeout_minutes` is bounded like the instance setting (1-360): every wait needs a bound.
- `ci_status`'s list of recent runs prints a run's jobs line for a wait only from a minute
  (`WAIT_SHOWN_MS`): every job waits a few seconds for its slot, and a line per run for that would
  bury the runs that matter; a run's own view (`run_id`) shows every wait of a second or more.
- The limit comes from where the jobs come from: the integration branch's `yulius.toml` (a pull
  request cannot raise its own), or a bootstrap pull request's own file while its branch has no CI,
  which the jobs' commands are trusted from too.
- `waited_ms` is measured on the monotonic clock (`tokio::time::Instant`), not as the difference
  of two wall-clock stamps: a duration must not move with a clock step (and the test harness's
  clock is fixed).
- The run page's stage waterfall keeps calling the job's wait "for a job slot" even when it was
  also its turn: one wait stage per job, as SPEC-385 shapes it; the job's own line says "waited".
