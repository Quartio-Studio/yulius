# 0446. Keep GitHub availability separate from task state

## Status

Accepted for SPEC-608. Extends ADR-0424.

## Context

An unavailable PR read created task attention notices and suggested restarting finished sessions.

## Decision

Task displays retain observed PR evidence during an unavailable read. Delivery decisions still require fresh verification. The instance exposes the GitHub App's active reset deadline to its views.

## Alternatives

A per-task outage notice would ask every owner to act on a shared condition. Marking all tasks Done would hide genuine failures and open work.

## Consequences

The displayed evidence can predate the outage. The next successful observation can change it. The reset time expires without a successful GitHub request.
