---
yulius-task: ec28e4c6-607e-424a-8865-3523f28bce59
status: in-progress
---

# SPEC-436: Yulius never reconnects a live Yulius MCP

Status: in progress
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
- [ ] Regression test: a bridge that never pings is never reconnected
- [ ] The watch leaves a live bridge alone and reconnects only a dropped one
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- An idle, Running task heard but never pinged is not reconnected at 3, 30 or 300 minutes of
  silence, whether its run was spawned by this server or reattached.
- A reattached run never heard by this server is not reconnected.
- The reconnect loop of the logs cannot happen: after a reconnect answered by a bridge that does
  not ping, no further reconnect is typed.
- A bridge that pinged and stopped, and a run spawned here that never connected, are still
  reconnected, and told to continue as before.

## Test plan

- Unit (`yulius-app::mcp_watch`): the regression test
  `a_bridge_that_never_pings_is_never_reconnected` (heard, spawned here and reattached, silent up
  to 5 hours) and `a_reconnect_answered_by_a_bridge_that_does_not_ping_is_the_last`, both failing
  before the fix; the existing tests of the reattached and heard-before-first-ping cases now
  expect no reconnect; the other MCR-003/MCR-004 tests stay as they are.
- Integration (`yulius/tests/restart.rs`): the existing silent-task test, whose bridge pinged, still
  gets the reconnect and the continue text.

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
