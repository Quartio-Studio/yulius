---
yulius-task: d8dfba5c-03dc-4fef-ad77-1e94b82a18a5
status: done
---

# SPEC-296: a snapshot import's worktree is reserved where no planted link can steer it

Status: done
Date: 2026-10-24
Owner: Yulius task #282
Scope authorization: issue [#824](https://github.com/Quartio-Studio/yulius/issues/824) (Vidoc
vidoc-1, low, found in task #255's change scan): "The path is reserved atomically [...] It is then
checked with `symlink_metadata` before Git and the extraction use it." Acceptance: a link planted at
the chosen `worktrees/import-…` path between the choice and the creation writes nothing outside the
sandbox directory (pause-point test); a normal import still picks `import-<job>`, then
`import-<job>-2` and so on when taken. Decision: [ADR-0213](../../docs/decisions/0213-a-snapshot-import-checks-out-in-a-server-private-folder.md).

## Goal

A process in a sandbox cannot make the service user's Git or the snapshot extraction write a
snapshot import outside the sandbox directory, and imports keep working.

## Context

- Symptom: `apply_snapshot` (`crates/yulius-app/src/imports.rs`) picked an unused
  `<sandbox>/worktrees/import-<job>[-n]` with `while path.exists()`, then ran
  `fs::create_dir_all(worktrees)` and `git worktree add <path>`, then
  `extract_snapshot(archive, path)`, all by path.
- Contained runs mount `sandboxes/<sid>/` read-write (ADR-0174, #649), so an agent that knows the
  job id can plant a link at the chosen name between the check and Git's create.
- Impact: `git worktree add` accepts an existing *empty* directory and checks emptiness through
  the link, so a link to an empty directory elsewhere (another sandbox's folder, a server folder)
  makes the service user's Git check the head out there, and `extract_snapshot` then writes every
  snapshot file there too: it calls `fs::canonicalize(root)`, so a link at the root is followed;
  `prepare_target` refuses links only below the root. The real adapter's removal then refuses the
  worktree (`OutsideOwnedRoot`), so the snapshot's files stay in the outside folder; the test
  double's removal follows the link and deletes that folder (reproduced, Evidence).
- Root cause: the temporary checkout lives in a folder the agent can change. A name reserved
  atomically there (create-new `mkdirat` through a handle) does not close it: the agent can rename
  the reserved folder away and plant the link before Git runs, and Git resolves the path itself
  and follows links. Only a folder out of the agent's reach closes it.
- Why the tests missed it: the import tests never plant anything in `worktrees/`.
- The snapshot worktree is temporary: it is created, filled, committed and removed in the same
  call; nothing else (hygiene, the preparation task) uses its path.

## Requirements

- SNW-001: the snapshot's temporary worktree is created under `<data>/import-worktrees/<sandbox>/`,
  a folder of the server (mode 0700) outside every sandbox directory, as `project_change` does with
  `<data>/changes` (SPEC-247).
- SNW-002: its name is reserved with create-new semantics (`mkdir`, `EEXIST` means taken, never
  an existence check): `import-<job>`, then `import-<job>-2`, `-3`, … up to
  `MAX_SNAPSHOT_BRANCHES`, then the import fails with a conflict. Git accepts the empty folder.
- SNW-003: `extract_snapshot` refuses a root that is a link or not a folder instead of following
  it: the root is opened with `O_DIRECTORY | O_NOFOLLOW` and used by the real path of the handle.
- SNW-004: entries an agent puts at `<sandbox>/worktrees/import-…` are never touched.
- SNW-005: a failure after the reservation (Git, extraction, commit) removes the worktree with
  Git's safe removal (never `git worktree prune`, SPEC-215) and its folder; disk upkeep lists an
  interrupted import's checkout in a live sandbox as an orphan worktree after an hour, removes
  `<data>/import-worktrees/<sandbox>` with a removed sandbox's worktrees and lists one no sandbox
  owns as unowned (review rounds 1 and 2).

## Plan

- [x] Spec and ADR record the private snapshot worktree
- [x] Regression test shows a planted link steers the snapshot worktree
- [x] Snapshot worktree reserved create-new in a server-private folder
- [x] extract_snapshot refuses a linked root
- [x] Docs, changes entry and gates green
- [x] Independent review answered and gates green on the answered head

## Acceptance criteria

- AC-1: with a link to an empty folder outside the sandbox planted at
  `<sandbox>/worktrees/import-<job>` exactly between the reservation and `git worktree add`
  (pause point `race::Point::SnapshotWorktree`), the outside folder stays empty, the link is left
  as it is, and the snapshot branch is still committed with the snapshot's files.
- AC-2: the reservation picks `import-<job>`, then `import-<job>-2` when that name is taken (by a
  folder, a file or a link, which is not followed), and fails past the bound.
- AC-3: `extract_snapshot` on a root that is a link to a folder fails and writes nothing there.
- AC-4: an import that fails after the reservation, before or after Git created the worktree,
  leaves `<data>/import-worktrees/<sandbox>` empty and no live Git worktree of it.
- AC-5: a removed sandbox's `import-worktrees/<sandbox>` is deleted by the automatic disk-upkeep
  round (a live one's stays); one with no sandbox record is listed as unowned; an hour-old one of a live sandbox is listed as an
  orphan worktree and kept for a person.

## Test plan

- `import_server::a_link_planted_at_the_snapshot_worktree_is_never_followed` (AC-1), red on
  `dev` with the pause point added: Git checks the head out into the outside folder.
- `imports::tests::snapshot_worktree_names_are_reserved_create_new` (AC-2).
- `imports::tests::snapshot_extraction_refuses_a_linked_root` (AC-3), red on `dev`: the files land
  in the link's target.
- `import_server::a_failed_snapshot_import_leaves_no_checkout_behind` (AC-4): Git refuses the
  reserved folder once a file is put in it at the pause point.
- `import_server::a_snapshot_import_failing_after_git_leaves_no_worktree` (AC-4): the stored
  archive turns unreadable at the pause point, so Git creates the worktree and the extraction
  fails.
- `leftovers::a_removed_sandbox_loses_its_disposable_parts_and_keeps_what_prj_009_keeps` and
  `leftovers::instance_leftovers_are_found_and_only_safe_ones_go_by_themselves` (AC-5).
- `import_server::import_resumes_applies_and_reruns_idempotently` stays green (the snapshot
  branch, its files and parent; no `import-*` left in `worktrees/`).

## Decisions

- **The checkout moves out of the sandbox** (ADR-0213), instead of the issue's suggestion of a
  name reserved by `mkdirat` in `worktrees/` plus a re-check after Git. Git resolves the worktree
  path itself and follows links, so a reservation inside the agent's folder is renamed away and
  linked before Git runs, and Git's own checkout writes outside before any re-check can refuse it:
  that would fail the import but not meet "nothing is written outside the sandbox". A private
  folder leaves the agent nothing to plant, and also keeps it from swapping folders inside the
  checkout during the extraction.
- Consequence for the issue's first criterion: a link planted at the old `worktrees/import-…` name
  no longer *fails* the import, it is simply not used; the import completes and nothing is written
  through it. That is the criterion's purpose (no write outside the sandbox) with less to fail.
- `extract_snapshot` still refuses a linked root (SNW-003) as a second line: it is public and takes
  any root.
- Out of scope, reported on #824 rather than fixed here: task worktrees (`supervisor.rs`,
  `imports.rs` `ensure_imported_worktree`) are still created by path in `worktrees/`, and the
  service user's Git works by path on the agent-writable `repo/` (hardened by `FIXED_CONFIG`,
  #54). Both predate this issue.

## Evidence

- Red on `dev` (3768adba plus the test-only pause point, commit e8aa1865):
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^import_server::a_link_planted_at_the_snapshot/)'`
  failed with "the folder outside the sandbox is not removed through the link": Git checked the
  head out into the planted link's empty target (with the test double, whose removal compares paths
  lexically, the worktree removal then deleted that folder; the real adapter refuses with
  `OutsideOwnedRoot` and leaves the files there). `imports::tests::snapshot_extraction_refuses_a_linked_root`
  failed: the file was written through the linked root.
- Review round 1 tests red on 4a095cb4's code (commit e1b46a92): the failed import left
  `import-<job>` in the private folder; the removed sandbox's `import-worktrees/old` stayed; the
  stray `import-worktrees/stray` was not listed.
- Green with the fix: `import_server::` 11 passed, `leftovers::` 24 passed; `yulius-app`
  `imports::`/`playground`/`snapshot` 18 passed (AC-2 unit test included).
- Gate: `scripts/dev ci` exit 0 on adb6a418 (2593 tests passed, 17 skipped); rerun on the
  answered head, see the pull request.
- Security: `scripts/dev security` exit 2, **unrun**: Vidoc answered HTTP 429 ("This API key has
  used its 50,000,000 tokens"); the 5 code files were not scanned. Not a pass.
- Independent review round 1 (fresh session, adb6a418), request changes, answered:
  - own, major: a failed import left its checkout in `<data>/import-worktrees`, which disk upkeep
    never scanned: fixed test-first (SNW-005, AC-4, AC-5).
  - own, minor: the docs said the worktree removal deleted the outside folder on `dev`; only the
    test double does: corrected (Context, changes, test comment).
  - own, minor: spec not finalized: done here.
  - own, nit: the root check and `canonicalize` were two calls: the root is now opened with
    `O_NOFOLLOW` and used by its handle's real path.
  - pre-existing, major: Git's administrative folder `repo/.git/worktrees/<name>` stays in the
    agent-writable repository (also for task worktrees). This task may not create issues; it is
    recorded on #824 for a person to file.
- Independent review round 2 (fresh session, f6d3cf97), approve with minors, answered:
  - own, minor: the failure path's `discard_worktree` ran a sandbox-wide `git worktree prune`,
    which SPEC-215 avoids: it now uses Git's safe `remove_worktree` and removes the folder; a
    record Git keeps is prunable (the hygiene round's). The test double never prunes, so no test
    can be red for this; the registered-worktree failure is covered by
    `a_snapshot_import_failing_after_git_leaves_no_worktree` (green before and after).
  - own, minor: an interrupted import's checkout in a live sandbox was never listed: disk upkeep
    lists it as an orphan worktree after an hour (red first in
    `leftovers::instance_leftovers_are_found_and_only_safe_ones_go_by_themselves`).
  - own, minor: the registered-worktree failure branch was untested: test added.
  - own, nits: the failed removal logs at `warn`; the plan's last item no longer claims delivery;
    the first #824 comment's claim that the real removal deletes the outside folder is corrected
    there. The `removed-worktrees` item's `repo` was left as the other parts': its records are
    dropped with the repository.
  - pre-existing: a failed import keeps its new snapshot branch, as on `dev`.
