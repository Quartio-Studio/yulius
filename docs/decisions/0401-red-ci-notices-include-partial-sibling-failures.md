# 0401. Red CI notices include partial sibling failures

## Status

Accepted for SPEC-566.

## Context

CI can store a test's failure before another job fails and stops its siblings. A task can fix both observed failures from the same run if its notice and CI details expose them.

## Decision

Read the stored sibling-stop reason and structured failure blocks when presenting a failed run. Identify the stopped job as cancelled and its observations as partial. Use the existing per-job name limit, aggregate detail budget and named-job lookup.

## Alternatives

Waiting for all jobs would spend CI capacity after the first failure. A second notice would wake the task again for evidence already available when the run ended. Reading log tails would lose failures displaced by later output.

## Consequences

Previously retained runs gain the same detail lookup. The stored verdicts, fail-fast schedule and event key continue to govern delivery.
