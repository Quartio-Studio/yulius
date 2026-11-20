# 0397. GPT models imply Codex

## Status

Accepted for SPEC-560.

## Context

The composer can name a harness alongside a model. HTTP and MCP clients can also supply a model alone, including one copied from a Codex task.

## Decision

A `gpt-*` model selects Codex by the same rule that selects Antigravity for `gemini-*`. Check known foreign provider prefixes against an explicitly selected or existing harness before storing the settings.

## Alternatives

Refusing every model-only GPT request would require callers to repeat information carried by the model ID. Requiring a catalog match would exclude moving aliases and custom model IDs accepted by the harness.

## Consequences

GPT requests reach the Codex account gate. Unqualified aliases still imply Claude Code, explicit Codex retains support for unqualified model names, and requests without a model retain their existing harness selection.
