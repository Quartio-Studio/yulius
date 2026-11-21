# 0450. Attribute integration test failures in first-parent order

## Status

Accepted for SPEC-612.

## Context

During the incident on 21 November, the same failed test produced fix-forward instructions on seven merges. Run completion time and staging request time can both differ from merge order. A closed owner has no session to receive an instruction.

## Decision

Compare full gates on the integration branch's first-parent history. Preserve compact job verdicts and test names when CI prunes logs. A failed job whose tests cannot all be identified still needs its current owner. A passed job ends the observed failures for that job. A full pass ends the interval for every test.

Queue one new candidate suite at a time in branch order and reuse eligible branch runs. Each tested merge can expose an additional failure or supply the fix. An inherited staging receipt records the origin and waits for a verified descendant. The queue completes it without resuming the task. CI and Status show unresolved failures with no available task, with a link to the run.

## Alternatives

Holding all candidates for a designated fixing task misses fixes from other tasks and new failures. Comparing job names alone assigns independent test failures in the same shard to the earlier owner.

## Consequences

New candidate suites wait for the previous candidate. Already scheduled integration runs remain usable. A red revision can reach staging only through a verified descendant. A person's rerun can resolve a flaky verdict on the same revision.

On upgrade, an undelivered failed receipt with retained CI evidence returns to judgement. Previously delivered receipts preserve what the old instance told their tasks.
