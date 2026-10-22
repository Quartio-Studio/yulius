---
yulius-task: 9685b367-0bc4-4b5f-b5d1-88d73e747c14
status: done
---

# SPEC-265: Editor delete, folder create and listing go through folder handles (#660)

Status: done
Date: 2026-10-22
Owner: Yulius task #251
Scope authorization: task #251, started by the nightly "Work through open issues" run: solve
issue #660 in one pull request into `dev`. Fixes #660. Completes
[ADR-0174](../../docs/decisions/0174-host-access-to-agent-owned-folders-goes-through-handles.md)
and [SPEC-220](SPEC-220-fix-657-host-writes-into-agent-owned-fol.md) for the operations it left
over.

## Goal

The editor's delete, folder creation and listing, and the removal of task, shell and draft
attachments, act through folder handles opened from the sandbox directory without following
links. A contained shell (root in its sandbox) that swaps a folder on the way for a link to a host
folder between the server's check and its action makes that one action fail; nothing outside the
sandbox is deleted, created or listed.

## Context

- Symptom (#660, reproduced by task #206 with a throwaway pause point): swapping `repo/docs` for a
  link to a host folder between `file_delete`'s parent check and its `remove_file` made
  `DELETE /api/sandboxes/s1/files?path=docs/a.txt` delete the host's `a.txt`.
- Root cause: in `crates/yulius-app/src/files.rs`, `file_delete` and `file_create` (`kind: dir`)
  canonicalize the parent and check it is inside the root, then act by path
  (`remove_dir_all`/`remove_file`/`create_dir`); `files_list` canonicalizes the folder
  (`existing`), then `read_dir`s it by path and `symlink_metadata`s each entry by path. In
  `uploads.rs`, `remove_attachments` and `prune_old_drafts` test the folders with
  `plain_dirs_exist` (a `symlink_metadata` walk), then `remove_dir_all`/`read_dir`/`remove_file`
  by path; `prune_shell_attachments` lists `attachments/shells` the same way. Any path resolved
  after the check follows whatever link the agent put there meanwhile.
- Why tests missed it: SPEC-220 added pause points (`yulius_app::race`) and race tests only for
  reads, saves, uploads, imports and skill copies; these operations had no pause point, and their
  link tests plant the link before the request (caught by the check), never between check and use.
- Existing helpers: `files::open_dirs_below` (walk from an anchor with `O_DIRECTORY | O_NOFOLLOW`),
  `files::open_dir_at`, `files::open_inside` (checks the handle's real path through `/proc/self/fd`
  or `F_GETPATH`), and `remove_tree` in `yulius-harness/src/skills.rs` (private to that crate,
  which `yulius-app` does not depend on).
- Every editor root (the sandbox repository or a task worktree) is checked to lie inside the
  canonical sandbox directory, the one folder the agent cannot replace (its mount point).

## Requirements

- EDH-001: `file_delete` opens the entry's parent folder from the canonical sandbox directory one
  component at a time with `O_DIRECTORY | O_NOFOLLOW` (the components of its resolved path, which
  is checked inside the root and out of `.git` first) and removes the entry relative to that
  handle: a file or a link with `unlinkat`, a folder with a handle-based tree removal in which
  every sub-folder is opened without following links and a link is unlinked, never followed. A
  folder on the way that is a link by the time it is opened fails the delete with `403` ("the path
  leads outside the folder"); a missing entry is `404`.
- EDH-002: `file_create` with `kind: dir` opens the parent the same way and creates the folder with
  `mkdirat` on the handle; an existing name is `409`, a swapped parent `403`, and nothing is
  created outside.
- EDH-003: `files_list` opens the folder the same way and lists it through the handle; each
  entry's kind and size come from `fstatat(AT_SYMLINK_NOFOLLOW)` on the handle. A folder swapped
  for a link is refused with `403` and no host name is returned. Links inside the folder are still
  shown as their target's kind when they resolve inside the root (unchanged).
- EDH-004: Removing a task's or a shell's attachments opens `attachments/<tasks|shells>` from the
  sandbox directory without following links and removes the owner's folder with the handle-based
  tree removal; pruning old drafts opens `attachments/drafts/<user>` the same way and unlinks old
  regular files through the handle; pruning ended shells' folders lists `attachments/shells`
  through its handle. A link anywhere on the way removes nothing (logged, as before).
- EDH-005: The tree removal is bounded: at most 256 folders deep and 1,000,000 entries per call,
  holding at most 512 names at a time; past a bound it stops with an error and leaves the rest. It
  opens only entries `fstatat` reports as folders, reads each folder's listing once while removing
  its entries (linear), then again from the start until it is empty.
- EDH-008: A file where a folder of the path should be (`docs/a.txt/new`) is `422` ("that path is
  not a directory"), not `403`.
- EDH-006: Correctness needs no `/proc`: the walk uses only `openat`, `mkdirat`, `fstatat`,
  `unlinkat` and `fdopendir`, and builds on Linux and macOS (`scripts/dev macos`).
- EDH-007: Test-only pause points `FilesList`, `FileCreate`, `FileDelete`, `AttachmentsRemove`
  and `DraftsPrune` sit between each operation's check and its use.

## Plan

- [x] Spec SPEC-265 written and validated
- [x] Pause points and failing race tests for delete, create, list and attachment removal
- [x] Editor delete, folder create and listing go through handles from the sandbox directory
- [x] Attachment, draft and shell-folder removal go through handles
- [x] Docs (ADR-0174 note, architecture, changes) updated and gates green
- [x] Independent review answered and PR merged into dev on green CI

## Acceptance criteria

- AC-1: A delete of `docs/a.txt` (and of a folder `docs/sub`) whose parent `docs` becomes a link
  to a host folder at the `FileDelete` pause point leaves the host folder untouched and answers
  `403`.
- AC-2: Creating `docs/new` while `docs` is swapped for a link at `FileCreate` creates nothing in
  the host folder and answers `403`.
- AC-3: Listing `docs` while it is swapped for a link at `FilesList` is refused with `403` and
  returns no host name.
- AC-4: Deleting a task whose sandbox's `attachments` folder becomes a link to a host folder at
  `AttachmentsRemove` removes nothing there; a draft upload whose `attachments` folder is swapped
  at `DraftsPrune` removes no old host file.
- AC-5: A tree removal unlinks a link found inside the tree without touching its target.
- AC-6: `scripts/dev ci` (which includes `scripts/dev macos`) passes.

## Test plan

- Integration (`crates/yulius/tests/files.rs`): pause-point tests for AC-1 (file and folder),
  AC-2 and AC-3, each swapping `repo/docs` for a link to `data/host` exactly at the pause point;
  each fails on the old code.
- Integration (`crates/yulius/tests/uploads.rs`): pause-point tests for AC-4 (task deletion and
  draft pruning), failing on the old code.
- Unit (`files.rs`): the tree removal removes nested folders and files and unlinks an inner link
  to an outside folder, leaving the outside intact (AC-5).
- Existing editor and attachment tests keep passing (deletes, folder creation, listing order,
  `.git` hidden, link kinds, attachments going with their task or shell, draft pruning).

## Decisions

- Anchor at the sandbox directory, not the editor root: the editor root (`repo` or a worktree) is
  itself inside the agent's reach. Walking from the sandbox directory over the canonical path's
  components needs no `/proc` (EDH-006); `open_inside` (real-path check) would, on Linux. The
  canonical path has no link in it at check time, so any link met by the walk was put there after
  the check and is refused. Links inside the root stay followed for reads where the editor
  resolves them by design (the resolved path is what is walked).
- A folder swapped for another real folder (not a link) is still inside the sandbox: the action
  lands in the agent's own folder, which the agent could change itself.
- A shared tree removal in `files.rs` rather than reusing `yulius-harness`'s private `remove_tree`:
  `yulius-app` does not depend on the harness and that one refuses folders of more than 1024
  entries (fine for skills, not for an editor delete of `node_modules`); this one removes in
  batches under explicit bounds (EDH-005). The harness's copy stays as is (out of scope).
- The removal runs off the runtime's threads (`projects::blocking`), as a large tree takes long.
- No ADR: this applies ADR-0174's decision; its Consequences line naming #660 is updated.

## Evidence

- Red first (commit 1829b6ec, pause points in the old code): the five race tests failed for the
  reported reasons: the delete removed the host's `a.txt`; the folder create made `host/new`; the
  listing returned `host-only.txt`; draft pruning removed the host's `old.png`; task deletion
  removed the host's `tasks/t1`.
- Green (c7421c64, b775d5bf): `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/files::|uploads::/)'`: 25 passed; `-p yulius-app` files/uploads unit tests (with
  `a_tree_removal_unlinks_links_and_never_follows_them`): 14 passed; `task_prompt`, shell and
  attachment tests: 39 passed. The review-round test
  `a_file_on_the_way_is_not_a_folder_to_create_in_or_delete_from` failed (403) before b775d5bf.
- `scripts/dev ci` at 18eb723c: exit 0 (2410 tests passed, 17 skipped; clippy for
  `aarch64-apple-darwin` included, so it builds for macOS); rerun after the review fixes, see the
  pull request.
- `scripts/dev security` (Vidoc vidoc-1) at 18eb723c: exit 0, no critical, high or medium finding
  in the change. One low in the change, not fixed: `files_list` still classifies a link entry's
  kind with `canonicalize(dir.join(name))` (as before this change). It only labels the entry
  (`dir`/`file`/`symlink`); names, sizes and the set of entries come from the handle, a target
  resolved through a swapped folder is outside the root and shows as `symlink`, and no action
  follows from the label (the reviewer agreed it is harmless). One medium outside the change
  (`uploads.rs:216` `check_components` accepts `.`/`..`; editor uploads also run
  `relative_components`, which refuses them): pre-existing, Yulius CI files it.
- Independent review (fresh session, om-code-review) of 18eb723c: APPROVE with low/nit findings,
  all own code, answered in b775d5bf:
  1. quadratic re-reading of large flat folders: fixed (one listing read on, then re-read until
     empty);
  2. every entry opened to learn its type (FIFO/permission concerns on macOS): fixed (`fstatat`
     first, only folders opened);
  3. task-attachment race test could pass without the swap: fixed (asserts the folder is a link
     afterwards);
  4. link kind by path: kept, as above;
  5. a file on the way answered 403: fixed (422, test first);
  6. a vanished entry between listing and `fstatat` can hide truncation: kept; the window is an
     entry deleted mid-listing of a folder with more than 2000 entries, and the next listing is
     right;
  7. spec unfinished: done here.
- Not tested here: a real macOS run (only compiled and linted for macOS).
