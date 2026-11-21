---
yulius-task: 4014aca0-70ee-4ff0-aae0-9347694f1f02
status: in-progress
---

# SPEC-597 Tests under load and the wait after a merge-first merge

## Goal

Let CI judge the behavior under test even when other jobs slow its host. Carry an agent through the full gate and staging decision after its last merge without a person restarting it.

## Context

On November 21 the operator reported eight browser timeouts on merge train #1590, two real-Git fixture deadlines on PR #1608, and an e2e failure on #1607. SPEC-594 isolated the two long scenarios within a CI shard. SPEC-593 already removed a five-second ambient toast race in #1607. Quartio-v2 reported four failed post-merge suites out of nine and three merged tasks idling with an unfinished step. The owner requested a report in this spec and the closing summary for their operator.

SPEC-575 already has a durable staging queue, explicit admission through `cd_deploy`, and outcome delivery to a running or parked agent. The staging admission rule allows a final step in progress. Generic merge receipts and task classification currently overlook that wait.

## Requirements

- **LHW-001** Wait for specific browser and merge-train conditions. Allow measured host contention within an explicit bound. Preserve assertion failures, diagnostic state and zero test retries.
- **LHW-002** Identify the affected quartio-v2 test and record the tests, evidence and recommended changes here for the quartio-v2 operator.
- **LHW-003** The merge receipt guides the agent to submit the staging request when the whole delivery has landed, including with its last step in progress. Intermediate merges continue the plan.
- **LHW-004** An outstanding staging request identifies the exact revision and whether it waits for the full gate or deployment. A stopped turn waiting for that result reads as working. The existing durable outcome wakes the task on success or failure. Explicit human holds, pauses and ended tasks keep their precedence.

## Plan

- [x] Trace the failed waits and report the quartio-v2 evidence
- [x] Make the affected browser and merge-train tests wait for observable progress
- [x] Keep the merged task waiting for its gate and staging decision
- [x] Approve the browser and merge-train test fixes for delivery
- [x] Direct a superseding final staging request to the newer landing
- [x] Deliver staging receipts that identify a completed check or its fix-forward action
- [x] Preserve a retried deployment’s outcome when delivery overlaps a previous failure
- [ ] Prove the staging failure and recovery paths and approve their delivery after the CI test merge
- [ ] Package the reviewed staging change with its verification evidence

## Acceptance criteria

- **AC-1** A delayed healthy browser and a progressing multi-head fixture reach the original assertions. A missing condition still reports failure within its bound.
- **AC-2** The quartio-v2 report identifies the observed failure and a way to distinguish a late event from a wrong result.
- **AC-3** On a final merge the agent receives the next action. Its open step and pending request show a machine wait. An answered request goes back to the agent with the outcome. A genuine request for help still surfaces.

## Test plan

Trace the retained run output to each relevant fixture. Use controlled delays or clocks and unmodified failure assertions. Exercise merge receipts, real staging request states and task status projections, including failure paths and parked sessions. Run scoped nextest and browser suites, repository checks, independent review and the delivery gate.

## Decisions

Deliver the load-sensitive test fixes in the first PR and the staging behavior in subsequent PRs. Keep the request mechanism and staging outcome notifier from SPEC-575. A task can have further slices after a merge, so the agent still decides when the whole delivery has landed. Use durable request state to explain an actual machine wait. For this delivery the owner asks for one staging request after the final merge. The request receipt belongs in the closing summary.

## Quartio-v2 operator handoff

Four of nine post-merge suites failed. Task #341 traced the dialog-backdrop selection failure at `a2e9d96e`: the dialog stayed open, but Chromium lost the selected sentence. At 12× CPU throttling the synthetic drag started over earlier text. Wait for fonts and opening animations before computing the drag coordinates, then assert the exact selection and that the dialog stays open after release. Quartio-v2 already made that change and recorded three throttled passes. [PR #1516](https://github.com/Quartio-Studio/quartio-v2/pull/1516)

