---
yulius-task: bc5e9db9-59da-4313-bc95-0a804b81deab
status: done
---

# SPEC-604 An idle Antigravity conversation shows Waiting after a restart

## Goal

A finished Antigravity task reopened at its prompt can be parked or closed normally, including when Gemini's announcement is visible.

Fixes #1640.

## Context

The incident report describes two completed and merged tasks that occupied session slots for 7.9 hours after a host move. After a host move, `revive_after_move` queues lost runs and `drain_queue_now` resumes recorded conversations without a prompt. The Antigravity launch adapter passes only the conversation ID on a bare resume. The supervisor defaults that launch to Working unless supplied an idle rest. No user turn starts and thus no Stop hook is due. Task activity comes from launch intent, hooks and transcript observations. The text of the banner and of the previous tool summary does not supply a turn event. [Antigravity Stop contract](../../docs/decisions/0419-antigravity-stop-settles-turn-and-armed-green-prs-have-a-stall-bound.md)

## Requirements

- **AGI-001** A bare Antigravity resume at its prompt defaults to Waiting and honors a saved idle activity. Completed work can release its session under the existing close and parking rules.
- **AGI-002** A fresh prompt, resumed prompt or interrupted-turn continuation starts working. Subsequent native prompt and Stop hooks continue to open and settle turns.
- **AGI-003** Suppress the Gemini announcement at launch if the pinned CLI provides a verified way that cannot affect a turn or a person's input.

## Plan

- [x] Trace the reported screen through turn detection
- [x] Recognize the idle prompt and protect active turns
- [x] Verify the parking path and startup behavior
- [x] Approve the fix for merge and staging

## Acceptance criteria

- **AC-1 (AGI-001)** Reopen a conversation without a message and display the reported screen. Its activity is Waiting. Parking releases its live attempt, and a person can close the conversation after its next turn.
- **AC-2 (AGI-002)** An actual prompt starts a turn. The announcement cannot end it. A Stop settles the turn normally.
- **AC-3 (AGI-003)** Check startup behavior on the pinned CLI. Exercise any banner preference through the adapter's generated files.

## Test plan

Reproduce a bare resume through the supervisor with a real PTY and a synthetic fixture of the reported screen. Exercise later input and closure. Run the relevant Antigravity and Codex launch and lifecycle tests, including the background-work Stop contracts. Undo the fix and verify that the regression returns. Run repository quick checks, independent review and Yulius delivery.

## Decisions

Use the resolved launch mode to classify a resumed conversation. The pinned CLI documents Escape on an empty prompt and hides the announcement on the next message. Yulius cannot guarantee that the prompt is still empty when a startup key arrives. [ADR-0445](../../docs/decisions/0445-antigravity-resume-activity-follows-its-launch-prompt.md)

## Evidence

- Before the fix, the PTY displayed the complete fixture and the bare-resume test failed on the activity assertion with `Some(Working)` instead of `Some(Waiting)`.
- The probe used the repository-pinned Linux x64 Antigravity CLI 1.5.12 with a verified SHA-512, an isolated home and a synthetic API key. Its bundled changelog documents empty-prompt Escape dismissal and automatic hiding on message submission. The probe reached the idle Gemini prompt without receiving the account-specific announcement. The integration test uses the supplied screen text.
- Scoped nextest ran 33 Antigravity lifecycle, host-move, Codex reopening and parking tests. All passed. The fixture-backed test observed Waiting, automatic parking with no live attempt, Working on a prompted resume, a native Stop, and closure with no live attempt.
- All four selected harness launch tests passed for Antigravity and Codex resume arguments.
- Manual mutation removed Antigravity from the bare-resume arm. The regression again read `Some(Working)` at the Waiting assertion. Restored the implementation and reran the fixture test successfully.
- Independent review approved the implementation at `64cdbc6f4` with no findings. The reviewer ran 12 Antigravity lifecycle tests and three native hook tests. Repository contracts, formatting and `spec_check` passed. Vidoc was disabled. The task delivery record identifies the merged revision and its staging request.
