---
yulius-task: 384125eb-6392-4239-8993-8e19b004e1c4
status: done
---

# SPEC-331: A hidden terminal resizes its session to 10×5

Status: done
Date: 2026-10-26
Owner: Yulius task #315
Scope authorization: task #315 (`/goal`, "it buggs sometimes. Fix" with a screenshot of the
Superagent), fixes #918. Keeps the size rules of SPEC-002 TSK-006 (F 2026-10-09/10: the viewer's
size travels with the connection; a writer reports the size its window fits).

## Goal

A terminal that is not shown (closed panel, another tool in front, another sandbox's kept view)
measures nothing and sends no resize, so hiding it never squeezes its session; showing it again
fits it to its box and sends that size again.

## Context

- **Symptom.** The screenshot shows the Superagent panel at full width with Claude Code drawn in
  about 10 columns: `for 6s ·`, `done` and `1:11 PM` on separate lines, a 10-cell divider. The
  server clamps a shell's size to at least 10×5 (`resize_shell`, `crates/yulius-app/src/shells.rs`),
  which matches.
- **Reproduced** in mock mode (`?mock=1#/s/studio/agent`), logging the `hello`/`resize` frames the
  page sends to the Superagent's shell session: opening it sends 50×37; closing the panel sends
  **10×5**; putting the Shell tool in front sends 10×5 for the Superagent, and coming back sends
  10×5 for the Shell's terminal.
- **Code path.** `terminal-core.js` observes its `fitBox` with a ResizeObserver; on every change
  `applySize()` asks `ownSize()` → `proposed()` → the fit addon's `proposeDimensions()`, and a size
  different from the last one sent goes to the server (`socket.resize`). `proposeDimensions()`
  reads `getComputedStyle(parent).width/height`; inside a `display: none` ancestor those are the
  *specified* values, `100%`, which `parseInt` turns into 100 px: (100 − 14 scrollbar) / ~8.4 px
  = 10 columns, 100 / ~15 px = 5 rows. `proposed()` only rejects ≤ 2 columns, so the hidden
  measure passes as a real one.
- **Why it stays broken.** Shell terminals (Superagent, Shell) follow the last resize
  (`routes/shells.rs`: no size-control policy, `controller: true` always). Another tab or device
  that hides its copy of the same Superagent shrinks the shared PTY to 10×5. The visible tab never
  sends its size again, because its own size did not change (`lastSent` equals it). In a single
  tab, everything Claude writes while the panel is hidden is wrapped at 10 columns and stays so
  in the scrollback. Task terminals do the same when the hidden view controls the size.
- **Why tests missed it.** No e2e test reads the size the page sends after a terminal is hidden.
  The mock applies any size without the server's clamp, and the terminals look right once shown
  again in the same tab.

## Requirements

- HTS-001: A terminal whose box is not rendered (zero width or height) measures nothing: it sends
  no resize, does not resize its own grid and does not refit another viewer's grid.
- HTS-002: Shown again, the terminal fits its box and sends that size, even when it equals the
  last size it sent (another window of the same shell may have resized the session meanwhile), as
  before.
- HTS-003: A terminal that connects while hidden connects with its current grid (its last shown
  size, xterm's 80×24 before any), never with the hidden measure.

## Plan

- [x] Spec SPEC-331 written and validated
- [x] Regression e2e test fails on dev: hiding the Superagent or a Shell terminal sends no resize
- [x] A hidden terminal measures nothing and reports no size
- [x] Docs and changes entry updated
- [x] Independent review answered
- [x] Local gate (scripts/dev ci) green
- [x] PR merged into dev on green

## Acceptance criteria

- AC-1 (HTS-001): with the Superagent open, closing the panel and putting the Shell tool in front
  leave its mock session at the size it had while shown (not 10×5). Putting the Superagent back in
  front leaves the Shell's session at its shown size too. The test fails on dev 2393032b.
- AC-2 (HTS-002): reopening the Superagent after it was hidden still fits it: the session takes
  the panel's size again after the panel is maximized and restored, and after another window set
  the session to 30×20 while this one was hidden (review round 1; fails on 6499449f).
- AC-3 (HTS-003): covered by the same guard (`proposed()` returns nothing for a hidden box, so
  `getSize` falls back to the grid xterm has); no separate test.

## Test plan

- e2e (`e2e/tests/agent-panel.e2e.ts`): `a hidden Superagent or Shell terminal keeps its session's
  size` (AC-1, AC-2), written first and run against dev's `terminal-core.js`. It waits on frames
  (the observer's callback runs in an animation frame), never on sleeps.
- The whole e2e suite (`npx e2e run`) and `scripts/dev ci` before delivery.

## Decisions

- **Guard the measure in `terminal-core.js` (chosen)** over patching the vendored fit addon (a
  third-party file kept as published) or a server-side minimum (10×5 is a legitimate size for a
  narrow window, and the server cannot tell a hidden measure from a real one). One guard in
  `proposed()` and `room()` covers every terminal: Superagent, Shell, task terminals and their
  mirroring.
- "Not rendered" is `fitBox.clientWidth` or `clientHeight` being 0, which is what a `display: none`
  ancestor or a detached box gives. It costs no extra layout: `ownSize()` already reads both.
- **Size-control policy for shell terminals: not in this change.** Two visible windows of the
  same Superagent still follow the last resize (existing, documented behavior). This change only
  stops hidden views from taking part.
- While hidden, the terminal forgets the size it last sent (`lastSent = null`), so its first
  measure once shown is sent even if unchanged; the server ignores a resize to the size it has.
- Known limit, not changed: `hello` must carry a size, so a view that (re)connects while hidden
  still connects with its current grid (HTS-003). Avoiding that needs a protocol change.
- No ADR: a measurement guard inside an existing component, recorded here.

## Evidence

- Reproduced (mock, Playwright probe logging the frames sent to the mock sessions): opening the
  Superagent sent 50×37, closing the panel sent `resize 10x5`, the Shell tool in front sent 10×5
  for the Superagent, and the Superagent back in front sent 10×5 for the Shell.
- Red: `npx e2e run tests/agent-panel.e2e.ts` at f4d1ea7d (test only, dev's `terminal-core.js`):
  `expected "10x5" to be "50x37"` right after Close panel; 25 others pass.
- Green: the same file at 6499449f: 26 passed.
- Review round 1 (fresh session, head ee6ea736): request changes. One major (own code: a terminal
  shown again no longer sent its size, so a session another window resized stayed so), one minor
  (the spec's Goal overstated the fix) and one nit (`ownSize()` switched fonts before the check).
  All three fixed: the test step at aaf1e6f3 was red (`agent-panel.e2e.ts:346`, poll timed out),
  and a88d5cae made it pass (26 passed). Round 2 (head a88d5cae): approve.
- `terminal.e2e.ts`, `terminals.e2e.ts`, `layout.e2e.ts` and `phone.e2e.ts` at load average
  about 120 on 12 cores: a different task-terminal test timed out on each run (`a read-only viewer
  cannot paste`, `another viewer's size fills this box…`, `file paths in the task terminal open
  the editor…`). Dev's `terminal-core.js` failed in the same file under the same load (`another
  viewer's size fills this box…`), so this is load flakiness (#510 tracks it), not this change.
  The pull request's CI e2e job runs the whole suite.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`) at ee6ea736: exit 0, nextest 2628 passed,
  16 skipped, plus doctests, lint, deps and the macOS compile. The later commits change only
  `terminal-core.js`, the e2e test and the spec.
- `yulius security-scan` at a88d5cae: **unrun**, exit 2, Vidoc HTTP 429 "This API key has used its
  50,000,000 tokens" (`terminal-core.js` and `agent-panel.e2e.ts` not scanned). It is not counted
  as passed; the pull request's CI `security` job runs the scan.
- Not tested: two real browsers on one live Superagent (the mock stands in for the other window by
  setting the session's size), and task terminals' mirroring with a hidden view. They use the same
  guard and have no test of their own.
