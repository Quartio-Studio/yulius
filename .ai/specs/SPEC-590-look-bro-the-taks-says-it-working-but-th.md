---
yulius-task: 2b0ce1c0-efb5-419f-94ab-2b216f995635
status: done
---

# SPEC-590. Continue a stopped Codex task in its saved conversation

## Goal

When a person restarts a stopped Codex task, its work resumes without a second message through the Superagent. A Codex conversation reopened for input shows its actual activity.

Fixes #1601.

## Context

In the screenshot, task #34 has an unfinished plan and an interrupted conversation. The Superagent reports that restarting left the prompt empty. The capture also shows a subsequent message already in a new turn. The other sandbox's earlier live state is unavailable here.

The page's Continue action and the Superagent's `task_restart` share `continue_task`. Its ended-task path always requests a resume without a prompt. Codex reopens the saved rollout, and `launch_attempt` defaults its activity to Working. An empty session sends no turn-end hook to settle that state. [SPEC-503](SPEC-503-accept-outside-pr-1146-codex-returns-as.md) tested native resume after a usage limit. [SPEC-558](SPEC-558-parked-codex-tasks-no-codex-exit-screen.md) tested a message waking a parked task. Neither exercises Continue after Stop.

## Requirements

- CCR-001. Continue on a stopped Codex task with a saved conversation submits one continuation in that conversation. A retry after a failed start follows the same rule. A task without a saved conversation starts with its original request. A recorded attempt ID recovers a missing history index entry. Conflicting or stale history must be resolved before resuming.
- CCR-002. A native Codex resume that opens without a turn shows idle activity. A new prompt starts work, and a recorded interruption returns the live task to Waiting. Opening a closed conversation allows the person to supply the next prompt.
- CCR-003. The session queue and capacity pause retain the selected fresh or resumed launch and its prompt until the requested start can run. A person stopping an agent authorizes no automatic restart.

## Plan

- [x] Trace and reproduce the stopped Codex task
- [x] Prove the interruption and resume behavior with regression tests
- [x] Keep task activity and resumed work in sync with Codex
- [x] Recover missing history and cancel an overtaken capacity worker
- [x] Preserve the replacement continuation when Stop overtakes an account move
- [x] Verify and independently review the fix
- [x] Check the final account move correction with mutation testing
- [x] Record verification evidence for the merge queue
- [x] Verify the CI timeout and worker recovery corrections on the integrated head

## Acceptance criteria

- AC-1. Stop a task with a known Codex session, then Continue. The resumed agent processes a continuation in the same session with no further operator input.
- AC-2. Close and reopen a Codex conversation. It reports Waiting until work begins. A stopped task with no rollout starts with the full request.
- AC-3. A restart queued for a session slot retains the continuation and submits it once. Moving a restart that waits for subscription capacity carries the saved prompt to the chosen account.
- AC-4. Stop cancels a pending capacity restart, including after the worker selected its prompt. A later Continue owns its own pause.

## Test plan

Exercise the application lifecycle, real PTY host, fake Codex and synthetic rollout. Check activity, session identity and the received prompt. Run focused lifecycle and harness tests, repository quick checks and Yulius CI. The screenshot supplies the incident report and the test fixture supplies reproducible evidence.

## Decisions

Use the ended task's state to distinguish the requested restart of stopped work from reopening a closed conversation. Carry the next turn in the existing resume prompt, which already survives admission delays. Use the launch mode to derive initial activity for a bare Codex resume. The task page and Superagent both consume the supervisor's state.

## Evidence

