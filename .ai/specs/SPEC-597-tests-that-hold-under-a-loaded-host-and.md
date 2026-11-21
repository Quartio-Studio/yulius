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
- [ ] Approve the browser and merge-train test fixes for delivery
- [x] Direct a superseding final staging request to the newer landing
- [ ] Prove the staging failure and recovery paths and approve their delivery after the CI test merge
- [ ] Deliver the reviewed change and request staging

## Acceptance criteria

- **AC-1** A delayed healthy browser and a progressing multi-head fixture reach the original assertions. A missing condition still reports failure within its bound.
- **AC-2** The quartio-v2 report identifies the observed failure and a way to distinguish a late event from a wrong result.
- **AC-3** On a final merge the agent receives the next action. Its open step and pending request show a machine wait. An answered request goes back to the agent with the outcome. A genuine request for help still surfaces.

## Test plan

Trace the retained run output to each relevant fixture. Use controlled delays or clocks and unmodified failure assertions. Exercise merge receipts, real staging request states and task status projections, including failure paths and parked sessions. Run scoped nextest and browser suites, repository checks, independent review and the delivery gate.

## Decisions

Deliver the load-sensitive test fixes in the first PR and the staging behavior in the second PR. Keep the request mechanism and staging outcome notifier from SPEC-575. A task can have further slices after a merge, so the agent still decides when the whole delivery has landed. Use durable request state to explain an actual machine wait.

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
- The first mutation campaign stopped at its baseline. The review’s new capacity regression had already been added while the command waited for the build lock, and reproduced the outstanding subscription finding. No mutant verdict was obtained from that run.

### Independent review, round two

The 42 scoped CI-profile Rust cases passed on `330207331`. The reviewer confirmed the session, UI and person-merge fixes and found a second stale-landing window. An external merge of B can leave the local delivery for B reading `pr_open` until the watcher confirms it. The new regression called `cd_deploy` during that window and got a live staging request for A. Admission now also defers while the current delivery is open. The same test covers the subsequent unconfirmed merged record, then requests the confirmed B. Vidoc was disabled by the project.

### Split delivery

The third review confirmed the previous landing, capacity and UI fixes. It identified a final receipt that could still point at an older testing or ready request. The tests-under-load slice delivers LHW-001 and the quartio-v2 operator handoff. The staging work and its supersession regression deliver LHW-003 and LHW-004 in the follow-up.

On the previous head, B’s receipt reported the pending request for A and the supersession regression failed. The receipt now uses the same admission decision as the staging queue. The same assertion passed, along with 38 other CI-profile Rust cases. A verified earlier revision still limits the task to one successful staging deployment.
