---
yulius-task: 0f066e1f-c5b7-41ec-a65e-f7289bf1cebb
status: draft
---

# SPEC-236: The Notes tab grip's hover check hovers the tab it reads (#503)

Status: draft
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
- (Cause and evidence are filled in as the reproduction runs.)

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GRH-001 | The case hovers "Me" through a locator, which finds the tab again and checks that it receives the pointer. A raw move to a box read earlier does not. If something covers the tab, the case fails and names it. | `notes.e2e.ts`; forced-cover run in Evidence |
| GRH-002 | The case opens the mock with `ambient=0`, so nothing on the page changes by itself while the pointer rests. | `notes.e2e.ts` |

## Plan

- [ ] Spec SPEC-236 drafted with the failure's candidates
- [ ] Failure reproduced under load and the cause named
- [ ] Hover check waits on a still, really hovered tab
- [ ] File passes 20 consecutive loaded runs
- [ ] Independent review answered and gates green
- [ ] Merged into dev with #503 and #510 commented

## Acceptance criteria

- The failure is reproduced, and its cause is named with the element that really has `:hover`.
- With the cause forced, the changed case fails and names what covers the tab. Without it, the case
  passes.
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

- (Filled in once the cause is confirmed.)
