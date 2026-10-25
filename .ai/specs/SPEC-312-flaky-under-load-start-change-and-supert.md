---
yulius-task: c2a0e6b6-f66d-47b9-badd-c1861b283082
status: in-progress
---

# SPEC-312: Start-change, supertask, ownership and reply e2e checks wait on what they need (#506)

Status: in-progress
Date: 2026-10-25
Owner: Yulius task #298
Scope authorization: the issue-triage request (task #298). Fix GitHub issue #506 in one pull
request (`Fixes #506`). Create no new GitHub issue; comment progress and leftovers on #506.

## Goal

The e2e checks #506 lists pass at any host load, because each waits on the condition it needs,
with no retries and no longer first-load budgets (#354's open decision).

## Context

- **Symptom (#506).** Under host load 33-78, `start-change.e2e.ts` ("a chained task changes to
  When I decide…", the check on `.attention-bar` button `/^Starts:/` timed out), `supertask.e2e.ts` ("Supertask is a start
  choice…", `toHaveText` failed), `ownership.e2e.ts` (two cases, text timeouts) and, in a comment,
  `reply.e2e.ts` ("Reply on Needs you…", `toHaveText` after a `touchTask`) failed; they pass alone.
- **Already fixed elsewhere.** Editor presence: #523 (023b11c9, the `MONACO_LOAD_MS` wait). The
  ownership text the issue cites ("@anna-q owns this task:") was replaced by #572 (60d144b4).
- **Candidates (from triage at dev 9e1a2870).**
  1. Ambient activity: the mock changes studio tasks every 4 s tick after the events socket opens
     (#8 queued→starting at ~12 s, running at ~20 s; #12 touched every 16 s).
  2. A `touchTask` before the page's events socket is open: the mock, like the server, announces a
     change only to connected pages (`mock.js` `emit`), and the first connection does not resync
     (`store.js` `startEvents`).
  3. A task view detail read (`GET /api/tasks/{id}`) answered before a change and arriving after the
     change's `task.updated` event: `taskview.js` `loadDetail` applied the answer's summary (owner,
     status, activity, control request) over the newer one. Its guard only orders reads among
     themselves (SPEC-056 WUI-105), not reads against events.
  4. start-change clicks the chip right after the URL changes. Ruled out: a click waits for its
     element within the action budget (30 s), not an assertion's 5 s.
- **Causes, each forced by a throwaway probe (Evidence: probe A, faster ambient ticks; probe B, a
  late `/api/events`; probe C, late detail answers).**
  - `supertask.e2e.ts`: candidate 1. With the ambient tick 10× faster the unchanged case failed
    with the issue's `toHaveText`, observing "Supertask · waits for #6, #8, #9 and 3 more": #8 had
    started before the supertask was created.
  - `ownership.e2e.ts` (both cases) and `reply.e2e.ts`: candidate 2. With `/api/events` opening 3 s
    late, the touch was lost and the next text never showed.
  - `reply.e2e.ts` and `start-change.e2e.ts` :95: candidate 3. With detail answers applied 1.5 s
    late, reply's waiting case had its `activity` set back, closing the field. start-change's
    signature (the `Starts:` chip still there after the hand-over) is this rollback: the change to
    When I decide reads the detail again, and the hand-over landed while that read was out.
  - `editor.e2e.ts`: fixed by #523; the ownership text the issue quotes is gone since #572.
- **Not reproduced by load alone.** The unchanged files on dev passed 6 of 6 runs at a held 0.2
  CPU share (load average ~70) and 5 of 5 at 0.1 (~90), as with #515: the races need a timing that
  a full suite's parallel runs hit and a single file's runs rarely do. The probes force each one.
- **Why the tests missed it.** No test answered a read before a change and delivered it after.
  The mock answers a read after its latency, from the state then, so the window was only the
  page's own handling of the answer.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FUL-001 | Each listed failure is reproduced (probe delays or a held CPU share) and its race named. | Context, Evidence |
| FUL-002 | `touchTask` (`e2e/tests/support.ts`) waits until the page is connected to `/api/events` (`eventsConnected`) before it changes the task; `ownership.e2e.ts` and `start-change.e2e.ts` use it. | probe B runs |
| FUL-003 | The supertask case that names the studio's working tasks opens the mock with `&ambient=0`. | probe A runs |
| FUL-004 | The task view counts the task changes it hears (`update`: events and answers to its actions). A detail read during which one arrived applies its details but not its summary; otherwise the summary applies as before. A read answered before a hand-over and arriving after it, both dated the same second, leaves the newer owner. | `ownership.e2e.ts` "a read answered before a hand-over…", fails without the fix |
| FUL-005 | The mock's `touch` takes `changes.updated_at`, so a test dates a change to the second of a read's answer, as the server dates to the second. | `mock.js` |
| FUL-006 | No retries, sleeps or longer first-load budgets (#354). | diff |

## Plan

- [ ] Spec SPEC-312 drafted with each test's race candidates
- [ ] Each failure reproduced with probe delays and its race named
- [ ] Supertask and reply checks independent of ambient activity
- [ ] Start-change's hand-over waits for the events socket
- [ ] Ownership touches wait for the events socket
- [ ] Task view keeps a newer event over an older detail read
- [ ] Files pass repeated runs under a held 0.2 CPU share
- [ ] Gate green, spec Evidence and changes entry written
- [ ] Independent review approves the final head
- [ ] Merged into dev with #506 commented

## Acceptance criteria

- Each race is forced by a probe and fails the unchanged check with the issue's signature; the
  changed check passes with the same probe.
- The changed files pass repeated runs under a held 0.2 CPU share, with no retries.

## Test plan

- Throwaway probes (not committed): delayed events socket, faster ambient ticks, delayed detail
  answers. Unchanged and changed checks under each.
- Repeated runs of the four files under a held 0.2 CPU share; `scripts/dev ci`; `scripts/dev security`.

## Decisions

- **Product fix for the rollback, not a test wait.** The server answers a read and broadcasts a
  later change on another connection, so a real page meets the same order. Options:
  - Wait in the test until the detail read is done before touching: hides a real page defect.
  - Read again after every event: more requests on every change, still racing.
  - Compare `updated_at`: first chosen, then dropped after review round 1. The server dates to the
    second (`rfc3339`, `crates/yulius-app/src/auth.rs`), so a read answered in the same second as
    the change carries the same time and the older answer went through. The new same-second case
    failed with it.
  - Millisecond times or a revision per task from the server: changes the API contract for a
    client-side ordering problem.
  - Count the changes the view hears, and leave the summary of a read during which one arrived:
    chosen. Every change that alters the summary is announced with the whole summary
    (`TaskSummary` holds every key `pickSummary` takes), so the view then shows the newest it
    heard, and a later change brings a later event. The read's details (steps, attempts, request)
    still apply. A read the view starts itself on a status change counts from after that change.
  - The store's `upsertTask`, and the answer to this page's own `PUT …/start`, have no ordering
    either (review round 1, pre-existing): an answer computed before a hand-over and arriving after
    it would bring back the old owner. Outside this change; noted on #506.
- **The socket wait lives in `touchTask`**, the helper every test uses to announce a change, so a
  caller cannot forget it. Other files that call `__yuliusMock.touchTask` directly were not
  reported and stay unchanged; they are listed on #506.
- **`ambient=0` for the supertask case only.** The second supertask case names no working task.
- **No ADR.** Applies existing rules (SPEC-056 WUI-105 for read ordering, wait on conditions).
