---
yulius-task: 91b96ef6-4cfd-4eac-98cd-1ee02e019526
status: done
---

# SPEC-259: Run summary padding (#776)

Status: done
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
  loading, at 1280 and 390 px wide (on a phone a writer's head wraps and its last button, "In
  Pipeline", starts its own line at the left; a reader's shorter head may fit on one line).
- PAD-002. The summary keeps sticking to the top of the page while the stages and the log scroll
  (SPEC-238 CRV-003), and its loading shape keeps the loaded summary's box (SPEC-120), also its
  height on a phone for an ended run without a reason, for writers and readers. A reason's length
  and a running run's longer facts line ("Now: …") are unknown while loading, so such a run may
  still grow when it loads.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The new `ci.e2e.ts` test "a run’s summary keeps its content off the edges of its card" fails on
  `dev` (insets 0) and passes with the fix.
- `ci.e2e.ts`, `cd.e2e.ts` and `loading-shapes.e2e.ts` pass (sticky summary, deploy view, loading
  shape).

## Test plan

- Regression: the new e2e test measures, in the page, the leftmost and rightmost head items' and
  the facts' insets inside the summary's border, at 1280 and 390 px, for a writer's run (two
  buttons) and a reader's (one, `sandbox-3`), each a page load of its own: first while the run's read is held (`hold=`, the loading shape asserted on screen), then
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

## Evidence

- Reproduced in the mock UI (`python3 -I e2e/serve.py 20105 crates/yulius-server/web`,
  `/?mock=1#/s/studio/ci?run=ci_s2`): badge and In Pipeline flush with the band's edges; after the
  fix both sit 12 px inside the card.
- Regression test, `npx e2e run tests/ci.e2e.ts --grep "off the edges"`: with `origin/dev`'s
  `styles.css` it fails (`{"left":0,"right":0,"facts":0}`); with the card but the old loading head
  it failed at 390 px (`right: 90`, both skeleton buttons wrapped); with the fix it passes.
- Measured loading vs loaded summaries at 1280 and 390 px for `ci_s2`, `ci_s3`, `ci_s4` (a
  Playwright probe in the session scratchpad): 1280 px heights equal (64); at 390 px a running run
  with a long facts line (`ci_s4`) still grows, as before this change.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py --install-rustup`), at 61a03174: exit 0,
  nextest 2395 passed, 17 skipped.
- `npx e2e run` (full suite) at 61a03174: 64 files passed, 1 skipped; 664 tests passed, 1 skipped.
- `scripts/dev security` at 61a03174: exit 0, no open critical/high finding and no medium one in
  the change (3 code files sent, 3 prose skipped).
- Review round 1 (fresh session, request changes): phone loading check never reloaded, loading shape
  wrapped differently on a phone, phone assertion broke with #775's Run again: all fixed in
  61a03174 test-first; dev merged (9db87669). Pre-existing focus ring around the run view filed as
  #777 (outside this change: the global `:focus-visible` box-shadow, not padding).
- Review round 2 (fresh session, approve at b6f67fa9 with minors): PAD-002 narrowed to ended runs;
  the reader case added to the regression test (a loading head ignoring access fails it:
  `toHaveCount` failed); `docs/changes.md` wording fixed; dev merged again.
- Review round 3 (fresh session) approved 6b4f74c8 (after the second dev merge, #779) with nits:
  PAD-001's phone wrap limited to a writer's head and the e2e README row naming both cases (fixed
  here); the deploy view's "Starting" head is flat and becomes the padded card once the deploy
  starts (pre-existing design of SPEC-189, left as is).
- At 6b4f74c8: `scripts/dev ci` exit 0 (its log was lost from the shared scratchpad, only the exit
  code is kept); `npx e2e run` 664 passed, 1 skipped, 1 failed: `cd.e2e.ts` "named environments
  show their stage and deploy by its rules" (from #779). It fails 5 of 5 here also with `origin/dev`'s
  `styles.css` and `pages/ci.js`, so not this change: it reads the sent confirmation once before
  the deploy POST lands, and passes when it polls. Filed as #785 (dev's CI passed it); left out of
  this pull request (one concern).
