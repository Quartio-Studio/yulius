---
yulius-task: 53390d34-4d7a-4418-9fd6-196d1dcb2d79
status: in-progress
---

# SPEC-617 The opaque tunnel custodian runs the departing server's own image

## Goal

Find and fix why `opaque_restart::a_destination_reset_during_restart_keeps_the_other_established_tunnel` sometimes loses the surviving tunnel with `UnexpectedEof` (issue #1676). The restart tests should fail only for a real defect and say which read failed.

## Context

SPEC-577 hands established opaque CONNECT tunnels to the next server. On shutdown the old server starts a short-lived custodian (`yulius sandbox-host hold-tunnels`) and sends it every tunnel's sockets and copy state. The successor adopts them from the custodian. The old server started the custodian from `AppConfig::yulius_exe`, the path that `current_exe()` returned at startup.

Task #596 ran two full local suites that failed. Nextest run `5f380c48-a4db-41de-a75a-e16b1378fe82` failed the reset test and `established_direct_connect_carries_queued_and_fresh_bytes_across_server_exit` together ("original client reading downtime marker: unexpected end of file"). It also failed `an_unfinished_http_download_cannot_delay_opaque_handoff_or_shutdown`. Run `aa96d6c2-9bc3-4f5c-98ad-683a1500d513` failed the reset test alone. During both runs the same task kept building in the same `target/` directory, and the full run's own log showed `Compiling yulius-egress` lines. The compiler cache materializes outputs by copying, so `target/debug/yulius` is briefly missing or open for writing.

The sibling `final_bytes_and_fin_during_restart_reach_the_half_closed_peer` failure in dev run `7df09ccf-8386-45d5-997e-b2e308cc891b` (at `af6beca37`, HTTP 404 during CONNECT setup) had a different cause. SPEC-597 traced it to the old fixture dropping its internal port, which let another server answer the readiness probe. PR #1661 fixed that after `af6beca37`, by keeping a reserved port and checking the child's own `yulius listening` line.

## Requirements

- **OCI-001** A departing server starts its tunnel custodian from its own running image (`/proc/self/exe`), whatever is at its install path at that moment. The custodian always matches the depositor's handoff protocol.
- **OCI-002** If the executable path is missing or still being written, the established tunnels still reach the successor with their queued bytes, in both directions.
- **OCI-003** The restart fixture names the failing read and the restart phase. Its error includes the handoff warnings of both server generations.

## Plan

- [x] Identify the connection and restart phase that lose the surviving tunnel
- [x] Name the failing read and keep both servers' logs in the restart fixture
- [x] Correct the cause with a test that fails without it
- [ ] Verify both directions on the surviving tunnel under concurrent load
- [ ] Approve the restart fix for delivery
- [ ] Merge into dev on green CI, request staging and comment on the issue

## Acceptance criteria

- **AC-1** The failure is attributed to one connection set and one restart phase, with reproduction evidence.
- **AC-2** A server started from an executable path that disappears before shutdown keeps its tunnel across the restart. Without the fix the same test fails.
- **AC-3** In the two-tunnel reset fixture, the surviving tunnel carries DOWN and NEXT under concurrent test load, and the reset tunnel ends.
- **AC-4** A restart fixture failure names its read and shows the servers' handoff warnings.

## Test plan

Reproduce the failure deterministically by removing the executable path, or by holding it open for writing, during the old server's shutdown. Add an integration test that starts the server from a hard link and removes the link before shutdown, and confirm that it fails on the unfixed code. Run every `opaque_restart` and `tunnel_handoff` case, stress the reset test with many parallel copies, and run the full `yulius` suite.

## Decisions

- The product was at fault, not only the fixture (ADR-0456). Starting the custodian from a mutable path makes the handoff depend on the file at that path, and its version, during shutdown. `/proc/self/exe` names the inode the departing server runs, even after the path is unlinked or replaced. The update helper stops the service before it renames the new binary into place, so updates behave as before.
- The fixture keeps both log generations and puts their handoff warnings into its read errors. The old message, a bare `UnexpectedEof`, did not say which read returned it.
- No retry and no sleep: the regression test is deterministic, because it removes the path before the old server stops.
