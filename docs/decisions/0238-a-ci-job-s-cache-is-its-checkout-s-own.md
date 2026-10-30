# 0238. A CI job's cache is its checkout's own

## Status

Accepted. Specified in [SPEC-351](../../.ai/specs/SPEC-351-fix-so-it-won-t-happen-again.md)
(CCH-001..004); task #333, #963. It refines [0015](0015-yulius-ci.md) (persistent caches) and the
lanes of SPEC-050 CI-106.

## Context

Yulius CI keeps one checkout per job, matrix position and lane (`ci/jobs/<job>[.<n>][@<lane>]`),
so two runs at once never share a checkout's ignored `target/`. `YULIUS_CI_CACHE` was one directory
per sandbox (`ci/cache`), shared by its lanes and by the parallel jobs of one run, and Yulius's own
guidance (`ci_setup`, docs/api.md, docs/operations.md) told projects to keep caches there.
pomerado-rust did: `CARGO_TARGET_DIR=$YULIUS_CI_CACHE/cargo-target`. Two lanes testing different
pull requests then wrote the same test binaries with different code; Cargo's lock covers the build,
not the execution, so a run executed or listed a binary the other had just relinked or removed
("never executed: No such file", pomerado-rust #460; PR #445 failed 3 of 6 quality runs). It
could as well have tested the other pull request's code and passed.

## Decision

- A job's `YULIUS_CI_CACHE` is `ci/cache/<checkout>`, named like its checkout: exactly as exclusive
  as the checkout (one job at a time) and as persistent (between runs of that job and lane).
- `HOME` stays shared by the lanes; installers that write there lock their own writes. Build
  outputs belong in the checkout or the cache, never in `HOME`.
- The disk upkeep prunes a job's cache whole, like the caches in its checkout: at once past a
  quarter of the quota, otherwise under pressure, never while a CI run executes (SPEC-008 NFR-009).
  A Cargo target directory is best in the checkout, whose unused build units the hourly sweep
  removes (ADR-0205); the guidance (onboarding prompt, `ci_setup`, infra-bootstrap) says so.
- An entry of `ci/cache` is a stale CI leftover by the stale-checkout rule (no run used a job of
  that name for 14 days, or a lane past the configured ones), removed under the CI claim like a
  checkout. The old shared cache's contents are such entries, so they go without a migration. A
  file or link where a job's cache goes is removed (never followed) before the directory is made.

## Alternatives

- **A lock held across a whole job on the shared cache.** Serialises the lanes, which exist to test
  two pull requests at once, and a job that never touches the cache would wait too.
- **Fix pomerado-rust only** (a target directory in its checkout). The next project that follows
  Yulius's own advice hits it again, and the advice stays wrong.
- **A per-run cache.** Never shared, but it persists nothing: no cache at all.
- **A per-lane cache shared by a run's jobs.** Still shared by a run's parallel jobs and matrix
  siblings, which build the same project at once.

## Consequences

- A lane's cache warms separately: the first run of each job and lane after the update starts
  cold, and the disk holds one cache per checkout. Before, the shared cache was outside the disk
  upkeep altogether; now each is capped and pruned, but whole: a project that keeps a Cargo target
  directory there rebuilds it cold after a prune, where one in the checkout loses only unused units.
- Existing contents of the shared cache stay until the automatic leftovers round removes them as
  stale (a sandbox with CI runs in the last 14 days), or until a person removes `ci/` as the
  operations guide says.
