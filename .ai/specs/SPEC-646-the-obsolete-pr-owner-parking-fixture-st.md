---
yulius-task: 32b2a708-0a8a-481a-86bd-25403f6e73a0
status: done
---

# SPEC-646: The obsolete-PR-owner parking fixture stays within its deadline and names its stalled stage

## Goal

Fix #1649. `yulius::main parking::candidate::an_unrelated_or_merged_pr_releases_the_original_candidate_owner`
failed with "timed out waiting for the task to park" (nextest run
`2f5c9b57-8f74-4967-bb0d-75c1054e0ce8`, 625.778 s reported) although its wait has a 30 s
deadline, and the failure named neither the PR condition (fork, merged, head branch, base branch)
nor the stage that made no progress. Each case must finish within its bound and, on failure, say
which case and which stage stalled.

## Context

- `parked()` (`crates/yulius/tests/parking.rs`) waits with `wait_for`
  (`crates/yulius/tests/first_message.rs`), whose deadline is compared only between two checks.
  Each check runs a whole `App::park_idle_sessions_now()` pass, which nothing bounds.
- A park pass of this fixture (`park_task_because`, `supervisor.rs`) awaits, in order: store reads,
  `verify_parking_prs` (fake GitHub list and pull reads; the git verifications
  `verification_checkout`, `is_ancestor` and `local_conflict`), the process probe (bounded,
  3 s), the launch lock, and under it the delivery, plan, spec and PR rechecks. Each git child is
  bounded only by the adapter's `DEFAULT_TIMEOUT` (120 s). The pass then requests the session's
  shutdown; the task becomes `Parked` when its run ends.
- The pause points `ParkProbed`, `ParkPrepared` and `ParkDecided` exist (`race.rs`). Their hooks
  are one-shot and taken in registration order, so a wait cannot listen on them without taking a
  hook another test registered (`contending_merge_request` pauses at `ParkDecided` and then calls
  `parked()`).
- The four cases run one after another in one test. `with_idle_candidate` (31 callers) seeds an
  answered candidate owner, restarts the server (`reconcile`, transcript replay, `watch_prs`),
  runs the check and stops the task: none of those phases has a bound of its own.
- nextest kills a test after 4 × 30 s (`.config/nextest.toml`, default and `ci` profiles, since
  2026-10-06), so 625.778 s cannot be this test's own duration under the committed profiles; it is
  most likely the whole run's time. The failure text is all that run kept.

## Root cause

The fixture's 30 s deadline does not bound the operation it waits for. `wait_for` checks its
deadline only after a check returns, and each check is a full, unbounded park pass. Probe delays
at each await of the pass (temporary code, not committed; `target/630/probe.diff`) show every one
of them stretches the wait past 30 s, with the same message naming no case or stage:

| Await held in a fork-case pass | `parked()` returned after |
| --- | --- |
| fake GitHub branch PR list (`pull_list_hold`, 45 s) | 45.2 s |
| fake GitHub single PR read (`pull_read_hold`, 45 s) | 45.1 s |
| before the launch lock (sleep at `ParkProbed`, 45 s) | 45.2 s |
| each git verification (`verification_git`, 12 s each; four per pass) | 48.2 s |
| each git verification, 45 s each | killed by nextest at 120 s, no message |

With git children bounded only at 120 s and four verifications per pass, one slow pass can run for
minutes. Which await was slow in run `2f5c9b57` cannot be recovered from its output; the defect is
that the fixture neither bounds nor reports it. No production await is unbounded beyond its own
design (git 120 s per child, the process probe 3 s; the launch lock waits for another launch by
design), so the fix is in the fixture.

## Requirements

- FIX-001: The park wait bounds the whole wait, including a pass still running at its deadline
  (the pass is cancelled there), and fails at the deadline.
- FIX-002: Its failure names the last park stage the task reached since the wait began and
  whether a pass was still running or the last pass ended without parking. The stages come from
  a passive record of the last pause point reached per key (`race::last_reached`, test-support
  only), which takes no hook. Two new pause points mark the pass's PR verification:
  `ParkVerifying` (before `verify_parking_prs`) and `ParkPrsVerified` (after it). In production
  builds they do nothing.
- FIX-003: Each of the four PR conditions is its own test, named by its condition, so each has
  its own nextest budget and a failure names its case.
- FIX-004: `with_idle_candidate` bounds each of its seeding, restart and check phases, all three
  together by one case budget (75 s), and its cleanup by a separate bound (25 s), so a case ends
  inside nextest's 120 s kill. A failure names the case and the phase, and a failed check is
  printed before the cleanup. The cleanup still runs after a phase that timed out.
- FIX-005: Coverage of the fork, merged, head-branch and base-branch cases is unchanged; no
  retries, no hidden assertion.

## Plan

