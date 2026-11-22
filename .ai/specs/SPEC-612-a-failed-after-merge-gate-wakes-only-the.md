---
yulius-task: 982d8a14-726a-4a9a-854b-0070710fca55
status: in-progress
---

# SPEC-612 A failed integration gate is repaired by a task still at work

## Goal

Keep finished tasks finished. Give the red integration branch one repair owner among the project’s working tasks. Keep later deliveries moving once a verified fix reaches staging.

## Context

On 21 November task #588 received a fix-forward instruction for a test already failing on task #589's merge. Six later revisions carried the same failure. In quartio-v2 a closed task's failed revision needed a manual rerun and nobody received a notice. SPEC-575 records a request for each delivery and CI already keeps named failing tests.

## Requirements

- GF-001 Compare full integration and candidate verdicts along first-parent history. Identify each test at its first failing revision since it last passed. Report the introducing revision and task separately from the repair assignment. Keep an unknown job failure actionable while later runs skip or cancel that job, including reruns of the same revision and after CI log retention.
- GF-002 Record the inherited test, originating revision and task on the later request. The staging queue owns that wait until a verified descendant serves it, the request is replaced or the target disappears. Restart and repeated ticks preserve the outcome. The inherited request never directs its task to fix the test.
- GF-003 Reuse the revision's integration or candidate verdict. Admit outstanding candidate suites in first-parent order, one at a time. A candidate runs once and is compared because any merge can both repair an open failure and introduce a new one. A person’s cancellation stays actionable through retention and prevents automatic readmission at that head.
- GF-004 Tell each running project task with work left about open failures. Keep one repair assignment until that worker leaves. A worker has a live attempt and an active turn, unfinished steps, an open delivery or outstanding goal or spec work before merge. Exclude paused and held workers. Choose the lowest project task number among eligible workers when a new assignment is needed. Tell the assignee first and notify peers once its session received the assignment. Persist delivery to avoid repeating the instruction at every merge, restart or tick.
- GF-005 With no worker available, show project people the failing tests, original revision and merge task in project CI and Status. Offer writers a rerun of the latest affected revision and a new task. Resolve the notice on a passing rerun or a containing fix.
- GF-006 Only a person resumes a parked or closed task. A running task with all steps complete and a merged delivery receives no machine prompt. Keep all staging verdicts on its delivery, page and lists. Explicit unfinished steps still receive actionable news in a running session.

## Plan

- [x] Specify failure ownership and candidate scheduling
- [x] Attribute new and inherited test failures in integration order
- [x] Serve inherited requests with the verified fix
- [x] Keep finished and parked tasks quiet on automatic outcomes
- [x] Assign the repair to one working task and inform its peers
- [x] Show people the rerun and start-task actions when no worker remains
- [x] Preserve unresolved job evidence across reruns of the same revision
- [x] Deliver candidate scheduling independently
- [x] Verify the incident regressions and review the delivery
- [x] Deliver the isolated quality-job compiler cache repair
- [ ] Deliver the reviewed integration failure ownership backend
- [ ] Deliver the project CI and Status repair actions on the verified API

## Acceptance criteria

- One failing merge followed by three with the same failure leaves all finished sessions untouched. One working task receives the repair and its peers receive that owner’s number. Each later receipt names the original task and revision.
- A later merge that also introduces a second failing test is identified as the origin of the second test. A passing revision ends the failure interval.
- The verified fix serves the three requests in branch order and each receipt identifies the served revision.
- When no task is at work, the failure appears to project readers with the originating merge, failing test and actions to rerun or start a task, including without a staging request. A parked task receives neither PR input nor an automatic resume.

## Test plan

For the quality repair, reproduce the three real compiler-cache misses on the job’s 1 GiB temporary filesystem. Exercise the same builds with the fixture’s own free-space reserve. Verify the complete Python quality suite with cache-hit and integrity assertions intact.

Exercise real staging requests against a temporary Git repository and recorded CI verdicts. Inspect notification recipients, request history, served revision and project-visible Status. Check a passing rerun and an unrelated failing job. Run affected tests, repository contracts and the UI checks before independent review.

## Decisions

The first delivery covers CI inventory reuse and admission. The owner assigned this task the independent quality-job repair on 22 November. Deliver the cache fixture repair first, then the reviewed failure ownership backend and the project CI and Status actions. Ask for staging once after the last merge.

Running each candidate once in order preserves evidence of mixed failures and accepts a fix from any task. Reuse existing gate and deployment receipts. Only a verified descendant can satisfy an inherited delivery. A durable project assignment follows the lowest-numbered eligible worker initially and survives other tasks joining. A person can resume finished work. See ADR-0450.

## Evidence

The ownership and staging regressions first ran on the combined implementation. The inventory delivery landed in PR #1675 at `3309d1e18` after Yulius CI run `bd6bb7cb-0438-4c81-98bb-5339713ff481` passed.

