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
- [x] Obtain independent approval for the relay lifecycle slice
- [x] Preserve buffered bytes and half-closes when an established copy pauses
- [x] Obtain independent approval for the resumable copy slice
- [x] Hold connected socket pairs and bounded copy state for one successor
- [x] Obtain independent approval for the socket custodian slice
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
- The first slice covers relay teardown. The tunnel continuity and credential acceptance tests
  belong to subsequent slices.
- A pre-change probe ran two real `yulius serve` processes on the same ports with an isolated
  temporary database and a synthetic run. The build was `95a332d8a` from #552. Its sole
  difference from dev `389789466` was SPEC-576. The original direct CONNECT peers exchanged
  markers in both directions. After SIGTERM and process exit, the destination wrote a distinct
  marker. The replacement answered `/internal/healthz`, but the retained client read EOF.
- The independent relay review approved `f610bc301` and reran all 14 relay tests. Quick
  checks passed. The project's Vidoc switch disabled the security scan.
- Relay mutation check: 1 listed, 1 caught, 0 missed, 0 timed out, 0 unviable, 0 not reached.
  The unmutated baseline passed all 3,243 selected tests (2 skipped).
- PR #1534 landed at dev squash `169fd9028`. The reviewed PR's CI head was `77018410c`.
  Dev run `d3807c5c-0373-4619-a24b-e69c2aa40a29` passed by identical tree. Quality, both
  test shards and e2e passed. CI skipped the disabled security scan.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/169fd90282f4f233eb774f729a5c506c9c044053?run_id=d3807c5c-0373-4619-a24b-e69c2aa40a29)
- On the delivered code, cancelling and restarting Tokio's bidirectional copy under backpressure
  lost three bytes from each partially written chunk. With a half-closed requester the reply
  resumed as `done` instead of `ongdone`. Both assertions failed before the change. The new
  copy keeps the write cursor and completed write-half flag with each direction. Tests pause
  twice, confirm exact ordering, measure the 8 KiB bound on read-ahead and check that a broken
  destination terminates the copy while the other peer is open.
- `cargo test --locked -p yulius-sandbox --lib relay::` passed all 18 relay tests.
  `cargo test --locked -p yulius-egress` passed 42 unit and integration tests, including
  opaque forwarding, host-bound injection, response redaction and admission under load.
  Clippy passed for both crates and all their test targets with warnings denied.
- Independent review of `5ee4cb804` found two failures. A synthetic interrupted read ended
  both directions before the marker arrived. A pause during write-half shutdown caused a second
  read past EOF, which returned a connection reset. Both regression tests failed on the reviewed
  code. The copy now retries interrupted reads and records EOF before polling shutdown. The
  new tests verify the marker, fatal error propagation, shutdown completion on resume and the
  peer's complete reply. All 20 relay tests and 42 egress tests passed with the corrections.
- A fresh reviewer approved `8f5eccc53`, checked both fixes and independently passed the
  20 relay and 42 egress tests. Repository quick checks and targeted Clippy passed. Vidoc
  was disabled. The copy slice's first mutation run caught all 7 listed mutants, with
  0 missed, 0 timed out, 0 unviable and 0 not reached.
- The mutation run against the review fixes (`scripts/dev mutants --base 5ee4cb804`) listed
  6 mutants and caught all 6, with 0 missed, 0 timed out, 0 unviable and 0 not reached.
  Its unmutated baseline passed 3,256 tests (2 skipped).
- PR #1536 landed at dev squash `4b1cc7149`. PR run `3888a35a-e5d9-4cfe-9f6c-9b103513cbd1` passed at `f4bcc2dc7`. Dev run `e35dfcce-4bd1-40c5-9148-9a3cdd6c2fb0` passed on the identical tree. Both test shards, quality and e2e passed. Security was disabled.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/4b1cc7149715864179372061203b05bf0b3ea852?run_id=e35dfcce-4bd1-40c5-9148-9a3cdd6c2fb0)
- After #1536 the retained client still read EOF in the two-server restart probe. In an
  isolated helper process fixture, the old executable rejected the handoff command and closed
  the control connection. With the helper, a successor obtained the original descriptors and
  private state, read bytes queued during the gap and wrote fresh bytes to the original peer.
  The first helper implementation failed with address-in-use on a stale socket name. The
  regression test starts with that exact condition. An exclusive lock on the private directory
  now protects stale socket cleanup and prevents a second deposit from displacing a live one.
- The isolated PR-watch timeout on #1536 was absent on the base (1.26 s), its reviewed head
  (1.80 s), and four concurrent head runs sharing one CPU (2.57–2.66 s). Both revisions
  observed the draft, held the news, saw the cleared box, delivered the news, then observed idle.
- Review of `61ea0c680` found the helper released its directory lock before sending the
  batch. A real stalled consumer left a first helper blocked while a second deposit obtained
  an acknowledgement. The integration regression failed at the lock assertion. The helper
  now holds the lock through delivery and bounds its kernel send queue. The test observes
  both peer closures and the lock becoming available after dropping the stalled consumer.
- The reviewer also requested an actual depositor exit. The revised process fixture waits
  for its depositing subprocess to exit, queues bytes in both directions, and adopts from a
  surviving process. Coupling the helper's lifetime to the depositor experimentally produced
  a connection reset and failed the assertion. With the production lifetime, the original
  peers carry the queued and fresh bytes and the second adoption finds no socket.
- At `de8e028c5`, deadline expiry left a peer connected if a stalled consumer retained a
  descriptor it had already received. The synthetic test retained both descriptors after
  receiving the batch and triggered expiry. It timed out awaiting peer EOF. The helper now
  keeps the original handles until it confirms receipt of the complete batch. On incomplete
  delivery it shuts down both stream halves before dropping its originals. With the consumer
  and its duplicate handles still open, both fixture peers now observe EOF.
- Independent round three approved `79d5c112b`. The reviewer passed all 12 custodian unit tests and three Linux process tests. All 32 relay tests, targeted Clippy and quick checks passed locally. The instance disabled Vidoc.
- A separate Linux process fixture exercised the production deadline at the approved head.
  It connected a successor and left the descriptor messages unread in the kernel. The helper
  exited after 120.0 seconds, and both original peers read EOF while the successor socket
  was still open.
- The mutation run on `61ea0c680` passed its unmutated baseline of 3,268 tests, caught 51
  mutants and found five that did not compile. The run on the lock correction at `de8e028c5`
  caught its one mutant. Neither run missed a mutant, timed out or left one untested.
- For the acknowledgement correction, the default mutation filter omitted the existing Linux
  process tests and missed both inverted acknowledgement checks. Running `scripts/dev mutants
  --base de8e028c5 --tests 'test(/^tunnel_handoff::/)'` on the same head selected those tests.
  The original-peer test failed on each inverted check. All 3,271 baseline tests passed,
  seven mutants were caught and one did not compile. None were missed, timed out or untested.
