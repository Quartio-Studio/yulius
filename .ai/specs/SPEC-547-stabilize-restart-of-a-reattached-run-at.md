---
yulius-task: 1c2c18ef-b209-4c4e-924b-19790ff0e5d6
status: in-progress
---

# SPEC-547 Stabilize restart of a reattached run at its limit

## Goal

Resume the interrupted turn on a subscription with room after a server restart discovers that a paused run still holds the previous owner's MCP servers.

## Context

Issue #1286 reports a timeout waiting for attempt two on plain dev under load. SPEC-483 AC-122 covers this recovery. Transcript replay schedules a subscription switch while reconciliation schedules a hand-over restart for the same attempt. Both read WaitingCapacity. When the pool wins the status change, the hand-over treats its lost update as a restart failure and stops the winning switch. In the reverse order, delayed pool work can act on the replacement using the old account's limit.

## Requirements

- RST-547-1. The old attempt ends and exactly one replacement runs with the current owner's servers, continuing the interrupted turn on an available subscription.
- RST-547-2. A controlled ordering reproduces the failure. The integration test waits on observable conditions with bounded deadlines.
- RST-547-3. A capacity pass that finds the old pause during the hand-over defers that task and continues through the pool. A later pass checks that the same pause is still open.
- RST-547-4. When a replayed refusal belongs to a revoked account, the restart uses an available subscription. When it belongs to a model selected inside the CLI, the replacement reports its return to the configured model.
- RST-547-5. When no subscription can take a required hand-over, the old host stops. A model refusal without a known reset stays in the pool’s retry history when the hand-over succeeds.

## Plan

- [x] Isolate the failing reattach and restart ordering
- [x] Prove the ordering with a failing regression
- [x] Restore reliable restart and normal CI coverage
- [x] Verify the restart under 48 busy loops
- [x] Protect the persisted pause during startup hand-over
- [x] Preserve replayed subscription and model refusals through hand-over
- [x] Preserve failed hand-over and bounded retries when capacity runs out
- [x] Verify probe progress and immediate resumption of ordinary pauses
- [x] Ready the independently reviewed fix for the green CI merge
- [x] Resume the restored workflow when its first capacity pass is deferred
- [x] Verify reattached refusals against the current sign-in after merging dev
- [ ] Verify subscription reset labels across midnight in the browser fixture

## Acceptance criteria

- AC-1. Reattaching the limited run while another subscription has room resumes it once, with its limit record removed and the current owner's servers.
- AC-2. The integration test passes 40 iterations beside 48 busy loops.
- AC-3. A capacity pass between hand-over preflight and the switch’s status update leaves the task waiting for that hand-over, schedules a prompt recheck, and yields one live replacement.

## Test plan

Exercise two server runtimes, a detached fake agent, and real subscription and hand-over paths. Delay one path at its decision until the competing path reaches the relevant state. Check the attempts, account, owner and resume arguments. Run the adjacent reattach tests and the requested loaded nextest command.

## Decisions

Reconciliation takes the task’s move lock before attaching the host. It releases the lock once ownership is checked, or transfers it to the required hand-over until that restart returns. Dropping either future releases the lock. Transcript replay can record the limit and complete while the deferred move waits. Every limit move waits for a pending hand-over and checks that its reporting attempt is still attached. Only a replayed refusal holds the lock through the pool decision. The watcher identifies a later live refusal on the same attempt so its published pause can resume immediately. The ownership restart uses the pool’s eligible alternative when the previous subscription is refused. With no eligible account it preserves the existing preflight refusal and stop. A model refusal without a known reset is recorded against the reporting account before the move so it counts toward the existing retry bound. Recording the refused model at observation time lets its immediate replacement report a model change. The capacity loop tries the same lock without blocking other tasks and rechecks the identity of each pause before acting. [ADR-0389](../../docs/decisions/0389-a-reattach-finishes-its-hand-over-before-a-limit-moves-the-run.md) records the decision. The quarantine is managed by Yulius and released when tracking issue #1431 closes (ADR-0356). The separate regression runs in CI while that quarantine is active.

## Evidence

