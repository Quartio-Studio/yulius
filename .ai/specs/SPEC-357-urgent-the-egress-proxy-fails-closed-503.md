---
yulius-task: 298e9e28-cb28-4826-bff6-3009173e18e2
status: in-progress
---

# SPEC-357: One busy run no longer closes the egress proxy for its whole sandbox

Status: in progress
Date: 2026-10-31
Owner: Yulius task #342
Scope authorization: task #342 (replaces #341), the quartio-v2 sandbox `7c46cdac-…` on the
production host, 2026-10-31. Builds on SPEC-353 (LAP-002, the per-sandbox share) and SPEC-355
(SLR-002, idle connections give way).

## Goal

A run whose processes hold many connections through the egress proxy no longer cuts off the
other runs of its sandbox. When a sandbox's share is full of active connections, the run holding
the most gives way to one holding fewer. A run is refused only when no other run has 2 more to
give up, and the message says how many it holds and whether that is the most.

## Context

### Symptom

From about 10:50 UTC to at least 12:17 UTC on 2026-10-31, every `CONNECT` from the quartio-v2
sandbox through `127.0.0.1:8081` was answered `503` (api.github.com, github.com, pypi.org,
registry.npmjs.org, crates.io and the project's dev site). At 12:20 UTC the same proxy tunneled fine
from the yulius sandbox. Every task in quartio-v2 lost git, gh, package downloads and its dev
site.

### Root cause

- **Only the share answers `CONNECT` with 503.** In `EgressProxy::handle` the only `503` is
  `slot()` refusing a place: `Full::Key` ("this sandbox's processes hold all of its 128
  connections") or `Full::Total`. A failed upstream dial happens after the `200`. A failed
  destination check is a `403`. An unknown token is a `407`. The policy state ("fingerprints") is
  not read on this path.
- **"Only api.anthropic.com works" is no exception.** Every run's environment has
  `NO_PROXY=…,anthropic.com,.anthropic.com,claude.ai,.claude.ai`. Claude's own traffic bypasses
  the proxy, and the Yulius MCP route is `127.0.0.1`, also outside it. So a proxy that refuses
  everything leaves exactly that working.
- **One sandbox, not the instance.** Another sandbox was served at the same time, so the limit met
  was quartio-v2's share (`Full::Key`), not the proxy's total.
- **Timeline.** Release #977 (`6658488f`, promoted 09:59 UTC) shipped SPEC-353's per-sandbox share
  of 128 without SPEC-355's give-way. A tunnel then held its place until it ended or for 24 h
  (`CONNECTION_MAX`). Before #977 the same connections only met the proxy's total of 256. Once
  quartio-v2's many parallel tasks and their leftovers (test jobs, a headless Chrome) held 128
  tunnels, every new one from any of its tasks was refused, until the server restarted. Release
  #979 (`42be970b`, promoted 12:22 UTC, now running) restarted the server, which emptied the
  table. Its SPEC-355 give-way closes a connection idle for 5 minutes, which is enough for
  leftovers that hold idle sockets.
- **What still fails closed on dev.** A connection that stays active (a page's live stream or
  WebSocket, polling, a long download) never gives way. At the share,
  `Connections::admit` looks only at the sandbox and refuses the new connection when none was idle
  for the floor. So a single run whose processes keep their connections active (an e2e run's
  leaked browsers, say) still locks every other run of the sandbox out of the proxy. Every task
  stalls because of one, and the 503 blames "this sandbox's processes" with no hint of whose.
- **Why tests missed it.** `one_sandbox_cannot_take_every_connection` checks the share with two
  runs holding 4 places each, which is the fair case. No test fills a sandbox's share from **one**
  run and then asks from another.

## Requirements

- **ESR-001** The proxy knows whose each connection is: the run (attempt), the CI job or the
  Superagent's shell (`EgressClient::use_key`), within its sandbox.
- **ESR-002** At a sandbox's share, the sandbox's longest-idle connection past the floor gives way
  first, as SPEC-355 SLR-002 already does. If none was idle that long, the oldest connection of
  the sandbox's busiest other run gives way, when that run holds at least 2 more than the asking
  run. A connection held in flight (a plain request waiting for its answer) never gives way. Only
  when no run holds 2 more than the asker is the connection refused. A sandbox whose runs share
  its places evenly is therefore refused as before.
- **ESR-003** At the proxy's total the same rule applies across sandboxes. The longest-idle
  connection of any sandbox past the floor gives way first. Otherwise the busiest sandbox's busiest
  run gives up its oldest connection, when that sandbox holds at least 2 more than the asking
  sandbox.
- **ESR-004** The refusal says who holds the places. At the share: how many of the sandbox's
  connections the asking run holds, and that it is the busiest run, or that the runs share them
  evenly. The proxy's own limit keeps its text.
- **ESR-005** A connection taken from a busier run is logged as a warning, at most once a minute
  per sandbox, naming the sandbox, the run and how many it held. Ordinary give-way and refusals
  stay as SPEC-353 and SPEC-355 log them.
- **ESR-006** Relays keep their behaviour: one key and one owner, the SPEC-355 give-way only.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec and ADR for the per-run give-way
- [x] The busiest run gives way at a sandbox's share, with a regression test
- [x] The busiest sandbox gives way at the proxy's limit
- [x] Refusals and warnings name whose the connections are
- [x] Operations guide, architecture and changes; follow-up tasks for the rest of #342
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green, pull request merged into dev on green CI

## Acceptance criteria

- One run of a sandbox holds its whole share with tunnels active within the floor. Another run of
  the same sandbox then gets a tunnel (red before: `503 … this sandbox's processes hold all of its
  8 connections`), and the busy run's oldest tunnel closes.
- The busy run asking again is refused `503`, and the text says that this run holds the sandbox's
  connections.
- Two runs holding 4 places each in a share of 8 are refused as before (the existing test).
- A plain request held in flight is never taken. The relays' tables behave as before (their
  tests).
- At the proxy's limit, a sandbox within its share gets a place from the busiest sandbox even when
  every connection is active.

## Test plan

- Unit (`yulius-domain::connections`): taking from the busiest owner at a key's share and from the
  busiest key at the total, the margin of 2, held connections, and ties going to the earliest.
- Integration (`yulius-egress/tests/proxy.rs`): real `CONNECT` tunnels from two runs of one
  sandbox at the default (300 s) floor, which is today's failure. Also a full proxy with active
  tunnels from two sandboxes.
- Existing share, give-way and relay tests unchanged and green.

## Decisions

- ADR-0243: at a full share or a full proxy, an active connection of the busiest run (or sandbox)
  gives way to a run (or sandbox) holding fewer. A fixed per-run cap was rejected: it lowers a
  lone run's ceiling, and many runs below their caps still fill the share. Raising the share was
  rejected: the sandbox's relay toward Yulius (256) also carries its MCP and hook calls, and
  SPEC-353 keeps half of it free for them. Closing the oldest active connection of the busiest run
  favours the run that holds the least, and leftover live streams are the oldest.
- The rest of task #342 (MCP clients reconnecting after a restart, waking tasks once Yulius is
  back, proxy and MCP health on the panel, CI slots per job and local builds yielding to CI) is
  separate. Each is its own concern and goes to its own chained task, so this urgent fix ships
  alone: #343 (MCP, waits and delivery across restarts), #344 (proxy and MCP health on the panel)
  and #345 (CI slots per job, local builds yielding to CI). Issue #982 tracks this defect.
