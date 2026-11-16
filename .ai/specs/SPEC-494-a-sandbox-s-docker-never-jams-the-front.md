---
yulius-task: e807d154-9d41-4466-bfed-481ba2d61e0e
status: done
---

# SPEC-494: A sandbox's Docker never jams: the front gives way and repairs itself

Status: done
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
- **DFJ-007** The tasks of the sandbox with a live session are told once per trouble, in their
  session (a pass every 15 s; a session that takes no typed text now is told at a later pass),
  that the sandbox's Docker is full and that they should not ask a person (unless it lasts more
  than 15 minutes, until slice 3 repairs it); once the trouble has stayed ended for five
  passes (75 s), the tasks told hear that Docker works again. A trouble that ends and begins again
  before they heard it goes on as one for them. Nothing is typed into a session where a person
  must act (a prompt on screen, an open Needs you), one paused or held by an update; a task told
  keeps waiting for "works again" until it rests. An adopted front's past refusals (after a
  server restart) are no new trouble; every path that drops a holder ends its trouble. A
  production holder's front (SPEC-279, CD deploys) is journaled only: its name is no sandbox of
  the panel and no task runs there. The refusal's message says the same to the
  calls that hit it. Since slice 3 both say that Yulius restarts it by itself and nobody needs
  to be asked.

### Yulius repairs it itself (slice 3)

