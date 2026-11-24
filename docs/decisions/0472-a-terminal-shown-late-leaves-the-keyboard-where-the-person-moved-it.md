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

The obvious rule, "remember which element had the keyboard at the request", was tried and dropped
in SPEC-052. The UI re-creates focused controls and focuses the new node: choosing a Shell tab
rebuilds the tab strip, and a count change repaints the sandbox tab row through `keepFocus`. A
node comparison reads both as the person moving on. SPEC-641's first fix fell into the same trap
(review round 1).

## Decision

`dom.js` counts the person's keyboard moves across the page. A move is focus reaching another
control while the control focused before it is still on the page, or with nothing focused before.
A control focused after its predecessor left the page is a re-render, so it is not a move.
Re-focusing the same control (after a window switch) is not a move either, and neither is focus
entering a terminal's own input: which terminal has the keyboard follows their requests.

A control that leaves the page with nothing re-created in its place must not hide the next move.
The control that opened a task view often goes with its page: the composer's prompt, Go to task, a
chain chip, a toast's "Open #N". So reading the count forgets a control that has already left the
page while the keyboard is on the page, and the person's next focus counts.

A terminal focus request stores the count. The lazy wrapper in `terminal.js` stores it when the
request comes before the terminal's code has loaded. At the reveal, the deferred focus lands only
if the count is unchanged or the keyboard is on the page itself. The WUI-102 order and the SPEC-052
text-field rule still apply on top. A request made after the terminal showed focuses at once, as
before.

## Alternatives

- Make the test wait for the terminal before focusing: the test would pass, and people would keep
  losing the keyboard to a late terminal.
- Compare the focused node at the request and at the reveal: rejected in SPEC-052, and it broke
  Shell tabs and the sandbox tab row again in SPEC-641's round 1.
- Mark each place where the app restores focus (`keepFocus`, every tab strip) and count everything
  else: each unmarked site would bring the same regression back.
- Count only moves after a pointer or key event: assistive technology and scripts acting for the
  person (as the e2e harness does) focus without one.
- Hold back only for focus inside toasts and dialogs: menus, tool buttons and header controls lose
  it the same way.

## Consequences

Opening a task by a link or button and waiting still gives its terminal the keyboard, also when
that control is repainted meanwhile, and a fork opened from a menu gets it when its dialog closes.
A Shell tab chosen while its terminal loads gets the keyboard when it shows. Focusing a toast action, a tool button or Show tools before a slow terminal shows now
leaves the keyboard there; the person clicks the terminal or presses its shortcut to type there.

A request made after the person's move, because the view opened later, still takes the keyboard as
the terminal asked for last.

One gap remains: the focused control leaves the page after the request, with nothing focused in its
place, and the person then focuses another control before the terminal shows. That move follows a
control that left the page, so it is not counted, and the terminal takes the keyboard as on dev.

The clipboard tests hold the terminal's first screen (`__yuliusHoldTerminal`) or its code (a held
`terminal-core.js` request), so they no longer depend on host speed.
