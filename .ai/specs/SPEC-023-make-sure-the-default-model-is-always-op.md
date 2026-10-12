---
yulius-task: 8e5a1bae-c2e1-4fcc-bf3b-7ae8518fb149
status: done
---

# SPEC-023: tasks run Opus 5.5 at high effort by default

Status: done
Date: 2026-10-12
Owner: Yulius task #21
Scope authorization: user request (task #21): "make sure the default model is always opus 5.5 and
default effort is high".

## Goal

A task that names no model runs Opus 5.5, and a task that names no effort runs at high effort,
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
| DEF-001 | A launch without a model passes `--model claude-opus-5-5` (supersedes the `opus` alias of HRN-012). | `launch.rs` test `claude_resume_uncontained_never_skips_permissions` |
| DEF-002 | A launch without an effort passes `--effort high`, for the default and any named model, except a Haiku model, which takes no effort level; a named effort is kept. | `launch.rs` test `a_named_model_without_effort_runs_at_high_effort_unless_it_takes_none` |
| DEF-004 | The Cockpit (SPEC-021), a person's plain Claude Code session, also starts with `--model claude-opus-5-5 --effort high`; `/model` and `/effort` change it within the session; its subscription is chosen by the weekly Opus cap. | `launch.rs` test `a_cockpit_is_plain_claude_code_with_the_token_only_in_its_environment`, `cockpit.rs` test `the_cockpit_skips_a_subscription_whose_opus_cap_is_spent` |
| DEF-003 | The composer, Settings and the task's model chip describe "Default" as Opus 5.5 and High. | `models.js`, `taskview.js` |

## Plan

- [x] Launches default to claude-opus-5-5 at effort high, proven by launch tests
- [x] UI labels, spec, ADR and docs say Opus 5.5 / High
- [x] Every gate green and the changes log updated

## Acceptance criteria

- A new task, a resumed task and a task moved to another subscription with no model or effort run
  `claude --model claude-opus-5-5 --effort high`.
- A task on Haiku 4.5 without an effort gets no `--effort`.
- The default's weekly cap is the Opus one (rotation reads `DEFAULT_CLAUDE_MODEL`, which is still
  an Opus id).

## Test plan

- `cargo test -p yulius-harness --test launch`: the argv of a launch without model or effort, a
  Sonnet model without effort, a Haiku model without effort and a named effort.
- Every gate with `scripts/dev check`.

## Decisions

- Pin the full model id rather than the `opus` alias, and apply the default at launch rather than
  storing it on the task (ADR-0021).
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
