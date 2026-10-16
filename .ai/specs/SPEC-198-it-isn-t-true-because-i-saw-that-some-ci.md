---
yulius-task: b5ceff68-9e19-4454-9681-e28c9bc6cb7f
status: in-progress
---

# SPEC-198: A running CI run's commit status counts the jobs that already ended

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #184
Scope authorization: task #184 (requested by MrFishPL), about the Superagent's answer "None. As
far as I can tell, Vidoc hasn't scanned a single PR yet": "It isn't true because I saw that some
CIs already got scanned. Find a root cause of this answer and fix". Fixes #604.

## Goal

The `yulius/ci` commit status of a running run says how many of its jobs have passed and how
many ended skipped so far. Agents and the Superagent read that status from GitHub, so they no
longer report a job that already ended, such as PR #600's real Vidoc scan, as still running.

## Context

- **Symptom.** Around 12:00Z on 2026-10-16 the Superagent was asked "how many prs were already
  checked by Vidoc". It read the commit statuses of PRs #462 to #603 and answered "None".
  - It listed #600, #602 and #603 as "Security job: still running".
  - It cited the open issue #470, "Record the first live Vidoc scan…".
- **What was true.** Run `521ba595` (PR #600, `369b3d9`) had already finished its `security` job
  at about 11:50Z. The job sent 45 files in 23 chunks to vidoc-1, found 10 medium or low
  findings and passed. That was the first live scan (SPEC-195).
- **Root cause.** `App::ci_execute_inner` (`crates/yulius-app/src/ci/exec.rs`) calls
  `ci_publish` at two moments only: when the run moves from queued to running, and when the run
  ends. Between them, `ci_job` records each job's end in the store and publishes nothing.
  - GitHub therefore kept `pending | Running on Yulius: 0 of 3 jobs passed` from 11:46:56Z to
    12:02:50Z.
  - `dom::commit_status` already writes `1 of 3 jobs passed, 1 skipped` (SPEC-162), and its unit
    test covers that text. Nothing ever published it.
- **Why the tests missed it.** `local_ci.rs` checks only the last status, after the run has
  settled. No test looks at a status while a run is going.
- **Contributing stale record.** Issue #470 stayed open after PR #600 delivered what it asked for.
  It is now closed, with run `521ba595` as the evidence.
- **What else the cause affects.** Every reader of a running run's status sees a stale count:
  the pull request page, agents using `gh`, and the Superagent. Yulius's own merge gate is not
  affected, because it reads the store (`ci_gate_result`).

## Requirements

- **RCS-001.** When a job ends passed or skipped while its run is still running and nothing has
  stopped the run, Yulius publishes the run's commit status again. The new status counts the
  jobs that have passed and those that ended skipped so far.
- **RCS-002.** A job-end publish never lands after the run's final status. Each job publishes
  inside its own job task, and the run awaits every job task before it records its end and
  publishes the final status.
- **RCS-003.** Some job ends publish nothing extra, and the run's end publishes the outcome, as
  before:
  - a job that failed, errored or was cancelled;
  - a run being stopped: a sibling failed, the run was superseded or stopped (its own cancel
    signal is checked too), or the disk quota was crossed;
  - the last job to end, because the run's end follows at once.
- **RCS-004.** The statuses published as jobs end use none of the final status's retries.
  `status_attempts` counts the failures since the last status GitHub took: a published status
  resets it to 0. A final status that GitHub refused is then retried by `ci_retry_statuses`
  (up to `STATUS_MAX_ATTEMPTS`), as before this change.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- On a run with a passing job `check`, a skipped job `scan` and a job `held` that waits for a
  file, GitHub gets `pending | Running on Yulius: 1 of 3 jobs passed, 1 skipped` while `held` is
  still running.
- After `held` ends, the last status is `success | Passed: 3 jobs, 1 skipped …`, and no
  `2 of 3` status was published before it.
- If GitHub refuses that final status, one `ci_retry_statuses` call later publishes it.
- The existing `local_ci` tests still pass, including those for failure, timeout, disk wait and
  supersede.

## Test plan

- Regression test `local_ci::a_running_run_s_status_counts_the_jobs_that_ended`, using the real
  PTY host and the fake GitHub:
  - `held` loops until a release file exists, so the test waits on a condition, not a sleep.
  - The test waits until the published status says `1 of 3 jobs passed, 1 skipped`, then creates
    the release file.
  - It then checks the final status.
  - On dev it fails: the only pending status is `0 of 3 jobs passed`.
- Regression test `local_ci::a_refused_final_status_is_retried_after_the_job_statuses`:
  - Same jobs. Once the count is published, the fake GitHub refuses statuses, `held` is
    released and the run settles, so its final status is refused.
  - Statuses are then allowed again, and `ci_retry_statuses` must publish `success`.
  - Before RCS-004 this fails: the job-end publishes had used up the 5 attempts.
- Gates: `scripts/dev ci`, plus `scripts/dev security`, which runs the change scan on vidoc-1.

## Decisions

- **Where to republish.** Republish inside the job's task in `ci_execute_inner`, after `ci_job`
  returns, rather than inside `ci_job`.
  - That task is the one the run awaits before it publishes the final status, which gives the
    ordering of RCS-002 with no lock.
  - It is also where the run already decides whether to stop the sibling jobs.
  - Publishing from the store layer instead, on each job's state change, would race the final
    publish.
- **Two jobs ending at once.** Each publish reads the jobs from the store when it builds the
  status, so without care two publishes could land in either order. GitHub would then show a
  lower count until the next publish, which may be the run's end. One `tokio` mutex per run
  makes the job-end publishes take turns, so the count on GitHub never goes back.
  - Rejected: no lock. The count could show 1 of 3 although 2 had ended.
  - Rejected: a lock in `ci_publish` for every caller. It is wider than this fix needs.
- **Volume and retries.** A run publishes at most one extra status per job, and none for its last
  job. GitHub allows 1000 statuses per commit and context.
  - Every publish used to add an attempt, successful ones included. With job-end publishes, a
    3-job run reached the limit of 5 before its final status, so a refused final status was
    never retried (review round 1).
  - Chosen: reset the count on every published status. It now bounds failures in a row, which
    is what a retry limit is for. A merge train or a playground records a note and never a
    published status, so its bound is unchanged.
  - Rejected: not counting job-end publishes. A job-end publish that fails must still count.
- **A known race, pre-existing.** The retry loop shares no lock with the executor (review
  finding 4). A retried pending status can still land after the final status. It no longer
  sticks, though: the stale `status_for` makes the next retry tick publish the final status
  again, and since RCS-004 that retry has attempts left.
- No ADR: this makes the existing commit-status contract (SPEC-162, ADR-0133) true while a run
  is going, and does not change it.
