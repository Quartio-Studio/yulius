---
yulius-task: 97d4c28f-ac17-4d91-9cc8-02534b58ae79
status: done
---

# SPEC-425: Work committed after a task's merge is delivered again

Status: done
Date: 2026-11-05
Owner: Yulius task #407
Scope authorization: task #407, asked by the project's owner with a screenshot ("yulius doesnt
pick up things. This is on free private org so some org features arent available but ci still
should work"). Fixes #1074.

## Goal

When a task's agent commits more work on its branch after the task's pull request was merged (a
follow-up fix in the same task), Yulius delivers it as it delivers any work: it pushes the branch
and opens a new pull request into the integration branch, and `delivery_merge` merges that one on
green. Nothing waits on a pull request that is already merged.

## Context

- Symptom (task #1 of a project in a free private GitHub organisation): PR #1 had merged; the
  agent then committed `379da3c`, called `delivery_merge` and waited in "Waiting for checks or
  merge". Nothing was pushed, no CI ran, and the merge never came. The agent pushed and opened
  PR #2 itself; Yulius CI ran on it and passed, but the task still showed PR #1.
- Root cause: `try_auto_open_pr` (`crates/yulius-app/src/delivery.rs`), which both the end of a
  clean turn and `delivery_merge` call, returns `Unchanged` whenever the delivery record is past
  `pr_open`. The record keeps the merged pull request (#68), so the new head is never pushed and
  no pull request opens. `delivery_merge` then finds the merged pull request with no checks
  (`none`); the standard profile requires checks, so it arms a merge that can never happen.
- The free GitHub plan is not the cause: Yulius CI needs no paid GitHub feature, and it ran as
  soon as a pull request carried the head.
- Why tests missed it: the delivery tests stop at the merge (`a_ready_task_opens_its_pull_request_by_itself`
  ends with "the merged pull request stays on its task"); none commits after it.
- What else the cause touches: the Git page and the task header keep showing the merged pull
  request (they read the same record), and the task counted as Done ("merged") while it held
  undelivered work.

## Requirements

- FUP-001. A task whose pull request is merged gets its work delivered again when its branch
  head carries work the merged pull request did not: the merged head is an ancestor of the head,
  the head is more than it plus Git's own clean merges of the integration branch
  (`review_covers`, SPEC-083 REV-001), and what it changed since the merged head adds to the
  integration branch (Git's merge of the two from the merged head changes the branch's tree).
  Every check reads a fresh fetch. Once the head is ready by the same rules as the first delivery
  (spec, plan, clean worktree; else the task stays merged and the reason is said), Yulius starts a
  new delivery of the same task (the record back to `none` if it still names that pull request,
  audited `delivery.follow_up`) and opens a new pull request (or takes on the branch's open one)
  at the end of a clean turn and on `delivery_merge`.
- FUP-002. A head equal to the merged head, behind it, only clean merges of the integration
  branch since it, or one the integration branch already holds (a follow-up merged outside
  Yulius) is not new work: the task stays merged and nothing is pushed or opened. The verdict is
  kept per head and pull request, so later Stops at that head ask GitHub nothing.
- FUP-003. `delivery_merge` on a merged task never arms a merge of the merged pull request. With
  no new work it answers `merged: true`, that pull request #N is already merged. When Yulius
  cannot judge (a branch rewritten so the merged head is no longer its ancestor, with the
  `git merge` that brings it back; a merged head missing from the clone, with the fetch of
  `pull/N/head`; what the head changed conflicting with the integration branch, with the merge
  to make; the merged pull request unreadable on GitHub; a failed fetch; new work while a live
  merge train holds the task, on an instance with CI) it refuses with that reason, the end of a
  turn notes it for `yulius wait` (the note goes once the head is covered again), and nothing is
  pushed, opened or reset.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- After a merge, a new commit on the task branch is pushed and pull request #2 of the task opens
  at the next clean Stop; `delivery_merge` arms it and Yulius merges it on green.
- `delivery_merge` alone (a goal task) also opens the new pull request before it arms.
- After a merge, the merged head and a clean merge of `dev` into it open nothing; the task stays
  `merged_integration`, and `delivery_merge` answers "already merged" with nothing armed.

## Test plan

- `crates/yulius/tests/delivery.rs` `work_committed_after_the_merge_opens_a_new_pull_request`
  (standard profile, real Git, the GitHub mock squash-merging into the origin): merged head →
  nothing opened, "already merged", not armed; clean merge of `origin/dev` → nothing opened;
  follow-up commit → pushed, PR #2 open, `delivery.follow_up` audited, armed and merged on green;
  another commit → the merge call alone opens PR #3 and arms it. Fails before the fix (armed
  with checks `none`).
- `work_after_the_merge_opens_nothing_it_cannot_judge`: an open step blocks and the task stays
  merged; a follow-up opened and merged by hand outside Yulius opens no third pull request and
  reads "already merged", also after `dev` changed the same file since, and again at that head
  while GitHub cannot answer (the kept verdict); a branch rewritten onto `dev` is refused with
  the `git merge` remedy, not pushed; the merged pull request unreadable on GitHub is refused
  with "is unknown", not armed. Without the integration-branch check it fails (mutation run).
- `a_follow_up_merged_by_hand_is_judged_since_the_merged_head`: a follow-up squash-merged by
  hand from a branch that never merged `dev`, then `dev` editing the merged pull request's file,
  reads "already merged" (fails without `--merge-base`); a conflicting change is unknown, said to
  the merge call and in `yulius wait`'s note, which goes once the head is covered again (fails
  without the clearing); new work with uncommitted changes keeps the task merged (fails without
  the readiness check before the reset); a merged head missing from the clone gives the
  `pull/1/head` fetch; an unfetchable origin is "could not be refreshed"; then #3 opens and arms.
- `work_after_the_merge_waits_for_a_live_merge_train`: in a live train, a head that only merged
  `dev` reads "already merged"; new work is refused ("merge train that has not ended") and not
  reset; once the train ends it opens its pull request.
- `a_leftover_train_holds_nothing_without_ci`: with CI off, a leftover trains file holds nothing
  and the follow-up opens (fails without the CI check in `in_train`).
- The existing delivery tests (`#68` follow-ups to an open pull request, the merged pull request
  staying on its task) still pass.

## Decisions

- [ADR-0269](../../docs/decisions/0269-work-after-a-tasks-merge-is-delivered-as-a-new-pull-request.md):
  the follow-up is a new pull request of the same task, not a new task and not a refusal.

## Evidence

- Reproduced: `work_committed_after_the_merge_opens_a_new_pull_request` committed first
  (4ced334b) failed on `dev`: `delivery_merge` on the merged pull request answered
  `armed: true, checks: none`, the stuck state of the report.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^delivery::/)'`: 62 passed
  at the final code (3e2c2979). `a_leftover_train_holds_nothing_without_ci` failed with
  `merge_train.rs` as before its fix (the test commit 7f6bd189) and passes with it.
