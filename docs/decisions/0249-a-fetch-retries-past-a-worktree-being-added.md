# 0249. A fetch retries past a worktree being added

## Status

Accepted. Specified in [SPEC-368](../../.ai/specs/SPEC-368-a-fetch-survives-a-worktree-being-added.md);
task #351, fixes #995. Sits beside the worktree-registration lock of SPEC-021 and
[SPEC-289](../../.ai/specs/SPEC-289-ci-checkouts-serialize-worktree-list-pru.md), which it leaves
unchanged.

## Context

`git worktree add` writes the new registration's `HEAD` as the null object id and sets the real
commit a moment later. A fetch that receives objects checks connectivity against every ref,
including every linked worktree's `HEAD`, and fails in that moment with `fatal: bad object
worktrees/<name>/HEAD` / `… did not send all necessary objects`. Yulius fetches into repositories
whose worktrees are added all the time: a sandbox's clone gets a CI job checkout per job, task
worktrees share the repository, and agents add worktrees with their own Git. A local CI test saw
it under load (#995); production sandbox syncs and integration merges are exposed the same way.

## Decision

- `Git::run` (`crates/yulius-git/src/runner.rs`) runs a `fetch` again when it failed and its
  standard error names a linked worktree's `HEAD` as a bad object (`bad object
  worktrees/<name>/HEAD`, `<name>` without `/`).
- At most 5 attempts in all. Before each retry it waits 100, 200, 400, then 800 ms (1.5 s at most),
  then for any in-process worktree registration change (`WORKTREE_CHANGES`) to end, so an add by
  this process is finished before the next attempt.
- Every other failure, of a fetch or any other command, returns at once. A fetch still failing
  after the last attempt returns that attempt's output, so a `HEAD` that stays broken is reported
  with Git's own message.

## Alternatives

- **Hold the registration lock (or the shared side of a per-repository read/write lock) for the
  whole fetch.** No retry, but every CI checkout and task worktree would queue behind fetches that
  may last up to the network timeout, and agents' adds outside the process would still break
  fetches.
- **Retry in each caller (sandbox sync, integration merge, bundle import, CD mirror).** The same
  rule repeated in several places, and new fetch paths would miss it.
- **Make the one test wait for its jobs' checkouts.** Hides the test's symptom and leaves
  production exposed.
- **Ask Git to skip the connectivity check.** `git fetch` has no option for it, and skipping it
  would also skip a real integrity check.

## Consequences

- A fetch overlapping an add now succeeds, from Yulius's own adds and from agents' adds alike.
- A fetch that hits the window waits up to 1.5 s of backoff more, plus each extra attempt's own
  run (bounded by its own command timeout) and each wait for the registration lock (bounded by the
  holder's command timeout, 120 s by default; the lock is not fair, so it may queue behind several
  adds). The worst case is 5 network timeouts, 1.5 s and 4 lock waits; in practice a retry is quick,
  since the failed attempt already stored the objects. A permanently broken worktree `HEAD` costs
  5 attempts before its error.
- The fetch is recognised past leading `-c` pairs, so a fetch written with them (a CD mirror's) is
  covered too; a bare mirror has no worktrees, so it does not meet this failure today.
- The retry depends on Git's message; a future Git wording falls back to today's behavior (one
  attempt, the error returned), never to hiding another failure.
- Commands other than fetch that read every worktree's `HEAD` (`gc`, `repack`) are unchanged.
