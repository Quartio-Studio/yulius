# 0417. Refresh parked task status alongside the Git overview

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

Rerunning CI changes whether a parked task belongs under Done without changing its delivery stage. A Git overview may be slow, unavailable or missing the task’s PR. A sidebar or terminal left open across the rerun needs the same verified group as a newly opened task list.

## Decision

Use the existing sandbox PR watch to refresh the bounded task-list endpoint whenever the browser has parked rows. Each source can finish and notify its views independently. Each source applies the watch’s three-second gap to announcements and allows one request in flight per sandbox. A change announced during a read schedules one follow-up. Apply a parked summary only while the same row is still current. A later task event or list replacement takes precedence.

Use the server’s verified group for the identified PR. Preserve a person’s outstanding decision, local work and a failing PR with a different identity. If the summary is unavailable and the browser knows an open PR, keep the task under Needs you until verification succeeds. The same rule applies when the overview first discovers an open replacement after the summary has failed.

## Alternatives

Waiting for the Git overview would leave stale groups on screen while the request is held. Polling each parked task separately would multiply requests and discard the shared verification budget.

## Consequences

One sandbox list request serves the sidebar and open task views for each refresh. A view can show the verified group before the overview can supply a detailed checks label. Reading either source only changes the display.
