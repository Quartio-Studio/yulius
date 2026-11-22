---
yulius-task: 5632227f-3279-48cb-9c18-3804eab24fc3
status: in-progress
---

# SPEC-620 — Claude Code on Opus 5.5 is the default for new tasks

ADR: [0457](../../docs/decisions/0457-new-tasks-default-to-claude-code-on-opus-5-5.md)

## Goal

A task that a person starts without choosing an agent runs Claude Code on Opus 5.5 at high
effort: from the composer in a browser with no saved choice, from `yulius task create`, from
either Superagent and from an approved cross-sandbox request. The person's request of
2026-11-22 (task #604): "Make opus and claude default model".

## Context

- [SPEC-583](SPEC-583-cli-nad-superagents-should-spawn-tasks-i.md)
  ([ADR-0422](../../docs/decisions/0422-cli-and-superagent-task-creation-defaults-to-codex.md))
  made `task_create` choose Codex when a caller without a task (the CLI through remote MCP, the
  sandbox Superagent, the Global Superagent, an approved cross-sandbox request) omits both
  `harness` and `model` (`crates/yulius-app/src/manage.rs`, `tool_task_create`).
- [SPEC-587](SPEC-587-codex-should-be-default-for-example-bart.md)
  ([ADR-0426](../../docs/decisions/0426-the-new-task-composer-defaults-to-codex.md)) made
  `modelPreferences()` (`crates/yulius-server/web/assets/models.js`), shared by the composer and
  Settings, answer `codex` (Codex default) when the browser has no saved model, an unreadable
  one or one no longer offered.
- The Claude defaults already are Opus 5.5 at high effort: `DEFAULT_CLAUDE_MODEL`
  (`claude-opus-5-5`, SPEC-493) and `DEFAULT_CLAUDE_EFFORT` (`high`, SPEC-462) in
  `crates/yulius-domain/src/harness.rs`. A Claude task without a model or effort runs them, and
  the first Anthropic model of `models.js` is `claude-opus-5-5`, which `ui_contract` keeps equal
  to `DEFAULT_CLAUDE_MODEL`.
- Out of scope: the Random draw itself (SPEC-520), which stays an explicit choice; the HTTP API's
  default for an absent `harness` and `model` (`auto`: the composer's Random sends no fields, so
  that default is Random's wire form); automations, which default to `auto` (SPEC-520 RHM-005);
  a running task's follow-ups, which keep their parent's agent (RHM-004).

## Requirements

- **CDM-001** `task_create` called with neither `harness` nor `model` by a caller without a task
  (the CLI, the sandbox Superagent, the Global Superagent, an approved cross-sandbox request)
  creates a Claude Code task with `harness_choice: "chosen"` and no model or effort override, so
  it runs `DEFAULT_CLAUDE_MODEL` (Opus 5.5) at `DEFAULT_CLAUDE_EFFORT` (high). Starting it needs
  a usable Claude subscription; without one the ordinary refusal names Claude.
- **CDM-002** A named harness, a model that implies one, an explicit effort and an explicit
  `auto` (Random) are honoured as before; a running task's follow-up keeps its parent's agent;
  automations and the HTTP API's absent harness keep their `auto` defaults.
- **CDM-003** In a browser with no saved model, an unreadable one or one no longer offered, the
  composer and Settings show Opus 5.5, and a task started untouched is a chosen Claude Code task
  on `claude-opus-5-5` at the inherited high effort. A saved model (Codex default included) and a
  saved Random choice (JSON null) are kept across visits. With no usable subscription the
  untouched composer offers Connect Claude.
- **CDM-004** The `task_create` and `sandbox_task_create` tool descriptions, the browser mock and
  the product, architecture, API and remote-control documents describe Claude Code on Opus 5.5
  as the default.

## Plan

- [x] Write and validate SPEC-620 and its ADR
- [ ] CLI, Superagent and cross-sandbox tasks default to Claude Code on Opus 5.5
- [ ] The composer and Settings start on Opus 5.5
- [ ] Product, architecture, API and remote-control docs say Claude Code is the default
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** (CDM-001) With Claude, Codex and Antigravity subscriptions all usable, the Superagent's
  `task_create` without `harness` or `model` records `claude` / `chosen`; the CLI's
  `yulius task create` with only a Codex subscription is refused naming Claude, and with a Claude
  subscription creates a task whose details show `harness_choice: "chosen"`,
  `model: "claude-opus-5-5"`, `model_default: true`, `effort: "high"`, `effort_default: true`.
  The Global Superagent and an approved cross-sandbox request record `claude` / `chosen`.
- **AC-2** (CDM-002) On the same callers, `model: "gemini-4-neon"` creates an Antigravity task,
  an explicit effort is kept, `harness: "auto"` draws (`random`), and a running Antigravity task's
  follow-up stays Antigravity.
- **AC-3** (CDM-003) In Chromium with empty storage, Settings and the composer show Opus 5.5; the
  started task's request carries `model: "claude-opus-5-5"` and no `harness` or `effort`, and the
  task is `claude` / `chosen` at effort `high`. An unrecognised and an unreadable saved model
  recover to Opus 5.5; a saved Random and a saved Codex default are kept. With no account the
  composer links Connect Claude and a refused start offers Connect Claude.
- **AC-4** (CDM-004) The tool descriptors' unit tests read Claude Code as the default and no
  longer Codex; `scripts/dev quick` passes.

## Test plan

- Rust, red first: `crates/yulius/tests/harness_draw.rs` (Superagent default and explicit
  choices), `remote_mcp.rs` (the real CLI against remote MCP, refusal then default details),
  `cockpit.rs`, `global_superagent.rs`, `cross_sandbox.rs`, `cross_sandbox_live.rs` (defaults
  recorded as Claude Code, the Codex fixture no longer needed), and the `mcp.rs` descriptor unit
  tests. Run each touched binary's tests in full (`yulius::main` filtered by file,
  `yulius-app`'s lib tests).
- Browser: `e2e/tests/model-choices.e2e.ts` (untouched default, recovery, empty-account
  connection, saved Random and Codex kept), `superagent-requests.e2e.ts` (approved request
  defaults), and the composer tests SPEC-587 gave `&codex=1` (drafts, filedrop, inline images,
  intake, new task, prepared, start change, supertask), now starting on the mock's Claude account.
- `scripts/dev quick`; Yulius CI runs the gate.

## Decisions

- **Reverse both Codex defaults, keep Random explicit
  ([ADR-0457](../../docs/decisions/0457-new-tasks-default-to-claude-code-on-opus-5-5.md),
  superseding ADR-0422 and ADR-0426).** The request names the default model; the places that
  default to Codex today are the CLI/Superagent rule and the browser's untouched choice. Changing
  the HTTP API's absent-harness default would turn the composer's Random (which sends no fields)
  into Claude, and changing automations would rewrite a contract the request does not mention.
- **The CLI and Superagents leave model and effort unset.** The existing launch defaults supply
  Opus 5.5 at high, and the task reports them as inherited (`model_default`), as SPEC-583 did for
  Codex.
- **The browser's untouched choice is the Opus 5.5 item** (`PROVIDERS[0].models[0]`, which
  `ui_contract` keeps equal to `DEFAULT_CLAUDE_MODEL`), sent as `model: "claude-opus-5-5"`. A
  harness-only "Claude default" item would list the same model twice and break the menu's
  one-to-one list with `OFFERED_CLAUDE_MODELS`. Saved preferences are read, never rewritten, so a
  person who chose Codex default or Random keeps it.
