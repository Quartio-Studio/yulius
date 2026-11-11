---
yulius-task: 34165c5e-1d18-4e09-ab9d-c25b750f0d82
status: in-progress
---

# SPEC-454: The security-job no-cancel test holds its jobs itself, not by the 8 s job limit

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #436, issue #1079
Scope authorization: the request of task #436 (from task #430, which works through open issues):
solve issue #1079 in one pull request, comment progress on the issue, open no new issue.

## Goal

`local_ci::the_security_job_and_the_others_never_cancel_each_other` proves SPEC-273 NOI-007 (a
failed `security` job and the other jobs never stop each other) however slow the host is: the jobs
it holds end only when the test releases them, and nothing else competes with them for a job slot.

## Context

- **Symptom.** The test fails now and then in Yulius CI and in local gates under load, after about
  62 s: `jobs never reached [("security", "failed"), ("quality", "running"), ("e2e", "running")]`,
  with `quality` and `e2e` in `error`, "timed out after 8 s" (since #1103: "timed out: ran 8 s, its
  time limit"). Seen on PR #1076 and in the gates of SPEC-409, SPEC-435 and SPEC-441. Alone and at
  rest it passes.
- **Root cause (reproduced).** The test's first `ci_sync_sandbox` queues two runs, not one: the
  integration branch's (`main == dev`, queued first) and the pull request's (`dom::plan`,
  `crates/yulius-domain/src/ci/mod.rs`). Both have the same three jobs, which loop until
  `<data>/release` exists, and the test writes that file only after it has seen the pull request's
  jobs. The fixture gives three job slots and an 8 s job limit (`fixture_tuned`,
  `crates/yulius/tests/local_ci.rs`), and the six jobs take the slots in the order their tasks
  arrive (`ci_job`, `crates/yulius-app/src/ci/exec.rs`):
  - the pull request's three first: it passes in 1.3 s;
  - the branch's three first: the pull request's jobs start only when the branch's are ended by
    the 8 s limit, so the test passes thanks to that limit, in 9.3 to 9.6 s (2 of 3 runs at rest);
  - mixed (the pull request's `quality` and two of the branch's): `security` gets a slot only when
    the limit ends those three, at the very moment it ends `quality`. "security failed while
    quality runs" never exists and the test fails after its 60 s wait. This is the failure CI
    recorded: `security` failed with `waited_ms` 8227, `quality` "timed out: ran 8 s, its time
    limit" with no wait, `e2e` "…; it waited 8 s before it started".
  A second, independent dependence: once its own jobs run, the test must see them and release them
  within their 8 s, in both phases (the second phase holds `security` the same way).
- **Why a longer limit alone is no fix.** With only the limit raised, the branch's held jobs keep
  the three slots for good whenever they take them first, and the test fails after 60 s with the
  pull request's jobs still queued (2 of 4 runs at rest, see Evidence).
- **Why the tests missed it.** The test passes at rest in two of the three orders, one of them
  only because of the limit, so nothing showed that its scenario has two runs.
- **What else the cause touches.** Nine other tests in `local_ci.rs` hold a job on a file under
  the same 8 s limit (`held_toml`'s two, `jobs_wait_for_their_cpu_budget_and_are_told_it`,
  `a_job_cancelled_while_it_waits_for_a_slot_ends_its_wait_cancelled`, `turn_toml`'s two,
  `a_run_s_jobs_record_their_stages_and_test_counts`,
  `ci_jobs_get_the_secrets_the_integration_branch_grants_them`,
  `running_ci_jobs_weigh_on_their_sandbox_until_they_end`). None of them has the slot contention:
  their other run fits beside the held job, is released up front, or is part of what they check.
  Their window is the whole 8 s from the held job's start, which needs the test's poll to stall
  for 8 s; no recorded failure of theirs has this cause. They stay as they are (see Decisions).
  The "error" a job could still end in here is not a time limit: a job that cannot be started or
  whose process host does not answer (30 s and 2 s bounds of the process host, product code
  outside this test).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NCT-001 | While the test holds the pull request's jobs, no other run holds or waits for a job slot: the integration branch's run and the pull request's first run have passed before the first held run is queued, and the test asserts it. | `local_ci::the_security_job_and_the_others_never_cancel_each_other` |
| NCT-002 | Only the test ends a held job (its release file, or the fail-fast it checks): this test's fixture has a job limit of an hour, far beyond its waits of 60 s each. The fixture's default of 8 s stays for the other tests, which time out on purpose. | the same test, under the probes of the Test plan |
| NCT-003 | No sleep is added as a wait: every wait stays a poll of a condition bounded by `WAIT`. | the diff |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- Reproduced with probe delays that make the current test fail: a slow first poll, a slow poll in
  the second phase, and the pull request's `security` job starting behind the branch's jobs.
- The test no longer depends on the 8 s job limit, neither to free the job slots nor to hold the
  intermediate state, and adds no sleep as a wait.
- The test passes under the same probes, repeatedly at rest and under CPU load, and in the full
  gate.

## Test plan

- **Regression, committed first (red).** The test asserts, while the pull request's jobs are
  held, that every other run passed. Today it fails in every slot order: the branch's run is
  still running behind the held jobs, or was ended by the limit (`error`).
- **Probes, never committed.**
  - R1: the test does not look for 20 s before its first poll.
  - R1b: the test does not look for 12 s before the second phase's poll.
  - R2 (product side, switched by an environment variable): the pull request's `security` and
    `e2e` and the branch's `e2e` ask for their slot 500 ms late, which forces the mixed order.
  - R3: only the limit raised (600 s), no other change.
- **After the fix.** The test under R1 + R1b and under R2; 20 runs at rest; runs under CPU load;
  the neighbouring `local_ci` tests; then every gate: `scripts/dev ci`, `yulius security-scan`.

## Decisions

- **The other runs go first, released, instead of only a longer limit.** R3 shows the limit alone
  turns the flake into a failure. Releasing first costs six jobs that end at once and removes the
  8 s wait of two runs in three, so the test gets faster (about 2 s instead of 9 s).
- **The limit comes from `fixture_tuned`, for this test only.** It is the file's existing way to
  tune one test (three users); the fixture's 8 s default stays because other tests time out on
  purpose (`slow`, `hang`). A per-job `timeout_minutes` would bring another product path (a job's
  own limit, SPEC-435 JTL-003) into a test about fail-fast.
- **An hour.** Any limit the test's waits cannot reach would do; an hour is far from the longest
  hold a passing run can have (two waits of 60 s), and the limit no longer decides anything. A
  held job does not outlive a failed test for long: the fixture's directory goes with the test
  and the process host ends a run whose directory is gone (seen after a failed run: the jobs'
  processes gone within 9 s, their process hosts within 35 s).
- **The regression test is an assertion in the test itself.** The defect is the test's scenario,
  so the guard states what the scenario needs (the other runs passed while the jobs are held) and
  fails without the fix in every order, with no probe. The limit has no such guard: only a wait
  longer than 8 s could show it, and the probes do.
- **The other tests that hold jobs stay as they are.** The issue is this test; their window is a
  full 8 s and none has failed for this cause. Named in Context and on the issue, not changed
  (one concern per pull request).
- **No decision record.** Nothing in the product or in the project's rules changes; the choices
  above are local to one test and recorded here.

## Evidence

- **The command throughout:** `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(=local_ci::the_security_job_and_the_others_never_cancel_each_other)'`, one run each time.
- **Reproduced at dev `911e3127`** with probes that were never committed. Without any, the
  unchanged test passed in 9.6 s, 1.3 s and 9.3 s: the 9 s runs are the branch-first order,
  saved by the 8 s limit.
  - R1 (no poll for 20 s after the first dispatch): failed after 80.7 s, `jobs never reached
    [("security", "failed"), ("quality", "running"), ("e2e", "running")]`, with `quality` and
    `e2e` in `error`, "timed out: ran 8 s, its time limit; it waited 8 s before it started".
  - R1b (no poll for 12 s in the second phase): failed after 81.0 s, `jobs never reached
    [("quality", "failed"), ("e2e", "cancelled"), ("security", "running")]`, with `security` in
    `error`, "timed out: ran 8 s, its time limit".
  - R2 (the mixed slot order, forced): failed after 60.7 s with the picture CI recorded:
    `security` failed after waiting 8038 ms, `quality` "timed out: ran 8 s, its time limit"
    with no wait, `e2e` "…; it waited 8 s before it started".
  - R3 (only the limit raised, to 600 s): 2 of 4 runs failed after 60.7 s, the pull request's
    jobs still queued behind the branch's held ones.
- **Regression first** (`3273fbae`, the assertion alone): 6 of 6 runs at rest failed on it,
  "another run holds job slots or was ended by its time limit": 4 with the branch's run in
  `error` (after 8.8 s), 2 with it still `running` (after 0.8 s).
- **With the fix** (`2e4b9244`):
  - At rest: 20 of 20 passed, in 1.56 to 1.72 s.
  - Under R2: 5 of 5 passed (3.0 s). Under R1 + R1b: 2 of 2 (33.3 s, 32 s of them the probes).
    Under all three: 2 of 2 (33.8 s).
  - Each half of the fix is needed (mutations, not committed). With the limit back at 8 s the
    test under R1 + R1b failed after 81.0 s: `quality` and `e2e` "timed out: ran 8 s, its time
    limit", with no wait. Without the runs going ahead it failed 4 of 4 times: 3 after 60.7 s
    (`jobs never reached …`, as in R3), 1 on the assertion after 0.8 s.
  - Under CPU load, pinned with busy loops to four CPUs (the fixture's capacity is the CPUs the
    test sees): 10 of 10 passed with 32 loops (a pinned probe got 0.11 to 0.16 of a CPU; 7.9 to
    9.2 s a run, five times the time at rest) and 5 of 5 with 120 loops (0.03 of a CPU; 29 to
    33 s a run). No job ended in `error`. For scale: SPEC-441 records the unchanged test at 16
    to 18 s alone after its gate hit the flake; here it took 9 s at rest.
  - `842dd4a0` rewords one comment of the test; it passed 3 of 3 after it (1.7 s).
- **The neighbours.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^local_ci::/)'`: 63 of 63 passed in 18.4 s.
- **Gate.** `scripts/dev ci` at `96eca926`: exit 1, on failures outside this change that block
  every pull request since the host move of 2026-11-11 (#1154; PR #1150 carries their fixes).
  - Doctor, check, lint and the macOS compile passed.
  - Tests: 3282 of 3284 passed, 16 skipped. The two that failed are
    `first_message::a_draft_typed_during_the_opening_turn_is_left_alone` and
    `first_message::focus_and_mouse_reports_do_not_keep_the_goal_out`, "timed out waiting for
    the continued turn" (#1154).
  - The stages the gate did not reach, run by hand: `scripts/doctests.py`, `cargo shear` and
    `cargo doc` passed; `cargo deny` failed on one new advisory, RUSTSEC-2026-0486 (rustix).
  - The gate is run again on top of dev once dev has those fixes.
- **Security scan.** `yulius security-scan`: "Security scan disabled: Vidoc is turned off in
  Yulius, for this project or the whole instance; nothing was scanned." Disabled, not passed.
- **Limits, and what was not tested.**
  - Yulius CI's own load cannot be produced on demand. The forced slot order, the slow polls
    and the load runs stand for it.
  - With a single CPU the test cannot pass, before or after this change. The sandbox's capacity
    is then one CPU and its jobs run one at a time, so `quality` and `e2e` never run together.
    Pinned to one idle CPU, `e2e` stayed `queued` for the 60 s; dev's version failed there too,
    its jobs ended one after another by the 8 s limit. The scenario needs two CPUs.