A subsequent gate at `b29cef99` recorded independent 30-second Docker/MCP initialization failures at load average above 300 and a mobile viewport assertion racing animation-frame handlers. Keep Docker admission bounded and measure startup to readiness separately from a failed initialization. For the mobile case, wait for the history panel to become inert before checking layout. The project tracks both. These records do not establish the cause of all four failed suites. [Docker #1519](https://github.com/Quartio-Studio/quartio-v2/issues/1519), [mobile layout #1433](https://github.com/Quartio-Studio/quartio-v2/issues/1433)

## Evidence

- PR #1608 run `7a94e552-efe3-49a9-875c-07b467d03a2e` exhausted two fixture deadlines in a shard that took 11m39s against roughly 4m unloaded. Both scenarios already poll merge, candidate and job state. SPEC-594 gave them exclusive slots within their own shard. SPEC-597 gives only those scenarios 300s across all phases and a 330s outer watchdog. Other shards and concurrent builds can still slow the host. A 105s elapsed-budget injection on the old fixture failed both without sleeping or changing their expected results.
- The seven browser files named in the #1590 incident read initial page data or route state, often with the default 5s assertion timeout. They now use the existing 30s first-load condition. Responsive layout still checks geometry. Sessions and notes retain controlled timer/animation tests. A 6.1s delay on the team-secrets response failed the original first-row assertion. A 9s delay observing Docker smoke checks missed the intermediate result as the mock advanced on the host clock. Both original assertions passed with those same delays after the fixes (`npx e2e run`, two cases, zero retries). The smoke mock now waits for the test to advance it after observing the first check. The #1590 retry `fdbc29eb-4c5b-43aa-9356-65acd770d387` passed unchanged. The original train run was unavailable for per-assertion attribution. #1607’s independent toast race was already reproduced and fixed in SPEC-593.
- An idle task with its last staging step in progress and an admitted request reproduced the production notice verbatim on the previous implementation. The browser test for both running and parked sessions also failed before the change and passed afterward. The Rust fixture drives the actual request and gate state. The process-host test checks delivery to the agent across a genuine human decision.
- Full local Chromium suite alongside the Rust build: 1,315 passed, one model-dependent test skipped, 3m46s, zero retries.
- With the same 105s elapsed-budget injection the bisection and four-head scenarios passed, including their original run-count and arm-order assertions. The committed tests run with the full budget. The fixture reports the run ID, revision and state on a candidate deadline.
- Scoped nextest with the probe: all 39 cases passed, including the real-process notification, queue recovery and green train receipt. Repository contracts, formatting, `git diff --check`, and `spec_check` passed.

### Independent review, round one

The reviewer requested changes at `1e9c763e3`. Four cases reproduced the findings.

- The local completion check counted an idle agent with a pending outcome as using zero subscriptions. The Capacity regression expects one. Both local completion and chain completion now consult the same durable delivery wait.
- A mounted task kept the old wait when an SSE summary omitted it, and a parked detail failed to update the shared list. Both browser regressions failed before the fix. The full Chromium suite after the fix passed 1,317 tests with one model-dependent test skipped.
- A person merging an armed green PR returned a generic receipt to the waiting agent. The HTTP regression failed to find the landed revision and the named staging call. The person and agent paths now provide the same staging action after a confirmed green merge.
- When slice A had a confirmed landing and slice B still awaited confirmation, the final delivery status directed the agent to deploy A. The two-slice regression reproduced that instruction. The staging receipt and admission now select the current PR’s confirmed integration landing. Missing evidence defers staging. A confirmed B can then enter the queue.

### Independent review, round two

The 42 scoped CI-profile Rust cases passed on `330207331`. The reviewer confirmed the session, UI and person-merge fixes and found a second stale-landing window. An external merge of B can leave the local delivery for B reading `pr_open` until the watcher confirms it. The new regression called `cd_deploy` during that window and got a live staging request for A. Admission now also defers while the current delivery is open. The same test covers the subsequent unconfirmed merged record, then requests the confirmed B. Vidoc was disabled by the project.

### Split delivery

The third review confirmed the previous landing, capacity and UI fixes. It identified a final receipt that could still point at an older testing or ready request. The tests-under-load slice delivers LHW-001 and the quartio-v2 operator handoff. The staging work and its supersession regression deliver LHW-003 and LHW-004 in the follow-up.

On the previous head, B’s receipt reported the pending request for A and the supersession regression failed. The receipt now uses the same admission decision as the staging queue. The same assertion passed, along with 38 other CI-profile Rust cases. A verified earlier revision still limits the task to one successful staging deployment.

The CI-only review approved `ead7b3699`. The reviewer found an existing Notes assertion that accepted the previous sandbox’s identical tabs. With the navigation omitted it passed on the old test, then failed on the amended observation. With navigation enabled the test observed the destination sandbox’s unique note and the expected tab order. The project tracks the reproduced browser timeouts in [#1629](https://github.com/Quartio-Studio/yulius/issues/1629) and the real-Git fixture timeouts in [#1610](https://github.com/Quartio-Studio/yulius/issues/1610).

PR #1630 landed in dev at `612f787b3` from green PR head `c36c14a5f` (run `e03bb582-bca5-4e15-b12a-b7f17d34f182`, train `2261b8f7-cc71-491d-8d56-6c80e10c0b3d`). On the isolated CI slice, Chromium passed 1,314 tests with one model-dependent skip in 6m32s. The two real-Git scenarios passed under host contention in 71.5s and 91.6s. Vidoc was disabled. [PR #1630](https://github.com/Quartio-Studio/yulius/pull/1630)

The shared goal Stop hook added in SPEC-589 returned “Continue the authorized goal” during an admitted, undelivered staging outcome. The staging slice checks that Stop yields until the outcome is delivered and then continues an unfinished step. Merge and staging receipts distinguish submitting the request from a plan that explicitly requires its verdict. The full Chromium run after restoration passed 1,317 tests with one model-dependent skip in 4m15s.

### Staging-only review

At `c4157acf0` the reviewer identified two outcome-delivery faults. Repeating `cd_deploy` for a previously delivered failed gate told the agent to end its turn and await another wake. The receipt now returns the gate run and fix-forward action. A repeated verified request reports completion. In the process fixture a human decision arrived while Chromium was busy. The original notifier sent the outcome anyway (one delivery, expected zero). It now bounds the browser lookup, reads the recipient and durable holds after the lookup and redaction, and leaves the outcome in the queue while a decision is open.

Manual reversion checks failed on the merge instruction, subscription retention, task wait projection, goal Stop hook, both browser update paths and delivery during a newly raised decision. The original terminal receipt also failed its fix-forward assertion. With the fixes restored, all 72 CI-profile Rust cases and 1,318 Chromium cases passed. One model-dependent browser case was skipped. Repository contracts, formatting and spec validation passed.

A second review found that the queue waited for a full task summary while Chromium was busy. The held-browser regression timed out after the failed gate was recorded. The deployment-event regression observed the ready and terminal labels but no deploying label. The two red regressions passed after restoration, as did all 73 scoped Rust cases. Each deploy transition now publishes its new state. A task update has a bounded wait and sends the existing sandbox-scoped resync event if viewers must refetch the summary.

A third staging review confirmed the earlier fixes and identified a receipt loop when CI disappeared before a run existed, and a retry whose previous failure could consume its later success notification. The terminal receipts are delivered independently. The remaining wait and wake work retains the retry race as a regression.

The merged load-test revision passed the full dev gate, including both Rust shards and e2e (run `953c3dd5-b1e6-4262-8185-f8156b86c7ff`). [PR #1630 verification](https://yulius.quartio.ai/api/tasks/4014aca0-70ee-4ff0-aae0-9347694f1f02/delivery/evidence/612f787b38fdfa3492cd5e0c4ecd87b23449d6fa)

With the terminal receipt changes removed, the repeated failed-gate and no-run gate regressions both failed. The second fixture drove the queue with CI unavailable and repeated the same request after restoring CI. Both passed with the receipt fixes restored, along with all 35 staging cases.

The multi-task fixture deployed revision B, then repeated A’s verified request. Before the fix its receipt said staging still served A. Successful receipts now report what the recorded deployment verified or included and name the available run IDs.

All 35 CI-profile staging scenarios passed with the historical receipt fix. Independent review approved `0b7400556`. The repository contract and format checks passed. Vidoc was disabled.

The receipt changes landed at `553e50b4b` from PR #1637 on green Yulius CI (PR run `18f60c3c-94aa-41fe-8eeb-f12e3bf1f59d`, train `7197a893-84f7-49a3-a73b-c454af0162b2`). [Verification evidence](https://yulius.quartio.ai/api/tasks/4014aca0-70ee-4ff0-aae0-9347694f1f02/delivery/evidence/553e50b4b62e56594f1bfd02cb79718cd10ab4fe)

With the retry guards removed, the real agent received the previous deployment failure after the retry verified (one stale wake, expected zero). On the same clock tick the SQLite store also accepted an old acknowledgment for a second failed attempt.

The full Chromium suite on the final staging slice passed 1,318 tests with zero retries. One model-dependent case was skipped. The run took 3m35s alongside the Rust build.

After restoration, both retry regressions passed. Scoped CI-profile nextest passed all 76 cases across staging, merge receipts, goal Stop, wake delivery and SQLite. Repository and format checks passed.
