---
yulius-task: 21cf8c86-c140-427a-a833-2da9c188a2e1
status: done
---

# SPEC-475: The reviewed fix of pull request #1162 reaches `dev` (issue #1079)

Status: done
Date: 2026-11-12
Owner: Yulius task #457, issue #1079
Scope authorization: the request of task #457 (from task #430, which works through open issues):
deliver the finished fix of pull request #1162 in one pull request that says `Fixes #1079`, write
no second fix, comment progress on the issue, open no new issue.

## Goal

The fix for the flaky `local_ci::the_security_job_and_the_others_never_cancel_each_other`, written
and reviewed by task #436 and never merged, is in `dev`: the same test change, proven again on
today's `dev`, delivered by one pull request that closes issue #1079.

## Context

- **The fix and its record are [SPEC-454](SPEC-454-local-ci-the-security-job-no-cancel-test.md).**
  Symptom, root cause, the probes, the mutations, the decisions and three review rounds are
  there and are not repeated or rewritten here. In short: the test queued two runs whose held
  jobs competed for the fixture's three job slots under an 8 s job limit; now the other runs
  pass before the test holds any job, the test asserts that, and its fixture's job limit is an
  hour. Only `crates/yulius/tests/local_ci.rs` changes, nothing in the product.
- **Why it was not in `dev`.** Pull request #1162 (head `d1835270`) was approved for exactly
  `6022027e` (the two commits after it only merge `dev`), `yulius/ci` passed at its head, and
  merge train #1162 passed too (run `d85af3b4`). Task #436 was closed two minutes after that
  train started (2026-11-11T15:36:54Z), and a closed task's pull request is not merged when its
  run passes. The pull request stayed open with no running task; `dev` moved 23 commits and
  GitHub shows it as conflicting, in `docs/changes.md` alone.
- **The defect is still on `dev`** (`2a31d50a`): `local_ci.rs` is untouched there since the
  pull request's merge base (`bbd65665`), so the test still gets the fixture's 8 s limit and
  still queues the two competing runs (measured, see Evidence).
- **What changed under the test since it was probed and reviewed.** SPEC-454's probes ran on
  `dev` `911e3127` and the review's own runs on `e2dd441d`. After the approval, the pull
  request's last merge of `dev` (`d1835270`) brought in #1163 (SPEC-459, `05caae42`): the CI job
  slots are no longer a plain semaphore sized once from `config.ci.max_jobs` but `JobSlots`
  (`crates/yulius-app/src/ci/slots.rs`), a semaphore that can be resized, sized from the limits
  in force (`App::ci_max_jobs`, `crates/yulius-app/src/limits.rs`). For this test nothing
  differs: the limits start from the configuration's numbers (`LimitState::new`) and stay there
  until `App::load_limits` or a change through the limits API replaces them. The server loads
  them at its start; a test does either only for its own fixture (two new `local_ci` tests and
  the `limits` tests), and this test does neither. So its fixture still has three slots, handed
  out first come first served. Yulius CI passed with #1163 twice (at `d1835270` and in merge
  train #1162); the probes had not been run on it.
- **SPEC-454's list of other tests is two short on today's `dev`.** It names nine tests of
  `local_ci.rs` that hold a job on a file under the fixture's 8 s limit. #1163 added two more,
  `a_raised_job_limit_starts_waiting_jobs_while_one_runs` and
  `a_lowered_job_limit_starts_no_job_beside_the_ones_it_lets_finish`. SPEC-454 predates them and
  stays as it is; they are neither analysed nor changed here (one concern per pull request).
  What the review's probes showed about them is in Evidence and reported on issue #1079.
- **What this task adds to the fix:** nothing in the test. It merges the pull request's head,
  moves the change log's entry to the day the fix lands, proves the fix on today's `dev` and
  delivers it.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| LND-001 | The delivered test change is the reviewed one: against `dev`, `crates/yulius/tests/local_ci.rs` differs exactly as it did at the approved `6022027e` against the `dev` of that time, and SPEC-454 is that head's file, unchanged. | `git patch-id` of both diffs; `git diff 6022027e HEAD -- .ai/specs/SPEC-454-*` empty |
| LND-002 | `docs/changes.md` has the entry for #1079 once, at the top (the day it reaches `dev`), and no other line of the file differs from `dev`. | `git diff origin/dev -- docs/changes.md` |
| LND-003 | The fix holds on today's `dev`: the test passes repeatedly, its guard still fails without the fix, the other `local_ci` tests pass beside it, and every gate that runs here passes on the merged head, with the security scan's state recorded. | the runs and the gate in Evidence |
| LND-004 | One pull request delivers it and says `Fixes #1079`; pull request #1162 is closed with a pointer to it, and its branch is not left stale. | the pull request, #1162, `repo_hygiene` |

