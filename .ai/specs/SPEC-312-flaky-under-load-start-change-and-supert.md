---
yulius-task: c2a0e6b6-f66d-47b9-badd-c1861b283082
status: done
---

# SPEC-312: Start-change, supertask, ownership and reply e2e checks wait on what they need (#506)

Status: done
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
  - `reply.e2e.ts` "Escape closes the reply field…" (not listed in #506; failed 1 of 10 loaded
    runs at the final head of round 1, and 1 of 4 probe C runs on dev): after Escape the keyboard
    was on the body. `renderAttention` takes every child but the reply field out of the bar and
    puts the kept Reply button back, and taking a focused element out of the page drops the focus.
    The mock's ambient touch of #12 (every 16 s) landed after Escape in a run slowed to 19.8 s.
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
| FUL-007 | The attention bar gives the keyboard back to a control it kept across a rebuild (Reply), so a task update leaves the focus where it was. | `reply.e2e.ts` "Escape closes…", fails without the fix |

## Plan

- [x] Spec SPEC-312 drafted with each test's race candidates
- [x] Each failure reproduced with probe delays and its race named
- [x] Supertask and reply checks independent of ambient activity
- [x] Start-change's hand-over waits for the events socket
- [x] Ownership touches wait for the events socket
- [x] Task view keeps a newer event over an older detail read
- [x] Files pass repeated runs under a held 0.2 CPU share
- [x] Changes entry and docs follow the final fix
- [x] Review round 1 answered: reads ordered by heard changes
- [x] Attention bar keeps the keyboard on Reply across task updates
- [x] Full e2e suite passes at the final head
- [x] Loaded runs pass at the final head
- [x] Gate green and spec Evidence at the final head
- [x] Independent review approves the final head
- [x] Merged into dev with #506 commented

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
    chosen. Every change that alters the summary is announced with the summary (`TaskSummary`
    holds every key `pickSummary` takes; only `account_email`, which the view does not show, is
    left out of events), so the view then shows the newest it
    heard, and a later change brings a later event. The read's details (steps, attempts, request)
    still apply. A read the view starts itself on a status change counts from after that change.
  - The store's `upsertTask`, and the answer to this page's own `PUT …/start`, have no ordering
    either (review round 1, pre-existing): an answer computed before a hand-over and arriving after
    it would bring back the old owner. Outside this change; noted on #506.
- **The socket wait lives in `touchTask`**, the helper every test uses to announce a change, so a
  caller cannot forget it. Other files that call `__yuliusMock.touchTask` directly were not
  reported and stay unchanged; they are listed on #506.
- **The bar refocuses a kept control rather than moving focus by position.** `keepFocus` (dom.js)
  would move the keyboard to whatever button takes the old one's place, which could put Enter on
  Hand over. Only a control that is the same element after the rebuild (Reply) gets it back; a
  field closed under typing still leaves it on the bar.
- **`ambient=0` for the supertask case only.** The second supertask case names no working task.
- **No ADR.** Applies existing rules (SPEC-056 WUI-105 for read ordering, wait on conditions).

## Evidence

All runs were on `feat/298` from dev 9e1a2870, in a sandbox with 12 cores and `YULIUS_CPUS=4` (2
e2e workers). A load controller (scratchpad, not committed) held a 2 s CPU-bound probe at a target
share by adding or removing busy loops (`sh -c 'while :; do :; done'`) every ~18 s, from the median
of three probes. Probes were throwaway edits, reverted before each commit.

- **Probe A (ambient tick 4 s → 0.4 s, `mock.js` `startAmbient`).** Unchanged `supertask.e2e.ts`:
  failed `expect.toHaveText` on `.strip-status`, expected `/^Supertask · waits for #6, #9, #10 and
  \d+ more$/`, observed "Supertask · waits for #6, #8, #9 and 3 more". With `ambient=0`: 3 of 3
  passed. `reply.e2e.ts` passed under it (6 of 6): its races are elsewhere.
