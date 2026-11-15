# 0344. The Docker front gives way at its limit and ends connections whose client is gone

## Status

Accepted. Specified in
[SPEC-494](../../.ai/specs/SPEC-494-a-sandbox-s-docker-never-jams-the-front.md), task #476.
Extends [0158](0158-docker-reaches-every-run-through-a-front-and-a-directory-moved-into-live-runs.md) (the front) with
[0241](0241-a-full-relay-closes-its-longest-idle-connection-and-the-superagent-restarts-in-place.md)'s rule.

## Context

On 2026-11-15 quartio-v2's Docker front held all 256 of its places for hours: every Docker call
of the sandbox's CI jobs and agents failed, and seven tasks asked a person to restart it. The
front kept a place until both ends of its connection closed and dropped a new connection at the
limit without a word or a log line.

A real `dockerd` keeps an exec stream open after its client stopped sending, as long as the exec
runs; it ends when the container's task goes. Under ADR-0339's old cgroup layout tasks were
never deleted, so the streams of tests killed mid-exec never ended, and the front waited for each
of them for good: `copy_bidirectional` waits for the engine's end once the client's side ended,
whether the client half-closed or hung up entirely.

## Decision

- **One admission table for every mode** (engine, relay, refuse), the relays' `Places` with
  `MAX_CONNECTIONS` (256) and `IDLE_FLOOR` (300 s): at the limit the connection idle longest past
  the floor is closed for the new one. When nothing is idle that long the new connection is
  answered a Docker-API 503 that says the front is at its limit and Yulius repairs it, so no
  agent asks a person; at most 16 such answers are written at once.
- **A hung-up client ends its connection.** Once the client's side read EOF, the front asks the
  socket each second whether its peer closed (`POLLHUP`); then it closes the engine's side at
  once, unless the engine ends its side within that second (an ordinary call whose client
  closed after its answer). A half-close is not a hang-up: the Docker CLI half-closes after
  stdin's EOF and still reads the output.
- **An ended engine side gives the client 30 s** to close its own before the front closes it.
- The at-limit answer is the whole message (not the engine-missing wrapper that names the
  operator). The front writes its events to `docker-front.log` at most once a minute per kind, and its state
  (held, limit, totals, at-limit since, engine running) to `docker-front-state.json` in the
  state directory every 5 s at most when it changed. The server reads that file at its upkeep tick
  and journals what grew, once a minute per sandbox and kind; later slices of SPEC-494 report the
  trouble, repair the front and replace outdated fronts from the same file.

## Alternatives

- **A larger limit**: delays the jam by the same leak's rate; does not end it.
- **A hard lifetime for every connection**: kills `docker logs -f`, `docker events` and long
  `docker wait` calls that are idle by nature.
- **Ending a connection at any half-close**: breaks `docker run -i … < file` and `docker exec -i`,
  which half-close and then read the output for minutes.
- **A socket from the front to the server for its events**: the front outlives server restarts
  and would need reconnection; the upkeep already ticks for each sandbox and reads its files.

## Consequences

- A front at its limit keeps serving active work; leftovers give way first. A long, silent
  `docker wait` may give way at the limit, as SPEC-355 accepted for relays.
- The fix reaches a sandbox when its front starts again (SPEC-494 DFJ-010 replaces outdated fronts
  when quiet).
- `docker-front-state.json` is a new state file the front writes; the engine cannot write the
  state directory.
