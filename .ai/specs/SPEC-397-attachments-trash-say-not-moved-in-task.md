---
yulius-task: 2090d8dd-e645-4fa8-a953-b392fe17dbd3
status: done
---

# SPEC-397: task_delete says not_moved in its docs and when settling stops early

Status: done
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
  attachments stayed where they are because moving them to the trash failed or was not tried
  (NMV-002).
- **NMV-002** `task_delete` answers `not_moved: true` when settling left the deleted task's folder
  in place before trying to move it:
  - `attachments` is there but cannot be opened as a plain folder (a link, or an error other than
    "missing" or "not a folder"): what is behind it is never looked into, so the folder may be
    there and nothing moved it;
  - the sandbox's tasks cannot be read and `attachments/tasks/<id>/` is a folder (looked up
    without following a link);
  - `attachments/tasks` cannot be opened but is there as a link (the same reasoning as a linked
    `attachments`) or a folder (out of descriptors, no permission).
  A missing `attachments` or `tasks`, or either being a plain file, still answers nothing (no
  folder can exist).

## Plan

- [x] Spec SPEC-397 written
- [x] task_delete's tool description and ATR-003 name not_moved
- [x] A deletion through a linked attachments folder answers not_moved
- [x] A deletion whose tasks cannot be read answers not_moved
- [x] Independent review approved and evidence recorded

## Acceptance criteria

- Deleting a task by MCP while `attachments` is a link to a folder outside the sandbox answers
  `not_moved: true`, and nothing outside changes (NMV-002).
- The `task_delete` tool description contains `not_moved` (NMV-001).
- Settling with unreadable tasks and the task's own folder present answers `not_moved: true`;
  without the folder it answers nothing (NMV-002).

## Test plan

- `crates/yulius/tests/attachment_trash.rs`: a deletion while `attachments` or `tasks` is a link
  (as the swapped-folder tests in `uploads.rs` set it up), checking the MCP answer, the outside
  folder and an old trash entry behind the link (never purged); a deletion with the sandbox's
  tasks unreadable (the `automations` table renamed away through rusqlite), with and without the
  task's folder.
- Unit tests: `left_in_place` in `attachment_trash.rs` (folder, file, link, missing; a `tasks`
  that could not be opened), and the `task_delete` description naming every answer field in
  `mcp.rs` (`tools/list` serves `tool_descriptors()` unchanged).
- Each written first and failing without the change.

## Decisions

- A link at `attachments` answers `not_moved` without looking behind it: checking whether the
  task's folder exists there would mean following the link, which ADR-0174 rules out. Saying
  "may still be there" when nothing is there costs an agent one look; saying "nothing" when the
  files are there is the misreading this task fixes.
- An `attachments` that is a plain file, or a missing `tasks` folder, answers nothing: no folder
  of the task can exist there. A link opened with `O_NOFOLLOW | O_DIRECTORY` fails with `ENOTDIR`
  as a plain file does, so the two are told apart by an `lstat` (`kind_at`) of `attachments`.
- No ADR: this refines the answer of ADR-0253's settling, no new lasting decision.

## Evidence

- `scripts/dev ci` at `0a95c5c4` (2026-11-03): exit 0. Repository contracts, script unit tests,
  `cargo check`, `fmt`, `clippy -D warnings` (Linux and `aarch64-apple-darwin`), `nextest`
  **2984 passed, 16 skipped**, doctests, `cargo deny`, `cargo shear`, `cargo doc`. The first run
  (at `b893180f`) failed on `cargo fmt --check` of the new integration test, fixed in `0a95c5c4`.
- After merging `origin/dev` (`306a5f95`, #1020; clean, `docs/changes.md` checked):
  `cargo test -p yulius --test main -- attachment_trash uploads import_server`: 39 passed.
- `yulius security-scan`: **disabled** ("Vidoc is turned off in Yulius"); nothing was scanned.
  Reported as disabled, not passed.
- Test-first:
  - `mcp::tests::task_delete_names_every_field_of_its_answer` failed (`not_moved` is not named)
    before the description changed;
  - `a_deletion_through_a_linked_attachments_folder_says_not_moved` failed (`{}` instead of
    `{"not_moved": true}`) on the parent's source (`git apply -R` of the source diff), and while
    a link was still taken for a plain file (`ENOTDIR`), which led to the `kind_at` check;
  - `a_deletion_with_unreadable_tasks_says_not_moved_when_the_folder_stayed` failed with the
    unreadable-tasks branch put back to an empty answer;
  - `settling_stopped_early_says_whether_the_folder_stayed` (unit, `left_in_place`) was written
    with the helper.
- Independent review in a fresh read-only session (skill `om-code-review`), two rounds:
  - round 1 (`b893180f`): approved with four minors and three nits (description narrower than the
    behaviour, an unopenable `tasks` folder answering nothing, no end-to-end test of the
    unreadable-tasks branch, the test plan's wording; two always-true assertions, a stale field
    comment, purge behind the link unchecked), all answered in `0a95c5c4`; one pre-existing
    finding (the 10,000-entry listing cap can hide the task's folder) filed as #1031.
  - round 2 (`0a95c5c4`): approved. Its doc nit (api.md and changes.md not naming an unopenable
    `tasks` folder) is answered here; its two test nits (an extra count of the outside trash, a
    variable name) are left: the checks in place already prove nothing moved behind the link.
- Not tested: a real contained sandbox (bubblewrap) planting the link; the tests plant it in the
  sandbox folder on the host, as the existing swapped-folder tests do.
