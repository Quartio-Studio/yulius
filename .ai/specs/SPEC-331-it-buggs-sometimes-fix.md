---
yulius-task: 384125eb-6392-4239-8993-8e19b004e1c4
status: in-progress
---

# SPEC-331: A hidden terminal resizes its session to 10×5

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #315
Scope authorization: task #315 (`/goal`, "it buggs sometimes. Fix" with a screenshot of the
Superagent), fixes #918. Keeps the size rules of SPEC-002 TSK-006 (F 2026-10-09/10: the viewer's
size travels with the connection; a writer reports the size its window fits).

## Goal

A terminal that is not shown (closed panel, another tool in front, another sandbox's kept view)
reports no size, so it never squeezes its session; showing it again still fits it to its box.

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
- HTS-002: Shown again, the terminal fits its box and sends that size, as before.
- HTS-003: A terminal that connects while hidden connects with its current grid (its last shown
  size, xterm's 80×24 before any), never with the hidden measure.

## Plan

- [x] Spec SPEC-331 written and validated
- [x] Regression e2e test fails on dev: hiding the Superagent or a Shell terminal sends no resize
- [x] A hidden terminal measures nothing and reports no size
- [ ] Docs, changes entry and gates green
- [ ] Independent review answered
- [ ] PR merged into dev on green

## Acceptance criteria

- AC-1 (HTS-001): with the Superagent open, closing the panel and putting the Shell tool in front
  leave its mock session at the size it had while shown (not 10×5). Putting the Superagent back in
  front leaves the Shell's session at its shown size too. The test fails on dev 2393032b.
- AC-2 (HTS-002): reopening the Superagent after it was hidden still fits it: the session takes
  the panel's size again after the panel is maximized and restored.
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
- No ADR: a measurement guard inside an existing component, recorded here.
