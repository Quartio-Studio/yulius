---
yulius-task: e6f60efa-0ed5-494a-8c00-b829a33a3748
status: in-progress
---

# SPEC-188: Supertask's description names one-shotting the product

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #173
Scope authorization: task #173, "Under the supertask add '...one-shotting the product etc' (make
sure grammar is correct)", with a screenshot of the start menu's Supertask option. Amends the
wording of SPEC-094 MDS-004.

## Goal

The start menu's Supertask option names a second example of broad work beside security
hardening: building the whole product in one go ("one-shotting the product"), in correct English.

## Context

- **Where the text lives.** `startchoice.js` `SUPERTASK_HELP` is the option's description in the
  start menu and the start chip's tooltip on a new task (`taskview.js`). Today it reads "Waits for
  this project’s running tasks, then runs alone while the project’s other tasks wait. For broad
  work such as security hardening." (SPEC-094 MDS-004, pinned by `menus.e2e.ts`).
- **Same example elsewhere.** The `task_create` MCP tool's description (`mcp.rs`) tells agents a
  supertask is "for broad work such as security hardening", and `docs/product.md` says the same.
  They describe the same feature, so they name the same examples.
- **Not touched.** The automation editor's Supertask tooltip (#407) says only the waiting rule,
  without examples, and stays as it is. Earlier specs keep their historical wording.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OSP-001 | The start menu's Supertask description (and the new task's start chip tooltip) reads: "Waits for this project’s running tasks, then runs alone while the project’s other tasks wait. For broad work such as security hardening or one-shotting the product." It still shows whole, inside the menu, at 1280 px and 390 px (SPEC-094). | e2e `menus.e2e.ts` "the start menu shows Supertask's whole description…" |
| OSP-002 | The `task_create` tool's description and `docs/product.md` name the same examples: "for broad work such as security hardening or one-shotting the product". | `mcp.rs` unit test on the `task_create` description |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The Supertask description names one-shotting the product, in the menu, the tool and the docs
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- Opening New task's start menu shows Supertask with "For broad work such as security hardening or
  one-shotting the product." as its last sentence, wholly visible.
- An agent's `task_create` tool description names the same two examples.

## Test plan

- `e2e/tests/menus.e2e.ts`: the SPEC-094 test's expected text is changed first and fails on the
  unchanged menu; the change to `SUPERTASK_HELP` makes it pass, at both viewports.
- `crates/yulius-app/src/mcp.rs`: a unit test that the `task_create` description names both
  examples, failing before the description changes.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security`.

## Decisions

- **Wording: "such as security hardening or one-shotting the product".** The request's "etc" is
  dropped: "such as" already says the list is examples, so "such as … etc." is redundant.
  "One-shotting" keeps the person's term, hyphenated as the compound verb is written; "or" joins
  two examples of one kind. Options considered: "…, one-shotting the product, etc." (redundant),
  "building the whole product in one go" (clearer to newcomers, but not what was asked; the term
  is common among the product's users).
- **Keep the tool and the docs in step.** The agent-facing description and the product summary
  name the same examples as the UI, so an agent asked to "one-shot the product" recognises a
  supertask case. The automation tooltip has no examples and is left alone.
- **No ADR.** A wording change; it takes no lasting decision.