- **Probe B (`/api/events` opens after 3 s, `MockSocket`).** Unchanged files: `ownership.e2e.ts`
  failed 2 of 4 ("someone else's task…" `toBeHidden` of "You asked @anna-q for control.", "the
  owner is asked…" `toBeVisible` of "@anna-q asks to take control…") and `reply.e2e.ts` 3 of 6
  (`.attention-bar` `toHaveText`). With `touchTask` waiting for the connection: 13 of 13 passed
  (ownership, reply, start-change).
- **Probe C (each detail answer applied 1.5 s late, `taskview.js` `loadDetail`).** On dev,
  `reply.e2e.ts` "Reply on a task waiting for your reply…" failed 4 of 4 runs (the field closed:
  `activity` set back to the read's) and "Escape closes…" 1 of 4. With the heard-changes order: 3
  of 3 runs of reply, ownership and start-change passed (14 tests each).
- **Regression tests, each failing before its fix.**
  - `ownership.e2e.ts` "a read answered before a hand-over…": failed at line 88 with no guard
    (f4b00714), and at line 96 with the `updated_at` guard once the change shares the answer's
    second (237245d5); passes with 5fd186c5.
  - `reply.e2e.ts` "Escape closes…": failed at line 72 (b6842d71), the keyboard on the body after a
    task update; passes with 72c7db74.
- **Load alone on dev (9e1a2870, unchanged files).** 6 of 6 runs passed at a held 0.2 share (load
  average ~66-78), 5 of 5 at 0.1 (~86-93).
- **Loaded runs of the changed files** (`start-change`, `supertask`, `ownership`, `reply`, 16
  tests per run, no retries):
  - 2b6a7497: 10 of 10 passed, share mean 0.208 (63 samples).
  - a7c21de4: 9 of 10 passed; run 5 failed reply's "Escape closes…" (focus on the body after
    19.8 s), the cause fixed test-first in b6842d71 and 72c7db74.
  - 7874f33e: 10 of 10 passed, share mean 0.213 (67 samples, min 0.053, max 0.42, load average
    41-80).
  - Seconds per test against the loaded dev runs (0.2 share): ownership 8.0 (dev 8.4), reply 8.3
    (7.9), supertask 13.1 (13.8), start-change 21.8 (22.8), so the proof was as heavy as the
    reproduction attempts.
- **Task-view suites.** ownership, start-change, reply, watching, live-updates, pause,
  queued-messages, task-done and prepared: 88 of 88 passed with 5fd186c5; reply, ownership,
  start-change, prepared, watching, cd and pause: 64 of 64 with 72c7db74.
- **Full suite.** `npx e2e run`: at a7c21de4, 711 tests passed, 1 skipped (agent, no model key),
  1810 s; at 7874f33e, 66 files passed, 1 skipped, 711 tests passed, 1 skipped, 1070 s.
- **Gate.** `python3 scripts/bootstrap.py && scripts/dev ci` exited 0 at a7c21de4 and again at
  7874f33e: 2615 tests passed, 16 skipped; 143 Python tests passed.
- **Security.** `yulius security-scan` at cd88d014: not run, exit 2 (Vidoc out of quota for this
  key, HTTP 429 "This API key has used its 50,000,000 tokens"). Unrun, not passed. The changed code
  is test code and two client-side ordering and focus changes in `taskview.js`, with no new input,
  network or storage path.
- **Review.** Round 1 (cd88d014) requested changes: the `updated_at` guard failed against the
  server's second-precision times; `holdanswer` duplicated `holdDetail`; spec and changes wording.
  All fixed (237245d5, 5fd186c5, a7c21de4). Round 2 (7874f33e) approved on the condition that the
  gate and full suite pass (they did), with two nits, fixed after it (a comment in `taskview.js`
  and a line in `docs/changes.md`), and one pre-existing note for #506: `publish_task` can publish
  two changes of one task out of order, which a read no longer corrects while it is out.
