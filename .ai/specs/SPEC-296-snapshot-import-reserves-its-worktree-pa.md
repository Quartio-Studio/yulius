---
yulius-task: d8dfba5c-03dc-4fef-ad77-1e94b82a18a5
status: in-progress
---

# SPEC-296: a snapshot import's worktree is reserved where no planted link can steer it

Status: in-progress
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
  `prepare_target` refuses links only below the root. Reproduced (Evidence): after the commit,
  `git worktree remove` also deletes the link's target folder.
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
- SNW-003: `extract_snapshot` refuses a root that is a link or not a folder (checked with
  `symlink_metadata`) instead of following it.
- SNW-004: entries an agent puts at `<sandbox>/worktrees/import-…` are never touched.

## Plan

- [x] Spec and ADR record the private snapshot worktree
- [ ] Regression test shows a planted link steers the snapshot worktree
- [ ] Snapshot worktree reserved create-new in a server-private folder
- [ ] extract_snapshot refuses a linked root
- [ ] Docs, changes entry and gates green
- [ ] Independent review answered and delivered on green CI

## Acceptance criteria

- AC-1: with a link to an empty folder outside the sandbox planted at
  `<sandbox>/worktrees/import-<job>` exactly between the reservation and `git worktree add`
  (pause point `race::Point::SnapshotWorktree`), the outside folder stays empty, the link is left
  as it is, and the snapshot branch is still committed with the snapshot's files.
- AC-2: the reservation picks `import-<job>`, then `import-<job>-2` when that name is taken (by a
  folder, a file or a link, which is not followed), and fails past the bound.
- AC-3: `extract_snapshot` on a root that is a link to a folder fails and writes nothing there.

## Test plan

- `import_server::a_link_planted_at_the_snapshot_worktree_is_never_followed` (AC-1), red on
  `dev` with the pause point added: Git checks the head out into the outside folder.
- `imports::tests::snapshot_worktree_names_are_reserved_create_new` (AC-2).
- `imports::tests::snapshot_extraction_refuses_a_linked_root` (AC-3), red on `dev`: the files land
  in the link's target.
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

To be filled when the work is verified.
