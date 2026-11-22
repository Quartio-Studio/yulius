# 0457. New tasks default to Claude Code on Opus 5.5

## Status

Accepted on 2026-11-22 for task #604 (the person's request: "Make opus and claude default
model"). Specified in [SPEC-620](../../.ai/specs/SPEC-620-make-opus-and-claude-default-model.md).
Supersedes [ADR-0422](0422-cli-and-superagent-task-creation-defaults-to-codex.md) and
[ADR-0426](0426-the-new-task-composer-defaults-to-codex.md).

## Context

ADR-0422 made `task_create` choose Codex when the CLI, a Superagent or an approved cross-sandbox
request omits both harness and model. ADR-0426 made the composer and Settings start on Codex
default in a browser with no saved model. Claude Code's own defaults are Opus 5.5
(`DEFAULT_CLAUDE_MODEL`) at high effort (`DEFAULT_CLAUDE_EFFORT`).

## Decision

- `task_create` from a caller without a task, with neither harness nor model, creates a Claude
  Code task (`harness_choice: chosen`) with model and effort unset, so it runs Opus 5.5 at high
  and reports both as inherited.
- In a browser with no saved model, an unreadable one or one no longer offered, the composer and
  Settings select Opus 5.5, the first Anthropic model, which a contract test keeps equal to
  `DEFAULT_CLAUDE_MODEL`. A saved model, Codex default included, and a saved Random stay.
- A named harness, a model's harness, explicit Random, a running task's follow-up, automations
  (`auto`) and the HTTP API's absent harness (Random's wire form) keep their rules.

## Alternatives

- **A harness-only "Claude default" entry in the browser**: lists Opus 5.5 twice and breaks the
  menu's one-to-one list with the offered Claude models.
- **Claude for the HTTP API's absent harness too**: the composer's Random sends no fields, so it
  would stop drawing.
- **Writing the new default into saved browser preferences**: would overwrite people's deliberate
  Codex or Random choices.

## Consequences

Untouched tasks need a usable Claude subscription; a project with only Codex or Antigravity
subscriptions names Claude when such a task starts, and the composer offers Connect Claude with
no account. Codex and Antigravity remain one pick away, and Random still draws when chosen.