- [x] Root cause: probe each await of the park pass and the case's setup and cleanup
- [x] Regression check: a stalled pass overruns the old 30 s wait and names no case or stage
- [x] Fix: the park wait bounds each pass and names case and stage; each case's setup and cleanup are bounded
- [x] Four cases under held CPU load and every parking test in full
- [x] Spec evidence, changes entry and issue comment
- [x] Independent review in a fresh session, findings answered

## Acceptance criteria

- With a held branch PR list read, the park wait fails within its bound plus a small margin and
  names the PR verification stage; the old wait does not return while the read is held.
- A case whose phase never ends fails within its budget naming its case and phase, and its
  cleanup runs.
- The four PR-condition tests pass beside held CPU load, without retries.
- Every `parking::` test of `yulius::main` passes.

## Test plan

- Regression tests in `crates/yulius/tests/parking/candidate.rs`, failing on the old helpers.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^parking::/)'`.
- The four cases with `--stress-count` beside CPU-bound busy loops holding a fixed share.

## Decisions

- Fix the fixture, not the pass: every production await probed is bounded by its own contract or
  waits by design; a whole-pass timeout in production would cancel a park mid-decision.
- A passive "last point reached" record rather than hooks: hooks are one-shot and would be taken
  from tests that pause at the same points.
- Four tests instead of one loop: each case gets its own nextest budget and name; the loop shared
  one 120 s budget across four sequential restarts.

## Evidence

- Baseline (dev `90ec692d2`), alone: the old four-case test passed in 18.5 s.
- Probes (temporary, reverted; diff kept in the ignored `target/630/probe.diff`): see the table in
  Root cause. Each held await kept the old `parked()` past its 30 s bound and failed with only
  "timed out waiting for the task to park".
- Regression checks, commit `0aaa49c2e`, on the old helpers:
  - `a_park_pass_held_in_its_pr_reads_fails_at_the_bound_and_names_the_stage`: FAIL,
    "the park wait outlived its 2s bound by 18 s" (the wait did not return while the branch list
    read was held).
  - `a_stalled_case_fails_at_its_bound_naming_the_case_and_phase`: FAIL after 90 s,
    "a stalled case did not fail within its bounds".
- With the fix (commit `05a6b6ad7`) both pass in about 4.5 s. Their messages:
  - "timed out waiting for the task to park within 2s: a pass was still running at the deadline
    after 1 pass(es); last stage reached: verifying the task's pull requests (GitHub reads, local
    Git checks) (1.2ms into the wait); task Some(Running), Some(Waiting)"
  - "stalled case, the check: did not finish within 2s", and the task was stopped (`Stopping`:
    its run's end is recorded by a reader on the case's runtime, shut down by then).
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^parking::/)'`:
  170 passed.
- Held CPU load (`target/630/loaded.sh`): the four PR-condition tests pinned with `taskset -c 0-3`
  beside 12 busy loops pinned to the same CPUs (about a 25 % share), `--stress-count 5
  --retries 0`: 20 of 20 passed, 4.0–4.8 s each (about 2.6 s each unloaded).
- `scripts/dev lint` and `scripts/dev quick`: passed.
- Not reproduced: which await was slow in run `2f5c9b57` cannot be told from what that run kept;
  the fixture now names it if it happens again.
- Review round 1 (yulius-reviewer, Fable 5.2) approved `3fe484227`: "approve — no blocker and
  no major in the change's own code". Its findings, all in this change's test code, spec or a
  doc comment, are answered after the approval:
  - Minor, a lossy stage once a park was decided or the idle facts refused it: the wait keeps
    when a pass first decided a park and then reports "a park was decided … into the wait; its
    session had not ended". `LiveRowPrNews` has a label, and a wait with no pass says the task
    was not running.
  - Minor, FIX-004 named one case budget, but each phase had its own 40 s (about 160 s worst case,
    over nextest's 120 s kill): the phases now share a 75 s case budget, the cleanup has 25 s, and
    a failed check is printed before the cleanup. The stalled-case wait is 110 s.
  - Nits: the hold test's wall-clock margin went from 3 s to 10 s (inside its 20 s outer bound).
    `ParkStateRecorded` reads "restoring the parked screen". The doc of `last_reached` says the
    record is not scoped to a runtime.
  - Not changed: `with_parked_candidate_owner` has no phase bounds (pre-existing, outside FIX-004).
- After the fixes: all 170 `parking::` tests pass, and `scripts/dev lint` and `scripts/dev quick`
  pass. Under the same held CPU load, the four PR-condition tests and the two regression tests
  passed 30 of 30 with `--stress-count 5 --retries 0`.
- Integration gate (dev `ee5ab98c4`, `xclipboard::tests::native::the_paste_owner_answers_only_its_text_and_lets_it_go`),
  assigned to this task: #1744 (`95e28bf18`) had already repaired it on dev with a round trip on
  the releasing connection. On this branch's dev merge it passed 300 of 300 with
  `--stress-count 300 --retries 0`, so no change here.
