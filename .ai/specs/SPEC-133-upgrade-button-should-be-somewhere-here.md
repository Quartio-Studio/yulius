---
yulius-task: a02a056c-9223-46de-9f91-36dab6bff0bf
status: done
---

# SPEC-133: the superadmin's Update button sits in the sidebar foot

Status: done
Date: 2026-10-14
Owner: Yulius task #121
Scope authorization: the person's request (task #121): "Upgrade button should be somewhere here
right in the superadmin, not hidden deeply in settings", with a screenshot of the home screen whose
sidebar foot shows the account button "Superadmin".

## Goal

The superadmin updates Yulius from where they always are: a button beside their account button
in the sidebar foot. When main has moved on it reads **Update**, and one click opens the same
confirmation as the instance page's "Update now…"; confirming starts the update. In every other
state the button stays there, quiet, and opens the instance page (version, Check now, progress).

## Context

- Updates exist (SPEC-001 OPS-006..OPS-009, SPEC-002 ADR-18, task #150): `GET /api/update` gives
  the view (`current`, `available`, `run`, `phase`, `helper_installed`, `check`), `POST
  /api/admin/update` starts one. `updates.js` (`createUpdateBanner`, `ctx.updates`) keeps the view,
  shows a pill in the tab row and adds a notification. The pill is dismissible per commit and only
  appears when an update is on offer, so once dismissed the only way in is Settings → Instance
  (`pages/settings.js` tile) — "hidden deeply in settings".
- `pages/instance.js` holds the confirmation dialog (`start(available)`) and the full page.
- The sidebar foot (`layout.js`, `createSidebar`) holds only the account button. The sidebar is
  200-480 px wide and a drawer on phones. The account menu (`openUserMenu`) lists an
  "Administration" section for instance administrators, without the instance page.
- The app calls this action "update" everywhere (pill, page, dialog, notifications); the request
  says "upgrade". One name per thing (#147).
- Parallel task #120 removes the Superagent button from the top bar; it does not touch the
  sidebar foot.

## Requirements

- UPB-001: For the superadmin only, the sidebar foot shows an update button to the right of the
  account button. Members and instance administrators never see it (the update API is the
  superadmin's).
- UPB-002: With an update on offer and none running, the button shows a download icon and the word
  "Update", in the accent tone; its accessible name is "Update <brand> to <short>" and its tooltip
  "Update available: <summary>".
- UPB-003: Clicking it opens the confirmation the instance page uses ("Update <brand> to
  <short>?", the three steps, confirm "Update to <short>"). Confirming starts the update exactly as
  "Update now…" does (one shared function), with the same toast and error handling; cancelling
  changes nothing. When the update helper is missing (`helper_installed === false`), the click
  opens the instance page instead, which explains it.
- UPB-004: While an update runs, the button shows a spinner and "Updating" and opens the instance
  page.
- UPB-005: Otherwise (up to date, unknown, never checked, failed check) the button is icon-only,
  quiet, named "Version and updates", and opens the instance page.
- UPB-006: Dismissing the tab-row pill does not hide the button.
- UPB-007: The account menu's Administration section lists "Version and updates" for the
  superadmin, leading to the instance page.
- UPB-008: The button never pushes the account name off the sidebar: the name ellipsizes, the
  button keeps its size, from 200 px up and in the phone drawer; axe finds no WCAG A/AA violation.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The superadmin's sidebar foot offers Update and starts an offered update after one confirmation, proven end to end
- [x] The button stays reachable when up to date or updating, members never see it, and the account menu lists Version and updates
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- With `?mock=1&update=1`, the home screen's sidebar foot has a button "Update Yulius to 9c1e7d3";
  clicking it opens the dialog "Update Yulius to 9c1e7d3?" without leaving the home screen, and
  confirming runs the update to "Updated to 9c1e7d3" (UPB-001..UPB-003).
- After dismissing the pill the button is still there (UPB-006).
- With `&update=1&helper=0` the click opens the instance page (UPB-003).
- Without an offer the button is "Version and updates" and opens the instance page; while an
  update runs it reads "Updating" (UPB-004, UPB-005).
- As a member (`&role=member`) no such button exists and the account menu has no "Version and
  updates" (UPB-001, UPB-007).
- At a 200 px sidebar the account name and the button both fit inside the foot (UPB-008).

## Test plan

End-to-end tests in `e2e/tests/update.e2e.ts` (mock mode), written before the code and seen
failing: the offered button and its dialog, the run to the end, the dismissed pill, the missing
helper, the quiet and running states, the member case, the account menu entry and the 200 px fit
with axe. The existing update tests keep passing (the pill, the instance page's "Update now…"). No
server change, so no Rust test.

## Decisions

- **The sidebar foot, beside the account button** ([ADR-0108](../../docs/decisions/0108-the-superadmin-updates-from-the-sidebar-foot.md)):
  the screenshot points there, it is on every screen, and it belongs to the person the action
  belongs to. The tab-row pill stays the announcement; the button is the always-there control.
- **"Update", not "Upgrade"**: every other place (pill, page, dialog, notifications, API) says
  update; a second word for the same action would read as a second feature.
- **One click to the confirmation**, not to the page: the request is a button that upgrades. The
  dialog already explains what happens; the commit list stays one click away on the instance page.
- **Always present for the superadmin**, quiet when there is nothing to do, so the version and
  Check now are never more than one click away either.

## Evidence

Run on the merged head 283eba38 (branch + `origin/dev` b18c548d), 2026-10-14:

- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/update.e2e.ts` before the code: the two
  new button tests failed with "locator matched no nodes" (no button in `.sidebar-foot`); before
  the menu entry the quiet-state test failed on the missing "Version and updates" menu item;
  before the review fix the helper-missing title assertion failed. Each passed after its change.
- `scripts/dev ci`: exit 0. Repository contracts passed (UI design tokens included: the first run
  caught a 10 px padding, fixed to the 4 px scale); nextest 1666 tests run, 1666 passed,
  16 skipped; cargo deny, cargo shear and cargo doc passed.
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run`: exit 0, 49 files passed and 1 skipped, 404
  tests passed and 1 skipped (`agent.e2e.ts` needs a model key). `update.e2e.ts` holds 12 tests:
  the offered button and its one-click confirmation (cancel and confirm), the dismissed pill, the
  running state, the missing helper, the quiet state, the account-menu entry, the 200 px sidebar,
  the phone drawer, instance administrators and members; axe finds no WCAG A/AA violation.
- Screenshots in mock mode (offer, quiet, 200 px) checked by eye: the accent "Update" pill and the
  quiet icon sit right of the account name, which ellipsizes at 200 px.
- `scripts/dev security`: exit 2, **unrun, not passed**. Vidoc answers 401 "Invalid or expired
  token" although `api.vidoc.dev` is bound to `VIDOC_API_KEY`; noted on #470 (the stored key needs
  replacing, which only a person can do). Decision after the third review round: deliver with the
  scan reported as unrun. ADR-0104 makes an unrunnable gate (an expired key named among its causes)
  something the agent reports as unrun, not a hold on delivery, and Yulius CI does not run it
  (#466). The change is client-side UI (JS, CSS), e2e tests and docs, with no new endpoint, input
  handling or dependency; the existing `POST /api/admin/update` is called exactly as before.
- Independent review (`om-code-review`, fresh sessions): round 1 approved 81198dc4 with six minor
  findings and nits, all fixed; round 2 approved 283eba38 with one optional nit (the order of checks in the
  administrators test), left as is; round 3 raised only the unrun scan, decided above.
- Not verifiable here: a real update through the root helper on a server (the mock walks the
  phases); the server API is unchanged.
