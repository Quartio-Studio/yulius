# 0418. Preserve lifecycle events during task list reads

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

The browser can load a project’s task list while an owner resumes a parked task or archives it. An older list response can otherwise restore the parked row. Initial navigation and stream resynchronization can also request the same list concurrently. Either request can fail.

## Decision

Keep the newest successful full list by request start order. Each read records the latest lifecycle update or removal for each affected task until the request settles. Apply those changes to the returned list before publishing it. A successful list remains usable when an overlapping request fails.

## Alternatives

Response order would let an older parked snapshot undo a resume. Discarding a request as soon as a newer one starts can lose the only successful cold load. Sharing every overlapping request can prevent resynchronization from taking a snapshot after an event gap.

## Consequences

Task pages and their sidebar can use a complete list after a partial request failure. Each request keeps one entry per changed task and releases those entries when its bounded API request settles. The read changes the browser display and has no authority to resume or archive a task.
