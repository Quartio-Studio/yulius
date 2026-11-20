---
yulius-task: 6f8be32a-a420-4198-b19f-2073647c10b0
status: in-progress
---

# SPEC-559 Fix actionable open issues

## Goal

Resolve the actionable reports in the November 20 GitHub issue inventory on green Yulius CI. Close reports superseded by a fix or a later product decision. Preserve unresolved reports and their acceptance evidence.

Fixes #1501

## Context

The inventory at dev 56f08db7f contained 20 issues. #1490 belongs to task #540. Task #537 owns CI repair publication and parked-session wake. PR #1266 owns the proposed fix for #1260 but currently conflicts with dev. SPEC-521 and SPEC-529 document the fix and direct verification of #1452. #1484 records 55 failures in 60 concurrent runs because the migration test compares elapsed time with ten seconds. SQL migration 28 indexes events by step and ID.

| Issues | Disposition at triage |
| --- | --- |
| #1452, #1429 | Closed. PRs #1439 and #1422 synchronized the exact racing operations. SPEC-521 and SPEC-513 record before-and-after stress evidence. |
| #466 | Closed. CI secret grants shipped. ADR-0157 and ADR-0202 retain the skip rule for an unavailable key, and ADR-0242 and ADR-0245 authorize optional scanning. |
| #1484 | This branch owns the migration test. |
| #1427, #1455, #1441, #1434, #1398, #1401, #1411, #1410, #1260 | Chained tasks #542 through #550 respectively own investigation, verification and delivery. #550 checks the stale conflicting PR before carrying its changes forward. |
| #1490 | Task #540 owns the parking fixes. Task #537 owns the related repair-publication and wake path. |
| #321, #1385 | Require host cgroup measurements or changes and measurements in quartio-v2. The host cgroup tree is absent here. GitHub cannot resolve the quartio-v2 repository with this project’s access. |
| #638, #1133, #529 | Tasks #551, #552 and #553 check the compatible interpreter-name policy, local queue batching and guidance, and opaque tunnel survival respectively. #1133 still requires quartio-v2 dispatch evidence for the historical idle interval. #638 must preserve supported GitHub credentials and report any outstanding product choice. |
| #1239 | Removing the compatibility writer requires evidence that old wrappers can no longer be live on eligible instances. |

## Requirements

- **ISS-001** Give each starting issue a verified disposition, including concrete prerequisites for unresolved work. Keep the work of the running tasks with their owners.
- **ISS-002** Run the step backfill on a version-27 fixture and check its output and SQLite execution work independently of machine load. Removing the lookup index must fail the test.

- **ISS-003** The intake test waits for the first sandbox task list within the shared first-load budget. Later proposal and chaining interactions keep their ordinary assertion timeouts.

- **ISS-004** When a red candidate checks a member whose own run is ending, preserve the member-failure outcome and the existing bounded wait. An active own run can still explain the red candidate.

## Plan

- [x] Account for the open GitHub issues and close obsolete reports
- [x] Prove migration 28 backfills step history within a deterministic work bound
- [x] Obtain independent approval for the store regression
- [x] Verify the green dev merge for the store regression
- [x] Reproduce the intake rail timeout with a held task list
- [x] Verify the intake scenarios against the first-load budget
- [x] Obtain independent approval for the intake regression
- [x] Reproduce the red-candidate split when its member fails
- [x] Preserve the known member failure in merge-train decisions
- [x] Obtain independent approval for the merge-train fix
- [x] Verify the landed cold-screen fixture against the failed CI shard
- [x] Reproduce the parked terminal Reply timeout after Resume
- [x] Verify Resume and Reply in Chromium against the reproduced failure
- [x] Obtain independent approval for the parked terminal change if needed
- [ ] Verify the independent fixes and reconcile the remaining issues

## Acceptance criteria

- **AC-1** Verify merged fixes at dev or identify why an issue still applies. Independent changes each follow their own issue and delivery history.
- **AC-2** Sixty concurrent executions pass. An unindexed backfill fails the deterministic work bound. The fixture produces the expected completed spans.

- **AC-3** A delayed task list can still produce a supertask proposal. A task list that never arrives fails at the existing first-load timeout.

- **AC-4** Hold the second member’s own run at the decision for a failed two-member candidate, then fail it. The first member merges, the second disarms, and the audit identifies the member failure. A genuine combination failure still takes the bisection path.

- **AC-5** After Resume, the parked-terminal test waits for the second park to reach the attention bar. Sending a reply wakes the task with the submitted prompt. The wait for that transition has a deadline.

## Test plan

Exercise the actual migration SQL and measure SQLite VM instructions. Run the focused Rust test with its index removed to prove the assertion. Run the intact fixture in 60 concurrent processes. Check the repository contracts, obtain independent review and verify the Yulius CI result on the merged tree. Check each linked issue, PR and existing owner's evidence separately.

Hold the mock task list past the default assertion budget and run the affected intake scenario before and after the fix. Check the permanently held case and run the intake file with Chromium.

Use the repository race-point fixture to exercise the order from PR #1397, and invoke the test directly outside its existing quarantine. Cover adjacent bisection, active, cancelled and errored cases.

