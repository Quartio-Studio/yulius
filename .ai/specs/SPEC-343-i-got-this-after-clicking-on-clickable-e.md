---
yulius-task: 0c8636eb-8928-44b3-890b-4dcae9f6c701
status: in-progress
---

# SPEC-343: a Git board card without a worktree says why and opens nothing

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #327
Scope authorization: task #327: a screenshot of the task changes view of quartio-v2 #36 showing
"Couldn't load the changes: the task's worktree is gone; its branch and pull request are on
GitHub" and "i got this after clicking on clickable element. element shouldnt be clickable but just
say that it's gone". Fixes #943.

## Goal

On the Git page, a task card whose worktree is not on disk is not a link and says why ("Worktree
gone", or "No worktree yet" for a task not started), instead of opening a changes view that can
only fail.

## Context

- Symptom: the card in "Where every task is" (`git.js` `card()`) links to `/s/<slug>/git/t/<N>`;
  there `GET /api/tasks/{tid}/changes` answers 409 from `App::review_target` when the task's
  `worktree_path` is unset or not a directory.
- Root cause: `GET /api/sandboxes/{sid}/git` (`TaskBranch`) lists every task with a branch but does
  not say whether its worktree exists, so the page links every card that has a task. A task gets
  its branch when created (`lifecycle.rs`) and its worktree only when it starts (`supervisor.rs`),
  so every queued, chained or prepared task is such a card, and so is a task whose worktree was
  removed. The screenshot's #36 is a task queued after #35 for 17 hours: most likely never
  started, so even the refusal's "worktree is gone" was wrong for it. No test covered either case.
- What else the cause could break: the task overview's "Review changes" button and Changes tile
  link the same way; they are not the Git board and were there before, so they are #945.

## Requirements

- **WTG-001** `TaskBranch` carries `worktree`: `present` (on disk), `not_created` (no worktree
  recorded: the task has not started) or `gone` (recorded, no longer a directory). The changes and
  commits reads refuse (409) exactly when it is not `present`, saying "the task has not started
  yet, so it has no worktree to read changes from" or "the task's worktree is gone; its branch and
  pull request are on GitHub". Older clients ignore it; a missing field reads as `present`.
- **WTG-002** The Git board renders a card whose `worktree` is not `present` without a link to the
  changes view (title as plain text, card not marked clickable), with the line "Worktree gone" or
  "No worktree yet" after the stage line; its pull request chip and Details stay.
- **WTG-003** A card whose worktree exists still opens the changes view.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] The Git overview says when a task's worktree is gone (test-first)
- [x] The Git board card of a gone worktree is plain text saying so (e2e test-first)
- [ ] Independent review answered, gates green, docs updated

## Acceptance criteria

- The Git overview of a task whose worktree was removed has `worktree: "gone"`; of a task not
  started, `"not_created"` (and its changes refusal does not say "gone"); otherwise `"present"`.
- On the Git page, those cards have no link and read "Worktree gone" / "No worktree yet"; other
  cards still link to `/git/t/<N>`.

## Test plan

- Rust integration test (`crates/yulius/tests/delivery.rs`,
  `git_overview_says_whether_a_tasks_worktree_is_there`): a started task before and after its
  worktree directory is removed, and a task with a branch never started, with the refusals.
- e2e (`e2e/tests/git.e2e.ts`) against the mock (`ambient=0`): tasks marked `gone` and
  `not_created` render without a link and with their text; another card still links. The mock
  derives `not_created` for chained and prepared tasks and refuses changes and commits like the
  server.

## Decisions

- The server, not the page, decides "gone", from the same check as `review_target`, so the page
  never offers what the server will refuse. A per-card request to probe the changes was rejected
  (one read per card, and it races the cleanup anyway).
- Three states, not a `worktree_gone` flag (review round 1): a task not started has no worktree
  either, and calling it "gone" is wrong. A typed `WorktreeState` enum (snake_case) with `present`
  as its default keeps an older server's rows linkable. The state comes from the recorded path
  alone: `worktree_path` is set when the task starts and never cleared, so unset means never
  created.
- The title is plain text, not a link to the task: the person asked for it not to be clickable;
  the sidebar and "Details" still reach the task.
