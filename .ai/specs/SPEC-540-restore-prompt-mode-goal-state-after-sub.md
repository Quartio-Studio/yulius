---
yulius-task: 471b3694-e07c-467b-b0bc-29a0822f3fbb
status: in-progress
---

# SPEC-540 — Restore prompt-mode goal state after subscription switches

## Goal

Keep a prompt-mode task working while Claude Code evaluates its person's goal after a subscription switch.

## Context

Issue #1182 records a real session and inspections of Claude Code 2.1.292, 2.1.311 and 2.1.322. On resume Claude Code restores the condition from the last open goal status in a trusted workspace where policy permits hooks. Yulius resets its goal flags in start_watcher and watches only records written since the attempt started. SPEC-468 carried unfinished work across idle moves, but a failed goal can be unfinished and closed. The fake agent previously installed its evaluator only for an explicit goal command.

## Requirements

- PRG-001. For manual, turn-boundary and usage-limit resumes, interpret the last goal status of the prompt task's Claude conversation. Set and not-met are open. Met, failed and cleared are closed.
- PRG-002. At a clean Stop with an open goal, apply the existing verdict wait. A new verdict, turn record or the existing 20-second deadline resolves it. When trust or policy prevents Claude's restore, Yulius types nothing and follows the same bounded fallback.
- PRG-003. The fake Claude restores the last open condition on resume and supports forgetting it to simulate a restricted workspace. Goal-mode launches keep their existing once-per-resume submission.

## Plan

- [x] Specify goal recovery from the resumed conversation
- [ ] Reproduce goal loss across subscription moves
- [ ] Restore prompt goals in the watcher and fake agent
- [ ] Verify the fix and obtain independent review

## Acceptance criteria

- AC-1. A manual move, pool rotation and resume past a limit each keep the person's goal open before a new verdict. The next clean Stop leaves the task working through evaluation.
- AC-2. A resumed fake process evaluates the last open condition without a second goal command. A final met, failed or cleared record and the forget option suppress that evaluation.
- AC-3. An existing goal-mode switch test observes one goal submission in the new run.

## Test plan

Exercise the supervisor with a detached PTY host and the fake CLI, including Stop hooks. Run the fake CLI against synthetic transcripts and inspect the records it appends. Run the existing goal-mode resume tests. A signed-in live Claude session is outside the automated fixtures.

## Decisions

[ADR-0386](../../docs/decisions/0386-prompt-goals-follow-the-resumed-claude-conversation.md) defines the restore assumption and the fallback.
