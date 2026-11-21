# 0366. A Codex task runs GPT-6.2 Astra at high effort by default

## Status

Accepted on 2026-11-19 (the person's request in task #493: "default model for codex should be
astra 6.2 effort high"). Specified in
[SPEC-511](../../.ai/specs/SPEC-511-default-model-for-codex-should-be-astra.md). Amends the model
part of [ADR-0355](0355-codex-returns-as-a-second-harness-with-a-pinned-cli.md) (SPEC-503
CDX-013: a Codex task without a model ran its account's default).

[ADR-0398](0398-astra-6-2-inherits-extra-high-effort.md) changed the effort for the
Astra comparison. [ADR-0444](0444-codex-tasks-inherit-high-effort-on-every-model.md)
defines the current High default for all Codex models.

## Context

- A Codex task without a model was launched with no `-m` and no `model_reasoning_effort`, so the
  account or the CLI chose both, and the run recorded no model.
- Codex's bundled model catalog (release 0.170.0) names the model `gpt-6.2-astra` ("GPT-6.2-Astra",
  efforts `low` to `ultra`, default `low`) for clients from 0.166.0. The pinned 0.169.0 does not
  bundle it: run against a stub API it sends `gpt-6.2-astra` at `high`; with an API-key sign-in it
  runs on fallback metadata (a warning, 0.169.0's generic request shape), with a ChatGPT sign-in
  on the entry the server's model list gives.
- Antigravity already has a server-side default model and effort (ADR-0361), filled in by the
  launch plan and recorded with the run.

## Decision

- `DEFAULT_CODEX_MODEL = "gpt-6.2-astra"` and `DEFAULT_CODEX_EFFORT = "high"` in
  `yulius-domain::harness`. A Codex launch without a model passes `-m gpt-6.2-astra`, without an
  effort `-c model_reasoning_effort="high"`; a task's own model or effort wins.
- The run records the model it was launched with, and the task view names it and the effort, as
  for Antigravity.
- The composer keeps its single "Codex" entry without a model id (a browser's saved choice keeps
  working, the server holds the only copy of the default) and says what it runs by default.

## Alternatives

- **The account's default (as before).** Rejected: the person asked for a fixed model and effort,
  and an unrecorded model cannot be compared between runs.
- **A model entry `gpt-6.2-astra` in the composer.** Rejected: it orphans the saved `codex`
  choice and puts a second copy of the default in the page.
- **Bumping the pinned Codex CLI to 0.170.0 first.** Not needed: 0.169.0 runs the model; a bump
  is its own change.

## Consequences

- Codex runs use GPT-6.2 Astra at high effort unless a task names another; usage per run is
  attributed to a named model.
- API-key Codex subscriptions run the default on fallback metadata until the pinned CLI bundles
  the model; a bump of the pin to such a release (0.170.0 or later) is its own change.
- A Codex task that started before this on its account's default resumes on GPT-6.2 Astra at
  high effort.
- When OpenAI retires the model, the constant changes in one place.
