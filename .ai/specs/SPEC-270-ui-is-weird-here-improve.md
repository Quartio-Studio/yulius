---
yulius-task: 237460fe-0fca-4057-b7d7-3db06539b335
status: in-progress
---

# SPEC-270: The API token form lines up (Settings → Connect your local agent)

Status: in-progress
Date: 2026-10-22
Owner: Yulius task #256
Scope authorization: task #256 ("UI is weird here, improve", with a screenshot of the token form).

## Goal

The token form on Settings → Connect your local agent reads as one row: the "Token name" and
"Scope" labels on one line, the name input, the scope select and the Create token button on the
next with the same top and height, and the scope's hint under the select.

## Context

- The form is `crates/yulius-server/web/assets/pages/apitokens.js` (SPEC-247 RMT-001, RMT-040);
  its layout is `.agent-token-form` in `styles.css`: a three-column grid with `align-items: end`.
- The Scope field has three parts (label, select, hint) and the name field two, so bottom
  alignment pushes the name label and input down to the hint's last line: the labels sit on
  different lines and the name input's bottom matches the hint's, not the select's.
- The button is `btn-sm` (`--control-sm`) beside `--control-md` fields, aligned to the hint's
  bottom too, so it floats beside the hint.
- The select carries `class: 'input'`, the only `<select>` in the web UI that does; every other
  one is `.select`, which gets the app's customizable-select look (surface, picker, chevron). This
  one stays the browser's native control.

## Requirements

- TKF-001. On a wide screen the "Token name" and "Scope" labels share a top; the name input, the
  scope select and the Create token button share a top and a height (`--control-md`); the hint
  sits under the select, within its column.
- TKF-002. The scope select is a `.select`, like every other list to choose from.
- TKF-003. When the form is narrower than 600px (a phone, or a window up to about 900px beside
  the sidebar, where one row left the name input too narrow for its placeholder), it stacks: name,
  scope with its hint, then the button, nothing overflowing horizontally.
- TKF-005. A field message (`.field-error`, added by `invalid()` in `dom.js`) takes its own row
  under the hint instead of drawing over it.
- TKF-004. Behaviour is unchanged: the hint follows the chosen scope, the name is still required,
  tokens are made, shown once, listed and revoked as before (the existing e2e tests).

## Plan

- [x] Spec SPEC-270 for the token form layout
- [x] Token form labels, controls and button share lines on a wide screen
- [x] Token form stacks cleanly on a narrow screen
- [ ] Independent review answered
- [ ] Gates green, evidence recorded, delivered

## Acceptance criteria

- The e2e test `the token form reads as one row` passes and fails on the previous CSS/markup.
- The e2e tests `the token form stacks at 390 px`, `… at 800 px` and `a message on the scope field
  sits under its hint` pass and fail on the first version of the fix (review round 1).
- The existing `api-tokens.e2e.ts` tests still pass, with no axe violations.

## Test plan

- `e2e/tests/api-tokens.e2e.ts`: measure the labels, the input, the select and the button with
  `getBoundingClientRect` on the mock Settings page at desktop width (tops and heights equal,
  hint below the select and inside its column, the select has `.select`), at 390 and 800px
  (items stacked in order, no horizontal overflow of the form or page), and with a `.field-error`
  appended to the Scope field (below the hint, the controls still on one line).
- Full e2e suite and `scripts/dev ci` before delivery.

## Decisions

- **Subgrid rows instead of bottom alignment.** The form gets three rows (label, control, hint);
  each field spans them with `grid-template-rows: subgrid`, and the button sits in the control row.
  Labels, controls and the button then align whatever the hint's length, and the hint never moves
  the other fields. Alternatives: `align-items: start` with a top margin on the button equal to a
  label line (breaks when the label's font size changes); moving the hint out of the field to a
  full-width line under the form (separates the hint from its select and lengthens the section).
  Subgrid is in every browser the UI supports (Chrome 117, Firefox 71, Safari 16). No ADR: the
  choice is local to this form's CSS.
- **Full-height button.** The Create token button drops `btn-sm` so it matches the fields'
  `--control-md`, like the full-size primary buttons of the superadmin and password-reset forms.
- **Stack by the form's width, not the window's.** A container query on `.agent-connect` (600px)
  replaces the 640px media query: with the sidebar, a 641-900px window left the one-row name input
  under 120px, cutting its placeholder (review round 1). 600px keeps the name input at least about
  200px beside the 240px select and the button.
- **Four subgrid rows.** Label, control, hint and a field message: a subgrid makes no implicit rows,
  so with three a message from `invalid()` landed in the hint's row and drew over it (review round
  1). The empty fourth row costs one 4px gap under the form.
