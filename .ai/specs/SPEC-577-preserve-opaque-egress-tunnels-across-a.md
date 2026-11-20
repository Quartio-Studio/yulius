---
yulius-task: 35adb14d-65fe-4a9f-b9fa-370d776d4139
status: in-progress
---

# SPEC-577. Preserve established opaque CONNECT tunnels during a server update

## Goal

A sandbox agent that has established an opaque HTTPS tunnel can continue exchanging bytes
on the same connection after an orderly Yulius update. A destination can keep sending while
the agent is paused, subject to the usual socket backpressure.

## Context

[Issue #529](https://github.com/Quartio-Studio/yulius/issues/529) follows
[SPEC-160](SPEC-160-tasks-should-be-paused-imidietly-not-wai.md) and
[ADR-0131](../../docs/decisions/0131-an-update-freezes-every-task-at-once.md).
At dev `389789466`, the holder forwards an agent's internal port to `out.sock`. The manager
accepts that Unix connection and copies it to the internal TCP listener. The proxy checks
the live client, destination and applicable host bindings before upgrading CONNECT and copying
bytes to the destination. Both copies run in the server. The SPEC-355 restart fixture makes a
new request after the old sockets reach EOF. The issue requires the original connection.

## Requirements

- **TUN-001** An established agent tunnel through the sandbox forward exchanges ordered bytes
  both ways on the same sockets after the original server exits and its successor starts.
  Pending bytes and independent write-half closure survive the handoff.
- **TUN-002** The live server owns all new admissions, destination checks, interception,
  credential injection and redaction. Intercepted streams end at restart. Each reconnection
  follows the current host policy. Only an established opaque pair and its matching relay
  qualify for handoff.
- **TUN-003** Restored connections count against the existing relay, sandbox and global
  limits before new admissions. Preserve byte activity, the five-minute idle eviction floor,
  the proxy's 24-hour lifetime and the existing fairness and cancellation contracts.
- **TUN-004** Cancelling an outbound relay releases its accepted sockets and their accounting.
  An orderly update transfers qualifying sockets before cancelling the old relay. This also
  keeps old relay tasks from owning sockets alongside their replacements.
  Peer closure, eviction, expired handoff and explicit cancellation reclaim all associated
  descriptors and buffers. A bounded restart handoff has a single consumer and fails closed
  if the replacement cannot adopt it. An ordinary stop has a bounded cleanup path.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Release outbound relay sockets when their serving task is cancelled
- [ ] Obtain independent approval for the relay lifecycle slice
- [ ] Preserve buffered bytes and half-closes when an established copy pauses
- [ ] Obtain independent approval for the resumable copy slice
- [ ] Hand established opaque connections and matching relay sockets to the restarted server
- [ ] Verify same-socket continuity, admission limits and intercepted traffic through restart
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1 (TUN-001)** A synthetic destination accepts once. The client establishes CONNECT
  through the holder forward, exchanges markers, and retains its socket. The old server exits.
  The destination writes a distinct marker before the successor starts. After the new server
  proves readiness, the client reads that marker exactly once and in order. The two existing
  peers then exchange fresh markers in both directions. Include partial transfer and write-half
  closure.
- **AC-2 (TUN-002)** A synthetic secret applies only to its bound destination. Its reflected
  value is redacted. Intercepted streams reconnect through the same checks after restart.
  Invalid credentials and denied destinations cannot obtain a handoff.
- **AC-3 (TUN-003)** A restored active tunnel consumes its share. Idle eviction, fresh byte
  activity, lifetime expiry and slot release have observable outcomes under reduced test limits.
- **AC-4 (TUN-004)** On relay cancellation both the Unix peer and the TCP destination see
  closure even if neither initiates it. A handoff with no successor expires and releases all
  endpoints. Repeated shutdown or adoption cannot duplicate a reader or leak resources.

## Test plan

Use a real Unix relay and TCP peers in temporary, private fixtures. Establish failing coverage
on the preceding implementation before changing each behavior. Use condition-driven readiness
and bounded I/O. Exercise the production proxy wiring in separate processes with a synthetic
run and destination. Run the existing relay, proxy and host-bound credential suites, scoped
mutation checks and quick checks. Review each slice independently and deliver through Yulius CI.

## Decisions

Use a bounded handoff of connected descriptors and in-memory copy state during an orderly
restart. A private host-side custodian can hold those descriptors between server processes.
This leaves all dialing and host-bound secret handling in the authorized server.
The install helper stops and starts the unit separately, so a listener preserved only across
a single systemd restart job would miss the update path. A reconnect and replay cannot restore
an opaque TLS session. Interception remains scoped out of continuity because its TLS and
redaction state belongs to the old server. A crash before an orderly handoff, an older binary
without adoption support or a remote peer that times out can end a tunnel.

## Evidence

- Relay lifecycle (TUN-004, AC-4). Against dev `389789466`, the synthetic cancellation test
  timed out after cancelling the serving task with an established Unix and TCP peer. With
  the relay owning the copy tasks, both peers observed EOF and the full-relay health alert
  cleared. `cargo test --locked -p yulius-sandbox --lib relay::tests::` passed all 14 tests.
- This slice covers relay teardown. The tunnel continuity and credential acceptance tests
  belong to the subsequent slices.
