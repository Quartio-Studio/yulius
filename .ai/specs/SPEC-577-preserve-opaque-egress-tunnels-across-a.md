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
- [x] Resume established direct opaque CONNECT sockets in the replacement server
- [x] Keep each original deadline during custody and preserve the live remainder of a mixed batch
- [x] Obtain independent approval for the custody deadline slice
- [x] Preserve original terminal receive queues and isolate connection loss during custody
- [x] Obtain independent approval for terminal socket custody
- [ ] Obtain independent approval for the proxy restart slice
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
A custodian launched by a delegated service uses the service-owned sessions unit's existing
legacy process leaf where available. A surviving child in a delegated server unit with active
controllers can prevent systemd 258 and later from starting it. Where the sessions unit cannot
take the child, deposit in that layout requires a systemd version that can restart it.
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
- PR #1538 landed at dev squash `9b66c9a3d`. Yulius CI run
  `5973c684-451f-4328-b4b5-69ffc731634e` passed both shards, quality and e2e at
  `69dd7df1b`. Dev run `e49687ed-4a8f-490b-b463-41370a61e7d3` passed the same jobs
  on the landed commit. Vidoc was disabled. The two-server probe still read EOF on the original
  direct client after the replacement became healthy.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/9b66c9a3d4dbf46c24bd23ec2d3083d45c64ba13?run_id=e49687ed-4a8f-490b-b463-41370a61e7d3)
- Before wiring the proxy, `cargo test --locked -p yulius --all-features --test main
  opaque_restart::` failed with unexpected EOF. The test starts two real server binaries in
  legacy mode. The destination listener closes after its sole accept. A prefix sent with
  CONNECT arrives at that peer, and the client receives its reply before the old server exits.
  The destination writes a new marker during the gap. The original client must read it after
  successor readiness, send another marker, half-close and read the peer's final reply.
- Discarding in-memory copy state on reconstruction made two bounded duplex fixtures lose
  pending bytes. One read `abcd` in place of `XYZa`. Another read `done` in place of
  `ongdone` after a half-close. The versioned opaque record carries both write-half phases,
  their unsent bytes, the sandbox and run, original idle activity and absolute expiry.
- The direct two-process test passed after wiring the proxy. An independent local fixture
  also passed on the real binaries. Bounded-socket tests exposed a stalled reply when the
  initial implementation wrote a large preread prefix serially. The revised copy forwards
  its excess in parallel with the peer's reply, keeps up to 8 KiB pending, then registers
  the established tunnel for handoff. The backpressure regression passed.
- The library and integration suites for `yulius-domain`, `yulius-sandbox` and
  `yulius-egress` passed 898 tests, with two sandbox environment tests ignored. This includes
  host-bound secret injection and redaction, idle eviction, per-sandbox and global admission,
  original lifetime expiry and cleanup. The Linux proxy tests also verify that interception,
  refused credentials and refused destinations contribute no records.
- With inherited placement, the cgroup fixture would acknowledge a helper in a delegated unit
  with active controllers and no sessions unit on systemd 258. It failed the restart-safety
  assertion. The helper now joins the detected sessions unit's service-owned legacy leaf. With
  no usable sessions unit, an active subtree requires a confirmed systemd earlier than 258.
  A service subtree with no enabled controllers and a standalone process keep their
  existing placement. This sandbox has no cgroup filesystem or systemd PID 1, so placement is
  tested with synthetic cgroup files. The two-process socket test runs real Linux binaries.
- Independent review of direct handoff at `cf48719473` reproduced a lost tunnel when an
  early CONNECT payload exceeded one copy buffer and the upstream write stalled. A real
  TCP read took 32,768 bytes on that head. Linux internal HTTP now reads at most 8 KiB per
  poll. A complete upgrade prefix fits the copy state, so the proxy registers the sockets
  before forwarding either way. Extra bytes stay on the original socket.
- A browser arrived after shutdown requested a full proxy's only slot and received HTTP 200.
  The regression now observes 503 and exchanges bytes on the original restored agent socket.
  Stopping admissions and claiming eligible records use the same table lock. The eligibility
  fixture completes intercepted TLS with synthetic host-bound injection and reflected-value
  redaction, and establishes real opaque streams for the agent, CI, superagent and browser.
  Broadening eligibility to all authenticated callers yielded three records. With the agent
  check in place it yields exactly the agent's two original TCP endpoints.
- A tunnel's original deadline elapsed while a helper waited for a successor. The peers
  stayed open beyond the test's timeout. Two further regressions timed out on peer closure
  during capture and on listener cleanup after the last pair expired. The handoff now carries
  a per-pair monotonic deadline. The collector, sender, helper and receiver all enforce it.
  An expired pair closes both peers, surviving records retain admission order, and the last
  expiry releases the waiting helper's listener and directory lock.
