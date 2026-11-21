# 0444. Codex tasks inherit High effort on every model

## Status

Accepted by the owner on 2026-11-21. Supersedes [ADR-0398](0398-astra-6-2-inherits-extra-high-effort.md)
and the effort default in [SPEC-561](../../.ai/specs/SPEC-561-default-astra-6-2-to-xhigh-for-new-work.md).
[SPEC-607](../../.ai/specs/SPEC-607-codex-tasks-run-at-high-effort-unless-th.md)

## Context

The owner compared 115 merged Astra tasks that ran wholly at one effort over 18–21
November. On one Yulius machine, 35 High tasks and 12 Extra high tasks had median
active times of 50.8 and 50.6 minutes. Extra high used 79k versus 35k output tokens
per task and read 37M versus 30M cached tokens. A second PR was needed for 2 of 35
High tasks and 1 of 12 Extra high tasks. Across projects, Extra high used about
1.3 times the output tokens. The instance used six subscriptions' weekly allowances
in roughly two and a half days.

## Decision

Resolve the task's optional effort on every Codex launch and resume. An omitted
effort resolves to `high` on every model. An explicit effort takes precedence.
GPT-6.2 Astra and Sol both support deliberate Extra high and Max choices. Codex
review sessions continue to request Astra at High.

## Alternatives

Keeping Astra at Extra high would retain the comparison default after the comparison
ended, with the observed higher token use. Storing High on task creation would
lose the distinction between inherited and explicit High and leave older tasks
with a different rule.

## Consequences

An existing task with omitted effort takes High the next time a process starts.
The task details mark the effective effort as inherited. A running process keeps
its launch setting. Rollout turn context continues to record the effort of each
actual turn.
