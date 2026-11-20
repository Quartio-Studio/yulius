---
yulius-task: 29105450-6c66-4146-97e3-5061f2033fc2
status: in-progress
---

# SPEC-576 Merge queue batching and agent guidance

## Goal

Verify that four eligible heads enter the next candidate together. Give agents accurate instructions while the integration branch moves and accurate commit identities after a merge.

## Context

[Issue #1133](https://github.com/Quartio-Studio/yulius/issues/1133) and the November 9 operator report in task #423 (session `c19aae80-4967-4661-a807-471fec6d91d6`, line 3170) record two PR pairs and a 09:24–09:34 UTC idle interval in quartio-v2. The report lacks admission and dispatch events. Task #513's investigation and [the issue comment](https://github.com/Quartio-Studio/yulius/issues/1133#issuecomment-7022218934) identify the same evidence gap. Today's admission contract includes running pre-checks under SPEC-493 and SPEC-497.

The requester reports that quartio-v2 task #331, PR #1481, named checked head `b37144fd2` as the dev landing and printed an empty peer list. GitHub and the task merge record give squash `55c569f5d`. Both have tree `3037ee84f`, and the checked head is outside the squash's ancestry. The train formatter uses the pushed head and unconditionally lists other members. The ordinary armed merge verdict and merged PR status also describe the checked head as a landing.

## Requirements

- **MQG-001** Four green, behind, armed and eligible PRs arriving during an active candidate form one candidate on the next formation pass in arm order.
- **MQG-002** Wait and delivery status tell an agent that the queue owns base updates while its armed PR waits behind the integration branch.
- **MQG-003** Merge guidance identifies the actual landing and the checked PR head separately. A singleton names no peers. A multi-member train names its peers in train order. If GitHub supplies no landing SHA for an observed merge, the note identifies only the known head and branch.
- **MQG-004** A protected merge reports its returned commit even if a read of the closed PR cannot confirm the base. A stale or mismatched PR observation supplies no landing identity.

## Plan

- [x] Verify queue behavior and the November 9 evidence
- [x] Cover the next four heads and reproduce the singleton verdict
- [x] Report the landed commit and verify agent guidance
- [x] Preserve immediate receipts and reject mismatched PR observations
- [x] Preserve the next plan step when a merge observation is stale
- [ ] Obtain independent approval for delivery to dev

## Acceptance criteria

- **AC-1 (MQG-001)** A singleton candidate stays active as four other heads become green. The first subsequent candidate includes exactly those four in arm order.
- **AC-2 (MQG-002)** The candidate member, waiting eligible PR and busy PR get clear queue ownership guidance. The waiting PR sees the same note in wait and status.
- **AC-3 (MQG-003)** With an equal-tree squash on a different SHA, the last train verdict and delivery status identify the landing on dev. The verdict labels the checked head and lists peers only for a multi-member train. Ordinary armed and externally observed merges also use the right identities and retain their existing CI and next-step guidance.
- **AC-4** Keep #1133 open until the November 9 admission and dispatch records support conclusions about both observations.
- **AC-5 (MQG-004)** Refuse the closed-PR read after a successful protected merge. The verdict still contains the returned commit, without attributing it to an unconfirmed base.

## Test plan

Use the existing strict GitHub fixture backed by a bare Git origin. Exercise current merge train and queue note tests. Record four passed heads while the first candidate is active and inspect the next run's membership. Compare hand, ordinary and train merge messages to the actual protected branch commit and checked head. Establish failing assertions before changing product code. Run quick checks, scoped tests, security and mutation checks, then independent review and Yulius CI.

## Decisions

Use the existing protected merge operation, candidate CI verdict and dependency order under [ADR-0285](../../docs/decisions/0285-armed-pull-requests-merge-through-a-queue-that-never-restarts-a-run.md) and [ADR-0357](../../docs/decisions/0357-a-merge-candidate-is-decided-by-its-own-run.md). This is a reporting fix and an acceptance test. Reproducing current batching cannot establish the cause of historical scheduling decisions.

## Evidence

- The regression commit `60d2c85b9` ran against unchanged product code. One running candidate held its place while four heads passed. The next candidate was `train/4+2+5+3`, exactly their arm order. The existing bisection and moving-base pre-check controls also passed.
- Five queue guidance assertions failed on that code. A non-ancestor squash with an identical tree produced the source SHA followed by `in a merge train with :`. Ordinary armed and externally observed merges likewise named the source as the landing. Member and fallback wait notes omitted the instruction about who updates the base. [Reporting defect #1529](https://github.com/Quartio-Studio/yulius/issues/1529)
- With the closed-PR read unavailable, relying on the refreshed delivery view omitted the SHA returned by the successful merge. The regression in `a_green_current_pull_request_merges_on_the_next_pass` failed on that implementation.
- After the fix, `cargo nextest run --no-fail-fast -p yulius -p yulius-app -E 'test(/^(merge_train|agent_wait|pr_watch)::/) | test(/^delivery::post_merge_evidence/)'` passed all 91 selected tests. The singleton compared the bare origin's dev SHA, checked head, trees and ancestry against the last verdict and status note. Multi-member verdicts named the correct intermediate landings, peers and candidate. The tests also covered incomplete and stale PR reads, protected merge refusals, red candidates and continuation of partially delivered plans.
- Round one reviewed `54662c4ca` and requested three changes. With GitHub returning #2 for #1, the task's status displayed #2's landing and the Git overview could advance the stored delivery. With a failed closed-PR read, an immediate green merge returned no receipt. Both acceptance tests failed on the reviewed code. Temporarily reversing peer display made the four-head test fail with `#3, #5, #4` in place of `#4, #5, #3`. The probe was removed.
- After those fixes, all 91 queue, wait, watcher and post-merge evidence tests passed again, including the extended four-head landing through PR #2's peer list. The separate immediate-merge test `delivery::a_task_of_several_slices_merges_each_with_the_rest_ahead` also passed.
- Round two reviewed `a79fad9f0`. A stale open PR read after a locally recorded partial merge withheld the unconfirmed branch and SHA but dropped the known next step from the live watcher notification. The regression at `f1da89749` failed on the missing step. After restoring it, all 92 focused tests passed in nextest run `a4fdd6f6-24f9-4f14-a23c-c7e1e0ff806c`.