- At `c0d3b617f` the direct two-server case passed with a 20 KiB CONNECT header and 96 KiB
  of synthetic early payload. Its peer read only the first marker before the old server exited,
  then checked the entire payload and half-close after replacement. All 903 domain, egress and
  sandbox unit/integration tests passed (two environment tests ignored), as did targeted
  Clippy. The first mutation run exposed missing checks for cold-start diagnostics and reuse of
  an existing private handoff directory. The process fixtures now exercise both conditions and
  a bad handoff directory at startup.
- Round two at `b3f7af710` independently passed 12 focused tests and confirmed the four
  first-round corrections. The reviewer kept an adopting process open after it received the
  batch and withheld its acknowledgement. Both original peers observed EOF at expiry, but a
  second deposit still found the helper's only place occupied. The corresponding regression
  timed out after three seconds on that implementation. The last original deadline now covers
  the entire sealed batch through final confirmation. With the successor and its duplicates
  still open, the helper exits and the directory lock becomes available. All 16 custodian
  unit tests passed with that correction.
- Round three at `e07e8d4ff` confirmed the helper releases its place after last expiry,
  including during a stalled acknowledgement. It found that a short-lived, half-closed TCP
  pair could expire after both pairs were queued as descriptor messages. Linux then returned
  `NotConnected` for the expired pair. The successor rejected the entire batch and both peers
  of the longer-lived tunnel observed EOF. A real TCP regression reproduced errno 107 on
  the reviewed implementation. The protocol now validates descriptor type and family on
  every frame, reads its absolute deadline before requiring live peers, and drains expired
  records with the same framing and capacity checks.
- The mixed-batch fix and original-deadline custody are being delivered separately. The
  direct proxy and service wiring tested above are saved for the following review and PR.
  The serve path on dev still ends established streams at restart.
- On the separated custody implementation, 37 relay tests and all four Linux helper-process
  tests passed. They include the real mixed TCP queue regression, malformed expired frames,
  an unconnected live stream, independent expiry before adoption and lock release when a
  successor keeps its received descriptors open without acknowledging them.
- Review of the separated custody protocol at `309f9aaac` found that a listening TCP
  descriptor in an expired frame could pass the stream-type check and lose its ability to
  accept connections when the helper shut down the pair. The wire regression observed that
  the retained listener had stopped listening. The public deposit regression also failed
  to reject it. The depositor now validates both descriptors before enrolling any for
  shutdown, and the receiver checks the listening flag before accepting either descriptor.
  Both fixtures exercise the retained listener by accepting a new local synthetic peer.
- In a partial timed frame, bypassing the per-pair deadline around the state read made a
  three-second fixture time out. The restored check releases both peers while the producer
  withholds the state suffix, then consumes that suffix and the following live frame. The
  listener-cleanup fixture now also bounds its entire acknowledgement and EOF sequence.
- The next reviewer independently passed 39 relay and five process tests on `4ef3da2bb`
  and found two gaps in one fixture. Its live-invalid case now uses a TCP stream that has
  never connected and expects `NotConnected`. The oversized sender half-closes after the
  size field, so a missing size guard reaches EOF promptly. Removing each matching guard
  made its assertion fail in under a second.
- An isolated container fixture started two real servers with `--contained --isolation
  namespaces` and a synthetic live holder running the production port forward. The agent
  reached the real manager's `out.sock`, established CONNECT, and exchanged markers before
  the first server exited. After the replacement passed its health check with the same holder,
  the original client read EOF in place of the downtime marker. The fixture used a private
  network, synthetic database and subordinate-ID files inside the disposable container.
  A second run half-closed the client and confirmed that the destination observed its FIN
  before stopping the first server. The destination sent the downtime marker on its open
  half, and the original client again received EOF after the replacement became healthy.
  With a separate synthetic client, Linux reported the child stopped by SIGSTOP before
  shutdown and again at successor readiness. After SIGCONT the child received EOF on its
  established connection and the original destination received no post-thaw marker.
  Nested namespace creation and a real systemd unit are unavailable in this task sandbox.
- The same holder forward carried HTTPS to two names on a local synthetic TLS origin. The
  fixture created a sandbox-scoped bearer through the server API and enrolled the public
  proxy CA. Before and after restart, the bound origin received the value and the client
  received `[secret:RESTART_FIXTURE]` in the reflected header and body. The unbound origin
  received only the placeholder. The old intercepted connection ended. The replacement refused
  an invalid token with HTTP 407 and accepted a fresh synthetic live attempt. Recovery had
  marked the first synthetic attempt lost because its fixture contained no agent process.
- Returning the quotient of the absolute deadline and current time could keep the helper
  checking a far-off deadline too frequently while eventual peer EOF still passed. A
  deterministic custody test expected the next wake after 450 ms and received 1 ms on
  that mutation. It checks the last millisecond, exact expiry and the next surviving pair
  with a supplied clock. Receipt and the helper's final deadline use the same saturating
  calculation of time remaining.
