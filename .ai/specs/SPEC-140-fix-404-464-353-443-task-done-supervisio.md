---
yulius-task: 82b37c7c-efa0-48af-bb6d-5ce9021f385d
status: in-progress
---

# SPEC-140: task_done, supervision and merge_train tests hold under host load

Status: in progress
Date: 2026-10-15
Owner: Yulius task #127
Scope authorization: task #127, part of task #118 ("Fix all open actionable GitHub issues without
creating new issues, and close them"; SPEC-138 lists the lanes). Fixes #404, #464, #353 and #443.
No new GitHub issue is created. Refines SPEC-065 (END-001) and SPEC-002 §7.5 (hooks).

## Goal

The integration tests named in #404, #464, #353 and #443 pass on a loaded host (Yulius CI, parallel
tasks) because the product does what they wait for and their bounds come from the product's own
bounds. Two product races found on the way are fixed test-first. No retry and no sleep is added.

## Context

All four issues were still open on `dev` at 4373149 (2026-10-15): none of their tests or the code
they exercise had changed since they were filed. The CI runs the issues cite have expired
(`ci_status`: not found), so the causes come from reproductions and measurements in this task's
sandbox (12 CPUs, shared with other tasks; load average 2 to 79).

**Reproductions under load.** The issues' tests run the same way, with 16 to 24 CPU-bound busy loops
beside them and, in one round, 3 writers that write and fsync 64 MiB files:
- the whole integration binary (533 tests, 12 threads), three times in a row at load 25 to 69: all
  passed;
- the ten tests named in the issues, 4 copies each, 16 at once, at load 63 to 79: 40 of 40 passed;
- every `task_done` test, 2 copies each, 16 at once, with the fsync writers, at load 36: 64 of 64
  passed. Each wait's longest time stayed well inside its bound (the longest one-shot waits, "the
  monitor counted" and "one background shell", under 2 s against 10 s and 20 s).

CPU and disk load stretch these tests about two to four times, which their bounds absorb. The CI
failures are of another kind: in #404 a test that takes 10 s waited its full 90 s for Done, and
another test took 12.5 s and failed a 10 s wait that takes under 2 s at load 36. Those runs did not
go slowly; they hung. Measuring each wait, and the hook handling on the server, shows two product
races and one test bound with no margin.

**#464: a stop that comes while a switch launches its run waits for the escalation.** Timing each
wait of `a_switch_waits_for_a_background_agent_launched_in_the_turn` alone, on an idle host: in 3 of
4 runs "the stop" took 20.1 s, in the other 0.1 s. 20 s is the stop escalation
(`stop_escalation_ms` = interrupt 5 s + term 5 s + kill 10 s). The test stops the task as soon as the
switch's new attempt exists. At that moment the switch is still launching it: the new run is
spawned, and Yulius is not attached to it yet. `stop_task_now` moves the task to Stopping, records
the cancel intent and asks the attached run to shut down (`request_shutdown`). There is no attached
run yet, so nothing is asked. `attach_run` then attaches the run and nothing asks it either, so the
run works on until the escalation terminates its host at 20 s and the run is recorded as lost. On a
loaded host, 20 s plus termination and recording passes the test's 30 s: the 48.6 s failure is the
test's 18 s plus its 30 s wait. A person who stops a task while it starts or switches waits 20 s
for it in the same way, and its run ends as lost instead of stopped. The same window exists for a
first launch (Starting) and for a close.

**#404: a hook whose client gave up is lost half-way.** The agents' hooks reach Yulius through the
`yulius hook` client. It waits `HOOK_TIMEOUT` (5 s) for the answer and then lets the agent go on
(fail-open), closing its connection. The server ran the hook's handling inside the HTTP handler, and
hyper drops a handler when its connection closes. A Stop hook does a dozen database round trips on
the store's single database thread and runs Git in the worktree (`stop_reasons`) before its last
step, `settle_clean_turn`. That step sets Done or Your turn and starts the probe of background
shells and monitors. Timed under load (load 36, fsync writers), Stop hooks took about 100 ms, and up
to 2.0 s. On the CI host (load 35 to 50, other gates building, memory pressure: SPEC-082) a Stop
that passes 5 s is cut off before that step. The task then stays Working, which is each symptom in
#404:
- "timed out waiting for Done" (90 s) and "…for Your turn" (90 s): the activity is never set;
- "…for one background shell" (20 s) and "…the monitor counted" (10 s): the probe that counts them
  never runs;
