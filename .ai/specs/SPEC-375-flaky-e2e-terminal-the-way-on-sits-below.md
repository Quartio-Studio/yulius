---
yulius-task: d9cb29b2-aaee-441e-9c17-c5fa6b2d0f17
status: in-progress
---

# SPEC-375: The held-card test waits for the way on to move below the card

Status: in progress
Date: 2026-11-01
Owner: Yulius task #358
Scope authorization: task #358, Fixes #1003 (opened by task #347, triaged by task #354). Test-only:
builds on SPEC-326 (HIS-002, the way on past the terminal's top sits below the cards over it).

## Goal

`e2e/tests/terminal.e2e.ts` › "the way on sits below a held card over the terminal (#911)" passes
however late the browser lays the page out, and still fails when the button really overlaps the
card.

## Context

**Symptom.** In a full local e2e run on a shared VM (797 tests, 48 min) the test failed once with
`the button is below the card: expected 77 to be greater than or equal to 225`
(`tests/terminal.e2e.ts:1013`) and passed when its file ran alone.

**Root cause.** The button's `top` is `max(var(--term-banner-h) + 12px,
var(--term-overlay-bottom) + 8px)` (`styles.css`, `.term-history-offer`). `--term-overlay-bottom`
is set on `.taskview-term` by a `ResizeObserver` watching the attention bar and the held card
(`taskview.js`, `overlays`). `renderHeld` only un-hides and fills the card; the observer's
callback, which moves the button, runs at the browser's next rendering update. The test waited
for the card and the button to be visible, then read both boxes once: under load the measure
came before that rendering update, so the button still sat at banner + 12 px (77) above the card's
bottom (225).

**Product or test?** Test only. ResizeObserver callbacks run inside the rendering update, after
layout and before paint, and a style change they make is laid out again before that paint
(HTML "update the rendering": "broadcast active resize observations", then "gather active resize
observations at depth" again). So no frame is ever painted with the button over the card; only a
script reading layout between the card being shown and the next rendering update sees the old
place. A person never sees it.

**Why the tests missed it.** On an idle host the rendering update runs within a frame of the
card showing, before the test's next round trip, so the single measure always came late enough.

**What else the cause could break.** Other layout read once right after an element shows, where
the layout depends on a ResizeObserver: in this file only the held-card test reads the offer's
box; the same pattern with the attention bar's `--attention-h` (corner notices below the bar) is
read once in `cd.e2e.ts` › "the longest needs_you request keeps its to-do and buttons in view on a
phone" (checked under the same reproduction, see Evidence).

## Requirements

- FLK-001: The held-card test reproduces the late observer on every run: an init script delays
  every ResizeObserver callback by 500 ms, as a loaded host does.
- FLK-002: The test polls the gap between the card's bottom and the button's top until it is
  0 or more, with the message "the button is below the card", instead of measuring once.
- FLK-003: A button that never moves below the card still fails the test (the poll's timeout).

## Plan

- [ ] Spec for the measure-once race
- [ ] Deterministic reproduction fails the measure-once test
- [ ] The measure polls until the layout settles
- [ ] Review, gates and delivery

## Acceptance criteria

- With the delay and the old single measure, the test fails every run with the issue's message.
- With the delay and the poll, the test passes every run; the whole terminal file passes.
- With the observer removed (a mutation: no `--term-overlay-bottom`), the polled test fails.
- The full e2e suite passes on this shared host.

## Test plan

- `npx e2e run tests/terminal.e2e.ts` before the fix (test commit only) and after, several runs.
- Mutation: comment out `termArea.style.setProperty('--term-overlay-bottom', …)` and run the file.
- `E2E_TELEMETRY_DISABLED=1 npx e2e run` (the whole suite) and `scripts/dev ci`.

## Decisions

- **Keep the delay in the committed test** rather than only in a throwaway probe: the test then
  guards the late-observer case on every run, not only on a loaded host; 500 ms is longer than a
  loaded frame and well inside the poll's budget. Alternative: wait for a non-zero
  `--term-overlay-bottom` first; rejected as it would test the mechanism, not the placement.
- **Poll the placement, not the card's shown state**: the card is already visible before the
  button moves; no product signal says the observer ran, and adding one only for tests would
  change the product for a test-only race.
- No ADR: a test-only change following the existing rule (wait on conditions, never on sleeps).
