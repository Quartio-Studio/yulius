# 0358. Tests run in shards, and a run starts from the build it builds on

## Status

Accepted. Specified in [SPEC-497](../../.ai/specs/SPEC-497-an-armed-pull-request-merges-in-about-on.md)
slice D (task #479), SHR-001 and SHR-002.

## Context

The request asks for shorter runs: shards side by side, a candidate reusing its members' builds,
and the gain measured. A run's critical path is its longest job.

- In this repository that is `quality`, one job running every gate in a row: doctor, check,
  lint, macOS check, tests, dependencies and docs. It took 12 min 34 s on the run of #1381, and
  18 min 7 s on #1378's.
- Projects can already split a job: `matrix = "1/2 2/2"` makes one job per value, each with its
  own persistent checkout, an equal share of CI's budget and `$YULIUS_CI_MATRIX`. Each job has a
  persistent checkout per lane (`ci/jobs/<job>[.<n>][@<lane>]`), and its `target/` is the build
  cache.
- A run took the lowest free lane. A merge candidate's run therefore started from whatever that
  lane last built, often not its members' heads. So did a pull request's next run.

## Decision

- **A job is split into shards with a matrix of `i/n` values** (SHR-001).
  - Yulius already runs each value as its own job, side by side where the budget allows, and a
    run passes only when every value passed. `local_ci::a_sharded_job_runs_its_shards_side_by_side`
    proves it with shards that wait for each other.
  - The shard is the project's to read: `cargo nextest run --partition hash:$YULIUS_CI_MATRIX`,
    `pytest --splits`, and so on. Operations says how.
  - This repository runs its tests that way. `quality` runs `scripts/dev checks` (every gate but
    the tests), and a `test` job with `matrix = "1/2 2/2"` runs `scripts/dev test`, which reads
    `i/n` as a nextest hash partition and runs the doctests in shard 1. `scripts/dev ci` still runs
    everything in one go locally.
- **A run takes the lane whose checkouts last built what it builds on** (SHR-002).
  - The dispatcher remembers each lane's last run. A merge candidate's run prefers a free lane
    whose last run was one of its members' own runs (the last member first), then one whose last
    run was a candidate sharing a member. A pull request's run prefers the lane of its own
    previous run. With none free, it takes the first free lane as before.
  - Each job's output says which build its checkout starts from ("lane 1, last taken by pr/12,
    which this run builds on"), so durations can be compared by it.

## Alternatives

- **One build, many test runners** (`cargo nextest archive` once, shards from the archive): saves
  each shard's compile, but needs a job that waits for another job's artifact, which Yulius CI
  has no notion of. Each shard's own persistent checkout builds incrementally instead.
- **Shards Yulius makes by itself** (a `shards = n` key): the split is the test runner's, and
  `matrix` already carries it. A key would only rename it.
- **A shared build cache across lanes** (sccache or one `target/`): one shared cache was relinked
  under another lane's running job (SPEC-351). Lanes keep their own; the run picks the lane.
- **Copy a member's `target/` into the candidate's checkout**: gigabytes per run, and the lane
  that built it is usually free to take as it is.

## Consequences

- This repository's run has five jobs instead of four. Each default share is a fifth of CI's
  budget instead of a third, and the run's critical path is the longest of `quality` without
  tests, a shard (its build and half the tests), and `e2e`.
  - Locally on 64 CPUs, the tests took 161 s in one go, and 91 s for each of two hash
    partitions run alone.
  - Measured on the instance: the run durations of pull requests before (#1378 18 min 7 s, #1381
    12 min 35 s) and after this change, read from `ci_status` (slice E).
- A shard builds the tests in its own checkout, so two shards compile them twice. The cost is CPU
  the budget admits, not wall time.
- A candidate's build starts from a member's when that member's lane is free. On this
  repository's test build (`cargo nextest run --no-run`), building `dev` took 105 s after an
  unrelated older build (as long as a cold one) and 87 s after a member's, 17 % less (SPEC-497's
  Evidence, slice D). Most of this workspace's test binaries depend on whatever crate a change
  touches, so the gain is part of the build; a project whose changes touch fewer crates gains
  more.
- The lane memory is in the process: after a restart the first runs take the first free lanes as
  before.
