# 0454. Terminal output reaches only the screen of the run that printed it

## Status

Accepted for SPEC-614 (#1624).

## Context

A task's terminal hub outlives its runs. Each run's PTY reader passed its chunks to the redaction sink with the task ID, and the sink wrote to whichever hub the task had then. The 40 ms release of a held redaction tail used the same sink. `Hub::output` drops bytes while a parked screen is shown and takes them again once a host attaches, so a chunk of the previous run that was still in transit (a slow redactor load, the tail timer, a reader that lost its run to an escalation) was drawn on the replacement run's screen and sent to its viewers. Two reproductions with real processes showed both orders on `dev`.

## Decision

Bind every write a run's reader makes to the hub (output, released tails, host snapshots and reattachment restores) to that run's attempt. `TaskRuntime::on_screen_of(run, write)` writes only while `screen_of`, the supervisor's existing record of whose screen the hub shows, names that run. An attach resets the hub and records its run in one step under the same lock (`show_screen_of`), so no chunk passes the check for the old run and is drawn after the new run's reset. Writes that fail the check are dropped. Two writes of a reader stay outside this check: the publication of a parked grid and the exit frame. Both happen before any replacement can attach, because a resume waits for the old reader's terminal cleanup, and both belong to the parking code SPEC-558 changes.

## Alternatives

- A run token kept inside the hub, checked under the hub's own lock. It would make the hub aware of runs and add a second record of whose screen it shows next to `screen_of`, which already decides whose final screen is saved and whether the Codex input screen can be read.
- Cancelling a run's pending tail release and draining its reader before the next attach. That needs every path that replaces a run (park and resume, a lost run, a switch) to wait for the old reader, which a reader stuck on its host cannot promise.

## Consequences

The lock order is `screen_of`, then the hub's screen lock; hub methods never call out while holding theirs. Late output of a replaced run is lost, which is intended: its screen was saved when it ended. Parked screens and the current run's output behave as before. Login shells and the Cockpit write their own hubs and are unchanged.
