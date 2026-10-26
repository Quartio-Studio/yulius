---
yulius-task: 24cd2813-d92c-49ab-9cbf-5c9d5fecfa39
status: in-progress
---

# SPEC-321: CI/CD run view draws no focus frame when focused by script

Status: in-progress
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

- [ ] Spec SPEC-321 written and validated
- [ ] Regression e2e test fails on dev (CI and deploy views)
- [ ] Run view drops the ring when focused by script
- [ ] Docs, changes entry and gates green
- [ ] Independent review answered
- [ ] PR merged into dev on green

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
