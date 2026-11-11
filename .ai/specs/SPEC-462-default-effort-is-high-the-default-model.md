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
| EFF-003 | The composer's and Settings' effort menu describes "Default" as High (none for Haiku); `models.js` says Fable 5.2 at high effort. | `models.js`, `ui_contract.rs` test |
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
- `cargo test -p yulius --test main -- ui_contract::`: the effort and model menus' "Default" and the
  composer's model chip name the launch constants.
- Every gate (`scripts/dev ci`), on Yulius CI.

## Decisions

- Change the constant, not a setting: the person asked to keep this small enough to ship today.
  Whether the default should be an instance setting is said in the closing summary, not built here
  (ADR-0295 records the alternative).
- `task_create`'s descriptions are built from `DEFAULT_CLAUDE_MODEL` and `DEFAULT_CLAUDE_EFFORT`
  rather than retyped, since the retyped text was wrong twice.
- Nothing stored changes: a task record keeps `None` for "Default" (ADR-0024), so running tasks and
  Cockpits take `high` at their next start, and a task that named `max` keeps it.

## Evidence

- Test first: with the constant still `max`, `cargo test -p yulius-harness --test launch` failed 6
  of 14 tests on `--effort max` against the expected `high` (new launch, resume, Sonnet without an
  effort, the three Cockpit and Superagent launches); with `DEFAULT_CLAUDE_EFFORT = "high"` all 14
  pass, including a named `max` and `low` kept and Haiku without `--effort`.
- `cargo test -p yulius --test main -- cockpit:: global_superagent:: ui_contract::`: 72 passed; the
  fake `claude` receives `--model claude-fable-5-2 --effort high` in every Cockpit and Superagent
  start and resume.
- Guards proven by mutation: with the effort menu's "Default" put back to "Max",
  `ui_contract::the_ui_describes_default_as_the_model_and_effort_a_launch_uses` fails ("the effort
  menu's Default is not High"); with `task_create`'s model description put back to "default Opus",
  `mcp::tests::task_create_names_the_default_model_and_effort_of_a_launch` fails. Both pass on the
  committed code.
- `scripts/dev check`: green. `scripts/dev lint`: green (first run refused an empty line after a
  doc comment in `mcp.rs`, removed).
- `scripts/dev security`: "Security scan disabled: Vidoc is turned off in Yulius" — disabled, not
  passed.
- Independent review of `48aa92a9` (fresh session, read-only): request changes. Own code: the
  empty line after a doc comment (the lint failure above, fixed); this spec cited an e2e check that
  does not exist (now the `ui_contract.rs` test); the module-comment assertion was brittle
  (loosened to the wording only); no new-session case with neither model nor effort (added to the
  launch test); three edited lines unwrapped (rewrapped). Pre-existing, fixed here because they
  state the default: `docs/api.md` said the Cockpit takes the best subscription "for Opus";
  ADR-0020 and ADR-0055 now carry an "amended by" note. Left: the mock's task record shows a
  default task as `claude-opus-4` (`mock.js`, mock data that e2e tests read, not a statement of the
  default), and `task_create`'s test repeats the `format!` on purpose (it proves the wording is
  derived; the pinned pair beside it proves the value).
- The full suite and the e2e suite run on Yulius CI at the pull request head.
- Not exercised: a live Claude Code run on a subscription with `--effort high` (the launch
  arguments are proven against a fake `claude`), and production, which gets the change only after
  a promotion to `main` and an update.
