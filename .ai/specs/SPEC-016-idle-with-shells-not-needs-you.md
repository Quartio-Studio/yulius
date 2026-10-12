---
yulius-task: 99b109c2-ad46-4a92-858d-70fe61d3f63c
status: in-progress
---

# SPEC-016: an idle agent with background work running does not "need you"

Status: draft
Date: 2026-10-12
Owner: Yulius task #14
Scope authorization: user request (screenshot of task #10): "It marked 'needs you' even if it
didnt need me. Fix."

## Goal

A task whose agent ended its turn but still has background shells running ("1 shell, 2 monitors
still running" in Claude Code's footer) reads as Working, not "Your turn", and is not counted
under "Needs you". The agent wakes by itself when the shell reports.

## Context

- The screenshot shows task #10 listed under "Needs you" with a "Your turn" chip and "The agent is
  waiting for your reply", while its footer says a shell and monitors still run and the agent text
  says "I'm waiting for it to finish".
- TSK-014 already makes an achieved goal (activity `done`) with background shells read as Working
  (`statusInfo` in `status.js`), but activity `waiting` (goal not achieved) ignored the shells.
- The server counts "needs you" with `AttentionFacts::needs_you` (yulius-domain), the same rule.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NEY-001 | An idle `waiting` task with background shells known to run reads as Working (group `working`) with its shells said; without shells it reads "Your turn". | `statusInfo`, `status.js` |
| NEY-002 | The server's "needs you" count agrees: `waiting` with background shells does not need a person. | `AttentionFacts::needs_you` test |
| NEY-003 | A permission prompt (`needs_attention`), held prompt, browser request and failing checks still need a person whatever the shells. | same test |
| NEY-004 | When the last shell ends the task is published again and returns to "Your turn". | existing `probe_idle_shells` publish |

## Design

`AttentionFacts` gains `background_shells`, filled from the probe's last count for a finished
turn. `needs_you` treats `Waiting` as needing a person only when it is false. The UI applies the
same rule in `statusInfo`.

## Acceptance criteria

- The domain test for the rule passes; `scripts/dev ci` passes.
- Limit: shells are detected by Claude Code's Bash wrapper; Monitor tool streams that are not
  such shells are not counted, so a task with only monitors still reads "Your turn".

## Test plan

- `needs_you_follows_the_sidebar_rule` covers NEY-002 and NEY-003.

## Plan

- [ ] Idle agent with running shells reads Working in the UI and server counts
- [ ] Gate green

## Decisions

- Reuse the TSK-014 background-shell count instead of a new signal.
