# 0016. Disk is managed by Yulius: caches of idle tasks are pruned, switches check first

## Status

Accepted ([SPEC-005](../../.ai/specs/SPEC-005-sandbox-disk.md), F 2026-10-11).

## Context

A sandbox over its 100 GB soft quota (112 GB, mostly Cargo `target` directories of closed
tasks) refused every start. A switch ended the healthy session before the start was checked, so
four running sessions ended `stopped_switch_failed`, and the quota reading was up to 10 minutes
old, so freeing space did not help at once.

## Decision

A switch runs the start's preconditions (repository, credential, quota, holder) before it ends
the running session. A quota refusal measures again when the reading is over a minute old.
Yulius removes recognised or declared build caches of tasks that do not work (ended tasks after a
10-minute grace; idle tasks only from 90 % of the quota, largest first, down to 80 %), removes
the clean worktrees of closed and cancelled tasks, and messages the agent of each task what was
freed. A working task is never touched.

## Alternatives

Hard quotas (need a dedicated volume); deleting whole worktrees of idle tasks (loses
uncommitted work); a cap on `target` size per task (does not cover other caches); asking people
to clean up (what failed).

## Consequences

Builds of idle tasks restart cold after a prune. A project with an unusual cache declares it in
`[disk] caches`. The quota stays soft: a sandbox full of working tasks' files still refuses new
starts.
