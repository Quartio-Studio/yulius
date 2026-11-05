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

- While the primary pointer button is pressed inside the bar, a render of it is only marked
  stale. The release (`pointerup` anywhere in the window) ends the press, and so does whatever may
  swallow the release: `pointercancel`, `contextmenu`, a `pointermove` with the button up, the
  window losing focus. A right press holds nothing.
- The stale render runs a task later, after a mouse's click (dispatched in the release's task) has
  acted. A tap's click comes with its gesture, later, and is hit-tested on the rebuilt bar, whose
  buttons stay in place for the same state; it is not waited for.
- Reply for a question in the terminal is one button kept across renders, as the field Reply
  (SPEC-156) already was, so the focus guard of #506 keeps the keyboard on it.

## Alternatives

- Diffing the bar's children against the last render: a reconciler for one bar of seven states
  whose buttons close over the current task; more code for the same guarantee.
- A kept node for every button of the bar: fixes focus, but Chromium still loses a click whose
  button is taken out of the page between press and release; every new button would need it too.
- Rendering only when the bar's words change: most handlers read the current task, but some
  close over values of their render (a browser request's tab, a request's link, the start chip's
  task), which would go stale on an unchanged bar.
- Waiting for a tap's click too: needs a fallback timer for a tap that never clicks, for a case the
  rebuilt bar already gets right.

## Consequences

- An update reaches the bar at most a click's length late; the rest of the strip renders at once.
- A click during a hold acts with the task as it is at the release, on a button drawn for the
  earlier state (Hand over after the request was withdrawn): the server checks every such action.
- A new button in the bar is covered without more code; a new floating card that rebuilds on
  updates should hold its renders the same way.
