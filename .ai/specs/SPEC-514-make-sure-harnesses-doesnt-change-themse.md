---
yulius-task: 576c1ff0-1205-45e0-83e2-d1af141d2544
status: done
---

# SPEC-514: a task's agent is set, never drawn

## Goal

The agent (harness) a new task runs is predictable: the one the person, the model or the calling
task names, else Claude Code. Yulius no longer draws Claude Code or Antigravity at random for a
task, for an agent's follow-up task or for an automation's run, so one task does not run Claude
Code and the next Antigravity (Gemini) without anyone choosing it. The person's request (task #496,
2026-11-19): "make sure harnesses doesnt change themselves between tasks (like claude -> gemini)
because it's unpredictable. A harness is set for a task".

## Context

- SPEC-505 slice E (PR #1407, ADR-0361, `F 2026-11-17 Antigravity`) made a task created without
  an agent (or with `auto`) draw Claude Code or Antigravity uniformly among those with a usable
  subscription (RND-001), made the composer and the automation editor default to Random, made an
  automation without an agent draw for each run (RND-002), and made the agents' `task_create` and
  `automation_create` tools default to `auto` (before, `task_create` always ran Claude Code and
  `automation_create` the calling task's agent).
- A task already keeps its harness for life: it is stored on the task at creation and every run,
  rotation, move and resume stays on it and within its provider (ADR-0361 "Harness", SPEC-002
  ADR-06; provider switching was removed by migration 0003). What changes "between tasks" is only
  the choice at creation.
- The draw is recorded in `task_harness_draws` (migration 0066), shown as `harness_choice`
  (`random` or `chosen`) on a task and in the analytics export (RND-003).
- The composer keeps a chosen model in this browser (`yulius.composer.model`), so a person who
  picks Gemini 4 Carbon keeps it for the next tasks; the no-choice item is what changes.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| PIN-001 | A task created without an agent (or with `auto`) runs the agent its model implies (Antigravity for `gemini-*`, else Claude Code), and without a model Claude Code, whatever subscriptions are usable; it is never drawn and its `harness_choice` is `chosen`. A Claude Code task without a usable Claude account is refused as any Claude Code task is. | `tests/harness_pin.rs` |
| PIN-002 | A task an agent creates with `task_create`, and an automation it creates with `automation_create`, without naming an agent or (for a task) a model runs the calling task's agent; without a calling task (the Superagent), Claude Code. | `tests/harness_pin.rs` |
| PIN-003 | An automation always runs the agent it names; an automation saved as `auto` (or without an agent) runs Claude Code, and the ones stored as `auto` before this change are stored as `claude`; an update that names no agent (the Automations list's enable switch) keeps the stored one. | `tests/harness_pin.rs`, store test |
| PIN-004 | The composer's and Settings' no-choice model item is Default (Claude Code, Opus 5.5) instead of Random, the automation editor offers Claude Code, Codex and Antigravity with Claude Code first and selected for a new automation; the mock creates tasks the same way. | `e2e/tests/antigravity.e2e.ts`, `e2e/tests/codex.e2e.ts`, `tests/ui_contract.rs` |
| PIN-005 | Tasks drawn before this change keep `harness_choice` `random` (their overview and the analytics export say so); no new task is recorded as drawn. | `tests/harness_pin.rs` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A task created without an agent runs Claude Code or its model's agent, never a drawn one
- [x] An agent's task_create and automation_create without an agent keep the calling task's agent
- [x] An automation runs the agent it names: stored auto becomes Claude Code (migration 0068)
- [x] The composer, Settings and automation editor default to Claude Code; the mock matches
- [x] ADR-0368 and the documents say a task's agent is set, never drawn
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** (PIN-001) With Claude Code and Antigravity both usable, ten tasks created without an
  agent and without a model all run Claude Code, `harness_choice` `chosen`; with only Antigravity
  usable, such a task is refused for want of a Claude account; one with `gemini-4-carbon` runs
  Antigravity.
- **AC-2** (PIN-002) An Antigravity task's `task_create` without harness or model creates an
  Antigravity task; with `model: claude-sonnet-5-5`, a Claude Code task; its `automation_create`
  without harness stores `antigravity`.
- **AC-3** (PIN-003) An automation created with `auto` reads back as `claude` and its runs are
  Claude Code tasks; a database with an `auto` automation migrates it to `claude`.
- **AC-4** (PIN-004) The composer's model chip reads "Default model" before a choice and a task
  started then is a Claude Code task; the automation editor's Agent group has no Random.
- **AC-5** (PIN-005) A task recorded in `task_harness_draws` still summarises as `random`.

## Test plan

- `crates/yulius/tests/harness_pin.rs` (replaces `harness_draw.rs`): AC-1, AC-2, AC-3, AC-5 over
  the HTTP API and the MCP tools with the fake agent, both providers' accounts usable.
- Store: the migration test turns an `auto` automation row into `claude`; the pinned-versions test
  lists 0068.
- e2e: the composer's Default item creates a Claude Code task with both agents connected; the
  automation editor's Agent radio group starts on Claude Code.
- `scripts/dev quick` (ui_contract, docs contracts).

## Decisions

- **No draw at all, rather than Random as an opt-in.** The person's newer request reverses the
  random default of `F 2026-11-17 Antigravity`; an opt-in Random would keep a second meaning of
  "no agent" (`auto` vs `random`) and an unpredictable path for agents' tools. A comparison can
  still be made by choosing the agent per task; drawn tasks stay labelled. ADR-0368.
- **An agent's follow-up keeps its own agent.** A Gemini task's follow-up is Antigravity and a
  Claude task's is Claude Code: the agent stays the one the work is running on, as
  `automation_create` did before slice E; Claude Code is the default only without a calling task.
- **Migrate stored `auto` automations to `claude`** instead of reading `auto` as Claude Code: the
  record then says what it runs, and the editor shows it.
- **Keep `task_harness_draws` and `harness_choice`** for the tasks drawn while the draw existed;
  dropping them would relabel those tasks as chosen in the analytics.
- **`auto` stays accepted** on the API and tools (it was the documented default since slice E) and
  means "the default agent", never a draw.

## Evidence

- Red first: `harness_pin::*` (6 tests) all failed on the base (drawn `antigravity`/`random`,
  `auto` stored, a no-agent task created with only Antigravity usable);
  `an_automation_keeps_its_agent_through_an_update` failed before the round-1 fix
  ("enabled false": `claude` instead of `antigravity`).
- `cargo nextest` over harness_pin, automations, manage_tools, cross_sandbox, analytics,
  agent_tools, superagent, ui_contract, task_control and the yulius-store, -domain and -app units
  after the dev merge: 1368 passed; after the round-1 fixes, harness_pin, automations,
  manage_tools, ui_contract and the app's automation units: 60 passed.
- Store: `version_sixty_seven_automations_on_auto_run_claude_code` (an `auto` row becomes
  `claude`) and the pinned-versions test (0067 at 67, 0068 at 68) pass.
- e2e: antigravity, codex, superagent-requests 42 passed; antigravity, codex, automations 38
  passed after round 1.
- `scripts/dev quick` passed; `cargo clippy -D warnings` on the touched crates passed.
- Yulius CI on 478d66846 passed (security skipped: Vidoc refused the instance key, HTTP 401;
  unrun, not passed).
- Review: round 1 (Fable 5.2) requested changes: an update without an agent reset it to Claude
  Code (the list's enable switch), no PUT test, two dropped PIN-005 assertions, two nits; all
  fixed. Round 2 approved d811439de with three nits: the index order and the ADR clause fixed
  here; `editable()` sending `harness` is kept as harmless (the server test holds the rule).
- Not tested: a live Antigravity (Google) account; the change only picks the agent at creation.
