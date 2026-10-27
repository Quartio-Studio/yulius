---
yulius-task: 0c8636eb-8928-44b3-890b-4dcae9f6c701
status: in-progress
---

# SPEC-343: a Git board card whose worktree is gone says so and opens nothing

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #327
Scope authorization: task #327: a screenshot of the task changes view of quartio-v2 #36 showing
"Couldn't load the changes: the task's worktree is gone; its branch and pull request are on
GitHub" and "i got this after clicking on clickable element. element shouldnt be clickable but just
say that it's gone". Fixes #943.

## Goal

On the Git page, a task card whose worktree is no longer on disk is not a link and says the
worktree is gone, instead of opening a changes view that can only fail.

## Context

- Symptom: the card in "Where every task is" (`git.js` `card()`) links to `/s/<slug>/git/t/<N>`;
  there `GET /api/tasks/{tid}/changes` answers 409 from `App::review_target` when the task's
  `worktree_path` is unset or not a directory.
- Root cause: `GET /api/sandboxes/{sid}/git` (`TaskBranch`) lists every task with a branch but does
  not say whether its worktree exists, so the page links every card that has a task. No test
  covered a task whose worktree was removed (cleanup, archive or a re-chained task).
- What else the cause could break: the task overview's "Review changes" button links on
  `task.branch` too; it is reached from the task, not from the Git board, and is left as is (the
  changes view already explains the state there).

## Requirements

- **WTG-001** `TaskBranch` carries `worktree_gone`: true when the task's worktree path is unset or
  not a directory (exactly when `review_target` refuses). Older clients ignore it; a missing field
  reads as false.
- **WTG-002** The Git board renders a card with `worktree_gone` without a link to the changes view
  (title as plain text, card not marked clickable) and with the line "Worktree gone" after the
  stage line; its pull request chip and Details stay.
- **WTG-003** A card whose worktree exists still opens the changes view.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] The Git overview says when a task's worktree is gone (test-first)
- [ ] The Git board card of a gone worktree is plain text saying so (e2e test-first)
- [ ] Independent review answered, gates green, docs updated
- [ ] Merged into dev on green CI

## Acceptance criteria

- The Git overview of a task whose worktree was removed has `worktree_gone: true`; with the
  worktree present, `false`.
- On the Git page, that task's card has no link and reads "Worktree gone"; other cards still link
  to `/git/t/<N>`.

## Test plan

- Rust integration test (`crates/yulius/tests/delivery.rs`): the overview before and after the
  worktree directory is removed.
- e2e (`e2e/tests/git.e2e.ts`) against the mock: a task marked `worktree_gone` renders without a
  link and with the text; another card still links.

## Decisions

- The server, not the page, decides "gone", from the same check as `review_target`, so the page
  never offers what the server will refuse. A per-card request to probe the changes was rejected
  (one read per card, and it races the cleanup anyway).
- Field named `worktree_gone` (default false) rather than `worktree: bool` (default true) so a
  missing field from an older server keeps today's behavior without a custom serde default.
- The title is plain text, not a link to the task: the person asked for it not to be clickable;
  the sidebar and "Details" still reach the task.
