---
yulius-task: 8156a710-48d9-42b5-b65f-019c1476aba1
status: done
---

# SPEC-511: A Codex task runs GPT-6.2 Astra at high effort by default

Status: done
Date: 2026-11-19
Owner: Yulius task #493
Scope authorization: the person's request of 2026-11-19 (task #493), verbatim: "default model for
codex should be astra 6.2 effort high".
ADR: [0366](../../docs/decisions/0366-a-codex-task-runs-gpt-6-2-astra-at-high-effort-by-default.md).

## Goal

A Codex task that names no model runs GPT-6.2 Astra (`gpt-6.2-astra`), and one that names no
effort runs at high effort, instead of whatever the Codex account or the CLI would pick. The run
records that model, the task view names it, and the composer says so. A task that names its own
model or effort keeps it.

## Context

- **Today (SPEC-503 CDX-013).** A Codex task without a model is launched with no `-m` and no
  `model_reasoning_effort`, so Codex runs its account's default model at its own default effort;
  the run records no model (`attempts.model` is `NULL`) and the task view shows an empty model.
  The composer's OpenAI group offers one entry, "Codex", with no model id (`harnessOnly`).
- **Antigravity already does this (SPEC-505 AGY-002).** `DEFAULT_ANTIGRAVITY_MODEL` and
  `DEFAULT_ANTIGRAVITY_EFFORT` in `yulius-domain::harness`; the launch plan fills them in when the
  request has none; the supervisor records the model; the task view shows both. Codex follows the
  same seams.
- **The model id.** Checked on 2026-11-19 in the Codex CLI's bundled model catalog: release
  0.170.0 lists `"slug": "gpt-6.2-astra"`, `"display_name": "GPT-6.2-Astra"`, reasoning levels
  `low`, `medium`, `high`, `xhigh`, `max`, `ultra` (default `low`), `"minimal_client_version":
  "0.166.0"`, `"supported_in_api": true`. Yulius pins Codex 0.169.0 (ADR-0355), whose bundled
  catalog stops at `gpt-6.1-*`. The review ran the real 0.169.0 against a local stub of the model
  API: every request carried `model: "gpt-6.2-astra"` and `reasoning.effort: "high"`. With an
  API-key sign-in the CLI asks for no model list and runs the model on fallback metadata ("Model
  metadata for `gpt-6.2-astra` not found. Defaulting to fallback metadata; this can degrade
  performance and cause issues."), sending 0.169.0's generic request shape (same context window,
  258400). With a ChatGPT sign-in it asks `/backend-api/codex/models?client_version=0.169.0` and
  takes the catalog entry the server gives, which lists the model for clients from 0.166.0
  (whether the real server does for 0.169.0 is not checked). So the pinned CLI runs it; a bump to
  a release that bundles it is its own change.
- **Effort.** `-c model_reasoning_effort="high"`, the key Yulius already writes for a chosen
  effort (`plan_codex`).

## Requirements

| ID | Requirement | Verification |
|---|---|---|
| CXD-001 | A Codex launch without a model passes `-m gpt-6.2-astra`; without an effort, `-c model_reasoning_effort="high"`. A model or effort of the task's own is passed instead. | `crates/yulius-harness/tests/launch.rs` `codex_defaults_to_gpt_6_2_astra_at_high_effort`, `codex_resume_argv` |
| CXD-002 | The run of a Codex task without a model records `gpt-6.2-astra`; the task view names `gpt-6.2-astra` with `model_default: true` and effort `high` (its own effort when it has one); its launch says no Claude model is used up; the pool weighs the task by that model's family, so its own subscription is left out at a limit. | `crates/yulius/tests/codex.rs` (the goal-mode test's CDX-013 part) |
| CXD-003 | The composer's Codex entry says it runs GPT-6.2 Astra at high effort by default; the browser mock creates a Codex task without a model on `gpt-6.2-astra` at `high`, its details "(default)". | `crates/yulius/tests/ui_contract.rs` `the_composer_says_codex_runs_its_default_model`, `e2e/tests/codex.e2e.ts` |

## Plan

- [x] Spec and ADR record that a Codex task defaults to GPT-6.2 Astra at high effort
- [x] A Codex task without a model or effort launches and records GPT-6.2 Astra at high effort
- [x] The composer, the mock and the docs name Codex's default
- [x] Independent review of the change

## Acceptance criteria

- AC-511-1: `codex … -c model_reasoning_effort="high" … -m gpt-6.2-astra …` for a Codex task that
  names neither (CXD-001).
