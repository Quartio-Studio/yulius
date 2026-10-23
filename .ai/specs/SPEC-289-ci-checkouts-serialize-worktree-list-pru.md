---
yulius-task: 6671e26d-ee17-4a62-b936-716717241058
status: in-progress
---

# SPEC-289: `git worktree list` never reads another checkout's half-written registration

Status: in-progress
Date: 2026-10-24
Owner: task #275
Scope authorization: task #275: "Solve GitHub issue #666 … in one pull request into `dev`".
Fixes #666. Extends CI-109 of [SPEC-050](SPEC-050-parallel-ci-and-gentle-delivery.md) (the
worktree-registration lock of SPEC-021) to `worktree list`.

## Goal

The jobs of one CI run create their checkouts of the same repository at the same time, and none of
them ends in `error` because another one is registering its worktree.

## Context

Confirmed against `dev` at 2025e0f0 with Git 2.43.0.

- **Symptom.** A CI job errors while checking out (#666, twice):
  `git worktree list failed (exit Some(128)): fatal: failed to read '.git/worktrees/split.2/locked': No such file or directory`,
  and in the guard test `ci_checkouts_created_at_once_all_succeed` on PR #764:
  `fatal: failed to read .git/worktrees/j1.1/commondir: Success`.
- **Code path.** `Git::ci_checkout` (`crates/yulius-git/src/ci.rs`) runs `worktree_list`, then,
  for a new checkout, `worktree prune` and `worktree add`. `Git::run`
  (`crates/yulius-git/src/runner.rs`) takes the process-wide `WORKTREE_CHANGES` lock only for
  `worktree add|move|remove|prune` (`changes_worktrees`), so a `worktree list` runs while another
  thread's `worktree add` is writing `.git/worktrees/<name>/`.
- **Root cause.** `git worktree add` creates `.git/worktrees/<name>/` and writes `gitdir`,
  `locked` ("initializing"), `commondir`, `HEAD` one after another, then deletes `locked`. A
  concurrent `git worktree list` enumerates that directory; Git dies when a file it saw exist is
  gone or still empty when it reads it. Probe on Git 2.43.0: a registration with `gitdir` and an
  empty `commondir` makes `list` exit 128 with exactly `failed to read .git/worktrees/zz/commondir:
  Success`; a `gitdir` alone or an empty directory does not.
- **Why the tests missed it.** The window is a few microseconds inside Git; 16 concurrent
  checkouts hit it only under load (seen once in a full gate and once on CI). CI-109's fix covered
  prune vs add, not list.

## Requirements

| ID | Requirement |
| --- | --- |
| WTL-001 | Yulius runs `git worktree list` under the same process-wide lock as worktree `add`, `move`, `remove` and `prune`, so in one Yulius process a list never overlaps a registration change. |
| WTL-002 | A CI checkout whose registration overlaps another checkout's `worktree add` succeeds (both checkouts are created). |

## Plan

- [ ] Spec SPEC-289 with root cause and regression test plan
- [ ] Deterministic regression test that fails on dev
- [ ] worktree list runs under the worktree-registration lock
- [ ] Docs: SPEC-050 CI-109, changes.md
- [ ] Independent review answered and gate green
- [ ] Merged into dev on green CI, #666 closed with summary

## Acceptance criteria

- `git_ops.rs::a_ci_checkout_never_lists_during_another_checkouts_add` fails on `dev` with the
  `commondir` error of #666 and passes with the fix.
- A unit test in `runner.rs` shows `worktree list` takes the lock and other commands do not.
- `ci_checkouts_created_at_once_all_succeed` and the full gate (`scripts/dev ci`) stay green.

## Test plan

- **Regression (integration, `yulius-git/tests/git_ops.rs`).** The adapter's Git is a wrapper
  script around the real `git`. On the first `worktree add` it writes the half-written state Git
  leaves mid-add (`.git/worktrees/zz/` with `gitdir` and an empty `commondir`), then waits until a
  `worktree list` has finished (a marker the wrapper writes after the real `list`) or 2 s have
  passed, removes `zz` and runs the real add. A second thread starts its own `ci_checkout` once the
  first add has written that state (wait on the marker, bounded). Before the fix the second
  checkout's `list` runs against `zz` and fails deterministically; with the fix it waits for the
  lock, so the first add's wait ends by its bound and both checkouts succeed. The bound only
  limits how long the fixed code waits; it never decides the result before the fix.
- **Unit (`runner.rs`).** The predicate that selects locked commands includes `worktree list` and
  excludes `rev-parse` and `worktree lock`.

## Decisions

- **Lock `list` per command instead of holding the lock across list/prune/add.** Taking
  `WORKTREE_CHANGES` for `worktree list` in `Git::run` removes every in-process overlap of a list
  with a registration change, for `ci_checkout` and every other caller of `worktree_list` (task
  worktrees, disk upkeep), in one line. Holding the lock across the whole block in `ci_checkout`
  would need a re-entrant lock or a second code path through `run` and would also serialize the
  `remove_dir_all` of a broken checkout, for no extra safety: the steps between list and add only
  touch the job's own path. A retry of `list` on "failed to read .git/worktrees/" was rejected as
  the main fix: it depends on Git's wording and covers neither file names nor states not yet seen.
- **Out of scope.** Git run outside this process (an agent's own `git worktree list` in its
  worktree) can still meet a half-written registration; Yulius cannot lock it, and only Yulius's
  own CI checkouts were seen failing. Other Git commands that enumerate worktrees (`branch -D`,
  `checkout` of a branch) were never seen failing this way and stay unlocked; locking every Git
  command of a repository would serialize all of Yulius's Git work for a race not observed there.
- No ADR: this applies the existing SPEC-021 lock decision to one more command.
