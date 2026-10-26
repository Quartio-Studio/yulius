---
yulius-task: b30c147b-ba4a-46d2-a5ec-328e37ebe201
status: in-progress
---

# SPEC-324: Status panel section summaries fit the panel at phone width

Status: in-progress
Date: 2026-10-26
Owner: Yulius task #305
Scope authorization: task #305 (`/goal`), fixes #549. Amends the Status panel of UI-007
(a7ca9fae); [SPEC-172](SPEC-172-in-every-subscribtion-view-we-want-both.md) lists #549 as out
of its scope.

## Goal

At a 360 px window every collapsible Status panel section header (Sandboxes, Subscriptions) fits
inside the panel and its whole summary stays readable, so "SUBSCRIPTIONS 4 ready · 1 at the
limit · 1 need attention" is no longer cut off at "…1 need attentio".

## Context

- **Symptom.** In mock mode (`/?mock=1#/s/studio/git`, Status, 360×780) the Subscriptions summary
  runs past the panel's right edge and is clipped by the panel's `overflow: auto`.
- **Root cause.** `statusSection` (`crates/yulius-server/web/assets/layout.js`) puts the summary
  in a `span.status-summary-text` beside the `h2` in the `summary` row. The row is `display: flex`
  without wrap, the `h2` is `flex: none`, and the span is `white-space: nowrap` without
  `min-width: 0` (`styles.css`): it can neither shrink nor wrap. At 360 px the panel has about
  310 px of content width; chevron, icon, "SUBSCRIPTIONS" and the gaps take about 140 px, the
  three-part summary about 230 px.
- **Why tests missed it.** The SPEC-172 phone test checks the account rows' widths, not the
  section heads; no test measured a `summary`.
- **What else the cause could break.** Any `statusSection` summary that gets longer (the
  Sandboxes "N running in M of K" with large counts) hits the same rule.

## Requirements

- SSF-001: At 360 px no Status section head (`.status-section`, its `summary`, the summary text)
  is wider than it is shown (`scrollWidth <= clientWidth`), and the summary text ends inside the
  panel.
- SSF-002: The whole summary text stays readable without hover: it wraps onto a further line,
  right-aligned under the first, breaking between its parts (` · `) before inside one.
- SSF-003: The open Status panel at 360 px has no WCAG 2.x A/AA violations (axe).
- SSF-004: At desktop width the head is unchanged: one line, summary at the right.

## Plan

- [x] Spec SPEC-324 written and valid
- [ ] Regression e2e: Status section heads fit at 360 px (fails on dev)
- [ ] Section summaries wrap inside the panel
- [ ] Docs, changes entry and gates green
- [ ] Independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- AC-1 (SSF-001, SSF-003): the e2e test at 360×780 with the Status panel open finds every section
  head within its width and the summary text inside the panel, and no axe violations; it fails on
  dev 91785a67.
- AC-2 (SSF-002): the Subscriptions summary's text is the full "4 ready · 1 at the limit · 1 need
  attention", shown on more than one line, each part whole on its line.
- AC-3 (SSF-004): at 1440 px the summary is one line.

## Test plan

- e2e (`e2e/tests/subscriptions.e2e.ts`, next to the SPEC-172 phone test): written first and run
  against the unfixed CSS, then after the fix.
- The full e2e suite and `scripts/dev ci` (the gate) after the fix.
- Not tested: real phones' fonts; the check measures Chromium's layout with the bundled font stack.

## Decisions

- **Wrap, not ellipsis.** The panel at 360 px is a phone, where a `title` tooltip cannot be
  reached (no hover), and an ellipsis would cut the last part, "1 need attention", which is the one
  that asks for a person. Wrapping keeps every word visible at the cost of one more line in the
  head. Each part is kept whole on a line (`white-space: nowrap` per part), so the summary breaks
  at its ` · ` separators and reads as a list. Alternative rejected: ellipsis with `title`
  (the hint in #549): it hides the most important part on exactly the devices that hit it.
- **No `title` on the summary.** Nothing is hidden any more, so a tooltip would only repeat the
  visible text.
