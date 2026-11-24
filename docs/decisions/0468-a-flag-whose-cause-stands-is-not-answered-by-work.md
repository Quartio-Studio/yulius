# 0468. A "could not end this session" flag is not answered by the session's work

## Status

Accepted for SPEC-635 (#1611). Narrows 0306: one flag is not answered by the session's work.

## Context

When Yulius cannot terminate a run's host (a stop, close, park or switch whose session survives
the kill), the run stays live and the task is flagged "Yulius could not end this session … Stop
the task to try again" (SPEC-065 END-001). A flag is read only while the task reads
`needs_attention`, and every session-driven activity write replaced that activity: the prompt
hook set Working, a turn's Stop or goal verdict settled it Done or Waiting, transcript work and
interrupts did the same. A turn already on its way when the failure was recorded therefore wiped
the flag, and the person never learned that the session could not be ended and blocks deletion.
CI hit this as the flaky `task_done::a_switch_whose_old_run_cannot_be_ended_starts_no_new_run`:
under load the goal turn's prompt hook landed after the flag.

## Decision

While the task's live run is the one recorded in the runtime's `end_failed` and the task still
reads the flag that failure raised (its stored reason), the session's activity writes (prompt
hook, clean-turn and verdict settles, idle settles, transcript turn starts, interrupts) are
refused and the flag stays. A person's typed prompt is such a write too: typing does not make the
host endable. The flag ends when its cause does: a person stops the task (the re-armed escalation
clears `end_failed`), or the run ends and the task changes status. Once something else replaced
the flag (a capacity pause and its release, another matter's flag that stood first so this one
was never raised), the session's activity rules again, as before. A later run of the task is not
affected: the check is against the live run's id, and a record whose run is over is dropped. The
check and the write share one per-task lock (`end_gate`) with the record of the failure, which
also marks the flag pending until it is raised: a write either lands before the failure is
recorded, or sees it and leaves the task as it is while the flag is being raised. The lock is
released before the flag is raised (whose path takes the attention lock, which some writers hold
before the gate) and holds nothing but store calls. The flag is known by its wording, so a second
failure of the same run, in other words, still holds the session's writes.

## Alternatives

- Only make the test wait for the launch turn to settle before switching: the test would pass,
  but production keeps losing the flag whenever a turn ends after the failure (a usage-limit
  switch happens mid-turn by nature).
- Store a "stands" attribute in the flag's reason and refuse in the store: the flag would survive
  a restart of Yulius, which reattaches the run and can end it again, so it would outlive its
  cause; the runtime's `end_failed` is the record that knows the failure.
- Raise the flag again after each overwriting write: the task would flicker between states, and
  each notice and publish would repeat.
- Refuse while `end_failed` names the live run, whatever the task reads (review round 1): after a
  pause and its release the task read Working for good, and another matter's flag that stood
  first could never be answered.
- Raise the flag again when a paused task runs again: a second notice for one failure, and the
  release paths would need to know about a supervisor matter; the terminal's notice and the
  delete's 409 still say the run cannot be ended.

## Consequences

A task with an un-endable run reads Needs you while its session works on. As with any standing
flag, its chains and the queue do not see it Done, its idle session is not parked or restarted on
a newer Claude Code, and Yulius types nothing into it (the pull request watch stays quiet), until a
person stops it or the session leaves. The guard costs a mutex look per activity write; the store
is read only while a failure is recorded for the live run.
