# 0279. A CI job's limit is its own, and Yulius holds its one-run-at-a-time lock

## Status

Accepted. Specified in [SPEC-435](../../.ai/specs/SPEC-435-ci-a-job-s-time-limit-counts-only-its-ow.md);
task #417. Builds on [ADR-0065](0065-processes-plan-for-a-cpu-and-memory-budget.md) (a job waits
for its budget before it starts).

## Context

The two CI lanes of a sandbox share the CI `$HOME`, so a project whose job cannot run twice at
once (quartio-v2's `websites`) serialises it with a `flock` inside the job. Yulius's time limit
starts when the job's process starts, so the second run's job spent its limit waiting on that lock
(39 of 60 minutes) and timed out with no test failing. Yulius cannot tell a job waiting on its own
lock from a job working, and the limit was one instance setting no job could raise.

## Decision

- A `[[ci.jobs]]` entry may say `exclusive = true`. Yulius then runs one run of that job (each
  matrix value apart) at a time per sandbox: a later run's job waits for it in queue order before it
  takes a job slot, as part of its `wait` stage, cancellable, outside its limit. The lock is
  process-local; a restart requeues every running run.
- A `[[ci.jobs]]` entry may say `timeout_minutes = N` (1-360). It replaces the instance's limit
  (`YULIUS_CI_JOB_TIMEOUT_MIN`) for that job; without it nothing changes.
- A job's limit starts when its process starts, after every wait. Each job records how long it
  waited (`waited_ms`); `ci_status`, the run page and a timeout's reason say how long it waited and
  ran.

## Alternatives

- **The job prints a "waiting" marker and Yulius pauses its clock** (`::yulius-wait` … `::yulius-run`):
  keeps the project's own lock, but a waiting job still holds a slot and its CPU budget, the clock
  depends on what the job prints (a missed marker means no limit, so the wait needs its own cap),
  and every project re-implements the lock.
- **Only raise the limit** (`timeout_minutes`): the second job still spends its wait inside its
  limit; a 120-minute limit hides a 39-minute wait today and fails on a longer queue tomorrow.
- **One lane per sandbox, or exclusive by default**: serialises every job of every project, which
  the lanes were added to avoid (parallel tasks waited hours for each other).
- **No limit while another run of the job runs**: unbounded, and a hung job would hold the next one.

## Consequences

- A project replaces its in-job lock with `exclusive = true` (an in-job lock left in place never
  waits any more, since Yulius already serialises the job).
- A waiting exclusive job holds no slot or budget, so other jobs run meanwhile.
- A job's record has three new columns (`timeout_minutes`, `exclusive`, `waited_ms`, migration 0048).
- An instance older than this refuses a `yulius.toml` using the new keys (unknown key): set them
  after the instance runs this version.