## Plan

- [x] Pull request #1162's head merged onto this branch, the test and SPEC-454 as reviewed
- [x] The #1079 entry of `docs/changes.md` stands once, at the top of the day it lands
- [x] The fix proven on today's `dev`: the test repeated, its guard red without the fix, its neighbours
- [x] Every gate run on the merged head
- [x] What differs from the approved `6022027e` laid out for the independent review

Once the pull request is open and after its merge, not Plan items (`delivery_merge` needs every
step done first, and it is what opens the pull request): its description says `Fixes #1079`, pull
request #1162 is closed with a comment that names its replacement, issue #1079 gets a comment that
says what was done, and `repo_hygiene` shows nothing of this task or of #436 left.

## Acceptance criteria

- `dev` contains the test change of pull request #1162, unchanged, through one merged pull
  request whose description says `Fixes #1079`; issue #1079 is closed by it.
- The test passed repeatedly and in the full gate on a head that contains today's `dev`, and
  Yulius CI passed on the head that is merged.
- An independent review approved the delivered head, having judged what differs from
  `6022027e`.
- Pull request #1162 is closed and points at its replacement; no second fix was written.

## Test plan

- **The test, on the merged head:** `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(=local_ci::the_security_job_and_the_others_never_cancel_each_other)'`, repeated at rest.
- **Both halves of the fix still matter on today's `dev`** (mutations, never committed):
  without the block that lets the other runs go first, the test must fail on the assertion
  "every other run must have passed before this one holds its jobs"; with the limit back at 8 s
  it must fail under a slow poll.
- **SPEC-454's probes, run again as recorded** (never committed; R1, R1b and R2 as its Test plan
  names them, each switched by an environment variable): the fixed test must pass under them,
  and `dev`'s test must still fail under them, which shows that they still force what they were
  written to force on the changed slot code. A short run under CPU load as well. Nothing is
  derived anew: the root cause and the probes are SPEC-454's.
- **The neighbours:** every `local_ci` test, several runs.
- **Gates:** `scripts/dev ci` on the merged head; `yulius security-scan` (its state is recorded:
  a disabled scan is not a passed one).
- **Yulius CI** on the pull request's head, before the merge.

## Decisions

- **Merge the pull request's head instead of copying its diff.** `git merge d1835270` keeps the
  pull request's fifteen commits and their messages (thirteen up to the approved `6022027e`,
  then two merges of `dev`), so "the test is the reviewed one" is a fact Git shows (`git diff
  d1835270 HEAD -- crates/yulius/tests/local_ci.rs` is empty), and the squash merge into `dev`
  makes one commit of it either way. A cherry-pick or a copied diff would have
  to be compared by hand. Alternative, reviving #1162 itself (merging `dev` into its branch):
  rejected, because that branch and pull request belong to a closed task: Yulius delivers and
  merges only the pull request of a running task's own branch, which is how #1162 was stranded.
- **SPEC-454 stays as it is.** It is the fix's record, `status: done`, with task #436's id. This
  spec is the delivery's record and points at it; nothing is copied, so the two cannot drift.
- **The change log's entry moves to the top of 2026-11-12 and names both tasks.** The file is
  newest first by the day a change reaches `dev`. The union merge left the entry where task #436
  wrote it, in the middle of 2026-11-11, between entries that landed that day, which would date
  the fix a day before it was in `dev`. Its text is task #436's; only the reference gains the
  delivering task and this spec.
- **SPEC-454's probes and mutations are run again; its analysis is not redone.** The request
  says not to derive them again, on the footing that nothing on `dev` touches what the fix
  rests on. The test file is indeed untouched since the merge base, but the job slots under it
  were replaced by #1163 after the probes and the review's runs (see Context). The fix is about
  who gets those slots, so "proven on today's `dev`" needs the recorded probes on the new slot
  code: a quarter of an hour of machine time against a fix that might only look proven. They
  were taken from SPEC-454 and task #436's session as they stand; the results match SPEC-454's.
- **No assertion on the job limit is added to the test.** The review noted that nothing
  committed fails when the hour is lost: with the limit back at 8 s the test passes at rest, and
  only a slow poll or a job that starts late shows that half. It offered an assertion on the
  configured limit as the author's call. Not taken: it would change the test this task delivers
  as reviewed (LND-001), it would restate the line that sets the limit instead of testing what
  the limit is for, and SPEC-454's Decisions already weighs it ("The limit has no such guard:
  only a wait longer than 8 s could show it, and the probes do"). The probes showed it again on
  today's `dev`, here and in the review.
- **No decision record.** Nothing in the product or in the project's rules changes.

## What differs from the approved `6022027e`

