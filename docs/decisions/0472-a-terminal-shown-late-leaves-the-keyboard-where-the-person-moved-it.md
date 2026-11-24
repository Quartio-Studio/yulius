# 0472. A terminal shown late leaves the keyboard where the person moved it

## Status

Accepted for SPEC-641 (task #625). Refines SPEC-056 WUI-102 and SPEC-052.

## Context

A task view asks its terminal for the keyboard when it opens. Until the terminal's first state
and screen are drawn, that request waits; `reveal()` then focuses the terminal unless another
terminal was asked for since (WUI-102) or the person is typing in a text field (SPEC-052).
Buttons, links and tabs did not hold it back. On a slow host the dev gate's e2e test
`browser.e2e.ts › expired clipboard actions preserve focus…` failed (Yulius CI run be25f19b): the
person (the test) had focused a toast's "Copy to my clipboard", the terminal showed afterwards and
took the keyboard, and the toast, now unfocused, closed 4 s later (ADR-0237). Any control reached
before the terminal shows (a toast action, a tools rail button, a header button) loses the
keyboard the same way, which surprises keyboard and screen reader users (WCAG 3.2.1, 2.4.3).

## Decision

A terminal records where the keyboard was when it was asked for. At the reveal, the deferred
focus lands only if the keyboard is still there, on the page itself, or in a terminal's own input;
otherwise the person moved on and the keyboard stays where they put it. The WUI-102 order and the
SPEC-052 text-field rule still apply on top. A request made after the terminal showed focuses at
once, as before.

## Alternatives

- Make the test wait for the terminal before focusing: the test would pass, and people would keep
  losing the keyboard to a late terminal.
- Hold back only for focus inside toasts and dialogs: menus, tool buttons and header controls
  lose it the same way; a list of places to protect would keep growing.
- Count only focus moved by a user gesture (pointer or key): focus moved by assistive technology
  or a script acting for the person (as the e2e harness does) would still be taken.

## Consequences

Opening a task by a link or button and waiting still gives its terminal the keyboard (that control
keeps focus until the reveal, or the page re-renders it away to the body). Clicking a tool button or
Show tools before a slow terminal shows now leaves the keyboard on that button; the person clicks
the terminal or presses its shortcut to type there. The clipboard test orders the terminal's first
screen explicitly with `__yuliusHoldTerminal`, so it no longer depends on host speed.
