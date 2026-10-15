# 0119. A CI job keeps its failing tests' output from all it printed

## Status

Accepted. Specified in [SPEC-145](../../.ai/specs/SPEC-145-fix-460-485-ci-keeps-a-failed-test-s-out.md);
task #128, #460 and #485.

## Context

A CI job's stored output is the end of what it printed (512 KiB), and `ci_status` shows the last
16 KiB of a failed job. Test runners print a failing test's output when it fails: nextest prints
`FAIL [ 0.547s] (290/1670) …` and the test's stdout and stderr then, and its final summary only
names the failed tests. In this repository's `quality` job (about 1700 tests, then other checks)
the block of an early failure is gone from those 16 KiB and can leave the 512 KiB too; the job's
log file is removed when the job ends, and `junit.xml` stays only in the lane's checkout until the
next run. Two flaky tests (#460, #485) failed once in CI and their assertion messages were lost.

## Decision

- While a CI job runs, the bytes Yulius already reads from its log also feed a failure scanner
  (`yulius-domain::ci::failures`): it keeps each failing test's block, nextest's (a status line
  with `FAIL`, a signal, `TIMEOUT` or `ABORT` before a bracketed duration, and the indented output
  after it) and libtest's (`---- name stdout ----` until `failures:` or `test result:`).
- Bounded: 8 KiB per test (its end, where the panic is), 32 KiB per job, further failing tests
  counted; lines are cut at 4 KiB. Blocks without output (nextest's summary lines) are not kept.
- Kept in `ci_jobs.failures` when the job ends (migration 0029), returned with the job's output in
  the run's detail, and shown by `ci_status` with `run_id` before the end of the output.

## Alternatives

- **Keep `junit.xml`.** Precise, but only nextest and Playwright write it, its path is the
  project's (a new setting per job), and reading it needs an XML parser.
- **Keep the whole log of a failed job.** Up to 256 MiB per job on disk, still to be searched for
  the failure, and agents have no file access to it.
- **nextest's `failure-output = "immediate-final"`.** Repeats failures at the end of the run, but
  only for this repository's nextest, and the 16 KiB tail still cuts a long block or what other
  checks print after the tests.
- **A larger tail.** Moves the limit without removing it, and costs every `ci_status` reader.

## Consequences

- Any project whose tests run under nextest or libtest gets readable failures without
  configuration; other runners' failures are still only in the end of the output.
- The scanner reads what Yulius reads: when a job writes faster than it is followed (over 2 MiB
  behind), the skipped bytes are not scanned either.
- A line that looks like a nextest failure status line inside other output starts a block; it
  costs at most the bounds above.
