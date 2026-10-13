# 0031. Deleting a Done task closes its session first

## Status

Accepted. Specified in [SPEC-039](../../.ai/specs/SPEC-039-this-task-is-done-but-i-still-can-t-dele.md).

## Context

A Done task (SPEC-001 TSK-014) keeps its Claude Code session attached and idle, so on the server
it is still `running`. Permanent deletion (TSK-009) refused every live task, and the menu
disabled "Delete permanently…" with "Stop the task before deleting it" for a task that the
sidebar listed as Done (task #37). Stopping it first would record it as stopped (cancelled),
which is not what happened. Archive had already met the same case: it closes a Done task's
session (`closed`) as it archives it.

## Decision

A Done task is treated as finished: deleting it closes its session like Archive does
(`close_task_now`, the task ends `closed`), then deletes it in the same request. The rule for Done
is the one the sidebar and Archive use (`task_is_done`, `taskInfo(...).group === 'done'`).
Deletion removes the worktree and run directories, so the server waits until the run has ended
and the task rests before deleting; a run that already ended by itself is simply deleted. The
close and the wait follow the task's update events (looking again every second) and are bounded
together by the close escalation (interrupt, TERM, kill: 20 s, after which the host is terminated and the run
marked lost, only once the host is proven gone: [ADR-0049](0049-a-run-ends-only-when-its-host-is-proven-gone.md))
plus 5 s; the wait also lets the run's ending be fully recorded (ADR-0049). If the run is still there then, the answer is 409 and nothing is deleted.
The UI's delete request waits 40 s. Any other live task still has to be stopped first. Its menu
item now reads "Still running: stop the task before deleting it".

## Alternatives

- Keep the refusal and explain it better (the request's first option). The user keeps an extra
  Stop step for work that is over, and the task is recorded as stopped.
- The UI closes the task, then deletes it once the close is seen. That takes two requests, which
  a closed tab or a second viewer can interrupt halfway, and a close endpoint of its own.
- A persisted "delete when closed" flag that the supervisor acts on when the run ends. It adds
  state that must survive restarts, for a wait of a few seconds.

## Consequences

- Deleting a Done task can take a few seconds while its session shuts down; in the worst case the
  close and its wait last up to 25 s, plus the usual deletion work.
- Archive and Delete agree on which live tasks they accept.
- If someone types into the session in that moment, it is still closed and deleted, as with
  Archive.
