---
yulius-task: bedc3a84-e034-4989-b94b-b99919cf8515
status: done
---

# SPEC-294: The disk-quota wait test is back in the suite, its fast failure hunted (#485)

Status: done
Date: 2026-10-24
Owner: Yulius task #280 (started by the nightly issue automation, task #277)
Scope authorization: task #280: "Try to reproduce the fast failure with probe delays at the await
points of the candidate above […]. If it reproduces, fix the product race test-first […]. Either
way, take the test out of quarantine […]. Prove it with repeated runs under a held CPU share (50 in
a row at 0.2). If you find no race, say so in the PR and on the issue with what you tried."

## Goal

`local_ci::a_run_over_the_disk_quota_waits_and_starts_when_space_frees` runs in every CI run again.
Its one CI failure (2.16 s, PR #481, 2026-10-15, assertion message lost) is hunted once more at the
places a fast failure could come from; a race found is fixed test-first, and otherwise the test's
next CI failure, now kept by SPEC-145, names the assertion.

## Context

- Symptom (#485): one CI failure after 2.16 s with no timeout; passing runs take 2-7 s in CI. Task
  #128 (SPEC-145) fixed a race in the test that only ever timed out (`recheck_until_started`),
  could not explain the fast failure and quarantined the test with `#[ignore]`. It has not run in
  CI since.
- SPEC-145 R1-R3 keep each failing test's output with the job and show it through `ci_status`, so
  acceptance criterion 1 of #485 is met; criterion 3 (the sibling disk-wait tests) was checked
  there.
- The issue's close condition (core team comment): put the test back in the suite.
- Candidate named by task #277, by reading only: `ci_publish` is not serialized per run, so a late
  publish from the background disk check (`ci_note_disk_wait`) could overwrite a newer status.
- Code path: `ci_dispatch` → `ci_disk_room` (`crates/yulius-app/src/ci/disk_wait.rs`) starts one
  background check per sandbox while `checking` is set; the check notes and publishes the wait on
  each queued run, then `ci_disk_checked` clears `checking`. Runs start only after that, in
  `ci_execute_inner` (`exec.rs`), which publishes its start, its jobs' ends (one at a time) and
  its end in order.

## Requirements

- R1. The fast failure is hunted with probe delays (0-300 ms, temporary, never committed) at every
  await point where an out-of-order status or state could come from: the publish's read and its
  record, the wait note, the end of the check, the dispatcher's disk reading, a run's start, its
  job-end publish and its end. Also with the test clock jumping while the runs execute.
- R2. A race reproduced is fixed in the product test-first (a regression test that holds the late
  step). None reproduced: no product change, and the PR and #485 say what was tried.
- R3. The test runs in the suite again: no `#[ignore]`; its doc comment says why it was out and how
  its next failure is read.
- R4. Proof: 50 runs in a row pass while a controller holds a CPU-bound probe at 0.2 of a CPU.

## Plan

- [x] Spec SPEC-294 with the hunt plan
- [x] Fast failure hunted with probe delays at the publish and clock races
- [x] Any race found fixed test-first (none reproduced: skipped, D1)
- [x] Disk-quota wait test back in the suite, 50 loaded runs green
- [x] Changes entry and gate green, security scan unrun (Vidoc quota)
- [x] Independent review answered and merged on green

## Acceptance criteria

- AC1 (R1, R2). The probe runs and their results are in Evidence; a reproduced race has a
  regression test that fails without its fix.
- AC2 (R3). `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^local_ci::/)'` runs
  the test (not skipped).
- AC3 (R4). 50 of 50 loaded runs pass, with the measured CPU share logged.
- AC4. The gate (`scripts/dev ci`) passes and the PR says `Fixes #485`.

## Test plan

- Probe rounds: the test alone with the probes on, sequential and 8 processes at a time; the test
  with 20 extra clock jumps of 61 s while the runs execute.
- Loaded proof: a controller adds or removes busy loops every few seconds so a 2 s CPU-bound probe
  gets 0.2 of a CPU; the test runs 50 times in a row under it.
- The full gate.

## Decisions

- D1. No product change without a reproduction. The `ci_publish` candidate cannot reach this test:
  the check publishes every note before it clears `checking`, and no run of the sandbox starts
  while `checking` is set, so the note publishes always end before a run's first publish. A
  serialization of `ci_publish` per run would be a refactor without a failing test (the workflow
  forbids it); the candidate is reported on #485 instead.
- D2. The test goes back without retries or a longer timeout: SPEC-145 keeps its next failure's
  assertion message, which is the remaining way to find the cause (the issue's close condition).
- D3. No ADR: nothing lasting is decided beyond SPEC-145's ADR-0119 (CI keeps failing tests'
  output).

## Evidence

- Probe rounds at dev `3768adba` (temporary `probe485` sleeps of 0-300 ms behind an environment
  variable, reverted; `grep -rn PROBE485 crates` is empty): at `ci_try_publish`'s read and record,
  the wait note, the end of the check, `ci_disk_room`'s reading, a run's start, the job-end publish
  and a run's end and final publish. The probes fired (one run: 7.5 s without, 11.5 s with).
  - 40/40 passed, sequential.
  - 64/64 passed, 8 processes at a time.
  - 24/24 passed with 20 extra clock jumps of 61 s while the runs execute.
- No race reproduced: no product change (step "Any race found fixed test-first" skipped, D1).
- Loaded proof (R4, AC3): 50/50 `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^local_ci::a_run_over_the_disk_quota/)'` in a row while a controller held a 2 s CPU-bound
  probe at 0.2 of a CPU (median 0.201 over 119 readings, 12-78 busy loops); the test took 3.3-12.5 s
  (median 7.1 s; 1.6-5.8 s unloaded).
- Gate (AC2, AC4): `scripts/dev ci` exit 0 at `7b85ca79`, 2591 tests run, 2591 passed, 16 skipped;
  `local_ci::a_run_over_the_disk_quota_waits_and_starts_when_space_frees` PASS 1.961 s (no longer
  skipped).
- `scripts/dev security`: exit 2, not run: Vidoc is out of quota for this key (HTTP 429). Unrun,
  not passed. The change is a test attribute and doc comment, a spec and a changes entry.
- Issue #485 comment with these results:
  https://github.com/Quartio-Studio/yulius/issues/485#issuecomment-6385884092
- Independent review (fresh session, `om-code-review`) of `470ca50e`: APPROVE; it confirmed D1
  from the code. Answered: the plan labels now say the race step was skipped and the scan is unrun
  (minor), the spec is done (minor), the test's doc comment credits only SPEC-294 with the probes
  (nit). Pre-existing, outside this change: the status retry loop can send a stale status after a
  run's final one in production (`ci_publish` is not serialized per run); it corrects itself and is
  reported on #485.
