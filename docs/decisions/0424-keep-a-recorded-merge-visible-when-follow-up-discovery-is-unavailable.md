# 0424. Keep a recorded merge visible when follow-up discovery is unavailable

## Status

Accepted for [SPEC-585](../../.ai/specs/SPEC-585-i-got-this-everywhere-fix.md). Refines the parked status display in [ADR-0412](0412-share-parked-pr-read-time-across-task-lists.md).

## Context

A parked task with a finished plan and a recorded merge acquired an actionable wait whenever GitHub could not list possible follow-up PRs inside the display budget. The suggested CI rerun concerned a PR that had already merged.

## Decision

Keep the recorded merge visible on a locally settled task when follow-up discovery is unavailable. Check the delivery and local work again before displaying Done. A known or newly discovered open follow-up requires its own verification. Cache the verified blocker and exclude directly confirmed closures even if the branch list lags. Keep the existing limit on overlapping overview reads and apply the same limit separately to each task branch. Dependency selection and release continue to require verification.

## Alternatives

Extending the request timeout would slow Home and would still produce false waits during outages. Skipping discovery for merged deliveries would hide externally opened follow-ups.

## Consequences

A later refresh can report an externally opened PR when GitHub recovers. Existing false wait episodes resolve on the next successful task status reconciliation, preserving their history.
