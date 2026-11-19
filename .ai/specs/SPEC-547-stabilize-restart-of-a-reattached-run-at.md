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

## Plan

- [x] Isolate the failing reattach and restart ordering
- [x] Prove the ordering with a failing regression
- [ ] Restore reliable restart and normal CI coverage
- [ ] Verify the restart under 48 busy loops
- [ ] Obtain independent review and deliver on green CI

## Acceptance criteria

- AC-1. Reattaching the limited run while another subscription has room resumes it once, with its limit record removed and the current owner's servers.
- AC-2. The integration test passes 40 iterations beside 48 busy loops.

## Test plan

Exercise two server runtimes, a detached fake agent, and real subscription and hand-over paths. Delay one path at its decision until the competing path reaches the relevant state. Check the attempts, account, owner and resume arguments. Run the adjacent reattach tests and the requested loaded nextest command.

## Decisions

Reconciliation takes the task’s move lock before attaching the host. It releases the lock once ownership is checked, or transfers it to the required hand-over until that restart returns. Dropping either future releases the lock. Transcript replay can record the limit and complete while the deferred move waits. The move compares the reporting attempt with the attached attempt after acquiring the lock. ADR-0389 records the decision. The quarantine is managed by Yulius and released when tracking issue #1431 closes (ADR-0356). The separate regression runs in CI while that quarantine is active.


## Evidence

- On dev 8d05c25b with diagnostic logging, nextest failed iteration 2 of 12 beside 48 busy loops. The hand-over logged a concurrent state change, the pool logged a stop during its switch, and the test timed out after 65.4 seconds. SQLite showed one killed attempt and a cancelled switch.
- A temporary task-keyed probe held the hand-over after preflight until the pool move finished. The task ended Stopped(Cancelled) in 6.3 seconds.
- The retained regression held the replayed limit move until the hand-over completed. Releasing it created attempt three. The two-attempt assertion failed in 7.9 seconds.
