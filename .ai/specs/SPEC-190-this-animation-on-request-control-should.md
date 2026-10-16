---
yulius-task: 18858dea-d13c-42ea-94be-1972a84a9669
status: in-progress
---

# SPEC-190: The watched task's Working line moves like Claude Code's

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #175
Scope authorization: task #175: "this animation on request-control should be better", with a
screenshot of the `✻ Working…` line of a watched task (someone else has control and the viewer may
Request control, SPEC-179).

## Goal

Someone watching another person's task can tell at a glance that the agent is working and for how
long: the `Working…` line at the bottom of the live transcript moves as Claude Code's own spinner
does (a turning mark and a light that runs through the word) and counts the time worked, instead of
standing still.

## Context

- **Where.** `crates/yulius-server/web/assets/watchview.js` draws the line under the transcript
  while the task is `running` and its activity `working` (SPEC-179 WAT-003, SPEC-185 kept the view
  at the bottom through it). CSS: `.watch-working`, `.watch-working-text` in `styles.css`.
- **Why it looks still (measured in the page).**
  - The mark `✻` is a fixed glyph in the dim colour: nothing turns.
  - The shimmer runs over `.ss-line-body`, which is `flex: 1`: its background spans the whole line
    (about 900 px on a desktop) while the words take about 70 px, so the light crosses the words
    for a few percent of each cycle.
  - The shared `shimmer` keyframes (`background-position` -150% → 250% with a 300% background)
    move the light over the element for only about 1/8 of the 1.6 s cycle even when the element is
    the words' width. Together: a flash about every 1.6 s, otherwise grey text (the screenshot).
- **Claude Code's spinner**, which the transcript imitates (it prints the session as the TUI does,
  SPEC-075): the mark turns through `· ✢ ✳ ✶ ✻ ✽` and back, a brighter band runs through the verb
  continuously, and the elapsed time follows in parentheses (`✻ Working… (12s · esc to
  interrupt)`). A watcher cannot interrupt, so only the time applies.
- **Time.** The task summary carries `working_since` (SPEC-087 TIM-002: the start of the working
  stretch, the same measure as the sidebar's "working 4m") and `state_since`.
- **Palette.** The brand has no accent hue (`tokens.css`): the line stays in the session's dim and
  foreground colours, which pass AA in both themes (SPEC-179 AC-WAT-4).
- Under reduced motion every animation stops (the shimmer rule exists already).

## Requirements

- **WRK-001 The mark turns.** While the line shows, its mark steps through Claude Code's frames
  `· ✢ ✳ ✶ ✻ ✽` and back (12 frames, 120 ms each), one character wide, in CSS (no timer). Under
  reduced motion it stands still on `✻`.
- **WRK-002 The light runs through the word.** The shimmer covers the words only (`Working…`, not
  the rest of the line), and its brightest point is over the words for at least 60 % of each
  cycle, moving left to right, continuously. Under reduced motion the words are plain dim text.
- **WRK-003 The time worked.** After the words the line shows the time the agent has worked, dim,
  in parentheses: `(12s)`, `(4m 5s)`, `(1h 20m)`, counted from `working_since` (else
  `state_since`), updated every second while the line is shown. A task without either shows no time.
  A clock behind the server's never shows a negative time (`0s`). The second's tick runs only while
  the line is shown and the view exists: hiding the line or leaving the task stops it.
- **WRK-004 Nothing else changes.** The line still shows exactly while the task runs and works, the
  view still follows the bottom through it (SPEC-185), and the transcript keeps no WCAG violations.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The Working mark turns through Claude Code's spinner frames
- [ ] The Working shimmer sweeps across the words the whole cycle
- [ ] The Working line counts how long the agent has worked
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-WRK-1.** e2e: on a watched working task the mark has a running animation of 12 steps; at
  each twelfth of its cycle the visible frame is the next of `·✢✳✶✻✽✽✻✶✳✢·`; the mark is one
  character wide. Fails before the change (no animation).
- **AC-WRK-2.** e2e: sampling the words' animation over its cycle, the brightest point of the
  background is within the words' own box in at least 60 % of the samples and moves left to right.
  Fails before the change (under 10 %).
- **AC-WRK-3.** e2e: a task working since 75 s ago reads `Working… (1m 15s)` (± the seconds the
  test takes) and the count goes up without a task event; `workingFor()` formats 0, 59 s, 61 s,
  59 min 59 s, 1 h 20 min and a negative time as `0s`, `59s`, `1m 1s`, `59m 59s`, `1h 20m`, `0s`;
  a task without `working_since` and `state_since` shows no time. Fails before the change.
- **AC-WRK-4.** e2e: the reduced-motion rules stop the mark's and the words' animations (the harness
  cannot emulate the media query, as in `sessions.e2e.ts`), and the still mark is `✻`.
- **AC-WRK-5.** The existing `watching.e2e.ts` tests (following through the Working line, colours,
  WCAG) pass unchanged.

## Test plan

- `e2e/tests/watching.e2e.ts`: AC-WRK-1..4, each written first and seen failing; the file's
  existing tests for AC-WRK-5.
- Gates: `scripts/dev ci`, `scripts/dev security`, the full e2e suite.
- Visual check in Chromium (dark and light) of the line in a watched task.

## Decisions

- **Claude Code's spinner, not a new design (ADR-0153).** The transcript already imitates the TUI
  (`⏺`, `⎿`, `>`), so the line that stands for its spinner moves as that spinner does. Alternatives:
  a pulsing dot or a progress bar (a second visual language beside the TUI's), only fixing the
  shimmer (the mark still stands still and nothing says how long).
- **The mark turns in CSS, as a strip of frames moved by `steps()`.** No timer to bound or clear,
  and every browser animates `transform`. Alternatives: a JS interval swapping the glyph (a second
  timer per view for decoration), animating `content` (not animated in every browser).
- **The words shimmer on their own box with their own keyframes.** The shared `shimmer` keyframes
  suit loading boxes (a flash, then rest) and stay as they are for them; the words get
  `working-sweep`, whose light starts just left of the words and ends just right of them.
- **The time is `working_since`.** The same measure as the sidebar and Tasks' "working 4m", so the
  two never disagree; finer (seconds) because the line is about the moment. A one-second tick
  through the view's injected timer, scheduled only while the line shows.

## Evidence

To be filled in when the work is done.
