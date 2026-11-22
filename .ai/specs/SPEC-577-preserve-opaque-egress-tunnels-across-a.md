---
yulius-task: 35adb14d-65fe-4a9f-b9fa-370d776d4139
status: done
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
- [x] Obtain independent approval for the proxy restart slice
- [x] Hand established opaque connections and matching relay sockets to the restarted server
- [x] Verify same-socket continuity, admission limits and intercepted traffic through restart
- [x] Quiesce outbound relay admissions without recording a capacity refusal
- [x] Obtain independent approval for the holder relay lifecycle slice
- [x] Restore matched proxy and relay pairs on the delivered holder lifecycle
- [x] Bind the original relay at CONNECT and retain final bytes until both copies drain
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Yulius CI passed with Vidoc disabled, and the docs and changes log are updated

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
An opaque HTTP/1.1 CONNECT gives its dedicated TCP connection to the copy. The bytes inside,
including TLS carrying HTTP/2 to the destination, stay opaque. An outer HTTP/2 CONNECT to
the proxy runs on a multiplexed session owned by the HTTP server and reconnects on restart.
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
- At `b0706ff51` the five direct server restart cases passed in 7.67 s. Independent
  review also passed those five and all five focused egress handoff tests. It found the
  product documentation overstated the eligible protocol. Its synthetic agent established
  a cleartext HTTP/2 CONNECT directly to the proxy, received HTTP 200, and exchanged
  `TEST` and `BACK`. After SIGTERM and successor readiness the destination read EOF.
  The source documentation now identifies the dedicated HTTP/1.1 CONNECT to the proxy.
- A separate real-server fixture negotiated TLS with HTTP/2 to a synthetic destination
  inside an opaque HTTP/1.1 CONNECT. The destination accepted one TLS session. Its original
  HTTP/2 stream delivered a response written while the direct server was down, received
  more request bytes after successor readiness, and returned its final reply.
- Fresh review approved the direct delivery at `a58afc499`. The reviewer passed all five
  real-server restart tests and five egress handoff tests. It verified both the original
  inner TLS/HTTP/2 session and the outer HTTP/2 reconnect. The successor refused the old
  synthetic attempt with HTTP 407 and admitted a fresh live attempt with HTTP 200. Locally
  the domain, egress and sandbox suites passed 929 tests, with two host-dependent skips.
  Targeted Clippy, repository contracts and formatting passed. Vidoc was disabled.
- The mutation run against the landed custody base `e8759b018` passed all 3,333
  selected baseline tests. Of 129 mutations, 77 were caught and 52 were unviable.
  None were missed, timed out or unreached. The follow-up on the protocol documentation
  changed no Rust lines.
- PR #1570 landed at dev squash `49009e859` with the reviewed `f490cb8bd` tree.
  PR run `899f3cd7-213f-4bbf-9c1c-af4528e50278` and dev run
  `02535bf9-dbcc-449b-961b-3ef0e68a2452` passed quality, both test shards and e2e.
  The security scan was disabled.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/49009e85931be97c15fdd40873f9ec95b26ce54b?run_id=02535bf9-dbcc-449b-961b-3ef0e68a2452)
### Holder integration rehearsal and separated deliveries

The following results cover the assembled holder and proxy candidate. The holder relay
registers and captures original connections in the first delivery. The successor's
matched relay and proxy restoration has its own review and CI.

- On the direct-only implementation, the real holder's retained client read EOF after
  the successor became ready. The frozen-client regression failed again with BrokenPipe
  in 0.38 seconds before integrating the holder change. The destination listener accepts
  once and closes. A separate regression lost the healthy original tunnel when another
  destination reset during the gap.
- The namespace-mode server captures the opaque proxy copy together with its original
  outbound relay. At adoption it checks the matching internal TCP endpoints, common
  deadline and live holder record. It restores proxy and relay accounting before new
  admissions. Two terminal-queue regressions first failed with NotConnected after both
  FINs. The old internal link recorded while connected identifies the verified terminal
  socket, which can still hold final application bytes.
