---
yulius-task: 1bd3a9e6-137a-4d87-aa41-1d48bb6a750b
status: draft
---

# SPEC-032: a task's status follows what its agent is doing

Status: draft
Date: 2026-10-12
Owner: Yulius task #30
Scope authorization: user request (three screenshots of tasks #23, #24 and #25): "Recently I got a
lot of wrong status messages. Analyse previous tasks and what happened, and fix it", and "for
example - task was marked as done but another agent was still working there".

## Goal

A running task's sidebar group and label (Working, Your turn, Needs your action, Done) agree with
what its Claude Code session is doing. No more "Done" while a background agent still works, "Your
turn" between the goal checks of a session that goes on, or "Needs you" for a notice nobody has to
answer.

## Context

Evidence: the three screenshots, and the Claude Code transcripts of tasks #23 and #24 on the
sandbox (`~/.claude/projects/*/*.jsonl`, Claude Code 2.1.292).

- Screenshots 2 and 3 (#24, #25): the task sits under DONE with a green PR badge while the terminal
  says "Waiting for 1 background agent to finish" and the agent list shows a running reviewer. The
  transcript of #24 at 15:52:58 shows the cause: the agent launches an `Agent` tool call in the
  background and ends its turn; Claude Code writes `turn_duration` with
  `"pendingBackgroundAgentCount": 1`, and 52 s later wakes the session with a `<task-notification>`.
  Yulius reads the idle agent's background work only from a process probe for Bash shells
  (SPEC-016), so a background agent is invisible: the clean Stop makes the task idle, and
  `taskInfo` turns "idle + pull request tested + plan complete" into Done (the same rule shows
  "Your turn" without a pull request).
- Screenshot 1 (#23): the task is listed under NEEDS YOU with the alert icon (`needs_attention`)
  while the agent runs a foreground Bash command ("Running… 53s"). `needs_attention` is only set by
  a `Notification` hook, the gate, a browser request or a goal that was never taken. Claude Code
  2.1.292 sends notification types beyond the permission prompt: `agent_completed` (a background
  agent finished, the review agent of #23 ended at 15:40:18, the turn that showed the alert began
  then), `push_notification`, `quota_auto_resume_*`, `computer_use_*`, `model_refusal_fallback`,
  `auth_storage_failure`. `notification_needs_a_person` treats every type except `idle_prompt` and
  `auth_success` as a person's turn, and nothing takes `needs_attention` back while the agent goes
  on: only a typed prompt or a Stop does.
- Between goal checks (all Yulius tasks run under `/goal`): the Stop hook of Yulius ends the turn
  (`waiting`, or Done with a tested pull request), then Claude Code's goal evaluator answers "not
  met" (`goal_status` with `met: false`, followed by a `Stop hook feedback` user line with
  `isMeta: true`) and the agent carries on. `goal_observed` ignores that verdict, and the meta line
  is not a prompt, so the task reads "Your turn" (or Done) until the next prompt. #24 did this at
  15:55:22 to 15:55:30 (end of turn, verdict, end of turn, verdict).
- After a Yulius restart (every merge to `dev` deploys one) the transcript replay folds the final
  state: `TurnComplete` closes the turn, `stop_hook_summary` closes it again, and an assistant
  line after a continued stop reopens nothing, so a session that is working reads as idle. Claude
  Code writes `stop_hook_summary` also when a Stop hook blocks (#23 at 16:22:08, the turn goes on);
  only `turn_duration` marks the true end.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| STA-001 | A turn that ends with background agents pending (`turn_duration.pendingBackgroundAgentCount` above 0) leaves the task Working, with the count said, not Your turn or Done; it is idle again when the count is 0 at the next turn end. Chained tasks and the Done rule see it as busy. | `claude.rs` observe test; `finish.rs` tests; task_done integration test |
| STA-002 | Only a notice that asks a person (`permission_prompt`, `worker_permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`) makes a task `needs_attention`; any other type, known or new, leaves the status alone. | `hooks.rs` unit test; `agent_hooks.rs` |
| STA-003 | A task flagged by such a notice is Working again once the agent produces work after the notice (an assistant line, not before the notice); a flag set by the gate, a browser request or an untaken goal is not cleared that way. | `finish.rs` and `agent_hooks.rs` tests |
| STA-004 | A goal verdict of "not met" makes an idle task Working again (the session carries on); "achieved" still makes it Done, "failed" leaves it waiting. | `finish.rs` decision test; task_done integration test |
| STA-005 | An assistant line that is not a turn's end (`stop_reason` other than `end_turn`) after a recorded turn end shows the turn continues, live (the task is Working) and in a replay. `stop_hook_summary` no longer closes a turn: only `turn_duration`, an interrupt or a failed goal does. | `claude.rs` and `finish.rs` tests |

## Design

- `Observed::TurnEnded` carries `background_agents: Option<u32>` and comes only from
  `turn_duration`. New `Observed::AgentWork { at }` for an assistant line that is not `end_turn`
  (sidechain lines are ignored, also for `TurnComplete`). The app's `Observation` mirrors both.
- The runtime keeps `background_agents` per run (reset at each turn end). `probe_idle_shells`
  reports `max(process probe, background agents)` as the idle task's background work, so every
  consumer of `background_shells` (UI, `AttentionFacts`, chains, `shells_running_now`) is correct
  without a new field. The UI says "background task(s)" instead of "background shell(s)".
- `on_observation`: `AgentWork` after the last stop (the `UserTurn` guard on `last_stop_ms`)
  makes an idle task Working, and takes back a notice-set `needs_attention`
  (`runtime.notice_attention_ms`); `Goal(NotMet)` makes an idle task Working.
- `notification_needs_a_person` becomes an allow-list. Older CLIs without `notification_type` keep
  the text rule.

What else the cause could break: Monitor tool streams are still not counted (SPEC-016 limit); a
goal that is never met and a question to the person under `/goal` now read Working while Claude
Code keeps asking the model, as they are. Yulius's Stop hook still sets "Your turn" for the second
or two before the goal verdict arrives (follow-up issue).

## Plan

- [ ] Spec with root causes written and validated
- [ ] A turn that ends with a background agent still running reads Working, not Done
- [ ] Informational Claude Code notices no longer flag Needs your action; an answered prompt returns to Working
- [ ] A goal the evaluator rejects, or a stop the hook continues, keeps the task Working live and after a restart
- [ ] Independent review, docs and ADR, every gate green, delivered to dev

## Acceptance criteria

- The transcript shapes quoted above (turn end with a pending agent, a blocked stop, a goal verdict
  of "not met") are fixtures of the tests and read as in STA-001 to STA-005.
- `scripts/dev ci` passes; the e2e suite passes for the UI wording.

## Test plan

- Unit: `claude.rs` (records), `finish.rs` (fold and decisions), `hooks.rs` (notification types).
- Integration: `agent_hooks.rs` (notifications), `task_done.rs` with the fake agent (a turn that
  leaves a pending agent, a rejected goal).
- e2e: the idle-with-background-work label.

## Decisions

- Trust Claude Code's own count (`pendingBackgroundAgentCount`) over tracking `Agent` launches and
  `<task-notification>` ids ourselves: it is exact, survives mid-turn absorbed notifications and
  restarts, and needs no state per tool call.
- Allow-list notices rather than deny-list: the CLI adds informational types with releases, and a
  false alarm costs the person's attention each time.
- Reuse the `background_shells` plumbing for agents (one count of "background work") instead of a
  second field through the store, API and UI.
