# 0450. Attribute integration test failures in first-parent order

## Status

Accepted for SPEC-612.

## Context

During the incident on 21 November, the same failed test produced fix-forward instructions on seven merges. Run completion time and staging request time can both differ from merge order. A closed owner has no session to receive an instruction.

## Decision

Compare full gates on the integration branch's first-parent history. Preserve compact job verdicts and test names when CI prunes logs. When a failed job has incomplete test evidence, include its name in the repair notice. A passed job ends the observed failures for that job. A full pass ends the interval for every test.

Queue one new candidate suite at a time in branch order and reuse eligible branch runs. Each tested merge can expose an additional failure or supply the fix. An inherited staging receipt records the origin and waits for a verified descendant. The queue completes it without resuming the task. Yulius records outcomes on finished deliveries. Its session input boundary checks for a running task with remaining work. Only a person resumes a parked task, including after PR and handoff events. This supersedes the automatic PR wakes in ADR-0367 and ADR-0391 and the automatic staging wake for finished tasks in ADR-0437.

Choose the lowest-numbered available running project task for the red branch. Eligibility excludes tasks waiting for a person, parked tasks, and completed merged deliveries. Retain the assignee while it stays eligible, including when additional failures appear. Persist the assignment and each worker’s receipt together with the failure details so an unchanged inherited test needs no new instruction. Identify the introducing merge separately for every test. When the owner leaves, assign its remaining work to the next eligible task. A held terminal or browser request excludes the worker from a new assignment. Peers hear the assignee’s name once its session received the instruction. When nobody is at work, CI and Status show the failures and offer people a rerun of the latest affected revision or a new task.

## Alternatives

Holding all candidates for a designated fixing task misses fixes from other tasks and new failures. Comparing job names alone assigns independent test failures in the same shard to the earlier owner.

## Consequences

New candidate suites wait for the previous candidate. Already scheduled integration runs remain usable. Retain compact job observations from unsuccessful reruns of the same revision so an interrupted rerun preserves the previous failure. A red revision can reach staging only through a verified descendant. A person's rerun can resolve a flaky verdict on the same revision.

On upgrade, an undelivered failed receipt with retained CI evidence returns to judgement. Previously delivered receipts preserve what the old instance told their tasks.
