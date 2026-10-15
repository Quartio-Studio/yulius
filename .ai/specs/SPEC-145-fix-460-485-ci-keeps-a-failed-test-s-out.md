---
yulius-task: 55625998-c034-4e18-9005-eff1d729b573
status: done
---

# SPEC-145: CI keeps a failed test's output; the cockpit and local_ci flakes are fixed

Status: done
Date: 2026-10-15
Owner: Yulius task #128 (part of task #118, SPEC-138)
Scope authorization: task #128: "Fix GitHub issues #460 and #485 […] 1. When a CI job fails, the
failing tests' output stays readable afterwards through `ci_status` (for example by keeping the
job's `junit.xml` or the nextest failure section with the run, bounded, and showing it with
`run_id` + `job`). Test it. 2. Find and fix each race: reproduce under parallel load […], name the
cause, and fix it with a regression test. If one cannot be found, quarantine that test with its
issue linked and say so in the issue; never add retries."

## Goal

A test that fails in Yulius CI can be diagnosed afterwards: the failing tests' own output (their
assertion message, stdout and stderr) is kept with the job and shown by `ci_status`, however much
the job printed after it. The two flakes this gap hid, #460 (`cockpit::a_cockpit_manages_the_project_for_its_owner`)
and #485 (`local_ci::a_run_over_the_disk_quota_waits_and_starts_when_space_frees`), are found and fixed.

## Context

- A job's output is read from its log into an `OutputRing` of `OUTPUT_BYTES` (512 KiB, its end)
  and stored on the job row; `ci_status` with `run_id` shows the last 16 KiB of each failed job.
  nextest prints a failing test's output when it fails (`FAIL [ 0.5s] … stdout ─── … stderr ───`),
  and its final summary only names the failed tests. In the quality job (1670 tests, then other
  checks) the failure's block is long gone from the last 16 KiB, and can leave the 512 KiB too.
- The job's log file is removed when the job ends; `junit.xml` stays only in the lane's checkout
  until the next run overwrites it.
- `crates/yulius/tests/cockpit.rs` and `crates/yulius/tests/local_ci.rs` drive the real app with a
  fake GitHub and the fake session host; CI runs them under heavy parallel load.

## Requirements

- R1. While a CI job runs, Yulius scans its whole output (not only what the ring keeps) for failing
  tests' sections: a nextest failure (its `FAIL`/signal/timeout status line and the indented
  `stdout ───`/`stderr ───` block that follows) and a libtest failure block (`---- name stdout ----`
  until `failures:` or `test result:`). They are kept, bounded: at most 8 KiB per test (its end,
  where the panic is) and 32 KiB per job; further failing tests are counted, not kept.
- R2. The kept sections are stored with the job when it ends (a nullable column) and returned with
  the job's output (API and `ci_status` with `run_id`).
- R3. `ci_status` with `run_id` shows, for each failed job (or the job named by `job`), its failing
  tests' output before the end of its output, saying it was kept from the whole run.
- R4. #460 and #485 are reproduced under CPU load, their causes named, and fixed with a regression
  test each; the sibling disk-wait tests are checked for the same pattern. A test whose failure
  cannot be explained is quarantined with its issue linked. No retries.

## Plan

- [x] Spec SPEC-145 records the plan and the decision
- [x] A failed CI job keeps its failing tests' output (domain scanner, bounded)
- [x] ci_status shows a failed job's kept failing-test output
- [x] The cockpit flake is reproduced under load, named and fixed with a regression test
- [x] The local_ci disk-wait flake and its siblings are reproduced, named and fixed
- [x] Docs, ADR, independent review and gates are done and the change is delivered

## Acceptance criteria

- AC1. A CI job that prints a failing nextest test followed by more than 512 KiB of other output
  ends failed with the failing test's assertion message stored with the job and shown by
  `ci_status` with `run_id` (R1-R3).
- AC2. The scanner keeps nextest and libtest failure blocks, ignores passing tests' lines, cuts a
  huge block to its end and stops keeping at the job bound, counting the rest (R1).
- AC3. A passing job keeps nothing (R1).
- AC4. Each flake's cause is named in this spec and in its issue, with a test that failed before the
  fix; an unexplained one is quarantined (R4). See Findings.

## Findings

- #460, `cockpit::a_cockpit_manages_the_project_for_its_owner`. `task_create` answers while the new
  task is `Queued`; its start fails in the background (no repository in the fixture,
  `Failed(Precondition)`). The test then archived it, assuming it had ended. Under load the archive
  came first and was refused ("stop the task before archiving it"), an assertion in the test's first
  half second, as in CI (0.547 s). With a 500 ms delay before the start's launch (a temporary
  probe), the original test fails in 0.155 s with that message; the fixed one, which waits for the
  task to rest (`task_rests`), passes. 400 runs of it, 16 at a time on 12 CPUs with 12 busy loops,
  passed before and after without the probe: plain load did not reach the window here.
