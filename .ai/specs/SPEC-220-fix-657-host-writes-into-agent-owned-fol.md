---
yulius-task: a51924d3-bee6-4035-8817-fe125fea03f1
status: done
---

# SPEC-220: Host writes into agent-owned folders do not follow links

Status: done
Date: 2026-10-18
Owner: task #206
Scope authorization: task #206: "Fix GitHub issue #657 … four places where the server, as the
service user, follows links or races path checks in folders a contained agent can write". Fixes
#657 (with the write side of the editor added to it in its comment). Same class as #634 and #649.
Decision: ADR-0174.

## Goal

Yulius runs outside the agent's containment as the service user. Wherever it writes into, or reads
from, a folder a contained agent (or a contained shell) can change, it reaches that folder through
a handle the agent cannot redirect, so a link the agent planted, or swapped in between a check and
the use, never makes the server read, create, truncate, replace or remove a file outside the
sandbox.

## Context

Found by task #160's Vidoc audit (SPEC-217) and confirmed against `dev` at 7b7ca3f3. A sandbox's
whole directory (`sandboxes/<sid>/`: `repo`, `worktrees`, `home`, `attachments`) is mounted
read-write into its runs and shells, so the agent can rename, replace or link any folder inside it,
at any moment. Only the sandbox directory itself (the mount point) is out of its reach.

The places, from the code on `dev` (34944a6d):

1. **Imported transcripts** (`yulius-app/src/imports.rs`, `prepare_imported_resume` →
   `rewrite_transcript`, `copy_claude_aux`). The transcript goes to
   `<sandbox>/home/.claude/projects/<slug>/<session>.jsonl`: `create_dir_all`, `File::create` of
   the fixed name `<session>.jsonl.tmp` and `rename` all follow links. A planted `.tmp` link
   truncates its target, a linked `projects/<slug>` puts the transcript anywhere.
2. **Attachment and editor uploads** (`yulius-app/src/uploads.rs`). The destination folder is
   checked by path (`ensure_plain_dirs`, `ensure_dirs`, the re-check in `finish_upload`) and then
   written by path (`.yulius-upload-*` creation, `hard_link`, `rename`, `set_permissions`). A
   folder swapped for a link after the last check places the upload (or, with overwrite, replaces
   a file) outside the sandbox. The module documents the gap.
3. **Skill copies** (`yulius-harness/src/skills.rs`). Skills go to `<sandbox>/home/.claude/skills`.
   A linked `skills` (or `.claude`, or `home`) makes `create_dir_all`, the copies, the renames and
   the `remove_dir_all` of stale copies act on its target. The staging folder's name
   (`.<skill>.yulius-stage-<pid>`) is predictable, and `fs::copy` follows a link the agent places
   in it after it is created, truncating and overwriting a host file (the SQLite database).
4. **Editor reads** (`yulius-app/src/files.rs`, `file_read`). The path is canonicalized and
   checked, then read by path: a middle folder swapped for a link in between shows a host file.
