---
yulius-task: 7b50f659-ab91-451e-ad88-c994a6edbf1f
status: in-progress
---

# SPEC-491: a task whose work is not finished never reads Done

Status: in-progress
Date: 2026-11-14
Owner: Yulius task #474
Scope authorization: the request of task #474 (2026-11-14). The person reported, about #465 (the
deep audit), which was still working: "the task gets marked as done". Amends SPEC-001 TSK-012 and
TSK-014 and SPEC-487 CHN-003; decision in
[ADR-0332](../../docs/decisions/0332-done-needs-a-pull-request-that-would-merge-and-a-spec-that-says-done.md).

## Goal

A task reads Done, and releases the tasks that wait for it, only when nothing of its work is
left: its pull request would merge as it is, and the task's own record of its work says it is
finished. One rule decides that for the sidebar, Home, the counts, chains, start rules, a
supertask's wait and the tools.

## Context

- **What happened.** On 14 November 2026 at about 18:11 UTC #465 read Done and #473, chained
  after it, started. #465's plan read 28/28, its pull request #1287 was open with its merge
  armed, and its spec named twelve findings still to fix. At 18:20:13 this task (#474) was
  created "after #465" and started within a second: #465 again read Done, with #1287 open,
  `yulius/ci` green at its head `56b576065` and, since `dev` moved at 18:16:56, conflicting with
  `dev` (GitHub: `mergeable: CONFLICTING`, `mergeStateStatus: DIRTY`). The same happened on 13
  November at 16:23 (#466 to #468) and 19:57 (#469).
- **Which code ran.** The instance runs `main` at `ec07d1331` (`/internal/version`), promoted on
  13 November at 22:40 UTC. Its `task_is_done` has neither the armed-merge rule nor the
  pull-request-news rule of SPEC-487 (CHN-001, CHN-002, ADR-0328), which are on `dev` since
  pull request #1270 and reach the instance with the next promotion. So the Done at 18:11 (a
  green pull request with an armed merge) is the case ADR-0328 already closes on `dev`. The
  report said the pull request had "no CI checks at all": it had a passed run at its head; what
  it lacked was a head that could merge.
- **What `dev` still decides wrongly** (`App::task_is_done`, `crates/yulius-app/src/delivery.rs`):
  1. *A pull request that cannot merge counts as tested.* An open pull request makes a task Done
     when its checks are `Success` or `None`. Whether it conflicts with its base is not asked.
     Once the armed merge of a conflicting head has ended and its agent was told and stopped
     again (in #465 the goal's Stop hook gave up after nine blocks), the task reads Done.
  2. *"No checks reported" counts as "none required"* in this one place. The Git overview
     already reads a pull request without reported checks as pending when the delivery profile
     requires checks (#68), so the sidebar says "In review" while the chain is released.
  3. *A complete-so-far plan counts as the whole work.* "Plan complete" is every step finished.
     #465 adds a slice's step when the slice before it is merged ("The Plan grows by a slice's
     steps when the slice before it is merged", SPEC-483), so between two slices its plan reads
     n/n with a merged pull request and nothing armed: Done by every rule on `dev`, also after
     SPEC-484 (which lets a plan hold later slices but cannot make an agent list work it has
     not sliced yet).
- **The record a task already keeps.** A task that needs a spec keeps `status:` in its
  frontmatter: `draft`, `approved`, `in-progress`, `done` or `abandoned`. The validator accepts
  `done` only with every Plan item ticked and an Evidence section. Of the 439 specs on `dev`,
  437 say `done`; the two that say `in-progress` are SPEC-483 (#465) and SPEC-487 (#469), the
  two tasks that deliver slice by slice and are still working. Yulius reads the spec at every
  turn's end, every few seconds of a live run (`sync_steps_from_spec`) and at every delivery
  call (`dlv_read_spec`), but uses the status only to validate the file.
- **Where Done is decided.** `App::task_is_done` (chains, start rules, archive, delete),
  `App::task_is_done_locally` (the session limit, a supertask's wait, the capacity queue),
  `AttentionFacts::group` (Home, the tabs, `tasks_list`, `task_questions`, `after_all_tasks`)
  and `taskInfo` in the web UI's `status.js` (the sidebar, the task page). The last two read the
  pull request from the Git overview, which lists pull requests; GitHub's list carries no
  mergeability. The pull request watch already knows a conflict from Yulius's own clone
  (`App::conflicts`, PRN-004), cached per head and base.

## Requirements

| ID | Requirement |
| --- | --- |
| DON-001 | An open pull request is **tested** only when it does not conflict with its base and its checks passed at its head, or none are required. It conflicts when GitHub says it is not mergeable, or when Yulius's own merge of its head with the integration branch, as last fetched, conflicts (the check of PRN-004). One domain function (`PullSignal::from_wire`) decides it for `task_is_done`, the Git overview and the counts. |
| DON-002 | "None required" means the project requires none: no Yulius CI run is due at the head and the delivery profile does not require checks. With a profile that requires checks, a pull request without reported checks is pending (as the Git overview reads it since #68), in `task_is_done` too. |
| DON-003 | A conflicting pull request of a finished turn reads "In review · PR #N · conflicts with `<base>`" (Working): its agent is told by the pull request watch (PRN-004) and goes on by itself. The watch asks the head of the pull request's run, or the branch's own head where no run exists (a project without Yulius CI). Failing or errored checks keep their own reading, whatever the conflict. An achieved goal stays Done (CHN-003). |
| DON-004 | A task's spec is its record of the work it declared. Yulius keeps the spec's `status` with the task whenever it reads the spec (`tasks.spec_status`). While it says `draft`, `approved` or `in-progress`, the task has work left even when every step is finished: it is not Done by its plan and pull request, merged or tested; idle, it reads "Your turn", as with open steps. `done` and `abandoned` say nothing is left. A task without a spec, or whose spec was never read, declares its work by its steps alone. |
| DON-005 | Chains, start rules (`#N done`), a supertask's wait, the session limit, archive and delete ask the same rule: a chained task does not start in either case, and a closed task whose spec says its work is not finished is not Done (its chain needs a person, as with open steps). |
| DON-006 | The API says both facts: `PullView.conflicts` and `TaskSummary.spec_unfinished`. The web UI's `taskInfo` and the server's `AttentionFacts::group` decide alike. |
| DON-007 | The agent is told what keeps its task from Done: when every step is finished and the spec does not say `done`, the step tools and `spec_check` say that the task is not Done and tasks waiting for it do not start until the spec says `done` or the remaining work is added as steps. |
| DON-008 | SPEC-001 TSK-012 and TSK-014, SPEC-487 CHN-003, the architecture document, the API reference and ADR-0332 state the one rule. |

## Plan

- [x] The cause verified against the code, the running server and #465's transcript; the spec written and validated
- [x] A pull request that conflicts or lacks required checks is not tested, and a chained task waits (DON-001 to DON-003)
- [x] A task whose spec does not say done has work left, and a chained task waits (DON-004, DON-005)
- [x] The API, the web UI and the counts say the same as the chains (DON-006)
- [x] The agent is told what keeps its task from Done (DON-007)
- [x] ADR-0332, SPEC-001, SPEC-487, the architecture, the API reference and the changes log state one rule (DON-008)
- [ ] Every gate green and the independent review of the exact head approved

## Acceptance criteria

- AC-1 (DON-001, DON-005): with a finished turn, a complete plan and an open pull request whose
  run passed at its head, a task chained after it starts; once `dev` moves so that the head
  conflicts, a task chained after it stays `chained`, and `#N done` in a start rule is false.
  The test fails on `dev` (the chained task is queued).
- AC-2 (DON-002): with a profile that requires checks, no Yulius CI and no check reported, the
  task is not Done and the chained task waits; with a profile that requires none it is Done.
- AC-3 (DON-004, DON-005): with every step finished, a merged pull request whose news was told
  and a spec that says `in-progress`, the chained task stays `chained` and the task counts as
  "needs you"; when the spec says `done` the chained task starts. The test fails on `dev`.
- AC-4 (DON-006): `GET /api/sandboxes/{sid}/git` carries `conflicts: true` for the conflicting
  pull request and the task summary `spec_unfinished: true`; `taskInfo` reads "In review · PR #N
  · conflicts with dev" and "Your turn" for the two cases, and Done without them.
- AC-5 (DON-007): `spec_check` and the answer of the step write that finishes the plan carry the
  sentence about the spec's status while it is not `done`, and not once it is.
- AC-6 (DON-003): in a project where no run exists at the head, the watch types the conflict
  into the session once; the test fails without the change ("the conflict was not typed").

## Test plan

- Domain unit tests: `PullSignal::from_wire` over state, checks, the profile's requirement and a
  conflict; `AttentionFacts::group` with a conflicting pull request and with work declared.
- Integration tests in `crates/yulius/tests/merge_train.rs` (real Git, the fake GitHub): AC-1 and
  AC-3 with a chained task, each run first without the fix to see it fail; AC-2 in
  `crates/yulius/tests/delivery.rs` or beside them.
- Store test: `spec_status` is kept and read back; the migration applies to a version 58 file.
- Watch test (`pr_watch.rs`, the fake agent in a real terminal): a conflict is typed without a
  run at the head.
- Web unit tests (`status` tests) for `taskInfo`; the end-to-end suite, since the web UI changes.
- The gate (`scripts/dev ci`), the security scan (reported as disabled if it is).

## Decisions

- **The spec's `status` is the signal for declared work**, not a reading of the spec's prose
  ("planned for later slices") and not a new flag. It is the one field the agent already keeps,
  the validator already guards (`done` needs every box ticked and Evidence), and 437 of 439
  specs on `dev` show it kept right. A flag on the plan ("final") would be the same statement a
  second time, and every existing task would lack it.
- **Stored, not read at the decision.** The counts ask for every idle task many times a minute
  and a closed task has no worktree left; the status is written when Yulius reads the spec
  anyway.
- **A conflict is read from GitHub or from Yulius's own clone.** GitHub's list of pull requests
  carries no mergeability, and one request per pull request per overview would cost the rate
  limit; the clone's answer is what the pull request watch tells the agent.
- **A conflicting pull request is Working, not "Needs you".** The watch tells its agent within
  20 seconds, so "Needs you" would be a false alarm each time `dev` moves; a pull request left
  that way for an hour is flagged as stalled (PRN-009).
- **A pull request behind its base is still tested.** Its head passed and merges cleanly;
  bringing it up to date is part of the merge, which Yulius does when a merge is asked for.
- **The merge gate is unchanged.** `check_merge` still lets a merge pass with no check
  reported: a project without any CI must be able to merge. Only what reads as Done changes.
