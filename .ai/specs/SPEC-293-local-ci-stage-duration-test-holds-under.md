---
yulius-task: 93a7464c-251c-4d9e-a7b9-eb7899a7e4c8
status: in-progress
---

# SPEC-293: The CI stage-duration test holds under host load

Status: in-progress
Date: 2026-10-24
Owner: task #279
Scope authorization: task #279 (issue #538, opened by task #155; started by the nightly issue
automation, task #277).

## Goal

`local_ci::a_run_s_jobs_record_their_stages_and_test_counts` passes regardless of host load while
still proving that the stages a CI job's output names are spans with real, ordered, contiguous,
non-zero durations inside the job, timed by Yulius as it reads the output (SPEC-137 STG-201..203).

## Context

- Symptom (#538): `real durations: 1040 ms, 480 ms` (task #155's gate, load ~38, 2026-10-15) and
  `real durations: 1520 ms, 480 ms` (PR #760, run 4c9d8e77). 3adeb5bd (#826) loosened the bound to
  `compiled >= 100 && tested >= 100 && compiled + tested >= 1500`, which both reports pass, but the
  bound still depends on load.
- Product path: `Executor` reads the job log every `READ_EVERY = 500ms` (`MissedTickBehavior::Skip`,
  `crates/yulius-app/src/ci/exec.rs`) and stamps each stage boundary with the injected clock at the
  read (`stages.feed(fresh, self.now())`, `stages.finish(self.now())`). A stage lasts from the read
  that saw its marker to the read that saw the next one. This is the documented contract (SPEC-137:
  "timed by Yulius as it reads the output"), not a defect.
- Test path: the job runs `sleep 1` per stage, and the fake clock `clock_follows_real_time` adds
  20 ms per `tokio::time::interval(20ms)` tick. Two independent things break any time bound:
  1. the clock lags real time when its task is starved (ticks are late; the sum of both stages
     falls below the 2 s the job slept);
  2. the read that sees a marker can be late by any amount: a late read moves the line between the
     stages (1520/480), and one starved read that sees two markers makes a stage zero long.
- Why the existing tests missed it: the test asserts a wall-clock bound on time measured by a clock
  the test does not control, at reads the test does not control; it only fails when the runtime is
  starved, which a quiet developer machine never is.

## Requirements

- TST-001: the job holds each stage until the test releases it, and the test moves the fake clock
  itself, only after it has seen the stage's start recorded; so the stage's recorded duration is
  exactly the time the test moved the clock by, whatever the load and whenever the reads happen.
- TST-002: the test still proves STG-201..203: the stages are children of the job span, contiguous
  (`compile.ended_at == unit.started_at`), non-zero, inside the job, with their figures and test
  counts, and all spans are `recorded`.
- TST-003: no product change: the read interval and read-time stamping stay as they are.

## Plan

- [x] Spec SPEC-293 with root cause and plan
- [ ] Flake reproduced with probe delays
- [ ] Stage test gated by files with a test-moved clock
- [ ] Proof: probes pass and 50 runs under 0.2 CPU share
- [ ] Independent review answered
- [ ] Gate green, docs/changes entry, delivered

## Acceptance criteria

- Temporary probe delays (one in the clock task, one before a log read) make the current test fail
  deterministically; the same probes leave the new test passing.
- The new test asserts exact stage durations set by the test (1000 ms each), contiguity, order and
  containment in the job, with no bound on wall time.
- 50 runs in a row pass under a held CPU share of 0.2.

## Test plan

- Probe A (clock starvation): a blocking delay in the fake clock's task, so its ticks lag.
- Probe B (late read): a one-off delay in `exec.rs` before a log read so a read sees two markers.
- Run the test with each probe, before and after the change; remove the probes (marker grep).
- 50 runs of the test with nextest under a load controller holding a probe's CPU share at 0.2.
- `scripts/dev ci`.

## Decisions

- Gate the stages on files and move the clock from the test, rather than a clock derived from
  `Instant::now()`: a real-time clock fixes the lagging clock but not the late read (a starved read
  that sees two markers still makes a zero-long stage), so any wall-time bound stays flaky. Gating
  makes the reads' timing irrelevant and the durations exact. The pattern already exists in this
  file (`jobs_wait_for_their_cpu_budget_and_are_told_it`, the `go` file).
- Gate files are named after `$YULIUS_CI_RUN`, so the other run of the fixture (the integration
  branch's) is released at once and never holds the pull request's run.
- No ADR: a test-only change with no lasting product decision.
