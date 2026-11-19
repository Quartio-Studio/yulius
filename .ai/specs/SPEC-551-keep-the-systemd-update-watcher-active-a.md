---
yulius-task: 826f97b8-3cc5-432b-abc1-3c6921baa5a6
status: in-progress
---

# SPEC-551 Keep the systemd update watcher active after rapid requests

## Goal

A person can ask for another update after several quick refusals and still get an answer from the helper.

## Context

The shipped path unit starts a oneshot when the service user writes `update/request.json`.
With the default service start limit, systemd refuses the sixth start in ten seconds and fails
the path with `unit-start-limit-hit`. Later writes then go unwatched. Existing helper tests
exercise request handling without running the shipped systemd units.
[Issue #1201](https://github.com/Quartio-Studio/yulius/issues/1201)

## Requirements

- **UPW-001** The shipped update service sets `StartLimitIntervalSec=0` in its `[Unit]`
  section. The path unit retains systemd's trigger limit for busy loops.
- **UPW-002** The operations guide gives operators of hand-installed units the setting,
  how to reload it, and how to restore a watcher that has already failed.

## Plan

- [x] Capture the update watcher failure in an isolated systemd VM
- [x] Pin the service start-limit setting in a shipped-unit regression
- [x] Keep the watcher active and document recovery for existing installations
- [x] Verify recovery and burst handling against the shipped units
- [ ] Independent approval for delivery into dev

## Acceptance criteria

- **AC-1 (UPW-001)** More than five distinct requests within ten seconds each reach the helper
  and leave `yulius-update.path` active and waiting in an isolated systemd VM.
- **AC-2 (UPW-002)** An operator can follow the guide from a failed watcher to a waiting watcher
  that processes a fresh request.

## Test plan

Assert the directive in the shipped service's Unit section. On a disposable Ubuntu QEMU guest,
write distinct malformed requests as the data directory owner and wait for each refusal and
the path to rearm. Compare the original units with the corrected service on the same guest.
Inspect the path and service results, time the burst and exercise the guide's recovery commands.

## Decisions

Apply the move helper's existing service setting to the update helper. The adopt service is
a dependency of server startup and has no request-file watcher. The operator can edit the
installed update unit in place to preserve site-specific settings.
[SPEC-470](SPEC-470-run-migration-the-button-in-settings-ins.md)

## Evidence

- `cargo test --locked -p yulius --lib update_helper::tests::shipped_update_service_disables_the_start_limit_in_unit_section -- --exact`
  failed on the original unit with `None` where `Some("0")` was expected. After the fix,
  `cargo test --locked -p yulius --lib update_helper::tests::` passed all 28 tests, including
  after merging current dev. `scripts/dev quick` passed. `scripts/dev security` reported
  that Vidoc was disabled for the project.
- Ubuntu 26.04.1 in QEMU with systemd 259.5, the shipped units and the real update helper:
  the data directory owner wrote six malformed requests in 8.097 s on the original units.
  Only five got a refusal. The service reported `start-limit-hit` and the path reported
  `failed/failed/unit-start-limit-hit`.
- From that failed state, installed the corrected service and ran the guide's reload,
  reset-failed and start commands. `systemctl status yulius-update.path` showed `active (waiting)`.
  Each of 24 malformed requests with a distinct ID received its own refusal under load. The
  shortest six-write window in that run was 12.295 s.
- For the fast-refusal case, used malformed requests without an ID, a temporary in-memory
  helper state directory, and waited on systemd's path-state signals between writes.
  Six writes took 5.428 s. `journalctl -u yulius-update.service -o short-monotonic` showed
  six separate starts from 1056.728318 to 1062.242171 s and six refusals. After the sixth
  the path showed `ActiveState=active`, `SubState=waiting`, `Result=success`, and its trigger
  limit was 200 in 2 s. The real helper then answered a fresh request with ID `later-req-1`.
