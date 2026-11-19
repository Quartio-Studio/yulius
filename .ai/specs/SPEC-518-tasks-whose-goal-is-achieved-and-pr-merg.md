---
yulius-task: 4b0ec4f3-090b-46e8-a7cb-672e088b7118
status: done
---

# SPEC-518: a finished task is shown Done, and what then

## Goal

The person's words (2026-11-19, about tasks #492, #493, #495, #496, #497 and #499): "tasks have
goal achieved but aren't marked as done". Each of them was still `running` after
Claude Code recorded `✔ Goal achieved` and its pull request was merged into `dev` on green
Yulius CI. A task in that state must read as finished to its person. Before anything changes, this
spec records what the person's pages actually showed, the cause, and the options, and asks the
person which behaviour they expect.

## Context

### What the records say (read 2026-11-19 13:42 UTC by the Superagent, and here)

| Task | PR | Goal achieved | Server record at 13:42 |
| --- | --- | --- | --- |
| #492 | #1419, #1425 | 12:06 | `running`, activity `done` since 12:06:42Z |
| #495 | #1422 | 12:06 | `running`, activity `done` |
| #496 | #1424 | 11:54 | `running`, activity `done` |
| #497 | #1423 | 12:19 | `running`, activity `done` |
| #493 | #1420 | 13:19 | `running`, activity `done` |
| #499 | #1430 | 13:30 | `running`, activity `done` |

For all six: `goal_unfinished` false, `spec_unfinished` false, `background_shells` 0,
`merge_armed` false, `yulius_wait` false. Every step of their plans was done.

### Each suspect, checked

- **Idle compaction.** #492's and #495's transcripts end with Claude Code's idle compaction
  (`compact_boundary`, the summary record with `isCompactSummary`, and the informational
  "Compacted while idle, before the prompt cache expired" at 13:01 UTC), an hour after the goal.
  The server's activity stayed `done` since 12:06:42Z, so the compaction changed nothing.
  SPEC-085 STT-001 already made these records start no turn, tested by
  `task_done::an_idle_compaction_leaves_a_done_task_done`. Ruled out.
- **A background shell or `yulius wait`.** `background_shells` 0 and `yulius_wait` false. Ruled out.
- **A person's request left open.** None of the five other tasks called `needs_you`. #499 called it once
  and resolved it twice before its goal. Ruled out.
- **Today's merges.** #1419/#1425 (take control) changed only a comment in `status.js` and the
  control paths in `tasks.rs`. #1420, #1422, #1424, #1428 and #1430 do not touch `statusInfo`,
  `taskInfo`, `goal_is_open` or the activity. Ruled out.
- **The goal status hook.** activity `done` is set from the recorded goal (`goal_observed`), so it
  was recorded. Ruled out.

### What the person's pages showed

With these inputs the web's one rule (`statusInfo` and `taskInfo` in
`crates/yulius-server/web/assets/status.js`) returns `Done` (`group: 'done'`, a green check).
With the merged pull request it returns "Done · PR #N merged". The sidebar's Active list puts the task in
its **Done** group, with "done 1h ago" (`tasktime.js`). The server's `AttentionFacts` count it the same
way. So the six were shown **Done**, and they stayed in the Active list as `running` with a live
Claude Code session.

