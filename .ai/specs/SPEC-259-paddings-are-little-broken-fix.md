---
yulius-task: 91b96ef6-4cfd-4eac-98cd-1ee02e019526
status: in-progress
---

# SPEC-259: Run summary padding (#776)

Status: in-progress
Date: 2026-10-21
Owner: Yulius task #245
Scope authorization: task #245 ("paddings are little broken, fix", with a screenshot of a CI run's
view). Fixes #776.

## Goal

The summary on top of a CI run's or a deploy's view (SPEC-238) keeps its content off the edges of
the band it is drawn on.

## Context

- Symptom (the person's screenshot, reproduced in the mock UI at `/s/studio/ci?run=ci_s2`): the
  "Passed"/"Failed" badge and the facts line start exactly at the band's left edge, and "In
  Pipeline" ends exactly at its right edge.
- Root cause: `.run-summary` (`styles.css`, SPEC-238) is sticky, so it paints an opaque
  `background: var(--bg)`; the page under it paints `var(--light), var(--texture)` on the same
  colour, so the summary reads as a flat band of its own. Its `padding: 8px 0` gives that band no
  inline padding. The loading shape (`runLoading` in `pages/ci.js`) uses the same class, so it has
  the same defect; the deploy view (`pages/cd.js`) shares `runView` and the class too.
- Since #775 (SPEC-258) the loaded head has two buttons for writers (Run again or Cancel, then In
  Pipeline) while the loading shape had one and a fourth bar for a reason; with the card's padding
  the shape no longer wrapped as the loaded head does on a phone (review round 1).
- Why tests missed it: `ci.e2e.ts` checks the summary's text and that it sticks, and
  `loading-shapes.e2e.ts` checks that the loading shape matches the loaded box; nothing measured
  the content's inset inside the band.
- The CI/CD page's own "Now" line (`.ci-now`) is the precedent: `padding: 8px 12px` in a bordered
  `var(--surface)` card with `var(--radius)`.

## Requirements

- PAD-001. The summary's head items and its facts are inset 12 px from the inside of the summary's
  card: the leftmost and rightmost head items and the facts sit exactly 12 px in, loaded and
  loading, at 1280 and 390 px wide (on a phone the head wraps and the last button, "In Pipeline",
  starts its own line at the left).
- PAD-002. The summary keeps sticking to the top of the page while the stages and the log scroll
  (SPEC-238 CRV-003), and its loading shape keeps the loaded summary's box (SPEC-120), also its
  height on a phone for a run without a reason (a reason's length is unknown while loading).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The new `ci.e2e.ts` test "a run’s summary keeps its content off the edges of its band" fails on
  `dev` (insets 0) and passes with the fix.
- `ci.e2e.ts`, `cd.e2e.ts` and `loading-shapes.e2e.ts` pass (sticky summary, deploy view, loading
  shape).

## Test plan

- Regression: the new e2e test measures, in the page, the leftmost and rightmost head items' and
  the facts' insets inside the summary's border, at 1280 and 390 px, each width a page load of its
  own: first while the run's read is held (`hold=`, the loading shape asserted on screen), then
  loaded, with the loaded card's height equal to the loading shape's.
- Existing: the full e2e suite and `scripts/dev ci`.

## Decisions

- The summary becomes a card like `.ci-now` (`padding: 8px 12px`, a 1 px `var(--border)` border,
  `var(--radius)`, `var(--surface)`) rather than a band bleeding into the page's gutter with negative
  margins: it then lines up with the log box and the "Now" card under and above it, needs no
  gutter-specific margins on phones (12 px gutter there), and reads as one deliberate surface. Its
  rounded top corners at the sticky position leave a 4 px corner where scrolled content can show;
  accepted as negligible. Too small and reversible for an ADR.
- The loading head drops its reason bar and gets the loaded head's buttons (two for writers, one
  for readers): it then wraps as an ordinary run's head on a phone. Bars cannot predict a reason's
  length, so a run with a reason may still grow when it loads.
