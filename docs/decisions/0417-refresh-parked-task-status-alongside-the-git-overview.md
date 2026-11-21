# 0417. Refresh parked task status alongside the Git overview

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

Rerunning CI changes whether a parked task belongs under Done without changing its delivery stage. A Git overview may be slow, unavailable or missing the task’s PR. A sidebar left open across the rerun needs the same verified group as a newly opened task list.

## Decision

Use the existing sandbox PR watch to refresh the bounded task-list endpoint whenever the browser has parked rows. Each source can finish and notify its views independently. Each source applies the watch’s three-second gap to announcements and allows one request in flight per sandbox. A change announced during a read schedules one follow-up. The watch and navigation share the newest successful task list. A parked lifecycle change during a list read can trigger one confirmation. The confirmation is ordered by its own start when successful. If it fails, the original snapshot keeps its place in the request order and contributes any known open PR to an unverified event. This distinguishes an archive from a same-second Undo. Events during confirmation take precedence and a permanent deletion suppresses delayed replies while the read is outstanding. Lifecycle events that omit the checks result keep a known open PR under Needs you and request verification through the same watch.

An older blocking Git chip stays visible when a later list verifies recovery on that PR. The sidebar and task lists use the newer result for grouping. A later Git request, a different PR or a summary without verified identity retains its own signal. Preserve a person’s outstanding decision, local work and a failing PR with a different identity. If the summary is unavailable, preserve a known open PR or the verifier’s prior Needs you decision until verification succeeds. A timeout can occur before the verifier identifies a PR. The same rule applies when the overview first discovers an open replacement after the summary has failed.

## Alternatives

Waiting for the Git overview would leave stale groups on screen while the request is held. Polling each parked task separately would multiply requests and discard the shared verification budget.

## Consequences

One sandbox list request refreshes the sidebar and notifies subscribers when a parked row changes. A view can show the verified group before the overview can supply a detailed checks label. Reading either source only changes the display.
