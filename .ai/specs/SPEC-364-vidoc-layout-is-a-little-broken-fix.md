---
yulius-task: 4f38a6d4-308b-46a5-bd63-d9a5e9b28b32
status: in-progress
---

# SPEC-364: The Security scan card's switch sits at the card's right edge

Status: in progress
Date: 2026-11-01
Owner: Yulius task #347
Scope authorization: task #347 ("vidoc layout is a little broken, fix", with a screenshot of a
sandbox's CI/CD page). Fixes #997. Builds on SPEC-359 (the sandbox's Security scan card, VSB-004).

## Goal

On a sandbox's CI/CD page, the Security scan (Vidoc) card lays out like the rest of the app's
setting rows: the label "Scan this project with Vidoc" on the left, its switch at the card's
right edge, the explanatory notes below in a readable line length.

## Context

**Symptom.** In the reported screenshot (a wide window) the card spans the page, but its switch
sits about halfway across it, with an empty right half; the label and the switch look detached
from the card and from the "Check for commits" button and the job list above, which reach the
right edge.

**Root cause.** SPEC-359 added `.vidoc-card .vidoc-project { max-width: 520px; }`
(`crates/yulius-server/web/assets/styles.css`) to the card's label/switch row, copying the
Instance page's Vidoc card, where the row sits in a 520px list of projects inside a settings
column. On the CI/CD page the card is full width, so `justify-content: space-between` puts the
switch at 520px, not at the card's edge. The Settings page's rows (`.settings-row`, a grid
whose last column, the control, is `auto`) put every switch at their right edge.

**Why the tests missed it.** The e2e tests of the card (`e2e/tests/vidoc-switch.e2e.ts`) check
its roles, text, states and axe, never its boxes.

**What else the cause touches.** Only the CI/CD card: the removed selector needs a `.vidoc-card`
ancestor, which only `pages/cividoc.js` draws. The Instance page's `.vidoc-project` rows, its
project list (`.vidoc-project-list`) and its "Scan every project with Vidoc" row (inside
`.vidoc-instance`), keep their own 520px caps, right in that narrow settings column, unchanged.

## Requirements

- VLF-001: The switch's right edge is the card's content right edge (its right padding inside
  the card's border), at desktop and phone widths.
- VLF-002: The label stays at the card's content left edge on one line, ellipsized when narrow.
- VLF-003: The notes keep their readable cap (72ch).

## Plan

- [x] Spec the Security scan card layout fix and file the issue
- [x] Regression test: the card's switch sits at the card's right edge
- [x] The Security scan card's switch at its right edge, like Settings rows
- [ ] Independent review, gates green, changes log and delivery

## Acceptance criteria

- At 1600 px and 390 px wide, the switch's right edge is within 1 px of the card's right edge
  minus its right padding, and the label's left edge within 1 px of the card's left padding edge.
- The test fails on `dev` before the fix (switch far from the edge at 1600 px) and passes after.
- The Instance page's Vidoc card is unchanged.

## Test plan

- e2e (`e2e/tests/vidoc-switch.e2e.ts`): open `/s/studio/ci` in the mock at 1600 px and 390 px,
  measure the card, its padding, the switch and the label with `getBoundingClientRect`.
- The existing Vidoc e2e tests and axe checks still pass.
- The label's one line guards its `white-space: nowrap`; the ellipsis and the notes' 72ch cap
  (VLF-002, VLF-003) are rules this change does not touch, so no new test covers them.

## Decisions

- Keep the label and switch in their own row under the title (as Settings rows do) rather than
  moving the switch into the card's title row: the label names what the switch does for screen
  readers and sighted users alike, and the notes stay attached to it. No ADR: a styling fix inside
  SPEC-359's decision.
- Keep the 72ch cap on the notes: long lines across a 1900 px card are hard to read; the switch,
  not the text, has to reach the edge.
