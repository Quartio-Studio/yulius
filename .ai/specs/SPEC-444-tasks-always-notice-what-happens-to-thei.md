---
yulius-task: 995d4b13-efa4-44af-b3a7-32794e7d4b6c
status: done
---

# SPEC-444: Tasks always hear what happens to their pull request

Status: implemented
Date: 2026-11-09
Owner: Yulius task #426
Scope authorization: the owner, verbatim: "spawn a task in yulius to make it notice failures etc".
General: every sandbox Yulius powers, with no setting. Builds on SPEC-441 (the merge queue, task
#423), which decides how pull requests are queued and merged; this spec is about the task always
hearing what happened. Amends SPEC-028 MSR-004 ("nothing is typed into an idle session") for a
task with an open pull request, as SPEC-432 did for a silent MCP. ADR:
[0287](../../docs/decisions/0287-yulius-types-what-happens-to-a-tasks-pull-request-into-its-session.md).

## Goal

A task with an open pull request hears, in its own session and within a minute, every event that
needs it: a red run on its head or on a merge candidate holding it, a conflict with the
integration branch, its armed merge dropped or refused, its merge, and why a green head is not
merging yet. Each message is short, plain and ends with the next step, including when to keep its
hands off. Nothing depends on a `yulius wait` the task keeps alive or on Claude Code's goal
check-ins, and a pull request that makes no progress for an hour is said to the task and shown on
the sandbox's task list.

## Context

- **Evidence, quartio-v2, 2026-11-08 (UTC)** (the request):
  - #116: CI failed on PR #723's head at 08:15; the session was quiet since 07:30 (Claude Code:
    "idle check-ins paused until your next message"); a person's session nudged it at 08:26.
  - #118: PR #744's own run failed at 13:47; its `yulius wait` watched the armed merge and did not
    end on its own head's failure; nudged at 13:56.
  - #121: `yulius wait` ended after its fixed 90 minutes while the run was still queued.
  - #118 (13:35), #120 (05:56) re-armed or merged `dev` while in a passing merge train, which
    dissolved it: nothing told them to keep their hands off.
  - #116 (02:24), #96 (02:58) planned to merge by hand when a green pull request did not merge.
  - Green pull requests behind `dev` sat 30 to 60 minutes with no word to the task (#738, #746).
- **Why (code).**
  - Yulius tells a task about its pull request only through `yulius wait` (`agent_wait.rs`,
    `wait_cli.rs`), which the agent must start and keep alive: a background shell that ends by
    itself (`--minutes 90`), and whose `settled` keeps waiting on a failed head while a merge is
    armed (`now.checks_ended() && !now.merge_armed`).
  - Claude Code's goal check-ins wake a session with background work every 30 minutes, then
    pause "until your next message": nothing else types into an idle session (SPEC-028 MSR-004),
    except Yulius's MCP reconnect (SPEC-432) and its "Yulius is back" (SPEC-358).
  - SPEC-441 writes where each armed pull request stands (MQ-007), but only `yulius wait`,
    `delivery_status` and the task page show it: nobody types it.
  - Conflicts are found only when Yulius tries DLV-203 on an armed, green pull request.
- **What exists to build on.** `type_into_session` (SPEC-230 TMS-004: refuses while a person typed
  in the last minute, before the session started, while another message is typed); the MCP watch
  (`mcp_watch.rs`), a 30 s pass over live tasks with a pure `decide`; CI records with each job's
  failing tests' output (SPEC-145); the armed slot's outcome and note and the merge train's notes
  (SPEC-441); `merged_text` (MQ-009); the PreToolUse guard (`guard_client.rs`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PRN-001 | **A watch, not a wait.** Every 20 seconds Yulius looks at each running task whose delivery has an open pull request, from its own records and its clone only (no GitHub request, except reading the merged head once for the merge notice). What needs the task is typed into its session as one bracketed paste starting "Yulius, about your pull request #N:", whatever the session's check-ins or waits. A session that cannot take input now (a person typed in the last minute, not started, paused or held by an update, another message being typed) gets it on a later pass, and so does one where a person must act first (a permission prompt or question on screen, an open `needs_you`): the paste would answer it. The pasted text is cleaned of control characters. | `pr_watch::tests`; `pr_watch::a_task_with_no_wait_is_told_each_event_once` |
| PRN-002 | **A red run.** A failed run at the pull request's head names each failed job with its exit code, up to 5 failing test names per job, and where to read the log (`ci_status` with the run id and job), then: fix it test-first on this branch. A run that errored says it could not run (not the code), that Yulius reruns it once by itself, else `ci_rerun`. | `pr_watch::tests::a_red_run_names_jobs_tests_and_the_log` |
| PRN-003 | **A red merge candidate.** A failed run of a merge candidate holding the pull request says so with its jobs and tests, that Yulius bisects it and tells the pull request that breaks it, and to keep hands off meanwhile. | `pr_watch::tests::a_red_candidate_says_hands_off` |
| PRN-004 | **A conflict.** When the pull request's head conflicts with the integration branch as Yulius's clone last fetched it (Git's merge with the repository's merge drivers), the task is told once per head: merge the integration branch in with `git merge`, resolve, rerun the tests, commit, end the turn; never rebase or force-push. | `pr_watch::a_task_with_no_wait_is_told_each_event_once` |
| PRN-005 | **Dropped, refused or merged.** How the armed merge ended (SPEC-441's outcome text: dropped from the queue or a candidate and why, refused and why) is typed once. A pull request found merged (by Yulius or anyone) is said with `merged_text`: whether it counts as merged on green and what is left; the watch then ends for it. | `pr_watch::a_task_with_no_wait_is_told_each_event_once`; `pr_watch::tests::merged_ends_the_watch` |
| PRN-006 | **Green but not merging, and when to keep hands off.** A head whose own run passed and that is not merging now is said once per reason: not armed (get the review of that head and call `delivery_merge`), or where the armed merge stands (SPEC-441 MQ-007). While a merge candidate holds it, the message ends "Hands off: do not re-arm, merge `<integration>` or push to this branch; Yulius merges it in order when the run passes." | `pr_watch::tests::green_says_why_and_hands_off_in_a_candidate` |
| PRN-007 | **Each event once.** Each event has a key (its run, head, outcome or pull request); what was typed is kept in `<data>/pr-notices.json`, so a restart does not repeat it. Several new events of one pass go in one message. A task whose pull request was never seen open by the watch is not told it merged. | `pr_watch::tests::each_event_is_typed_once`; `pr_watch::tests::state_survives_a_restart` |
| PRN-008 | **Check-ins do not pause.** While the pull request is open and not merged, a session that has been idle with nothing typed into it and no turn for 45 minutes gets a check-in: the pull request's state in one line and what to do (often: nothing, end the turn). Not while the task waits for a person (`needs_you`) or its goal is achieved. Without Yulius CI it says so instead of waiting for a run. | `pr_watch::tests::a_quiet_session_gets_a_check_in` |
| PRN-009 | **Stall detector.** A pull request with no progress for 60 minutes (no run of its head or of a candidate holding it queued, started or ended; no delivery or armed-merge change; no event typed) while no such run is queued or running, the agent's turn has been closed for 5 minutes and the project has Yulius CI, is stalled: the task is told once per stall, with its state and next step, and its task summary carries `pr_stalled` (the sentence), shown on the sandbox's task list until progress. | `pr_watch::tests::a_pull_request_without_progress_stalls`; `pr_watch::a_task_with_no_wait_is_told_each_event_once`; e2e `task-done` ("a task whose pull request stalled") |
| PRN-010 | **Waits never end silently.** `yulius wait` has no time limit while the run at its head is queued or running, a merge is armed or a deploy runs; `--minutes` (90) bounds only a wait with no run at its head, and its last line says why ("no Yulius CI run at … after 90 min: was the head pushed?"). A wait with a merge armed ends when the run at its head fails or errors. | `wait_cli::tests::a_queued_run_never_times_out`; `wait_cli::tests::an_armed_wait_ends_on_its_heads_failure` |
| PRN-011 | **Merging by hand is refused.** The PreToolUse guard denies `gh pr merge` and a `gh api` call to a pull request's `/merge`, saying Yulius merges through `delivery_merge` and its queue, that `delivery_status` and Yulius's messages say why a green pull request is not merging, and that a hand merge does not end the goal's delivery. | `guard_client::tests::hand_merges_are_refused` |
| PRN-012 | **Tasks are told.** The briefing (within its budget), the definition of done and the bundled `delivery-pipeline` skill say that Yulius types every event of the task's pull request into its session with the next step, that no wait has to be kept alive for it, and to obey "Hands off". | `briefing::tests`; `request::tests` |

## Plan

- [x] Write the spec and the ADR for pull request notices
- [x] A red run on the head or a merge candidate is typed into the task's session once
- [x] Conflicts, dropped or refused armed merges and merges are typed with their next step
- [x] A green head that cannot merge says why, with hands off in a merge candidate
- [x] Quiet sessions get a check-in and a stalled pull request is told and shown on the task list
- [x] yulius wait never ends silently and an armed wait ends on its head's failure
- [x] Merging by hand is refused with the reason
- [x] Briefing, skill and documents say Yulius tells the task
- [x] Independent review findings answered test-first
- [x] Gates green and the evidence recorded

## Acceptance criteria

- A live task with no `yulius wait` running and an open pull request is typed, within one watch
  pass (20 s): a failed run at its head (jobs, tests, `ci_status` run id), a conflict with the
  integration branch, and its merge, each once and each with its next step.
- A green head held by a merge candidate is told to keep its hands off.
- A quiet session gets a check-in after 45 minutes; a pull request without progress for 60 minutes
  is told and shows `pr_stalled`.
- `yulius wait` does not end while the run at its head is queued; an armed wait ends on its
  head's failure.
- `gh pr merge 12 --squash` in a task's Bash call is denied with the reason.
- `scripts/dev ci` passes.

## Test plan

- Unit (`yulius-app` `pr_watch::tests`): the message of each event, the once-per-key rule and its
  saved state, the check-in and stall clocks, and what stops the watch.
- Unit (`yulius-domain`): failing test names read from the kept failures output (nextest and
  libtest).
- Integration (`crates/yulius/tests/pr_watch.rs`): a live fake-agent task with real repositories
  and the production Git adapter, a pull request record and CI records: one watch pass types the
  failure, a commit on the integration branch that conflicts is typed, the merge is typed, and the
  session echoes each once; a task that needs a person (a prompt on screen) is typed nothing.
- Unit (`wait_cli::tests`): the no-limit rule and the armed wait on a failed head.
- Unit (`guard_client::tests`): hand merges denied, other `gh` calls allowed.
- Web: the e2e suite (`task-done`) for the stalled label in the header and on the task list.

## Decisions

- **Yulius types, the agent does not wait.** A server-side watch over records Yulius already keeps
  needs nothing alive in the sandbox and works whatever Claude Code's check-ins do. A wait stays
  useful to end a turn on and is fixed, but is no longer the only channel.
- **Records, not GitHub.** The watch reads CI runs, deliveries, the armed slot and the clone, so
  its cost does not grow with GitHub calls; conflicts are computed with the clone's merge (merge
  drivers apply) and cached by head and integration commit. GitHub's own view arrives with the
  3-minute delivery round, as before.
- **Once per key, saved.** Typing the same news twice costs turns and trust; a file like
  `armed-merges.json` survives restarts without a migration.
- **Check-ins from Yulius, not Claude Code's.** Detecting Claude Code's pause text would break
  with its wording; 45 quiet minutes with an open pull request is the fact that matters, and a
  typed message also resumes Claude Code's own check-ins.
- **Stall excludes check-ins.** A check-in is not progress, or the stall would never come.
- **Once per head and reason, not per wording** (review round 1, minor 5). The armed note's
  wording changes with the agent's activity (busy or idle) and each pass; keying on it would type
  a new message every time the agent's turn, which a message starts, changes it: a loop. A green
  head is told once per head and kind (not armed, armed, held); a later change of the note is
  seen in `yulius wait`, `delivery_status` and the stall.
- **Not typed over a person's prompt** (review M1). `type_into_session` refuses only a person's
  recent typing; a permission prompt or question would take the paste and its Enter as an answer,
  so the watch holds events while the task needs attention or a person.
- **The guard is a nudge, not a boundary** (review minor 9). It refuses `gh pr merge` and
  `gh api -X PUT …/pulls/N/merge` where `gh` is the command (also inside `bash -c`), not `curl`
  or a script that calls them; GitHub's ruleset and Yulius's merge gate stay the boundary.
- **Known limits.** The failing test count can undercount tests whose output was not kept (the
  scanner keeps 32 KiB per job). On the first pass after a deploy, every open pull request is told
  its current state once. Check-ins continue every 45 quiet minutes while the pull request is open
  (ADR-0287 accepts the cost).
- See ADR-0287 for the alternatives.

## Evidence

- **Tests first.** Each behaviour change has a test that failed without it:
  - `pr_watch::a_task_with_no_wait_is_told_each_event_once` (integration: a live fake-agent task,
    real repositories, the production Git adapter, no `yulius wait`): one watch pass types the red
    run (job, exit code, failing test, `ci_status` run and job), the conflict with `dev` together
    with the green head's "no merge is armed", the stall (shown by `task_summary().pr_stalled`) and
    the merge, each once, and nothing while the task needs a person. Two mutations were seen to
    fail it: dropping the person-must-act hold, and dropping it from the merge notice (review
    rounds 1 and 2). An earlier form of the hold check passed vacuously because the session did not
    take input yet; it was moved to a pass after a typed message.
  - Unit `pr_watch::tests` (10): each message's words, once per key, saved state, check-ins,
    stalls (CI idle, no CI, agent working, a turn just closed), the person-must-act hold, green
    reasons.
  - `wait_cli::tests::an_armed_wait_ends_on_its_heads_failure` failed before the fix;
    `a_queued_run_never_times_out` covers `limit_line`; `restart::a_wait_rides_out_a_restart` now
    expects the "No Yulius CI run at … after 1 min" line.
  - `guard_client::tests::hand_merges_are_refused`: the reviewer's cases (option values, `bash
    -c`/`-lc`, `NAME=value gh`, `$(gh …)`, `grep -c`) failed before each fix.
  - `failures::tests::failing_test_names_are_read_from_the_kept_headers`; `briefing::tests` and
    `request::tests` for the guidance; e2e `task-done` "a task whose pull request stalled".
- **Gate.** `scripts/dev ci` passed at 8a61b12e (exit 0): format, lint, macOS targets, repository
  contracts, 3,270 tests passed, 16 skipped, docs. An earlier gate at fb90e7a2 was stopped
  (superseded by review fixes, and cargo had run beside it).
- **E2E.** `npx e2e run` at 8a61b12e: 75 files passed, 1 skipped; 877 tests passed, 1 skipped
  (`task-done` alone: 21 passed).
- **Independent review.** Four rounds by a fresh read-only subagent:
  - round 1 (fb90e7a2): 2 majors (typing over a prompt a person must answer; wrong stall and advice
    without Yulius CI), 11 minors, nits; all fixed test-first or recorded under Decisions;
  - round 2 (7ccb1935): 1 major (the merge notice bypassed the hold), 4 minors; fixed;
  - round 3 (e156d448): approved, 1 new minor (`grep -c` refused) and a nit (stall label on a
    merged pull request); both fixed test-first;
  - round 4: approved 8a61b12e. Left as documented: `bash -l -c` passes the guard (a nudge, not a
    boundary); no dedicated test for the dropped/refused outcome event as built from the slot, nor
    for "a pull request never seen open is not told it merged" (both read by the reviewer).
- **An unrelated flaky e2e test.** CI run ea691b6c at 20aa33ef failed one e2e test outside this
  change, `update.e2e.ts` "cancelling an update shows its outcome from the cancel's answer"
  (expected 1 held update read taken, got 2): a page read arrived between counting the held reads
  and releasing them. Reproduced every time by making that read on purpose; fixed in the test by
  counting while releasing in one call (its own commit); the file passed 3 runs of 3.
- **Security scan.** `yulius security-scan`: "Security scan disabled" (Vidoc is turned off for
  this project in Yulius). Disabled, not passed.
- **Live check on the instance** (main 4d9aebc4, which holds this change), with this task's own
  Claude Code session and no `yulius wait` running:
  - Red run: follow-up pull request #1134 carried one test failing on purpose (head 87b442e2,
    merge armed). Run 533de2e1 failed `quality` at about 15:56:00 UTC on 2026-11-09; at 15:56:15
    Yulius typed "Its Yulius CI run 533de2e1-… at 87b442e25 failed: `quality` exited 1 (failing
    tests: yulius-domain ci::failures::tests::spec_444_red_run_on_purpose). Read the log with
    `ci_status` …, fix it test-first …" and "The merge you armed was dropped …". The test was then
    reverted.
  - Conflict and merge: pending (a follow-up records them).
- **Could not be tested here:**
  - Paused check-ins: this session runs under a `/goal` whose evaluator never paused its
    check-ins, so the live check had no wait running but did not have paused check-ins. The
    integration test covers a session nothing else wakes.
  - quartio-v2 itself: outside this sandbox's access.
