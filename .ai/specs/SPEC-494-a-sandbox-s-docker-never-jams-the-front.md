---
yulius-task: e807d154-9d41-4466-bfed-481ba2d61e0e
status: in-progress
---

# SPEC-494: A sandbox's Docker never jams: the front gives way and repairs itself

Status: in progress
Date: 2026-11-15
Owner: Yulius task #476
Scope authorization: task #476, the person's request of 2026-11-15 ("spawn a task in yulius so it
doesn't happen again") after the quartio-v2 incident below. Builds on SPEC-196 (the Docker
front, ADR-0158), SPEC-355 (SLR-001, ADR-0241: relays give way at their limit), SPEC-361 (CHR-*,
ADR-0246: connection troubles on the panel) and ADR-0339 (the engine's cgroup layout).

## Goal

A sandbox's Docker front never ends up holding every connection: at its limit it gives way as the
relays toward Yulius do, and a connection whose client is gone ends. When the front is jammed
anyway, Yulius sees it once, tells the people and tasks of the sandbox once, and restarts the
front and engine itself at the moment that loses the least. Tasks that hit the same
sandbox-level problem make one Needs you, not seven. Fronts do not run old code for days.

## Context

### What happened (production, quartio-v2, 2026-11-15)

- From about 20:00 UTC every Yulius CI run of quartio-v2 failed its Docker-backed tests
  ("docker.sock EOF, broken pipe, connection reset"); so did the agents' own Docker tests.
- Between 20:18 and 22:04 UTC seven tasks (#203, #209, #210, #211, #220, #221, #223) raised nine
  Needs you, all asking a person to restart the sandbox's Docker engine or front.
- The front (`yulius sandbox-host docker`) held 256 connections, `relay::MAX_CONNECTIONS` (516
  sockets at 22:04; 227 at 20:18, 230 at 20:46, 235 at 21:52). A probe got 24 of 80 connections
  through, a later one none.
- `docker_front.rs` and `Supervisor::serve` keep a connection's place until both of its ends
  close, and drop a new connection at the limit without a word (`try_acquire_owned` fails,
  `continue`). `docker-front.log` had been empty since 11 Nov.
- That front had run since 12 Nov 08:11 UTC, kept across six Yulius updates (it still ran the
  12 Nov build and, per ADR-0339's Consequences, the engine's old cgroup layout).
- Repaired by hand at 22:08 UTC: `SIGTERM` to the front; the upkeep started a new one within
  30 s ("the sandbox's Docker front ended; starting it again"); then 80 of 80 concurrent calls
  answered. The engine and its containers stopped with the old front; volumes stayed.
- The engine log showed, every hour of 15 Nov, about 7,000 "copy stream failed", about 3,300
  "failed to delete task" with as many errors about the threaded `/sys/fs/cgroup/docker`
  (ADR-0339's old layout), and dozens of "Error setting up exec command" on the tests'
  containers.
- Separately, about 5,000 network creates an hour failed since 13 Nov with "cannot restrict
  inter-container communication or run without the userland proxy: stat
  /proc/sys/net/bridge/bridge-nf-call-iptables: no such file". The host does not load
  `br_netfilter`.

### Root cause (verified)

- **The engine keeps an exec stream open after its client stopped sending, for as long as the
  exec runs.** Reproduced against a real `dockerd` 29.9.3 in this sandbox
  (`target/t476/exec_probe.py`): `POST /exec/{id}/start` with `Upgrade: tcp` for `sleep 600`,
  then `shutdown(SHUT_WR)`: no end of the stream within 30 s. `docker rm -f` of the container
  ended it at once (3.0 s): the stream ends only when the container's task goes.
- **The front waits for the engine's end even when its client is gone.** `relay::splice` is
  `copy_bidirectional`: once the client's side reads EOF it shuts the engine's writing and waits
  for the engine's EOF. A test killed mid-exec (or one whose client library closes the socket)
  leaves the place held until the exec ends.
- **Under the old cgroup layout tasks were never deleted** ("failed to delete task", threaded
  `docker` group), so those execs never ended, and their places added up over three days. The
  engine itself keeps its own limits; the front's places are what ran out.
- So the leak has two halves: execs that never end (the engine's old layout, fixed by ADR-0339
  for a restarted engine, and by DFJ-010 for fronts adopted across updates), and a front that
  holds a place for a client that is gone (DFJ-002). Either alone would not have jammed it.

### In scope

The front's admission and connection ends; its log and a state file the server reads; the
sandbox's Docker trouble on the panel and to its tasks; Yulius's own repair and the Superagent's
tool; one Needs you per shared problem; replacing outdated fronts; `br_netfilter`; counters.

Non-goals: the egress proxy and relays toward Yulius (SPEC-355 stays); per-container limits of
the engine (unchanged: a sandbox's engine keeps its own limits); the host's Docker (no sandbox
ever reaches it).

## Requirements

### The front never jams (slice 1)

- **DFJ-001** Every mode of the front (engine, relay, refuse) admits connections through one table
  of `MAX_CONNECTIONS` (256) places. At the limit it closes the connection idle longest (no byte
  either way) for the new one, if that one was idle at least `IDLE_FLOOR` (300 s), SPEC-355's
  rule. When every place was active within the floor the new connection gets a Docker-API answer
  (HTTP 503, JSON `message`) saying the sandbox's Docker front is at its limit and how it is
  freed; once slice 3 repairs it, that Yulius repairs it and nobody needs to be asked; at most `REFUSING_AT_ONCE` (16) such answers
  are written at once, beyond that the connection is closed.
- **DFJ-002** Once a client has stopped sending (its side read EOF), the front looks every
  `HANGUP_CHECK` (1 s) whether the client hung up entirely (the socket's peer is closed, `POLLHUP`).
  Then the engine's side is closed, and unless the engine ended its own within one more check
  (an ordinary call whose client closed after its answer, which is not counted) the connection
  counts as hung up. A client that only shut its
  writing and still reads (`docker run -i … < file`) keeps its connection.
- **DFJ-003** Once the engine ended its side, the client has `CLOSE_GRACE` (30 s) to close its
  own; then the connection ends.
- **DFJ-004** Each give-way, refusal and ended connection (DFJ-002, DFJ-003) is said in
  `docker-front.log` (the front's stderr) at most once a minute per kind, with the totals so far.
- **DFJ-005** The front keeps its state in `docker-front-state.json` in the sandbox's state
  directory (not `docker/`, which the engine can write): connections held, the limit, totals of
  give-ways, refusals and ended connections, since when it is at its limit, and whether its
  engine runs. It writes it (replaced atomically) at most every `STATE_EVERY`
  (5 s) when something changed. The server reads it at each upkeep tick, a regular file of at
  most 16 KiB of the front it knows (never a link or a FIFO), and logs in the journal each kind
  whose total grew since it last said it, at most once a minute per sandbox and kind.

### Yulius sees a jam once (slice 2)

- **DFJ-006** A front that refused a connection, or holds its whole limit, is a connection trouble
  of its sandbox (`docker`), in SPEC-361's registry: it begins at the first state read that shows
  it, ends once the front holds at most seven eighths of its limit and refused nothing since the
  last read, and shows once on the panel and in the notification center like the other troubles.
- **DFJ-007** The tasks of the sandbox with a live run are told once per trouble, in their
  session, that the sandbox's Docker is jammed, that Yulius repairs it and that they should not
  ask a person; the refusal's message says the same to the calls that hit it.

### Yulius repairs it itself (slice 3)

- **DFJ-008** Yulius restarts the front and its engine (as the hand repair did: stop the front,
  start it again at the next upkeep) when the front has been at its limit, or has refused, for
  `REPAIR_AFTER` (2 min), or when its engine runs and a `GET /_ping` through the front failed on
  three ticks in a row. It waits for the moment that loses the least: no CI job of the sandbox
  runs, or at most `REPAIR_WAIT` (10 min) after the repair was due. Volumes stay (the engine's
  storage is kept); running containers stop. Each repair is logged and counted.
- **DFJ-009** Once the restarted front answers, the tasks told in DFJ-007 are told that Docker
  works again and they can rerun. The Superagent gets `sandbox_docker_restart`, which does the
  same repair at once; tasks do not get it.

### One shared problem, one Needs you (slice 4)

- **DFJ-011** `needs_you` takes an optional `cause` naming a sandbox-level problem (`docker`).
  Needs you of one sandbox with the same open cause show as one card naming its tasks; answering
  it answers them all, and Yulius's repair of that cause answers them all too.

### Fronts do not run old code for days (slice 5)

- **DFJ-010** The front records the build it runs. At each upkeep tick a front from another
  build is replaced at its first quiet moment: no connection held and its engine not running (the
  engine stops by itself once idle, ADR-0151), so nothing is lost. A front that never gets quiet
  for `STALE_AFTER` (24 h) is logged once a day.

### br_netfilter (slice 6)

- **DFJ-012** Decided in slice 6 after checking a real host: either the host setup loads
  `br_netfilter` with the host namespace's `bridge-nf-call-*` set to 0 (bridged traffic of the
  host unchanged, a sandbox's network namespace keeps the kernel's default 1), or the engines run
  with `DOCKER_IGNORE_BR_NETFILTER_ERROR=1`.

### Measures (slice 7)

- **DFJ-013** Front restarts (by repair, by update, ended), connections that gave way, and Needs
  you with cause `docker`, per week, are readable on the instance.

## Plan

- [x] Spec, ADR and the leak's root cause reproduced
- [x] The front admits through one table and gives way at its limit
- [x] A connection whose client hung up or whose engine ended ends in bounded time
- [x] The front logs its events and writes its state; the server journals them
- [ ] Slice 1 reviewed independently, findings fixed test-first
- [x] Slice 1's round-2 review notes fixed test-first
- [ ] A jammed front is a sandbox trouble on the panel
- [ ] The sandbox's tasks are told once per trouble
- [ ] Slice 2 reviewed independently, findings fixed test-first
- [ ] Yulius restarts a jammed front at a quiet moment and tells the tasks
- [ ] The Superagent restarts its sandbox's Docker
- [ ] Slice 3 reviewed independently, findings fixed test-first
- [ ] Needs you of one sandbox-level cause show as one card
- [ ] Slice 4 reviewed independently, findings fixed test-first
- [ ] An adopted front from another build is replaced when quiet
- [ ] br_netfilter decided on a real host and applied
- [ ] Front restarts, give-ways and Docker Needs you are counted on the instance
- [ ] Slices 5 to 7 reviewed independently, findings fixed test-first

## Acceptance criteria

- **AC-1** (DFJ-001) A front whose places are all held by idle connections whose engine side never
  ends still serves a new connection, closing the longest-idle one; with every place active it
  answers 503 with the limit's message (red before: the new connection is dropped).
- **AC-2** (DFJ-002) A client that sends a request and closes its socket while the engine never
  answers frees its place within a few seconds (red before: held for good). A client that only
  shuts its writing keeps receiving the engine's bytes.
- **AC-3** (DFJ-003) An engine that ends its side while the client never closes frees the place
  after `CLOSE_GRACE`.
- **AC-4** (DFJ-004, DFJ-005) Many give-ways within a minute write one log line; the state file
  shows the counts; the server journals each kind once a minute.
- **AC-5** (DFJ-006, DFJ-007) A state file at the limit opens one `docker` trouble, published once;
  live tasks of the sandbox get one notice; it ends below seven eighths.
- **AC-6** (DFJ-008, DFJ-009) A front stuck at its limit is restarted once no CI job runs (or after
  the bounded wait), counted, and the told tasks are told it works again.
- **AC-7** (DFJ-011) Two tasks' Needs you with cause `docker` show as one card naming both; the
  repair answers both.
- **AC-8** (DFJ-010) A front of another build with no client and no engine is replaced at the
  next tick; one with a client is kept.

## Test plan

- Unit (`yulius-sandbox::docker_front`): AC-1 to AC-3 with a fake engine socket that never ends
  (the incident's shape) and a real client; AC-4's log throttle and state file.
- Unit (`yulius-sandbox::manager`): reading the state file (bounded, no links) and the journal
  throttle; the replace-when-quiet decision (AC-8); the repair's decision and moment (AC-6).
- Integration (`yulius` tests): the trouble opened from a state file, its notice to live tasks,
  the repair and its notice, the shared Needs you card (AC-5 to AC-7).
- Probe against the real engine in this sandbox (`target/t476/exec_probe.py`, and a local front
  relaying to it): an exec whose client is killed frees its place.
- Real host (QEMU guest, Ubuntu 26.04) for `br_netfilter` (DFJ-012).

## Decisions

- ADR-0344: the Docker front gives way at its limit (SPEC-355's rule), ends connections whose
  client is gone, and Yulius restarts a jammed front itself. Alternatives: a larger limit (only
  delays the jam), a hard lifetime for every connection (kills `docker logs -f`, `events`,
  long `docker wait`), ending a connection at any half-close (kills `docker run -i < file`).
- The front tells the server through a state file the upkeep already has the tick for, rather
  than a socket to the server: the front outlives server restarts, and a file needs no
  reconnection or new listener.
- "Hung up" is the peer's close (`POLLHUP`), not a half-close: the Docker CLI half-closes after
  stdin's EOF and still reads.
- Slices are separate pull requests of this task (one concern each, ~400 lines).

## Evidence

### Slice 1 (the front never jams)

- Root cause on the real engine (`dockerd` 29.9.3 of this sandbox), `target/t476/exec_probe.py`:
  an exec's stream after `shutdown(SHUT_WR)` stays open ("no EOF within 30 s"); `docker rm -f`
  ends it ("EOF from engine after 3.0s").
- `YULIUS_REAL_DOCKER=/run/yulius/docker/docker.sock:t476probe cargo test -p yulius-sandbox --lib
  -- --ignored real_engine` (a relay front to the real engine, an exec of `sleep 600` whose client
  read the whole head and hung up): with the old `relay::splice` swapped back in, `FAILED`
  ("never: the place is free", 10.31 s); with the change, `ok` (0.33 s).
- Red before the change (old splice and silent drop swapped back in): `a_front_of_active_calls_
  refuses_with_a_docker_error`, `a_client_that_hung_up_frees_its_place_though_the_engine_never_
  ends`, `a_client_that_never_closes_after_the_engine_ended_is_closed_after_the_grace` FAILED;
  green after.
- `cargo test -p yulius-sandbox -p yulius-contracts`: 177 + 52 + 2 + 6 passed.
- `cargo clippy -p yulius-sandbox -p yulius-contracts --all-targets`: no warnings.
  `python3 scripts/check_repository.py`: passed.

### Slice 1, review round 1 (request changes; every finding own code, all fixed)

- Ordinary closes counted as hang-ups: `an_ordinary_call_closed_after_its_answer_is_not_a_hang_up`
  (keep-alive engine closing 5 ms after the client) failed before (hung_up counted), passes
  after: a hang-up gives the engine one more check to end its side.
- The at-limit answer named the operator: the test asserts the whole `message` (`AT_LIMIT`),
  failed before, passes after (`answer_error` writes it whole).
- Growth lost inside the journal's throttle: `the_server_journals_what_grew_once_a_minute_per_kind`
  now expects the unsaid give-ways past the minute; failed before, passes after.
- `read_state`: a link, a FIFO, a file over 16 KiB or another front's pid are not read
  (`the_server_reads_only_its_fronts_bounded_regular_state_file`).
- The state writer asks the engine without waiting on its lock (`running_now`); missed ticks are
  delayed; an unchanged state is not rewritten (`an_unchanged_state_is_not_written_again`).
- `at_limit_since_ms` follows `held` at each read and clears below the limit (asserted).
- Added: refusals bounded at 16 and their permits returned, refuse mode at its limit, engine mode
  giving way with its client count dropping, a half-close followed by a close.
- `cargo test -p yulius-sandbox --lib -- front_gate:: engine_supervisor:: docker_front::`: 32
  passed (the gate's tests 8 times in a row: 14 passed each); the real-engine probe passed.

### Slice 1, review round 2 (approve; minor notes fixed in slice 2's pull request)

PR #1330 merged into dev at 201749a4d on green CI (merge train run 1f932ccc passed). Its notes,
own code, fixed test-first here: the ordinary-close test waits up to 20 s for the engine's end
(it ends at the engine's close, so it stays fast and still fails without the round-1 fix); the
size case writes a valid state padded past 16 KiB; the unchanged-state test compares inode and
modification time; a state that cannot be written is said once a minute; the at-limit answer
says only what holds before slice 3 (retry; restarting the front frees it); `DockerFrontState`
states its compatibility rule and an older front's state is read; `running_now` counts a held
lock as running; the front's log lines start with their UTC time. The reviewer's other cause,
a container start in the sandbox failing a parallel e2e job's page loads, is commented on
#1331.