What did not happen: nothing **closes** such a task. A Done task stays `running` until a person
archives, closes or deletes it (TSK-014, SPEC-039). The only automatic path is SPEC-229 RBT-001: a
run lost while its task was Done (a reboot, a lost process) records `Closed`. That is why earlier
tasks of this sandbox (#479, #447, #449, … all updated 2026-11-12 07:44 UTC) read `closed`, and
today's six did not: today's server update reattached their live sessions, so no run was lost.

### Cause (working conclusion)

No part of the Done rule disagreed. The person expects a task whose goal is achieved and whose
pull request merged to be **closed** (to leave the Active list as finished, as the earlier tasks
did after a restart), and Yulius only marks it Done and leaves it open. The person has not yet
confirmed which page and label they saw, so this is asked before any behaviour changes.

### Options

1. **Keep it as it is.** Done tasks stay in the Active list's Done group until a person archives
   them, from the task menu or the Done group. Nothing changes. The next restart still closes some
   of them and not others, which is the inconsistency the person noticed.
2. **Close a finished task by itself after a short grace period (recommended).** A task that has been Done for
   a grace period (default 15 minutes, so a person can still type a follow-up) is closed by
   Yulius. Done here means: its goal achieved, or its plan complete with its pull request merged,
   with no background shell, no armed merge and no request to a person. Closing it ends its Claude Code session and keeps its final
   screen and transcript, as RBT-001 does. It leaves the Active list, and **Continue** resumes the
   session. One rule, the same after a restart or not.
3. **Close at once, when the merge lands.** Like option 2, but with no grace period. It is simpler, but a person reading the
   result can no longer type into the live session without Continue.

## Requirements

- DON-001: The finding above (shown Done, never closed by itself) and its options are given to the
  person with `needs_you` before any behaviour changes.
- DON-002 (if the person chooses option 2 or 3): a running task that stays Done (`statusInfo` group
  `done` by the server's rule: goal achieved, or plan complete and pull request merged or
  tested; no background shell, no `yulius wait`, no armed merge, no `needs_you`) for the grace
  period is closed by Yulius, as a person's archive closes it (`shows_done`, then
  `close_task_now`). An agent that only answered (SPEC-429 "Answered", no goal, no plan) stays
  open: its person may still follow up. Its final screen is kept and
  Continue resumes it. A prompt, a shell or any activity in the period cancels it.

## Plan

- [x] Trace the six tasks' records, transcripts and the Done rule, and write the finding and options
- [x] Ask the person which behaviour they expect (needs_you), and record the answer

## Acceptance criteria

- The person's answer is recorded in Decisions.
- With option 2 or 3: an integration test that fails on today's `dev` shows a running task whose
  goal is achieved and whose pull request merged, after Claude Code's idle compaction too, reading
  `closed` once the grace period passed. A prompt typed within the period keeps it running.
- With option 1: no code changes. The finding is the result, and the steps after the question are
  skipped with that reason.

## Test plan

- Integration (`crates/yulius/tests/task_done.rs`, fake agent): goal achieved → idle compaction
  (`--idle-compact-ms`) → grace period elapses on the test clock → task `closed`, `closed_at`
  set, final screen kept. Failure cases: a prompt in the period, a background shell, an armed
  merge or an open `needs_you` keep it running.
- Unit: the grace period's start and reset follow the activity's `state_since`.

## Decisions

- The six tasks were shown Done (by the evidence above). The open question is whether a Done task
  should also close by itself, which changes what a person can do with a finished session, so the
  person decides (DON-001).
- The person answered "mark as done" (2026-11-19): the task ends with the finding, and no
  behaviour changes (option 1). DON-002 and its test, code, docs, review and merge steps were not
  done. Options 2 and 3 stay recorded above for a later task.

## Evidence

- Server records (read by the Superagent at 13:42 UTC): all six `running`, activity `done`,
  `goal_unfinished` false, `spec_unfinished` false, `background_shells` 0, `merge_armed` false,
  `yulius_wait` false; #492 `state_since` 12:06:42Z, after its 13:01 idle compaction.
- Transcripts (`home/.claude/projects/...-worktrees-<task-id>/*.jsonl`): #492's ends with
  `compact_boundary`, the `isCompactSummary` record and "Compacted while idle" at 13:01:18Z; no
  `needs_you` call in #492, #493, #495, #496, #497; #499's one request resolved before its goal.
- Code read: `statusInfo`/`taskInfo` (`status.js`) give `Done` for those inputs; `git log` since
  2026-11-17 shows no change to the Done rule (#1419 only a comment in `status.js`); the only
  automatic close is RBT-001 (`close_task_now` on a lost Done run); `task_done::an_idle_compaction_leaves_a_done_task_done` covers the compaction.
- Not tested: what the person's browser rendered (the runs were closed before this task started,
  and the server's logs are outside the sandbox).