- On Linux, even a failed shutdown on a never-connected TCP socket can disable later writes
  through a retained duplicate. An expired-frame fixture subsequently connected that original
  socket to a local listener and got `BrokenPipe` on its first write. Cleanup now shuts down
  endpoints which still have a peer and drops disconnected handles. The connected half of the
  expired frame sees EOF and the original owner can still connect and send its synthetic marker.
- Independent review approved `a4a184d25`. The reviewer passed 41 relay library tests and
  all five separate-process helper tests, and checked the diff against dev `5addbfbb8`.
  Targeted Clippy and repository quick checks passed locally. The instance disabled Vidoc.
- Mutation testing on the initial custody change listed 37 mutations. The tests caught 29,
  six did not compile, and two survived. The survivors changed the deadline comparison at
  equality and divided the deadline by the current time. Both operations were replaced by
  the shared time-remaining calculation. The wakeup fixture covers a positive remainder,
  the final millisecond and exact expiry. All four mutations in the descriptor-role fix
  were caught. The test-only correction added no product lines for mutation testing.
- The final mutation run on the clock and connected-endpoint cleanup fixes passed all
  3,292 selected baseline tests. Its summary was 11 listed, 9 caught, 0 missed,
  0 timed out, 2 unviable and 0 not reached.
- PR #1540 landed at dev squash `2571d8155`. PR run `f0d72521-e19a-4b4d-a275-e709064c3dec`
  passed at `fb8bc445c`, and dev run `f0fb5457-e381-4b67-a8cb-0091fbbbd02c` passed on
  the identical tree. Both shards, quality and e2e passed. Vidoc was disabled.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/2571d8155c21b530a887a918e718e238ffca6931?run_id=f0fb5457-e381-4b67-a8cb-0091fbbbd02c)
- On the custody-only code the two direct-server tests failed. The original client got EOF
  instead of the downtime marker, and an invalid handoff directory produced no diagnostic.
  With the direct proxy integrated both passed in 0.41 s. The domain, egress and sandbox
  suites passed 918 tests, with two host-environment cases ignored. The same two-server
  Rust fixture using the production holder forward and manager relay still failed with EOF.
- Four earlier mutations removed the HTTP transport's scalar or vectored write capability
  or bypassed flush and write-half shutdown. A synthetic scalar writer exposed the incorrect
  capability and missing flush before correction. The scalar and real TCP fixtures caught
  all four mutations on the corrected adapter.
- Independent review of the direct proxy at `5aea045e1` reproduced three defects. With two
  original CONNECT tunnels, a TCP reset during downtime made the healthy client read EOF.
  A second fixture reset a destination before deposit and got `NotConnected` for the whole
  batch. A disconnected frame with missing or oversized state also bypassed its framing error.
  Deposit and receipt now retain connected siblings and validate every frame within the
  original descriptor and state limits.
- An established direct tunnel shared a server with a plain HTTP download. Its synthetic
  origin declared 128 bytes, sent 64 and held its socket open. After the client received
  body data, the old server still had not exited ten seconds after SIGTERM. Capture now runs
  concurrently with bounded HTTP shutdown. The real-server regression keeps the unfinished
  download open through successor readiness and requires the original tunnel to transfer in
  each direction.
- Restoring a full one-place proxy reported a sandbox-share refusal. Ordinary admission on
  the same table reported a total refusal. A domain fixture covers a share equal to the
  whole table and a separate, smaller sandbox share. Restoration now uses the same limit
  precedence as admission.
