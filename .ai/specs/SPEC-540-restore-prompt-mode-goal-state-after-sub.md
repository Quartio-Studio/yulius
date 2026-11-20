---
yulius-task: 471b3694-e07c-467b-b0bc-29a0822f3fbb
status: done
---

# SPEC-540 — Restore prompt-mode goal state after subscription switches

## Goal

Keep a prompt-mode task working while Claude Code evaluates its person's goal after a subscription switch.

## Context

Issue #1182 records a real session and inspections of Claude Code 2.1.292, 2.1.311 and 2.1.322. On resume Claude Code restores the condition from the last open goal status in a trusted workspace where policy permits hooks. Yulius resets its goal flags in start_watcher and watches only records written since the attempt started. SPEC-468 carried unfinished work across idle moves, but a failed goal can be unfinished and closed. The fake agent previously installed its evaluator only for an explicit goal command.

## Requirements

- PRG-001. For manual, turn-boundary and usage-limit resumes, interpret the last goal status of the prompt task's Claude conversation. Set and not-met are open. Met, failed and cleared are closed.
- PRG-002. At a clean Stop with an open goal, apply the existing verdict wait. The wait survives PTY attachment. A new verdict, turn record or the existing 20-second deadline resolves it. Exit or loss of the run cancels its pending wait. When trust or policy prevents Claude's restore, Yulius types nothing and follows the same bounded fallback.
- PRG-003. The fake Claude restores the last open condition on resume and supports forgetting it to simulate a restricted workspace. Goal-mode launches keep their existing once-per-resume submission.

## Plan

- [x] Specify goal recovery from the resumed conversation
- [x] Reproduce goal loss across subscription moves
- [x] Restore prompt goals in the watcher and fake agent
- [x] Verify the fix and obtain independent review
- [x] Preserve dropped goal submission retries in the resumed fake terminal
- [x] Verify restart recovery with and without a restored goal
- [x] Preserve Enter submission of a restored terminal draft

## Acceptance criteria

- AC-1. A manual move, pool rotation and resume past a limit each keep the person's goal open before a new verdict. The next clean Stop leaves the task working through evaluation.
- AC-2. A resumed fake process evaluates the last open condition without a second goal command. A final met, failed or cleared record and the forget option suppress that evaluation.
- AC-3. An existing goal-mode switch test observes one goal submission in the new run.

## Test plan

Exercise the supervisor with a detached PTY host and the fake CLI, including Stop hooks. Run the fake CLI against synthetic transcripts and inspect the records it appends. Run the existing goal-mode resume tests. A signed-in live Claude session is outside the automated fixtures.

## Decisions

[ADR-0386](../../docs/decisions/0386-prompt-goals-follow-the-resumed-claude-conversation.md) defines the restore assumption and the fallback.

## Evidence

- Before the fix, all three real-process switch tests failed at the assertion that the resumed goal was open. A separate two-process fake CLI probe appended two answer records and zero verdicts on the second process.
- After initialization from the transcript, the manual, pool and usage-limit tests passed, including a server restart after the manual move. The restricted-hook fixture stayed Working after Stop and settled at the verdict deadline. The existing goal-mode switch test observed exactly one goal submission.
- Holding the resumed PTY attach exposed an additional early Stop failure. The task read Waiting before its evaluator could report. The verdict slot now lasts through attach and is cancelled when the run exits or is lost.
- The fake CLI contract exercised open and closed conditions, sidechain records and the opt-out. The adapter test exercised all five outcomes, a missing transcript, malformed input, the 4 MiB record limit and a partial trailing record.
- The automated tests use detached PTY hosts and the fake CLI. The evidence for native Claude Code restoration is from issue #1182. This task did not run a signed-in Claude session.
- Independent review requested a check that the same early Stop settles after attachment. The gated test now observes Working before releasing attach and Waiting without further input or verdict. Restoring the single slot clear in an isolated worktree made the test fail with “timed out waiting for the same Stop's deadline after attach”. It passed in the normal test binary. The reviewer approved the revised implementation and test at `c980e4094`.
- `scripts/dev quick` passed, as did `cargo clippy --locked -p yulius-harness -p yulius-app -p fake-agent --all-targets -- -D warnings` and the same Clippy check on `-p yulius`. `scripts/dev security` reported Vidoc disabled for this project. `scripts/dev mutants --base db567d76d` found no mutable lines in the review's test-only correction.
- The process-isolated nextest selection covering the switch tests and pre-attach Stop passed all 57 tests. A second nextest selection passed ten existing task completion, goal evaluation and restart tests. `cargo test -p fake-agent --test cli` passed all 30 CLI tests. `cargo test -p yulius --lib resumed_goal_uses_the_last_complete_main_thread_status` passed with records at exactly 4 MiB and one byte over. The first boundary fixture hit Rust's formatting width limit before calling the parser. The corrected fixture writes its bytes directly.
- `scripts/dev mutants` listed 54 mutants. Its unmodified workspace baseline reached the 300-second test deadline with 2,780 tests passed, three in progress terminated and 402 outstanding. The command reported “cargo test failed in an unmutated tree, so no mutants were tested”. All 54 remain untested by that campaign. The separate one-line reversal above produced the expected failure. `scripts/dev mutants --base c980e4094` found no mutant in the final boundary-fixture correction.
- Yulius CI run `17ab4dd9` failed in `task_done::a_goal_the_tui_dropped_is_submitted_again`. The exact test also timed out locally on the reviewed head. A pipe regression showed the cause: after a dropped submission, the fake consumed a bare Enter as an empty turn and marked the previously restored goal met. Direct and queued prompt input now skip empty submissions. All 31 fake CLI tests passed, along with six selected process tests covering the CI failure, typing exclusion, the three prompt-mode moves and goal-mode resubmission.
- Yulius CI run `09bd0979` failed in `first_message::a_goal_still_to_be_typed_at_a_restart_is_flagged`. The original test reproduced locally. Its synthetic transcript contained an open sentinel followed by a met verdict after the move, and the restarted server removed the pending marker. The missing-goal fixture now uses `forget_goal`. A companion test runs the default restore and verifies that no missing-goal notice is raised. Both verify marker removal and that the stopped typist sent no new goal. All 24 `first_message` process tests passed under nextest, as did Clippy on the integration test binary.
- Yulius CI run `01721acb` and a local nextest run both timed out in the two restored-draft cases of `task_message`. A fake CLI regression reproduced an empty transcript when Enter should have submitted the restored draft. The fake now tests the assembled prompt for emptiness after reading its draft. Bare Enter in an empty box still creates no turn. All 32 fake CLI tests and 43 selected process tests passed, including both draft cases, the dropped goal retry and all three goal moves. Fake-agent Clippy passed with warnings denied.
