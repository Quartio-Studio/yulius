# 0048. A stopped or closed run ends only when its host is proven gone

## Status

Accepted. Specified in [SPEC-065](../../.ai/specs/SPEC-065-fix-284-285-290-a-stopped-or-closed-run.md)
(issues #284, #285, #290). Refines [ADR-0031](0031-deleting-a-done-task-closes-it.md).

## Context

A stop or close asks the session to shut down, then escalates: after interrupt, TERM and kill
(20 s) the run's host is terminated and the run recorded as lost. The result of `terminate` was
ignored, so a host that survived SIGKILL, or a signal that could not be delivered, still counted as
ended. Deleting a task waits for "no live attempt", so it removed the worktree and run directory
of an agent that might still run (AGENTS.md: a timeout is not proof a subprocess died). Deleting a
Done task (ADR-0031) also returned as soon as the store said the run ended, while the run's
finalization still advanced the chains that the deletion was about to change.

## Decision

- The run is recorded as lost only when `terminate` succeeds. When it fails the run stays live:
  the failure is logged, said in the task's terminal, and a running task needs attention. The
  run's reader, still attached, records the run's end once its processes go; stopping the task
  again re-arms the escalation. A close for deletion answers 409 "the task's session could not
  be ended" at once and deletes nothing.
- Each task counts the run endings being recorded (`finalize_attempt`, `mark_lost`) in a
  `tokio::sync::watch`: raised before the attempt is marked ended, lowered when its chains and
  queue work is done. A close for deletion succeeds only when the task rests, has no live
  attempt and the count is zero, within the same 25 s bound.
- The escalation is armed before the run lock is taken, and the shutdown request is bounded
  (2 s), so a stuck host or a cancelled caller cannot leave a stop or close without escalation or
  hold the lock that recording the end needs. The wait looks again every second on an interval
  that other events do not reset. A task that was not closed and is neither resting nor stopping
  is refused at once.
- The escalation time is configuration (`stop_escalation_ms`, default 20 s) so tests can
  shorten it.

## Alternatives

- Retry `terminate` in a loop until it works: a process that survives SIGKILL (uninterruptible
  sleep) can take minutes; a loop adds a background task per stuck run for little gain, since the
  attached reader already records the end when the processes go, and a person can stop again.
- Record the run as lost and keep a separate "unreaped" flag that deletion checks: a second
  notion of "live" that every other reader of the attempt state (capacity, chains, the sidebar)
  would have to learn.
- A one-shot "finalized" flag per run instead of a count: a run that ended by itself just before
  the close, and two observers of the same end, would need generation bookkeeping.
- Spawn the whole close in its own task instead of arming the escalation first: one task more for
  the same guarantee, since nothing after setting the intent needs to finish once the escalation
  is armed.

## Consequences

- A run whose processes cannot be killed keeps its session slot and blocks its task's deletion
  until they are gone; the task says why.
- Deleting a Done task returns after its chained successors have their outcome, so they see one.
- A stopping task can be stopped again through the API, which re-arms the escalation.
