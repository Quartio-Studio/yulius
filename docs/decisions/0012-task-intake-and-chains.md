# 0012. Task intake with Jev and chained tasks

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-16); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Mixed workstreams and mid-task prompts need deterministic handling.

## Decision

TypeSafe's Jev decides on splits and on whether a prompt is context, new scope, dropped scope or a redirect; a headless Claude run proposes the split; the `UserPromptSubmit` hook holds non-context prompts; tasks can wait as `chained`.

## Alternatives

An LLM in the hook; classifying every prompt; one task per listed item.

## Consequences

Deterministic and optional (no key, no checks); one click overrides a wrong hold.
