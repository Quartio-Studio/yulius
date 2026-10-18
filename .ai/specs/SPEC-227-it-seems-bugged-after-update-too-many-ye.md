---
yulius-task: fdb5eeed-6307-4664-a643-c9df761aa8dc
status: in-progress
---

# SPEC-227: A host check that keeps Docker says so, once

Status: in-progress
Date: 2026-10-18
Owner: task #213
Scope authorization: task #213: "it seems bugged after update, too many yellow warnings" (a
screenshot of Settings → Instance after the update to 77925fd). Fixes #674. Amends SPEC-203
HST-012 and HST-015 (how the re-check is shown).

## Goal

After an update, Settings → Instance says what the host check found in proportion: "sandboxes may
have lost Docker" only when the check's own "sandbox Docker" verdict is not ok, the list of items
once (in the Sandbox Docker card), and each item's reason without repeating its file.

## Context

Symptom (the screenshot): three yellow banners. Two of them are the same three lines, under
Updates ("Sandboxes may have lost Docker with this update.") and under Sandbox Docker ("The last
host check (yulius 77925fd, 41 min ago) found that sandboxes may have lost Docker."), while that
card's verdict badge is green ("sandboxes get Docker"). The third, under Leftovers, is a separate
defect (below).

Root cause:

- The server's `yulius-update.service` was installed before #620 (SPEC-203), so it lacks
  `ReadWritePaths=/etc/modules-load.d -/etc/needrestart/conf.d`. HST-009's drop-in that adds it is
  written only by `sudo yulius host setup`, which has not run on this server since. Under
  `ProtectSystem=strict` the re-check (`yulius host check`, HST-012) cannot write the needrestart
  rule and the modules boot list, and reports the missing drop-in. The three lines are true and
  `sudo yulius host setup` clears them; the helper cannot write its own drop-in (`/etc/systemd`
  is read-only to it, by design).
- `sandboxdocker.js` `hostCheck` and `instance.js` `dockerRecheck` both take any failed item
  (`report.ok == false`) as "may have lost Docker", ignoring the report's own `sandbox Docker`
  line (HST-013's verdict), which here is ok: the modules are loaded, only the boot list and the
  needrestart rule are missing.
- The update outcome renders the same list as the card: the run's report and the card's latest
  root check are the same `host-check.json` until a setup runs.
- `RealHost::write` formats its error as `<path>: <error>` and each item prefixes
  `cannot write <path>: ` again.

Why tests missed it: the mock's failed reports (`docker=rebuild`) carry no `sandbox Docker` line
and only a Docker-losing failure, so "items failed, Docker kept" was never rendered; the e2e tests
assert the alarming headline only. `RealHost` has no unit test of its error text.

Out of scope: Leftovers' "Failed: …/ci: Permission denied (os error 13)" is the automatic cleanup
failing on a sandbox's CI folder; a different defect, tracked separately.

## Requirements

- **HCW-001** The Sandbox Docker card's host-check banner: when the report has a `sandbox Docker`
  line whose outcome is ok and no `error`, its headline says the check left N items to fix, that
  sandboxes get Docker, and to run `sudo yulius host setup` on the server; the failed items
  follow. Otherwise (verdict failed, missing, or a report error) the headline stays "… found that
  sandboxes may have lost Docker."
- **HCW-002** The update outcome lists no items: ok → "Sandboxes still get Docker."; Docker kept
  with failed items → one line "Sandboxes still get Docker; the host check after it left N items
  to fix (Sandbox Docker, below)."; otherwise a warning "Sandboxes may have lost Docker with this
  update." that points to the Sandbox Docker card's latest host check.
- **HCW-003** `RealHost::write` reports a failure to write the file without its path (the caller
  names it); a failure to create its folder still names the folder.

## Plan

- [ ] Spec and issue for the post-update host-check warnings
- [ ] Host write errors name the file once
- [ ] Host-check banner headline follows the sandbox Docker verdict
- [ ] Update outcome points to Sandbox Docker instead of repeating the list
- [ ] Docs, changes entry and gates green
- [ ] Independent review answered and delivered

## Acceptance criteria

- AC-1 (HCW-001) With a report of failed items and an ok `sandbox Docker` line, the card shows
  "sandboxes get Docker", the items and `sudo yulius host setup`, and no "may have lost Docker".
- AC-2 (HCW-001) With a failed verdict, the card still says "may have lost Docker".
- AC-3 (HCW-002) After an update whose check kept Docker, the outcome shows one line and none of
  the items; after one that lost it, "Sandboxes may have lost Docker with this update." and no
  items.
- AC-4 (HCW-003) `RealHost::write` into a folder where no file can be created fails without the
  file's path in the error.

## Test plan

- e2e `sandbox-docker.e2e.ts`: a new mock mode `docker=kept` (the screenshot's report: three failed
  items and an ok `sandbox Docker` line) for AC-1 and AC-3's kept case; the existing
  `docker=rebuild` tests for AC-2 and AC-3's lost case, now asserting the items are absent from the
  outcome.
- Unit test in `host_setup/system.rs` (Linux): writing `/proc/<name>` fails (no file can be created
  in procfs, even as root) and the error does not contain the path (AC-4).
- `scripts/dev ci` and the e2e suite.

## Decisions

- The headline reads the report's own `sandbox Docker` line, not the card's live verdict: the
  report describes the host at check time; mixing the two would misdate the claim.
- The update outcome points to the card instead of repeating it. The card shows the latest root
  check, so a later `host setup` that fixed the host is not contradicted by a stale list; the
  outcome keeps its one-line result, which is the run's own history.
- The helper does not escape its own sandbox (e.g. `systemd-run`) to write the drop-in itself:
  that would bypass the operator's `ProtectSystem=strict`; `sudo yulius host setup` once is the
  documented fix (HST-009).
- No ADR: this refines how SPEC-203's report is shown, no new lasting design choice.
