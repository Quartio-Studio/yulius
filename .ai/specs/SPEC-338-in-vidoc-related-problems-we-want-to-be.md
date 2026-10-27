---
yulius-task: efbe86b7-7ba9-41de-a251-2a3ca03480eb
status: in-progress
---

# SPEC-338: Vidoc scans can be turned off per project, at onboarding and in the admin panel

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #322
Scope authorization: task #322 (`/goal`, with a screenshot of a "Needs your decision" card: CI's
security job and the local scan stopped with exit 2 because the Vidoc key used its 50,000,000
tokens, and the agent asked whether to raise or renew the quota; the person typed "skip vidoc for
now"). The request: "we want to be able to just disable vidoc for the sandbox until quota is there,
and vidoc should be optional in general and it should be managed on onboarding and in admin's
panel". Extends SPEC-195 (instance key), SPEC-217 (built-in security job) and ADR-0107 (the
committed switch).

## Goal

A person can turn Vidoc security scans off for one project (sandbox) in Yulius, for example while
the key's quota is used up, and on again later, without removing the instance key, editing the
repository or asking an agent. While it is off nothing is sent to Vidoc for that project, nothing
asks anyone about Vidoc's key or quota, and every scan says "disabled", never "passed". The choice
is offered when a project is added (onboarding) and kept per project on the Instance page's Vidoc
card (the admin panel).

## Context

- Today Vidoc is on for a project when its runs can reach a `VIDOC_API_KEY`: the instance key
  (Settings, Instance; SPEC-195 VDK-005) or the project's own agent secret. The only ways to stop it
  are removing the instance key (every project, and it must be pasted again later), deleting the
  project's secret, or a reviewed commit of `enabled = false` in `.config/vidoc.toml` (ADR-0107),
  which needs a pull request in that repository and is not visible in Yulius.
- With the key out of quota (HTTP 429 "used its tokens"; memory: 2026-10-22 to 2026-10-27) every
  task's scan ends "not run" (exit 2), CI's `security` job ends skipped, and agents raise
  `needs_you` decisions about the quota, as in the screenshot.
- Where Vidoc being on matters (`vidoc_key().is_some()` or the applied `VIDOC_API_KEY`):
  - `App::ci_scans` adds the built-in `security` job to the trusted gate and keeps it on pull
    requests (`crates/yulius-app/src/ci/mod.rs`, SPEC-217 VDP-001);
  - the briefing's `## Security scan` section (`crates/yulius-domain/src/briefing.rs`, `vidoc`);
  - the onboarding prompt's `security_setup` (`crates/yulius-app/src/onboarding.rs`);
  - the scanner itself, `scripts/vidoc_scan.py` (embedded as `yulius security-scan`), run by
    agents and by a repository's own `security` job (this repository's `yulius.toml`).
- A sandbox's delivery profile is stored as contracts `Profile` JSON (`profile_json`) and read
  directly by `profile_of` and the sandbox view; its `gate` field already lives only there.
- The Instance page (superadmin) has the Vidoc key card (`instancekeys.js`); the "Add sandbox"
  dialog (`actions.js` `openSandboxDialog`) imports or creates a project.

## Requirements

