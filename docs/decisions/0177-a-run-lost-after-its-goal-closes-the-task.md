# 0177. A run lost after its goal closes the task, and every settled turn keeps its screen

## Status

Accepted. Specified in [SPEC-229](../../.ai/specs/SPEC-229-this-can-t-be-this-way-something-got-fuc.md);
task #215, fixes #685. Refines restart reconciliation in SPEC-002 §7.1 (still no respawn).

## Context

- The host rebooted on 2026-10-18 at 17:54 UTC. A reboot ends every session host, so the next
  server could reattach none and recorded every live run `lost`. Every task with a session
  became `stopped(server_lost)`: Stopped, with a warning mark and "Yulius restarted while the
  agent was working".
- Among them were #213 and #214, whose goals had been achieved and whose pull requests had been
  merged hours before. Their sessions were idle and kept only for follow-ups (TSK-014): ending
  them lost nothing, yet the finished work left Done and read as an accident.
- A task without a live run restores its terminal from the run's `final-screen` file, which was
  written only when Yulius saw the process exit. A run lost while Yulius was down never had one,
  so every such terminal was blank.

## Decision

- **A finished task is Closed.** When a run is lost (reconciliation or a lost process) and its
  task was `running` with activity `done`, the task becomes `closed`, the state a person's close of
  a Done task gives (`TaskStatus::after_loss`). Every other lost task keeps `stopped(<reason>)` and
  stays continuable.
- **A settled turn keeps its screen.** Whenever a running task's activity becomes idle (`waiting`
  or `done`), the run's current screen is written to its `final-screen` file; the run's end writes
  it again as before. A run lost later shows the screen its last turn settled on.
- **Honest words.** The `stopped_server_lost` message says Yulius restarted and the session
  ended, not that the agent was working (idle sessions end too).

## Alternatives

- **Respawn every lost session after a restart.** Rejected: SPEC-002 §7.1 never respawns (a
  respawn re-runs a turn nobody asked for, possibly on another subscription); a person continues.
- **A new status for "finished, session gone".** Rejected: `closed` already means a finished task
  whose session ended; a new status would need every view, count and the API to learn it.
- **Keep `stopped(server_lost)` and let the web UI show Done from the last activity.** Rejected: the
  store clears the activity of an ended task, and chains, counts and the Dashboard read the status.
- **Save the screen periodically, or on the server's graceful shutdown.** A timer writes while
  nothing changes; a shutdown hook misses a crash or a hard reset. A settled turn is the screen
  worth keeping, at one small write per turn. A run cut mid-turn shows its previous turn's screen.

## Consequences

- After a reboot, finished tasks read Closed (a check mark, no warning) and every task's terminal
  shows its last settled screen; interrupted tasks still read Stopped (server restarted) with
  Continue.
- A Done task closed this way counts as `closed` in analytics, like one a person closed.
- One extra write of a few kilobytes into the run directory per settled turn.