- Before the change, the integration fixture failed on the first receipt. Its message named only the job `full` even though CI retained the test `delivery::gate`. The closed-owner fixture failed because the project returned no notice.
- The Chromium notice test failed on the missing test name in the CI page. With the notice in both views, it passed and followed the Status link to the existing rerun action.
- The staging fixture records a single open failure through three more deliveries, introduces a distinct failure on a fifth, and checks each inherited receipt after a passing descendant deploys.
- Against the first reviewed head, all six review regressions failed. An answered source received a second handoff, an explicitly unfinished verifier lost its inherited wait, and a held worker prevented the available workers receiving the right assignments. The rerun linked the introducing commit instead of its red descendant, an interrupted job disappeared, and a candidate below 201 later CI admissions allowed a concurrent suite.
- The full Chromium suite passed 1,325 tests with one configured skip. It exercised the failing test, first merge and both actions on project CI and Status. The catalog primitive passed APM audit. Vidoc was disabled for this project.
- The same six regressions passed with the fixes. The rerun fixture submits the action through HTTP, verifies the tested SHA and clears the notice on a passing result. The terminal fixture verifies that a drafted worker receives no repair instruction.
- Four assertions failed on the second reviewed head. CI ran a finished candidate twice after 201 newer admissions. A test remained in the repair notice after it moved matrix cells and passed. A cleared native goal hid the people’s notice, and an outstanding native goal in a prompt-mode session showed it. The goal fixtures use actual CLI sessions and native transcript outcomes.
- All four second-review regressions passed after the fixes. The scheduler retains the current head’s verdict independently of the recent-run window. Gate history releases a passing matrix cell’s earlier test, and worker selection reads the native session’s outstanding goal.
- On the third reviewed head, a partial rerun at the same SHA lost the first job error. The corrected comparison passed all 50 staging and parked-candidate cases, including a test that prunes both earlier attempts, checks both failures still target the current run, then clears them on a full pass.
- A retained pull-request verdict that skipped its after-merge job incorrectly covered the integration head after 201 more admissions. The regression observed only the old PR run. The scheduler now checks the integration reference and actual full-suite coverage when extending its inventory, retaining configuration refusals in admission order.
- Independent review approved inventory head `b47237a65`. All four scheduler regressions passed on the corrected binary, including the skipped PR and later configuration refusal.
- On the ownership review head, a green rerun that skipped the unresolved job returned an empty Status notice. A manually cancelled candidate became a fresh testing request after pruning. Both regressions failed before the corrections.
- At `a572d4924`, all 3,435 selected Rust tests passed (run `7fbcca68-bcea-4397-b2c7-293bf89e61f7`). Both retention regressions passed in a separate six-case incident run. Chromium passed 1,326 tests with one configured skip on the integrated UI. Clippy passed with warnings denied.
- On the next review head the project notice kept both unresolved tests but the red descendant’s durable receipt named only the observed one. The regression failed with `quick::broken` in the receipt and `full::broken` missing. The receipt now uses the same open test history as the project notice.
- At `2c1c49d49` the receipt regression passed. Of 3,436 selected tests, the sole failure was an intermittent EOF in the unchanged opaque tunnel restart test, also reproduced on the inventory delivery and tracked in issue #1676. Review found that output from a passed retry kept a repaired test active when a sibling job failed. The regression reproduced the incorrect inherited `full::retried` alongside the new `quick::new`. The history now consults the job's final verdict before retaining a failure name. The task page and list passed all 28 Chromium cases with the project notice UI reserved for the final delivery.
- On the split backend head `a60bde75b`, all 3,437 selected Rust tests passed in run `89a5554e-1663-47f2-a7d7-72d5e717c081`, including the successful retry. Extending the manual cancellation case through a failed replacement run then exposed a stale `cancelled` notice beside `suite::broken`. The history now replaces a run-level verdict when a suite settles and retains unresolved job evidence separately.
- At `0f6b911c8`, all 3,437 selected Rust tests passed in run `7b27268e-89b0-40aa-a3f1-41e82f71f8ef`. A further regression reran a green ancestor through HTTP and pruned the red integration head's logs. Status incorrectly returned an empty notice. The archive now retains the run metadata, so the original run ID opens the head's detail and retries its exact SHA even after its logs expire.
- Independent backend review approved `904d7f8e0` after seven focused incident checks. All 3,438 selected Rust tests passed in run `2869beb4-0e8e-43b5-b81b-b32d256e66c6` with 24 configured skips. The archived head regression exercised both notices, run detail and the public rerun endpoint. Project notice presentation follows in the final UI delivery.
- PR #1678's file-contract job failed at `eb357c470` in run `e1930f4b-6a25-4886-999c-82ea91437ad0`. The catalog contract reproduced locally because the delivery skill had dropped the instruction to end the task after requesting staging. Restoring that instruction alongside the new ownership rules made the original assertion pass. All 166 tests selected by `scripts/repository_tests.py` passed in run `8e0bcff3-4d99-457c-be74-ff0da2d421a9`. The APM audit found no drift.

- In CI run `9b898c87-816d-4b23-875a-1889e12b9112`, three real compiler-cache tests failed to record a warm hit. All three reproduced on the 1 GiB temporary filesystem with and without `YULIUS_BUILD_SCRATCH`. The pinned mbx reported a default minimum free-space reserve of 5 GiB. With a 64 MiB reserve scoped to the fixtures, all three passed in the same environment. The quality job's complete Python suite passed all 278 tests. [Issue #1680](https://github.com/Quartio-Studio/yulius/issues/1680)
- Independent review approved the isolated repair at `962f83552` after running the three failing cases on the 1 GiB filesystem.
- The isolated quality repair landed in PR #1678 at `ff4ed8b89` on green Yulius CI run `bd55499c-3190-4dda-8383-df1c12dc6f49`.
