---
yulius-task: 8011053b-1b32-4e33-95cf-f22494349369
status: done
---

# SPEC-549 — Separate network admission testing from job timeout

## Goal

Make the local CI integration test reliably observe the refusal to retry a job when Docker has no network room.

## Context

[Issue #1408](https://github.com/Quartio-Studio/yulius/issues/1408) reported an eight-second job timeout in CI run `1627fc16`. The original fixture held both first attempts on a release file. It allowed 60 seconds for both to start and 60 seconds to settle, but the job watchdog could kill the first attempt before it reported pool exhaustion. Under one-CPU affinity, the fixture also depended on two jobs starting together.

## Requirements

- **NET-001** Give each job a deadline beyond its combined start and settle budgets.
- **NET-002** Preserve the pull request job's exact full-pool refusal reason and one recorded attempt per run. Coordinate on the existing start and release conditions.
- **NET-003** Make progress with one available CPU, including when the branch and pull request use the same checkout directory.

## Plan

- [x] Trace the deadline that masks the network refusal
- [x] Prove the fixture allows the job deadline to expire
- [x] Give network refusal its full observation budget
- [x] Exercise both checkouts with one admission slot
- [x] Independently review the single-slot fixture for delivery

## Acceptance criteria

- **AC-1 (NET-001)** The fixture's effective job limit exceeds both observation windows combined.
- **AC-2 (NET-002, NET-003)** Under controlled delay past eight seconds, the pull request job records the network-room refusal and each run records exactly one attempt. The same assertions hold in repeated one-CPU runs.

## Test plan

Check the budget relationship in the existing integration test. Exercise its release gate with a temporary delay to witness the old timeout and the fixed refusal. Run the committed test with nextest stress count and process affinity restricted to one CPU.

## Decisions

Scope the job timeout override to this test. Give each run the network pool in turn. Open the pull request after the branch settles and key attempt files by `YULIUS_CI_RUN`, since both jobs can use the same checkout directory. The existing condition waits bound each run.

## Evidence

- A temporary 12-second hold after both first starts reproduced `timed out: ran 8 s, its time limit` (nextest run `f2e5becc`). With a 180-second job deadline the same hold produced the exact network-room refusal and one attempt per checkout (run `864f90d7`).
- Under `YULIUS_CPUS=1 taskset -c 0`, the two-job barrier hit the start bound (run `52467848`). Staging the runs exposed checkout reuse (run `3ec446f5`). A diagnostic run found two attempt lines in one checkout-keyed file. The final fixture counts by run ID.
- The independent reviews of the timeout override (`ceb0b8742`) and the staged fixture (`a73c2c9ee`) approved with no findings.
- `scripts/dev quick` passed repository contracts and formatting. The mutation runs found no mutant in the changed Rust lines. `scripts/dev security` reported that Vidoc was disabled.
- Independent review approved the run-keyed fixture at `70ccd431c`. The helper comment now describes counts per attempt file for both checkout-keyed and run-keyed callers.
- Final fixture, `YULIUS_CPUS=1 taskset -c 0 cargo nextest run --stress-count 5` with the target test filter and its prebuilt binary: 5/5 passed in 52 seconds (run `19336fa0`).
- A separate one-CPU run held each job at its existing release-file poll for 12 seconds using a temporary PATH wrapper. It logged two distinct CI run IDs. The test passed in 36 seconds with the exact refusal reason and one attempt per run (run `c3eb2672`).