5. **Editor saves** (`file_write`, added to #657 by task #160's review). The parent is
   canonicalized and checked, then the temporary file is created and renamed by path: a middle
   folder swapped in between puts the saved file outside the root. #655 closed only the planted
   temporary-name link.

Why the existing tests missed it: they plant links before the request, which the path checks
catch; none changes a folder between the check and the use, and none plants a link where the
server writes a fixed or predictable name.

The repository already has the pattern: `vault/env_files.rs` `open_folder` (each folder opened
from an anchor with `O_DIRECTORY | O_NOFOLLOW`, the fd's real path checked, every read and write
through `openat`/`renameat`), `hooks.rs` `read_repo_doc` (the opened file's real path checked),
`leftovers.rs` `remove_below` (#649's rule: no link between the data directory and the item), and
`claude_config.rs` `open_regular` (#634: `O_NOFOLLOW | O_NONBLOCK`, regular files only).

## Requirements

- **HW-001** In the five places above, every folder the server uses below a sandbox is opened
  from an anchor the agent cannot replace (the sandbox directory, or a canonical root whose real path is checked),
  one component at a time with `O_DIRECTORY | O_NOFOLLOW`; a link or a non-folder on the way is
  refused. Missing folders are created with `mkdirat` relative to their parent's handle.
- **HW-002** Where links inside an editor root are followed by design (EDT-001: a link to a file
  in the same folder behaves like its target), the folder or file is opened from its resolved path
  and used only when the opened handle's real path (`/proc/self/fd`, `F_GETPATH` on macOS) is
  still inside the root and out of `.git`.
- **HW-003** Files are created through the folder's handle with `O_CREAT | O_EXCL | O_NOFOLLOW`
  under an unpredictable name (random suffix), then linked (`linkat`) or renamed (`renameat`)
  within that handle; their mode is set on their own handle. A name the server tests or replaces
  is checked with `fstatat(AT_SYMLINK_NOFOLLOW)` through the same handle; a link is never
  replaced, removed through or written through.
- **HW-004** Reads of agent-writable files open with `O_NOFOLLOW | O_NONBLOCK` and use only a
  regular file (`fstat`).
- **HW-005** Skill installation removes stale copies, staging and backup folders through handles:
  each directory is opened relative to its parent with `O_NOFOLLOW`, entries are removed with
  `unlinkat`, so a link inside is removed, never followed.
- **HW-006** Behavior that does not involve a link is unchanged: imports, uploads (all three
  placements), skills (install, replace, prune, bounds) and the editor (read, save, create) work as
  before.

## Plan

- [x] Write the spec, the ADR and the plan
- [x] Editor reads use the opened file's real path
- [x] Editor saves go through the parent folder's handle
- [x] Imported transcripts are written through handles from the sandbox directory
- [x] Uploads hold their folder's handle from the check to the placement
- [x] Skill copies go through handles from the sandbox directory
- [x] Docs, Vidoc ignores and gates
- [x] Review fixes: bounds, blocking calls, attachment count, test and wording

## Acceptance criteria

- `prepare_imported_resume` with `<session>.jsonl.tmp` planted as a link to an outside file
  leaves that file untouched (and with a linked project folder writes nothing through it).
- An upload whose folder is swapped for a link between the check and the placement leaves the
  outside folder empty (test-only pause hook).
- With a `skills` folder, or a file in the staging folder, that is a link to outside, the outside
  target is unchanged.
- `file_read` refuses a path whose middle folder became a link to outside the root.
- A save whose parent folder becomes a link to outside between the check and the write leaves the
  outside folder untouched.
- The existing import, upload, skills and files test suites pass unchanged.

## Test plan

Each place gets one regression test, committed before its fix and shown failing:

- `files::a_read_refuses_a_folder_swapped_for_a_link_after_the_check` and
  `files::a_save_never_lands_in_a_folder_swapped_for_a_link_after_the_check` (yulius integration
  tests): a pause point swaps a middle folder for a link to an outside folder between the check and
  the use.
- `import_server::import_resumes_applies_and_reruns_idempotently` (in `assert_imported_resume`): a
  `<session>.jsonl.tmp` link to an outside file; unit tests
  `imports::tests::a_linked_project_folder_is_never_written_through` and
  `auxiliary_trees_are_copied_to_a_bounded_depth`.
- `uploads::an_attachment_folder_swapped_for_a_link_leaves_the_outside_empty` and
  `uploads::an_editor_upload_never_replaces_a_file_through_a_swapped_folder`: three pause points
  model a `RENAME_EXCHANGE` loop (a link at each use, the folder at the re-check); the upload must
  still succeed in the real folder.
- `yulius-harness` `skills::a_linked_skills_folder_is_never_written_through` (a linked `skills` or
  `.claude`), and unit tests `a_link_planted_in_the_staging_folder_is_never_written_through` (a
  `cfg(test)` pause point) and `a_skills_folder_with_too_many_entries_is_refused`.

The race tests use pause points compiled only with `yulius-app`'s `test-support` feature (and
`cfg(test)` in the harness), so they are deterministic: no sleeps, no rename loops.

## Decisions

- ADR-0174: host access to agent-owned folders goes through handles opened without following
  links; races are tested with test-only pause points.
- One pull request for the five places: one defect class, one fix pattern and one issue; split
  only if it grows past about 400 changed lines outside tests.
- Pre-existing paths of the same class found on the way (the independent review): the transcript
  step-back and the home's creation (#661), the config dir's preparation (commented on #634).
- `files_list`, `file_create` (folders) and `file_delete` resolve by path too; they are outside
  #657's scope and are tracked in #660 (reproduced for `file_delete`), with the attachment
  removals (`remove_attachments`, `prune_old_drafts`).

## Evidence

- Each regression test was committed before its fix and run failing on the unfixed code:
  - the editor read returned `host secret`;
  - the save replaced the outside `a.txt` with `new`;
  - the import overwrote `victim.db` with the transcript;
  - the attachment landed in the outside folder, and the overwrite replaced the outside file;
  - the skills install wrote into and pruned the linked host folder;
  - the planted staging link overwrote `yulius.db`;
  - the review-round tests failed too: aux depth and the listing cap.
- `scripts/dev ci` at 886ceaa8: exit 0.
  - Repository contracts, fmt, Clippy and the macOS Clippy (`scripts/dev macos`) passed.
  - nextest: 2223 tests passed, 18 skipped. Doctests and `cargo doc` passed.
  - Two earlier runs failed and were fixed: `mode_t` is `u16` on macOS, and a private doc link.
- `scripts/dev security` at 886ceaa8: exit 0, "no open critical or high finding, and no open medium one in the change" (vidoc-1, 22 files in 15 chunks).
  - Outside the change it reported `file_create`/`file_delete` (tracked in #660), `aux_relative` and `move_tree` in `imports.rs`, and `opening_pastes` in `supervisor.rs`; Yulius CI files these.
  - No low findings in the change.
- Vidoc ignores removed with their fixes: the five "real: tracked in #657" entries (`files.rs` ×3, `imports.rs`, `uploads.rs`). The `skills.rs` source-side entry stays; it is a false positive about instance-owned sources, and its reason was updated.
- A throwaway pause point (not committed) reproduced `file_delete` deleting a host file through a swapped folder; filed as #660.
- Independent review, round 1 (fresh session): no blockers.
  - Fixed here, test-first where behavioral: aux copy depth and handles, bounded skills listings, upload placement in `spawn_blocking`, the attachment count through the handle, the race tests requiring success in the real folder, and narrower wording.
  - Pre-existing findings: commented on #634 (config dir) and filed #661 (step-back, home creation).
- Not tested: macOS at run time (only compiled; `real_path` uses `F_GETPATH` there), and a live agent racing a real `RENAME_EXCHANGE` loop. The pause points model it deterministically.
