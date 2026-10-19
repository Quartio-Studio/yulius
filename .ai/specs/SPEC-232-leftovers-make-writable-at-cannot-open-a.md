---
yulius-task: 3c89fa41-ddce-47c7-bcde-4566c81174fb
status: in-progress
---

# SPEC-232: A leftover tree with an unreadable directory of its owner is removed

Status: in-progress
Date: 2026-10-19
Owner: task #218
Scope authorization: task #218: "Fix GitHub issue #680". Fixes #680, found in the review of
SPEC-228 (task #214). Keeps SPEC-215 (leftovers), ADR-0174 (host access through handles, no link
followed) and ADR-0176 (`remove_below` as the body of `sandbox-host remove-tree --below`); decision
in [ADR-0179](../../docs/decisions/0179-a-tree-is-made-writable-through-o-path-handles.md).

## Goal

`remove_below` removes a tree that contains a directory its user owns but cannot read (mode
`0o000`, `0o300`, any mode without the owner's `r`), as it already removes one whose directories
are only read-only, without ever following a link.

## Context

Confirmed against `dev` at b9080ea4.

- **Symptom.** Without an isolation, and for the service user's own trees outside `sandboxes/`,
  a leftover holding a directory of mode `0o000` fails on every round with "Permission denied".
- **Root cause.** `remove_below`'s fallback and `make_writable_at`
  (`crates/yulius-app/src/leftovers.rs`) open the tree top and each child directory with
  `openat(O_DIRECTORY | O_NOFOLLOW)` (read-only) *before* `fchmod` gives the owner `0o700`. That
  open needs the owner's read permission on the directory itself (not search: `0o300` fails too),
  so it fails with `EACCES`, the failure is skipped silently, the directory keeps its mode and the
  second `remove_tree_at` fails on it again.
- **Why the tests missed it.** `making_a_tree_writable_never_follows_a_link` locks a directory
  with `0o4555`, which is still readable; and the gate runs as root (the sandbox's mapped root),
  whose `CAP_DAC_OVERRIDE` opens any directory, so a `0o000` case would have passed there anyway.
- **Since #675** every leftover below `sandboxes/` is removed as root of the mapped namespace,
  where `CAP_DAC_OVERRIDE` hides the bug; it remains on the path without an isolation.

## Requirements

- R1. A directory whose owner lacks read or search permission on it is given the fixed mode
  `0o700` and removed with its tree, at the tree top and at every level below it.
- R2. No link is followed: a link inside the tree, or a directory swapped for a link at any
  moment, never carries the mode change or the walk outside the tree (ADR-0174).
- R3. The regression test runs in the gate, which runs as root: as root it drops
  `CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH` and `CAP_FOWNER` from its own thread's effective set
  so the kernel checks the owner's bits, as for the service user.

## Plan

- [x] Spec for #680 written and valid
- [ ] Failing regression test for a 0o000/0o300 directory
- [ ] make_writable_at and the tree top change modes through O_PATH handles
- [ ] ADR, docs and changes entry
- [ ] Independent review answered
- [ ] Gates green and delivered on green CI

## Acceptance criteria

- `remove_below` removes a tree holding a `0o000` and a `0o300` directory (one at the top, nested
  ones below), run without the DAC capabilities; the test fails on `dev` b9080ea4.
- `making_a_tree_writable_never_follows_a_link` and the other leftovers tests still pass.
- `scripts/dev ci` and `scripts/dev security` pass.

## Test plan

- `leftovers::tests::a_tree_with_unreadable_directories_goes` (new, Linux): builds
  `item/{a 0o000 dir, b 0o300 dir holding c 0o000}` with files, plus the item itself `0o000`,
  drops the DAC capabilities of its thread when root, calls `remove_below` and checks the item is
  gone and its parent kept. Without the fix it fails with "Permission denied".
- `making_a_tree_writable_never_follows_a_link` keeps proving R2 for a link inside the tree.

## Decisions

- **`O_PATH` handle, mode set through `/proc/self/fd/N`.** Each directory is opened with
  `O_PATH | O_DIRECTORY | O_NOFOLLOW` relative to its parent's handle (no permission needed on
  the directory itself; a link fails or is skipped), checked to be a directory with `fstat`, its
  mode set with `chmod` on `/proc/self/fd/N`, which resolves to the very inode the handle pinned,
  and it is listed and descended through that same handle. `fchmodat(parent, name)` was rejected:
  it resolves the name again and follows a link swapped in after the check (no
  `AT_SYMLINK_NOFOLLOW` on kernels before 6.6). Reopening the child by name after the chmod was
  rejected for the same race. See ADR-0179.
- **Capabilities in the test, not a skip.** A test skipped as root never runs in the gate (it runs
  as root); dropping the thread's DAC capabilities makes root an ordinary owner. Capabilities are
  per thread, so other tests are unaffected. It uses `rustix` (already in the lockfile) with its
  `thread` feature, as a Linux dev-dependency of `yulius-app`.
- Other platforms (macOS, a development host) have no `O_PATH`: they keep the read-only open, and
  `/proc/self/fd`, which the walk already needs, is Linux only anyway.
