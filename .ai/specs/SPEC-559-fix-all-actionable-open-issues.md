---
yulius-task: 6f8be32a-a420-4198-b19f-2073647c10b0
status: in-progress
---

# SPEC-559 Fix actionable open issues

## Goal

Resolve the actionable reports in the November 20 GitHub issue inventory on green Yulius CI. Close reports superseded by a fix or a later product decision. Preserve unresolved reports and their acceptance evidence.

Fixes #1441

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

## Plan

- [x] Account for the open GitHub issues and close obsolete reports
- [x] Prove migration 28 backfills step history within a deterministic work bound
- [x] Obtain independent approval for the store regression
- [x] Verify the green dev merge for the store regression
- [x] Reproduce the intake rail timeout with a held task list
- [x] Verify the intake scenarios against the first-load budget
- [ ] Obtain independent approval for the intake regression
- [ ] Verify the independent fixes and reconcile the remaining issues

## Acceptance criteria

- **AC-1** Verify merged fixes at dev or identify why an issue still applies. Independent changes each follow their own issue and delivery history.
- **AC-2** Sixty concurrent executions pass. An unindexed backfill fails the deterministic work bound. The fixture produces the expected completed spans.

- **AC-3** A delayed task list can still produce a supertask proposal. A task list that never arrives fails at the existing first-load timeout.

## Test plan

Exercise the actual migration SQL and measure SQLite VM instructions. Run the focused Rust test with its index removed to prove the assertion. Run the intact fixture in 60 concurrent processes. Check the repository contracts, obtain independent review and verify the Yulius CI result on the merged tree. Check each linked issue, PR and existing owner's evidence separately.

Hold the mock task list past the default assertion budget and run the affected intake scenario before and after the fix. Check the permanently held case and run the intake file with Chromium.

## Decisions

Use one pull request per independent concern. Retain outstanding reports that require a host measurement, another repository, an upgrade prerequisite or a product decision. Keep the SQL and production migration path unchanged for #1484.

## Evidence

- At 56f08db7f, 60 concurrent copies of the original store test passed with ordinary CPU placement. Pinning all 60 to one CPU reproduced the failure in all 60. One reported 21.31 seconds against the ten-second cutoff.
- All 60 concurrent copies of the replacement passed on one CPU. Each ran migration 28 against 4,000 events and verified 2,000 completed spans. Removing the SQL index locally made the test fail at 32,253,511 SQLite instructions against a budget of 2,048,000. The migration was restored after the experiment.
- Independent review approved 8ae505256. The reviewer verified the replacement in 60 concurrent processes on one CPU and ran the adjacent version-27 upgrade through the production store entry point. The repository quick checks passed. Vidoc was disabled.
- All 101 store library tests passed. The focused test also passed after integrating PR #1493.
- The project mutation command reported “No mutant in the Rust lines changed against origin/dev.” The index-removal experiment above exercises the SQL regression directly.
- Task #541 took #1441 from the queue after its first delivery. Task #544 will verify the issue after delivery.
- PR #1494 merged and GitHub closed #1484. Yulius CI run 21f32f87 passed quality, both Rust shards and e2e on PR head c1c940306. Run 9f9b51c0 recorded the same passing tree on dev at 9c3b3d92f. Security was skipped with Vidoc disabled.
- #1441 reproduced in Chromium with the task-list response held for 6.5 seconds. The five-second visibility check failed on the seeded Checkout API link before the proposal. With the shared first-load budget, all 18 scenarios in the intake file passed. In a separate probe with the response held indefinitely, the browser trace recorded a 30.0-second visibility check followed by failure on that same link. The probe was removed.
