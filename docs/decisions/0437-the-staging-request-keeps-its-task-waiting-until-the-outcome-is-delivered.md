# 0437. The staging request keeps its task waiting until the outcome is delivered

## Status

Accepted for [SPEC-597](../../.ai/specs/SPEC-597-tests-that-hold-under-a-loaded-host-and.md).

## Context

Merge-first projects run their full gate on the landed revision. A task can finish its turn while its final delivery step is in progress. Yulius already has a staging queue and delivers each outcome to its task. The task list previously asked a person to continue the unfinished step during this interval.

## Decision

When the whole delivery has landed, the merge receipt directs the agent to submit its staging request before ending the turn. If the plan calls for the staging verdict, the goal Stop hook yields to the durable request without consuming a continuation attempt. The request keeps the idle task in Working while its gate runs, its revision waits to deploy, staging verifies it, or its outcome awaits delivery. The task list identifies the stage of this wait. The queue owns waking a live or parked agent. A request for a person or a pause takes precedence.

## Alternatives

A background shell can wait for ordinary PR checks and CD runs, but its process lifetime does not describe the durable staging request. Automatically deploying every merge would deploy intermediate slices while the task still has work ahead.

## Consequences

The existing request and its delivered flag survive a restart. The outcome ends the machine wait, including when a failed gate leaves the revision off staging. The agent can then finish the plan or fix the failure forward.
