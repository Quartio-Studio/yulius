---
yulius-task: 982d8a14-726a-4a9a-854b-0070710fca55
status: in-progress
---

# SPEC-612 A failed integration gate wakes the task that introduced the failure

## Goal

Give each newly failing test one owner. Keep later deliveries moving once a verified fix reaches staging.

## Context

On 21 November task #588 received a fix-forward instruction for a test already failing on task #589's merge. Six later revisions carried the same failure. In quartio-v2 a closed task's failed revision needed a manual rerun and nobody received a notice. SPEC-575 records a request for each delivery and CI already keeps named failing tests.

## Requirements

- GF-001 Compare full integration and candidate verdicts along first-parent history. Identify each test at its first failing revision since it last passed. Notify a later owner only of new failures. Keep unknown failures actionable.
- GF-002 Record the inherited test, originating revision and task on the later request. The staging queue owns that wait until a verified descendant serves it, the request is replaced or the target disappears. Restart and repeated ticks preserve the outcome. The inherited request never directs its task to fix the test.
- GF-003 Reuse the revision's integration or candidate verdict. Admit outstanding candidate suites in first-parent order, one at a time. A candidate runs once and is compared because any merge can both repair an open failure and introduce a new one.
- GF-004 Give project readers a notice on the project and in Status for a failed integration revision whose owner has closed or disappeared, including its test and access to the existing run-again action. Resolve the notice on a passing rerun or a containing fix.

## Plan

- [x] Specify failure ownership and candidate scheduling
- [ ] Attribute new and inherited test failures in integration order
- [ ] Serve inherited requests with the verified fix
- [ ] Show unowned gate failures and the rerun on project and Status views
- [ ] Verify the incident regressions and review the delivery

## Acceptance criteria

- One failing merge followed by three with the same failure leaves only the first task with a fix-forward instruction. Each later receipt names the original task and revision.
- A later merge that also introduces a second failing test receives a fix-forward for the second test. A passing revision ends the failure interval.
- The verified fix serves the three requests in branch order and each receipt identifies the served revision.
- A closed or missing task's failure appears to project readers with the failing test and a way to rerun it, including when it has no staging request.

## Test plan

Exercise real staging requests against a temporary Git repository and recorded CI verdicts. Inspect notification recipients, request history, served revision and project-visible Status. Check a passing rerun and an unrelated failing job. Run affected tests, repository contracts and the UI checks before independent review.

## Decisions

Running each candidate once in order preserves evidence of mixed failures and accepts a fix from any task. Reuse existing gate and deployment receipts. Only a verified descendant can satisfy an inherited delivery.

## Evidence

- Before the change, the integration fixture failed on the first receipt. Its message named only the job `full` even though CI retained the test `delivery::gate`. The closed-owner fixture failed because the project returned no notice.
- The Chromium notice test failed on the missing test name in the CI page. With the notice in both views, it passed and followed the Status link to the existing rerun action.
- The staging fixture records a single open failure through three more deliveries, introduces a distinct failure on a fifth, and checks each inherited receipt after a passing descendant deploys.
