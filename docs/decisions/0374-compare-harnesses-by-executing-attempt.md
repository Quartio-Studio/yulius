# 0374. Compare harnesses by executing attempt

## Status

Accepted for SPEC-522.

## Context

A task can have several attempts and historical tasks can have attempts from different harnesses. Some attempts have no token telemetry.

## Decision

Use the existing sandbox analytics cohort. Assign each task to its current harness for the task count. Attribute usage and run counts to each attempt's executing harness. Compute median and p90 wall time over ended attempts only. Show the duration sample size and token-reporting coverage alongside their measurements.

## Alternatives

Grouping all usage by the task's current harness misattributes earlier attempts. Computing percentiles in the browser would require sending individual attempt records. Timing live attempts would make the sample depend on when the page opens.

## Consequences

The comparison uses the existing summary read and needs no migration. Durations describe elapsed session time including idle time. A harness can appear with tasks and no attempts, or attempts and no currently assigned tasks.
