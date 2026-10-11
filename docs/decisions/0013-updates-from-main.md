# 0013. Self-updates from main through a root helper

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-18); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Updates must not lose running work.

## Decision

Build while tasks keep working, drain at pause points, install through a oneshot root helper started by a systemd path unit with backup and rollback, then resume staggered.

## Alternatives

Pausing before building; polkit/sudo; auto-update.

## Consequences

Tasks pause about a minute; a failed build touches nothing; the helper runs code from `main` as root.
