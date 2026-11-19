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

- [ ] Capture the update watcher failure in an isolated systemd VM
- [x] Pin the service start-limit setting in a shipped-unit regression
- [x] Keep the watcher active and document recovery for existing installations
- [ ] Verify recovery and burst handling against the shipped units
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
