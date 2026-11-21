# 0422. CLI and Superagent task creation defaults to Codex

## Status

Accepted for task #567. Amends the direct task creation rule in [ADR-0377](0377-random-draws-uniformly-across-harnesses-with-available-subscriptions.md). [SPEC-583](../../.ai/specs/SPEC-583-cli-nad-superagents-should-spawn-tasks-i.md)

## Context

The CLI, the sandbox Superagent and the Global Superagent reach the same project task creation tool. An approved cross-sandbox request reaches it in the target project. These callers previously drew a random agent when they omitted both harness and model.

## Decision

Choose Codex in that case. Record the chosen harness and leave omitted model and effort unset. The assigned Codex subscription and the existing launch planner determine the service tier. A model still implies its harness. A running task's follow-up inherits its harness and explicit Random still draws.

## Alternatives

Supplying model, effort and tier at each caller would duplicate launch policy and record inherited settings as overrides. A fallback draw on a project without Codex could start the requested work on a different provider.

## Consequences

These tasks inherit the Codex defaults for model and effort, plus Ultrafast on a ChatGPT Pro 500 subscription. [ADR-0444](0444-codex-tasks-inherit-high-effort-on-every-model.md) defines the current effort default. Starting one requires an eligible Codex sign-in. A prepared task can acquire its subscription when started. Explicit choices and automation creation keep their existing precedence.
