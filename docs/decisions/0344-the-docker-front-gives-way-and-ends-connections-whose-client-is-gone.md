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
  answered a Docker-API 503 that says the front is at its limit and how it is freed (once
  SPEC-494's repair lands, that Yulius repairs it, so no agent asks a person); at most 16 such
  answers are written at once.
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

- **A full front is its sandbox's trouble, told once** (SPEC-494 slice 2). The upkeep reports the
  state file to SPEC-361's registry as a `docker` trouble: it begins when the front refused since
  the last read or holds its whole limit, and ends once it holds at most seven eighths and
  refused nothing since. The panel's pill and the notification center show it once. Every 15 s
  the tasks of that sandbox with a live session are told once, typed into their session, that
  Docker is full and not to ask a person; once the trouble has stayed ended for five passes
  (75 s), the tasks told hear that it is no longer full. Nothing is typed where a person must
  act or a task is paused; a task told waits for the end until it rests. An adopted front's
  past refusals are no new trouble. What was told lives in memory.

- **Yulius restarts a jammed front itself** (SPEC-494 slice 3). A front full or refusing at
  every tick for 2 minutes, or whose running engine got no answer to three pings in a row (asked
  only beside a client: a ping is a client, and would keep an idle engine from stopping), is
  restarted as the hand repair did: the front stops (its engine with it; running containers
  stop, volumes and images stay) and a new one starts at once. It waits for no CI job of the
  sandbox to run, at most 10 minutes after the repair was due. Each restart is journaled and
  counted. The Superagent's `docker_restart` does it at once when the person asks; task agents
  do not get it, since it would stop every other task's and CI job's containers.

- **One shared problem, one Needs you** (SPEC-494 slice 4). `needs_you` takes `cause: docker`
  for a problem of the whole sandbox. The tasks of a sandbox naming it are grouped under one card
  in the sidebar's Needs you group, and the notification center holds one entry for it. A
  person's answer to one running task's request, the end of the Docker trouble or
  `docker_restart` ends the requests asked before it, audited, and their tasks are told. Anyone
  who may write to one of those tasks answers them all: the problem is the sandbox's, not one
  owner's. The agent names the cause: Yulius cannot tell from free text that requests are the
  same.

- **A front from an earlier build is replaced when quiet** (SPEC-494 slice 5). The upkeep compares
  the file a front process runs (`/proc/<pid>/exe`) with the server's executable; when they
  differ (an update replaced it) and the front is quiet, it is restarted. Quiet is read live just
  before: no child process (its engine) and no connection at its socket, accepted or waiting
  (`/proc/<pid>/net/unix` lists those of the front's network namespace, every run, CI job and
  CD run of the sandbox, under the listener's path; an engine front's containers are covered by
  its engine, the front's child), and, when the front writes a state, the state agrees. A relay
  front (a legacy holder's, ADR-0158) has no engine child: a client of it from another namespace
  is seen only in its state, and before this change not at all. A Docker call that comes
  between that read and the new front's `ready` fails at once (refused, or cut once the old
  front accepted it) and can be run again. A state alone is up to 5 s old, and a front from
  before this change writes none and holds four sockets when idle, so neither the state nor a
  socket count decides (review round 1 of slices 5 to 7). A busy one is kept and journaled once
  a day.

- **Engines do not ask for `br_netfilter`** (SPEC-494 slice 6). A sandbox's engine runs with
  `DOCKER_IGNORE_BR_NETFILTER_ERROR=1`: networks with `enable_icc=false` are created, without
  enforcing ICC inside the sandbox; the host loads no module.

- **Measures on the instance** (SPEC-494 slice 7). Restarts and give-ways become audit entries
  about the sandbox, written by the app from the upkeep's bounded event queue; a Needs you naming
  `docker` is audited too. `GET /api/analytics/docker` and the Pipeline tab's Docker card read
  them per week, like the merge queue's figures.

## Alternatives

- **A larger limit**: delays the jam by the same leak's rate; does not end it.
- **A hard lifetime for every connection**: kills `docker logs -f`, `docker events` and long
  `docker wait` calls that are idle by nature.
- **Ending a connection at any half-close**: breaks `docker run -i … < file` and `docker exec -i`,
  which half-close and then read the output for minutes.
- **Telling only the tasks whose calls were refused**: the front sees bytes on a socket, not
  which run made a call; every live task of the sandbox shares the one Docker, so all are told.
- **Waiting for a moment with no container running at all**: a sandbox's long-lived dev
  containers would hold the repair off for good while every Docker call fails.
- **Recognising a shared problem from the request's words**: wordings differ by task and would
  merge unrelated requests; a named cause is exact, and an unknown one is refused.
- **Recording the build in the front's record**: fronts started before the change have none,
  and the incident's front was one of them; the executable's file tells every front apart.
- **Loading `br_netfilter` on the host**: every bridged frame of the host would go through
  iptables, cutting bridged guests on a host whose FORWARD policy drops, unless a boot-ordered
  sysctl pinned it off again; for an isolation inside one tenant's engine that Yulius does not
  promise.
- **Letting tasks restart Docker**: seven tasks would restart it under each other's containers;
  Yulius knows when, and a Superagent acts for its person.
- **A socket from the front to the server for its events**: the front outlives server restarts
  and would need reconnection; the upkeep already ticks for each sandbox and reads its files.

## Consequences

- A front at its limit keeps serving active work; leftovers give way first. A long, silent
  `docker wait` may give way at the limit, as SPEC-355 accepted for relays.
- The fix reaches a sandbox when its front starts again (SPEC-494 DFJ-010 replaces outdated fronts
  when quiet).
- `docker-front-state.json` is a new state file the front writes; the engine cannot write the
  state directory.
