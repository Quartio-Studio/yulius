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

## Plan

- [x] Verify queue behavior and the November 9 evidence
- [ ] Cover the next four heads and reproduce the singleton verdict
- [ ] Report the landed commit and verify agent guidance
- [ ] Obtain independent approval and verify delivery to dev

## Acceptance criteria

- **AC-1 (MQG-001)** A singleton candidate stays active as four other heads become green. The first subsequent candidate includes exactly those four in arm order.
- **AC-2 (MQG-002)** The candidate member, waiting eligible PR and busy PR get clear queue ownership guidance. The waiting PR sees the same note in wait and status.
- **AC-3 (MQG-003)** With an equal-tree squash on a different SHA, the last train verdict and delivery status identify the landing on dev. The verdict labels the checked head and lists peers only for a multi-member train. Ordinary armed and externally observed merges also use the right identities and retain their existing CI and next-step guidance.
- **AC-4** Keep #1133 open until the November 9 admission and dispatch records support conclusions about both observations.

## Test plan

Use the existing strict GitHub fixture backed by a bare Git origin. Exercise current merge train and queue note tests. Record four passed heads while the first candidate is active and inspect the next run's membership. Compare hand, ordinary and train merge messages to the actual protected branch commit and checked head. Establish failing assertions before changing product code. Run quick checks, scoped tests, security and mutation checks, then independent review and Yulius CI.

## Decisions

Use the existing protected merge operation, candidate CI verdict and dependency order under [ADR-0285](../../docs/decisions/0285-armed-pull-requests-merge-through-a-queue-that-never-restarts-a-run.md) and [ADR-0357](../../docs/decisions/0357-a-merge-candidate-is-decided-by-its-own-run.md). This is a reporting fix and an acceptance test. Reproducing current batching cannot establish the cause of historical scheduling decisions.
