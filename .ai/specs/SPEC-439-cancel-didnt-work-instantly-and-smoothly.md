---
yulius-task: f5a0491f-3b72-4fd8-b276-6623874735bf
status: in-progress
---

# SPEC-439: Cancelling an update stops its build at once

Status: in progress
Date: 2026-11-08
Owner: Yulius task #421
Scope authorization: task #421 ("cancel didnt work instantly and smoothly"), with a screenshot of
**Settings → Instance → Updates** during the Build step and the "Cancel the update?" confirmation.
Fixes #1117. Amends SPEC-002 ADR-18 (the update helper) and SPEC-001 OPS-008 (cancelling an update).

## Goal

**Cancel update** acts at once, on the page and on the host: the page shows the cancelled outcome
as soon as the server answers, and the release build stops within about a second, so the host is
free again and a restart or a new update does not wait behind it.

## Context

### Symptom

A superadmin cancels an update while it builds. The confirmation closes, but the card keeps saying
"Updating to …" with a spinner and an enabled **Cancel update** button until a further read of the
update view answers; on the host the build keeps compiling for minutes.

### Root cause

1. **The helper never hears of the cancel.** `cancel_update` (`crates/yulius-app/src/updates.rs`)
   marks the run cancelled and removes `<data>/update/request.json` (`clear_request`). The root
   helper (`crates/yulius/src/update_helper.rs`) read that file once, before the build; `compile`
   then waits on `cargo build --release` with `wait_bounded`, which only kills cargo after its
   2-hour bound (and only cargo, not the `rustc` processes it started). So a cancelled build runs
   to its end at nice 10 on every core, and a restart or a new update asked for meanwhile waits
   for it (the server's `await_ack` sees the helper busy and keeps waiting).
2. **The page drops the cancel's answer.** `cancel()` in `pages/instance.js` ignores the
   `UpdateView` the POST returns and asks for the view again (`ctx.updates.refresh()`), so the card
   changes only after a second round trip on a server busy with the build. The button gives no
   feedback meanwhile; a second click gets a 409 toast.

The existing tests missed it: the helper tests never withdraw a request mid-build, the server
tests (`crates/yulius/tests/self_update.rs`) use a fake helper with no process, and no e2e test
cancels an update.

## Requirements

- **CUB-001 A withdrawn build stops.** While the helper builds, it checks every 500 ms whether the
  request is still there: once `request.json` is gone or holds another request id (a cancel or a
  failure removed it, a new update replaced it), it kills cargo's whole process group (cargo,
  `rustc`, build scripts), stages nothing, reports the build `failed` with "The build was
  cancelled." and goes on with the next request, if any. A file it cannot read is not a
  withdrawal. Installs and restarts are never stopped this way.
- **CUB-002 The build's process group is killed at its bound too.** The 2-hour bound kills the same
  group, not cargo alone.
- **CUB-003 The page shows the cancel at once.** Confirming **Cancel update** (or **Cancel
  restart**) marks the button busy and disabled until the server answers, then shows the view the
  cancel returned (the cancelled outcome), without waiting for another read. A failed cancel
  re-enables the button and says why.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Regression tests for a withdrawn build and the page's cancel
- [ ] The helper stops a withdrawn build and its compilers
- [ ] The page shows the cancel from its answer, busy meanwhile
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- Removing the request while the fake cargo runs ends the helper's run within seconds; cargo's
  child process is gone; nothing is staged; the status is `failed`, "The build was cancelled.".
- A new request replacing the one being built stops the old build and builds the new one in the
  same helper run.
- With update reads held in the mock, confirming **Cancel update** shows "The last update was
  cancelled" and the Cancel button is gone.
- `docs/operations.md` (Updates) says a cancel stops the build.

## Test plan

- `crates/yulius/src/update_helper/tests.rs`: `a_withdrawn_build_stops_with_its_compilers` and
  `a_replaced_build_stops_and_the_new_request_is_built` with a fake cargo that starts a child and
  waits, run in a thread; the test waits on conditions (the child's pid file, the helper thread),
  never on fixed sleeps.
- `e2e/tests/update.e2e.ts`: `cancelling an update shows its outcome from the cancel's answer`,
  with a mock build held at "compiling" (`&build=hold`) and update reads held.
- The full gate (`scripts/dev ci`), Yulius CI on the pull request.

## Decisions

- [ADR-0283](../../docs/decisions/0283-a-cancelled-update-s-build-stops-when-its-request-is-withdrawn.md):
  the request file is the cancel signal: the helper polls it during the build and kills the
  build's process group. Rejected: a separate cancel file or a second systemd unit (a new
  protocol for what the request file already says), `systemctl stop yulius-update.service` from
  the server (the unprivileged server cannot, and it would also stop an install).
- The page applies the POST's view through `ctx.updates.apply(view)`, which goes through the same
  ordering as reads (#502), so an older read answering later cannot bring "Updating" back.
