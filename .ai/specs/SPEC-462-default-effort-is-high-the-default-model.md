---
yulius-task: 175ec3ab-48e5-4288-b7d9-7a5e88ea650e
status: in-progress
---

# SPEC-462: the default effort is high (the default model stays Fable 5.2)

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #444
Scope authorization: the person's decision of 2026-11-11: "the default effort is **high**. Not max,
not xhigh. The default model stays **Fable 5.2** (`claude-fable-5-2`)."

## Goal

A task, Cockpit or Superagent that names no effort runs at `high`. One that chose its own effort
keeps it, and the default model stays `claude-fable-5-2`. Every place that states the default says
so.

## Context

[SPEC-023](SPEC-023-make-sure-the-default-model-is-always-op.md) owns the default model and effort.
`DEFAULT_CLAUDE_EFFORT` (`crates/yulius-domain/src/harness.rs`) was `high` until 2026-11-10, when
#1145 (ADR-0289) moved the default to Fable 5.2 at `max`. `plan_launch` and `plan_cockpit`
(`crates/yulius-harness/src/launch.rs`) send the constant at every start for a session that names
no effort, so a running task follows the default at its next start; no stored data changes.

The MCP tool `task_create` still described its arguments as "Model override (default Opus)" and
"Effort override (default high)": the model was wrong since #1145, and the effort was wrong for a
day. SPEC-002's launch table, SPEC-011 and SPEC-021 still named Opus 5.5 as the current default.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| EFF-001 | A launch (new, resume, subscription switch) without an effort passes `--effort high`; a named effort is kept; Haiku gets none; the model default stays `claude-fable-5-2`. | `launch.rs` tests |
| EFF-002 | The Cockpit and both Superagents start with `--model claude-fable-5-2 --effort high`. | `launch.rs`, `cockpit.rs`, `global_superagent.rs` tests |
| EFF-003 | The composer's and Settings' effort menu describes "Default" as High (none for Haiku); `models.js` says Fable 5.2 at high effort. | `models.js`, e2e |
| EFF-004 | `task_create` describes its `model` and `effort` arguments with the constants, so the text cannot fall behind them again. | `mcp.rs` test |
| EFF-005 | SPEC-023, ADR-0295 (supersedes the effort of ADR-0289), `docs/api.md`, `docs/product.md`, SPEC-002's launch table and the changes log state Fable 5.2 at high. | the documents |

## Plan

- [ ] Launches without an effort send --effort high, proven by launch and Cockpit tests
- [ ] The composer, Settings and task_create describe the default as Fable 5.2 at High
- [ ] SPEC-023, ADR-0295, the API and product documents and the changes log state the default
- [ ] Every gate green and the independent review answered

## Acceptance criteria

- A new task, a resumed task and a task moved to another subscription with no model or effort run
  `claude --model claude-fable-5-2 --effort high`.
- A task that chose `max` (or any other effort) still gets it.
- The Cockpit and the Superagents start with `--model claude-fable-5-2 --effort high`.
- No tracked file states `max` as the default effort, or Opus as the default model, as the current
  state.

## Test plan

- `cargo test -p yulius-harness --test launch`: the argv of a launch without an effort, of a named
  effort (`max` kept), of Haiku and of the Cockpit.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^(cockpit|global_superagent)::/)'`:
  the arguments the fake `claude` receives.
- `cargo test -p yulius-app mcp`: `task_create`'s descriptions name the constants.
- e2e: the effort menu's "Default" reads "High (none for Haiku)".
- Every gate (`scripts/dev ci`), on Yulius CI.

## Decisions

- Change the constant, not a setting: the person asked to keep this small enough to ship today.
  Whether the default should be an instance setting is said in the closing summary, not built here
  (ADR-0295 records the alternative).
- `task_create`'s descriptions are built from `DEFAULT_CLAUDE_MODEL` and `DEFAULT_CLAUDE_EFFORT`
  rather than retyped, since the retyped text was wrong twice.
- Nothing stored changes: a task record keeps `None` for "Default" (ADR-0024), so running tasks and
  Cockpits take `high` at their next start, and a task that named `max` keeps it.
