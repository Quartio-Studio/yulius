---
yulius-task: d8beda05-6739-4c5a-9afd-7f58aeb205cf
status: in-progress
---

# SPEC-453: A prompt read from the transcript never undoes its own turn's clean Stop

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #435
Scope authorization: task #435, fixing #1066 ("Flaky a third time:
`task_done::a_goal_the_tui_dropped_is_submitted_again` times out waiting for Done in the full
suite"). Extends SPEC-346 LPH-002 / ADR-0234 (the per-task turn order) from the prompt hook to the
prompt the transcript watcher reads; keeps SPEC-079 BGW-005 (a clean Stop waits for the goal
verdict).

## Goal

A turn whose clean Stop reaches Yulius at the moment the transcript watcher applies that turn's
prompt still ends: a goal task reads Done once Claude Code records the goal as met, and any other
task reads Your turn, instead of staying Working for good with the agent idle.

## Context

- **Symptom (#1066, task #402 at 97e3f299):** in a full `scripts/dev ci` run the test failed with
  `Error: "timed out waiting for Done"` after 62.4 s (3102 of 3103 passed). Alone it passes in
  about 17 s. Third report after #243 (fixed by #340) and #921 (fixed by #960, SPEC-346).
- **Reproduced** with temporary probes (never committed):
  - E1, forced: the goal turn's Stop held 1.5 s before its turn lock, and the watcher, right
    after its last-Stop check of that turn's prompt, waiting until the Stop opened its verdict
    wait: the test fails with `Error: "timed out waiting for Done"` at 61.1 s, the issue's
    signature.
  - E4, statistical: 100 ms of delay at each of the two points and the watcher's start shifted
    by a random 0-999 ms: 2 of 48 runs fail the same way (61.13 s and 61.09 s), 46 pass. Of the
    3 runs in which the watcher read the goal's records within the Stop's latency (18, 19 and
    57 ms after they were written), 2 failed.
  - Negative: E2, the watcher 13 s late on the goal's records (the triage's lead (a), stray
    Enters of the submit retries): passes in 29.5 s. E3, the same 100 ms delays without the phase
    shift: 32 of 32 pass, because an unloaded launch locks the watcher's 1 s poll about 410-425 ms
    behind the goal's records, far behind the Stop (about 11 ms).
- **Which wait runs out:** the test's 45 s wait for Done. Nothing in the server is waiting any
  more: the task reads Working and nothing will end the turn again.
- **Root cause: a check-then-act race in the watcher's prompt.** `on_observation`'s
  `Observation::UserTurn` arm (`crates/yulius-app/src/supervisor.rs`) checks "a prompt of a turn
  whose clean Stop already came is not a new turn" (`at <= last_stop_ms`), then awaits the store
  (`hold_overtaken`), then starts a new turn: it clears the verdict wait (`verdict_stop_ms`) and
  sets Working. The turn's Stop hook runs on another task. When `clean_stop`
  (`crates/yulius-app/src/finish.rs`) stamps `last_stop_ms` and opens the verdict wait between
  that check and that clear, the watcher wipes the wait the Stop just opened. Claude Code's "goal
  met" then finds no waited turn (`goal_observed` → `settle_idle(None)`), which leaves a Working
  task alone, and `await_goal_verdict` gives up because its wait is gone. Seen in the failing E4
  runs: `clean_stop` at …399 with the goal open, the prompt passing its check with the wait
  already open at …400, "met" applied with no wait, the task Working.