The independent review of pull request #1162 approved exactly `6022027e`. The head delivered
here differs from it as follows, with the command that shows each.

- **The test change: nothing.** Against `dev`, `crates/yulius/tests/local_ci.rs` changes by the
  same patch as `6022027e` did against its `dev` (`e2dd441d`), +32 −6: `git diff e2dd441d
  6022027e -- crates/yulius/tests/local_ci.rs` and `git diff origin/dev HEAD --
  crates/yulius/tests/local_ci.rs`, each piped into `git patch-id --stable`, give `d46f2d5f`.
  The file is the pull request's head's: `git diff d1835270 HEAD --
  crates/yulius/tests/local_ci.rs` is empty.
- **SPEC-454: nothing.** `git diff 6022027e HEAD --
  .ai/specs/SPEC-454-local-ci-the-security-job-no-cancel-test.md` is empty.
- **`docs/changes.md`: the entry's place and its reference.** It moved from 2026-11-11 to the
  top of 2026-11-12 and now reads "(SPEC-454, task #436; delivered by task #457, SPEC-475;
  #1079)"; the sentences after it are task #436's, wrapped anew. `git diff origin/dev HEAD --
  docs/changes.md` shows seven added lines and nothing else.
- **This spec: new.**
- **`dev` underneath: 29 commits** (`e2dd441d..2a31d50a`), none of them part of this pull
  request's diff. Three merges brought them in, each with no conflict resolved and nothing else
  changed: `3308cd33` and `d1835270` (task #436, after the approval) and `5852aff7` (this task,
  the pull request's head into today's `dev`). For each, `git merge-tree --write-tree <first
  parent> <second parent>` gives the merge commit's own tree.
- **Of those 29, one changes the CI code this test's scenario rests on:** #1163 (`05caae42`),
  the job slots (see Context). It also added two tests to `local_ci.rs` (116 lines, `git diff
  e2dd441d bbd65665 -- crates/yulius/tests/local_ci.rs`), which is why the file differs from
  `6022027e`'s although the fix's patch does not. `git diff --stat e2dd441d origin/dev --
  crates/yulius-app/src/ci crates/yulius-domain/src/ci crates/yulius-app/src/resources.rs`
  lists what else changed in that code: `run_disk_max` and `lanes` read from the limits in
  force (`cd.rs`, `environment.rs`, `mod.rs`), nothing in the domain's plan or the capacity.
  The fixture runs through more than that (the tests' support code, the store with migrations
  0050 to 0052, `context.rs`, the router). The review read those commits' diffs: none touches
  runs, slots, limits, the plan, Git or the process host.

## Evidence

- **The command throughout** (SPEC-454's): `cargo nextest run -p yulius -E
  'binary_id(yulius::main) &
  test(=local_ci::the_security_job_and_the_others_never_cancel_each_other)'`, one run each
  time. Every run below is on `e588c163`: today's `dev` (`2a31d50a`) with the pull request's
  head merged in and the change log's entry moved; the commits after it change only this spec.
  The host has 64 CPUs.
- **At rest:** 20 of 20 passed, in 1.54 to 1.60 s.
- **The defect is still on today's `dev`.** `dev`'s version of the test file on the same tree:
  8 of 8 passed, 5 of them in 9.31 to 9.34 s (the integration branch's jobs took the slots
  first and the 8 s limit ended them: passed thanks to the limit) and 3 in 1.32 to 1.33 s. The
  two classes SPEC-454 recorded.
- **Each half of the fix is needed** (mutations, not committed).
  - Without the block that lets the other runs go first: 6 of 6 runs failed in 0.68 to 0.72 s on
    "every other run must have passed before this one holds its jobs".
  - With the limit back at 8 s: failed under R1 after 81.0 s (`quality` and `e2e` in `error`,
    "timed out: ran 8 s, its time limit", no wait) and under R1b after 73.5 s (`security` in
    `error`, the same reason). Without a probe it passed 2 of 2 (1.6 s): only a slow poll shows
    this half, as SPEC-454 says.
- **SPEC-454's probes on today's slot code** (not committed; R1: no poll for 20 s before the
  first wait, R1b: no poll for 12 s before the second phase's wait, R2: the pull request's
  `security` and `e2e` and the branch's `e2e` ask for their slot 500 ms late).
  - The fixed test: under R2 3 of 3 passed (3.0 s); under R1 + R1b 2 of 2 (33.3 and 33.4 s,
    32 s of them the probes); under all three 2 of 2 (33.8 s). SPEC-454's figures.
  - `dev`'s test, so that a pass above means something: under R2 2 of 2 failed after 60.8 s,
    `jobs never reached [("security", "failed"), ("quality", "running"), ("e2e", "running")]`
    with `security` failed after waiting 8045 ms, `quality` "timed out: ran 8 s, its time
    limit" with no wait and `e2e` "timed out: ran 8 s, its time limit; it waited 8 s before it
    started": the picture Yulius CI recorded. Under R1 it failed after 80.8 s (`quality` timed
    out the same way), under R1b after 73.1 s (`jobs never reached [("quality", "failed"),
    ("e2e", "cancelled"), ("security", "running")]`, `security` timed out).