- On dev 8d05c25b with diagnostic logging, nextest failed iteration 2 of 12 beside 48 busy loops. The hand-over logged a concurrent state change, the pool logged a stop during its switch, and the test timed out after 65.4 seconds. SQLite showed one killed attempt and a cancelled switch.
- A temporary task-keyed probe held the hand-over after preflight until the pool move finished. The task ended Stopped(Cancelled) in 6.3 seconds.
- The retained regression held the replayed limit move until the hand-over completed. Releasing it created attempt three. The two-attempt assertion failed in 7.9 seconds.
- Seven targeted nextest cases passed, including the original fixture and the delayed limit move, replay with unchanged ownership and hand-over without a recorded limit.
- Eleven adjacent workflow-limit, refused-subscription and failed hand-over cases passed. These cover the pool’s ordinary limit moves, workflow resumption after restart, and stopping an old owner’s run when the required restart genuinely fails.
- The original test passed 40/40 iterations in 331.5 seconds beside 48 busy loops. All workers were alive at both ends of the run.
- Independent review of 1113a4157 found that the startup capacity pass could also claim the persisted pause. Holding the hand-over after preflight and calling resume_round reproduced it in 7.1 seconds. The task was already Running when the hand-over still owned a WaitingCapacity pause.
- After the capacity fix, all 57 pool supervision, subscription pool and workflow limit integration tests passed, including the controlled preflight capacity pass.
- Review of 3d948255e found two replay cases. Before their fixes, the revoked-account case stopped instead of running after 5.0 s. The switched-model case resumed but omitted the Fable-to-Opus terminal notice after 6.8 s.
- With both replay fixes, 91/91 pool supervision, subscription pool, workflow limit and usage credit integration tests passed. The merged dev base also passed the three startup regressions.
- First review mutation campaign: 11 listed, 1 caught, 0 missed, 0 timed out, 10 unviable, 0 not reached (bound 30 min, took 1595 s).
- Before the final compatibility fixes, an exhausted pool left the hand-over in WaitingCapacity instead of stopping it (3.8 s). Once each move finished, refreshing the first account after the second account spent its retry launched a fourth attempt instead of leaving the third waiting (6.2 s).
- The delayed-move fixture timed out before any hand-over decision in a mutation baseline and an independent run. A one-worker Tokio executor reproduced the blocked synchronous probe after 63.5 seconds. The retained probe yields its worker while waiting.
- The no-candidate hand-over and spent blind retry passed on 0763285e8. Host termination is verified independently of the earlier stopped task status. A separate existing test sampled Running immediately after its attempt became Live; the same two database writes and assertion are on dev, and were reported on #1286.
- The capacity review mutation campaign listed six mutants, with five unviable and one missed. Its filter included no ordinary successful capacity pass. The final campaign includes that pass to check the missed negation of the open-pause check.
- With the probe yielding its worker, the delayed move, preflight capacity pass and spent blind retry all passed on one worker. Holding the end of an ordinary published pause reproduced the overly broad lock: one capacity pass left the task WaitingCapacity instead of Running (1.8 s).
- A later live refusal after an unchanged-owner reattach also reproduced WaitingCapacity instead of Running in one capacity pass (4.5 s). The watcher distinguishes the refusal replayed at startup from subsequent live observations.
- Changing the hand-over condition from conjunction to disjunction in the mutation run let an ordinary hand-over use a different account. The added non-limit hand-over regression failed against the generated mutation in 2.8 s. It checks Stopped(Cancelled) and one attempt. The exhausted-pool regression separately waits for the old host’s recorded exit.
- On approved product head aa227146a, all 95 integration tests in pool supervision, subscription pool, workflow limit and usage credits passed. Repository contracts, Rust formatting and spec validation passed. The project has Vidoc disabled.
- The reviewer independently ran the later-live capacity pass, ordinary revoked-account hand-over and spent blind retry on aa227146a. All three passed in 6.63 s. The broader mutation run caught the previously missed open-pause negation and the launch-time model-notice condition.
- The requested stress run repeated on aa227146a with 40/40 passes in 196.356 s. All 48 busy loops were alive both before and after nextest. The loaded workers were then reaped.
- The compatibility mutation run tested all 11 listed changes in 784 s. Two were caught and eight were unviable. The full reviewed change tested all 30 in 1249 s, with 12 caught and 17 unviable. Both runs missed only the hand-over conjunction. The new non-limit regression subsequently caught that exact mutation. Neither campaign timed out.
- The later source-propagation filter omitted the existing review-round suite. Its subscription-wide reviewer refusal passed on the final code in 3.1 s and failed in 61.5 s with the exact generated mutation which removes the subagent limit handler. It timed out waiting for the replacement.
- Final source-propagation campaign on aa227146a: 12 listed, 4 caught, 1 missed, 0 timed out, 7 unviable, 0 not reached (bound 30 min, took 959 s). The dedicated review-round test above caught its only survivor.
- PR #1481 CI run cb4cdbc8 timed out after 63.3 s waiting for a restored workflow. Holding the replayed limit pause across the fixture’s only capacity pass reproduced the timeout in 62.6 s. That pass returned the one-second recheck which the fixture discarded. The test now verifies the deferral and starts the server’s capacity loop to drive subsequent passes. The forced ordering passed 12/12 iterations in 38.8 s, and all 95 related integration tests passed on the updated dev base.
- After the CI fixture fix and clean merge of dev at 514b07925, the original test again passed 40/40 in 219.170 s with all 48 busy loops still running.
- Merging dev at e03e050c7 combined attempt-scoped access refusals with the run directory's credential check from #1365. All 110 related tests passed, including replay on a revoked subscription, a refusal after a new sign-in, and a run from before credential tracking.
