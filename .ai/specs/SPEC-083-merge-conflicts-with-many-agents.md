---
yulius-task: none
status: done
---

# SPEC-083: parallel tasks stop colliding at merge

Status: implemented
Date: 2026-10-13
Owner: the 2026-10-13 coordination round (engineer B), from the `yulius` sandbox's data
Scope authorization: user request (F 2026-10-13): "Tests kept getting ... rejected because of the
conflicts on merge while many agents at once were working. Fix the root cause and not just patch
... we'll onboard other repos soon (studio, product)."

## Goal

With 8 to 12 agents at once, a task's pull request is no longer refused or failed at merge for
reasons parallel work creates, in this repository and in any repository onboarded next:
- numbered files (decision records, specs, migrations) never take a number another task holds;
- the logs every task appends to (the changes log, the decision index) merge without conflicts,
  without any setup in the repository;
- bringing a branch up to date with the integration branch costs no new review.

## Context

Data of the `yulius` sandbox, 2026-10-11 to 13 (74 agent transcripts, the audit log, the retained
CI runs):

- **Numbered records.** Parallel tasks each took the next free number of their own checkout.
  The second to merge failed the repository check ("shares decision number … the record merged
  later takes the next free number") seconds into CI, renumbered, needed a new review, armed
  again, and often collided with the next one: task #57 went 0052 → 0053 → 0054 → 0058, task #62
  0052 → 0053 → 0055 → 0061, task #66 0051 → 0055 → 0060. 81 decision records were renamed (`git
  mv docs/decisions/NNNN`) in 36 sessions; the collision message appears 65 times in 10 sessions.
  6 of the 11 failed CI runs retained (2026-10-13 15:06 to 19:28) were these collisions, each
  failing in 7 to 12 s. Migrations collided the same way (task #70: "renumbered migration
  0019/ADR-0062"). Specs were already numbered by Yulius (SPEC-020 SPC-001), from the task's
  checkout and the other tasks' spec paths only.
- **Shared logs.** Yulius's briefing has every task "add a short entry at the top of" the changes
  log and record its decisions, so every pair of tasks edits the same lines. This repository
  commits `merge=union` for `docs/changes.md` (since 2026-10-12 05:56) and the ADR index (since
  2026-10-13 01:19); local merges have conflicted there 0 times since (before: 9 and 30 conflicts
  in 3 and 13 sessions). GitHub's merge ignores merge drivers: on 2026-10-12 between 19:10 and
  23:44, 13 armed merges were refused with "Pull Request has merge conflicts". Since Yulius merges
  the integration branch locally before it merges (SPEC-050 DLV-203), 0 were. A repository
  onboarded next (studio, product) has no such `.gitattributes`: every armed merge would be put
  on hold for its changes log.
- **Reviews of dev merges.** Agents merged `origin/dev` 196 times in 54 sessions. A merge moves
  the head, and `delivery_merge` accepts only a review of the exact head: 33 of the 57 agent
  merges (2026-10-12/13) record a review round spent on dev merges or renumbering; tasks armed
  168 times for 57 merges (up to 12 times one task).
- **What stays real.** 14 merges were put on hold by DLV-203 for conflicts Git cannot resolve:
  6 only in prose documents every feature edits (`docs/product.md`, `docs/architecture.md`,
  SPEC-001), 4 in code and `e2e/README.md`, 4 in code only. 3 failed runs were compile errors
  after a dev merge (a field added on dev). Those need the agent.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NUM-001 | `record_number` (an MCP tool of every task) gives the next free number of a directory's numbered files (`<prefix><digits><rest>`: `0066-x.md`, `SPEC-083-x.md`, `V12__x.sql`): past every such file on the integration branch (fetched first), in every live task's checkout (uncommitted files included; its pushed branch when it has no checkout) and every number given out before. It writes the number as the directory does (prefix, zero-padded width) and names the file to create. A `prefix` picks a series; without one, the directory's largest. Dates, timestamps (8 digits or more) and versions are no sequence numbers; a directory with no numbered file, or outside the repository, is refused. | `delivery::parallel_tasks_get_their_own_record_numbers`; `numbering::tests` (5) |
| NUM-002 | A number given out is reserved for the task in the store (`record_numbers`, one per sandbox, directory, prefix and number, schema version 20): reservations survive restarts and go with a deleted task; reserving is serialized. | `store::record_numbers_are_reserved_once_and_go_with_their_task`; `delivery::parallel_tasks_get_their_own_record_numbers` |
| NUM-003 | A task asking again before a file uses its number gets the same number back; once the file exists, the next free one. | `delivery::parallel_tasks_get_their_own_record_numbers` |
| NUM-004 | A task's spec number (SPEC-020 SPC-001) is given out the same way, so it is never a number another task reserved or a spec merged since its checkout was made. | `delivery::a_spec_number_is_never_one_another_task_reserved` |
| NUM-005 | The briefing tells every agent that numbered files (ADRs, migrations) take their number from `record_number`; the tool is announced and served. | `briefing::tests::short_parts_donate_budget_to_long_ones`, `briefing::tests::tool_list_is_unique_and_rendered`, `mcp::tests::every_tool_the_briefing_announces_is_served` |
| MRG-001 | The logs Yulius has every task append to merge by Git's union driver in every repository: the changes log, and the decisions file or the decision directory's `README.md` index, from the project's `yulius.toml`. At every task start Yulius keeps them in its own block of the clone's `info/attributes`, which every worktree of the clone reads; the rest of that file is kept, and the repository needs no committed `.gitattributes`. A path that a gitattributes pattern would read as more than a path is left out. | `git_ops::merge_attributes_in_info_reach_every_worktree`; `delivery::the_changes_log_merges_by_union_in_any_repository`; `supervision::task_worktrees_guard_protected_branches_and_status_changes_reach_the_ui` (a real start keeps the lines); `integration_merges::tests::the_changes_log_and_the_decision_index_merge_by_union`; `merges::tests::the_managed_block_is_replaced_and_the_rest_kept` |
| REV-001 | `delivery_merge` accepts a `reviewed_head` the branch moved past only through Git's own merges of the integration branch: walking first parents from the head back to it (at most 16), every commit is a merge of two parents, the second already on the integration branch, recording exactly the tree Git's merge of them gives with the repository's merge drivers. What the integration branch holds is fetched first, never taken from a remote-tracking ref a worktree could have moved. New work, a conflict resolved by hand, a change slipped into a merge or a merge of unmerged work still need a review of the new head. The review summary in the audit says the review carried over. | `git_ops::only_clean_merges_of_the_integration_branch_keep_a_review`; `delivery::a_review_carries_over_clean_merges_of_the_integration_branch`; `delivery::agents_merge_only_through_gated_tools` (a stale review is still refused) |

## Plan

- [x] Spec written and validated
- [x] Numbered names, conventions and the next free number in the domain
- [x] Reservations in the store
- [x] `record_number` over the integration branch, every live task and the reservations
- [x] Spec numbers given out the same way
- [x] The shared logs merge by union in every repository
- [x] Reviews carry over clean merges of the integration branch
- [x] Docs, ADR, review and gates

## Acceptance criteria

- Two tasks asking `record_number` for one directory never get the same number, whatever their
  checkouts contain and whatever merged since they started.
- A repository without `.gitattributes` gets its changes log merged by union when Yulius brings
  an armed branch up to date, instead of a hold.
- An agent that merges `origin/<integration>` cleanly after its review arms or merges with that
  review; any other change after it is refused.
- The SPEC-050 DLV-203 tests and the `delivery` suite still pass; `scripts/dev ci` passes.

## Test plan

- Unit: `numbering` (names, conventions, widths, dates, timestamps, versions); the managed block
  of `info/attributes`; the union lines from a layout.
- Git, against real repositories (`git_ops`): attributes in `info/attributes` reach a linked
  worktree's merge and `merge-tree`; clean merges keep a review, and new work, a change slipped
  into a merge, a hand-resolved conflict and a merge of other work do not.
- Store: reservations once per number, ordered, surviving a reopen, deleted with their task.
- Integration (`delivery`, fake GitHub over a real bare repository): two tasks' numbers with an
  uncommitted record and a record merged since they started; a spec number past a reservation;
  a review carried over an agent's own dev merge; the changes log on hold without the attribute
  and merged with it.

## Decisions

ADR: [0066](../../docs/decisions/0066-parallel-tasks-stop-colliding-at-merge.md).

- **Yulius gives the numbers out, not a convention change.** Renaming numbered records to dates
  or slugs would fix this repository only; studio and product, and every migration tool, number
  files. A reservation table is the one place that knows what other tasks hold before their
  files exist anywhere.
- **Union in `info/attributes`, not fragments.** Changelog fragments would remove the shared file
  but change every onboarded repository's documents. The logs are Yulius's own convention, so
  Yulius makes them merge, without any file in the repository. GitHub never sees the
  conflict because Yulius merges locally first (DLV-203).
- **No pushes into pull requests whose merge is not armed.** GitHub's conflict state of an
  unarmed pull request blocks nothing: Yulius CI tests the head, and Yulius brings a branch
  current before it merges. Each push starts a CI run (quality median 6.7 min, e2e 9.7 min, two
  lanes): with 8 to 10 open pull requests and the integration branch moving up to 4 times an
  hour (2026-10-13), keeping them all current would need 30 to 40 runs an hour, several times
  what the lanes finish, and the armed merges would wait behind them. A push under an agent that
  is not waiting on its merge would also race its next turn. With REV-001 an agent's own dev
  merge costs no new review, so staying current costs only the CI run its merge needs anyway.
- **What is left to the agent.** Conflicts in prose documents and code, and compile errors after
  a dev merge, need judgement: DLV-203 names the conflicting files to the agent, and CI at the
  merged head catches a semantic conflict before the merge.

## Evidence

- **Data.** The queries ran read-only on the VM (`audit_log`, `ci_runs`/`ci_jobs`, the 74
  transcripts in the sandbox's Claude home); the numbers are in Context.
- **Red first.** Each test failed without its fix:
  - `parallel_tasks_get_their_own_record_numbers`: no `record_number` tool;
  - `a_spec_number_is_never_one_another_task_reserved`: SPC-001 gave `SPEC-002`, the number
    task 1 held;
  - `a_review_carries_over_clean_merges_of_the_integration_branch`: "the independent review
    covered …, but the branch is at …"; without the fetch, unreviewed work merged through a
    moved `origin/quartio` was armed;
  - `the_changes_log_merges_by_union_in_any_repository`: the second update was put on hold for
    `docs/changes.md`.
- **Tests.** See the requirement table; the commands and counts are in the pull request.
