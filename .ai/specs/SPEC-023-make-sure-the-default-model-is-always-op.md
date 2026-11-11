---
yulius-task: 8e5a1bae-c2e1-4fcc-bf3b-7ae8518fb149
status: done
---

# SPEC-023: tasks run Fable 5.2 at high effort by default

Status: done
Date: 2026-10-12
Owner: Yulius task #21
Scope authorization: user request (task #21): "make sure the default model is always opus 5.5 and
default effort is high"; amended 2026-11-10 (person in the Superagent): "make fable 5.2 effort
max default model" (ADR-0289); amended 2026-11-11 (the person's decision, SPEC-462): "the default
effort is high. Not max, not xhigh" (ADR-0295).

## Goal

A task that names no model runs Fable 5.2, and a task that names no effort runs at high effort,
whatever Claude Code's aliases or a subscription's defaults say.

## Context

SPEC-011 (HRN-012) launched a task without a model with Claude Code's `opus` alias, which follows
each new Opus release, and sent no `--effort`, so Claude Code chose the effort itself. The composer
and Settings call "Default" "Claude Code's default" and "Claude Code decides". Resumes, account
switches and the composer's "Default" all go through the one launch plan
(`yulius_harness::launch::plan_launch`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DEF-001 | A launch without a model passes `--model claude-fable-5-2` (supersedes the `opus` alias of HRN-012; Opus 5.5 until 2026-11-10). | `launch.rs` test `claude_resume_uncontained_never_skips_permissions` |
| DEF-002 | A launch without an effort passes `--effort high` (max on 2026-11-10 only; SPEC-462), for the default and any named model, except a Haiku model, which takes no effort level; a named effort is kept. | `launch.rs` test `a_named_model_without_effort_runs_at_high_effort_unless_it_takes_none` |
| DEF-004 | The Cockpit (SPEC-021), a person's plain Claude Code session, also starts with `--model claude-fable-5-2 --effort high`; `/model` and `/effort` change it within the session; no per-model weekly cap chooses its subscription (Fable has none Yulius reads). | `launch.rs` test `a_cockpit_is_plain_claude_code_with_the_token_only_in_its_environment`, `cockpit.rs` test `a_spent_opus_cap_does_not_keep_the_cockpit_off_a_subscription` |
| DEF-003 | The composer, Settings and the task's model chip describe "Default" as Fable 5.2 and High; the model list offers Fable 5.2. | `models.js`, `taskview.js`, `ui_contract.rs` test `the_ui_describes_default_as_the_model_and_effort_a_launch_uses` |

## Plan

- [x] Launches default to claude-opus-5-5 at effort high, proven by launch tests
- [x] UI labels, spec, ADR and docs say Opus 5.5 / High
- [x] Every gate green and the changes log updated

## Acceptance criteria

- A new task, a resumed task and a task moved to another subscription with no model or effort run
  `claude --model claude-fable-5-2 --effort high`.
- A task on Haiku 4.5 without an effort gets no `--effort`.
- No per-model weekly cap binds the default (rotation reads `DEFAULT_CLAUDE_MODEL`, a Fable id,
  as `ModelFamily::Any`); a task that names an Opus or Sonnet model keeps its cap.

## Test plan

- `cargo test -p yulius-harness --test launch`: the argv of a launch without model or effort, a
  Sonnet model without effort, a Haiku model without effort and a named effort.
- Every gate with `scripts/dev check`.

## Decisions

- Pin the full model id rather than the `opus` alias, and apply the default at launch rather than
  storing it on the task (ADR-0024).
- Haiku is recognised by name, as `ModelFamily::of` already recognises Opus and Sonnet; the UI's
  model list (`models.js`, `effort: false`) stays the place that lists models.
- The Cockpit, merged into `dev` while this task was in review, started without `--model` (the
  subscription's default, Sonnet). "Always" includes it, so it gets the same flags; it stays
  plain otherwise (no settings, MCP or approval bypass).
- The split proposer (`SPLIT_MODEL`) is a short one-off request, not a task, and keeps its model.

## Evidence

- `cargo test -p yulius-harness --test launch`: both launch tests failed before the change
  (`--model opus`, no `--effort`) and pass after it.
- `scripts/dev check`: green (the repository check first refused the product name in UI strings;
  the strings were reworded).
- `scripts/dev lint`: green after `cargo fmt`.
- `scripts/dev test`: 1150 tests run, 1150 passed, 16 skipped.
- After merging `dev` with the Cockpit (SPEC-021): the Cockpit test first failed on the new
  `--model claude-opus-5-5 --effort high` expectation and passes with `plan_cockpit` sending them.
  The Cockpit integration tests (`crates/yulius/tests/cockpit.rs`) failed the same way (3 tests)
  and pass with their expected arguments updated. Then `scripts/dev check` and `lint` green,
  `scripts/dev test` 1161 run, 1161 passed, 16 skipped; e2e 32 files, 139 tests passed.
- Review of that head: the Cockpit chose its subscription without the Opus cap.
  `the_cockpit_skips_a_subscription_whose_opus_cap_is_spent` failed (it took the spent
  subscription's token) and passes once `pool_account` takes the run's model family.
- `cd e2e && npx e2e run`: 31 files, 130 tests passed, 1 skipped.
- Not exercised here: a live Claude Code run accepting `--model claude-opus-5-5 --effort high`
  (this session itself runs `claude-opus-5-5`).

## Amendment 2026-11-10: Fable 5.2 at max effort

- Anthropic's models overview lists Claude Fable 5.2 (`claude-fable-5-2`, $8 / $40 per MTok, cache
  reads $0.20, effort `low` to `max`). The constants, the price table, the UI labels and the tests
  above moved to it; Fable 5.1 stays in the model list.
- Claude Code 2.1.311 does not know the id yet: `claude -p --model claude-fable-5-2` logs
  `unrecognized_model` and still sends it. Not exercised here: a live run on a subscription (this
  sandbox's `claude` is not signed in).
- A subscription without Fable access cannot run a default task; picking Opus 5.5 in the composer
  or Settings still works.

## Amendment 2026-11-11: high effort again

- The person decided the default effort is `high` ("Not max, not xhigh"); the default model stays
  `claude-fable-5-2` ([SPEC-462](SPEC-462-default-effort-is-high-the-default-model.md), ADR-0295).
  `DEFAULT_CLAUDE_EFFORT`, the effort menu's "Default" and the tests above moved back to it. A task
  that chose `max` keeps it; a task or Cockpit that names no effort takes `high` at its next start.
- The MCP tool `task_create` now builds its "default" wording from the two constants, and a test
  holds the composer's and Settings' "Default" labels to them.
