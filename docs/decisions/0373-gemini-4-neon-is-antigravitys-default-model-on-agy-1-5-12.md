# 0373. Gemini 4 Neon is Antigravity's default model, on pinned agy 1.5.12

## Status

Accepted on 2026-11-19 (`F 2026-11-19 Neon`: the person's request in task #501, "this is a test
task, confirm everything work on gemini. if so, add Gemini 4 Neon to the models and make Neon the
default model"). Specified in
[SPEC-519](../../.ai/specs/SPEC-519-this-is-a-test-task-confirm-everything-w.md). Amends the
**Model** and **CLI** parts of
[ADR-0361](0361-antigravity-is-a-third-harness-picked-at-random-by-default.md) (SPEC-505 AGY-002,
AGY-007, AGY-010); unnamed harness selection continues to follow
[ADR-0377](0377-random-draws-uniformly-across-harnesses-with-available-subscriptions.md) (SPEC-520,
which superseded [ADR-0368](0368-a-tasks-agent-is-set-never-drawn.md)).

## Context

- Task #501 ran live on `Harness::Antigravity` on a Google sign-in subscription inside a contained
  sandbox, confirming that the sealed Google sign-in token file, the run's bound `.gemini` home,
  shared `brain/` and `conversations/`, `SessionStart` briefing injection, `PreToolUse` shell guard
  and the `yulius mcp` stdio bridge work end to end.
- On the live Google sign-in subscription, `antigravity models` lists `gemini-4-neon-{low,medium,high,xhigh}`
  ("Gemini 4 Neon"). However, on the pinned `agy` 1.5.10, launching with a `GEMINI_API_KEY`
  subscription (`modelProvider: "gemini"`) and `--model gemini-4-neon --effort high` fails at
  startup with exit code 1 (`--effort is not supported for model "gemini-4-neon"`).
- Antigravity CLI 1.5.12 (`1.5.12-5281398513336320`, published 2026-11-18) adds `gemini-4-neon`
  (`low`, `medium`, `high`, `xhigh`) for `GEMINI_API_KEY` connections and keeps the same single
  binary layout, hooks, transcript format, `AGY_CLI_DISABLE_AUTO_UPDATE=true` variable and
  `oauth-token` file path.
- Earlier on 2026-11-19, the same person set Codex's default model (`gpt-6.2-astra` at `high`,
  ADR-0366) and fixed Yulius's unnamed-agent default to Claude Code (`claude-opus-5-5`, ADR-0368)
  so tasks without an explicit agent or model never switch to Gemini unexpectedly.

## Decision

- **Default Antigravity model.** `DEFAULT_ANTIGRAVITY_MODEL = "gemini-4-neon"` and
  `DEFAULT_ANTIGRAVITY_EFFORT = "high"` in `yulius-domain::harness`. An Antigravity task that names
  no model launches with `--model gemini-4-neon`, records `gemini-4-neon` on its attempt and shows
  `gemini-4-neon` (`model_default: true`) at `high` effort in its detail view.
- **Offered Google models.** `models.js` lists `gemini-4-neon` ("Gemini 4 Neon") first under Google
  (`harness: 'antigravity'`) and keeps `gemini-4-carbon` ("Gemini 4 Carbon") second; the browser
  mock defaults an Antigravity task without `model` to `gemini-4-neon` at `high`. The composer's
  unnamed `Random` item continues to draw across available harnesses (`ADR-0377`, naming
  `Antigravity (Gemini 4 Neon)` and offering the common `EXTRA_HIGH` tiers).
- **Pinned Antigravity CLI.** `yulius_domain::agent_cli::antigravity` pins `1.5.12`
  (`1.5.12-5281398513336320`) with the `sha512` digests from Google's `linux_amd64` and
  `linux_arm64` manifests, installed into `<data>/antigravity-cli/1.5.12-<platform>`.

## Alternatives

- **Make Gemini 4 Neon Yulius's sole unnamed default (routing every unnamed task to Antigravity).**
  Rejected: unnamed harness selection is governed separately (`ADR-0368`, superseded by `ADR-0377`'s
  uniform draw across connected harnesses), and forcing every unnamed task onto Gemini would break
  instances that have only Claude or Codex subscriptions connected.
- **Replace `Gemini 4 Carbon` in `models.js` instead of keeping it second.** Rejected: the request
  asks to add Gemini 4 Neon and make it the default, and keeping Carbon preserves explicit selection
  and browsers with `yulius.composer.model = "gemini-4-carbon"`.
- **Keep `agy` pinned at `1.5.10`.** Rejected: unlike Codex 0.169.0 running `gpt-6.2-astra` on
  fallback metadata (ADR-0366), `agy` 1.5.10 exits with code 1 at launch when given
  `--model gemini-4-neon --effort high` on a `GEMINI_API_KEY` subscription.

## Consequences

- Antigravity tasks without an explicit model (including tasks drawn to Antigravity by `Random`) run
  `gemini-4-neon` at `high` effort on both Google sign-in and `GEMINI_API_KEY` subscriptions;
  existing Antigravity tasks without a pinned model resume on `gemini-4-neon` at their next launch.
- Both `Gemini 4 Neon` and `Gemini 4 Carbon` can be picked under Google in the composer and
  Settings; unnamed `Random` tasks draw across available harnesses (`ADR-0377`).
- First Antigravity start or server warm-up after the update downloads `agy` 1.5.12 (~100 MB) and
  prunes `1.5.10` once no running process holds it.
