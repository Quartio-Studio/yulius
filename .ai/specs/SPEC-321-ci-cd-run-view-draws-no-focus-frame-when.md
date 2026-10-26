---
yulius-task: 24cd2813-d92c-49ab-9cbf-5c9d5fecfa39
status: done
---

# SPEC-321: CI/CD run view draws no focus frame when focused by script

Status: done
Date: 2026-10-26
Owner: Yulius task #306
Scope authorization: task #306 (`/goal`), fixes #777. Amends the focus behavior of
[SPEC-238](SPEC-238-we-need-some-other-gui-for-ci-so-we-can.md) (CRV-001: opening a run takes the
keyboard to its view).

## Goal

A CI run or deploy view that the page focuses by script (so the keyboard goes with the run that
opened) draws no focus frame around its content; controls inside the view keep their ring when the
keyboard reaches them.

## Context

- **Symptom.** Open `/?mock=1#/s/studio/ci?run=ci_s2` (or press Enter on a run): a 2 px light
  frame surrounds "All runs", the stages and the log, touching them. The deploy view
  (`?section=cd&run=cd_s2`) does the same.
- **Code path.** `runView()` (`crates/yulius-server/web/assets/runview.js`) builds
  `section.run-view` with `tabindex="-1"`; `focusIfLost` focuses it, called from `pages/ci.js` and
  `pages/cd.js`. Chromium matches `:focus-visible` on a script focus when the last interaction was
  the keyboard or there was none (a page load).
- **Root cause.** `styles.css` draws every focus ring as the global
  `:focus-visible { outline: 2px solid transparent; outline-offset: 2px; box-shadow: var(--focus); }`.
  `.run-view:focus { outline: none; }` (SPEC-238, #713) meant "no indicator on the region" but
  cancels only the outline, which is transparent anyway; the visible part, the `box-shadow`, stays.
  A box-shadow has no offset, so it hugs the section's border box, and the section has no padding.
- **Why tests missed it.** `ci.e2e.ts` asserts that the region gets focus and that axe finds no
  violation; nothing reads the region's computed focus style. axe does not judge focus indicators.
- **Same pattern elsewhere.** The stylesheet already cancels the global ring with
  `box-shadow: none` where another element shows focus or none is wanted (`.md-text`,
  `.composer-prompt`, `.tl-link`). No other `tabindex="-1"` region in the stylesheet cancels only
  the outline.

## Requirements

- RVF-001: The run view (`.run-view`, CI run and deploy alike) focused by script draws no focus
  indicator: neither outline nor box-shadow.
- RVF-002: A control inside the view reached by the keyboard (Tab) keeps the global focus ring.
- RVF-003: The view stays the script focus target (SPEC-238 CRV-001 unchanged) and the page has no
  axe violations.

## Plan

- [x] Spec SPEC-321 written and validated
- [x] Regression e2e test fails on dev (CI and deploy views)
- [x] Run view drops the ring when focused by script
- [x] Docs, changes entry and gates green
- [x] Independent review answered
- [x] PR merged into dev on green

## Acceptance criteria

- AC-1 (RVF-001): opening `ci?run=ci_s2` and `cd?run=cd_s2`, the focused `.run-view` has computed
  `box-shadow: none` and `outline-style: none`; the test fails on dev 91785a67.
- AC-2 (RVF-002): Tab from the focused view moves to "All runs", whose computed `box-shadow` is the
  ring (not `none`).
- AC-3 (RVF-003): `document.activeElement` is the region after opening, and `axeViolations` is
  empty.

## Test plan

- e2e (`e2e/tests/ci.e2e.ts`): `a run view focused by script draws no frame; its controls keep
  their ring` opens the CI run and the deploy run by address and checks AC-1 to AC-3; written first
  and run against the unfixed stylesheet.
- The whole e2e suite (`npx e2e run`) and `scripts/dev ci` before delivery.

## Decisions

- **No ring on the region (chosen)** over a ring with an offset. The section is a focus target
  outside the Tab order, not a control: focusing it only moves the keyboard and the screen reader
  with the run (announced by its name), and the person cannot act on it. A ring around the whole
  page area tells nothing a ring on a control would, and the original rule (`outline: none`)
  already meant no indicator. An offset ring would also need room the layout does not have (the
  view fills its column; a negative offset would overlap content). This follows the existing
  `box-shadow: none` exceptions, and because CI and CD share `runView()` one rule fixes both.
- Selector stays `.run-view:focus` (not `:focus-visible`): both declarations apply to every focus
  of the region, pointer or keyboard; controls inside are other elements and keep the global rule.
- No ADR: a stylesheet exception in the line of existing ones, recorded here.

## Evidence

- Red: `npx e2e run tests/ci.e2e.ts` at 958bc1d9 (test only, dev's stylesheet): the new test fails,
  `last: {"label":"CI run","boxShadow":"rgb(16, 15, 14) 0px 0px 0px 2px, rgb(233, 228, 218) 0px 0px 0px 4px",…}`;
  the other 18 pass. After review round 1, in a scratch copy with dev 91785a67's `styles.css`,
  the CI half and a deploy-only variant each fail with `visible: true` and the `--focus` ring, and
  both pass with the fixed `styles.css`.
- Green: `npx e2e run tests/ci.e2e.ts tests/cd.e2e.ts` with the fix: 38 passed.
- Full e2e suite (`npx e2e run`) at df9b9d3c: 66 files passed, 1 skipped; 713 tests passed, 1
  skipped (the agent-step file, no model key).
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`): exit 0, 2619 nextest tests, doctests,
  lint, deps, macOS compile.
- `scripts/dev security` at df9b9d3c: **unrun**, exit 2: Vidoc answered HTTP 429 "This API key has
  used its 50,000,000 tokens" (`styles.css`, `ci.e2e.ts` not scanned). Never counted as passed;
  the pull request's CI `security` job runs the live scan.
- Review round 1 (fresh session, head 3c3dc83a): approve; one minor (the test did not prove the
  `:focus-visible` trigger, and its deploy half was not shown red) and three nits (the deploy pass
  waited on the CI log, Plan boxes unticked, README wording), all own code, fixed in df9b9d3c and
  this commit. Pre-existing, out of scope: headings, popovers and the hygiene card focused by script
  still get the ring, a real indicator with a gap on a bordered element, not this bug's frame.
