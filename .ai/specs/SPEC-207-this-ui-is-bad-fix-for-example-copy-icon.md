---
yulius-task: c6a411ec-cfaa-412a-8191-26a60b2118a9
status: in-progress
---

# SPEC-207: a shown secret's value, notes and key each sit in one box with their own copy icon

Status: in-progress
Date: 2026-10-17
Owner: Yulius task #194
Scope authorization: the person's request (task #194), "this UI is bad, fix. For example copy
icon is missing and overall", with a screenshot of a team secret's Show dialog.

## Goal

On the Secrets page, the dialog that **Show** opens (team secrets, and agent secrets for the
people who manage them) shows one self-contained box for the value. The box holds the value
(masked as before), the eye button and a copy icon button, always visible. The separate **Copy**
text button beside the box goes. The notes, and the key in the two-factor setup dialog, sit in the
same kind of framed box with their own copy icon. A copy shows that it worked on the button itself,
not only in a toast.

## Context

- The screenshot shows the dialog after SPEC-114 (ADR-0093). The value box has the masked dots
  and the eye, then an empty gap: the copy icon (`.secret-quick-copy`) is `visibility: hidden` until
  the box is hovered or holds focus, so a person who does not hover never sees it ("copy icon is
  missing"). Beside the box, a `Copy` text button (`Copy value`) does the same thing, is shorter
  than the box and looks detached from it. The notes are bare monospace text under a label, with no
  frame, so the dialog reads as loose pieces ("and overall").
- Code: `crates/yulius-server/web/assets/pages/secrets.js`, `maskedValue()` and `showRevealed()`;
  the setup key in `enterCode()` is a bare `<code>` plus a `Copy` text button. Styles:
  `.secret-box`, `.secret-quick-copy`, `.secret-notes` in `styles.css`.
- `copyValue()` copies, clears the clipboard after 30 s (VLT-002) and shows a toast.
- `dom.js` already has the `copy`, `check`, `eye` and `eye-off` icons and `iconButton()`.
- e2e `secrets.e2e.ts` covers SPEC-114: it asserts the quick copy is hidden before hover and that a
  `Copy value` button sits beside the box (`.secret-box + .btn`). Both change with this spec.
- Related: SPEC-114 (masking), SPEC-040 (two-factor), SPEC-001 VLT-010 and AC-VLT-9.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| SCB-001 | The Show dialog's value box always shows its eye and its copy icon button (`Copy value`), with no hover or focus needed; the copy copies the value whether masked or shown, with the toast and the 30-second clipboard clearing of VLT-002. The value stays masked and out of the DOM as SPEC-114 requires. | e2e |
| SCB-002 | No separate Copy text button beside the value box: the box spans the field's width. | e2e |
| SCB-003 | After a copy, the copy button shows a check icon and the tooltip "Copied" for about 2 seconds, then returns to the copy icon; its accessible name stays constant. | e2e |
| SCB-004 | The notes sit in a framed read-only box like the value box, wrapping long lines and scrolling past 240 px, with a `Copy notes` icon button that copies them the same way. | e2e |
| SCB-005 | The two-factor setup key sits in the same framed box with a `Copy key` icon button in place of its `Copy` text button. | e2e |
| SCB-006 | No WCAG A/AA violations in the dialog, masked or shown, in both themes; the boxes and their buttons stay inside the dialog at 390 px. | e2e (axe) |
| SCB-007 | SPEC-001 (VLT-010, AC-VLT-9), the product summary, a decision record superseding ADR-0093's hover copy, `e2e/README.md` and `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The value box always shows its eye and copy icons, with copied feedback, and no separate Copy button
- [ ] Notes and the two-factor key sit in framed read-only boxes with their own copy icon
- [ ] Docs, decision record and changes log describe the new secret dialog
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-SCB-1: right after the two-factor code, the Show dialog's `Copy value` button is visible
  without hovering, sits inside `.secret-box` after the eye, and clicking it while masked puts the
  value on the (stubbed) clipboard and shows "Value copied"; the value is still not in the page.
- AC-SCB-2: the dialog has exactly one button named `Copy value`, and no `.btn` follows the value
  box.
- AC-SCB-3: after the click, the button's title is "Copied" and its icon the check; it returns to
  "Copy value" and the copy icon (waited on, not slept).
- AC-SCB-4: the notes are inside a `.secret-box` with a visible `Copy notes` button, which puts the
  notes on the clipboard.
- AC-SCB-5: the two-factor setup dialog shows the key in a `.secret-box` with a `Copy key` button,
  and no button named `Copy` remains there.
- AC-SCB-6: axe reports no violations in the dialog, masked and shown, dark and light; at 390 px no
  box or its buttons leave the dialog.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/secrets.e2e.ts` (mock mode, clipboard stubbed in the page): the SPEC-114 test is
  changed to assert the copy is visible without hover and the only copy of the value (AC-SCB-1, 2,
  3, 6); a new assertion block covers the notes' copy (AC-SCB-4); the two-factor helper asserts the
  key box (AC-SCB-5). Red first: today the copy is hidden before hover, a second `Copy value` button
  follows the box and the notes and key have no box or `Copy notes`/`Copy key` button.
- No server change: the reveal API, its audit and the two-factor unlock are unchanged.

## Decisions

- **One copy control per value, always visible, inside its box.** Two controls for the same copy
  (a hover icon and a text button) were the confusion; a hover-only control is invisible on touch
  screens and to anyone who does not hover. The icon stays in the box next to the eye, which is
  where password managers and code blocks put it. ADR-0164 records it and supersedes the hover copy
  and kept button of ADR-0093; the masking of ADR-0093 stays.
- **The button keeps the name "Copy value"**, which the SPEC-114 tests and assistive technology
  already know; only its tooltip says "Copied" for the moment after a copy, and the toast announces
  the result (it is a live region), so the name does not have to change.
- **Notes get a copy too.** They hold the user name and recovery codes people copy next to the
  value; copying them clears the clipboard after 30 s like the value.
- **Notes stay shown** (not masked), as SPEC-114 decided.
- **The setup key uses the same box** so the vault's dialogs share one pattern; it stays shown.
- **Not taken:** a read-only `<input>` for the value (announced as an editable field, ADR-0093);
  a text "Copy" button inside the box (wider, and it squeezes the value at 390 px).