- In an isolated rehearsal the nine Linux `opaque_restart::` tests passed, including all
  four opt-in namespace cases. Two independent server processes used the production
  holder forward and manager's out.sock. One test kept the client stopped in the kernel
  through successor readiness, then delivered the complete ordered 1 MiB reply on its
  original socket after a partial write in the restart gap. Another lost a destination
  while its healthy sibling delivered queued and fresh bytes on both original peers.
  The holder fixtures supply a live process and its actual namespace identities, a
  synthetic record and subordinate ID maps. Nested holder creation and the systemd
  cgroup freezer require a host with those facilities. The test uses SIGSTOP/SIGCONT.
- A separate synthetic TLS origin negotiated HTTP/2 inside an opaque HTTP/1.1 CONNECT
  through the holder, accepted one TLS session and kept its original request and response
  stream through the two-server restart. Over the intercepted route, an echoed synthetic
  credential was redacted in both response headers and body. Only the bound origin received
  its injected value before and after restart. The unbound origin received the placeholder.
  The old intercepted stream closed, an invalid token received 407, and a fresh live
  synthetic attempt reconnected through both host policies.
- On the task branch based on dev `49009e859`, the nine real Linux restart tests passed
  in 9.51 seconds, including all four holder cases. The separate eight-case holder
  matrix and the inner TLS/HTTP/2 transfer passed. The domain, sandbox and egress
  nextest suites passed 934 tests with two host-dependent cases skipped. The two
  server-wiring tests passed, including a mixed batch that arrived out of order.
  Each retained copy had both original internal endpoints and a common deadline.
  Dropping unmatched records closed their original external peers.
- Independent holder review at `8637b3b97` passed the nine restart fixtures and
  reproduced a connection loss after a surviving holder's network helper changed PID.
  The manager persists that PID during upkeep without replacing the Unix forward.
  The new native regression uses two fixture-owned helper processes, updates the saved
  PID before opening CONNECT, and then restarts the server. The original client received
  EOF on the reviewed server in 0.52 seconds. Restoration now tolerates a changed helper
  PID while checking the holder's generation, boot and live namespaces. All nine
  restart tests and 934 domain, sandbox and egress tests passed after the correction.
- The reviewer also found that the idle-eviction test assumed five minutes of host uptime.
  An isolated monotonic clock starting one second after boot reproduced the failure at
  the idle-place assertion. The corrected test drives the restored table to one millisecond
  before the five-minute floor, then to its boundary. It checks that the active peer stays
  admitted. All four holder relay tests passed with the young clock.
- A distinct-binary upgrade in the holder fixture used the same HTTP/1.1 CONNECT and TLS
  session through both server processes. The HTTP/2 origin accepted once, delivered
  the gap bytes, accepted more request bytes and sent the final reply. Both binaries
  supported the namespace handoff format.
- Second independent review at `0d597e184` passed the nine restart cases, four relay
  tests and six proxy tests. It identified two copies that could hold separate admission
  places after explicit cancellation. Two new reduced-limit fixtures reproduced both
  directions. Dropping the relay manager left an idle proxy place, and evicting a
  one-place proxy left an idle relay place. Both failures occurred with the external
  peers' write halves still open. The replacement now links the cancellation of these
  two tasks. An ordinary FIN or orderly capture still lets the other copy drain.
- The focused mutation run on the network-helper identity correction listed one mutation.
  The change could not compile because the restored record has no default. The baseline
  passed and the run reported zero missed, timed out or unreached.
- With the two restored host-side copies sharing administrative cancellation, both
  reduced-limit regressions passed. The nine real restart cases still passed, including
  terminal replies and capture through another update. One original connection also
  survived four consecutive restarts, exchanging queued and fresh bytes with each of five
  distinct server processes. It delivered both final FINs. The synthetic intercepted-host
  matrix and the distinct-binary TLS/HTTP/2 upgrade passed.
