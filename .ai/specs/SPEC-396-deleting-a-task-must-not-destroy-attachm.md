---
yulius-task: 4f1be670-bd99-4c4e-bec0-03bc83810322
status: in-progress
---

# SPEC-396: Deleting a task keeps the attachments other tasks name, and a week's trash

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #386
Scope authorization: the owner's goal for task #386 (data loss in the quartio-v2 sandbox on
2026-11-02): deleting a task must not silently remove files another task references, deleted
attachments are restorable for a while, and a waiting task's start changes without recreating it.

## Goal

Deleting a task never removes an attachment that another task's request still names, the other
attachments of a deleted task can be restored for 7 days, and an agent or the Superagent changes
how a waiting task starts in place instead of deleting and recreating it.

## Context

- Prepared task #61 of quartio-v2 had screenshots in `attachments/tasks/<#61>/` (pasted in its
  request and its terminal). To chain it after another task the Superagent created #63 with the
  same prompt (which names the files by their absolute paths) and deleted #61; `delete_task_as`
  called `remove_task_attachments`, which removes the folder at once (TSK-009,
  `crates/yulius-app/src/uploads.rs`). #63 now names four files that are gone.
- The web UI already changes a waiting task's start in place (SPEC-126,
  `PUT /api/tasks/{tid}/start`, `App::change_task_start`), but no MCP tool does: an agent or the
  Superagent had only delete-and-recreate.
- Queued messages (SPEC-052) are added to the request (`task.prompt`), so a request covers both
  the prompt and the messages a task got before it started. Text typed into a running session is
  stored nowhere but its transcript.
- Attachment folders are inside the sandbox directory, which a contained process can change;
  every walk goes through handles without following links (ADR-0174, #660).

## Requirements

- **ATR-001** A task's attachment folder `attachments/tasks/<id>/` is *named* by a task when the
  task's request (prompt with its queued messages) contains `attachments/tasks/<id>/`. Every task
  of the sandbox counts, archived ones included.
- **ATR-002** When a task is deleted, every attachment folder of the sandbox whose task no longer
  exists is settled: a named one stays where it is, whole, so the paths in the naming requests keep
  working; any other goes to the sandbox's trash, `attachments/trash/<secs>-<number>-<id>/`
  (deletion time in Unix seconds; number `0` when no longer known), by one rename through folder
  handles that never follows a link. A folder kept earlier goes to the trash once the last task
  naming it is deleted.
- **ATR-003** `task_delete` answers what happened to the task's attachments: `kept_for` (the
  numbers of the tasks naming them) or `trash` (the entry, its files and until when it can be
  restored). The UI's delete confirmation says that attachments another task names stay and the
  rest wait 7 days in the trash.
- **ATR-004** A trash entry older than 7 days is removed: when a task is deleted, when the trash is
  listed, at start-up and by the workspace reaper (at most once an hour). Removal never follows a
  link.
- **ATR-005** `attachments_trash` (task agents and the Superagent) lists the project's trash
  entries (deleted task id and number, deleted at, restorable until, file names) and, with
  `restore` (the deleted task's id or number) and `into` (a task number; a task agent's own task by
  default), moves the entry's regular files into that task's attachments folder under free names
  (`name`, `name-2`, …) and answers their new paths. Restoring needs write access, respects the
  folder bound, and is audited (`attachment.restored`).
- **ATR-006** `task_update` (task agents and the Superagent) changes how a waiting (chained or
  prepared) task starts, as the UI's change does (SPEC-126), with `task_create`'s start arguments
  (`after_task`, `start_rule`, `supertask`, `prepared`, `start_at`; none of them means start now).
  A task that has started is refused, as in the UI.

## Plan

- [x] Spec and ADR for keeping named attachments and a week's trash
- [ ] A deleted task's attachments go to the trash unless another task's request names them
- [ ] The trash is purged after 7 days and its files are restored into a task
- [ ] task_update changes a waiting task's start in place
- [ ] Delete confirmation, tool descriptions and docs say where attachments go
- [ ] Independent review approved and findings answered
- [ ] Gates green with evidence in the spec

## Acceptance criteria

- Deleting #61 while #63's request names `attachments/tasks/<#61>/x.png` keeps the file at that
  path, and `task_delete` answers `kept_for: [63]`; deleting #63 afterwards moves #61's folder to
  the trash (ATR-001, ATR-002, ATR-003).
- Deleting a task nobody names moves its folder to `attachments/trash/`, and its file can be read
  there; `attachments_trash` lists it and restores it into another task, whose folder then holds
  the file under a free name (ATR-002, ATR-005).
- An entry older than 7 days is gone after the next listing or deletion; a younger one stays
  (ATR-004).
- A folder swapped for a link outside the sandbox when the deletion settles moves or removes
  nothing outside (ATR-002, ATR-004).
- `task_update` with `after_task` turns a prepared task into a chained one in place (same id and
  number); on a running task it is refused (ATR-006).

## Test plan

Integration tests in `crates/yulius/tests/` through the HTTP API and the MCP endpoint, as the
existing upload and management tests do: the kept, trashed, purged and restored cases above, the
swapped-folder case (the existing `AttachmentsRemove` race point), restore refusals (unknown entry,
a read-only person), and `task_update` on a prepared and on a running task. Each test is written
first and fails without the change.

## Decisions

- [ADR-0253](../../docs/decisions/0253-a-deleted-tasks-attachments-stay-while-named-and-wait-a-week-in-the-trash.md):
  named folders stay in place (paths keep working, no request is rewritten); the rest go to a trash
  inside the sandbox's attachments folder, kept 7 days; restoring moves files into a live task.
- `task_create` does not copy the attachments its prompt names: keeping named folders in place
  already protects the files, and copying would mean rewriting the prompt's paths.
- Only requests are searched for references: messages typed into a running session are kept
  nowhere Yulius can search (its transcript only); the trash covers those.
- The tool is named `task_update` (what an agent looks for) and takes only the start for now.

## Evidence

To be filled in when the gates run.
