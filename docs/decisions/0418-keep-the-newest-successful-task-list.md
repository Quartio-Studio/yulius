# 0418. Keep the newest successful task list

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

Initial navigation and stream resynchronization can both request a project’s full task list. An older response can show a task as parked after another list has already observed its resume. Either request can fail.

## Decision

Keep the newest successful list by request start order. A superseded response uses the newer published list. When one overlapping request fails, a successful snapshot remains usable. If the direct task page showed an initial list error, reopen that route when its project’s list arrives. An isolated refresh failure still reports the error.

## Alternatives

Publishing in response order can restore an older parked row. Discarding a request as soon as a newer one starts can lose the only successful cold load. Sharing all overlapping requests can prevent resynchronization from taking a snapshot after an event gap.

## Consequences

A slow full-list response cannot undo a newer full-list observation. Reading the lists changes the display and has no authority to wake, archive or restore a task. Ordering an individual lifecycle event against a snapshot is a separate part of the parked-page watch.
