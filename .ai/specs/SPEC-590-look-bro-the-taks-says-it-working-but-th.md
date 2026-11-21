---
yulius-task: 2b0ce1c0-efb5-419f-94ab-2b216f995635
status: in-progress
---

# SPEC-590. Continue a stopped Codex task in its saved conversation

## Goal

When a person restarts a stopped Codex task, its work resumes without a second message through the Superagent. A Codex conversation reopened for input shows its actual activity.

Fixes #1601.

## Context

In the screenshot, task #34 has an unfinished plan and an interrupted conversation. The Superagent reports that restarting left the prompt empty. The capture also shows a subsequent message already in a new turn. The other sandbox's earlier live state is unavailable here.

The page's Continue action and the Superagent's `task_restart` share `continue_task`. Its ended-task path always requests a resume without a prompt. Codex reopens the saved rollout, and `launch_attempt` defaults its activity to Working. An empty session sends no turn-end hook to settle that state. [SPEC-503](SPEC-503-accept-outside-pr-1146-codex-returns-as.md) tested native resume after a usage limit. [SPEC-558](SPEC-558-parked-codex-tasks-no-codex-exit-screen.md) tested a message waking a parked task. Neither exercises Continue after Stop.

## Requirements

- CCR-001. Continue on a stopped Codex task with a saved conversation submits one continuation in that conversation. A retry after a failed start follows the same rule. A task without a saved conversation starts with its original request.
- CCR-002. A native Codex resume that opens without a turn shows idle activity. A new prompt starts work, and a recorded interruption returns the live task to Waiting. Opening a closed conversation allows the person to supply the next prompt.
- CCR-003. The existing session queue and capacity pause retain the continuation until the requested start can run. A person stopping an agent authorizes no automatic restart.

## Plan

- [x] Trace and reproduce the stopped Codex task
- [x] Prove the interruption and resume behavior with regression tests
- [x] Keep task activity and resumed work in sync with Codex
- [ ] Verify and independently review the fix
- [ ] Record verification evidence for the merge queue

## Acceptance criteria

- AC-1. Stop a task with a known Codex session, then Continue. The resumed agent processes a continuation in the same session with no further operator input.
- AC-2. Close and reopen a Codex conversation. It reports Waiting until work begins. A stopped task with no rollout starts with the full request.
- AC-3. A restart queued for a session slot retains the continuation and submits it once.

## Test plan

Exercise the application lifecycle, real PTY host, fake Codex and synthetic rollout. Check activity, session identity and the received prompt. Run focused lifecycle and harness tests, repository quick checks and Yulius CI. The screenshot supplies the incident report and the test fixture supplies reproducible evidence.

## Decisions

Use the ended task's state to distinguish the requested restart of stopped work from reopening a closed conversation. Carry the next turn in the existing resume prompt, which already survives admission delays. Use the launch mode to derive initial activity for a bare Codex resume. The task page and Superagent both consume the supervisor's state.

## Evidence

On dev at `86eb47202`, the stopped-task regression found an empty queued resume. Reopening a closed conversation returned `working`. The task stopped before its first launch received its full request. [Issue #1601](https://github.com/Quartio-Studio/yulius/issues/1601)

The corrected supervisor passed all 132 Codex and parking integration tests in nextest run `dd248435-1a69-42a0-8672-7dab0545ab96`. The new cases verified one continuation in the original rollout after a queued restart, the original request after a pre-launch stop, recovery from a failed launch, and Waiting → Working → Waiting across input and interruption. The PTYs and application are real. The Codex process and account are fixtures.
