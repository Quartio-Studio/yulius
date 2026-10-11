# 0004. SQLite in WAL mode as the only database

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-04); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Single-VM product with a handful of users and simple backup needs.

## Decision

rusqlite (bundled) in WAL mode, one write connection behind an async actor, versioned SQL migrations via `user_version`.

## Alternatives

Postgres; sled; plain files.

## Consequences

Backup is `VACUUM INTO`; write throughput is ample for the target scale; no external service.
