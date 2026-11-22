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
- GF-003 Reuse the revision's integration or candidate verdict. Admit outstanding candidate suites in first-parent order, one at a time. A candidate runs once and is compared because any merge can both repair an open failure and introduce a new one.
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
- [ ] Verify the incident regressions and review the delivery

## Acceptance criteria

- One failing merge followed by three with the same failure leaves all finished sessions untouched. One working task receives the repair and its peers receive that owner’s number. Each later receipt names the original task and revision.
- A later merge that also introduces a second failing test is identified as the origin of the second test. A passing revision ends the failure interval.
- The verified fix serves the three requests in branch order and each receipt identifies the served revision.
- When no task is at work, the failure appears to project readers with the originating merge, failing test and actions to rerun or start a task, including without a staging request. A parked task receives neither PR input nor an automatic resume.

## Test plan

Exercise real staging requests against a temporary Git repository and recorded CI verdicts. Inspect notification recipients, request history, served revision and project-visible Status. Check a passing rerun and an unrelated failing job. Run affected tests, repository contracts and the UI checks before independent review.

## Decisions

The first delivery covers CI inventory reuse and admission. The second covers staging queue order, failure history, task input and project notices. Request staging once after both deliveries.

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
