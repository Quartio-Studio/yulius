---
yulius-task: b236ae1b-aff5-4c42-85cf-89cc222ed718
status: in-progress
---

# SPEC-563 Honor a close or stop taken by the reader when escalation records host loss

## Goal

Honor the person's close or stop of an answered task when its reader has taken the request and the escalation wins the attempt transition. Fixes #1434 for task #541.

## Context

On dev at `ba2c36264`, including task #540's PRs #1492 and #1493, the supervisor's reader takes the end intent before recording the attempt. Only a park leaves an attempt-scoped mark. The loss recorder then sees an empty intent and uses the activity-based loss rule. An answered task has waiting activity and that rule records a lost stop. SPEC-512's race test covers park alone.

## Requirements

- END-1. If loss wins while the reader holds Close, an answered task ends closed.
- END-2. In the same order with Cancel, the task ends stopped_cancelled.
- END-3. Preserve the winner's lost attempt record, the other recorder orders, newer end intents and the existing parking and switch rules.

## Plan

- [x] Reproduce close and stop at the reader’s run-end race point
- [ ] Honor the winning recorder’s task outcome and verify the other orders
- [ ] Obtain independent approval for the host-loss fix
- [ ] Prepare the verified head and issue-closing reference for delivery

## Acceptance criteria

- With the reader held after taking Close or Cancel, escalation records a lost attempt and the task settles according to END-1 or END-2.
- A reader that records first retains its usual exit reason. A loss that consumes a pending intent before the reader takes it retains its existing behavior. A parked task retains its screen and activity.

## Test plan

Use the real supervisor, detached ptyhost, fake agent and bounded race-point handshakes. Assert the durable task status and the winning attempt. Run the neighboring parking, task-end and loss regressions and quick checks. Record independent review, security scan availability, CI and issue closure.

## Decisions

Reuse the supervisor's attempt-scoped handoff and existing task transitions. The request concerns the in-process competition between two recorders. The existing store transition decides the winner and keeps its factual exit reason.

## Evidence

- At regression commit `cf86be837` on dev `ba2c36264`, both reader-held tests failed at the task status assertion. Each recorded one lost attempt and returned `Stopped(Lost)`. Close expected `Closed` and Cancel expected `Stopped(Cancelled)`.
- The two loss-before-reader controls passed on the same baseline binary. Both produced `Stopped(Lost)` when loss consumed the live intent first.