- Additional fixtures cover duplicate restore records, stale namespaces, changed internal
  endpoints and a late holder registration during shutdown. A strict namespace startup
  check exercises the holder forward on an eligible host and requires the documented
  missing-prerequisite diagnostic on an ineligible host. The disposable namespace suite
  ran all ten cases, including that positive startup check, in 12.10 seconds. The task
  host passed the prerequisite diagnostic. Six focused relay tests and four server-wiring
  tests passed.
- The third review of the combined holder change approved the earlier corrections and
  independently passed all ten native restart cases, six relay, four wiring and six
  egress handoff tests. It found one remaining defect. Two empty stopping relays
  emitted the 256-place capacity warning and incremented the process-wide drop count
  from zero to two. A subprocess regression reproduced both the warning and the count.
  Admission now distinguishes a stopped table from a full table under the same lock.
  The regression passed with a zero delta and quiet stderr. The live relay capture
  and the successor's matched restore are separate deliveries.
- The original integrated candidate listed 109 mutations. The first run caught 62,
  missed 10 and found 37 unviable. All were reached with zero timeouts. On the corrected
  candidate, the strict startup test caught the missing-manager substitution. The
  isolated relay checks caught the other nine substitutions in endpoint verification,
  holder validation, registration cleanup, stopping and adoption. Across 63 relay
  mutations this focused run caught 48 and found 13 unviable. Two changes to paired
  cancellation survived the sandbox-only tests. The service-level cancellation run
  caught both of them and all 12 mutations on the cancellation change, with zero
  missed, timed out or unreached.


### Holder admission and capture slice

- With a synthetic HTTP response held open and the old public listener closed after
  SIGTERM, dev `49009e859` accepted a fresh connection on the real holder's `out.sock`.
  Its peer timed out waiting for EOF. The stopping server now closes the late Unix
  connection immediately. The same isolated real-server fixture observed quiet stderr.
- Three holder relay fixtures captured the original Unix and TCP sockets once, preserved
  the final internal receive queue after both FINs, and exercised replacement and late
  registration against an empty stopping table. The process-wide capacity count stayed
  unchanged. The successor's joined relay and proxy restoration has a separate delivery.
- Independent review of `25118872c` found that two admitted relays could have the
  same internal addresses after a terminal TCP tuple became reusable. A synthetic two-place
  table reproduced capture choosing the first entry. Capture now requires a unique match
  and leaves both controls intact on ambiguity. The reviewer also requested proof that
  application bytes already read from TCP survive a blocked Unix write. Filling the Unix
  send queue and observing the TCP receive queue empty made that state deterministic.
  Substituting an empty copy state in the reviewed code failed the saved-suffix assertion.
  All five relay cases passed with the real captured state, including the complete ordered
  reply and FIN on the original client.
- A fresh reviewer approved `f7897c77c` and passed all five holder capture fixtures,
  the host prerequisite case and 56 relay tests serially. The committed SIGTERM test
  failed on dev in 2.91 seconds because the old server admitted a late relay. The six
  isolated Linux cases passed on the candidate in 8.02 seconds. The broader domain,
  sandbox and egress suites passed 934 tests with two environment-dependent skips.
  Scoped Clippy and quick checks passed. Vidoc was disabled.
- One concurrent review run hit `AddrInUse` in the older forward capacity fixture. The
  fixture releases an ephemeral listener before binding the same address again on dev
  `d95fb3f5f`. [Issue #1572](https://github.com/Quartio-Studio/yulius/issues/1572)
  tracks the separate fixture correction.
- `scripts/dev mutants` against dev for the holder admission slice reached all
  69 mutations in 18 minutes. It caught 30 and found 39 unviable. The follow-up
  against the reviewed head for the unique-link correction reached all six mutations,
  caught two and found four unviable. Both runs reported zero missed or timed out.

- PR #1573 merged at dev `4e6095745` with the same tree as reviewed head `7d53333a2`.
  PR run `7d035774-e362-41d3-8964-ce678f9f5234` and dev run
  `647eab7e-5495-44b6-b092-db32111b6771` passed quality, both test shards and e2e.
  The security scan was disabled.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/4e6095745150d9233b5f1a4958a013bf7c8fc0f6?run_id=647eab7e-5495-44b6-b092-db32111b6771)

