---
yulius-task: 2090d8dd-e645-4fa8-a953-b392fe17dbd3
status: in-progress
---

# SPEC-397: task_delete says not_moved in its docs and when settling stops early

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #387
Scope authorization: the owner's goal for task #387, the two minors SPEC-396's final review left.

## Goal

An agent reading `task_delete` knows its answer can say `not_moved`, and a deletion that leaves
the deleted task's attachment folder where it is never answers as if there were no attachments.

## Context

- SPEC-396 (ADR-0253) settles a deleted task's `attachments/tasks/<id>/` folder: kept while
  named, else moved to `attachments/trash/`. `DeletedAttachments::not_moved`
  (`crates/yulius-app/src/attachment_trash.rs`) is set when the move itself fails; docs/api.md
  documents it, but ATR-003 and the `task_delete` tool description (`crates/yulius-app/src/mcp.rs`)
  do not.
- `App::settle_attachments` returns an empty answer early in three cases: the `attachments`
  folder is not a plain folder (a link, ADR-0174: never followed), the sandbox's tasks cannot be
  read (everything is kept), or there is no `tasks` folder. In the first two the deleted task's
  folder may stay where it is, but the answer reads like "no attachments".

## Requirements

- **NMV-001** `task_delete`'s tool description and SPEC-396's ATR-003 name `not_moved`: the
  attachments stayed where they are because moving them to the trash failed.
- **NMV-002** `task_delete` answers `not_moved: true` when settling left the deleted task's folder
  in place before trying to move it:
  - `attachments` is there but cannot be opened as a plain folder (a link, or an error other than
    "missing" or "not a folder"): what is behind it is never looked into, so the folder may be
    there and nothing moved it;
  - the sandbox's tasks cannot be read and `attachments/tasks/<id>/` is a folder (looked up
    without following a link).
  A missing `attachments` or `tasks` folder, or an `attachments` that is a plain file, still
  answers nothing (no folder can exist).

## Plan

- [x] Spec SPEC-397 written
- [ ] task_delete's tool description and ATR-003 name not_moved
- [ ] A deletion through a linked attachments folder answers not_moved
- [ ] A deletion whose tasks cannot be read answers not_moved
- [ ] Independent review approved and evidence recorded

## Acceptance criteria

- Deleting a task by MCP while `attachments` is a link to a folder outside the sandbox answers
  `not_moved: true`, and nothing outside changes (NMV-002).
- The `task_delete` tool description contains `not_moved` (NMV-001).
- Settling with unreadable tasks and the task's own folder present answers `not_moved: true`;
  without the folder it answers nothing (NMV-002).

## Test plan

- `crates/yulius/tests/attachment_trash.rs`: a deletion while `attachments` is a link (as the
  swapped-folder tests in `uploads.rs` set it up), checking the MCP answer and the outside folder;
  the tool description's `not_moved` checked through `tools/list`.
- A unit test in `attachment_trash.rs` for the unreadable-tasks answer (a store failure is not
  reachable through the HTTP harness without a fault port), on the helper that decides it.
- Each written first and failing without the change.

## Decisions

- A link at `attachments` answers `not_moved` without looking behind it: checking whether the
  task's folder exists there would mean following the link, which ADR-0174 rules out. Saying
  "may still be there" when nothing is there costs an agent one look; saying "nothing" when the
  files are there is the misreading this task fixes.
- An `attachments` that is a plain file, or a missing `tasks` folder, answers nothing: no folder
  of the task can exist there.
- No ADR: this refines the answer of ADR-0253's settling, no new lasting decision.
