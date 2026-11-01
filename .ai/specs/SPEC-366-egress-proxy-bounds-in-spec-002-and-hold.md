---
yulius-task: 4dbbe8d1-89f3-4613-873f-551636744ed8
status: in-progress
---

# SPEC-366: The egress proxy's bounds in SPEC-002, and its holders in the proxy-full warning

Status: in progress
Date: 2026-11-01
Owner: Yulius task #349
Scope authorization: task #349, the remainder of issue #964 found by task #348's verification.
Builds on SPEC-353 (LAP-002, ADR-0240), SPEC-355 (SLR-002, ADR-0241), SPEC-357 (ESR-002 to
ESR-005, ADR-0243) and SPEC-361 (CHR-001, ADR-0246).

## Goal

Close issue #964. Its fair share, idle give-way, busiest-run give-way and trouble report were
delivered by PRs #971, #976, #984 and #992 without linking it. What is left:

1. SPEC-002 §2.7 still states the proxy's bounds as before #964 ("256 connections, … 24 h per
   connection"). It must state every bound and link the ADRs that decided them.
2. The proxy's own once-a-minute warning at a full proxy (`egress proxy: every connection is
   taken`) names only the asking sandbox, so the journal does not say who holds the connections.
   It must name the busiest holders with their counts, as the share warning's trouble does.
3. Criterion 3 of #964 (idle tunnels reclaimed after a bounded quiet time) is met as ADR-0241
   decided: an idle connection gives way once a limit is full and it was idle for `IDLE_FLOOR`
   (300 s), not on a timer. A test with an explicit clock shows it at the proxy's real floor.

## Context

- **Symptom.** On 2026-10-30 at about 21:36 the proxy answered every `CONNECT` with
  `503 Yulius proxy: too many connections; try again` for minutes. The journal line of a full
  proxy, `crates/yulius-egress/src/proxy.rs` in `EgressProxy::slot`, logs `sandbox` (the asker)
  only: whoever reads it cannot tell which sandbox held the 256 places.
- **Root cause of the gap.** `Slots::admit` computes the busiest holders
  (`connection_health::busiest(table.keys())` at the proxy's limit,
  `busiest(table.owners_of(&key))` at a share) under the table's lock, but hands them only to the
  trouble registry (SPEC-361). `slot()` receives the answer and the asking run's counts, never the
  holders, so its own warning cannot name them. SPEC-361's trouble line (`connection trouble
  began`) does name them, but only once per trouble; the proxy's own line, once a minute, does
  not. No test reads the proxy's warnings.
- **SPEC-002 §2.7** was not updated by SPEC-353, 355, 357 or 361.
- **Coordination.** Task #352 (#975, plain-HTTP head timeout) changes the same file; this change
  keeps its part of `proxy.rs` to `Slots::admit`'s return and the two warnings.

## Requirements

- **EPB-001** The warning at a full proxy (`egress proxy: every connection is taken`) carries
  `holders`: the busiest sandboxes, at most 3, busiest first, each as `<sandbox> (<count>)`, as
  counted at that refusal. The warning at a full share carries the busiest runs of the sandbox
  the same way. Both stay at most once a minute per limit and sandbox (LAP-002).
- **EPB-002** SPEC-002 §2.7 states the bounds: 256 connections in all, 128 per sandbox, the
  longest-idle connection past 300 s giving way at a full share or proxy (never one in flight),
  then the busiest other run or sandbox holding 2 more, the `503` texts, the warnings and the
  trouble on the panel, the timeouts, and 24 h per connection; it links ADR-0240, 0241, 0243 and
  0246.
- **EPB-003** A test shows, at the proxy's own `IDLE_FLOOR` and default configuration, that a
  connection idle for the floor gives way at a full limit and one idle for less does not.

## Plan

- [ ] Proxy-full and share warnings name the busiest holders, test-first
- [ ] Idle connections give way at IDLE_FLOOR, shown with an explicit clock
- [ ] SPEC-002 §2.7 states every egress bound and links its ADRs
- [ ] Independent review, gates green, changes log and delivery

## Acceptance criteria

- A test fills a proxy evenly from two sandboxes; the refused `CONNECT` writes one `warn` line
  `egress proxy: every connection is taken` whose `holders` names both sandboxes with their counts;
  a second refusal within the minute writes none. Red before the change.
- A test with the default `EgressConfig` floor admits at a full limit by closing a connection idle
  for exactly `IDLE_FLOOR`, and refuses while the remaining one is idle 1 ms less.
- SPEC-002 §2.7 states the bounds of EPB-002 and links the four ADRs.
- `scripts/dev ci` green; Yulius CI green on the PR head.

## Test plan

- Integration (`crates/yulius-egress/tests/proxy.rs`): a full proxy's warning names its holders,
  logs captured by a thread-local `tracing` subscriber (the test runtime is current-thread, so
  the proxy's tasks log into it).
- Unit (`crates/yulius-egress/src/proxy.rs`): the idle floor with explicit `now`.
- Failure cases: a refusal within the minute writes no second line; the younger idle connection
  is not taken.

## Decisions

- **No new ADR.** The bounds were decided by ADR-0240 (share of 128 of 256), ADR-0241 (give-way of
  the longest-idle past 300 s instead of a fixed idle timeout), ADR-0243 (the busiest run or
  sandbox gives way) and ADR-0246 (the trouble on the panel); adding the holders to an existing
  log line decides nothing lasting. SPEC-002 links them.
- **Criterion 3 stays as ADR-0241 decided**: closing every tunnel quiet for 300 s would cut
  legitimate long polls and idle keep-alive connections while nobody else needs the place; a
  connection gives way only when a limit is full. Not reversed here.
- **Holders formatted once, in the domain** (`connection_health::listed`), so the proxy's warning
  and the app's trouble line read the same.
- **The log test captures with `tracing-subscriber`** (already a workspace dependency), as a
  dev-dependency of `yulius-egress` only.
