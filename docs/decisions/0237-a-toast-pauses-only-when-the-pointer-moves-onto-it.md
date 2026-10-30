# 0237. A toast pauses only when the pointer moves onto it

## Status

Accepted. Specified in [SPEC-350](../../.ai/specs/SPEC-350-a-toast-under-a-resting-pointer-stays-op.md)
(TST-001..003); task #334, #954.

## Context

No toast disappears while the pointer or keyboard focus is on it (WCAG 2.2.1): `toast()` in
`dom.js` cleared its close timer on `pointerenter` and armed it again on `pointerleave`. Chromium
sends `pointerover`, `pointerenter` and `mouseover`, but no `pointermove`, to an element that
appears under a pointer that does not move. A toast raised where the pointer rests, or brought
there by the stack's flip to the top edge (`keepClear`) or its move between a dialog and the page
(`toastHost`), so paused at once, and with no `pointerleave` to come it stayed until the pointer
moved. The flipped stack sits over the page header's add button: in `secrets.e2e.ts` the "Agent
secret added." toast was still there 30 s later and took the click on "Add team secret" (#954);
#349 was the same over the Notes tabs. A person whose mouse rests there meets it too.

## Decision

- A toast pauses its close timer on `pointermove` on it (and on `focusin`, as before): the person
  moved the pointer onto it. A pointer it appeared or moved under does not pause it.
- Leaving it (`pointerleave`, `focusout`) arms the timer again as before (at most 4 s).

## Alternatives

- Check `:hover` when the toast is created and pause only if it is not hovered then: misses a
  toast the flip or the host move brings under the pointer after it was created.
- Wait the toast out in the test before clicking the header (the issue's fallback): fixes one test,
  leaves the toast covering the header for a person and for the next test that clicks there.
- Make toasts `pointer-events: none`: loses Undo, Dismiss and the WCAG 2.2.1 pause.

## Consequences

- A toast under a resting pointer closes on its timeout; moving the pointer even a pixel on it
  keeps it, as reading it with the mouse does.
- On a touch screen a tap that does not drag no longer pauses a toast before it acts; its buttons
  still take the tap.
- The tests' workarounds for #349 and #503 (`browserask=0`, waiting a toast out in
  `notes.e2e.ts`) are no longer needed for this reason; they stay, as they also keep the sign-in
  toast off what those tests point at.