- **VOF-001 Per-project switch.** Each sandbox has a Vidoc switch, stored as `vidoc` in its
  profile JSON: absent or `true` is on (today's behaviour), `false` is off. It is shown as
  `profile.vidoc` (always a boolean in responses) in the sandbox view. `PATCH /api/sandboxes/{sid}`
  takes `{"vidoc": bool}` (sandbox `admin`, like the delivery profile); a profile update without it
  keeps the stored value. A playground has no CI scan and refuses the switch. Each change is audited
  as `delivery.vidoc.update` with `on` or `off`.
- **VOF-002 Off means nothing is scanned.** For a sandbox whose switch is off:
  - agent runs, terminals (and the Cockpit) and CI jobs get `YULIUS_VIDOC=off` in their
    environment;
  - the scanner (`yulius security-scan`, `scripts/vidoc_scan.py`) with `YULIUS_VIDOC=off` prints
    `Security scan disabled: Vidoc is turned off for this project in Yulius; nothing was scanned.`
    and exits 0 before it reads a key or contacts Vidoc; with `--ci` it ends the job skipped
    (`$YULIUS_CI_SKIP_CODE`, ADR-0133), never passed;
  - the instance key is not added as the virtual `VIDOC_API_KEY` for its agent runs and CI jobs;
  - Yulius CI adds no built-in `security` job to its gate or pull requests.
- **VOF-003 Agents are told.** The briefing's `## Security scan` section, with an instance key or
  not, says for a switched-off sandbox that Vidoc is off for this project in Yulius, that the scan
  reports "disabled" (never a pass) and that nobody is to be asked about Vidoc's key or quota. The
  onboarding prompt of such a sandbox adds no scan rule and says why.
- **VOF-004 Onboarding.** The "Add sandbox" dialog, when the instance has a Vidoc key
  (`me.instance.vidoc`), offers "Scan pull requests with Vidoc" (checked); unchecked, the create
  request carries `"vidoc": false` (`POST /api/sandboxes`, import or create) and the server switches
  the new sandbox off before its onboarding starts, for whoever may add it. A playground does not
  offer it.
- **VOF-005 Admin panel.** The Instance page's Vidoc card lists every project (not playgrounds)
  with its switch, so a superadmin turns one off or on there; the card says that a switched-off
  project scans nothing until it is turned on again.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] Per-project Vidoc switch stored in the profile and changed through the sandbox API
- [ ] The scanner stops as disabled under YULIUS_VIDOC=off, set for switched-off sandboxes
- [ ] CI, the virtual key, the briefing and the onboarding prompt follow the switch
- [ ] The Add sandbox dialog and the Instance page's Vidoc card manage the switch
- [ ] ADR, architecture, AGENTS.md and product docs describe the switch
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- `PATCH /api/sandboxes/{sid}` `{"vidoc": false}` by an admin stores it, the view shows
  `profile.vidoc: false`, a later profile update keeps it, a writer gets 403, a playground 422.
- `YULIUS_VIDOC=off python3 scripts/vidoc_scan.py` exits 0 and prints the disabled line without a
  key; with `--ci` and `YULIUS_CI_REF=pr/1` it exits `$YULIUS_CI_SKIP_CODE`.
- A switched-off sandbox with an instance key: `ci_scans` is false (no built-in job), agent and CI
  environments carry `YULIUS_VIDOC=off`, applied agent secrets have no instance `VIDOC_API_KEY`,
  the briefing says Vidoc is off and the onboarding prompt adds no scan rule.
- The UI: the Add sandbox dialog's unchecked box switches the new sandbox off; the Instance card's
  switch changes a project; both are covered by the e2e suite with the mock.

## Test plan

- Python unit tests of the scanner (`scripts/test_vidoc_scan.py`): disabled by env, locally and in
  CI mode, without a key and without Git.
- Rust integration tests (`crates/yulius/tests/vidoc_key.rs` and the sandbox API tests): the PATCH
  contract and permissions, the profile update keeping the switch, `ci_scans`, the applied secrets,
  the agent and CI environment, the briefing and the onboarding prompt.
- Domain unit tests of the briefing's three security texts.
- e2e (`e2e/`): the Instance card's project switches and the Add sandbox checkbox, on the mock.

## Decisions

- **Stored in the profile JSON, not a new column.** The profile is already the per-sandbox
  settings blob, read directly by `profile_of` and the view (like `gate`); a column would need a
  migration and every `SandboxRecord` literal (28) changed. The profile editor's request does not
  carry it, so the update keeps the stored value unless `vidoc` is given (VOF-001).
- **An environment variable tells the scanner, set by Yulius.** The scanner runs in the
  repository (agents) and in CI jobs, which Yulius launches with its own environment; a project's
  own `VIDOC_API_KEY` and its own `security` job (this repository's) must stop too, which removing
  only the instance key would not do. ADR-0107 rejected an environment variable as an agent's own
  switch; here only Yulius sets it, from a setting people change in the UI, and an agent that sets
  it itself only skips its own local run, which CI (whose environment agents cannot change) still
  does or does not run as the setting says.
- **Default on.** Absent means on, so existing projects behave as before and an instance key keeps
  meaning "scan"; turning it off is the explicit, audited choice.
- **Sandbox admins and superadmins.** The same permission as the delivery profile (which can
  already turn required checks off); the Instance card is the superadmin's overview.
- **Not an instance-wide pause.** Removing the instance key already stops every project that
  relies on it; the per-project switch also covers projects with their own key, which was the
  screenshot's case.
