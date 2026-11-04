---
yulius-task: b528c3e8-58ff-4dd0-9339-dd70999c941c
status: in-progress
---

# SPEC-417: The CD page shows its environments in two columns, not three

Status: in-progress
Date: 2026-11-04
Owner: Yulius task #399
Scope authorization: task #399 ("i want 2 columns not 3", with a screenshot of the CD page
showing the Dev, Dev-both and Production cards side by side and Prod-both alone below them).

## Goal

On a wide window the CD (deployment) page lays its environment cards out in two columns, never
three or more. A narrow window keeps one column, as today.

## Context

- `pages/cd.js` puts one `.cd-env` card per `[cd.<name>]` into `.cd-envs`. In `styles.css`,
  `.cd-envs` is `repeat(auto-fit, minmax(min(100%, 320px), 1fr))`: as many 320 px columns as the
  width holds. With the side panel closed on a wide screen that is three (or more), and each card
  is then too narrow for its long facts (secret names, commands, the rollback notes), so they
  wrap into tall cards and a fourth card sits alone on a new row (the screenshot).
- SPEC-152 already relies on this grid for the deploy-button layout at "two 362 px cards"; the
  card's head stays correct at any column width (e2e `each card keeps its deploy button top
  right…`, at 390 px).
- The cards are of different heights; the grid already aligns rows, nothing else changes.

## Requirements

- CDC-001. The `.cd-envs` grid never has more than two columns, at any window width.
- CDC-002. Two columns appear once each can be at least 320 px wide; below that the cards stack
  in one column without horizontal scrolling (unchanged).
- CDC-003. With an odd number of environments, the last card keeps the width of one column.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The CD page shows at most two environment cards per row, one on a narrow screen
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- An e2e test with four environments at 1920 px wide counts the cards per row (grouped by their
  top edge) as two and two, of equal width; it fails on `dev` (three and one) and passes with the
  change. With three environments the rows are two and one, the last card one column wide.
- At 390 px each row holds one card and the page does not scroll sideways.

## Test plan

- e2e (`cd.e2e.ts`): add two environments to the mock's studio (as the existing staging and
  platform test does), set the viewport to 1920 × 1080 with the side panel as the mock opens it,
  and count the `.cd-env` cards per row (grouped by their top edge) and compare their widths;
  then with one environment fewer, then at 390 px.
- Run the CD e2e file, then the full e2e suite and `scripts/dev ci`.

## Decisions

- Cap the column count in CSS with the minimum column width
  `max(320px, (100% - gap) / 2)`, still clamped by `min(100%, …)`. A column can then never be
  narrower than half the row, so at most two fit, and below 652 px the single column remains.
  Alternatives: `repeat(2, …)` plus a container query to fall back to one column (two rules and a
  breakpoint to keep in sync with the gap), or a page-wide `max-width` on the grid (wastes the
  width and still shows three narrow cards on mid-size screens). No ADR: a layout value of one
  page, recorded here.
