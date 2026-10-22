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
  counted or audited. Off Linux, where `remove_below` cannot work, the upkeep finds, measures and
  removes no cache.
- DUL-003. A closed or cancelled task's worktree is checked again by the same rule right before it
  is handed to Git for removal.
- DUL-004. Caches reached without links are pruned as before (every existing `disk.rs` test
  passes unchanged).

## Plan

- [x] Spec for link-refusing disk upkeep
- [x] Regression tests for linked worktrees/ and ci/jobs (failing)
- [x] Scan and removal refuse links from the sandbox directory
- [x] Review round 1: ended-worktree re-check test, macOS removal, split tests
- [x] Docs, changes entry and gates green
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

## Evidence

- Regression tests fail without the fix. On the old logic (`disk.rs` of 74f4185b, which only
  adds the `CachePrune` pause point to `dev`'s code), `cargo nextest run -p yulius -E
  'binary_id(yulius::main) & test(/^disk::/)'` gives 24 run, 17 passed, 7 failed: all seven new
  tests fail at their victim-side assert. Four print "pruned another sandbox's cache" with
  `caches: 1`, the swap test prints "the removal followed a link swapped in after the scan", and
  the two ended-worktree tests print "was removed" with `worktrees: 1`. With the fix minus only
  the DUL-003 re-check, `an_ended_worktree_swapped_for_a_link_after_the_scan_is_not_removed`
  fails alone.
- With the fix: `disk::` and `leftovers` integration tests 40 run, 40 passed. The 17 tests that
  were there before pass unchanged (DUL-004).
- `scripts/dev ci` at 095b8169: exit 0. That covers fmt, clippy `-D warnings` for Linux and
  `aarch64-apple-darwin`, nextest 2411 run, 2411 passed, plus deny, shear and docs. The earlier
  run at 95451995 also passed.
- `scripts/dev security` at 095b8169: exit 0, no open finding in the change. History of the scan:
  - At 95451995 Vidoc found a medium finding, a path-based removal off Linux after the re-check.
    It is fixed by refusing instead (see Decisions).
  - The same scan found a low finding: `worktree.join(".git").exists()` follows a linked `.git`.
    It is left as is. The worktree itself must already be a real directory reached without
    links, and a `.git` link only steers read-only Git queries. Caches and the worktree Git
    removes are checked from the sandbox directory, so a linked `.git` reaches nothing outside
    the sandbox. The finding is pre-existing; the scan at 095b8169 did not report it, the one at
    121b3e06 did again.
  - At 095b8169, one low finding outside the change (`leftovers.rs:1462`) is CI's to file.
- Review round 1 (fresh session, at 303feb63) requested changes. All own-code findings are fixed:
  - A test for the DUL-003 re-check was added.
  - Off Linux the upkeep now refuses instead of reporting a removal that never happened.
  - Each cache is checked again right before it is measured.
  - The link tests are split so each link location fails on its own.
  - The CI tests assert `caches == 0`.
  - The changes entry no longer overclaims.
  The pre-existing Git canonicalize window is reported on #649 and recorded in Decisions.
- Review round 2 (fresh session, at 0adf339f) approved, with findings fixed after it:
  - The Git window note is now really on #649 (comment 6331430833). It also corrects the earlier
    comment's overstatement: `git worktree remove` refuses a folder whose `.git` does not point
    back, so the registered-victim tests guard defence in depth.
  - The DUL-003 check moved to just before `remove_worktree`, after the `ignored_entries` Git
    call.
  - Off Linux no caches are found, so they are not measured or warned about every round.
    `docs/operations.md` says caches are pruned on Linux only.
  - Left as is: a cache folder swapped for a link after the last check has the link removed,
    which `remove_below` reports as success, so the round counts and audits it. Only a sandbox
    acting on its own folder can do that, and nothing outside it is touched.
  - Pre-existing: `remove_below` reports success on macOS while removing nothing (#681). This is
    noted on #649, because this task may not file issues.
- After round 2's fixes and a merge of `origin/dev` (121b3e06):
  - `scripts/dev ci` exit 0, with 2411 tests run and 2411 passed.
  - `scripts/dev security` exit 0, with no open critical, high or in-change medium finding.
    Besides the `.git` low above, it reported one low outside the change: an mtime overflow at
    `disk.rs:392`, which CI files as an issue.
  - The DUL-003 test still fails with only the moved re-check removed.
- Review round 3 (fresh session, at 5b22f22e) approved. Its one own-code low: off Linux the
  `disk.rs` integration tests expected pruning that no longer happens there. That file is now
  `#![cfg(target_os = "linux")]`. The leftovers tests were already Linux-only in practice (#681).
- Not tested here: macOS behaviour at runtime (only compiled and linted for
  `aarch64-apple-darwin`).

