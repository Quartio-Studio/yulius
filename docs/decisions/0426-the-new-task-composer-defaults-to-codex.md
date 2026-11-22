# 0426. The new-task composer defaults to Codex

## Status

Accepted for task #571. Amends the browser default in [ADR-0377](0377-random-draws-uniformly-across-harnesses-with-available-subscriptions.md). [SPEC-587](../../.ai/specs/SPEC-587-codex-should-be-default-for-example-bart.md)

[ADR-0457](0457-new-tasks-default-to-claude-code-on-opus-5-5.md) supersedes this decision: the browser now starts on Opus 5.5.

## Context

Settings and the composer share a browser preference. The previous reader treated an absent key and an explicit Random choice as the same value. Direct CLI and Superagent tasks already default to Codex.

## Decision

Use the existing Codex default option when the stored value is absent, unreadable or no longer offered. Preserve JSON null as the saved Random choice and restore recognized model IDs. A default task sends the Codex harness and inherits its launch settings.

## Alternatives

Writing Codex into every browser would overwrite deliberate choices. Changing the random draw at the server would also affect explicit Random requests and automation runs.

## Consequences

A browser with no saved model immediately offers Codex and starting a task requires a usable Codex subscription. Existing Random preferences keep drawing a harness per task. The browser needs no storage migration.
