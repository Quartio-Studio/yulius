---
yulius-task: bc5e9db9-59da-4313-bc95-0a804b81deab
status: in-progress
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

- **AGI-004** The post-merge gate verifies that a composer task drawn to Antigravity with both settings omitted inherits Gemini 4 Neon and High.

## Plan

- [x] Trace the reported screen through turn detection
- [x] Recognize the idle prompt and protect active turns
- [x] Verify the parking path and startup behavior
- [x] Approve the fix for merge and staging
- [x] Reproduce the merged composer contract failure
- [x] Verify the shared composer correction alongside idle resume
- [x] Approve the corrected staging revision
- [x] Reproduce the merged review-round failure
- [x] Verify delivery behavior for Claude Code and Codex
- [ ] Approve the delivery correction for staging

## Acceptance criteria

- **AC-1 (AGI-001)** Reopen a conversation without a message and display the reported screen. Its activity is Waiting. Parking releases its live attempt, and a person can close the conversation after its next turn.
- **AC-2 (AGI-002)** An actual prompt starts a turn. The announcement cannot end it. A Stop settles the turn normally.
- **AC-3 (AGI-003)** Check startup behavior on the pinned CLI. Exercise any banner preference through the adapter's generated files.

- **AC-4 (AGI-004)** Choose Random and draw Antigravity. Observe the created task’s effective model, effort and both inherited-setting indicators. Run the corrected repository contract and the idle-resume regression on the delivery revision.

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

### Post-merge staging gate

PR #1646 merged into dev at `13a632c40` after train run `21c0a1a2-1cb3-4932-b043-9aa973413f6c` passed. Staging request `b18c6879-1b4f-467c-bdb0-67df55a401f8` recorded a failed full gate. In run `fc6b9b38-9a54-4bde-9a2b-5eb1c7f0d883`, the second test partition failed at `ui_contract::the_composer_offers_antigravity_on_its_default_model`. Its assertion required the previous spelling of the mock's High-effort default. Running the exact test locally against the same mock reproduced the assertion. PR #1645 introduced the shared High default. This branch uses task #591’s reviewed correction to the Rust contract from `7b6419ed9`. Its browser assertion reads the resulting task through the Random composer flow.

All 11 Antigravity browser tests passed. A manual mutation marked the inherited effort as explicit. The new assertion failed on the inherited flag while the effective effort still read High. Restoring the mock made the same browser test pass.

All twelve Rust UI contracts and the PTY idle-resume regression passed together on this branch with the shared correction. PR #1651 landed that same Rust change at `1f9314339`.

Independent review approved the fix-forward at `2eb1ab6d9` with no findings. The reviewer independently ran all 11 Antigravity Chromium tests, the 12 Rust UI contracts and the idle-resume regression. The task delivery record identifies the follow-up merge and renewed staging request.

### Review rounds after a merge

PR #1654 landed at `3b95efbd8` after its merge train passed. Staging request `df03c04f-6c06-4c00-b50f-be68422d5bea` failed in full-gate run `ffd7ab0f-7208-498c-8453-95656973dcb3`. In test partition 2, `delivery::agents_merge_only_through_gated_tools` expected the Claude Code task's rounds for the next PR to be empty. It received the approved round assigned to the merged PR. The harness-specific contracts are recorded in [SPEC-605](SPEC-605-record-the-review-rounds-of-codex-tasks.md).

The exact nextest case failed locally on the landed tree with one completed Claude Code review where an empty list was expected. The branch incorporates task #589's correction from `ddf2a8809`. The delivery view passes the task's harness to the round lookup. Codex retains its landed review history, and Claude Code again reads the rounds available for its next pull request.

Scoped nextest ran 154 tests across `delivery`, `review_rounds`, Codex review hooks, the Antigravity idle PTY fixture and the composer contract. All passed, including the exact gated-tools case from the failed staging run and the Codex merged-analytics case.

After integrating dev at `0cf7267a3`, the four key regressions passed again. They cover the gated merge, Codex review history, the idle Antigravity PTY and its inherited composer settings.