### Matched successor restoration

- The stopped-client regression failed with BrokenPipe on the shipped capture-only tree.
  Its destination listener accepts once and closes, so the test requires both original
  peers to exchange bytes through the replacement server.
- The earlier pairing implementation retained both the healthy tunnel and an ambiguous
  one in a mixed batch containing duplicate records. The new selection rejects duplicate
  internal endpoints before requesting a relay or starting either restored copy. The
  updated fixture kept only the healthy tunnel. Both rejected external peers observed EOF.

- On the exact dev server at `4e6095745` the stopped client lost its original tunnel
  with BrokenPipe in 0.43 seconds. The first joined candidate `71879bf4d` passed the
  ten real-server fixtures, five service wiring cases and 938 library and integration
  tests. Two host-dependent cases were skipped.
- Independent review of that candidate found that an internal TCP address pair can
  outlive its original connection and that the proxy can finish before an outbound
  relay drains. The synthetic identity regression retained two records where only
  the healthy original qualified. A separate Linux fixture waited for the old proxy
  to finish its internal write half before SIGTERM. The original Unix client received
  196,608 of 1,048,576 ordered bytes after the successor became ready.
- The departing proxy now enrolls the actual outbound socket during an authorized
  opaque CONNECT, while both internal TCP endpoints still name one another. The
  namespace record carries its kernel socket cookie. Capture and adoption check this
  identity, and each copy remains available while its enrolled peer finishes a write.
  The same process fixture received the full ordered 1,048,576 bytes and EOF on the
  original Unix socket. The identity regression retained only the healthy tunnel. A
  live relay test also confirmed that cookie-based capture refuses an unenrolled
  connection. The synthetic intercepted host policies and four consecutive updates
  on one original connection passed with this change.
- The checked-in final-reply regression reproduced the 196,608-byte short read on
  `71879bf4d` in 0.50 seconds. All 11 `opaque_restart` cases passed on the corrected
  binary in 10.05 seconds in an isolated Linux network. Two distinct compatible builds
  exchanged fresh request and response bytes on the original TLS session and HTTP/2 stream.
  Six service wiring cases, ten relay handoff cases and 940 domain, sandbox and egress
  tests passed. Two environment-dependent sandbox tests were skipped. Scoped Clippy passed.
- A second independent review of `077006a7c` found that startup discarded a distinct
  enrolled socket when its addresses had been reused. The synthetic two-place fixture
  failed with InvalidData. Startup now checks the original relay cookie and admission
  order. The fixture reserves both distinct sockets, rejects the replay, and verifies
  both restored cookies.
- The same review found a cancellation race before the first restart. When eviction
  closed the checkpoint sender, the proxy could finish its transfer with a success
  result and leave the idle relay waiting for Unix input. The bounded fixture failed
  in 2 seconds on that path. An abandoned checkpoint reply reproduced the same failure
  on each copy after the sender-close fix. Both transfers now propagate cancellation
  when either side of the checkpoint is gone. The fixtures observe EOF with the external
  write halves open and verify that the relay place can be admitted again.
- A fresh reviewer approved `d9e7f9e3d` against `ec580fc3a` and independently
  passed 11 relay handoff, three proxy handoff and six service wiring tests. The author
  passed 942 domain, sandbox and egress tests, with two environment-dependent skips,
  and all 11 real-server cases. The isolated fixtures also passed the four successive
  replacements, the distinct-binary TLS/HTTP2 upgrade and intercepted-host checks.
  Scoped Clippy and quick checks passed. The Vidoc scan was disabled in Yulius.
