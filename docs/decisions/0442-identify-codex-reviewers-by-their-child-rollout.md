# 0442. Identify Codex reviewers by their child rollout

## Status

Accepted for [SPEC-605](../../.ai/specs/SPEC-605-record-the-review-rounds-of-codex-tasks.md).

## Context

A Codex collaboration spawn's lifecycle hooks can identify the child thread and its
active model. The hook's agent type is normally `default`, and the spawn's prompt
can be encrypted in the parent rollout. The child's rollout metadata includes its
task path, parent thread and launch commit. [Codex 0.170.0 hook runtime](https://github.com/openai/codex/blob/rust-v0.170.0/codex-rs/core/src/hook_runtime.rs)

## Decision

The briefing asks for a fresh Codex child named `yulius_review_<n>` with a distinct
positive integer for each launch. On `SubagentStart`, Yulius verifies the child's
name and parent in the header of the rollout inside the task's sandbox. It uses the
launch commit as the reviewed head and assigns the next number in the shared review
store. A corresponding `SubagentStop` with a verdict settles the same record.

## Alternatives

A custom role would offer a hook matcher but would depend on a role parameter the
managed collaboration spawn does not expose. Looking for verdicts in the parent's
messages would have no reliable way to identify a review at launch or verify which
model answered.

## Consequences

Codex rounds feed the same delivery and dashboard data as Claude Code rounds. Older
reviewers have no reliable launch marker. Antigravity's swarm uses a separate lifecycle.
