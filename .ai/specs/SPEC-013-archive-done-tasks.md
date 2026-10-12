---
yulius-task: e2a485d6-ea7b-4727-b43e-284366ca0ad5
status: done
---

# SPEC-013: a Done task can be archived

Status: implemented
Date: 2026-10-12
Owner: Yulius task #11
Scope authorization: user request, with a screenshot: "tasks that seem finished by their icon
are not archivable, fix".

## Goal

A task the sidebar shows as Done (a green check) can be archived from its menu. Today the menu
says "Stop the task before archiving", although the task looks finished and there is nothing to
stop in the user's eyes.

## Context

- TSK-014: a task is Done when the agent's goal turn ended cleanly, the goal is recorded as
  achieved and no background shell is left. Its status is still `running` (the session stays
  attached) with `activity = done`.
- UI-006 archived only resting tasks (`closed`, stopped, failed), in both
  `taskActions` (`web/assets/status.js`) and `App::archive_task` (`crates/yulius-app/src/tasks.rs`),
  so Done tasks were refused as live.
- Stopping a task is `App::stop_task_now`; archiving only sets `archived_at`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ARC-001 | Archiving a task that is running, Done and has no background shell succeeds; the session is stopped as part of it. | `task_done.rs::a_done_task_can_be_archived` |
| ARC-002 | Every other live task (working, your turn, needs action, background shell, starting, queued) is still refused with Conflict. | `archive_task` guard |
| ARC-003 | The task menu enables Archive for exactly the tasks ARC-001 accepts, and the web mock follows the same rule. | `status.js` `taskActions`, `mock.js` |

## Design

`archive_task` takes `self: &Arc<Self>`; when the task is Done it calls `stop_task_now` first,
then sets `archived_at` as before. Stopping before archiving keeps no hidden live session behind
an archived task. The UI rule uses the displayed status (`Done` label), which already accounts
for background shells.

## Failure and compatibility

If the stop fails, the error is returned and the task is not archived. The task's archived
status then reads Stopped rather than Done. No contract or storage change.

## Acceptance criteria

- A Done task archives from the menu and leaves the sidebar; its session ends.
- A task waiting for the user or with a background shell still shows "Stop the task before archiving".

## Test plan

- `cargo test -p yulius --test task_done` (five tests, including the new one).

## Plan

- [x] Reproduce: Done task refused by the UI rule and the backend guard
- [x] Allow archiving Done tasks (backend stop-then-archive, menu rule, mock) with a regression test

## Decisions

- Stop-then-archive rather than archiving a live session: nothing is left running unseen.
- Delete permanently for Done tasks is out of scope; the request was about archiving.
- "Your turn" tasks stay non-archivable: they wait for the user, so they are not finished.

## Evidence

- `cargo test -p yulius --test task_done`: 5 passed.
- `cargo check -p yulius-app -p yulius-server` clean.
