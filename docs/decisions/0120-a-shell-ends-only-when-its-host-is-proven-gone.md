# 0120. A shell ends only when its host is proven gone; a launch goal is watched across a restart

## Status

Accepted. Specified in [SPEC-146](../../.ai/specs/SPEC-146-fix-346-453-a-shell-ends-only-when-its-h.md)
(issues #346, #453). Extends [ADR-0050](0050-a-run-ends-only-when-its-host-is-proven-gone.md)
to sandbox shells and the Superagent, and [ADR-0095](0095-a-goal-task-starts-with-its-goal-as-the-launch-prompt.md)
to a server restart.

## Context

Ending a sandbox shell or the Superagent (a close, the idle reaper, the Superagent's move past a
usage limit, SPEC-074 SUP-004) asks it to exit, then terminates its host after 10 s. The result of
`terminate` was ignored: the shell was recorded as ended and its run directory removed while its
process might still run. For the Superagent's move this meant a second Claude Code process on the
same conversation while the first could still append to the transcript the move replaces.

A goal task's launch `/goal` (SPEC-117) is watched by a task of the server process; a restart before
Claude Code recorded the goal left the task running without a goal and without a flag. The launch
prompt is not persisted, and the transcript replay of a reattached run set the goal's state but not
that a goal was taken.

## Decision

- A shell asked to end is recorded as ended, and its run directory removed, only when it exited or
  `terminate` succeeded (which waits for its processes to be gone). Otherwise it stays live: it
  keeps its registration and run directory, a pending close is cleared so it is listed and closable
  again, the failure is logged and its viewers get a notice. Closing it again, or the idle reaper's
  next pass, retries. The wait before terminating is configuration (`shell_exit_ms`, 10 s).
- `end_shells` says whether every shell ended. The Superagent's move starts the new Superagent only
  when the old one ended; otherwise the old one stays on its subscription, no longer switching, and
  its viewers are told why.
- A run whose launch prompt is a `/goal` gets a `launch-goal-pending` marker in its run directory
  before its spawn; the goal watch removes it when it ends with an outcome (goal recorded, task
  flagged). Reconcile restarts the watch for a reattached run that still has the marker. The
  transcript replay counts any goal record of the run as the goal taken.

## Alternatives

- Retry `terminate` in a background loop until it works: rejected as in ADR-0050; a process that
  survives SIGKILL can take minutes, and a person (or the reaper) can retry.
- Mark the shell ended but keep an "unreaped" flag: a second notion of "live" for the list, the
  attach, the per-sandbox limit and the reaper.
- Persist the launch prompt (or a flag) on the attempt record: a schema change for what the run
  directory already carries in the analogous `opening-pending` marker.
- Decide from the transcript whether the launch prompt was a `/goal`: a harness read per
  reattached run, and a goal not yet written to the transcript would be missed.

## Consequences

- A shell or Superagent whose processes cannot be killed keeps its slot among the user's shells and
  its run directory until they are gone; its viewers know why.
- The idle reaper retries such a shell every pass (each a bounded 2 s `terminate`).
- A restart while a launch goal is unconfirmed flags the task as a launch without restart would,
  later by the time the new server needs to see the session read input.
- Not covered (noted on #346): a lost host connection (`pump_shell`), an unadoptable run directory
  at start and a shell whose start failed still ignore `terminate`'s result.
