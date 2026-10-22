---
yulius-task: 38d860e8-19b3-484b-a9d5-23517b101d83
status: in-progress
---

# SPEC-263: Disk upkeep refuses links at worktrees/ and ci/jobs (#649)

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #249
Scope authorization: task #249 (nightly "Work through open issues"). Fixes #649.

## Goal

The disk upkeep (SPEC-008, SPEC-019) measures and removes only caches it reaches from the
sandbox's own directory without a link on the way, checked when it scans and again right before
each removal, so a sandbox can never steer it into another sandbox's files.

## Context

- Symptom (#649): a sandbox's runs and shells mount the whole `sandboxes/<sid>/` read-write. With
  `ln -s ../victim/worktrees sandboxes/<attacker>/worktrees`, the attacker's upkeep lists the
  victim's worktrees as its own: an orphan worktree untouched for a day loses its `target`,
  `node_modules` and declared caches. A linked `ci` or `ci/jobs` does the same to the victim's CI
  checkouts whenever the attacker's sandbox runs no CI. A link at one worktree entry
  (`worktrees/wt-x -> ../../victim/worktrees/wt-y`) does it too.
- Root cause: `App::disk_scan` (`crates/yulius-app/src/disk.rs`) builds `worktrees/<entry>` and
  `ci/jobs/<entry>` and gates them only on `<dir>/.git` existing, which follows links.
  `real_dir_below(worktree, relative)` checks from the worktree down to the cache, never the
  worktree itself or the folders above it. `prune` then calls `std::fs::remove_dir_all(path)` by
  path, so a folder swapped for a link between the scan and the removal (the scan can take
  minutes) steers the removal too. A closed or cancelled task's worktree reached through a link is
  handed to `remove_worktree` as well.
- Why tests missed it: `a_cache_that_is_a_link_is_never_followed` puts the link at the cache
  itself, the only place `real_dir_below` looks.
- The leftovers module (SPEC-215) already has the rule (`no_link_between`) and a removal that
  opens every folder with `O_NOFOLLOW` (`remove_below`, ADR-0174's "through handles").

## Requirements

- DUL-001. A worktree, an orphan worktree and a CI job checkout count only when every folder from
  the sandbox directory (exclusive) down to it, and it itself, is a real directory, not a link;
  a cache counts only when, in addition, every folder down to it and it itself is one, checked
  again right before it is measured. Nothing else is listed as ended or removed.
- DUL-002. Right before each cache removal the rule is checked again, and the removal goes through
  `remove_below` from the sandbox directory, so a folder swapped for a link after the check fails
  the removal instead of following it. A refused removal is skipped with a warning and not
  counted or audited. Off Linux, where `remove_below` cannot work, the upkeep removes no cache.
- DUL-003. A closed or cancelled task's worktree is checked again by the same rule right before it
  is handed to Git for removal.
- DUL-004. Caches reached without links are pruned as before (every existing `disk.rs` test
  passes unchanged).

## Plan

- [x] Spec for link-refusing disk upkeep
- [x] Regression tests for linked worktrees/ and ci/jobs (failing)
- [x] Scan and removal refuse links from the sandbox directory
- [ ] Review round 1: ended-worktree re-check test, macOS removal, split tests
- [ ] Docs, changes entry and gates green
- [ ] Independent review and fixes
- [ ] Merge into dev on green CI

## Acceptance criteria

- New tests in `crates/yulius/tests/disk.rs` fail on `dev` and pass with the fix:
  - a linked `worktrees`, and a linked worktree entry, leave another sandbox's orphan cache alone;
  - a linked `ci/jobs`, and a linked `ci`, leave another sandbox's checkout cache alone;
  - a `worktrees` swapped for a link after the scan, right before the removal
    (`race::Point::CachePrune`), leaves the other sandbox's cache alone;
  - a closed task's worktree reached through a linked `worktrees`, at the scan or swapped in after
    it, is not handed to Git (its repository registers the other sandbox's folder).
- Every existing disk test passes unchanged (DUL-004).

## Test plan

- Integration (`crates/yulius/tests/disk.rs`, real Git, real worktrees): two sandboxes `s1`
  (victim) and `s2` (attacker); the links are made in `s2`, the upkeep runs for `s2`, and the
  victim's `target/blob` must survive. The race test registers a `CachePrune` hook that swaps
  `s2/worktrees` for a link to `s1/worktrees` (holding a same-named orphan) between scan and
  removal.
- Full gate: `./scripts/gate.sh` (or the AGENTS.md gate).

## Decisions

- Root of the check is the sandbox directory, not the data directory. Everything the upkeep
  touches lies below `sandboxes/<sid>/`, and that is exactly what a sandbox can change; the
  sandbox cannot replace its own directory (it is the mount's source). The data-directory root of
  the leftovers module would also refuse an operator's link at `sandboxes` (data moved to another
  disk), which would silently stop the upkeep that keeps sandboxes under quota. This matches
  ADR-0174, whose helpers walk from the sandbox directory. No new ADR: ADR-0174 already decides
  that host access to agent-owned folders goes through handles.
- Reuse `no_link_between` and `remove_below` from the leftovers module (made `pub(crate)` where
  needed) rather than a second implementation.
- `remove_below` instead of `remove_dir_all` on Linux: it removes through `O_NOFOLLOW` handles,
  closing the window between the re-check and the removal, and also removes a tree a tool made
  read-only. It reaches entries through `/proc/self/fd`, which macOS lacks (there it removes
  nothing and reports success; pre-existing in the leftovers module since #681). Off Linux the
  upkeep therefore refuses each cache removal with a warning instead of counting a removal that
  did not happen (review round 1) or removing by path after the check (Vidoc: a link swapped in
  between is followed). Only macOS development hosts, whose runs are not contained
  (docs/operations.md), lose the pruning of caches; the leftovers module already removes nothing
  there. A Vidoc `[[ignore]]` was tried first and dropped: Vidoc words the finding differently on
  each scan.
- Out of scope, reported on #649 (review round 1, pre-existing): `worktree_remove_if_safe`
  resolves the owned root and the worktree with two `canonicalize` calls and Git resolves the path
  again, and `sandboxes/<sid>/repo` is never checked for links, so a sandbox that keeps swapping
  folders can still race the DUL-003 check. Closing it means removing worktrees through handles in
  `yulius-git`, which is a change of its own.