For the parking failure on PR #1499, force the live queue to drain before the queued-to-starting assertion on the failed head. Integrate PR #1496's cold SQLite fixture from dev and run both callers directly.

## Decisions

Use one pull request per independent concern. Retain outstanding reports that require a host measurement, another repository, an upgrade prerequisite or a product decision. Keep the SQL and production migration path unchanged for #1484.

Task #541 owns #1398. Task #546 was archived before it started. Its independent successor #547 now follows #545 directly.

The later reports #1498 and #1502 cover the same parking fixture repaired in PR #1496. The browser timeout in #1501 was discovered during that PR but concerns a different test. Task #541 will investigate it separately after the merge-train delivery.

## Evidence

- At 56f08db7f, 60 concurrent copies of the original store test passed with ordinary CPU placement. Pinning all 60 to one CPU reproduced the failure in all 60. One reported 21.31 seconds against the ten-second cutoff.
- All 60 concurrent copies of the replacement passed on one CPU. Each ran migration 28 against 4,000 events and verified 2,000 completed spans. Removing the SQL index locally made the test fail at 32,253,511 SQLite instructions against a budget of 2,048,000. The migration was restored after the experiment.
- Independent review approved 8ae505256. The reviewer verified the replacement in 60 concurrent processes on one CPU and ran the adjacent version-27 upgrade through the production store entry point. The repository quick checks passed. Vidoc was disabled.
- All 101 store library tests passed. The focused test also passed after integrating PR #1493.
- The project mutation command reported “No mutant in the Rust lines changed against origin/dev.” The index-removal experiment above exercises the SQL regression directly.
- Task #541 took #1441 from the queue after its first delivery. Task #544 was archived after #1495 merged, without starting a session.
- PR #1494 merged and GitHub closed #1484. Yulius CI run 21f32f87 passed quality, both Rust shards and e2e on PR head c1c940306. Run 9f9b51c0 recorded the same passing tree on dev at 9c3b3d92f. Security was skipped with Vidoc disabled.
- #1441 reproduced in Chromium with the task-list response held for 6.5 seconds. The five-second visibility check failed on the seeded Checkout API link before the proposal. With the shared first-load budget, all 18 scenarios in the intake file passed. In a separate probe with the response held indefinitely, the browser trace recorded a 30.0-second visibility check followed by failure on that same link. The probe was removed.
- Independent review approved the intake fix at 015dd8f7c. The reviewer reran the delayed proposal and checked all eleven first-load assertions and the permanently held trace. The repository quick checks passed. The mutation command found no changed Rust code in this slice. Vidoc was disabled.
- PR #1495 merged and GitHub closed #1441. The merge candidate passed quality, both Rust shards and e2e in run c4b1e6af. Yulius recorded success for head ed9bcb044 and the resulting dev commit ba2c36264 (run 38d7b047). Vidoc was disabled.
- The controlled failure of member 2 between the two checks reproduced the historical audit from PR #1397 on unchanged decision logic. The candidate was bisected even though the member’s own run had failed.
- All 48 merge-train integration tests passed with the fix, including the original quarantined test, the forced race, the wait deadline, and bisection for a genuine combined failure. The independent reviewer approved adc28b79b and ran the forced race and deadline control on that head.
- After integrating dev at 3cc2b1969, the original quarantined test, the forced race and the deadline control also passed on the reviewed head (3/3).
- The mutation baseline passed 3,207 tests. The command reported “mutants: 2 listed, 1 caught, 0 missed, 0 timed out, 1 unviable, 0 not reached (bound 30 min, took 655 s)”. Vidoc was disabled.
- PR #1499 run e45eacb9 failed in the parking fixture. Forcing the live queue to drain immediately before the queued-to-starting assertion reproduced the same failure on 561f47b76. PR #1496 had already fixed the fixture by isolating its cold reads in a SQLite copy. After integrating dev at a561121e7, both parking callers and all three focused merge-train tests passed (5/5). The dev tree passed Yulius CI in run 32a60a22, and GitHub closed #1427. Task #541 closed the parking reports #1498 and #1502 with this evidence, ending the quarantine.

- PR #1499 merged on green Yulius CI at 7c7f742af and closed #1398. The unchanged #1501 scenario passed by itself. The original full-suite trace timed out at the second Reply assertion and subsequently captured the parked attention bar with Reply.
- A second park arriving after 6.5 seconds reproduced the five-second visibility failure. A 15-second wait for the parked attention bar synchronized the fixture. All nine Chromium scenarios in the parking file passed, including delivery of the reply text to the mock session. The original trace lacks event timestamps to attribute its delay to one component.
- With the second park held past the deadline in a temporary probe, the test failed at the parked-bar wait before clicking Reply. The fixture was restored after the probe.
- The reviewer found that a suffix comparison also accepted a reply with an extra prefix. With ANSI styling removed, a comparison of the complete prompt line rejected the deliberately altered message. The product mock was restored after this probe.
- Round two approved 91146b0ac. All nine Chromium parking scenarios passed with the whole-line comparison. Quick checks passed. The mutation command found no changed Rust code. Vidoc was disabled.
