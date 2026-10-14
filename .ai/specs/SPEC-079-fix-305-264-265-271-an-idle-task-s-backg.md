---
yulius-task: 831042b3-96c4-40eb-bb79-8642438dd5e1
status: done
---

# SPEC-079: an idle task's background work and goal verdict decide its status

Status: implemented
Date: 2026-10-13
Owner: Yulius task #60 (part of task #47, "fix all open actionable GitHub issues")
Scope authorization: task request: fix GitHub issues #305, #264, #265 and #271 following their
acceptance criteria; no new issues.

## Goal

A task's status and its subscription switches follow what its Claude Code session really still
does after a turn ends:
- the queue drains as soon as a Done task's last background work ends (#305);
- a goal task does not read "Your turn" or Done before the goal verdict says so (#264);
- Monitor streams count as background work (#265);
- a proactive switch does not end a background agent launched in the turn that ends (#271).

## Context

Builds on SPEC-032 (STA-001 to STA-005: background agents from `turn_duration`, the goal verdict)
and SPEC-050 SES-101 (a Done task frees its session slot; the queue drains when a task is Done).

- **#305.** `settle_idle` drains the queue at Done. `probe_idle_shells` (finish.rs), when the last
  background shell or agent ends (`previous > 0 && count == 0`), spawns only `advance_chains`, so a
  queued task waits for the 60 s tick. The clean Stop that makes a task Done directly (goal record
  before the Stop) has the same gap: it advances chains and does not drain.
- **#264.** Yulius's Stop hook runs before Claude Code's goal evaluator (another Stop hook) and sets
  `waiting`/`done` at once. The verdict (`goal_status`) comes a second or more later. "Not met"
  makes the task Working again (STA-004), but in between the sidebar, notifications and
  `advance_chains` see an idle task.
- **#265.** Claude Code 2.1.292's records, found in the sandbox transcripts:
  - **Start.** A Monitor starts with a tool result `"Monitor started (task <id>, expires in …)"`
    whose `toolUseResult` is `{"taskId", "timeoutMs", "persistent"}`.
  - **Events and end.** Claude Code writes events and the end as `<task-notification>` blocks with
    `<task-id><id></task-id>`, either as a user prompt (it wakes an idle session) or as a
    `queued_command` attachment (it arrives mid-turn). An event, `<summary>Monitor event: …`, has no
    `<status>`. The end, `<summary>Monitor "…" stream ended</summary>`, has
    `<status>completed</status>`; a stop has `stopped`. Expiry is an event:
    `[Monitor expired after 10m with no events delivered…]`.
  - **No count.** `turn_duration` has no monitor count: `pendingBackgroundAgentCount` was absent at
    turn ends with live monitors.
  - **Processes.** A command monitor runs in Claude Code's Bash wrapper, so the process probe
    (SPEC-016) counts it as a shell. A WebSocket monitor (`ws`) is no process at all and is
    invisible. Every monitor expires after its `timeoutMs` (at most 30 minutes, default 5).
- **#271.** At the Stop hook the turn's `turn_duration`, which counts pending agents, is not
  written yet, so `shells_running_now` misses an agent launched in that turn, and a proactive
  switch restarts the session ("Background agent … didn't finish before the previous session
  ended"). The launch is in the transcript before the Stop: its tool result has
  `toolUseResult.status == "async_launched"` with `agentId`, and its end is a
  `<task-notification>` with that id and a `<status>`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| BGW-001 | When the last background work of an idle task ends, its chains advance and the queue drains at once; a clean Stop that makes the task Done drains it too (SES-101). Claude Code's wake-up prompt (a `<task-notification>`, live or replayed) keeps an achieved goal, as the prompt hook does, so the turn it starts ends Done again. | `task_done::a_done_task_whose_shell_ends_frees_its_slot_at_once` (the shell's end wakes the session with a user prompt, like Claude Code); `finish::tests::a_wake_up_keeps_an_achieved_goal` |
| BGW-002 | The transcript's background launches are observed: a Monitor start (its tool result says "Monitor started (task …"; its expiry is `timeoutMs`, capped at one hour, 5 minutes when absent) and a background agent launch (`async_launched`); a `<task-notification>` with a `<status>` or a "Monitor expired" event ends the one with its task id, unless its note says the agent stopped with background work of its own still running (BGW-006). Sub-agent lines are ignored. | `claude::tests::background_launches_and_their_ends_are_observed`, `observe::claude_goal_mode_records` |
| BGW-003 | An idle task with a live Monitor (started, not ended, not past its expiry) reads Working with it counted, and settles when it ends or expires. | `finish::tests::live_launches_follow_expiry_and_the_turn_record`; `task_done::a_live_monitor_keeps_the_task_working_until_it_ends`, `task_done::a_monitor_that_never_reports_counts_until_it_expires` |
| BGW-004 | A proactive switch decided at a Stop waits while a background agent launched and not yet ended is pending (whatever turn launched it, at most an hour without its notification), as it does for shells and live Monitors. It is made at a later Stop once the agent ended. Before deciding, the Stop hook lets the transcript watcher read the lines written before it (two of its rounds, bounded by 5 s). | `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn`; `finish::tests::live_launches_follow_expiry_and_the_turn_record` |
| BGW-005 | While a goal is active and not achieved, a clean Stop leaves the task Working until the verdict, bounded by `GOAL_VERDICT_WAIT` (20 s): "met" makes it Done (chains and queue follow), "failed" makes it wait, "not met" or agent work after the stop keeps it Working with no idle status published in between, and the turn record without a verdict or the timeout settles it as before. | `finish::tests::the_first_to_take_the_verdict_slot_settles_the_turn`, `finish::tests::a_goal_is_open_until_it_is_decided`; `task_done::a_goal_not_met_never_reads_idle_between_turns`, `task_done::a_failed_goal_leaves_the_task_waiting`, `task_done::a_turn_closed_without_a_verdict_settles_as_before` |
| BGW-006 | A background agent whose `<task-notification>` (`<status>completed</status>`) carries Claude Code's note that it "stopped with background work of its own still running" and that "the result below may be interim" has not ended: it resumes by itself when that work ends, and the same task id notifies again. The note is matched in Claude Code 2.1.292's words, whatever their case and line breaks, only in a `<note>` before the `<result>` (the agent's own text); any other note ends the agent as before. Such an agent is resumable, its TTL counted again from that notification. It counts as the idle task's background work (Working, not Your turn) until the same task id's final notification or an hour after its last one, whatever the turn record after it says, and a switch waits for it (BGW-004). It is one view with the turn record's `pendingBackgroundAgentCount`, not added to it. One whose launch Yulius did not see (a restart) is remembered from the notification. | `claude::tests::an_agent_that_stopped_with_its_own_work_running_has_not_ended`; `finish::tests::a_resumable_agent_counts_until_its_final_notification`; `task_done::an_agent_that_stopped_with_its_own_work_running_keeps_the_task_working` |

## Plan

- [x] Spec written and validated
- [x] A Done task whose last background work ends drains the queue at once
- [x] Background launches and their ends are read from the transcript
- [x] An idle task with a live Monitor reads Working until its stream ends or expires
- [x] A proactive switch at a Stop waits for a background agent launched in that turn
- [x] A goal task's clean Stop waits for the goal verdict before it reads idle
- [x] Docs, ADR, independent review and every gate green
- [x] An agent that stopped with background work of its own running counts until its final
  notification (BGW-006, 2026-10-14)

## Acceptance criteria

- The four issues' acceptance criteria hold, each shown by the tests named above.
- The SPEC-032 integration tests (`task_done`) still pass: the shell, pending agent, goal not
  met, continued stop and replay cases.
- `scripts/dev ci` passes.

## Test plan

- Unit: `claude.rs`, records for a Monitor start, an agent launch and the notifications that end
  them (prompt and `queued_command`). `finish.rs`, the live count of background launches with
  expiry, and the verdict decisions.
- Integration with the fake agent (`--goal-monitor`, `--goal-agent` with the launch record,
  `--goal-verdict`), in `task_done`: the queue drains at once when a Done task's shell ends under
  session limit 1; the live and the silent monitor; the deferred switch; no idle status while a
  goal is not met; a failed goal and a turn closed without a verdict.

## Decisions

- **Track launches from the transcript, keep Claude Code's agent count.** Monitors and agent
  launches are read from the transcript, while `pendingBackgroundAgentCount` stays the count of
  agents once a turn closed (SPEC-032). Launches fill the two gaps the count leaves: monitors,
  which it never counts, and the turn that has not closed yet (#271). The map is in memory and
  bounded (64 entries; expired monitors are dropped). Launches are ordered by when Yulius read
  them, the clock of the turn record. A restart forgets it, which loses at most
  a WebSocket monitor's count until it expires; command monitors stay counted by the process probe.
- **Defer only the idle status, not the hook.** Claude Code's evaluator runs after Yulius's Stop
  hook, so the hook cannot wait for it. The hook returns, and whoever comes first settles the
  turn: the verdict, agent work, a new prompt or stop, the turn record, or the timeout (one
  atomic slot, compare-and-swap).
- **Read the transcript before deciding the switch.** The Stop hook waits for the watcher's next
  round (bounded) instead of reading the file itself. It waits only when a switch was decided, which
  is rare, and the watcher stays the one reader.

ADR: [0063](../../docs/decisions/0063-background-work-and-goal-verdict-from-the-transcript.md).

## Evidence

- **Claude Code's records, from the sandbox transcripts (2.1.292).**
  - `toolUseResult` `{taskId, timeoutMs, persistent}` with "Monitor started (task …)";
    `{status: "async_launched", agentId}`.
  - `<task-notification>` ends: `<status>completed|failed|stopped</status>`, "Monitor expired".
  - `turn_duration` had no `pendingBackgroundAgentCount` while monitors were live.
  - A live `Monitor` of this session ran as `/bin/bash -c … eval '<cmd>' … pwd -P >| …`, so the
    process probe counts a command monitor.
- **Red first.** Each fix was removed in turn and its test failed:
  - the drains (#305): the queued task never started;
  - the wake flag: the woken task settled Waiting and held the slot;
  - monitors in the count (#265): no monitor counted;
  - the launch guard (#271): the switch was made while the agent was pending;
  - the verdict wait (#264): `waiting` was published before Done.
  The files were restored byte for byte after each run.
- **Tests.**
  - `cargo nextest run -p yulius-app -p yulius-harness`: 270 passed.
  - `task_done` integration file: 27 passed.
- **Independent review, three rounds in fresh read-only sessions.**
  - Round 1 requested changes:
    - agents launched in earlier turns were not counted;
    - ADR number collision with dev;
    - probe before chains;
    - a late launch was not probed;
    - clocks;
    - Monitor detection;
    - verdict evidence;
    - the watcher wait.
  - Round 2 requested changes:
    - the renumber had rewritten task #62's ADR-0061 references;
    - wake-up prompts reset an achieved goal, so #305 did not hold in production;
    - the failed-verdict test did not prove the verdict settled the turn;
    - the cooldown wait.
  - All findings were fixed test-first. Round 3 approved `0d396c4`. Its nits were left:
    - the prompt hook's wider wake-up test;
    - a duplicate `switch_cooling` call;
    - the fake holding its lock for 6 s.
  - A race was accepted without change: a typed prompt landing between `settle_waited_turn`'s
    read and its write. The window is the same as on the Stop path that already existed.
- **Gate.** `scripts/dev ci` on `0d396c4` (merged with dev at 567586e) passed:
  - 1428 tests run, 1428 passed;
  - format, clippy, deps and repository contracts.
- **Not tested here:** a real Claude Code session against a deployed Yulius. Everything above
  runs the fake agent, shaped on the real records.
- **BGW-006 (2026-10-14), task #74 after build 8ee0e53.** One false "Your turn" in the live run:
  - 00:32:18 the review agent `a994b4d8…` launched; 00:32:20 the turn record counted it.
  - 00:34:53 its notification said `completed` with the note "This agent stopped with background
    work of its own still running … the result below may be interim". The harness read it as the
    agent's end; the turn record at 00:34:55 had no `pendingBackgroundAgentCount`.
  - The reviewer's own test shells kept the task Working through the process probe until about
    00:39:19. Then nothing counted: Your turn, which notifies the person.
  - 00:39:49 the final notification of the same task id ("… stops with no live background
    children of its own") woke the session.
  - The sandbox's transcripts hold 6 such interim notifications in 5 sessions (2026-10-12 to
    2026-10-14). The final one of the same id followed each, 55 s to 5 minutes later. None of
    the 5 turn records in between counted the agent.
  - Red first: without the note's match, the harness test failed and the integration test read
    idle with nothing running before the final notification. The same happened when the
    notification ended the launch. Adding resumable agents to the turn record's count failed the
    double-count case of the unit test (2 instead of 1).
