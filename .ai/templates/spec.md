# SPEC-NNN: title

Status: proposed
Date: YYYY-MM-DD
Owner: responsible person or team
Scope authorization: user request / issue / accepted decision

## Problem and evidence

User outcome, current behavior, and cited observations. Distinguish facts from assumptions.

## Scope

In scope, explicit non-goals, and dependencies on unresolved product decisions.

## Requirements

| ID | Observable requirement | Evidence needed |
| --- | --- | --- |
| R-001 | Concrete behavior | Named test or manual verification |

## Design and contracts

Affected modules; inputs/outputs; state transitions; invariants; versioning;
authorization; data ownership; retries, limits, cancellation and recovery.
Only include sections that affect this change. For each nonterminal state name
its owner, exit trigger and behavior when that trigger never arrives.

## Failure and compatibility

Partial failure, malformed input, process exit, persistence recovery and existing
behavior that must survive. Migration/rollback plan when durable data changes.

## Delivery and acceptance

Complete vertical slices, commands and behavioral scenarios tied to requirement IDs.
Include negative cases, fixtures, expected outcomes and manual UI evidence as needed.

## Open decisions

Each choice: owner, proposed default, evidence, blocking/nonblocking and revisit trigger.

## Implementation evidence

Pending until actual commands and behaviors have been observed. A skipped gate is
not a passing gate; specs and test names are not substitutes for executed results.

## Changelog

- YYYY-MM-DD: initial proposal.