- AC-511-2: The attempt and the task view of such a task name `gpt-6.2-astra` at `high` (CXD-002).
- AC-511-3: The composer's Codex entry names GPT-6.2 Astra at high effort by default (CXD-003).

## Test plan

- Unit: the Codex launch plan's argv with neither, and with the task's own model and effort.
- Integration: a Codex task without a model run on the fake agent records the default model, and
  its detail names it and high effort.
- UI contract: the composer's Codex description names the server's default.
- The real CLI: not run against a real Codex account (none is connected to this sandbox); the
  model id and its support from client 0.166.0 come from the CLI's own bundled catalog.

## Decisions

- **The server applies the default, the composer sends no model for Codex.** The Codex entry keeps
  its id `codex` and `harnessOnly`, so a browser's saved choice keeps working and the default has
  one source (`DEFAULT_CODEX_MODEL`), as Antigravity's effort does. Alternative: a model entry
  `gpt-6.2-astra` in the OpenAI group; rejected, it would orphan the saved `codex` choice and put a
  second copy of the default in the page.
- **No Codex CLI bump.** 0.169.0 runs the model (see Context); moving the pin is its own change.
- **Lasting decision** in ADR-0366.

## Evidence

- Model id: `strings` of the Codex CLI 0.170.0 and 0.172.0-alpha.1 linux-x64 binaries (npm
  `@openai/codex@<v>-linux-x64`) hold the bundled catalog entry `"slug": "gpt-6.2-astra"`,
  `"display_name": "GPT-6.2-Astra"`, efforts `low` … `ultra`, `"minimal_client_version":
  "0.166.0"`; 0.169.0's binary lists `gpt-6.1-*` at most and holds the fallback-metadata warning; how it
  asks for models per sign-in kind is in Context (the review's stub run).
- Test-first: `cargo test -p yulius-harness --test launch codex` failed before the launch change
  (`codex_defaults_to_gpt_6_2_astra_at_high_effort`: no `-m`, no effort; `codex_resume_argv`),
  then 26/26 passed; `cargo test -p yulius-harness` all green.
- `cargo test -p yulius --test main codex::a_codex_goal_task_gets_its_request` failed before the
  supervisor change (`left: None, right: Some("gpt-6.2-astra")`), then `codex::` 20/20 passed;
  `antigravity:: model_fallback:: task_delete::` 26/26; `cargo test -p yulius-app -p yulius-domain`
  all green.
- `ui_contract::the_composer_says_codex_runs_its_default_model` failed before the composer and
  mock change, then `ui_contract::` 12/12 passed.
- `npx e2e run tests/codex.e2e.ts`: 13/13 passed (the composer test now checks the entry's text
  and the created task's `gpt-6.2-astra` at `high`).
- `cargo clippy -p yulius-domain -p yulius-harness -p yulius-app -p yulius --all-targets -- -D
  warnings`: clean; `scripts/dev quick`: passed.
- Review round 1 (Fable 5.2) approved `d6e0c652e` with four minors and three nits. Answered:
  the pool's family (`capacity.rs` `task_model`; the goal-mode test failed with `left: Some(Any),
  right: Some(Opus)` before it), the launch notice (the same test; with the filter in
  `supervisor.rs` mutated to `true` it failed with `["Opus 5.5 is used up on every subscription:
  this session runs on Gpt 6.2.astra."]`), the mock's `model_default` (the e2e composer test
  failed with "gpt-6.2-astra · High effort" before it), the `POST …/tasks` row and the line
  wraps, and this Context. Left: the mock's duplicate effort arm (its line is over 200 bytes, which
  a change after the approval may not touch), and the mock's Run now and Superagent tasks still
  getting `claude-opus-4` (pre-existing, mock-only, Antigravity alike; the server is right).
- Review round 2 (Fable 5.2) approved `356719673` with one minor and two nits, answered: the
  Antigravity goal-mode test now checks the pool family too (with the Antigravity arm of
  `task_model` set to `None` it failed with `left: Some(Any), right: Some(Opus)`), this Evidence's
  first bullet, and the e2e file's `Mocked.tasks` type.
- `npx e2e run` of `codex`, `antigravity`, `responsive`, `model-fallback`, `claude-code` and
  `steps`: 71/71 passed.
- Security scan: not run. Yulius CI's `security` job was skipped (exit 78: Vidoc refused the
  instance key, HTTP 401), not caused by the change.
- Not tested: a real Codex run on a real OpenAI account (no Codex account is connected to this
  sandbox); the model's availability to the pinned 0.169.0 rests on the catalog's
  `minimal_client_version`.