- #485, `local_ci::a_run_over_the_disk_quota_waits_and_starts_when_space_frees`. One race found: the
  disk check that notes the wait runs in the background and dates the next check from its own end
  (`ci_disk_checked`). Ending after the test's single +61 s clock jump, it put the next check a
  minute past the frozen clock, and the runs never started. With random delays (0-300 ms) at the
  publish, the check and a run's start and end, the test failed 13 of 15 runs; with
  `recheck_until_started` (time passes until the runs start), 0 of 15. That race only ever times
  out (60 s), while CI saw a failure after 2.16 s with no timeout, near the end
  (`all_passed`, the wait note, the final status). That failure stays unexplained: 444 runs of
  every `local_ci::` and `cockpit::` test at 32 parallel processes with busy loops, 240 runs pinned
  with 4 busy loops to 2 CPUs and two rounds of injected delays reproduced nothing else. The test is
  quarantined (`#[ignore]`, #485 linked) and #485 stays open.
- Siblings: `a_job_refused_for_disk_puts_its_run_back_in_the_queue` also jumps the clock once, but
  no background check runs there: `ci_disk_backoff` sets the deadline before the requeue note the
  test waits for. `a_run_waiting_for_disk_is_superseded_like_any_queued_run` asserts only states,
  which a late publish of the check cannot change. Both passed every round, with delays too.

## Test plan

- Unit (`yulius-domain::ci::failures`): nextest blocks (real 0.9.146 output), libtest blocks,
  passing output, per-test and per-job bounds, chunk boundaries inside a line.
- Unit (`mcp_text`): `ci_status` with `run_id` renders the kept failures before the tail.
- Integration (`local_ci.rs`): a real job through the process host prints a nextest failure and
  then 600 KiB; the run's job keeps the assertion message (AC1), a passing job keeps none (AC3).
- Store: the column round-trips; migration 0029.
- Flakes: loops of the named tests under CPU load (busy loops on every CPU, as `stress-ng` is not
  installed here) before and after each fix.

## Decisions

- #485's test is quarantined rather than left running with the race fixed: the task's rule for a
  failure whose cause is not found, and a once-in-hundreds failure that blocks other pull requests'
  merges costs more than the coverage, which its sibling (the requeue path waits and passes once
  space frees) keeps in part. Back in the suite, its next failure is readable (R1-R3).
- `ci_status` shows at most 32 KiB of failing tests across a run's jobs (review round 1): more,
  and the jobs past it point to `job`, which shows a job's own in full.
- [ADR-0119](../../docs/decisions/0119-ci-keeps-failing-tests-output-from-the-whole-job.md): scan
  the streamed output for failing tests' sections and keep them with the job, rather than keeping
  `junit.xml` (needs a per-project path setting and an XML parser; only nextest/Playwright write
  it) or the whole log (unbounded disk, and still needs a search to find the failure), or
  repeating failures at the end with nextest's `failure-output = "immediate-final"` (only this
  repository's nextest, and still cut by the 16 KiB tail).

## Evidence

- `scripts/dev ci` at the merged head (dev at #497 merged in): exit 0; nextest `Summary [ 612.598s]
  1794 tests run: 1794 passed (6 slow), 17 skipped` (the quarantined test among the skipped);
  fmt, clippy, repository checks and `cargo deny` (advisories, bans, licenses, sources ok) passed.
- `cargo nextest run -p yulius --run-ignored all -E 'binary_id(yulius::main) & (test(/^local_ci::/) |
  test(/^cockpit::/))'`: 42 passed, the quarantined test included.
- Scanner unit tests (`ci::failures::`, 9) failed against a stub `finish` returning `None`, then
  passed; the review-round tests (status alignment, libtest `successes:`) failed before their fix.
- `mcp_text` rendering test failed before the rendering (no "Failing tests" section), then passed.
- `local_ci::a_failed_jobs_failing_tests_stay_readable_after_long_output`: a real job prints a nextest
  failure and 670 KB more and fails; its output no longer holds the message, `failures` does; a
  passing job keeps none (AC1, AC3). Store test: the column round-trips and a requeue clears it.
- Flakes: see Findings (probe delays were temporary and are not in the change).
- CI run `7b347fa3` (PR #505 at `0b6558a`) failed `local_ci::a_failed_jobs_failing_tests_stay_readable_after_long_output`
  after 2.4 s: the test had a passing sibling job, which a failing job cancels (CI fails fast), so
  under load it could end cancelled, not passed. The sibling is gone; a passing job keeping no
  failures is asserted in `runs_jobs_in_parallel_publishes_statuses_and_gates_merges`. 60 runs of
  the test, 12 at a time with 12 busy loops, passed.
- `scripts/dev security`: not run, exit 2 (Vidoc rejected the key, HTTP 401; #470, ADR-0104).
- Not testable here: the real CI host's load; whether #485's fast failure recurs (quarantined).
