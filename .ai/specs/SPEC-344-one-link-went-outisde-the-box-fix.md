---
yulius-task: 6b7279b3-e2a7-4fde-83c8-3c6316d78a70
status: in-progress
---

# SPEC-344: A long host chip wraps inside its card

Status: in-progress
Date: 2026-10-28
Owner: Yulius task #328
Scope authorization: task #328 (`/goal`, with a screenshot of the `AWS_BEARER_TOKEN_BEDROCK`
agent secret card whose host chip `bedrock-runtime.us-east-1.amazonaws.com` ran past the card's
right edge), "one link went outisde the box, fix". Fixes #956.

## Goal

A chip in a `.chips` row (a secret's bound or requested hosts, an MCP server's secret keys) stays
inside its card at any width, and shows its whole text.

## Context

- **Symptom.** On a narrow Secrets page, the second bound host chip of an agent secret ran past
  the card's right edge.
- **Root cause.** `.badge` (`crates/yulius-server/web/assets/styles.css`) is
  `white-space: nowrap` with a fixed `height: 18px` and no `max-width`. `.chips` is a wrapping
  flex row, so it moves a chip to its own line, but a chip wider than the row cannot shrink or
  wrap and overflows `.item-main` and the card.
- **Why tests missed it.** The secrets e2e tests use short mock hosts (`api.stripe.com`), and the
  390 px checks look only at the page's own width (the card overflows visibly without widening
  the page).
- The same `.chips` row holds requested hosts (`agentHosts`, `secrets.js`) and an MCP server's
  secret keys (`pages/mcp.js`). Other badges (titles, tables, the task bar) are short labels that
  need to stay one line.

## Requirements

- **HCW-001 Inside the card.** A badge in a `.chips` row is never wider than the row. A longer
  text wraps onto more lines inside the chip and breaks anywhere, since a host name has no spaces.
- **HCW-002 Whole text.** The chip shows its whole text: no ellipsis. A person allows a requested
  host by reading it (SPEC-121), so it must not be cut.
- **HCW-003 Unchanged elsewhere.** A chip that fits stays one line at 18 px. Badges outside
  `.chips` are unchanged.
- **HCW-004 A wrapped chip is a box.** A chip on more lines is a rounded rectangle (`--radius-lg`,
  12 px, which one 18 px line clamps to the same pill) with its icon on the first line, not an oval with a centred icon.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- With long bound and requested hosts on agent secret cards, every `.chips` badge lies inside its
  `.item-main` and shows its whole text (`scrollWidth <= clientWidth`) at 1440 and 390 px. The page
  has no horizontal overflow and no WCAG violations.
- A chip that fits is 18 px high; a wrapped one (a long wildcard request) has a corner radius of at
  most 12 px and its icon within 5 px of its top.

## Test plan

- e2e `secrets.e2e.ts` "a long bound or requested host wraps inside its card". It fails on `dev`
  (the long chips have `inside: false` at 390 px) and passes with the fix.
- The whole e2e suite and `scripts/dev ci` for regressions.

## Decisions

- **Wrap, not ellipsis.** An ellipsis would hide part of a requested host before a person allows
  it (SPEC-121). Wrapping keeps it readable at the cost of a taller chip, which happens only for
  hosts wider than the card.
- **Scoped to `.chips > .badge`.** Every chip list with free-form text uses `.chips`. Other badges
  are fixed short labels in rows that rely on `nowrap`, so `.badge` itself stays as it is.
