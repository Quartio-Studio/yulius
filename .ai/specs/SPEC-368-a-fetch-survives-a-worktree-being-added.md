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
- `git_ops.rs::a_fetch_retry_waits_for_this_process_s_worktree_add` shows no retry starts while
  this process's own `worktree add` runs.
- Unit tests in `runner.rs` show which stderr texts count as a half-added worktree and that the
  subcommand is read past `-c` pairs.
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
- **Lock wait (integration).** A `worktree add` through a wrapper holds the registration lock
  until a second fetch attempt shows up or 2 s pass, then completes the half-added `HEAD`; the
  fetch's first attempt fails meanwhile, and a later attempt that starts while the add runs marks
  an overlap. Removing the lock wait makes it fail.
- **Unit (`runner.rs`).** The subcommand is read past `-c` pairs. The stderr predicate matches Git's message for a worktree name with `@`,
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
  attempt keeps its own command timeout (the network timeout), and each wait for the registration
  lock is bounded by the holder's own command timeout (120 s by default; `std::sync::Mutex` is not
  fair, so a fetch may wait behind several adds in a row). The worst case is therefore 5 network
  timeouts plus 1.5 s plus 4 lock waits, as for every command already queued on that lock. In
  practice a fetch meets this failure only after it has stored the objects it received (probed on
  Git 2.43.0), so each retry is quick. One deadline across attempts was considered and not taken:
  it would need a second timeout rule for one rare path, and an attempt cut short would turn a
  recoverable fetch into a timeout.
- **The subcommand is read past leading `-c` pairs**, so a CD mirror's fetch (`-c
  transfer.fsckObjects=true -c fetch.fsckObjects=true fetch …`) is covered too. The worktree
  registration predicate keeps reading the first argument: every worktree command starts with it.
- **Out of scope.** Agents' own fetches meet the same window; their Git is not Yulius's adapter.
  Other commands that read every worktree `HEAD` (`gc`, `repack`, `rev-list --all`) were never
  seen failing this way.

## Evidence

- **Reproduction (Git 2.43.0).** A clone with a commit to fetch and a planted
  `.git/worktrees/build@1/` (null-id `HEAD`, `commondir`, `gitdir`) fails `git fetch --quiet
  --prune origin` with exit 1 and exactly `fatal: bad object worktrees/build@1/HEAD` / `error:
  …/origin.git did not send all necessary objects`; the fetched commit is in the clone afterwards
  (`git cat-file -t` answers `commit`), so a retry has nothing left to download.
- **Fails before the fix.** At b74d37c7 (tests only), `cargo nextest run -p yulius-git --test
  git_ops -E 'test(a_fetch_)'` failed both new tests: `a_fetch_survives_a_worktree_being_added_beside_it`
  with `Failed { command: "fetch", code: Some(1), stderr: "fatal: bad object
  worktrees/build@1/HEAD\nerror: …/remote.git did not send all necessary objects" }`, and
  `a_fetch_retries_only_a_half_added_worktree_and_gives_up` with 1 attempt instead of 5. The unit
  test `the_subcommand_comes_after_the_configuration_pairs` did not compile before `subcommand`
  existed (6b691bc9).
- **Lock wait is proven.** With the wait for `WORKTREE_CHANGES` removed (mutation, not committed),
  `a_fetch_retry_waits_for_this_process_s_worktree_add` fails ("a fetch retry ran while this
  process's worktree add was still running"); restored, it passes.
- **Passes after.** At 93c0e356, `cargo nextest run -p yulius-git` passed 85 of 85; `cargo clippy
  -p yulius-git --all-targets -- -D warnings` is clean. `local_ci::running_ci_jobs_weigh_on_their_sandbox_until_they_end`
  (the test of #995) passed at bcc042cc.
- **Full gate.** `scripts/dev ci` at 10a53cc8 exited 0: 2932 tests passed, 16 skipped, lint, deps,
  macOS build and docs green. The later commits (review answers) are rechecked by Yulius CI on the
  pull request head.
- **Security.** Vidoc is turned off for this project in Yulius: the scan reports itself disabled,
  which is not a pass.
- **Review.** An independent read-only review in a fresh session approved 10a53cc8 with no blocker
  or major finding. Its minor findings are answered: the CD mirror's `-c … fetch` is now recognised
  (`subcommand`, unit-tested); the lock wait has a test that fails without it; the real worst-case
  time (attempt timeouts and lock waits) is stated under Decisions and in ADR-0249; the Plan is
  kept current. Its nits: the wrapper now completes the `HEAD` with the clone's own commit; the
  give-up test's 1.5 s of real backoff is accepted.
- **Not tested here.** Agents' own fetches (their Git is not Yulius's adapter; out of scope). The
  original race under real load is shown only by deterministic tests, since it is too rare to hit
  on demand.
