---
yulius-task: 4f1be670-bd99-4c4e-bec0-03bc83810322
status: done
---

# SPEC-396: Deleting a task keeps the attachments other tasks name, and a week's trash

Status: done
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
  task's request (prompt with its queued messages) or an automation's prompt contains
  `attachments/tasks/<id>/`. Every task of the sandbox counts, archived ones included.
- **ATR-002** When a task is deleted, every attachment folder of the sandbox whose task no longer
  exists is settled: a named one stays where it is, whole, so the paths in the naming requests keep
  working; any other goes to the sandbox's trash, `attachments/trash/<secs>-<number>-<id>/`
  (deletion time in Unix seconds; number `0` when no longer known), by one rename through folder
  handles that never follows a link. A folder kept earlier goes to the trash once the last task
  naming it is deleted. The folders are listed before the tasks are read, so a task created
  meanwhile is never taken for a deleted one, and each folder's task is asked for again just before
  it moves (one unarchived between the readings); a folder a parallel deletion moved first,
  without its number, gets it back from its own task's settling. The file system work runs off the runtime.
- **ATR-003** `task_delete` answers what happened to the task's attachments: `kept_for` (the
  numbers of the tasks naming them, `kept_for_automations` the automations' names) or `trash` (the entry, its files and until when it can be
  restored), or `not_moved: true` when they stayed where they are because moving them to the trash
  failed or settling stopped before it (SPEC-397). The UI's delete confirmation says that attachments another task names stay and the
  rest wait 7 days in the trash.
- **ATR-004** A trash entry older than 7 days is removed: when a task is deleted, when the trash is
  listed, at start-up and by the workspace reaper (once an hour), at most 100 entries a round.
  Removal never follows a link.
- **ATR-005** `attachments_trash` (task agents and the Superagent) lists the project's trash
  entries (deleted task id and number, deleted at, restorable until, file names) and, with
  `restore` (the deleted task's id or number) and `into` (a task number; a task agent's own task by
  default), moves the entry's regular files into that task's attachments folder under free names
  (`name`, `name-2`, …) and answers their new paths. Restoring needs write access, respects the
  folder bound, and is audited (`attachment.restored`). The entry goes once it is empty: what a
  restore could not move (a folder, a link, a file without a free name) stays in it and is
  reported (`left_in_trash`). Restored names are made safe as an upload's are. Listing creates
  no folder.
- **ATR-006** `task_update` (task agents and the Superagent) changes how a waiting (chained or
  prepared) task starts, as the UI's change does (SPEC-126), with `task_create`'s start arguments
  (`after_task`, `start_rule`, `supertask`, `prepared`, `start_at`; none of them means start now).
  A task that has started is refused, as in the UI. Audited with the caller (MGT-010). Both tools
  are also served to the remote MCP.

## Plan

- [x] Spec and ADR for keeping named attachments and a week's trash
- [x] A deleted task's attachments go to the trash unless another task's request names them
- [x] The trash is purged after 7 days and its files are restored into a task
- [x] task_update changes a waiting task's start in place
- [x] Delete confirmation, tool descriptions and docs say where attachments go
- [x] Independent review approved and findings answered
- [x] Gates green with evidence in the spec
- [x] Final review approves the evidenced head

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
- Requests and automations' prompts are searched for references (review round 1 added the
  automations); messages typed into a running session are kept nowhere Yulius can search (its
  transcript only), and the trash covers those.
- Composer drafts a waiting task names are still pruned after a week, and a shell's attachments go
  with the shell: older paths to the same kind of loss, outside this change (#1023, #1027; fixed by
  SPEC-413).
- A task created after the namers were read whose request names an orphan folder nobody else
  names can still see that folder go to the trash (the fresh question covers only the folder's own
  task); it is restorable, and creating a task naming a deleted task's files in that instant is
  far-fetched (review round 3).
- The tool is named `task_update` (what an agent looks for) and takes only the start for now.

## Evidence

- `scripts/dev ci` at `dfa0d76d` (2026-11-03): exit 0. Repository contracts, script unit tests,
  `cargo check`, `fmt`, `clippy -D warnings` (Linux and `aarch64-apple-darwin`), `nextest`
  **2975 passed, 16 skipped**, doctests, `cargo deny`, `cargo shear`, `cargo doc`. Earlier runs
  failed and were fixed in this branch: clippy `manual_is_multiple_of` (reaper round counter) and
  rustdoc `private_intra_doc_links` (`TrashEntry::files`).
- `cd e2e && npm ci --ignore-scripts && npx e2e run`: **799 passed, 1 failed, 1 skipped**. The
  failure is `filedrop.e2e.ts` "files dropped on a folder of the explorer are added to it" (51:130,
  an un-waited class read in the editor's explorer, code this change does not touch). It passed
  3 of 3 runs of that file alone; filed as #1029.
- `yulius security-scan`: **disabled** ("Vidoc is turned off in Yulius"); nothing was scanned.
  Reported as disabled, not passed.
- Test-first: the new tests in `crates/yulius/tests/attachment_trash.rs` and the changed one in
  `uploads.rs` were run against the product code with each round's source diff reversed
  (`git diff HEAD~1 HEAD -- crates/yulius-app | git apply -R`):
  - first slice: all 5 failed;
  - round 1: `archived_tasks_and_automations_name_attachments_too`, `task_update_…` (audit) and
    `the_trash_keeps_what_it_cannot_restore_…` failed;
  - round 2: `restored_names_need_no_quoting`, `settling_asks_again_before_it_moves_a_folder`,
    `the_trash_keeps_…` (`left_in_trash`) and `task_update_…` (one audit row) failed;
  - round 3: `a_trash_swapped_for_a_link_receives_nothing` failed on the missing `not_moved`.
  - Two tests are regression guards that pass on older code: `a_trash_swapped_for_a_link_…`'s
    no-escape part (the handle walk was already safe) and `listing_an_empty_trash_creates_nothing`
    (guards the round-2 fix).
- Independent review in fresh read-only sessions (skill `yulius-review`), three rounds:
  - round 1 (`e1f8c85c`): changes requested. A blocker (Cockpit tool-list test), a major
    (`task_update` audited as the person) and nine minors, all answered in `af0ade60`.
  - round 2 (`af0ade60`): changes requested. A major (unreported leftovers of a restore) and five
    minors, answered in `8c748fcd`. Not changed, with reasons: remote `attachments_trash` keeps
    the Work scope (restoring must not ride a view-only token), and a second concurrent restore is
    benign.
  - round 3 (`94c39cea`): approved. Its two minors and four nits were answered in `b67c5f8d`.
- Pre-existing paths to the same kind of loss, outside this change: composer drafts pruned after a
  week (#1023), shell attachments removed with the shell (#1027).
- Not tested: a real contained sandbox (bubblewrap) deleting a task. The tests run the HTTP and MCP
  paths against the sandbox folder on the host, as the existing upload tests do.