- **Under CPU load:** pinned to four CPUs together with 32 busy loops (a pinned probe got 0.12
  of a CPU before and after), 5 of 5 passed in 7.95 to 8.85 s, five times the time at rest. No
  job ended in `error`.
- **The neighbours.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^local_ci::/)'`: 65 of 65 passed, in each of 3 runs (17.9 to 18.1 s). SPEC-454 had 63:
  #1163 added two.
- **Gate.** `scripts/dev ci` at `e588c163`: exit 0. The repository check and the scripts' own
  tests, `cargo check`, `fmt`, `clippy` for Linux and for macOS, nextest with 3957 tests passed
  and 16 skipped (this test in 1.6 s), the doctests, `cargo deny` ("advisories ok, bans ok,
  licenses ok, sources ok"), `cargo shear`, `cargo doc`. `python3 scripts/check_repository.py`
  passed again after the commits that follow.
- **Security scan.** `yulius security-scan`: "Security scan disabled: Vidoc is turned off in
  Yulius, for this project or the whole instance; nothing was scanned." Disabled, not passed.
- **Independent review, round 1** (a fresh session with `om-code-review`, on `1f971341`):
  approve. It ran the gate itself at that head (exit 0, 3957 tests passed, 16 skipped; the scan
  disabled), confirmed every claim of "What differs from the approved `6022027e`" with Git, and
  repeated this Evidence in a worktree of its own: 10 of 10 at rest, `dev`'s test in both
  classes, both mutations, the probes on the fixed test and on `dev`'s, the load run, the
  neighbours. Its own additions: four product mutations of NOI-007 (a failed `security` stops
  the others, a failed `quality` also cancels `security`, no fail-fast among the others, the
  failed scan not published while the others run) each made the test fail; with `security`,
  `quality` or `e2e` asking for its slot 3 s late, and with `security` and `e2e` 9 s late, it
  passed; with only the hour (SPEC-454's R3) 1 of 4 runs failed after 60.8 s; 32 of 32
  concurrent copies passed. Findings:
  - Minor, own code: this spec did not say that #1163's two tests hold jobs under the 8 s
    limit too, so SPEC-454's nine read as today's list. Added to Context.
  - Minor, not from this change (`9080cf3e`, #851): `until_jobs` checks its deadline only once
    the run it waits for exists; if a sync queued no run, the test would spin until nextest
    ends it. Not changed here; reported on issue #1079.
  - Minor, not from this change (`05caae42`, #1163): its two tests depend on the 8 s limit, the
    second also on the slot order. Shown with a probe that makes a named job ask for its slot
    late; neither order was observed without it. With two jobs 9 s late the first failed after
    60.8 s (job `a` "timed out: ran 8 s, its time limit"); with the `a` jobs 500 ms late the
    second failed after 8.9 s ("two at once under a limit of 1"). Not changed here; reported on
    issue #1079.
  - Nits, taken: "one changes code this test runs through" was wider than its command; "the
    fifteen reviewed commits" (thirteen were reviewed); LND-003 said every gate passes beside a
    disabled scan.
  - Nit, the author's call, not taken: an assertion on the job limit (see Decisions).
  - Nit, no change asked: "only the test ends a held job" is short for SPEC-454 NCT-002's "its
    release file, or the fail-fast it checks".
- **Final review.** The same reviewer looks at the commit that answers round 1, which changes
  only this spec. Its verdict and that head are in the pull request and on the issue: a commit
  recording them here could not be the head it reviewed.
- **Limits, and what was not tested.**
  - Yulius CI's own load cannot be produced on demand; the forced slot order, the slow polls
    and the load run stand for it, as in SPEC-454.
  - Not run again by the author: SPEC-454's R3 (only the limit raised, which shows why the
    limit alone is no fix), its run with 120 busy loops and its runs pinned to one, two and
    three CPUs. They concern the analysis and a limit of the test that this change does not
    touch (it needs three CPUs, before and after), not whether the fix holds on today's `dev`.
    The review ran R3's equivalent (above) and the pinned runs: one CPU failed after 61.2 s with
    `e2e` queued, two and three CPUs passed 3 of 3 each.
  - Why pull request #1162 was never merged is a defect of its own and not fixed here: a task
    closed while its merge train runs leaves an approved, green pull request open with no
    owner. It is reported on issue #1079 and in this task's summary (this task opens no issue).
