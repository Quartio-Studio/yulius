---
yulius-task: 0f066e1f-c5b7-41ec-a65e-f7289bf1cebb
status: in-progress
---

# SPEC-236: The Notes tab grip's hover check hovers the tab it reads (#503)

Status: in-progress
Date: 2026-10-19
Owner: Yulius task #222
Scope authorization: the person's request (task #222). Fix GitHub issue #503 in
`e2e/tests/notes.e2e.ts` ("the tabs show a grip and say they can be clicked or dragged"). Create
no new GitHub issue; mention the fix on #510 without closing it.

## Goal

The grip's hover check passes on any machine, under any load, because it waits on a condition the
page really reaches: the pointer is on the "Me" tab, and its grip takes the tab's colour. If
something covers the tab, the test fails and says what covers it. It no longer fails with
`expected false to be true` after 5 s.

## Context

- **Symptom (#503, task #143, 2026-10-15 ~08:00Z).** One full `npx e2e run`, next to
  `scripts/dev test`, failed the hover poll (`notes.e2e.ts:606` when filed, :621 now):
  `expect.poll(...).toBe(...) timed out after 5000 ms, last: expected false to be true`.
- **Candidates.**
  1. A toast over the tab. The mock's sandbox browser asks for an AWS sign-in 5 s after the start.
     While a notepad has the keyboard, the toast moves to the top edge (WCAG 2.4.11), over the Notes
     tabs, and pauses while pointed at.
  2. The mock's ambient task changes (about 12, 16 and 20 s) moving something under a still
     pointer, as in #515.
  3. A slow colour transition. Ruled out: the grip's colour is a plain `:hover` rule.
- **Cause: the sign-in toast over the Notes tabs (candidate 1).** With the toast on and the
  unchanged case hovering after the toast shows, `:hover` is on `#toasts > .toast.toast-warn`. The
  case then fails with `expect.poll(...).toBe(...) timed out after 5000 ms, last: expected false to
  be true`, the issue's exact signature. Without the toast it passes. A run at rest reaches the
  hover in about 2 s, before the toast; a loaded run reaches it after 5 s.
- **Already removed from the file by #523.** #503 was filed at 08:00Z on 2026-10-15. PR #523
  (SPEC-148 E2L-006, merged 14:02Z the same day) found the same cause under load ("failed 10 of
  10") and opened every notes mock page with `browserask=0`. It did not reference #503, so the
  issue stayed open.
- **Candidate 2 ruled out.** On dev (`browserask=0`), a probe hovered "Me" again and again until
  28 s after load under a held 0.2 CPU share: 1922 hovers in 8 page loads, 0 misses. The task
  changes at 12-20 s regroup the sidebar, not the Notes tabs.
- **Why the test was fragile.** It read the tab's box once and moved the mouse there with no hit
  check. Anything over the tab at that moment turned into a 5 s poll that cannot recover, because
  the toast stays while pointed at.
- **Not a product defect.** The toast moves away from the focused notepad and pauses under the
  pointer as WCAG 2.4.11 and 2.2.1 ask; a person moves the pointer.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GRH-001 | The case hovers "Me" through `browser.locator('#notes-tab-me').hover()`, which finds the tab again, waits until it is visible and stable, and moves the pointer only once nothing covers it (Playwright's hit check). It no longer makes a raw `mouse.move` to a box read earlier. A toast over the tab is waited out instead of hovered. The tab's box for the drag is read after the hover. | `notes.e2e.ts`; forced-toast run in Evidence |
| GRH-002 | The case opens the mock with `ambient=0`, so no ambient activity (task changes, toasts) runs while the pointer rests. | `notes.e2e.ts` |
| GRH-003 | A grip that does not take the tab's colour on hover still fails the case. | mutation run in Evidence |

## Plan

- [x] Spec SPEC-236 drafted with the failure's candidates
- [x] Failure reproduced under load and the cause named
- [x] Hover check waits on a still, really hovered tab
- [x] File passes loaded runs 1-10
- [x] File passes loaded runs 11-20
- [x] Gate green and spec Evidence written
- [ ] Independent review approves the final head
- [ ] Merged into dev with #503 and #510 commented

## Acceptance criteria

- The failure is reproduced, and its cause is named with the element that really has `:hover`.
- With the cause forced (the toast shown before the hover), the unchanged case fails with the
  issue's signature and the changed case passes. With the grip's `:hover` rule removed, the changed
  case fails.
- `npx e2e run tests/notes.e2e.ts` passes 20 runs in a row under a held 0.2 CPU share, with no
  retries and no sleeps.

## Test plan

- A probe that is not committed: it hovers "Me" again and again until 24-28 s after load and logs
  `document.querySelectorAll(':hover')`. Run with the toast on and with `browserask=0`, at rest and
  under a held 0.2 CPU share.
- The changed case with the cause forced (toast on, hovering after the toast shows), then without.
- 20 runs of the whole file under a held 0.2 CPU share; the full suite; `scripts/dev ci`;
  `scripts/dev security`.

## Decisions

- **Hover through the locator, not a point.** Options:
  - A poll alone does not help: the toast stays while pointed at, so the hover never recovers.
  - Hover again until the colour changes: a retry, which the task rules out, and it would hide a
    grip whose colour really fails.
  - `locator.hover()`: Playwright resolves the tab, waits until it is visible and stable, and
    checks the hit target. It is the condition the page really reaches ("the pointer is on the
    tab"). Chosen.
- **`ambient=0` as well, though the probe showed the task changes do not reach the tabs.** The case
  asserts a still `:hover` state. A future ambient event with a toast would race it the same way,
  and `browserask=0` only leaves out the browser request. Cost: none, the case uses no ambient
  activity.
- **Fix the test, not the page.** The toast's placement and pause are right for a person.
- No ADR. This applies the existing rule (wait on conditions, on a page that only the test
  changes), as SPEC-148 E2L-006 and SPEC-222 did.
- **Out of scope.** The other cases in `notes.e2e.ts` that point at the tabs already avoid the
  toast through `browserask=0`. #510 lists other notes timeouts under load; this change covers
  only this case, so #510 stays open.

## Evidence

All runs were on `feat/222` at dev b9080ea4, in a sandbox with 12 cores and `YULIUS_CPUS=4` (2
e2e workers), on a host at load average 150-195. A load controller (scratchpad, not committed) held
a 2 s CPU-bound probe at 0.2 of a CPU. It added or removed busy loops (`sh -c 'while :; do :; done'`)
every ~20 s, from the median of three 2 s probes.

- **Reproduction of the exact signature (throwaway copy of the case, removed).** The case was copied
  unchanged, with two changes: it opened the mock without `browserask=0`, and it waited for
  `.toast-warn` to be visible before reading the "Me" box. This forces the timing of a run that
  reaches the hover after 5 s.
  - Toast on: `:hover` was `["toasts", "toast toast-warn"]`. The case failed at the hover poll with
    `ASSERTION_FAILED: expect.poll(...).toBe(...) timed out after 5000 ms`, `last: expected false
    to be true`, the issue's exact output.
  - Toast off (`browserask=0`): `:hover` was `["term-tabs notes-tabs", "notes-tab-me"]`. Passed.
  - At rest, a probe that hovered "Me" again and again with the toast on had `:hover` on
    `#toasts > div.toast.toast-warn` from 6.6 s on. It was still there 5 s later with the pointer
    still.
- **Ambient task changes ruled out (throwaway probe, removed).** With `browserask=0` (dev's
  setting), the probe hovered "Me" again and again until 26-28 s after load.
  - At rest: 3 page loads, 1204 hovers, 0 misses.
  - Under the held 0.2 share: 8 page loads, 1922 hovers, 0 misses.
- **Changed case with the cause forced.** The same throwaway copy, with
  `browser.locator('#notes-tab-me').hover()` instead of the raw move. Toast on, hovering after the
  toast showed: passed, `:hover` on `notes-tab-me` (the hover waited the toast out). Toast off:
  passed.
- **Mutation.** `.notes-tabs .term-tab:hover .notes-tab-grip` removed from `styles.css:2295`, with
  the changed case alone: failed with `expect.poll(...).toBe(...) timed out after 5000 ms`, `last:
  expected false to be true`. The hover does not hide a grip that loses its hover colour. Reverted.
- **20 runs in a row under load.** `E2E_TELEMETRY_DISABLED=1 npx e2e run tests/notes.e2e.ts`, with
  no retries (`retries: 0`) and no sleeps.
  - 20 of 20 runs passed, 36 of 36 tests each (720 tests), from 14:38 to 16:45.
  - Each run took 305-467 s (10-13 s per test).
  - The controller logged 381 samples over the window: mean share 0.201 (min 0.051, max 0.493),
    with 11 loops on average. `scripts/dev ci` ran during runs 2-13, so the controller held
    the share with fewer loops.
  - Comments in the case changed during run 1 (review wording, c4159509); the code did not.
- **Gate.** `python3 scripts/bootstrap.py && scripts/dev ci` exited 0: 2258 tests passed, 17
  skipped.
- **Security.** `scripts/dev security` ran Vidoc on vidoc-1 over 3 changed files (1 sent in 2
  chunks, 2 prose skipped): "no open critical or high finding, and no open medium one in the
  change".
- **Not run.** The full e2e suite: this change touches one case of `notes.e2e.ts`, which passed 20
  loaded runs in full. Yulius CI's own load, before the PR's CI run.
- **Not tested.** A toast that appears in the few milliseconds between `hover()`'s hit check and
  its move would still get the pointer and time the hover out. Here it would fail with Playwright's
  call log rather than the colour poll. Nothing in this case shows a toast (`browserask=0`,
  `ambient=0`).
