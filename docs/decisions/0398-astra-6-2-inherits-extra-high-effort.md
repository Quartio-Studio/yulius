# 0398. GPT-6.2 Astra inherits extra high effort

## Status

Superseded by [ADR-0444](0444-codex-tasks-inherit-high-effort-on-every-model.md) on 2026-11-21. Originally accepted for task #554 on 2026-11-20. It amended the effort rule in
[ADR-0366](0366-a-codex-task-runs-gpt-6-2-astra-at-high-effort-by-default.md).
[SPEC-561](../../.ai/specs/SPEC-561-default-astra-6-2-to-xhigh-for-new-work.md)

## Context

The owner wants to compare speed and quality for ordinary Astra tasks with explicit High
controls. SPEC-511 applied one fallback effort to all Codex tasks. A task already stores an
optional effort and the Codex rollout records the model and effort in each turn context.

## Decision

Use the task's explicit effort first. When it is absent, resolve the effective model on each
Codex process start. Exactly `gpt-6.2-astra` gets `xhigh` and other Codex models get `high`.
Expose `effort_default` alongside the effective effort in task details.

## Alternatives

Raising the harness-wide fallback would also affect Sol and older Astra models. Writing the
resolved value into each new task would discard whether the owner chose that effort.

## Consequences

An existing task with omitted effort inherits the new setting at its next process start,
including resume. Its open process keeps its launch settings. To compare launches across a
release, pair the running Yulius build commit and task's stored choice with the model and
effort from the corresponding rollout turn context.
