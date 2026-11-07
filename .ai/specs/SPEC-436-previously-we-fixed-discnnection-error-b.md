---
yulius-task: ec28e4c6-607e-424a-8865-3523f28bce59
status: done
---

# SPEC-436: Yulius never reconnects a live Yulius MCP

Status: implemented
Date: 2026-11-07
Owner: Yulius task #418
Scope authorization: task #418 ("previously we fixed disconnection error but running reconnect is
a patch. Just make it not disconnecting"), with a screenshot of a `/goal` session showing
`/mcp reconnect yulius` → "Successfully reconnected to yulius" four times in a row. Fixes #1108.
Amends [SPEC-432](SPEC-432-fix-so-this-never-ever-happen-again-also.md) MCR-003 and ADR-0276.

## Goal

A task session's Yulius MCP stays connected: Yulius never types `/mcp reconnect yulius` into a
session whose bridge may still be alive. Only a bridge known to be gone is reconnected.

## Context

### Symptom

Idle task sessions show `/mcp reconnect yulius` and "Successfully reconnected to yulius" again and
again. To a person it reads as the server disconnecting all the time.

### What really happens

Claude Code's MCP logs in the Yulius sandbox (`~/.cache/claude-cli-nodejs/*/mcp-logs-yulius/`):

- Three idle sessions connected on 2026-11-06 at 20:19, 20:45 and 21:02 UTC stayed connected for
  hours. From 2026-11-07 00:19 UTC, the first server build with the SPEC-432 watch, until the
  sessions ended at 07:51 UTC, each logged `Sending SIGINT to MCP server process`, "exited
  cleanly", then a new connection in under 600 ms, every 210 s: 129-130 times each. 210 s is the
  watch's 3-minute silence plus its 30 s tick.
