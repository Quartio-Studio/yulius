---
yulius-task: b35ca245-ec20-4f1c-9b80-f9c148f00648
status: in-progress
---

# SPEC-022: a start rule is picked in a simple editor

Status: in-progress
Date: 2026-10-12
Owner: Yulius task #20
Scope authorization: user request: "when rule is true should be a simple editor, not a string to
be written" (screenshot: the composer's Run after menu, "When a rule is true…").

## Goal

The composer's "When a rule is true…" opens a small editor in which a person picks conditions
("#12 Checkout API · is done") and whether all or any of them must hold, instead of typing a
sentence such as `#12 done and #13 done`. The editor writes that sentence for the server, so
start rules themselves (SPEC-017) do not change.

## Context

- [SPEC-017](SPEC-017-task-start-rules.md) and [ADR-0018](../../docs/decisions/0018-task-start-rules.md):
  a start rule is text over `#N done|merged|ended` atoms with `and`, `or`, `not` and parentheses,
  parsed and evaluated by `yulius_domain::rule`; the API takes it as `start_rule`.
- Today `openRuleDialog` in `crates/yulius-server/web/assets/taskview.js` shows one text input,
  so people must know the grammar and the task numbers by heart; a mistake is only found when
  the task is created.
- The composer already lists the sandbox's tasks for "After #N" (`chainCandidates` in
  `intake.js`); `.seg` radio groups and `.select` controls exist in `styles.css`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RED-001 | "When a rule is true…" opens an editor of conditions, each one a task of this sandbox (`#N title`, newest first, archived ones left out) and a state: "is done", "is merged" or "has ended". There is no text field to write the rule in. | e2e |
| RED-002 | With two or more conditions the editor asks whether **all** (`and`) or **any** (`or`) of them must be true; Add condition adds a row (preferring a task not yet used) and each row can be removed. | e2e |
| RED-003 | The editor shows the sentence it builds ("Starts by itself when #12 done and #13 done") and Use this rule sets it; the text is the canonical form the server stores (`#12 done and #13 done`). | in-page unit test, e2e |
| RED-004 | Opening the editor again shows the picked conditions, so they can be changed; Start now clears them. | e2e |
| RED-005 | In a sandbox with no other task the menu does not offer a rule. | e2e |
| RED-006 | The API keeps the full grammar (`not`, parentheses, mixed `and`/`or`); only the composer is simpler. | unchanged API tests |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Picked conditions become the start rule text the server reads
- [ ] The composer's "When a rule is true…" opens a condition editor instead of a text box
- [ ] A picked rule opens again for changes, and a sandbox without tasks says so
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- Picking `#12 Checkout API is done`, adding `#4 Payment webhook has ended`, choosing Any and
  using the rule starts a task queued "when #12 done or #4 ended".
- No step of creating a rule needs typing.
- The editor has no WCAG violations; `scripts/dev ci` and the e2e suite pass.

## Test plan

- In-page unit test of the pure `ruleText` function (one condition, all, any, empty).
- e2e (`intake.e2e.ts`): build a two-condition rule with Any, see the chip and the queued task;
  reopen the editor and find the conditions; remove a row; Start now clears the rule; axe on the
  open editor.
- e2e: a sandbox whose only tasks are archived or none offers no rule (mock sandbox without
  tasks).

## Decisions

- **A flat list of conditions joined by all/any**, not a nested tree with `not`: it covers the
  examples people gave (`X and Y finished`, `either`) with two controls per row. Rules that need
  `not` or parentheses are still possible through the API. Recorded in ADR-0019.
- **No "edit as text" escape hatch** in the composer: the request is to stop writing strings, and
  a second input mode doubles what has to be tested and explained.
- **Native `<select>` controls** for task and state: keyboard and screen-reader support for free,
  styled by the existing `.select` rules.
- The editor lives in its own module (`ruleeditor.js`) with a pure `ruleText` function, so the
  sentence it builds is unit-tested without the dialog.
