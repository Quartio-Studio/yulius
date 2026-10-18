---
yulius-task: d2a80c6a-e19c-4259-91c0-276df322fe29
status: done
---

# SPEC-224: Flaky e2e under load: watching self-poll and onboarding duplicate toast

Status: done
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
- [x] Onboarding: a left composer's late read says nothing (regression test, fix)
- [x] Watching: self-poll waits each read on its own budget (probe reproduction, fix)
- [x] Docs, changes entry, gates and a full e2e run green
- [x] Review round 1 answered: pause flake recorded on #354, plan and wording corrected
- [x] Delivered: the approved head merged into dev on green CI (Fixes #581)

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
- **The onboarding tests wait for the start's read in the sidebar.** Found while writing the
  regression test: the document title follows task events only (`app.js` refreshes `currentTask`
  on `task`, not on `tasks`), so the old wait for "Read again" in the title saw the mock's own
  task event 1.2 s after the start, never the start's read of the tasks. `endStart` waits for that
  event first, then renames the task, releases the held read and waits for the sidebar's list.
  The list follows task events too, but each event carries the task as it was when it went out,
  so after that event only the held reads bring the new title. That the title keeps a list reload's new title only until the next
  task event is a small pre-existing display lag, outside this change (noted on #581).
- **A mock hold for the recovery read** (`holdRecovery = promise`), like the mock's other
  per-route holds (`holdApprove`, `holdCreate`), since `hold=` keeps only one path and the test
  already holds the tasks read with it.
- **Wait per read, not a longer single budget.** Three 10 s waits keep the test's meaning (each
  read comes on its own, faster than the mock's task events) while one 30 s wait would let event
  reads alone pass it.
- No ADR: no lasting decision beyond these test-local and view-local choices.

## Evidence

All on this branch (base `dev` a3b9b04a), 2026-10-18, sandbox CPU budget 4 (`YULIUS_CPUS`), host
load average 17–58 (parallel tasks' suites).

- Onboarding reproduction (temporary probes, not committed): trace points in `openOnboarding`, its
  timer, `destroy()`, the start's check and the task route; 0–900 ms extra mock latency; a
  0–600 ms open timer. 2 of 8 runs showed the destroyed composer's `openOnboarding` called from the
  recovery read (`taskview.js:1239`) after `composer destroy` (trace in Context).
- New test "a composer left before its reads answer adds no second toast to the start", run 5×
  and (after the `endStart` rework) 4× in a temporary copy against `dev`'s `taskview.js`: 9/9
  failed with `expected ["Onboarding started: opened #1.","Onboarding started."]`, task #204's
  failure. With the fix: 4/4 passed, together with the old #581 test 4/4.
- Watching reproduction: a temporary `await delay(1800)` in the mock's
  `GET /api/tasks/{id}/transcript`, the test run 3×: 3/3 failed with `expected 5 to be greater than
  5` (the issue's own failure). With the per-read waits: 3/3 passed under the same probe. With
  `WATCH_POLL_MS = 1e9` (a view that does not poll by itself): 3/3 failed, so the test still
  checks self-polling.
- `npx e2e run tests/onboarding.e2e.ts`: 14/14 passed.
- Full `npx e2e run` at load average ~56: 617 passed, 1 failed, 1 skipped (619); onboarding 14/14
  and watching 24/24 passed. The failure, `pause.e2e.ts` "the Tasks list row menu pauses and
  resumes a task" (the row's action not visible within 29 s), is outside this change (no pause,
  task-list or row-menu code touched); `tests/pause.e2e.ts` alone then passed 3/3 (12/12). No
  open issue tracked it; it is recorded on #354 (first-load waits under host load, open), since
  this task opens no issues. The acceptance criterion's full run is therefore CI's e2e job at the
  head (below, in the pull request).
- `scripts/dev ci`: passed (exit 0; nextest 2233 passed, 18 skipped).
- `yulius security-scan` (vidoc-1, change scan at b825edee): no open critical or high finding. One
  medium, `taskview.js:340` (a `needs_you` link used as `href`), is in code this branch does not
  touch (its changes are at ~1239 and ~1310), so it does not block; Yulius CI files it.
- Probe files and edits removed; `git status` clean before the evidence commit.

Independent review, round 1 (fresh session, head 8d045109, read-only): no own-code defect. It
confirmed the cause, the two guards (no TDZ: every synchronous `renderBanner` call comes after
`let destroyed`), the regression test's order, `endStart`'s premise and the watching waits. Its
findings and their answers:

1. Blocker (pre-existing): the full local e2e run had the `pause.e2e.ts` flake. Answer: it was
   recorded on #354, which stays open; the green full run is CI's e2e job at the delivered head.
2. Minor (own): the Plan ticked review and merge before they happened. Answer: the review now has
   its own item. The delivery item stays ticked in the head that gets merged, as in SPEC-219,
   because nothing can be committed after the merge. `delivery_merge` merges only this exact
   head, approved and green; until then the pull request shows the item is not yet true.
3. Nit (own): the Decisions said the sidebar follows list reads only. Answer: corrected.
4. Minor (pre-existing, `taskview.js:1317`, unchanged): a view alive when it calls
   `openOnboarding` but destroyed before its 0 ms timer still deletes the shared announcement.
   Answer: left as it is. Only one composer is mounted at a time, and a composer mounted next on
   a `preparing` sandbox has `offered` false, so it neither adds nor needs the entry; the start's
   check meanwhile finds it missing only if that same view never showed its toast, and then the
   start's own "Onboarding started." is the right single toast. Noted on #581.
5. Nit (own): the regression test pins the two guards together, not each alone. Answer: accepted,
   as the review did.
