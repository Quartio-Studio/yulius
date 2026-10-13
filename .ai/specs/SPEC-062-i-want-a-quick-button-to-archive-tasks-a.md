---
yulius-task: 9e8d2778-3e0c-4956-81cf-8e8b7cf2bc1a
status: in-progress
---

# SPEC-062: quick Archive on a sidebar row and "Archive all" for Done

Status: in progress
Date: 2026-10-13
Owner: Yulius task #65
Scope authorization: user request (task #65): "i want a quick button to archive tasks and
'archive all' for done", with a screenshot of the sidebar's Done group (13 tasks, each a row
with a pull request chip, a title and an age; the hovered row shows the `⋮` task menu button in
place of the age).

## Goal

Clearing finished work out of the sidebar takes one click per task, or one click for the whole
Done group, instead of opening each task's menu and picking Archive.

## Context

- Archiving exists (UI-006, SPEC-013, TSK-014): `POST /api/tasks/{tid}/archive`, offered as
  "Archive" in the task menu (`openTaskMenu` in `web/assets/actions.js`) when
  `taskActions(...).archive` holds: write access and a task that is ended (stopped, closed,
  failed) or Done by the one rule of `taskInfo`. A Done task's session is closed as it is
  archived. `archiveTask` removes the row, offers Undo (restore) in its toast and leaves the
  task's page if it is open.
- The sidebar (`layout.js`) groups active tasks: Needs you, Working, Done, Stopped. A row is one
  line (state mark, pull request chip, title, age); on hover or focus the `⋮` menu button takes
  the age's place. Phones show the `⋮` button all the time.
- The Done group's head is a plain band (icon, name, count); only Stopped's head is a button
  (collapse).
- There is no bulk endpoint; each archive is checked by the server on its own.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| QAR-001 | A sidebar row whose task can be archived (`taskActions(...).archive`) has an "Archive task #n" button beside its `⋮` button, shown like it on hover and keyboard focus. One click archives the task as the menu's Archive does (toast with Undo, the row leaves, the task page is left if open). A row that cannot be archived (working, needs you, read access) has no such button. | e2e `archive.e2e.ts` |
| QAR-002 | The Done group's head has an "Archive all" button when at least one of its tasks can be archived. One click archives every archivable Done task shown, without a confirmation: the requests run at most 4 at a time, the rows leave as they succeed, and one toast says "Archived N Done tasks." with one Undo that restores them all. The button is disabled while it runs. Other groups have no such button. | e2e `archive.e2e.ts` |
| QAR-003 | When some archives of "Archive all" fail, the others stay archived and one error toast names how many failed and the first reason ("Couldn’t archive 1 of 2 tasks: …"); no toast per task. | e2e `archive.e2e.ts` |
| QAR-004 | Phones (≤ 720 px, the drawer) keep one row action: the quick Archive button is not shown there (the `⋮` menu has Archive); "Archive all" is shown. | e2e `archive.e2e.ts` |
| QAR-005 | SPEC-001 UI-006, an ADR, `docs/changes.md` and `e2e/README.md` describe it. | review |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] A sidebar row's quick Archive button archives that task with Undo
- [ ] "Archive all" on the Done group archives every Done task with one Undo
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-QAR-1: in the mock, hovering Done task #13's row shows "Archive task #13"; clicking it shows
  "Task #13 archived." and the row leaves the Done group; Undo brings it back. Working task #12's
  row has no Archive button.
- AC-QAR-2: the Done group's "Archive all" archives #13 and #11 in one click: "Archived 2 Done
  tasks.", the Done group is gone, the mock records both archived; Undo restores both. The
  sidebar has no WCAG violations with the buttons shown.
- AC-QAR-3: with the archive of #11 failing (mock `&fail=`), "Archive all" archives #13, keeps
  #11 in Done and says "Couldn’t archive 1 of 2 tasks".
- AC-QAR-4: at 390 px the row has no quick Archive button; "Archive all" is visible.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/archive.e2e.ts` (mock mode): AC-QAR-1 to AC-QAR-4, each red before its slice.
- `ui_contract.rs` already checks the literal API paths the UI calls (no new path).
- No server change: the archive endpoint and its rules are covered by `task_done.rs` and
  `task_delete.rs`.

## Decisions

- "Archive all" is the client running the existing per-task endpoint, at most 4 at a time, not a
  new bulk endpoint: the server keeps checking each task on its own (a Done task is closed, a
  live one refused), Done is decided by the same `taskInfo` rule the person sees (it needs the
  pull request's checks, which the client already holds), and a partial failure is reported per
  task. See [ADR-0040](../../docs/decisions/0040-archive-all-runs-the-per-task-archive.md).
- No confirmation for "Archive all": archiving is reversible (UI-006 keeps it one click), and
  the toast's Undo restores the whole batch. Delete stays in the menu with its confirmation.
- The quick button is shown for every archivable row (Done and Stopped), not only Done: the
  request asks for a quick archive of tasks in general, and Stopped rows are archivable too.
- On phones the quick Archive button is left out: the row already gives a 48 px target to `⋮`,
  and a second one would leave too little of the title; Archive stays one tap away in the menu.

## Evidence

(filled when done)
