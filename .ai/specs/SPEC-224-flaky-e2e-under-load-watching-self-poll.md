---
yulius-task: d2a80c6a-e19c-4259-91c0-276df322fe29
status: in-progress
---

# SPEC-224: Flaky e2e under load: watching self-poll and onboarding duplicate toast

Status: in-progress
Date: 2026-10-18
Owner: task #210
Scope authorization: task #210: "Fix GitHub issue #581 … Both tests wait on conditions that hold
under host load and pass in a full run." Fixes #581.

## Goal

The two end-to-end tests of #581 fail only for real defects. A full `npx e2e run` passes them
under host load, without retries:

- `watching.e2e.ts` "a failed first read retried by a task event reads on by itself";
- `onboarding.e2e.ts` "leaving and coming back while Start onboarding runs still shows it loading".

## Context

Both tests pass alone and fail in full suite runs on loaded hosts (#581 and its comments). The
two causes, reproduced on `dev` at a3b9b04a:

**Watching: a too-short wait, the test's own.** `watchview.js` schedules each transcript read
`WATCH_POLL_MS` (1.5 s) after the previous one has been answered (`schedule()`), and the mock
answers after 50–200 ms. The test waits for three more reads in one 10 s poll, about 5 s of
reads on an idle host. A temporary 1.8 s delay in the mock's
`GET /api/tasks/{id}/transcript` (a read answered slowly, as on a loaded runner) failed it 3/3
with the issue's own message, `expected 5 to be greater than 5`. No read timer is lost: the
reads keep coming, just more slowly than 10 s for three.

**Onboarding: a product race, a duplicate toast.** The duplicate (`["Onboarding started: opened
#1.","Onboarding started."]`, task #204) comes from a composer that was already destroyed. With
temporary trace points in `openOnboarding`, the composer's `destroy()`, the start's check and the
task route, plus a random 0–900 ms of extra mock latency and a 0–600 ms open timer, 2 of 8 runs
showed this order after the task view mounted:

```
toast+navigate; route task 1 …; route task mount; composer destroy;
open opening=false destroyed=true offered=true at renderBanner < taskview.js:1239 (the recovery read)
timer destroyed=true connected=false
```

The superadmin's `GET /api/auth/recovery`, which every composer asks for when it mounts, answers
`.then((state) => { recovery = state; renderBanner(); })` without checking `destroyed` (the plan
read beside it does check). A composer left before that read answers renders once more: its
sandbox is now `preparing` with its task known, so `openOnboarding` runs on the dead view. It adds
the sandbox to the shared `onboardingAnnounced` again, and its timer, finding the view destroyed,
deletes it ("not shown"). If that deletion lands before the running start's own check (after its
tasks read), the start finds no announcement and toasts "Onboarding started." as well. In the
2 traced runs the start's check came first by chance, so no toast showed; under load either order
happens. The issue body's first diagnosis ("the toast is likely already gone") was fixed by
399e3fc7 (#603, toasts recorded as they show) and is out of date.

Task #207 suspected the `opening = false` reset after `navigate` (the route mounts the task view
asynchronously). The traces show no render in that gap: with the task list known, the task route
mounts within the same task's microtasks, and nothing that renders the composer runs in between.
That reset is left as it is.

Why the tests missed it: no test lets a composer's mount-time read answer after the composer was
left, and the test that shows the duplicate depends on it happening by chance.

## Requirements

- **FLK-001** A composer that was destroyed acts on no late answer: the recovery read does not
  render it, and `openOnboarding` on a destroyed view does nothing (no announcement added or
  removed, no toast, no navigation).
- **FLK-002** One Start onboarding shows exactly one onboarding toast, also when a composer that
  offered it was left before its own reads answered.
- **FLK-003** The watching test waits for each of its three self-paced reads on its own budget,
  so a slow read on a loaded runner does not fail it. It still fails when the view reads only on
  task events (one per 16 s in the mock): each read must come within 10 s of the one before.

## Plan

- [x] Spec with both reproduced causes
- [ ] Onboarding: a left composer's late read says nothing (regression test, fix)
- [ ] Watching: self-poll waits each read on its own budget (probe reproduction, fix)
- [ ] Docs, changes entry, gates and a full e2e run green
- [ ] Independent review answered, PR merged into dev on green

## Acceptance criteria

- The new onboarding regression test fails on `dev`'s `taskview.js` (two onboarding toasts) and
  passes with the fix.
- The watching test passes with the 1.8 s read-delay probe that failed it 3/3 before.
- A full `npx e2e run` passes, both #581 tests included.
- The project gate (`scripts/dev ci`) and `scripts/dev security` pass (or the scan is reported
  unrun).

## Test plan

- `onboarding.e2e.ts`, new test: the mock holds `GET /api/auth/recovery`
  (`__yuliusMock.holdRecovery`, added for this) while the person leaves and comes back during a
  held Start; the start is released and the task opens; then the held recovery read answers, and
  only after that the start's tasks read. Exactly one toast, "Onboarding started: opened #1.".
- `watching.e2e.ts`: three successive polls (`> after`, `> after + 1`, `> after + 2`), 10 s each.
- Probe runs (temporary, not committed): the read-delay probe for watching; the trace and latency
  probes for onboarding.
- Full `npx e2e run`; `scripts/dev ci`; `scripts/dev security`.

## Decisions

- **Guard the destroyed view, keep `opening`'s reset.** Keeping `opening` true after navigating
  (task #207's suggestion) would also stop a second open in the same view, but the traces show
  no such second open, and a view kept after a navigation that did not replace it (a newer route
  back to the same sandbox page) would then never open the task again. The defect is a dead view
  acting, so the fix is there: the recovery read checks `destroyed` like the plan read does, and
  `openOnboarding` returns on a destroyed view, which covers any other late caller.
- **A mock hold for the recovery read** (`holdRecovery = promise`), like the mock's other
  per-route holds (`holdApprove`, `holdCreate`), since `hold=` keeps only one path and the test
  already holds the tasks read with it.
- **Wait per read, not a longer single budget.** Three 10 s waits keep the test's meaning (each
  read comes on its own, faster than the mock's task events) while one 30 s wait would let event
  reads alone pass it.
- No ADR: no lasting decision beyond these test-local and view-local choices.
