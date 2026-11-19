---
yulius-task: 8011053b-1b32-4e33-95cf-f22494349369
status: in-progress
---

# SPEC-549 — Separate network admission testing from job timeout

## Goal

Make the local CI integration test reliably observe the refusal to retry a job when Docker has no network room.

## Context

In [issue #1408](https://github.com/Quartio-Studio/yulius/issues/1408), CI run `1627fc16` reported an eight-second job timeout. In `a_second_run_that_finds_no_network_room_is_not_made`, the first attempts wait for a release file. The test allows 60 seconds for both starts and another 60 seconds to settle. The fixture overrides the retry's network wait to 500 milliseconds but inherits an eight-second job limit. A slow start of the other job can therefore consume the first job's limit before it reports address pool exhaustion.

## Requirements

- **NET-001** Give the jobs in this test a deadline beyond the combined start and settle budgets.
- **NET-002** Preserve the full-pool refusal reason and one recorded attempt per checkout. Coordinate on the existing start and release conditions.

## Plan

- [x] Trace the deadline that masks the network refusal
- [x] Prove the fixture allows the job deadline to expire
- [ ] Give network refusal its full observation budget
- [ ] Independently review the fix and deliver it on green CI

## Acceptance criteria

- **AC-1 (NET-001)** The fixture's effective job limit exceeds both observation windows combined.
- **AC-2 (NET-002)** Under controlled delay past eight seconds, the pull request job records the network-room refusal and both checkouts record exactly one attempt. The same assertions hold in repeated one-CPU runs.

## Test plan

Check the budget relationship in the existing integration test. Exercise its release gate with a temporary delay to witness the old timeout and the fixed refusal. Run the committed test with nextest stress count and process affinity restricted to one CPU.

## Decisions

Scope the override to this test. Both 60-second waits already bound the scenario, so letting the fixture's job outlive them isolates the network refusal without adding a sleep or extending the test's waits.

## Evidence

With the eight-second fixture, a temporary 12-second hold between both first starts and the release file reproduced the original assertion. The actual reason was `timed out: ran 8 s, its time limit` (nextest run `f2e5becc`).