- Across 529 connections there since 2026-10-25 no `yulius` server failed on its own (3
  `ENOENT` failures came from a smoke test's fake setup). The slowest connect took 2 s of the
  30 s bound.

So the server was not disconnecting: Yulius was restarting a healthy server every 3½ minutes.

### Root cause

`mcp_watch::decide` counts a task as cut off when nothing reached `POST /mcp` from its session
for 3 minutes, whatever kind of bridge it has. An idle bridge sends only its pings, and a bridge
older than SPEC-432 sends none. A running sandbox keeps the `yulius` binary of the sandbox base it
started on (the agent's `/usr/local/bin/yulius` is in the base's lower layer), so after an update
its bridges, including the ones `/mcp reconnect` spawns, are the old build until the sandbox is
restarted on a new base. Every reconnect is answered by the new bridge's `initialize`, which
resets the watch's state, so the backoff after 3 unanswered reconnects never starts. ADR-0276
expected "one `/mcp reconnect yulius`, which starts the new bridge"; the new bridge is not new.

The existing tests missed it because they encode the faulty expectation:
`a_reattached_session_that_never_pinged_is_reconnected_but_not_woken` and
`a_session_heard_before_its_first_ping_is_cut_off_from_that_contact` reconnect a bridge that was
heard but never pinged. No test follows the watch across a reconnect answered by a bridge that
still does not ping.

## Requirements

- **NRC-001 A bridge that does not ping is left alone.** A task whose session's MCP client reached
  `POST /mcp` on this server but never pinged here is never reconnected, however long it is
  silent: its bridge may be an older build that is alive.
- **NRC-002 A reattached run never heard is left alone.** A run spawned before this server started
  that has not reached it is never reconnected: its bridge may be an older build.
- **NRC-003 A bridge known to be gone is reconnected.** Unchanged from SPEC-432 MCR-003 otherwise:
  a bridge that pinged on this server and went silent for 3 minutes, or a run this server spawned
  whose MCP never reached it for 3 minutes, gets `/mcp reconnect yulius` when idle, spaced 10
  minutes (hourly after 3 unanswered). MCR-004 (the continue text) is unchanged for them.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec, issue and ADR for reconnecting only a bridge known to ping
- [x] Regression test: a bridge that never pings is never reconnected
- [x] The watch leaves a live bridge alone and reconnects only a dropped one
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- An idle, Running task heard but never pinged is not reconnected at 3, 30 or 300 minutes of
  silence, whether its run was spawned by this server or reattached.
- A reattached run never heard by this server is not reconnected.
- The reconnect loop of the logs cannot happen: in a run whose bridge never pinged on this server,
  a reconnect answered by a bridge that does not ping is the last one typed.
- A bridge that pinged and stopped, and a run spawned here that never connected, are still
  reconnected, and told to continue as before.

## Test plan

- Unit (`yulius-app::mcp_watch`): the regression test
  `a_bridge_that_never_pings_is_never_reconnected` (heard, spawned here and reattached, silent up
  to 5 hours) and `a_reconnect_answered_by_a_bridge_that_does_not_ping_is_the_last`, both failing
  before the fix; they replace the two tests that reconnected a bridge heard (or reattached)
  but never pinged, which are deleted; the other MCR-003/MCR-004 tests stay as they are.
- Unit (`yulius-app::mcp_watch`): `a_reattached_bridge_that_pinged_here_and_stopped_is_reconnected`,
  the usual real drop after an update or restart (every live run is then reattached).
- Integration (`yulius/tests/restart.rs`): the existing silent-task test, whose fake agent never
  reaches `POST /mcp` (a run spawned here that never connected), still gets the reconnect and the
  continue text.

## Decisions

- **Reconnect only on evidence that the bridge is gone** (ADR-0280). A bridge that pinged and
  stopped is gone (it pings every minute whatever the session does); a session spawned by this
  server that never reached it never connected. Anything else heard may be alive. Alternatives:
  - *Keep reconnecting, but give up after 3 reconnects answered by a bridge that does not ping.*
    Still restarts a healthy server 3 times per session per server start, which is what the person
    sees as disconnecting.
  - *Make the sandbox's bridge binary current (restart sandboxes on the new base after an
    update).* Cuts every running session; the skew is harmless once the watch stops assuming
    pings.
  - *The bridge pings at once on start, so "heard, never pinged" means an older bridge for sure.*
    Only narrows the remaining gap (a new bridge dying within its first minute); not needed for
    the fix, so left out.
  - *Remove the watch.* A real drop (SPEC-432's FW-N2-Device incident) would again wait for a
    person; with this rule the watch only acts when the server is gone, so it never disconnects
    anything.
- **What is no longer covered:** a bridge that dies before its first ping (60 s after it starts),
  and a reattached run dropped before the server restarted, are not reconnected. Claude Code's
  logs show neither in 529 connections; `yulius tool` stays the shell fallback.
- Found on the way, outside this change: the `/yulius/bin/yulius` wrapper refuses `yulius tool`
  (#1109).

## Review

Independent reviewer in a fresh session (read-only, `om-code-review`, `yulius-review`), on
`origin/dev...HEAD`.

- **Round 1** (`1881bc33`): request changes; all own code, no defect in `decide`.
  - Medium: no test of the usual real drop after an update (a reattached run whose bridge pinged
    here and stopped). Added `a_reattached_bridge_that_pinged_here_and_stopped_is_reconnected`;
    adding `&& facts.spawned_ms >= bounds.started.0` to the ping branch makes it fail.
  - Low: the test plan said the integration test's bridge pinged (it never reaches `POST /mcp`);
    the loop guarantee was wider than the code (now "in a run whose bridge never pinged here");
    three doc sentences (ADR-0276 note, operations, the decisions index). All fixed.
  - Informational, not adopted: treat a never-pinged bridge as gone once another bridge of the
    same sandbox pinged here (the binary then pings). Narrows the accepted gap; left for later.
- **Round 2** (`3ff6ac27`): approve, no new findings.

## Evidence

- Root cause from Claude Code's own logs (`~/.cache/claude-cli-nodejs/*/mcp-logs-yulius/`): 3 idle
  sessions reconnected every 210 s from 2026-11-07 00:19 to 07:51 UTC (129-130 `Sending SIGINT to
  MCP server process` each, each followed by a clean connect); no server failure on its own in
  529 connections since 2026-10-25.
- Regression tests failed before the fix (`eed25a0f`):
  `a_bridge_that_never_pings_is_never_reconnected` and
  `a_reconnect_answered_by_a_bridge_that_does_not_ping_is_the_last` (`left: Reconnect, right:
  None`); pass after it. `cargo nextest run -p yulius-app -E 'test(/mcp_watch::/)'`: 11 passed.
- Integration: `restart::a_task_whose_yulius_mcp_went_silent_is_reconnected_then_continues` and
  `mcp_coordination::an_agent_request_records_its_tasks_mcp_contact` passed.
- `scripts/dev ci` on `3ff6ac27`: doctor, check, lint, macos passed; nextest 3199 of 3200 passed,
  1 failed: `cockpit::an_idle_superagents_strays_are_reaped_after_the_bound` timed out waiting
  for the reap after 21 s under the gate's load. It passed 5 of 5 runs alone (2.3 s each), it is
  outside this change (cockpit stray reaping), and it is #1110. The stages after nextest, run
  separately on the same head: `scripts/doctests.py` exit 0, `scripts/dev deps` exit 0,
  `cargo doc --workspace --all-features --no-deps` exit 0. So the gate is green apart from that
  flaky test; Yulius CI on the pull request head is the gate of record.
- Security scan: Vidoc is turned off for this project in Yulius (`yulius security-scan` reports
  it disabled); not run, not passed.
- Not tested: a live instance with a sandbox on an older base after this build is installed (the
  rule is unit-tested with the facts the logs show); sessions in other sandboxes, such as the
  screenshot's, whose logs are out of this sandbox's reach.
