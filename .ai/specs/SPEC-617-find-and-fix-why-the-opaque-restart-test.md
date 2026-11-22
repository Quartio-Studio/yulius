---
yulius-task: 53390d34-4d7a-4418-9fd6-196d1dcb2d79
status: done
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
- [x] Verify both directions on the surviving tunnel under concurrent load
- [x] Approve the restart fix for delivery
- [x] Hold the fixture's handoff report with a test and name the custodian in process lists
- [x] Approve the review follow-up for delivery

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

## Evidence

- Unchanged dev code passed the reset test 200 of 200 times in sequence (nextest `--stress-count 200`) and 400 of 400 times with 48 copies in parallel. Nine full `yulius` suites passed every `opaque_restart` case. One run had an unrelated failure, `wiring::session::tests::a_loopback_listener_keeps_a_port_reserved`. Neither load nor a full suite alone reproduces the failure.
- With `/tmp` filled to 1 GiB during the old server's shutdown, the handoff still succeeded. A full tmpfs is not the cause.
- Removing `target/debug/yulius` during `stop(first)`, or replacing it with a file still open for writing, reproduced the failure every time: `surviving client reading DOWN: unexpected end of file`. The old server logged `cannot hand opaque tunnels to the next server kind=NotFound` or `kind=ExecutableFileBusy`. The connection set is every tunnel of the departing server. The phase is the deposit, when the custodian is spawned. Without the custodian, every socket closes, and the surviving client reads FIN before DOWN.
- `a_removed_install_path_still_hands_the_tunnel_to_the_next_server` and `an_install_path_being_rewritten_still_hands_the_tunnel_to_the_next_server` failed with `serve.rs` and `wiring/tunnel_handoff.rs` restored from the first commit's parent: `client reading DOWN after the restart: unexpected end of file; handoff warnings: … kind=NotFound` and `… kind=ExecutableFileBusy`. Both pass with the fix.
- With 160 busy loops on 128 CPUs (load average about 300), 400 test processes, 20 at a time, each ran all nine direct `opaque_restart` cases: 3,600 passes and no failure, with peak `/tmp` use of 356 MB. At 48 to 64 processes the same set exhausted the shared 1 GiB `/tmp` ("database or disk is full", server exit 101). Those failures came from the overloaded fixture environment, not from the handoff.
- `scripts/cargo clippy -p yulius -p yulius-sandbox --all-targets -- -D warnings` passed. All 85 `opaque_restart`, `tunnel_handoff`, custodian and relay tests passed. `scripts/cargo nextest run -p yulius -p yulius-sandbox` passed 3,582 tests (run `99ad5486-7c3a-45b5-8c2c-ffa2bef7a947`). `python3 scripts/check_repository.py` passed.
- PR #1694 merged into dev at `02c8ee41a` from PR head `0c01ee858`, whose own Yulius CI run passed. Review round 1 approved `e855b39f4`. It reported one minor in own code, one pre-existing minor and three nits.
- Review follow-up. `an_invalid_handoff_directory_reports_the_kind_and_still_serves` now starts two server generations. It asserts that `handoff_warnings` reports `cannot adopt opaque tunnels kind=PermissionDenied` twice, as plain text. The test failed when the colour-escape stripping was disabled, and again when the previous generation's log was dropped. `the_waiting_helper_names_itself_yulius_in_process_lists` failed before `arg0("yulius")` was added, because `argv[0]` was the executable path, and passes now. The fixture's spawn error names its executable. ADR-0456 now says the #1676 cause was reproduced, not observed. One nit was not changed: if a SIGKILL lands within a second of the hard link, a temporary link directory stays in `target/debug`. Any other exit removes it, and the build directory is disposable.
- After the follow-up: all 18 `opaque_restart` and `tunnel_handoff` tests passed. Clippy passed with `-D warnings`. `scripts/cargo nextest run -p yulius -p yulius-sandbox` passed 3,588 tests (run `521f3b54-5444-4304-8200-43dfa8cd08de`). The repository checks passed.
- Review round 2 approved `25b84ab97`. It found that the naming test counted sibling tests' helpers when one process ran all `tunnel_handoff::` tests (`cargo test`, 15 of 15 failures). The test now keeps only its own directory's helper and skips runtime threads that exit during the `/proc` scan. Run that way, it passed 40 of 40 times (`--test-threads 8`). Under nextest, all 18 `opaque_restart` and `tunnel_handoff` tests passed and clippy passed. ADR-0456 and the custodian comment now say that command lines name the helper while its kernel command name stays `exe`. The two-generation diagnostics test has the 30-second bound the other restart tests use.
