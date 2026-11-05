# 0268. A pressed attention bar holds its render until the press ends

## Status

Accepted. Specified in [SPEC-424](../../.ai/specs/SPEC-424-sometimes-i-cant-click-reply.md);
task #406.

## Context

A task's attention bar (UX-03) rebuilds its buttons on every render, and the task view renders it
on every task update, detail read, steps event, pull request and browser request; a running task
is updated every few seconds. A browser fires `click` on the nearest common ancestor of the press
and the release, so a button replaced (or taken out and put back) between the two lost its click
to the bar: "sometimes I can't click Reply". The bar's terminal Reply was also a new button each
time, so the keyboard on it fell to `body` on the next update.

## Decision

- While a pointer is pressed inside the bar, a render of it is only marked stale; the release
  (`pointerup`, `pointercancel` anywhere in the window) or the window losing focus ends the press,
  and the stale render runs a task later, after the release's click has acted.
- Reply for a question in the terminal is one button kept across renders, as the field Reply
  (SPEC-156) already was, so the focus guard of #506 keeps the keyboard on it.

## Alternatives

- Diffing the bar's children against the last render: a reconciler for one bar of seven states
  whose buttons close over the current task; more code for the same guarantee.
- A kept node for every button of the bar: fixes focus, but Chromium still loses a click whose
  button is taken out of the page between press and release; every new button would need it too.
- Rendering only when the bar's words change: its buttons' handlers close over the task, so an
  unchanged bar would act on a stale task.

## Consequences

- An update reaches the bar at most a click's length late; the rest of the strip renders at once.
- A new button in the bar is covered without more code; a new floating card that rebuilds on
  updates should hold its renders the same way.