On dev at `86eb47202`, the stopped-task regression found an empty queued resume. Reopening a closed conversation returned `working`. The task stopped before its first launch received its full request. [Issue #1601](https://github.com/Quartio-Studio/yulius/issues/1601)

The corrected supervisor passed all 132 Codex and parking integration tests in nextest run `dd248435-1a69-42a0-8672-7dab0545ab96`. The new cases verified one continuation in the original rollout after a queued restart, the original request after a pre-launch stop, recovery from a failed launch, and Waiting → Working → Waiting across input and interruption. The PTYs and application are real. The Codex process and account are fixtures.

Independent review of `19c85e1e7` found two recovery paths requiring changes. Against that head, three regression cases failed. A missing index entry lost the continuation, stale history reopened the wrong session and moving a capacity-paused restart dropped the prompt. The correction shares the parked task’s session identity check and forwards the saved start-pause continuation on a move.

After both corrections, the 159 Codex, parking, subscription move and native harness checks passed in nextest run `090c7c24-3bbb-493c-8320-65ced95d22e2`. They include recovery from the missing index write and refusal of an unrelated rollout. Repository quick checks passed. Vidoc was disabled for this project.

The second independent review identified the interval between SessionStart recording an ID and the first rollout write. On `85a755963`, nextest run `64df5e76-7867-4bdf-8a46-0b2ba2d77b15` reproduced Continue queueing a native resume for an ID with no file. Continue checks the rollout lookup and admits a fresh start when Codex has no saved conversation. A lookup error is returned to the caller.

The queue and both forms of capacity recovery preserved the fresh launch in run `1a1d81f6-e9b1-42cd-84b8-25cfddc9a5a1`, even when the first rollout appeared late. This wider run passed 198 of 200 checks. Two existing Claude parking fixtures intermittently failed before the Codex restart logic, tracked as [#1605](https://github.com/Quartio-Studio/yulius/issues/1605) and [#1606](https://github.com/Quartio-Studio/yulius/issues/1606). The affected application crate passed Clippy with warnings denied.

The third review found the capacity worker could select a continuation, then launch it after Stop. Two deterministic tests held the worker immediately before launch. In nextest run `48d0706a-1b6a-471c-a795-a20b3212ff25`, both observed a second Codex attempt after cancellation. Admission now checks both the observed waiting state and the original pause ID under the launch lock.

The fourth review identified a fresh host spawn failure after attempt insertion and an old capacity worker cleaning up a replacement pause. In run `133056e4-c38f-44f1-b7b1-7896c1cd3b43`, the previous two cancellation regressions passed. The new regressions returned a conflict on retry and lost the replacement pause. A fresh attempt that failed to spawn retries its original request. Stale cleanup closes only the observed pause and subscription request.

All 93 selected Codex, account move, pool supervision, parking, persistence and native harness checks passed in run `e9bd4abf-2d88-4aca-a9ad-eaba8edf9a07`. The fifth review found another interleaving. Stop can win after a worker checks the pause and before it claims the status. In run `aa9dfb6d-8a27-403d-bbde-baa4196aa681`, that worker erased the replacement pause. A worker that loses the claim now reports that it started nothing. A genuine launch failure settles its own state while the launch lock still protects it.

The next review confirmed the worker correction and found a manual Move could take a later Continue's pause. On the reviewed product, run `a69556d6-1bdd-4b9a-8df9-8cf557b14f64` passed 98 of 99 tests. The failing Move erased the new native resume. Run `f5314de7-fbd1-4580-9ef0-74b58c0de42d` also found that a Move started a session while the only slot was already occupied. Move now submits the saved start through the same pause and slot checks as capacity recovery, retains the manually chosen account when queued and records the switch outcome.

The bounded full-diff mutation run listed 110, caught 25, missed 18, timed out on 0, found 60 unviable and did not reach 7 in 30 minutes. A wider preliminary filter had exceeded the unmutated baseline time limit. The cancellation correction's scoped run listed 61, caught 2, missed 0, timed out on 0, found 59 unviable and reached all 61. Four mutations in pause handling were caught in a focused rerun with the machinery-start and persisted-pause cases.

Focused cargo-mutants reruns caught all 18 initially missed variants. The pause run caught 4. Account moves caught 7 including a previously caught control. The launch and history runs caught 7 and 1. These cases distinguish an idle move, an interrupted working turn, a held turn, a reviewer continuation, a different provider's history and a legacy Codex conversation without a local attempt. The spawn and stale-cleanup correction's scoped run listed 36, caught 6, missed 0, timed out on 0, found 30 unviable and reached all 36.

The launch claim correction's scoped run listed 29, caught 0, missed 0, timed out on 0, found 29 unviable and reached all 29. All 29 replacements failed Rust type checking. The deterministic lost-claim test supplies the behavioral evidence for that branch.

The final 100 Codex, pool, account Move, parked wake, persistence and native harness checks passed in run `c88d84e6-f843-40f4-8227-01a0c3402c4c`. Independent review approved `fc640ee92fc07de17cd97e421f9cc37bf50bcf4f` and reran the overtaken Move and session cap regressions against the approved code.

Four account-move controls passed in run `e84d9581-55d4-4776-9867-62cdcda21dbc`, including a replacement that is already running and a held operator prompt. The application and store libraries and the product integration test target passed Clippy with warnings denied.

On the final account Move correction, the last counts available before the session restarted were 69 listed, 7 caught, 1 missed, 0 timed out and 54 unviable. The final outcome of the other 7 was unavailable after restart. The missed variant dropped a held operator prompt. A focused repeat with the held-prompt and replacement-run controls caught all 7 prompt and switch-outcome variants, including that one.

After resuming the verification session, a focused run on the three remaining switch conditions passed its seven unmutated regression checks, caught two variants and found the third unviable. All observed missed variants have failing regression coverage.

The first pull request CI run `bb2385d2-2faa-45cb-b16a-3bddd82d17be` exposed three timing-sensitive integration tests. Two merge-train scenarios exhausted their fixture-wide 100-second bound. The branch incorporated the scheduling correction for these exact tests from [PR #1608](https://github.com/Quartio-Studio/yulius/pull/1608). The parked-delivery test expected a healthy status as soon as its blocked Git workers were released. Yulius CI also saw this failure on PR #1581 and quarantined it in [issue #1614](https://github.com/Quartio-Studio/yulius/issues/1614).

Run `0451c7fe-5fcd-4114-a96d-90ce125f2f07` reproduced the parked-delivery assertion by holding Git workers between release and completion. The corrected test checks the pessimistic status during that interval, waits for the worker pool to regain its permits, and reads the healthy status once.

On the integrated branch, CI-profile nextest run `49930f14-8fbf-4574-9735-c330ad743a98` passed all six selected tests. These included all three CI failures, a parked conflict-read control and the two primary Codex restart and activity regressions. The quarantined worker-recovery test ran explicitly in this verification.
