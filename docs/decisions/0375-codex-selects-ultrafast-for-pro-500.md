# 0375. Codex selects Ultrafast for Pro 500

## Status

Accepted for task #505 in [SPEC-523](../../.ai/specs/SPEC-523-make-ultrafast-default-speed-in-codex-if.md).

## Context

A Codex task can move between subscriptions. Each launch already receives the assigned
subscription's sign-in. Codex calls the individual plan with Astra Ultrafast Pro 500 and
represents it as `promax`.

## Decision

Set the top-level Codex home default `service_tier = "ultrafast"` when the assigned ChatGPT
credential's ID token identifies Pro 500. Codex resolves the tier against the model's catalog
and feature settings. Other credentials receive the usual configuration.

## Alternatives

Subscription labels can be edited and need not identify the plan. Caching a task's speed would
carry one subscription's default to the next. A launch-time usage request would introduce a
network dependency for information already present in the sign-in.

## Consequences

The next launch uses the plan in the latest stored sign-in. Codex can apply project and session
choices above the home default. An absent or unreadable plan claim leaves speed selection to
Codex. OpenAI enforces the actual entitlement.
