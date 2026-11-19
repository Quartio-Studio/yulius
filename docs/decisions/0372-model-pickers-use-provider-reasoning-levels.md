# 0372. Model pickers use the provider's reasoning levels

## Status

Accepted for [SPEC-521](../../.ai/specs/SPEC-521-i-should-be-able-to-selected-every-model.md).

## Context

The composer and browser Settings share model preferences. Harnesses support different effort levels, and models within one harness can differ as well.

## Decision

Keep one browser catalog of selectable coding models and supported reasoning levels, sourced from the public Antigravity model selector and the visible entries in the stable Codex CLI catalog compatible with the pinned client. Use the catalog for both controls, preference restoration and submission. Preserve compatible effort when a model changes. Fall back to the server default for an incompatible effort. Keep the existing Codex default choice for people who want to follow the server.

Ship Antigravity 1.5.12 and Codex 0.170.0 with the matching catalog. Antigravity 1.5.10 rejects Neon’s effort flag. Codex 0.169.0 uses fallback metadata for GPT-6.2 when the account supplies no model catalog. The existing installer verifies both supported Linux packages for each CLI.

## Alternatives

A live account lookup would require choosing an account before the pool places the task. A shared list of tiers offers invalid combinations on several models.

## Consequences

The picker works offline and on the mock. When the supported CLIs or provider catalogs change, check model IDs and tiers together. A listed model still requires a subscription that can run it.
