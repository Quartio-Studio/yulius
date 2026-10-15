# 0134. A playground's CI tests the task head merged into its integration branch, built in Yulius's clone

## Status

Accepted. Specified in [SPEC-163](../../.ai/specs/SPEC-163-fix-411-yulius-ci-runs-the-gate-on-a-pla.md);
amends [0081](0081-a-playground-keeps-its-git-on-the-server.md) (decision 4: "there is no CI in a
playground").

## Context

Yulius CI follows GitHub pull requests: each open pull request head and the integration and
production heads get a run, and `delivery_merge` merges on green. A playground (ADR-0081) has no
GitHub and no pull request, so its CI page stayed empty and `delivery_merge` squashed at once, with
only the agent's own gates behind it (#411). A playground task's branch is never pushed anywhere:
it lives in the sandbox clone's worktree, and only the squash reaches the playground's `origin`.

## Decision

1. **The sync loop tests a playground's integration head** with the CI in force there, once per
   commit, as for a GitHub project, without a token or a GitHub call. The production branch is
   not tested: only a promotion moves it, and a playground has none.
2. **`delivery_merge` tests the task head merged into the integration branch as it is now.**
   Yulius builds that merge in its clone with the merge train's builder (SPEC-084; merge drivers
   apply, nothing is pushed) as a merge commit, and queues a run of it with the reference
   `task/<n>`. Its parents name what it tests (the integration head, then the head or Yulius's merge
   of the integration head into it), so the run of a head is matched exactly, never by ancestry
   (a branch reset to an older commit would take a later commit's verdict). Once it passed, the
   squash lands exactly the tested commit's tree (Git's squash of the head can order a
   `merge=union` file the other way), so the integration head after the merge reuses the result
   (`ci_reuse_tree`).
3. **The verdict decides as on GitHub.** Passed: squashed now. Failed or cancelled: refused (merge
   on green only). Queued, running or errored: armed, and Yulius decides it when a run of the
   sandbox ends: it squashes once the run passed, drops it with why when it failed or conflicts,
   and builds and tests a new merge when the integration branch moved. A person's merge applies the
   same verdict without arming, unless the profile requires no checks (as GitHub's merge gate). A
   playground with no CI squashes at once: nothing will ever report.
4. **Nothing is published:** a playground's run records "not published: a playground has no
   GitHub repository".
5. **One decision of a playground's merges at a time:** the agent's call, a person's merge and the
   armed-merge pass hold the sandbox's armed-merge pass lock, so builds never share the scratch
   checkout.

## Alternatives

- **Test the task head alone.** A branch behind the integration branch would pass and then break
  once merged; the issue asks for the head merged into it.
- **Bring the task branch up to date in the agent's worktree first** (GitHub's DLV-203). It
  changes the agent's worktree under it and needs a fetch and push path a playground does not have,
  for the same tree the built merge gives.
- **A reference per head** (`task/<n>/<sha>`). It names the run directly, but retention keeps the
  newest verdict of every reference, so those runs would never be pruned.
- **A local forge emulating pull requests** (rejected in ADR-0081 for the same reasons: far more
  code, and it pretends pull requests exist).

## Consequences

- A playground's CI page shows its `dev` runs and its tasks' `Task #n` runs; `ci_status`,
  `ci_rerun` and `yulius wait` work for its agents, and their briefing and definition of done say
  so.
- Each delivery costs one CI run plus none for the integration head after it (same tree), as on
  GitHub.
- The candidate commit exists only in Yulius's clone; a restart keeps the run (the store) and the
  armed merge (`armed-merges.json`), and the next pass finds the run again by its parents.
- The scratch checkout `ci/candidate` costs one checkout of the repository per playground with CI.
