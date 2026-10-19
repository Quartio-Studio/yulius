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
  `scripts/dev test`, failed the hover poll (`notes.e2e.ts:606`, now :613):
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
| GRH-001 | The case hovers "Me" through `browser.locator('#notes-tab-me').hover()`, which finds the tab again and waits until the tab itself takes the pointer. It no longer makes a raw `mouse.move` to a box read earlier. A toast over the tab is waited out instead of hovered. The tab's box for the drag is read after the hover. | `notes.e2e.ts`; forced-toast run in Evidence |
| GRH-002 | The case opens the mock with `ambient=0`, so nothing on the page changes by itself while the pointer rests. | `notes.e2e.ts` |
| GRH-003 | A grip that does not take the tab's colour on hover still fails the case. | mutation run in Evidence |

## Plan

- [x] Spec SPEC-236 drafted with the failure's candidates
- [x] Failure reproduced under load and the cause named
- [x] Hover check waits on a still, really hovered tab
- [ ] File passes 20 consecutive loaded runs
- [ ] Independent review answered and gates green
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