- **DFJ-008** Yulius restarts the front and its engine (as the hand repair did: stop the front,
  then start a new one at once) when the front has been full, or refusing, at every upkeep tick
  for `REPAIR_AFTER` (2 min), or when a `GET /_ping` through the front (connected without
  following a link, as the engine's control socket is) got no answer within 5 s, connecting
  included,
  on three ticks in a row. The ping is asked only while a client holds a connection, the engine
  runs and is not starting or stopping, and the front is not jammed: a ping is itself a client
  of the engine, and asked at other times it would keep an idle engine from stopping
  (ADR-0151) or start a stopped one. A tick that does not ask starts the count again; a socket
  the server may not connect to is no answer either way. A production holder's front (SPEC-279)
  is never restarted by the upkeep: its one user is a production CD run. It
  waits for the moment that loses the least: no CI job of the sandbox runs (a job's containers
  would stop under it), or at most `REPAIR_WAIT` (10 min) after the repair was due, since a
  jammed front fails the jobs anyway. Volumes and images stay (the engine's storage is kept);
  running containers stop. Each restart is journaled with why and counted.
- **DFJ-009** The restarted front's state ends the sandbox's trouble, and the tasks told in
  DFJ-007 hear that it is no longer full (and that containers that ran stopped), so they rerun.
  The Superagent gets `docker_restart` (a person's tool, `Work` on the project, the sandbox
  Superagent's only), which restarts it at once; task agents do not get it: a task restarting
  Docker would stop the containers of every other task and CI job of the sandbox, and Yulius
  already does it at the right moment.

### One shared problem, one Needs you (slice 4)

- **DFJ-011** `needs_you` takes an optional `cause` naming a sandbox-level problem; the one cause
  now is `docker` (the sandbox's Docker does not answer), and another value is refused. The agent
  names the cause (Yulius cannot tell from free text that seven requests are the same problem;
  the tool's description says when). In the sidebar's Needs you group, the tasks of a sandbox
  naming the same cause are grouped under one card that names them (each a link, first asker
  first), with their own rows under it; the notification center holds one entry per such problem
  (from its oldest open request), not one per task; the Superagent's task list says the request
  is a shared problem. A person's answer to one of them (a prompt or message to a running task
  whose request is shown) ends the requests of the sandbox's other tasks naming that cause and
  asked before, and those tasks are told so in their session; the end of the sandbox's Docker
  trouble and the Superagent's `docker_restart` end the requests asked before them, and their
  tasks hear that Docker works again. A request replaced meanwhile stays; each request ended
  this way is audited (`task.needs_you.answered_shared`). Counts (the Home card, the tab's
  "need you", the group's count) stay per task: they count tasks that wait, and the card shows
  they wait for one thing.

### Fronts do not run old code for days (slice 5)

- **DFJ-010** At each upkeep tick a front that runs another build than the server is replaced at its
  first quiet moment. Another build: the file the front process runs (`/proc/<pid>/exe`) is not the
  server's executable (an update replaced it), by device and inode; no record is needed, so fronts
  started before this change are recognised too. Quiet, read live just before the stop: the front
  has no child process (its engine, which stops by itself once idle, ADR-0151) and no connection at
  its socket, accepted or still waiting (`/proc/<pid>/net/unix` lists those under the listener's
  path; no listener seen is a doubt, and a doubt is not quiet); a front that writes a state must say
  so there too. The table lists the clients of the front's network namespace (every run, CI job and
  CD run of the sandbox); an engine front's containers are covered by its engine, the front's child.
  A relay front (a legacy holder's, ADR-0158) has no such child: a client of it from another
  namespace is seen only in its state, and before ADR-0344 not at all. Besides that, nothing is lost
  then, short of a Docker call that comes between that read and the new front's `ready`: it fails at
  once (its socket missing or refused, or cut once the old front accepted it) and can be run again.
  A busy one is kept and the journal says so once a day.

### br_netfilter (slice 6)

- **DFJ-012** The engines stop asking for `br_netfilter`: a sandbox's engine runs with
  `DOCKER_IGNORE_BR_NETFILTER_ERROR=1`, the switch the engine's own error names ("set environment
  variable DOCKER_IGNORE_BR_NETFILTER_ERROR=1 to ignore"). A network that restricts
  inter-container traffic (`enable_icc=false`) is then created, and its containers can still
  reach each other inside the sandbox; the sandbox's network namespace is the boundary Yulius
  keeps, not ICC. The host loads nothing new. It reaches an engine when its front starts again
  (slice 5 replaces kept fronts when quiet).

### Measures (slice 7)

- **DFJ-013** Front restarts (by a repair, a person's ask or an update), connections that gave
  way at a front's limit, and Needs you naming the sandbox's Docker are counted per week and
  readable on the instance: `GET /api/analytics/docker?sandbox&from&to` (weeks from their
  Monday, UTC, and totals) and a Docker card on the Dashboard's Pipeline tab (totals and how many
  a week). The upkeep keeps the fronts' events (bounded, 4096) until the app takes them every
  15 s and writes them to the audit log (`sandbox.docker.restarted` with why,
  `sandbox.docker.gave_way` with how many, about the sandbox's slug); a request naming `docker`
  is audited `task.needs_you.docker`. A production holder's events count for its sandbox.

## Plan

- [x] Spec, ADR and the leak's root cause reproduced
- [x] The front admits through one table and gives way at its limit
- [x] A connection whose client hung up or whose engine ended ends in bounded time
- [x] The front logs its events and writes its state; the server journals them
- [x] Slice 1 reviewed independently, findings fixed test-first
- [x] Slice 1's round-2 review notes fixed test-first
- [x] A jammed front is a sandbox trouble on the panel
- [x] The sandbox's tasks are told once per trouble
- [x] Slice 2 reviewed independently, findings fixed test-first
- [x] Yulius restarts a jammed front at a quiet moment and tells the tasks
- [x] The Superagent restarts its sandbox's Docker
- [x] Slice 3 reviewed independently, findings fixed test-first
- [x] Needs you of one sandbox-level cause show as one card
- [x] Slice 4 reviewed independently, findings fixed test-first
- [x] An adopted front from another build is replaced when quiet
- [x] br_netfilter decided on a real host and applied
- [x] Front restarts, give-ways and Docker Needs you are counted on the instance
- [x] Slices 5 to 7 reviewed independently, findings fixed test-first
- [x] Manager tests start a rewritten fake front without a race (#1339)
- [x] The texts state the relay-front limit and one window wording (#1341)
- [x] The follow-up reviewed independently, findings fixed test-first
- [x] The quiet texts name the relay limit; the fake front's writer is always reaped
- [x] The last notes' fix reviewed independently

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
- `br_netfilter` (DFJ-012): the engine's environment, asserted in the engine's argv test; the
  error that names the switch is the sandbox engine's own (reproduced here).

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
- `br_netfilter` (DFJ-012): the engines stop asking for it rather than the host loading it.
  Loaded on the host, the module sends every bridged frame of the host through iptables
  (`bridge-nf-call-iptables` defaults to 1), so a host whose FORWARD policy drops (ufw's
  default) would cut its own bridged guests, and pinning the host namespace's switch to 0 needs
  a sysctl drop-in ordered after the module's load on every boot. ICC restriction inside one
  sandbox's engine protects containers of one tenant from each other, which Yulius does not
  promise; the sandbox's own network namespace is the boundary.

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

### Slice 2 (the jam seen once)

- Pill: `npx e2e run tests/connection-health.e2e.ts`: the new "a full Docker front says the
  sandbox’s Docker is full …" failed before (no "Docker full: studio"), 4 passed after.
- `a_full_front_is_one_docker_trouble_of_its_sandbox_until_it_has_room` (front_gate) and
  `the_upkeep_reports_its_fronts_state_as_the_sandboxs_docker_trouble` (manager, through
  `watch_front`; another front's file is not read): passed.
- `cargo test -p yulius --test main -- docker_trouble_notices::` (fake agent): told once, nothing
  more within the trouble, told once that it works again; another sandbox's trouble says
  nothing: passed.

### Slice 2, review round 1 (request changes; every finding own code, all fixed)

- An adopted front's old refusals began a trouble after every restart:
  `an_adopted_fronts_past_refusals_are_no_new_trouble` (a front first seen, not started by
  this server, baselines its refusals; one it started counts from zero).
- The pass typed into sessions where a person must act: it now skips an open Needs you, a
  prompt on screen, an open pause and an update's hold. The integration test sets a Needs you
  before "works again": nothing typed over it; red with the guard removed ("typed over a
  Needs you"), green with it.
- A told task not running at the end pass lost "works again": told tasks are kept until they
  rest (closed, stopped, failed), as the pull-request watch keeps them.
- Flapping: "works again" waits until the trouble stayed ended for three passes; a trouble that
  ends and begins again before that goes on as one (both in the integration test).
- Every path that drops a holder ends its trouble (`stop_holder`; tested in
  `the_upkeep_reports_its_fronts_state_as_the_sandboxs_docker_trouble`). "Works again" became
  "no longer full" (a holder that ended also ends the trouble). `docker` is documented as a
  wire value (`docs/api.md`, the contract and domain docs). The at-limit answer and the notice
  cite no Yulius path and say to raise a Needs you only past 15 minutes, until slice 3.
- Pre-existing, outside the change: the subscriptions e2e test that fails before midnight
  (issue #1334).

### Slice 2, review round 2 (request changes; every finding own code, all fixed)

- The notice-direction Needs-you check now holds the Needs you open for 3 s of passes after the
  session's start window (deterministic): red with the Needs-you clause removed ("typed over a
  Needs you" at the notice), green with it.
- Kept-until-rest and the four blocking reasons are pure functions with unit tests
  (`a_told_task_waits_for_the_end_until_it_rests`,
  `a_prompt_a_needs_you_a_pause_or_an_update_hold_each_block_typing`); the task is read again
  right before each paste.
- `a_front_this_server_starts_reports_its_refusals_as_new` (a fake front started by the
  upkeep): red with `started` removed from `keep_front`, green with it.
- A production holder's front is journaled only (`a_production_fronts_state_is_no_panel_trouble`).
- `BACK_AFTER` is 5 passes (75 s, past two upkeep ticks); stale "works again" wording updated.

### Slice 2, review round 3 (approve)

PR #1333 approved at 3e206c557 (round 3, Opus 5.5); its notes (the ADR's "three passes" and
"nobody needs to be asked" wording, two stale comments in `docker_trouble_notices.rs`) are fixed
in slice 3's pull request.

### Slice 3 (Yulius repairs it itself)

- `front_repair::tests`: a front full for two minutes is restarted once no CI job runs, waits
  while one runs at most ten minutes after it was due; room again cancels; three unanswered
  pings in a row make a repair due; the ping reads an engine's answer and fails within its
  bound.
- `a_front_full_for_long_is_restarted_when_no_ci_job_runs` (manager, a fake front): held while
  a CI job runs, restarted (a new front, counted) once none does;
  `an_asked_restart_starts_a_new_front_and_ends_the_trouble`: the restart asked for ends the
  sandbox's trouble once the new front's state is read.
- `the_superagent_restarts_its_sandboxs_docker_or_hears_why_not` (Cockpit MCP) and
  `the_cockpit_asks_before_every_changing_tool` (`docker_restart` asks first); a task agent is
  never served it (`mcp::tests`).
- The pill says Yulius restarts it (`connection-health.e2e.ts`, 4 passed).

### Slice 3, review round 1 (request changes; every finding own code, all fixed)

- The upkeep's ping kept every engine from idling out (it is a client): it is now asked only
  beside a client, with the engine settled (`should_ping`,
  `a_ping_is_asked_only_beside_a_client_of_a_settled_engine`); the front's state says whether a
  start or stop holds the engine (`engine_busy`).
- "Three in a row": a tick that does not ask starts the count again
  (`three_unanswered_pings_in_a_row_make_a_repair_due`); a refusal (503) or silence is no answer
  and the ping is bounded (`a_refusal_or_silence_is_no_answer`); a socket the server may not
  connect to (`PermissionDenied`) is no answer either way.
- Production holders are never restarted by the upkeep
  (`a_production_front_is_never_restarted_by_the_upkeep`).
- A front that does not start again keeps the trouble open, counts nothing and the asked restart
  says why (`a_front_that_does_not_start_again_keeps_the_trouble_open`); a restart is counted
  and journaled only once the new front serves; the repair starts over after a restart (second
  tick in `a_front_full_for_long_is_restarted_when_no_ci_job_runs`).
- `docker_restart` reaches the port with the caller's own sandbox and answers its success
  (`the_superagent_restarts_its_sandboxs_docker_or_hears_why_not`, a recording isolation).
- Not tested here: the server connecting to a real front's socket from the host (a QEMU guest
  would show it); by reading, the socket is the service user's own 0660 path socket in its
  state directory; a connect refused for permission is no answer either way, while a missing or
  refusing socket counts as unanswered.
- A repair after unanswered pings, or an asked restart with no trouble open, tells no task (no
  task was told of a trouble); the Superagent's answer says what stopped.

### Slice 3, review round 2 (request changes; every finding own code, all fixed)

- The ping followed a link planted at the front's socket (its directory is writable from the
  sandbox): it now connects through `connect_control` (`O_PATH | O_NOFOLLOW`, must be a socket)
  within one 5 s bound (`a_link_at_the_fronts_socket_is_never_followed`: the decoy behind the link
  sees no request).
- The ping rule is pinned at its call site:
  `the_upkeep_pings_only_beside_a_client_and_restarts_an_unanswered_engine` (a socket answered by
  the test: no ping with no client, a busy or no engine over nine ticks; three 503s beside a
  client restart the front, `Why::Unanswered`); `engine_now` is `(true, true)` while a start
  holds the lock.
- A successor front starts its own count (`a_successor_front_is_not_restarted_for_its_predecessors_jam`;
  `keep_front` resets it when it starts a front); a restart whose front starts only at a later
  tick is counted and journaled then. The instance reads the counts in slice 7
  (`/api/analytics/docker`).
- Wording: a relay or refuse front has no engine of its own to stop.

### Slice 3, review round 3 (request changes; own code, fixed)

- The pending restart's count had no test: `a_front_that_does_not_start_again_keeps_the_trouble_open`
  now puts the executable back and shows one count for a person's restart asked before the next
  tick (red before the fix: counted twice) and one for a pending restart that a later tick
  starts.
- `docs/operations.md` quotes the journal lines the code writes.
- Accepted, not changed: an unanswered ping waits up to 5 s under the sandbox's upkeep lock (once
  per tick, only beside a client of a running engine); bounded, and releasing the lock mid-tick
  would let a run start between the reading and the repair.
### Slice 4 (one shared problem, one Needs you)

- `requests_naming_the_sandboxs_docker_are_answered_together` (fake agents, two tasks): a person
  answering one ends both; the end of the Docker trouble ends both, and both hear that it is no
  longer full, each once.
- `npx e2e run tests/shared-needs-you.e2e.ts`: two tasks naming `docker` show as one row naming
  #4 and #12, and each on its own row once answered: failed before (no such row), passes after;
  `attention.e2e.ts`, `navigation.e2e.ts` still pass (22 passed).

### Slice 3, review round 4 (approve; its two nits fixed in the next pull request)

PR #1335 approved at 3714d7af1 (round 4, Opus 5.5). Its nits, own code: a restart that stopped no
front (none ran) is no restart, now or when a later tick starts one
(`an_asked_restart_that_stopped_nothing_is_not_counted_later`: red before, counted 1); the
operations line is wrapped.

### Slice 4, review round 1 (request changes; every finding own code, fixed)

- The end pass answered requests asked after the trouble ended: requests are answered only if
  asked before the end (or the restart), through `end_task_needs_you_asking` (a request replaced
  meanwhile stays). In `requests_naming_the_sandboxs_docker_are_answered_together`: a request
  asked after the end stays (red with the cutoff removed).
- A person's answer to one task dropped the others without a word: it spreads only from a
  running task's shown request, and the tasks answered through it are told (`DOCKER_ANSWERED`,
  typed with the usual guards); the test answers through the Superagent's `task_message`, and a
  request without the cause stays (red with the cause filter removed).
- The sidebar no longer hides rows: the shared tasks are grouped under one card naming them
  (links, first asker first) with their own rows, so keyboard, menus, ownership and counts are
  unchanged (`shared-needs-you.e2e.ts`: the card, its links' order, both rows, focus kept on an
  activity change, one notification entry).
- Tests: the tool's `cause` (accepted, shown, refused for an unknown one; `cd.rs`), the
  Superagent's `docker_restart` answering a request (`cockpit.rs`), the Superagent's task text
  naming the shared problem (`mcp_text`).
- The Evidence of slices 5 to 7 leaves this pull request with their code.

### Slice 4, review round 2 (request changes; every finding own code, fixed)

- The bell's one entry depended on the browser having loaded that sandbox's task list (6 of 10
  e2e runs failed): the server now names each shared request's episode (`NeedsYouView.episode`,
  the oldest open request naming the cause among the sandbox's working tasks; a resting task's
  stored request does not count), and the entry is keyed by it and names the project.
  `shared-needs-you.e2e.ts`: one entry also when another project is open; 10 of 10 runs passed.
  The integration test asserts both tasks share one episode and a task without the cause has
  none.
- A Continue of a stopped task with a stored Docker request answers no other
  (`continuing_a_stopped_task_answers_no_other_tasks_request`: red with the running check
  removed).
- The audit row names its source ("a person's answer to task #N", "the end of the sandbox's
  Docker trouble", "a restart asked through the Superagent") and is asserted for the task
  answered through another, not for the one answered directly.
- An entry created only by an answer or a restart no longer gets an "end" 75 s later: only an
  entry that saw a trouble, or a restart (whose own time is the cutoff), ends requests and tells
  that Docker works again.
- The texts say what answers a shared request (a person's answer, the trouble's end, a restart
  through the Superagent), not "Yulius's own repair" in general: an upkeep restart for an
  unanswered engine opens no trouble.

### Slice 5 (fronts do not run old code for days)

- `front_repair::tests`: `a_front_from_another_file_runs_another_build` (this test process
  against its own executable and `/bin/sh`), `a_quiet_front_holds_no_connection_and_runs_no_engine`
  (from a state file, and from `/proc` for a front without one: a child makes it busy).
- `an_outdated_front_is_replaced_once_it_is_quiet` (manager, the fake front, whose file is not
  the server's executable): kept while its state holds connections, replaced (counted) once it
  holds none. (Round 1 of the review changed how quiet is read; see below.)
- The quiet test first failed only under the shell tool here: its inherited stdin was a socket;
  the server starts fronts with a null stdin, as the test now does.

### Slice 6 (br_netfilter)

- Reproduced the failure in this sandbox's engine: `docker network create -o
  com.docker.network.bridge.enable_icc=false t476b` → "cannot restrict inter-container
  communication or run without the userland proxy: stat /proc/sys/net/bridge/
  bridge-nf-call-iptables: no such file or directory: set environment variable
  DOCKER_IGNORE_BR_NETFILTER_ERROR=1 to ignore".
- `engine::tests::the_engine_gets_its_own_namespaces_capabilities_and_environment` asserts the
  variable in the engine's argv: failed before, passes after.
- Not tested here: a network create on an engine started with the variable (this sandbox's
  engine is started by the production server, not by this branch; it takes it once its front
  starts again after the update).

### Slice 7 (measures on the instance)

- `give_ways_are_counted_once` (front_gate): each give-way counted once, an adopted front's
  history not again; `an_asked_restart_starts_a_new_front_and_ends_the_trouble` takes one
  `restarted` event, once.
- `the_instance_counts_docker_restarts_give_ways_and_needs_you_per_week` (Cockpit fixture, an
  isolation reporting events): restarts 2 (one of the production engine), give-ways 5, Docker
  Needs you 1, in one week; an unknown sandbox's event is dropped.
- `a_week_is_named_by_its_monday` (the week's Monday, UTC).
- `npx e2e run tests/pipeline.e2e.ts`: the Docker card's totals and "a week" figures (failed
  before: no card; 18 passed after).

### Slice 4, review round 3 (approve; its notes fixed in the next pull request)

PR #1337 approved at 9b39c0590 (round 3, Opus 5.5). Its notes, own code: the shared card's
tooltip names what answers the requests (a person's answer, the trouble's end, a restart through
the Superagent), and a stopping task's request no longer opens an episode (it is not shown).

### Slices 5 to 7, review round 1 (request changes; every finding own code, fixed)

Reviewer (Fable 5.2) at 56602da9d; Yulius CI of that head passed.

- Major: a front without a state file was never quiet. An idle real front holds four sockets
  (its runtime's signal pair and duplicate, and the listener), so "no socket but its listener"
  never held, and no front from before ADR-0344 would ever be replaced. Fixed: the socket count
  no longer decides (it stays a test helper).
- Major: the decision trusted a state up to 5 s old, so a client that connected after the last
  write could be cut. Fixed: quiet is read live just before the stop. The front must have no child
  and no connection at its socket, accepted or waiting, read from its namespace's
  `/proc/<pid>/net/unix`, where such sockets carry the listener's path; a state, if written,
  must agree. No listener seen at the path counts as a doubt, and a doubt is not quiet.
  `front_repair::tests::a_quiet_front_holds_no_connection_and_runs_no_engine` uses a front-like
  process with four sockets: idle is quiet, and a client waiting unaccepted is not. It failed
  against the old decision (idle not quiet). `a_front_with_an_engine_or_no_listener_is_not_quiet`
  covers a child, and a socket at another path. `an_outdated_front_is_replaced_once_it_is_quiet`
  (manager) holds the fake front stopped (SIGSTOP) with a client waiting while its state says
  held 0: kept. Without a state file it is replaced, and its `restarted` event says why.
- Minor: a restart whose new front started at a later tick was not in the measures. Fixed: that
  path emits its `restarted` event; `a_front_that_does_not_start_again_keeps_the_trouble_open`
  asserts it (red before).
- Minor: the Docker Needs you count was proven by a hand-written audit row. Now
  `cd::a_needs_you_request_reads_what_happened_problem_and_to_do` reads
  `GET /api/analytics/docker` before (0) and after (1) the real `needs_you` call with
  `cause: docker`. Red with the production audit write removed.
- Minor: the route had no HTTP test. Added
  `analytics::the_docker_read_counts_restarts_give_ways_and_needs_you_per_week`: two weeks,
  another sandbox's entry, one outside the range, a hidden sandbox (404) and a reversed range
  (422).
- Minor: the stopping-task rule had no test. Now
  `docker_trouble_notices::requests_naming_the_sandboxs_docker_are_answered_together` adds a
  stopping task with an older Docker request; the episode stays the first task's own request.
  Red with the rule reverted.
- Minor: a sentence in `docs/operations.md` sat in the networks bullet. It is moved, and it now
  says which fronts still need a manual restart: one from before ADR-0344 that leaked
  connections stays busy and writes no state, so it is never seen full.
- Nits: a quiet replacement is journaled at info level as a replacement, no longer as a cut
  engine. `record_docker_events` uses `production_sandbox`. `docs/api.md` states the 20,000-entry
  bound. Slice 6's evidence is no longer duplicated under slice 4. Slice 4's review plan item is
  ticked.
- Direction notes, answered in words: ICC is not enforced inside one sandbox's engine; ADR-0344
  and operations.md say so. "Front restarts" mixes reasons, and the audit detail keeps the reason
  for a split later. The engine-level network create with the variable is not run here; see
  slice 6.

### Slices 5 to 7, review round 2 (approve; its notes fixed in a follow-up)

PR #1338 was approved at 0c3f8feec (round 2, Fable 5.2). Yulius CI run 24871c45 passed: quality and
e2e passed, and security was skipped because Vidoc is off for this project or the whole instance.
The PR merged into dev at that head. The reviewer confirmed on this host that the sandbox's
production front is listed in `/proc/net/unix` under `state_dir/docker/docker.sock`. A connection
made through it shows as one non-listening row there and disappears when closed. The new tests
passed 30 sequential runs and 270 loaded ones. Its notes, own code, fixed in the follow-up:

- The table lists only clients in the front's network namespace, because the kernel creates the
  accepted socket in the client's namespace. DFJ-010, ADR-0344 and the code comment now say so,
  and that the engine covers a container's connection, since the engine is the front's child.
- The window runs until the new front is `ready`, not only until the stop: a call that comes in
  that time fails at once. DFJ-010, ADR-0344, operations.md and the journal's comment now say
  so (each case named since the last follow-up).
- `a_quiet_front_holds_no_connection_and_runs_no_engine` now also holds a connection the front
  has accepted, with its state saying 0, and the front is not quiet.
- Pre-existing: two manager tests that rewrite the fake front's executable and start it at once
  failed twice in 240 loaded runs. That is issue #1339, outside this change. Their lines come
  from slice 3.

### Follow-up: the last review's leftovers (#1339, #1341)

- #1339, own code (slice 3's tests). Reproduced 5 times in 120 runs of `manager::` with 32
  threads. A probe printed `Text file busy (os error 26)` from `start_front`: another test forked
  while this process held the fake front's file open for writing, and the child kept it open
  until its exec. `put_executable` now has `sh` write a staged file, which the test process
  renames into place. After the fix, 0 of 300 runs failed under the same load.
- #1341, texts. The relay-front limit is stated in `connections_on`, DFJ-010 and ADR-0344. The
  window wording is the same in DFJ-010, ADR-0344, operations.md and the `quiet` and manager
  comments, and covers a call that is cut after it was accepted. The body says `Status: done`.
  The test pins the reason: `connections_on` is `Some(1)` for an accepted connection.

### Follow-up: the last review's notes

PR #1342 was approved at 6ac2c4d16, Yulius CI run 0b52dea0 passed, and it merged at that head.
#1339 and #1341 are closed with their evidence. The reviewer's own load loop showed 0 failures
in 2240 runs at that head and 13 in 640 with the binary from before the fix. Its notes, own code:

- The `quiet` doc and DFJ-010 now name the relay-front limit beside the window.
- The window says what a call meets: its socket missing or refused, or cut once the old front
  accepted it. This wording is in DFJ-010, ADR-0344, operations.md and both code comments.
- `put_executable` reaps its writer whatever the write did, and only then reports the write's
  error.
- Round 2's record no longer quotes the earlier wording as current.

### On the instance

The instance runs main a040c3b5b (`/internal/version`), from before slice 1. Promoting dev to
main is a person's, so the task asks for it with `needs_you`: promote, update, then read the
Docker card and the journal's replacement of the kept fronts. Until then the measures are read
on dev's build in the tests named above: the HTTP read, the Cockpit read and the e2e Pipeline
card.

### Not tested here

- A real front in another user namespace being read by the server: namespaces are refused in
  this sandbox. The reviewer read the kernel source instead: `get_proc_task_net` has no ptrace
  check. On this host, the production front of this sandbox was read as expected.
- A network create with `enable_icc=false` on an engine started with the variable. See slice 6.
- A real front restart on the instance (repair, request, or replacement after an update). The
  manager tests use a fake front with the same contract (socket, `ready`, state file), and
  fronts in this sandbox were never filled or restarted (task rule).
