# 0417. Refresh parked task status alongside the Git overview

## Status

Accepted for [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).

## Context

Rerunning CI changes whether a parked task belongs under Done without changing its delivery stage. A Git overview may be slow, unavailable or missing the task’s PR. A sidebar left open across the rerun needs the same verified group as a newly opened task list.

## Decision

Use the existing sandbox PR watch to refresh the bounded task-list endpoint whenever the browser has parked rows. Each source can finish and notify its views independently. Each source applies the watch’s three-second gap to announcements and allows one request in flight per sandbox. A change announced during a read schedules one follow-up. The watch and navigation share the newest successful task list. A parked lifecycle change during a list read can trigger one confirmation. The confirmation is ordered by its own start when successful. If it fails, the original snapshot keeps its place in the request order and contributes any known open PR to an unverified event. This distinguishes an archive from a same-second Undo. Events during confirmation take precedence and a permanent deletion suppresses delayed replies while the read is outstanding. Lifecycle events that omit the checks result keep a known open PR under Needs you and request verification through the same watch.

An older blocking Git chip stays visible when a later list verifies recovery on that PR. The sidebar and task lists use the newer result for grouping. A later Git request, a different PR or a summary without verified identity retains its own signal. Preserve a person’s outstanding decision, local work and a failing PR with a different identity. If the summary is unavailable, preserve a known open PR or the verifier’s prior Needs you decision until verification succeeds. A timeout can occur before the verifier identifies a PR. The same rule applies when the overview first discovers an open replacement after the summary has failed.

The mounted parked terminal takes the owner notice from the current task summary. It uses a cached plan’s active ordinal while it still names the active step in the current summary and fits the plan. A list that observes completion can therefore update the terminal while its next detail read is pending.

The mounted terminal publishes the parked fields from its detail read to the existing row. Each read receives its order when it starts. A later successful list or detail supplies the verified status and PR identity. An unavailable watch preserves an observed open PR conservatively and a later confirmed closure rules out an older observation of the same PR. The row only receives public summary fields. A terminal that sees a missed park or resume publishes the lifecycle of an existing task. A later watched list can also update the terminal as the resumed task changes between running and waiting for capacity. It copies public summary fields including the current subscription ID. Each older outstanding list retains the transition until it completes. A later event, permanent removal or newer successful confirmation takes precedence. A detail that began with a visible live row and discovers its park can restore it after an older list omits it. A detail confirming the same parked state leaves an omitted row absent. A later archive, permanent removal or newer empty list takes precedence. A Closed, Stopped or Failed task can be archived and restored with the same process status. Its unarchived detail verifies active-list membership from the start of the read, and a newer list or deletion supersedes that observation. A successful list that restores a missing row notifies the mounted terminal. Archiving a parked task publishes Closed before the archive flag. A terminal that observes the flag removes the active row and publishes the first archive notification even when a list already omitted it. Pending detail and list reads retain the archive order so a newer Undo takes precedence. A PR announcement seen before the first parked row appears schedules one verification through the existing watch.

## Alternatives

Waiting for the Git overview would leave stale groups on screen while the request is held. Polling each parked task separately would multiply requests and discard the shared verification budget.

## Consequences

One sandbox list request refreshes the sidebar and notifies subscribers when a parked row changes. A view can show the verified group before the overview can supply a detailed checks label. Reading either source only changes the display.
