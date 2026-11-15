# 0343. A CI job names the files it tests, and is never skipped on doubt

## Status

Accepted (2026-11-15). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (CJP-001 to
CJP-004, fix 5); task #472. Follows [ADR-0133](0133-a-ci-job-that-checked-nothing-ends-skipped.md)
(a job that checked nothing is skipped, never passed) and the audit's rule that one function says
which jobs test a commit (`ci_gate_for_head`, #1254).

## Context

quartio-v2's `websites` job is its longest (median 15.9 min, p90 29.3). It tests only
`crates/pomerado` and the connectors, and 12 of the 80 pull requests merged 11 to 13 Nov touched
them. Every other pull request waited for it, and in a merge candidate it ran again. Nothing in the
job depends on the rest of the code.

## Decision

- **`paths` on a `[[ci.jobs]]` entry.** 1 to 50 globs in the scope claims' form (`*`, `?`,
  `**`, a trailing `/` for a folder; normalized). A malformed or repeated one is a configuration
  error naming the job.
- **Decided when the run is queued**, in the one place every run's jobs are created
  (`ci_queue_placed`, after `ci_gate_for_head` decided the jobs in force). For a pull request's
  head, a merge candidate and a playground task's head, the changed files are the head against
  its merge base with the integration branch (`base...head`). A candidate is built on the
  integration branch, so that is its members' changes (`--no-renames`: a file moved out of a
  job's paths counts there). A job whose `paths` none of them match ends **skipped** at once,
  with "no file this job tests changed (paths: …)" as its reason and output and the skip code
  78. It never starts its command, waits for no `exclusive` turn or slot, and takes no share of
  the run's budget. A run whose every job is skipped passes at once and takes no lane. A skipped
  job blocks no merge, and the run page and `ci_status` show it with its reason (ADR-0133). A
  path without a glob (`crates/web`) is that file and that folder. A requeued run (a restart,
  the disk quota) keeps the skip.
- **It runs regardless**:
  - on the integration and production branches' own runs, so every merged state and a
    promotion's head are tested by every job;
  - when the change touches `yulius.toml`, the job's own definition;
  - when the changed files cannot be told: no merge base, a failed diff, more than 20,000 files,
    a path that is not UTF-8, or one no glob can be matched against (a backslash, a control
    character).
  Never skipped on doubt.
- **A rerun decides again** with the same rule. A copied verdict (a train's, a reused tree's)
  keeps the skipped state, as for any skipped job, except into a branch's own run: a tree
  whose source skipped a job for its `paths` is run again there.

## Alternatives

- **Decide when the job starts.** The run's lane and slot would be taken first, and two places
  would decide which jobs test a commit.
- **Skip on the integration branch too.** A merge could break the job's files through a file the
  job does not name (a shared crate); only the integration branch's run would catch it.
- **A list of files that do not matter (`ignore`).** A new folder would run every job until
  someone lists it; the files a job tests are what its author knows.

## Consequences

- A pull request that touches none of a long job's files merges without waiting for it; its
  candidate skips it too when no member touches them.
- A job whose `paths` miss a dependency it really has can pass a pull request that breaks it.
  The integration branch's own run then fails, and the author widens the `paths`.
- How to read it a week later: the Pipeline tab's job stages (`ci.job` by name) per day, runs
  against skips, before and after the project sets `paths`.