- Restoring service needed no operator action from this task: the update to #979 at 12:22 UTC
  restarted the server, which emptied the proxy's table, and its SPEC-355 give-way frees idle
  places since. The owner was asked (needs_you) to confirm from quartio-v2 and given the
  journal and `nsenter … ss` recipe in case active holders fill the share again before this
  fix is deployed.

## Review

Independent reviews in fresh sessions (skill `yulius-review`), read-only over
`origin/dev...HEAD`:

- Round 1 on 1fb0e847: approve with no critical or major findings. It found seven minor
  own-code findings and four nits, all fixed in this pull request. The tests were strengthened
  first (025a7e09, red: the refusal texts and `Busier` did not exist).
  - Counts for the refusal were computed on every connection under the table's lock. Now they
    are computed only on a refusal.
  - The refusal counted runs whose connections were all held in flight, so "no other run holds
    2 more" could be false. `most_of_others` now uses the same takeable-aware choice as
    admission (unit test).
  - A sandbox-level take was logged as a run's, with the sandbox's count.
    `Evicted::busier` is now `Busier::Owner(n)` or `Busier::Key(n)`, each with its own warning.
  - Test gaps closed: the two refusal texts, a run holding just one more than the asker, a
    fully held busiest run passed over for the next, and which tunnel closes at the full proxy.
  - The operations guide's "only when every connection was active" and its list of warnings
    now cover the new rule. The ADR's consequence about finding the run's processes was
    reworded.
  - Nits: `Open`'s generic order matches `Evicted`'s; the `Full` texts mention the busier
    owner; the refusal reads "the most of the sandbox's runs".