- Two pre-existing proxy warning-capture tests could interfere when plain libtest ran them
  in parallel. This reproduced on the older test binary. Each passed in isolation, as run
  by nextest in CI. The independent test issue is tracked in
  [#1544](https://github.com/Quartio-Studio/yulius/issues/1544).
- On corrected head `959ab7448`, both isolated real-process tests passed. The unfinished
  download stayed open while the old process exited and the original CONNECT carried new
  bytes through the successor. Resetting a second destination during downtime preserved the
  healthy original sockets. The domain, egress and sandbox suites passed 920 tests with two
  environment-dependent tests ignored. Targeted Clippy and quick checks passed. Vidoc was disabled.
- The second independent reader identified an orderly close that resembled a disconnected
  socket. The client half-closed first and the destination observed EOF. In the restart gap
  the destination sent `last reply` followed by its FIN. The original client received zero
  of the ten bytes after the successor became healthy. The real-server regression reproduced
  this on `959ab7448`. With both FINs already exchanged before deposit, a separate helper
  fixture also returned `NotConnected` with the final reply still in the original TCP queue.
  The regression covers both transfer directions and a terminal receive queue whose remaining
  application bytes are already in the saved copy cursor.
- With the final-FIN correction, all five direct real-server tests passed. The two original
  peers delivered the last reply and last request, then EOF, across the replacement. A
  separate proxy test previously failed with errno 107 after both FINs and an empty kernel
  receive queue. It now flushes exactly five pending application bytes to its original
  destination. A mismatched socket identity is rejected and its original Unix peer can
  still transfer bytes. Targeted Clippy and the repository quick checks passed.
- The third direct-slice review at `fbfa58eea` requested two custody corrections. The
  legitimate sender's first identity message already carried both descriptors before the
  receiver could verify the cookies. A partial metadata read could therefore grant cleanup
  authority too soon. A wire regression failed because the metadata carried descriptors.
  With the new framing, the deadline and both cookies precede the descriptor transfer.
  Receipt interrupted before the second cookie, including at the original deadline, leaves
  the sender able to exchange its synthetic marker.
- On an isolated TCP connection the reviewer observed `NotConnected` from the peer-name
  query and `TimedOut` from a terminal queue check. A deterministic fixture injected that
  read outcome and the preceding implementation aborted the batch with errno 110. The
  corrected terminal queue check discards that pair. The fixture transfers the surviving
  original sockets and exchanges bytes in both directions. Role, cookie and framing errors
  still invalidate the delivery.
- The terminal socket custody changes are a separate delivery. The saved direct proxy
  implementation passed 120 targeted nextest cases on `fbfa58eea`. Its service wiring and
  the production holder integration remain open for their own reviews and CI.
- On the separated custody change, 47 relay library tests and all seven helper-process
  tests passed. They cover the original final receive queue before deposit, single adoption,
  independent expiry, reset isolation, identity mismatches, interrupted metadata and final
  confirmation. Repository contract and formatting checks passed.
- Independent review of the separated scope at `7d815ba59` passed the 47 relay and seven
  helper-process tests. It found that the earlier, in-process collection of capture replies
  could enroll a pair before verifying its identity. The live capture regression expected
  rejection for a mismatched cookie and received a pair instead. Additional fixtures found
  that collection accepted oversized state and an identified record without a deadline.
  Collection now performs the same checks as deposit before its cleanup state takes a
  socket. The original peer can still send its marker after the refused record's deadline.
  The updated relay library suite passed all 49 tests.
- The mutation run on the first direct review's corrections passed 3,312 selected baseline
  tests. Its summary was 17 listed, 15 caught, 0 missed, 0 timed out, 2 unviable and 0 not reached.
- The initial direct proxy mutation run listed 126, caught 72, found 54 unviable and
  missed none. The final-FIN run listed 23, caught 17, found five unviable and missed one.
  The separated custody run listed 33, caught 24, found eight unviable and missed the same
  mutation. All three campaigns reached every mutation without a timeout. The surviving edit
  replaced bitwise union with exclusive-or for two disjoint Linux receive flags. Both produce
  0x42 from MSG_PEEK (0x02) and MSG_DONTWAIT (0x40). The terminal read now uses the flag
  type's named union operation. The final-FIN process fixture checks that the peek leaves
  the queued bytes for the successor.
- After merging current dev `1d97b363c`, independent review approved the custody slice
  at `407be69d9` and reran all 49 relay tests. All seven helper-process tests passed
  on the same head. The review corrected ADR-0413's description of wire receipt.
  The receiver verifies original socket roles and identities before taking custody. The
  original deadline covers the subsequent state-length read and bounded allocation.
- The capture-validation mutation run passed all 3,302 selected baseline tests. It listed
  seven mutations, caught five and found two unviable. The terminal-read change passed
  all 3,314 selected baseline tests and caught both mutations. Both runs reported zero
  missed, timed out or unreached. Targeted Clippy passed with warnings denied. The instance
  disabled Vidoc.
- On the current custody-only server, the original client and destination exchanged bytes
  through the holder before shutdown. The successor became healthy with that same holder.
  The original client then received EOF in place of the marker written during restart.
  Proxy and holder wiring remain necessary for AC-1.
- PR #1569 landed at dev squash `e8759b018`, with the same tree as the reviewed head
  `aae723883`. PR run `66c61f08-cd34-4865-b380-9e1fd1d75c0f` and dev run
  `51949d71-9ca7-4f3f-9df2-f26be8ce2242` passed. Quality, both shards and e2e
  passed. Vidoc was disabled.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/e8759b018f629627a1583daf5eb445db61de2ce3?run_id=51949d71-9ca7-4f3f-9df2-f26be8ce2242)
- Before restoring the direct wiring on this base, the direct two-process acceptance
  fixture failed in 0.53 s. The original client read EOF where it expected the marker
  written between server processes. The five real-process direct cases passed in 8.37 s
  in the rehearsal. All 17 source, test and Cargo files in this integration match those
  exercised in the rehearsal.
