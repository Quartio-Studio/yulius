# 0087. A change of a sandbox's pull requests is announced, and browsers read them again

## Status

Accepted. Specified in
[SPEC-108](../../.ai/specs/SPEC-108-we-got-wrong-status-again-investigate-st.md).

## Context

A task's status in the web UI combines two sources: the task's own state, pushed live on the event
stream (`task.updated`), and its pull request, read from the sandbox's Git overview (`GET
/api/sandboxes/{id}/git`). The browser polled that overview every minute while the tab was in view
and never while it was hidden (PERF-12). The server knew when a pull request changed (it opens and
merges them, reconciles them with GitHub and runs their checks), but said nothing. Task #56 read
Done with "checks running" 4.5 minutes after Yulius had merged its pull request (task #93's
screenshot); with an older "checks failing" read, the same rule shows "Needs you".

## Decision

The event stream carries `pulls.updated { sandbox_id }`, addressed to the sandbox's viewers. The
server publishes it when a task's delivery stage moves (`dlv_transition`, through which every stage
write of `yulius-app` goes) and when a Yulius CI run of the sandbox is queued, starts or ends
(`ci_publish`). The browser reads the overview again when it watches that sandbox: at once while in
view, or when the tab is shown again, and at least 3 s after the start of its last read. Reads of a
sandbox never overlap; an announcement during a read brings one more read after it. A pull request
Yulius recorded as merged reads merged even while GitHub still lists it open. The minute poll stays
for changes Yulius does not see.

## Alternatives

- Push the pull request itself in the event: the overview joins GitHub's list, Yulius CI and the
  delivery records with the viewer's own GitHub access, so a second, per-viewer projection would
  have to be kept equal to it.
- Read the overview again after every `task.updated`: far more reads (tasks change many times a
  minute), and still blind to a merge or CI run that comes with no task change.
- Poll faster: costs GitHub and Git work for every watching browser all the time and still lags.
- Keep pulls out of `taskInfo` while they may be stale: the rule needs them to tell Done from In
  review.

## Consequences

A merge, an opened pull request or a finished CI run reaches every watching browser within seconds.
Announcements cost at most one overview read per 3 s per watching browser in view. A hidden tab
reads nothing until it is shown. A merge on GitHub outside Yulius is seen at the reconcile loop's
next pass or at the next poll, whichever is first.
