---
yulius-task: d9cb29b2-aaee-441e-9c17-c5fa6b2d0f17
status: done
---

# SPEC-375: The held-card test waits for the way on to move below the card

Status: implemented
Date: 2026-11-01
Owner: Yulius task #358
Scope authorization: task #358, Fixes #1003 (opened by task #347, triaged by task #354). Test-only:
builds on SPEC-326 (HIS-002, the way on past the terminal's top sits below the cards over it).

## Goal

`e2e/tests/terminal.e2e.ts` › "the way on sits below a held card over the terminal (#911)" passes
however late the browser lays the page out, and still fails when the button really overlaps the
card. The same holds for the one other test that measured an observer's layout once,
`e2e/tests/cd.e2e.ts` › "the longest needs_you request keeps its to-do and buttons in view on a
phone".

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
phone": under the same reproduction it fails (`cornerBelowBar: false`), so it is fixed here too
(FLK-004). The other observer-driven checks of the suite already poll (watch view follow,
strip fit in `responsive.e2e.ts`, splitter `aria-valuenow`), found by the independent review.

## Requirements

- FLK-001: The held-card test reproduces the late observer on every run: the shared helper
  `lateResizeObservers` (`e2e/tests/support.ts`, an init script) delays every ResizeObserver
  callback by `LATE_OBSERVER_MS` (2 s), as a loaded host does, and drops entries of targets
  unobserved or disconnected meanwhile, as a real observer would.
- FLK-002: The test polls the gap between the card's bottom and the button's top until it is
  0 or more, with the message "the button is below the card", instead of measuring once.
- FLK-003: A button that never moves below the card still fails the test (the poll's timeout,
  `LATE_OBSERVER_MS` + 5 s, so the artificial delay does not eat the poll's usual budget).
- FLK-004: The phone needs_you test runs with the same helper and polls its whole placement
  (to-do and buttons in view, the corner below the bar) with the message "the to-do, the buttons
  and the corner are placed", instead of measuring once.

## Plan

- [x] Spec for the measure-once race
- [x] Deterministic reproduction fails the measure-once test
- [x] The measure polls until the layout settles
- [x] Review, gates and delivery

## Acceptance criteria

- With the delay and the old single measure, the held-card test fails every run with the issue's
  message, and the phone test with `cornerBelowBar: false`.
- With the delay and the poll, both tests pass; the whole terminal and cd files pass.
- With the observer removed (a mutation: no `--term-overlay-bottom`), the polled test fails.
- The full e2e suite passes on this shared host, apart from failures already filed (#1002).

## Test plan

- `npx e2e run tests/terminal.e2e.ts` before the fix (test commit only) and after, several runs.
- `npx e2e run tests/cd.e2e.ts` with the polls swapped back to single measures, and with the polls.
- Mutation: comment out `termArea.style.setProperty('--term-overlay-bottom', …)` and run the file.
- `E2E_TELEMETRY_DISABLED=1 npx e2e run` (the whole suite) and `scripts/dev ci`.

## Decisions

- **Keep the delay in the committed tests** rather than only in a throwaway probe: the tests then
  guard the late-observer case on every run, not only on a loaded host. **2 s, not 500 ms**: at
  500 ms the phone test's measure still came after the observer in 2 of 3 runs (its waits can take
  longer than that); 2 s outlasts both tests' waits (6 of 6 failing runs) and the polls get it on
  top of their 5 s. Alternative: wait for a non-zero
  `--term-overlay-bottom` first; rejected as it would test the mechanism, not the placement.
- **Poll the placement, not the card's shown state**: the card is already visible before the
  button moves; no product signal says the observer ran, and adding one only for tests would
  change the product for a test-only race.
- No ADR: a test-only change following the existing rule (wait on conditions, never on sleeps).

## Evidence

Host: the shared VM, load average 27-65 during the runs. Logs kept in the session's scratchpad.

- Reproduction before the fix (`da9f8138`, 500 ms): `npx e2e run tests/terminal.e2e.ts` → 1
  failed, 28 passed: `ASSERTION_FAILED: the button is below the card: expected 77 to be greater
  than or equal to 225`, the issue's exact signature.
- Phone test, single measure, delay: 500 ms failed 1 of 3 runs; 2 s failed 2 of 2
  (`"cornerBelowBar":false`).
- Final helper (`e90e8bab`), polls swapped back to single measures (not committed):
  terminal.e2e.ts 1 failed (`expected 77 to be greater than or equal to 225`), cd.e2e.ts 1 failed
  (`cornerBelowBar: false`). With the polls: terminal.e2e.ts 29 passed, cd.e2e.ts 22 passed.
- Mutation (`--term-overlay-bottom` never set in `taskview.js`, not committed): the held-card test
  fails, `expect.poll(...).toBeGreaterThanOrEqual(...) timed out`, "the button is below the card".
- Full suite at `8e15cfdb` on the loaded host (load average 36 rising to 65, beside the gate):
  `E2E_TELEMETRY_DISABLED=1 npx e2e run` → 796 passed, 1 skipped, 1 failed in 40 min 43 s;
  terminal.e2e.ts (29) and cd.e2e.ts (22) passed. The failure is
  `vidoc-switch.e2e.ts` › "a refresh that overtakes the read after a change still shows the
  change" (`expected 3 to be 1`), which also fails with its file run alone; it is the open issue
  #1002, untouched by this change.
- `scripts/dev ci` at `8e15cfdb`: exit 0. The later commits change e2e tests and docs only.
- `yulius security-scan`: Vidoc is disabled for this project in Yulius (reported as disabled, not
  passed).
- Independent review (fresh session, om-code-review) of `d7346573..8e15cfdb`: APPROVE with three
  low findings and two nits, all fixed in `e90e8bab` and this spec (helper follows
  observe/unobserve/disconnect, polls get `LATE_OBSERVER_MS` + 5 s, the phone poll's message and
  comment, the spec's 2 s and FLK-004).
