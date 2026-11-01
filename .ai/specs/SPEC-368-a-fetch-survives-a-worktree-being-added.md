---
yulius-task: 43c627d9-ed04-4265-bc69-a84cf84acc0a
status: in-progress
---

# SPEC-368: A fetch survives a worktree being added beside it

Status: in-progress
Date: 2026-11-01
Owner: task #351
Scope authorization: task #351: "Solve GitHub issue #995 … in one pull request whose description
says `Fixes #995`". Fixes #995. Extends the worktree-registration lock of
[SPEC-289](SPEC-289-ci-checkouts-serialize-worktree-list-pru.md) (SPEC-021, CI-109 of
[SPEC-050](SPEC-050-parallel-ci-and-gentle-delivery.md)) to fetches; decision in
[ADR-0249](../../docs/decisions/0249-a-fetch-retries-past-a-worktree-being-added.md).

## Goal

A fetch into a repository never fails because a worktree of that repository is being added at the
same moment, whether the add is Yulius's own (a CI job's checkout, a task worktree) or an agent's.

## Context

Confirmed against `dev` at 50f1faf5 with Git 2.43.0.

- **Symptom.** `local_ci::running_ci_jobs_weigh_on_their_sandbox_until_they_end` failed once in a
  full gate (#995): `git fetch failed (exit Some(1)): fatal: bad object worktrees/build@1/HEAD` /
  `error: …/origin.git did not send all necessary objects`.
- **Code path.** The CI poll's sync of a sandbox (`ci_sync_sandbox`, `crates/yulius-app/src/ci/mod.rs`)
  refreshes the sandbox clone (`dlv_refresh_clone` → `sync_clone` → `Git::fetch`,
  `crates/yulius-git/src/repo.rs`), while a CI job's `ci_checkout` (`crates/yulius-git/src/ci.rs`)
  runs `git worktree add --detach jobs/build@1` in the same clone. The test waits for 3 running
  jobs and then fetches; a job counts as running before its checkout, so the third add can overlap
  the fetch.
- **Root cause.** `git worktree add` writes the new registration's `HEAD` as the null object id and
  points it at the real commit only afterwards (a `git update-ref` / `symbolic-ref` child). A fetch
  that received objects checks connectivity with `rev-list --objects --stdin --not --all`, and
  `--all` includes every linked worktree's `HEAD`; a null `HEAD` there makes the fetch fail with
  exactly the message above. `WORKTREE_CHANGES` (`crates/yulius-git/src/runner.rs`) serializes
  worktree add/move/remove/prune/list, never fetch.
- **Reproduction (Git 2.43.0).** A planted `.git/worktrees/build@1/` with a null-id `HEAD`,
  `commondir` and `gitdir` fails `git fetch --prune origin` every time a commit is to be fetched
  (task #348 and again here). Task #348's add/remove loop beside push+fetch failed 19 of 77 fetches.
- **Why the tests missed it.** The window is the few milliseconds between Git writing the null
  `HEAD` and its child setting the commit; only a loaded full suite stretched it into the fetch.
- **Also exposed.** Every fetch of a repository with linked worktrees: the CI poll's
  `ci_sync_all` (logged and retried at the next poll), `merge_integration`'s fetch in a task
  worktree (task worktrees share the repository), bundle imports and CD mirror fetches, and the
  agents' own fetches, which Yulius does not run.

## Requirements

| ID | Requirement |
| --- | --- |
| FWA-001 | A fetch run by Yulius's Git adapter whose failure names a linked worktree's `HEAD` as a bad object (`bad object worktrees/<name>/HEAD`) is run again, at most 5 attempts in all, after a backoff of 100, 200, 400 and 800 ms (1.5 s at most) and after any in-process worktree registration change has ended. |
| FWA-002 | Any other fetch failure, and any failure of another command, is returned at once, as before. |
| FWA-003 | A fetch still failing after the last attempt returns that attempt's error unchanged (a genuinely broken worktree `HEAD` is reported, not hidden). |

## Plan

- [x] Spec and ADR for the fetch retry
- [x] Regression test reproducing the half-added worktree
- [x] Fetch retries past a worktree add in progress
- [x] Docs and changes entry
- [ ] Full gate green
- [ ] Independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- `git_ops.rs::a_fetch_survives_a_worktree_being_added_beside_it` fails without the fix with the
  `bad object worktrees/build@1/HEAD` error of #995 and passes with it.
- `git_ops.rs::a_fetch_retries_only_a_half_added_worktree_and_gives_up` shows a `HEAD` that stays
  bad fails after exactly 5 attempts with Git's error, and a fetch from a missing remote is
  attempted once.
- A unit test in `runner.rs` shows which stderr texts count as a half-added worktree.
- `scripts/dev ci` stays green, including the `local_ci` test of #995.

## Test plan

- **Regression (integration, `yulius-git/tests/git_ops.rs`).** A clone has a commit to fetch and a
  planted half-added registration `.git/worktrees/build@1/` (null-id `HEAD`, `commondir`,
  `gitdir`), the state `git worktree add` leaves for a moment. The adapter's Git is a wrapper
  around the real `git` that records each fetch's exit status and, after the first fetch, writes
  the real commit into that `HEAD`, as the add's own child does. Without the fix the first
  (failing) attempt is the result; with it the second attempt succeeds and the remote branch is
  fetched. Deterministic: no timing decides the result.
- **Bounds (integration).** The same wrapper without the completion: the fetch fails after 5
  attempts with Git's message; a fetch from a missing remote is attempted once.
- **Unit (`runner.rs`).** The stderr predicate matches Git's message for a worktree name with `@`,
  and not a bad object elsewhere (`bad object HEAD`, `refs/heads/x`) or a nested path.

## Decisions

- **Retry the fetch, not lock it** ([ADR-0249](../../docs/decisions/0249-a-fetch-retries-past-a-worktree-being-added.md)).
  A retry on Git's exact message covers adds in this process and outside it (agents' Git in the
  sandbox, which Yulius cannot lock). Taking `WORKTREE_CHANGES` (or a shared side of a per-repo
  lock) for the whole fetch would make every CI checkout wait behind a network fetch of up to the
  network timeout, and still miss agents' adds. Waiting for the lock before a retry is kept as a
  cheap addition: it ends an in-process add before the next attempt.
- **In `Git::run`, for every `fetch`.** Every fetch path (sandbox sync, task worktree, bundle
  import, CD mirror) gets it in one place, as the registration lock does.
- **Bounds.** 5 attempts and 1.5 s of backoff: the window is milliseconds; several adds in a row
  (a CI run's jobs) each reopen it briefly, and the backoff lets the fetch fall between them. Each
  attempt keeps its own command timeout, so a retried fetch can take up to 5 timeouts in the worst
  case; a fetch only reaches this failure after it has received its objects, so in practice each
  retry is quick (the objects are already present).
- **Out of scope.** Agents' own fetches meet the same window; their Git is not Yulius's adapter.
  Other commands that read every worktree `HEAD` (`gc`, `repack`, `rev-list --all`) were never
  seen failing this way.
