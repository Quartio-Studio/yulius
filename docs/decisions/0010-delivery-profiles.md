# 0010. Delivery profiles in yulius.toml

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-11, ADR-19); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Product and Studio repositories use different branches; delivery should run autonomously.

## Decision

Profiles `standard`, `product` and `studio` in `yulius.toml`. One autonomous discipline: effort-weighted steps, specs only where they matter, independent review in a fresh session, merge on green, infra bootstrap without GitHub Actions.

## Alternatives

Hard-coded dev/main; a hard step gate; specs for everything.

## Consequences

Progress means something without ceremony; the harness checks what a machine can check and nudges the rest.