- `scripts/dev mutants --base 4e6095745` reached all 71 mutations on the first
  successor candidate in 17 minutes. It caught 55 and found 16 unviable, with zero
  misses or timeouts. On the corrected head the relay fixture also caught all six
  mutations of the endpoint and uniqueness predicates in `OutboundRelays::enroll`.
- `scripts/dev mutants --base 71879bf4d` reached all 72 review-fix mutations in
  19 minutes. It caught 45, found 24 unviable and missed three, with zero timeouts.
  The missed changes removed the live manager binding, returned a constant socket
  cookie and allowed duplicate enrollment candidates. The real-server fixture below
  caught the binding change. The final two-place fixture caught both constant-cookie
  mutations, and the six enrollment predicate checks caught the duplicate-candidate change.
- The opt-in real-server holder fixture passed with the normal manager binding.
  `cargo mutants` then removed only `bind_holder_relays`. The original client received
  EOF while reading the downtime marker in 0.54 seconds. The fixture caught that mutation.
- The final review corrections (`scripts/dev mutants --base 077006a7c`) reached all
  eight mutations in nine minutes. Seven were caught and one was unviable. None were
  missed or timed out.
- PR #1575 run `dc901963-b755-425f-b61d-fab252fce2f6` on `746436fdf` reported
  the two restored-connection cleanup fixtures failing during argument parsing.
  The runner sets `YULIUS_CI=1`; the server's CI argument accepts `on` or `off`.
  Setting the same environment locally reproduced both failures. With the fixture
  passing `--ci off` explicitly, all six service-wiring cases passed under nextest
  with `YULIUS_CI=1`, including the original socket-closure and admission assertions.
- PR #1575 run `36ba6f1e-42f4-47d5-9fe5-e533dbc8472a` on `33e3eaab2` passed
  quality and both Rust shards. The Chromium job reported `route.fulfill: Route is
  already handled!` during mock startup. The startup fixture was identical on the PR
  and dev. Two held loopback responses reproduced the error in 0.404 seconds on the
  branch. The final regression also checked a new request during teardown and failed
  in 0.793 seconds on dev `d1ff0e4ee`. Removing interception let Chromium
  continue the second request while its handler still owned the response. Teardown
  now drains active handlers before unregistering routes, within the fixture's
  30-second bound. All eight scheduled regression runs preserved both original
  response markers and passed the new request through to the static server. All
  14 startup and mock-readiness cases passed with retries disabled.
  [Browser fixture report #1551](https://github.com/Quartio-Studio/yulius/issues/1551)
- Independent review found that the fresh-response assertion also accepted a 404.
  Substituting a synthetic 404 confirmed the gap. The strengthened test rejected
  that status and a synthetic empty 200 response. All eight repetitions then passed
  with HTTP 200 and the exact repository `api.js` returned by the static server.

### Delivery

- [PR #1575](https://github.com/Quartio-Studio/yulius/pull/1575) merged into `dev`
  at `737746a8f`. The independently approved head `de9be383b` passed quality, both Rust
  test shards and Chromium. Chromium ran with retries disabled and reported 1,297 passed
  and one optional agent test skipped. The Vidoc scan was disabled.
  [PR verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/737746a8f44c3cab81962e84fee790fb567208a6?run_id=f0de7c8a-8cf6-47c5-b80d-fcc20b8a2daa)
- The `dev` run reused the passing results for the identical Git tree. The real-server
  fixtures used synthetic peers, a surviving holder and SIGSTOP/SIGCONT. Production cgroup
  freezing, nested holder creation and a live model provider were outside those checks.
  [Dev verification](https://yulius.quartio.ai/api/tasks/35adb14d-65fe-4a9f-b9fa-370d776d4139/delivery/evidence/737746a8f44c3cab81962e84fee790fb567208a6?run_id=a03d23d5-8dac-4b25-91d7-65ba9fb0560a)