- Round 2 on baba7a66: approve. Two minor own-code findings, both fixed test-first (the red
  commit has `share_refusal` and `most_of_others` with held connections missing):
  - The refusal could call the asker "the most" while another run held more connections, all
    held in flight. "The most" now compares every connection, held ones included
    (`share_refusal`, unit test). `most_of_others` counts them. The takeable-aware count went
    away: "no other run has 2 more to give up" is what the table found.
  - The run-level and sandbox-level takes shared one warning's rate limit. They are now
    separate kinds (`busier_kind`, unit test).
  - Nits fixed:
    - The refusal's margin comes from `MARGIN`.
    - The changes entry no longer claims only the busiest is refused.
    - The operations guide says which id names a CI job and a Superagent.
    - The `Slots` doc mentions the take.
  - Nit left: `busiest()` is quadratic in the open connections, at most 256. It runs only at a
    full share with nothing idle, at about 0.3 ms by the reviewer's estimate. A one-pass count
    is worth it only if the limits grow.
- Round 3 on fe8b3d72: request changes. Two minor own-code findings and three nits, all fixed
  test-first (the red commit, `a_refused_destination_takes_no_place_from_a_busier_run`: the busy
  run got `200` after a sibling's refused request had taken its place):
  - A request refused anyway (a bad target, a local or unresolvable destination) took its place
    first. At a full share, that could close a busier run's active tunnel for nothing. `handle`
    now takes the place only after the target and destination checks, and a plain request is
    held in flight at once (the nit about the window before `hold()`).
  - Nothing tested that an idle connection goes before an active one is taken. Now
    `an_idle_connection_goes_before_the_busiest_gives_way` covers the share and the table, with
    the idle one not the busiest's oldest. It passed on first run because the behaviour was
    already right; it guards against a swap.
  - Docs: the Goal, ADR-0243, the operations guide and two doc comments said "only the busiest
    is refused" or that relays are safe for having a single owner. Reworded.
- Round 4 on cd51f9f1: request changes. The round-3 fixes were verified, and three more minor
  own-code findings came up, all fixed test-first (red commit:
  `refused_plain_requests_take_no_place_from_a_busier_run` got `200` after a refused upgrade,
  and `admit_held_by` did not exist):
  - A plain request was held only after `slot()` released the table, so another run could
    take it in between. It is now admitted held under the table's lock
    (`Connections::admit_held_by`, unit test). Round 3's claim that the gap was closed was
    wrong.
  - Plain requests refused after the place was taken (a protocol upgrade, a policy refusal)
    could still close a busier run's tunnel. `forward` is split into `prepare` (upgrade check,
    hop headers, the policy) and `send`, and `handle` takes the place between them.
  - The plain path's reordering had no test. The new test covers a local destination (`403`),
    an upgrade (`501`) and a policy refusal (`409`).
  - Nit kept, as the reviewer allowed: a request refused at a full share now pays its DNS lookup
    and policy check first.