- **The same race without a goal:** the Stop reads the task (Your turn: its prompt hook was lost,
  or the turn is Claude Code's own wake-up), the watcher's prompt passes its check and sets
  Working, the Stop stamps and settles from its copy, which already read Your turn, so
  `set_task_activity` writes nothing: Working for good. The window is `hook_stop`'s gates and
  rotation reads, between the Stop's read of the task and `clean_stop`.
- **Why the earlier fixes missed it:** SPEC-346 ordered the prompt *hook* with the Stop
  (`turn_order`, `turn_end_ms`). The transcript's prompt, the fallback for a lost prompt hook,
  takes no part in that order and checks a different mark (`last_stop_ms`, stamped without the
  lock).
- **Why only under load:** unloaded, the watcher reads the turn's records hundreds of ms after
  the Stop was answered. Load moves the poll's phase and stretches the Stop's handling, which is
  the window.
- **The triage's other leads:** (b) the clocks of the ADR-0234 guard and (c) a watcher stall past
  45 s did not show in any run; the cause above explains every failing run.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TPO-001 | A prompt the watcher reads from the transcript and a clean Stop of the same task are ordered by the task's turn order (ADR-0234): the watcher holds it from its last-Stop check until the new turn is started (verdict wait cleared, Working set), and a clean Stop takes it to remember its time. A Stop arriving meanwhile opens its verdict wait, or settles, after that prompt; a prompt applied after the Stop remembered its time is not a new turn. | `task_done::a_goal_whose_stop_races_its_transcript_prompt_is_done` (fails without: "timed out waiting for Done", the #1066 signature) |
| TPO-002 | A clean Stop settles the task as it is once it remembered its time, not as the Stop read it on arrival: a prompt the watcher applied in between (Working) is settled, Your turn or Done. | `task_done::a_stop_that_raced_its_transcript_prompt_settles_the_turn` (fails without: Working) |
| TPO-003 | `task_done::a_goal_the_tui_dropped_is_submitted_again` passes with the race forced (the E1 probes) and under the E4 probes, where it failed before. | The probe runs, in Evidence |

## Plan

- [x] Reproduce the stuck Working under probe delays and find the wait that runs out
- [ ] Spec with the root cause, the fix and its regression tests, and a progress comment on #1066
- [ ] Regression tests that fail without the fix: a wiped verdict wait and a stale Stop copy
- [ ] Fix: the transcript's prompt and the clean Stop take the turn order
- [ ] Proof on the original test: the forced race and the phase loop pass with the fix
- [ ] ADR, architecture, changes entry and the full gate
- [ ] Independent review and its findings answered

## Acceptance criteria

- AC-1 (issue: reproduce under load and find which wait runs out): done, see Context.
- AC-2 (issue: fix the race test-first): each new test fails without the fix and passes with it.
- AC-3: `task_done::a_goal_the_tui_dropped_is_submitted_again` passes with the probes that made it
  fail (TPO-003). No quarantine, no retry.
- AC-4 (issue): the full `scripts/dev ci` passes with the test included.

## Test plan

Two `race` pause points (test-support builds only; no-ops in production), keyed by the task id:
`UserTurnChecked`, in the watcher right after a transcript prompt passed its last-Stop check, and
`CleanStopLock`, at the start of `clean_stop`, before it remembers its time. The tests are end to
end in `task_done` (the real hook client, the fake agent's transcript, the real watcher), because
the watcher has no entry of its own.

- TPO-001: a goal task that reached Done gets a second goal typed (`/goal …`). Its turn's Stop is
  parked at `CleanStopLock` until the watcher's prompt reached `UserTurnChecked`; the watcher then
  gives the Stop 2 s to be answered (a test layer on the internal listener counts answered Stops)
  before it goes on. Without the fix the Stop is answered at once (its verdict wait open), the
  watcher clears the wait, and "met" leaves the task Working: the test times out waiting for Done.
  With the fix the Stop waits for the turn order, the watcher's 2 s run out, the prompt starts its
  turn, the Stop opens its wait after it, and "met" settles Done. The test also checks that both
  points were reached, so it never passes without the race. Either order of arrival at the two
  points gives the same result.
- TPO-002: a prompt task that reads Your turn gets a line typed whose prompt hook never reaches
  Yulius (the test layer answers it, as a hook client failing open would). Its Stop is parked at
  `CleanStopLock`, after it read the task, until the watcher's prompt set Working; then it is
  released and must leave Your turn. Without the fix it settles from its copy and the task stays
  Working. (When the watcher happens to apply the prompt before the Stop reads the task, a few ms
  of each second, the Stop reads Working itself and the run proves nothing; it still passes.)
- TPO-003: the reproduction's probes applied on top of the fix, for the evidence only.
- What else the cause could break, checked by the existing suites: the prompt hook's order
  (`intake`, SPEC-346), goal verdicts and woken turns (`task_done`), the transcript fallback for
  lost hooks (`session_sync`), holds a prompt overtakes (`task_pause`, `subscription_pool`).

## Decisions

- **The turn order's lock, not a second check.** Checking `last_stop_ms` again after the await
  still leaves the store write of Working behind the check; a Stop settling in between would be
  overwritten. ADR-0234 chose the lock for the prompt hook for the same reason, and the
  transcript's prompt is that hook's fallback.
- **Keep the watcher's own mark (`last_stop_ms`), stamped under the lock.** Checking
  `turn_end_ms` instead would need no change in `clean_stop`, but it also moves for Stops that
  did not end the turn (a gate block, a held Stop), whose turn's prompt must still start the turn
  for the watcher.
- **The Stop takes the lock only to stamp, then reads the task again.** Once the time is
  stamped under the lock, no prompt of this turn changes the task any more (the hook's is
  ordered by `turn_end_ms`, the transcript's by `last_stop_ms`), so the settle needs no lock, as
  in `hook_event`. The lock is held across the watcher's own store writes only, never across a
  hold or a scope check.
- **Two pause points, not sleeps.** The existing `StopTurnLock` sits before the Stop's first
  lock, where the fix's first half alone would already order it; `CleanStopLock` also proves the
  stamp is under the lock.
- **Not done here, pre-existing, noted on #1066:** `await_goal_verdict`'s 20 s expiry landing on
  the next turn's prompt (heals at the next work line); a failed Stop's `settle_unanswered_turn`
  against the next turn's prompt; a transcript prompt without a timestamp always starts a turn;
  the opening Stop of a resumed goal session stamps no time; a late Stop (its client gave up)
  against the next turn's prompt (ADR-0115: heals with the agent's next work).
