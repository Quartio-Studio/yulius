---
yulius-task: b236ae1b-aff5-4c42-85cf-89cc222ed718
status: done
---

# SPEC-563 Honor a close or stop taken by the reader when escalation records host loss

## Goal

Honor the person's close or stop of an answered task when its reader has taken the request and the escalation wins the attempt transition. Fixes #1434 for task #541. Fixes #1498 in the parked-screen regression fixture.

## Context

On dev at `ba2c36264`, including task #540's PRs #1492 and #1493, the supervisor's reader takes the end intent before recording the attempt. Only a park leaves an attempt-scoped mark. The loss recorder then sees an empty intent and uses the activity-based loss rule. An answered task has waiting activity and that rule records a lost stop. SPEC-512's race test covers park alone.

## Requirements

- END-1. If loss wins while the reader holds Close, an answered task ends closed.
- END-2. In the same order with Cancel, the task ends stopped_cancelled.
- END-3. Preserve the winner's lost attempt record, the other recorder orders, newer end intents and the existing parking and switch rules.

## Plan

- [x] Reproduce close and stop at the reader’s run-end race point
- [x] Honor the winning recorder’s task outcome and verify the other orders
- [x] Obtain independent approval for the host-loss fix
- [x] Prepare the verified head and issue-closing reference for delivery

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

- Independent review of `b42d72ea9` requested evidence for a superseding Cancel, reader-first attempt reasons and stale attempt ownership. The tests at `8305a8cc8` exercise all three. An isolated fault probe preferred the older Close, wrote `exited` for reader-first requests, and let a stale attempt take the handoff. All four assertions failed with their expected diagnostics.
- The first review's unmutated baseline passed 3,213 selected tests. The integrated branch incorporates dev `3cc2b1969`. All five host-loss request orderings passed there. The wider run exposed a race in the parked-screen fixture from task #540 when its queue drain consumed a slot the test assumed unavailable.
- For #1498, forcing the drain after the synthetic Parked-to-Queued transition reproduced `Running` where the fixture required `Queued`. Both parked-screen callers now occupy their only slot with a live fixture task, force the drain, and check the queued screen before advancing through the remaining states.
- On the integrated branch, nextest passed 46/46 selected parking, Codex resume, switch, and task-end integration tests. The attempt-owner unit test passed separately. Repository contracts and formatting passed. Vidoc was disabled.
- Fresh independent review approved `1a831e26e` after independently running nine lifecycle integration tests and the ownership unit test. It confirmed all first-round findings were resolved and the capacity fixture retained its cold-screen checks. The review follow-up introduced test assertions only. The mutation selector listed zero candidates in those follow-up lines.
- `scripts/dev mutants` reported 16 listed, 12 caught, 0 missed, 0 timed out, 4 unviable, and 0 not reached (30-minute bound, 1,297 s). All viable mutations in the changed runtime lines were detected.
