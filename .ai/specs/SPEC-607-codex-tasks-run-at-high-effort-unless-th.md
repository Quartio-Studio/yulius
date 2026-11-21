---
yulius-task: 8acbc127-7f7e-4736-84eb-46f1b6476024
status: done
---

# SPEC-607 — Codex tasks inherit high effort

## Goal

Use High for ordinary Codex work and let people deliberately select stronger effort
for a task. This implements the owner's decision of 2026-11-21.

## Context

[SPEC-561](SPEC-561-default-astra-6-2-to-xhigh-for-new-work.md) made Astra inherit
`xhigh` for a comparison. The owner's 18–21 November sample covers 115 merged Astra
tasks that each ran wholly at one effort. On one Yulius machine, 35 High tasks and 12
Extra high tasks had median active times of 50.8 and 50.6 minutes. Extra high used
79k versus 35k output tokens per task and 37M versus 30M cached tokens. A second PR
was needed for 2 of 35 and 1 of 12 tasks, respectively. Across projects, Extra high
used about 1.3 times the output tokens. Six subscriptions used their weekly allowances
in about two and a half days.

The pinned Codex catalog lists both `xhigh` and `max` for GPT-6.2 Astra and Sol.
Yulius stores the task effort as an option and resolves it when planning a process.

## Requirements

- **CEF-001** A fresh or resumed Codex process receives the task's explicit effort
  or inherits `high` for every model, including implicitly selected Astra. Existing
  tasks keep their stored effort option and resolve it at the next process start.
- **CEF-002** The composer and Settings describe inherited High and offer explicit
  Extra high and Max for Astra and Sol. Random also describes High because each
  eligible default model inherits it. The task creation and model-edit tools describe
  the default and accept the explicit choices. Default clears a model edit's override.
- **CEF-003** Task details and the browser mock distinguish inherited High from
  explicitly chosen High and report explicit Extra high and Max without a default marker.
- **CEF-004** Codex review sessions continue to request GPT-6.2 Astra at High.
  Subscription, service tier, current processes and historical turn records keep their
  existing behavior.

## Plan

- [x] Use high for inherited Codex effort on launch and resume
- [x] Offer explicit Codex max in the composer and task model tool
- [x] Document the default and verify the affected flows
- [x] Get independent approval for the delivery

## Acceptance criteria

- **AC-1** Fresh and resumed Codex argv contain High for omitted effort on implicit
  Astra, named Astra and named Sol. Explicit `xhigh` and `max` appear in argv in
  both modes (CEF-001).
- **AC-2** Browser controls show High as the default for Astra and Sol and submit
  explicit Max when selected. The model-edit tool stores Max and can clear the override
  for the next launch (CEF-002).
- **AC-3** Details show High effort (default) for an omitted effort. Explicit choices
  appear without the default marker, including explicitly selected High (CEF-003).
- **AC-4** Existing review and launch contracts still pass (CEF-004).

## Test plan

Exercise both launch modes in the argv regression and confirm the fresh and resumed
fake-process paths. Test HTTP details and the remote tool's edit/reset. Exercise
composer and Settings through Chromium against the mock and run the browser suite.
Record red and green results and deliver on the Yulius merge gate.

## Decisions

Resolve the optional override with one harness-wide fallback. Storing a resolved High
would erase the distinction between an inherited and deliberate choice and would
require a migration for existing tasks. The existing process planner already passes
the resolved setting to Codex on every launch and owns that boundary. The decision
replaces the Astra effort default of SPEC-561. [ADR-0444](../../docs/decisions/0444-codex-tasks-inherit-high-effort-on-every-model.md)

## Evidence

- Before the resolver change, `scripts/cargo test -p yulius-harness --test launch
  codex_resolves_effort_for_new_and_resumed_tasks` failed on omitted effort.
  Fresh Astra argv contained `model_reasoning_effort="xhigh"`. With the fix,
  `scripts/cargo test -p yulius-harness --test launch` passed all 27 tests,
  including the new/resumed argument matrix and the subscription-specific tier.
- Against the old browser assets, the Chromium checks observed the Extra high
  composer and Settings labels and the mock recorded `xhigh` for an untouched task.
- Before the `task_model` descriptor change, the remote CLI test failed because
  the tool described no Codex default. The same test checks explicit Extra high
  and Max and clears the override through the normal remote MCP endpoint.
- `npm ci --ignore-scripts` and `npx e2e run` in `e2e/` completed. Chromium
  passed 1,319 tests and skipped one credential-dependent test. The Codex and
  model-choice tests cover omitted and explicit High, explicit Extra high, and
  Max on both GPT-6.2 models. The Superagent tests cover inherited High.
- `scripts/cargo build -p fake-agent` and
  `scripts/cargo test -p yulius -p yulius-app -p yulius-domain --lib --test main`
  with the nine Codex effort, task creation, MCP schema and reviewer filters
  passed. The native process fixture captured High on fresh and resumed launches
  of an Astra task with omitted effort, and Max after a running task was edited.
  The reviewer briefing test still names GPT-6.2 Astra at High.
- `python3 scripts/decision_slots.py`, `python3 scripts/check_repository.py`
  and `scripts/dev quick` passed.
- The first review of `dd1b3e684` requested an accurate Random effort label.
  `npx e2e run model-choices --grep 'Settings preserves'` failed before the fix.
  The menu displayed “Default Depends on the drawn model” where High was expected.
  With the shared label fixed, all nine tests in `npx e2e run model-choices`
  passed. Random still offers Low through Extra high as explicit choices.
- The second independent review approved `ce2b543d720a0a370602331f70078a1a71570f3a`
  after exercising the shared menus and tracing the fresh and resumed launch paths.
- `scripts/dev security` reported that Vidoc was disabled for the project.
