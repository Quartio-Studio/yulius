---
yulius-task: 3db6d0f4-6c71-4d37-90aa-91afcdc929eb
status: done
---

# SPEC-031: a long task title no longer breaks the start-rule editor

Status: implemented
Date: 2026-10-12
Owner: Yulius task #29
Scope authorization: user request (F 2026-10-12, task #29): "View is broken. Fix.", with a screenshot of
the "Start when a rule is true" dialog.

## Goal

In the start-rule editor (SPEC-022) a condition whose task has a long title shows it on one line,
ending in an ellipsis, inside the control, instead of wrapping into two lines that spill over the
control's border.

## Context

The screenshot shows "#27 Too much elements here. Only Cockpit, Tasks, Git, CI (and CD in the
future), Sec…" on two overlapping lines in the task field of a condition.

Root cause: where the browser supports customizable selects (`appearance: base-select`, the
`@supports` block of `styles.css`), the picked option is drawn by the select's built-in button, which
lives in the browser's shadow tree. Page CSS cannot reach it, so `white-space`, `overflow` and
`text-overflow` set on `.select` do not truncate it: the label wraps inside the fixed-height
(`--control-sm`) select and overflows. The condition's task options are `#N <title>`, and titles are
free text, so any long title breaks the row. The e2e suite never opened the editor with a long title.

Only the rule editor's task select is changed. Other selects hold short, fixed labels; the CSS rule
below is generic, so one that can hold a long label opts in the same way.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RLF-001 | The task select of a rule condition shows its picked option on one line; a label wider than the control ends in an ellipsis and never spills over the control. | e2e `intake.e2e.ts` |
| RLF-002 | The editor stays free of WCAG violations (the author-provided select button is as tall as the select). | e2e `intake.e2e.ts` (axe) |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Regression e2e test that fails: a long title in a condition
- [x] Fix: author-provided `<button><selectedcontent>` in the task select, one-line ellipsis styles
- [x] Gates, review, changes log

## Acceptance criteria

- AC-RLF-1: with a task titled with 120 characters, the condition's task select shows one line of its
  label, truncated, inside the control.
- AC-RLF-2: choosing a task, adding and removing conditions and the keyboard flows behave as before.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `intake.e2e.ts`: "a long task title stays on one line in a condition, ending in an ellipsis"
  (red before the fix: the select had no one-line label; green after), plus the existing rule-editor
  tests, which include the axe check.
- Visual check in Chrome 155 against the screenshot's case.

## Decisions

- Provide the select's button in markup (`<select><button><selectedcontent>`) rather than style the
  browser's own: it is the only way to make the picked label shrinkable. Browsers without customizable
  selects ignore the extra child and keep the native control.
- Not a global change: adding the button to every select would touch many pages for no reported defect.
- The button overlaps the select's 1px border (`margin-block: -1px`) so its target is 24px tall, as
  the select is (WCAG 2.2 target size).

## Evidence

- Reproduced in Chrome 155 (headless) with a static page of the real stylesheets: two overlapping
  lines, as in the screenshot; the same page with the fix shows one line ending in an ellipsis.
- The new e2e test failed before the fix (commit d380e29: no one-line label, `fits: false`) and passes after.
- `scripts/dev ci` exit 0; e2e full suite: 156 passed, 1 skipped.
- Independent review (fresh read-only session): APPROVE, no major findings; applied `type="button"`
  on the inner button and a sturdier test (right edge inside the select, no NaN line height).
- Not covered: browsers without customizable selects (Firefox, Safari) were not run; they ignore the
  extra child and keep the native control.
