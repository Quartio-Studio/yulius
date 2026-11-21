---
yulius-task: 3367da21-6523-4d0d-93c0-ce31e5f50549
status: done
---

# SPEC-561 — GPT-6.2 Astra inherits extra high effort

[SPEC-607](SPEC-607-codex-tasks-run-at-high-effort-unless-th.md) supersedes the inherited effort rule. This spec records the comparison release.

## Goal

Give the owner an identifiable release in which ordinary GPT-6.2 Astra tasks inherit
`xhigh` and an explicit `high` task can serve as the comparison control.

## Context

Task #493 and [SPEC-511](SPEC-511-default-model-for-codex-should-be-astra.md) introduced a
single Codex effort default. API creation stores an optional effort. The supervisor passes
that option into the shared new/resume launch plan. Task details independently resolve the
option and MCP task briefs expose the stored choice. The composer and mock both describe High
as the default. The subscription and service tier are selected independently of effort.

## Requirements

- AST-001. A Codex process starting on exactly `gpt-6.2-astra`, including by default model
  selection, inherits `xhigh`. Other Codex models inherit `high`. Every explicit task effort
  takes precedence, including `high` and `xhigh`.
- AST-002. Apply this rule on fresh launches and process resumes. Preserve running turns,
  historical metrics, the reviewer's explicit High, subscriptions and service tier.
- AST-003. Preserve the optional choice at creation. Show the effective setting and whether
  the effort is inherited in task details. The composer, Settings and mock reflect the model
  selection, including Random where the model has yet to be drawn.
- AST-004. Identify the PR and integrated commit. After the normal platform release, verify
  the loaded Yulius build and the first turn's model and effort on an ordinary newly created
  Astra task with omitted effort. The operator owns matched measurements.

## Plan

- [x] Specify the Astra default and the release comparison
- [x] Resolve inherited effort for fresh and resumed Codex launches
- [x] Distinguish inherited effort in task details and creation controls
- [x] Verify the change and obtain independent review
- [x] Package the reviewed change for delivery to dev on green CI

## Acceptance criteria

- New and resume argv contain one effective model and one effort for implicit Astra,
  named Astra, explicit High, and a different model. The existing service-tier tests still pass.
- An omitted effort and explicit High survive API creation distinctly. The detail view shows
  Extra high effort (default) versus High effort and can distinguish explicit Extra high too.
- A normal released launch is attributable by task, UTC launch time, build commit and rollout
  turn context. If the release or eligible task is unavailable at dev merge, record the gap.

## Test plan

Exercise launch argv in both modes, app creation/detail and fake process launch, the UI contract
and browser interactions. Run quick checks, browser tests and the existing CI gate. Use only
the model and effort from real turn context when checking the released build.

For the live check, read `GET /api/update` field `current.commit` and confirm that the dev
squash is its ancestor. Select a new, ordinary Codex task created after that build went live.
Record its number, first attempt start in UTC, omitted effort at creation and the first
Codex rollout `turn_context.payload.model` and `.effort`. Expected values are `gpt-6.2-astra`
and `xhigh`. Task details alone describe a launch setting and do not prove what the CLI used.

## Decisions

Resolve the optional task override in the domain by effective model at process start. The
current task settings describe its next launch. Existing rollout records describe actual turns.
Reuse the nullable effort for provenance and the existing release/build identifiers for the
comparison. No migration or new measurement infrastructure is required.

## Evidence

- Before the launch change, `codex_resolves_effort_for_new_and_resumed_tasks` failed on the
  first case with the actual argument `model_reasoning_effort="high"` and expected `xhigh`.
- Before the UI change, the composer test observed “GPT-6.2 Astra at high effort by default”.
  The updated Chromium Codex suite passed all 14 tests, including the explicit High control.
- The running task for this implementation predates the release and was created with explicit
  XHigh. It cannot supply post-release evidence of an inherited launch.
- HTTP creation/detail failed against the old application with `high` instead of `xhigh`.
  All 27 launch-plan tests passed after the change, including subscription-specific Ultrafast.
- The independent review of `b93d80c00` requested two changes. The fake-process assertion
  needed the production plan's arguments. The Superagent mock created a Codex task with
  `claude-opus-4` and `default` effort. The approval regression reproduced that mock result.
- The Chromium suite finished with 1,168 passed and one credential-dependent test skipped.
  After the mock fix, all 38 Codex, model-choice and Superagent request tests passed.
- Round two approved `1b8624094` with a minor mock issue. A literal model ID `codex` inherited
  Astra's effort. The Superagent test reproduced it. Only the composer's “Codex default” entry
  now resolves that alias to Astra. The mock checks effective model IDs exactly like the server.
- Without the fixture change, the fake-process test failed on the missing model argument.
  Its capture contained only the model and effort inputs (both absent) and the program name.
- The final four browser cases for the literal model, both composer choices and Settings passed.
  The initial mutation baseline passed 3,207 tests. No Rust mutation applies to the review fixes
  (the Rust edits are integration tests and their fixture).
- Final focused nextest run passed all 16 tests across the API, MCP descriptors, UI contracts
  and fake-process paths. The initial process planned `-m gpt-6.2-astra` and
  `model_reasoning_effort="xhigh"`. Its subscription rotation resumed the same session with
  `model_reasoning_effort="high"` after its owner set High during the first process.
- `scripts/dev mutants` reported 9 listed, 4 caught, 0 missed, 0 timed out, 5 unviable,
  0 not reached (bound 30 min, took 887 s). Quick checks passed. Vidoc was disabled.
- Task #554 and SPEC-561 identify the release comparison. The implementation commit is
  `b93d80c00`, the approved head is `1b8624094` and the final product change is `7332e8c86`.
  The PR identifies the dev squash.
  Before delivery, the instance reported version 0.1.0 at
  `1f5fc4f7500ec1c42228ab7abdb34e71a9fa733a`. Post-release turn-context verification needs
  a running build containing the squash and ordinary new work on it.
