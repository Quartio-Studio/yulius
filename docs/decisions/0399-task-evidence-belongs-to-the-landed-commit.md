# 0399. Task verification evidence belongs to the landed commit

## Status

Accepted for SPEC-562.

## Context

Task delivery keeps the current PR number. CI and CD have individual run IDs, job verdicts and environment commits. A branch can advance or revert a change before a task checks its deployment. CI and CD prune run details, and release records lack check outcomes.

## Decision

Record the task PR's intended integration branch when it opens or Yulius adopts it. A merge observed later on GitHub must match that original branch. Record the task, branch, PR, PR head and landed SHA with the delivery transition. If GitHub cannot confirm the merge immediately, persist the expected branch, head and merge SHA. Later confirmation must match all three. At upgrade, recover the expected branch from the task's original PR-opening audit entry. An absent or ambiguous entry leaves a legacy landing unknown. Join CI by sandbox, exact reference and commit. Join deployment attempts by sandbox and exact landed SHA. Keep each attempt's identity and the stored explanation of a reused gate. Copy only the compact projection when retention deletes a linked run. Provide a task-authorized permalink for the landing with bounded pages and a filter by attempt ID. A successful CD wrapper confirms its configured commands. For an unsuccessful run, project output and diagnostic spans cannot establish which commands passed.

## Alternatives

The mutable branch tip and commit ancestry can include later behavior. A successful release alone misses failed reruns and unconfigured verification. Keeping complete runs forever would retain logs and commands beyond their normal lifetime. A repository PR that copies the results adds a review and another CI run.

## Consequences

A person or agent can cite a particular attempt after its detailed log expires. A merge without an authoritative landed SHA or an exact environment run leaves that proof unavailable. Source-document requirements, task completion and delivery permissions remain project decisions.
