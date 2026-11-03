---
yulius-task: 04f3e36b-6a99-4183-bb3c-4cb9ac3d5b45
status: in-progress
---

# SPEC-413: A shell's and a composer's attachments stay while a request names them

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #395
Scope authorization: the owner's goal for task #395 (fix #1027, #1023 and #1031).

## Goal

No attachment a task's request or an automation's prompt names is removed: not a shell's when the
shell ends, not a composer draft after a week, and `task_delete` never answers "no attachments"
for a deleted task whose folder is there.

## Context

- SPEC-396 (ADR-0253) keeps a deleted task's `attachments/tasks/<id>/` folder while a remaining
  task's request (prompt with its queued messages, archived tasks included) or an automation's
  prompt names it (`App::namers`, `crates/yulius-app/src/attachment_trash.rs`). Two older paths
  remove files without asking (`crates/yulius-app/src/uploads.rs`):
  - `App::remove_shell_attachments` removes `attachments/shells/<shell>/` when the shell ends
    (`finish_shell`), and `App::prune_shell_attachments` removes the folders of shells that are
    gone at start-up (#1027). A screenshot pasted into the Superagent that it names in a
    `task_create` prompt is gone once the Superagent's shell ends.
  - `App::prune_old_drafts` removes every regular file of `attachments/drafts/<user>/` older than
    7 days at that person's next composer upload (#1023). A prepared task (`start_at` up to 366
    days ahead) or a chained one waiting more than a week loses the files its request names.
- `App::settle_attachments` lists `attachments/tasks/` with `names_in(…, MAX_PRUNED_ENTRIES)`
  (10,000, readdir order). With more entries the deleted task's folder may not be listed: it is
  neither kept nor moved and `task_delete` answers `{}` (#1031).

## Requirements

- **ANR-001** A shell's attachment folder is *named* when a task's request or an automation's
  prompt (SPEC-396 ATR-001's texts, read by the same `App::namers`) contains
  `attachments/shells/<shell>/`. When the shell ends, and at start-up for shells that are gone, a
  named folder stays where it is, whole; an unnamed one is removed as before. A kept folder is
  asked about again at each start-up and removed once nothing names it. When the tasks or
  automations cannot be read, the folder stays.
- **ANR-002** A composer draft older than 7 days is pruned only when no task's request or
  automation's prompt contains `attachments/drafts/<user>/<file>`: at its owner's next upload, and
  by the leftovers round (SPEC-215 `old-draft`, listed and checked again before it deletes). The
  texts are read only when there is an old draft; when they cannot be read, nothing is pruned. The
  drafts folder's bound (`MAX_ATTACHMENTS_PER_FOLDER`, 500) counts the drafts that are not older
  than a week (those still waiting for a task), so kept ones never block the next drop.
- **ANR-003** Settling a deleted task (ATR-002) always considers the deleted task's own folder:
  when it is not among the listed names it is looked up directly in `attachments/tasks/` (an
  `lstat`, never following a link) and, if it is a folder, settled like a listed one, so
  `task_delete` answers `kept_for`, `trash` or `not_moved`, never `{}`, while the folder is there.
- **ANR-004** Every walk stays handle-based and never follows a link (ADR-0174, #660); the file
  system work and store reads keep their bounds (`MAX_PRUNED_ENTRIES`).

## Plan

- [x] Spec and ADR for naming shell and draft attachments
- [x] Named shell attachments survive the shell's end and the start-up prune
- [x] Old drafts a request names survive the next upload
- [x] task_delete finds its folder past the listing cap
- [x] Docs and changes say what stays
- [ ] Independent review approved and findings answered
- [ ] Gates green with evidence in the spec

## Acceptance criteria

- A shell whose folder a task's request names (`attachments/shells/<id>/data.csv`) is closed: the
  file is still readable at that path; another shell nobody names loses its folder; at the next
  start-up an orphan shell folder a request names stays and an unnamed one is removed (ANR-001).
- A draft older than 7 days that a task's request names survives its owner's next upload and the
  leftovers round with its content; an equally old unnamed one is removed; 500 kept old drafts
  do not refuse the next drop, 500 fresh ones do (ANR-002).
- With more than `MAX_PRUNED_ENTRIES` entries in `attachments/tasks/`, deleting a task whose
  folder is there answers `trash` and moves it (ANR-003); the listing helper with a cap of 0 still
  yields the deleted task's folder, and never a link (ANR-003, ANR-004).

## Test plan

- `crates/yulius/tests/uploads.rs`: extend the shell test (a named shell folder survives the close;
  the start-up prune keeps a named orphan and removes an unnamed one), the drafts tests (an old
  named draft survives, an old unnamed one goes; unreadable requests keep everything; the bound
  counts waiting drafts only).
- `crates/yulius/tests/leftovers.rs`: an old named draft is no `old-draft` leftover, one named
  between the scan and the deletion stays, an unnamed one goes.
- `crates/yulius/tests/attachment_trash.rs`: a deletion with more than 10,000 planted folders in
  `attachments/tasks/` answers `trash`.
- Unit test in `attachment_trash.rs`: the listing of orphan folders with a cap of 0 yields the
  deleted task's folder when it is a folder, and not when it is a link or a file.
- Each written first and failing without the change (the 10,000-folder test may pass by chance of
  readdir order; the unit test is the deterministic one).

## Decisions

- [ADR-0259](../../docs/decisions/0259-shell-and-draft-attachments-stay-while-a-request-names-them.md):
  the SPEC-396 naming rule covers shell folders and composer drafts; a named one stays in place
  (no request is rewritten, no file copied into a task), an unnamed one goes as before.
- An unnamed shell folder is removed, not moved to the trash: it is the shell's scratch space and
  nothing names it; the trash is keyed by a deleted task (`<secs>-<number>-<task>`).
- Matching is a substring of the path, with the trailing `/` for a folder and the file name for a
  draft. A draft whose name is a prefix of a named one (`shot` of `shot-2`) is kept too: an extra
  week on disk, never a loss.
- A kept shell folder is asked about again only at start-up (the existing prune); it holds at most
  500 files and counts toward the sandbox's quota.
- Named drafts are no longer bounded by the week: the folder's bound counts only drafts younger
  than a week (review round 1). Rejected: protecting drafts only for tasks that have not ended
  (a restarted task, or another task copying the paths, would lose them, as ADR-0253 counts
  archived tasks), and leaving the bound as it was (an active person would hit 409 within weeks).
  Old named drafts count toward the sandbox's quota.
- Residual race, as SPEC-396 records for its own: a task created between reading the texts and
  removing an unnamed shell folder or draft can still see it go; the window is one store read.
- Pre-existing, outside this change (review round 1): the lookup of a trash entry a parallel
  deletion moved without its number scans at most 10,000 trash names; file system work of these
  clean-ups still runs on the async threads, as before (bounded).
- One mechanism: the namers SPEC-396 reads at deletions, no reference table.

## Evidence

Pending.