- Mutation runs (each restored afterwards): adds_to always "follow-up" → the cannot-judge test
  fails; no `--merge-base`, no readiness check before the reset, no note clearing → each fails
  `a_follow_up_merged_by_hand_is_judged_since_the_merged_head`; the round-4 train case failed
  before its fix (commit "a train member with nothing new reads already merged").
- `scripts/dev ci` at 5901ad6d: exit 0, 3129 tests passed, 16 skipped. At ef110353: 3128
  passed, 1 failed: `task_done::a_done_onboarding_task_is_archived_and_the_sandbox_is_ready`
  timed out waiting for the shell probe (90 s) under full-suite load; it passed in the previous
  full run and 5 of 5 isolated runs, touches no delivery code, and is filed as #1078. Rerun at
  the final code 3e2c2979: exit 0, 3130 tests passed, 16 skipped; Yulius CI runs the merged
  head.
- `scripts/dev security`: exit 0, "Security scan disabled: Vidoc is turned off in Yulius" —
  unrun, not passed.
- Independent review in fresh sessions: rounds 1-3 requested changes (all own-code findings fixed
  test-first, listed in the commits "review round N"), round 4 approved 5901ad6d with one low
  finding (a train member with nothing new read "unknown"), fixed test-first in 29201a37; round 5
  approved ef110353 with two minor findings (docs wording; `in_train` ignored CI being off),
  fixed, the second test-first; round 6 approved 3e2c2979 (Evidence brought up to date after it,
  docs only).
- Pre-existing, outside this change: #1075 (an armed merge whose pull request merged outside
  Yulius is never dropped), found by review round 2.
- Not tested: a real GitHub repository on a free private organisation (the reporter's); the
  delivery path is the same for every plan, and the GitHub mock squash-merges into a real origin.
  Git older than 2.40 (no `merge-tree --merge-base`): reads as unknown by design, not exercised.