- `a_done_task_can_be_archived` after 90.5 s: its "the shell probe" wait for Done.
Shown deterministically: a hook sent over TCP by a client that hangs up while the server handles
it is never applied. With the client still connected, it is applied (control). In use, a loaded
host leaves a finished task Working with nothing to wake it.

**#353: a stop's bound with 10 s of margin.** `supervision::a_done_task_frees_its_slot_for_a_queued_task`
stops two tasks and waits 30 s for each, one after the other. Its stops took 0.1 s in every run
measured (5 alone, 4 at load 79), and both runs were attached when stopped, so the launch race above
does not apply. The product bounds a stop by the escalation: at `stop_escalation_ms` (20 s) the
run's host is terminated and the run recorded as lost (SPEC-065 END-001), so a test's 30 s leaves
10 s for the termination, the recording and the test's own polling, on a starved runner. The
failure (33.8 s at load 35 to 50) cannot say which step was slow: the test reports only "timed out".

**#443: per-phase deadlines in a test whose total the runner bounds.** `merge_train::a_red_train_falls_back_to_one_by_one_merges`
took 9.2 to 10 s alone and 37 to 41 s in 8 copies at once beside 24 busy loops (load 34 to 79). Per
phase under that load: fixture 4 to 9 s, the train's run and failure about 14 s, the one-by-one
rounds 10 to 11 s and 2 s (the product's `merge_armed` alone 8 to 9 s of Git work in round one).
Each phase has its own fixed `WAIT` of 50 s, and the test's total is bounded only by nextest
(`slow-timeout` 30 s × `terminate-after` 4 = 120 s). A busy host that stretches one phase past 50 s
fails the test although its total is far from 120 s.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| STP-001 | A stop or close that comes while the task's run is launching (spawned, Yulius not attached yet) asks the run to shut down once it is attached, so the run ends as stopped or closed at once, not at the escalation. A stop that comes later still finds the run itself; the process host takes a second shutdown request as the first. | `task_done::a_stop_while_the_run_is_launching_ends_it_once_attached` (fails without: the stop waits for a 60 s escalation); `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` ("the stop" 20.1 s → 0.1 s) |
| STP-002 | An end intent that no run consumed (a stop or close that ended no run: between a switch's two runs, before a failed spawn) is not left for the next run: a launch that no switch drives starts with no intent, and the stop that ends a Stopping task with no live run clears it. A stop asked at the attach also arms the escalation for that run (the stop came before the run existed, so it armed none). | `task_done::a_stop_that_ended_no_run_does_not_end_the_next_one` (fails without: the continued task's resumed run is killed as `cancelled` at its attach) |
| HKS-001 | A hook event Yulius received is handled to its end even when its client stops waiting and closes the connection: the server runs it in its own task, and the HTTP handler only waits for its answer. The status line is unchanged (it only reads). | `agent_hooks::a_hook_whose_client_hangs_up_is_still_applied` (fails without: the notice is never applied) |
| TWT-001 | A test that stops a task waits for the stop within the product's own bound: the stop escalation (`stop_escalation_ms` of that app) plus a settle margin for the termination and its record, waiting for every stopped task at once. When the bound passes, the error names each task's status and its runs' states and exit reasons. | `support::wait_stopped`, used by `task_done` and `supervision` |
| TWT-002 | The merge train tests share one deadline from the start of the test, under nextest's kill bound with room to report what was seen (100 s of the 120 s), instead of a fixed 50 s per phase. | `merge_train::*` |

## Plan

- [x] Spec: the four flakes reproduced under load, each cause named
- [x] A stop during a run's launch ends it once attached (#464), test-first
- [x] A hook whose client gave up is applied to its end (#404), test-first
- [x] Stop waits use the stop's own bound and say what they saw (#353)
- [x] Merge-train waits share one deadline under nextest's kill bound (#443)
- [x] Loaded repeat runs of every named test pass
- [ ] Decision record, architecture and changes log; every gate green
- [ ] Independent review in a fresh session, findings fixed test-first
- [x] A stop that ended no run leaves nothing for the next run (review), test-first

## Acceptance criteria

- AC-1 (STP-001, HKS-001): each new test fails without its fix and passes with it.
- AC-2: the tests named in #404, #464 and #353 pass 10 or more runs in a row each, run in parallel
  copies beside CPU-bound busy loops (load average 30 or more), with the load recorded; #464's
  "the stop" takes under 1 s in every run.
- AC-3 (#443): the red-train test's loaded duration is well under its bound, and it passes 10 runs
  in parallel beside CPU-bound load.
- AC-4: `scripts/dev ci` passes; Yulius CI is green at the merged head.

## Test plan

- STP-001: a session host that attaches a run only once the test releases it, and says when an
  attach is asked for. The test stops the task while the attach is held, releases it, and expects
  the task stopped as cancelled within 30 s with its run not lost, under an escalation of 60 s. Without
  the fix the run ends only at the escalation.
- HKS-001: the internal router served over TCP, with a test layer that reads the request, starts its
  handler, says so and never takes it further (as a dropped handler). A raw client sends a
  "permission" notice and hangs up once the handler started. The task must need attention.
  Without the fix the notice is never applied. Shown with hyper too: with the layer resuming the
  handler after 1 s, the notice is lost when the client hangs up and applied when it stays.
- Loaded runs: `hammer.sh` (scratchpad) runs N copies of each test binary filter in parallel beside
  busy loops (`load.sh`) and fsync writers (`ioload.sh`), recording each run's exit and time and each
  wait's time (temporary instrumentation, not committed).
- `scripts/dev ci` once at the end.

## Decisions

- **Hooks run detached from their connection (ADR-0115).** The agent goes on once the client gives
  up, so the event happened; applying it late is right, cutting it off half-way leaves a state that
  nothing repairs. A longer client timeout only moves the threshold and keeps the agent waiting
  longer at every boundary. Rejected: a server-side deadline (it would cut the same work off, only
  at a known point).
- **A launch asks the run it attached, rather than a stop waiting for the launch.** The stop returns
  at once as before; the launch already reads the task's state after the spawn. Waiting for the
  launch would make a stop block on a spawn and its Git work.
- **Test bounds come from the product's bounds and the runner's.** A stop is bounded by the
  escalation, so a test waits the escalation plus a settle margin (40 s: three times the
  measured worst recording under load, which is under 1 s, would do; the margin covers a starved
  runner and stays inside the 120 s kill). A merge train test is bounded by the runner, so its
  phases share the runner's budget. A failure then says what it saw.
- **No hang was reproduced from CPU and disk load alone**, so the hook loss is shown at the point
  of the race (a client that hangs up while its hook is handled) rather than by retrying a loaded
  run until it hangs.
- **Found on the way, left as is (follow-up in the issue comment):** a stop that comes while a switch
  waits for the old run to exit replaces the switch's intent; the old run's end is then read as the
  stop, and the switch goes on to launch the new run of a stopped task. The window is the old run's
  exit (at most `SWITCH_TIMEOUT_MS / 2`). It is not what the tests here meet and needs its own
  regression test.

## Evidence

Host: this task's sandbox (12 CPUs, shared with other tasks). Load generators in the scratchpad:
`load.sh N` (N `while :; do :; done` loops), `ioload.sh 3` (3 writers of 64 MiB files with
`conv=fsync`), `hammer.sh` (copies of one test of the integration binary, 16 at once, each run's
exit and time recorded). Per-wait timings came from a temporary `eprintln!` in the tests' wait
helpers and in the hook route, not committed.

Before the fixes (dev at 4373149):
- Whole integration binary, `NEXTEST_TEST_THREADS=12 cargo nextest run -p yulius --profile ci -E
  'binary_id(yulius::main)'` beside 16 busy loops, 3 runs: 533/533 each (114 s, 192 s, 231 s; load
  25, 57, 69).
- The ten tests of the issues, 4 copies each beside 16 loops: 40/40 (load 63 to 79). `merge_train`'s
  red train test took 39.9 to 41.5 s per copy.
- Every `task_done` test, 2 copies each beside 16 loops and the fsync writers: 64/64 (load 36).
  Stop hooks: 246 handled, about 100 ms each, slowest 1.44 to 2.00 s.
- `a_switch_waits_for_a_background_agent_launched_in_the_turn` alone, 4 runs: "the stop" 0.11 s,
  20.11 s, 20.11 s, 20.15 s (the escalation).
- `a_done_task_frees_its_slot_for_a_queued_task` alone, 5 runs: both stops 0.0003 to 0.12 s.
- Red train test alone: 9.99 s (fixture 1.0 s, train failed at 4.7 s, two rounds 3.2 s and 0.6 s).
  8 copies beside 24 loops: 37.1 to 40.8 s (fixture 3.6 to 9.4 s, train failed at 18.5 to 22.4 s,
  first round 9.9 to 11.6 s, of which `merge_armed` 7.8 to 9.4 s).

The new tests without their fixes:
- `task_done::a_stop_while_the_run_is_launching_ends_it_once_attached`: FAIL, "timed out waiting for
  the stop" (30.4 s; escalation 60 s). With the fix: PASS in 0.5 to 1.3 s.
- `agent_hooks::a_hook_whose_client_hangs_up_is_still_applied`: FAIL, "the notice was not applied:
  Some(Working)". With the fix: 5/5 PASS (0.18 to 0.24 s). With the layer resuming the handler after
  1 s instead of holding it (hyper's behaviour): without the fix, FAIL when the client hangs up, 3/3
  PASS when it stays connected (control); with the fix, 5/5 PASS.

After the fixes (all commits of this branch), `hammer.sh` with 10 copies of each of the ten tests
of the issues and the two new ones, 16 at once, beside 24 busy loops and the fsync writers, two
rounds back to back (load 16 → 48, then 40 → 53):

| Test | Runs | Failed | Mean | Max |
| --- | --- | --- | --- | --- |
| `agent_hooks::a_hook_whose_client_hangs_up_is_still_applied` | 20 | 0 | 0.9 s | 1.8 s |
| `merge_train::a_red_train_falls_back_to_one_by_one_merges` | 20 | 0 | 43.2 s | 49.0 s |
| `supervision::a_done_task_frees_its_slot_for_a_queued_task` | 20 | 0 | 3.0 s | 3.9 s |
| `task_done::a_background_shell_keeps_the_task_working_until_it_ends` | 20 | 0 | 12.2 s | 13.4 s |
| `task_done::a_done_task_can_be_archived` | 20 | 0 | 3.2 s | 4.2 s |
| `task_done::a_live_monitor_keeps_the_task_working_until_it_ends` | 20 | 0 | 12.3 s | 13.6 s |
| `task_done::a_monitor_that_never_reports_counts_until_it_expires` | 20 | 0 | 19.2 s | 23.4 s |
| `task_done::a_stop_while_the_run_is_launching_ends_it_once_attached` | 20 | 0 | 2.0 s | 2.9 s |
| `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` | 20 | 0 | 20.8 s | 21.8 s |
| `task_done::a_typed_compaction_leaves_a_done_task_done` | 20 | 0 | 5.6 s | 6.5 s |
| `task_done::an_agent_waiting_for_yulius_does_not_need_a_person` | 20 | 0 | 8.1 s | 8.9 s |
| `task_done::an_idle_compaction_leaves_a_done_task_done` | 20 | 0 | 10.3 s | 11.2 s |

"The stop" in the switch test: 0.24 to 0.32 s at the slowest (20.1 s before); in the slot test
0.34 s at the slowest. The red train test's 49.0 s is within its 100 s budget (the old per-phase
bound was 50 s).

Review round 1 (head 5bcdd606, request changes): one major finding, a Cancel or Close that no run
consumed stays in the task's runtime and, with STP-001, shut the next run down at its attach (the
task then read Running with no run). `task_done::a_stop_that_ended_no_run_does_not_end_the_next_one`
without the fix: FAIL, "the resumed run ended: … state: Killed … exit_reason: Some(\"cancelled\")"
(1.1 s); with it 3/3 PASS (3.0 to 3.4 s). Minor findings taken: the attach-time shutdown arms the
escalation; ADR-0115 lists the late effects of a hook finished after its client went on; the hook
test's wording says what it shows and its start wait is bounded; the join error is logged.
`scripts/dev ci` at 5bcdd606: exit 0, 1762/1762 tests.

Review round 2 (head 21549750, approve): `scripts/dev ci` exit 0, 1763/1763. Minors taken: the
launch clears an intent only where it moves a waiting or resting task with no live run to Starting
(it cleared before the status check, so a launch refused for a Stopping task dropped that stop's
intent, and a Continue while a stopped run still ended dropped its); the switch wait in the new test
is bounded. Not separately tested: the launch-time clear alone. The new test passes with either
clear (the second stop's clears first); the launch clear's own path, a stop within milliseconds of
a natural exit, is a race this suite cannot make certain. Pre-existing, left for a follow-up (issue
comment): a stop during a switch's handover drops the switch's sender, and the switch goes on to
resume the task, which can relaunch a task the person stopped.
`scripts/dev security`: not run (unrun, not passed), the Vidoc key answers 401 since 2026-10-14
(#470, ADR-0104).

Also run: `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^supervision::/) |
test(/^task_done::/))'` 51/51; `test(/^merge_train::/)` 7/7; `test(/^agent_hooks::/)` 15/15.
