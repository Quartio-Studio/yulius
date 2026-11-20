# 0409. Report the PR that blocks a parked task

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

A task can have an older recorded merge and an open replacement on the same branch, or two open PRs targeting different bases. GitHub can still show a green head after Yulius recorded a failing ordinary CI run on one of those PRs.

## Decision

An individual task read verifies the recorded and current branch PRs through the parking and dependency verifier. An exact-head ordinary CI failure identifies both the PR and the reason for Needs you. A held prompt, a request for the owner or locally unfinished work already determines the group and requires no remote verification. Keep known PR identity when an installation cannot provide fresh evidence.

Allow 750 ms for the parked status check. A dispatched native Git operation holds one of eight worker permits until it returns, including when the requester times out. The read has no authority to wake a task, resolve a human decision or release a successor.

## Alternatives

An overview-only read can miss a replacement that opened after a recorded merge. A commit-level check misses which PR failed when two PRs share a head. A Boolean eligibility result discards the identity and cause of the final ordinary CI blocker.

## Consequences

The individual HTTP task detail returns its parked group and PR identity. MCP `task_get` uses the same facts for its group and wait reason. Verification may time out, leaving the task under Needs you until a later read. The list budget for Home, task lists and MCP is a separate delivery.
