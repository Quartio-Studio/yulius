---
yulius-task: 8ce2f8dd-6bd7-4932-82d7-5536c475034c
status: in-progress
---

# SPEC-560 Route GPT task models to Codex

## Goal

Fixes #1427.

A person or agent can copy a GPT model ID into a task and get a Codex task. A conflicting model choice should be actionable before the task runs.

## Context

Issue #1427 was reported during SPEC-511. On dev at `56f08db7`, both HTTP creation and MCP task_create use Harness::implied_by, whose only special case is Gemini. The validator used by creation and model edits accepts every model for Claude Code. SPEC-520 covered explicit Codex and implicit Gemini choices, leaving the GPT case untested.

## Requirements

- **RTE-001** With no harness or with auto, a GPT model selects Codex and records a chosen harness. Match the prefix case-insensitively after trimming, as for Gemini.
- **RTE-002** Refuse a known foreign provider's model on creation or edit. For GPT or Gemini on Claude, name the harness to choose. An unsuccessful edit leaves the stored settings intact.
- **RTE-003** Preserve Claude aliases, Codex custom model names, the Random default and unadorned task-agent follow-up inheritance.

## Plan

- [x] Reproduce model routing and document the task contract
- [x] Exercise creation and model edits through API and MCP
- [x] Route GPT models and reject conflicting harnesses
- [ ] Verify the fix and obtain independent review
- [ ] Record the acceptance and review evidence for delivery

## Acceptance criteria

- HTTP creation and MCP task_create choose Codex for a model-only GPT request, even from another harness.
- Creating an explicitly conflicting task returns HTTP 422. A GPT model edit on an existing Claude task fails, names Codex, and preserves the previous model and effort.
- Claude aliases still work and an unnamed model and harness still use SPEC-520 selection. The task creation API documentation states the routing and validation rules.

## Test plan

Use the integration router, MCP tools and isolated store. Exercise absent and auto harnesses, known conflicting prefixes, compatible aliases, default draws and caller inheritance. Run the focused Rust tests before and after the fix and verify the browser mock matches production. Submit the reviewed head to Yulius CI.

## Decisions

Infer Codex from the GPT prefix, extending the existing Gemini rule. Keep unrecognized IDs usable where the harness already accepts them. [ADR-0397](../../docs/decisions/0397-gpt-models-imply-codex.md) records the compatibility choice.

## Evidence

- Against dev `56f08db7`, the four Rust regressions in commit `85d941a20` failed. API POST and MCP task_create returned Claude for GPT. Explicit Claude/GPT creation returned 201. The Superagent task_model edit stored GPT and low effort on a Claude task. The browser mock likewise returned Claude and accepted the conflict.
- With the fix, the nextest selection covering harness_draw, the Superagent model edit, the Codex model edit, and task_delete passed all 18 tests. This includes the Random draw, caller inheritance, explicit auto, aliases and readback after a refused edit.
- The Codex Chromium file passed all 14 tests, including the new mock regression. These tests exercise task creation and stored settings with isolated fixtures.
- Review of `dda794454` requested two fixes in this change. The HTTP GET regression read back `" GPT-synthetic "` with its spaces. Task creation now stores the trimmed model used for routing. The browser approval regression displayed `GPT-6.2 Sol (default) · High effort` for an explicit model. Both mock creation paths now use the same model settings. The Codex and Superagent approval browser files passed all 34 tests after the fixes.
