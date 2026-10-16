---
yulius-task: 8dcb47b3-4dfd-49ae-b9bd-7b1410e844d5
status: done
---

# SPEC-197: The shipped automations are the Automations page's defaults

Status: done
Date: 2026-10-16
Owner: Yulius task #183
Scope authorization: task #183 (MrFishPL): "those 2 automations should be defaults, like default
workflows", with a screenshot of the Automations page: "Keep the docs up to date" and "Work through
open issues" in the list, each badged "Created by an agent", and below them "Ready-made automations"
offering "Deploy dev every night" with an Enable button.

## Goal

The automations Yulius ships read as defaults, as the default workflows do: the Automations page
opens with a **Defaults** section holding every shipped automation, each switched on or off in
place and badged "Default", and a **Custom** section with the project's other automations. A
shipped automation that an agent created by name (as on Yulius's own instance) is a default, not
"Created by an agent".

## Context

- **Shipped automations** (SPEC-042 AUT-007..011, [ADR-0034](../../docs/decisions/0034-ready-made-automations-catalog.md)):
  `catalog/automations/*.md` (`docs-up-to-date`, `open-issues`, `nightly-dev-deploy`), embedded at
  build time. `GET /api/sandboxes/{sid}/automation-templates` lists them with `automation_id`, the
  project's automation of the same name (case-insensitive) or `null`; `POST …/{id}/enable` creates
  that automation enabled (201) or re-enables the existing one (200). Nothing links the two in the
  store but the name.
- **The page today** (`crates/yulius-server/web/assets/pages/automations.js`): one list of the
  project's automations, then "Ready-made automations" with the templates whose `automation_id` is
  `null`. A shipped automation once added is indistinguishable from any other; one created by an
  agent with `automation_create` (OPS-010 of SPEC-042 did exactly that on this instance) carries
  "Created by an agent".
- **Default workflows** (`pages/workflows.js`, AUT-006): a "Defaults" section first ("kept current
  by Yulius"), each card badged with a lock and "Default", then "Custom" ("yours to change") with
  its own empty state. Default workflows are read-only and follow the catalog.
- **Automations differ from workflows in one way that matters:** an automation is per-project
  state (on/off, schedule, run history, start choice) and people edit its prompt. ADR-0034 chose to
  copy the shipped content once and never overwrite people's edits; that stays.

## Requirements

- **AUT-032 Defaults first.** The Automations page shows a section "Defaults" (hint: "ship with
  Yulius, on or off per project") listing every shipped automation in catalog order, then a
  section "Custom" (hint: "yours to change") with the project's other automations. "Ready-made
  automations" goes away. A project automation counts as a default when it has a shipped
  automation's name (the existing `automation_id` link).
- **AUT-033 A default's card.** Every default card carries a lock badge "Default" (tooltip: it
  ships with Yulius; changes made here are kept) and the shipped description, never "Created by an
  agent". An added default shows what any automation shows (schedule, next/last run, switch, Run
  now, history, delete, opens in the builder). A default not added yet shows its schedule and
  "Off", a switch that is off, and nothing else to press; it does not open.
- **AUT-034 The switch turns a default on.** Turning on a default that is not added calls the
  existing enable endpoint, which creates it enabled; the card becomes the added default with its
  switch on, and keyboard focus stays on that switch. A second press while it is being enabled
  keeps it on (one automation per switch); the switch is not disabled meanwhile, which would drop
  focus. Turning an added default off or on is the ordinary update. Without write access the
  switch is disabled. A failed enable leaves the switch off and says why.
- **AUT-035 Deleting a default resets it.** Deleting an added default keeps the one verb behind
  the trash icon (Delete, #147), and its dialog says it stays among the defaults, off, and that
  turning it on again starts from the current shipped version; the builder's Delete says the same
  for a default. Afterwards the card shows the not-added default.
- **AUT-036 Custom's empty state.** Without custom automations the Custom section shows "No custom
  automations yet" with New automation (write access). When the shipped list cannot be read, the
  page shows all automations under Custom as before, so the page still works.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Defaults and Custom sections with the Default badge on the Automations page
- [x] A default is switched on in place and deleting it resets it
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-AUT-31: in the mock studio project, "Defaults" lists the three shipped automations and
  "Custom" the two others; a shipped automation created by an agent shows "Default", not "Created
  by an agent"; "Ready-made automations" is gone; no WCAG violations.
- AC-AUT-32: turning on "Keep the docs up to date" (not added) from the keyboard creates it
  enabled: the toast says so and its switch stays on, focused, in the Defaults section, now with
  Run now, still "Default" and opening in the builder, while Custom is unchanged; turning it off
  pauses it. A failed enable leaves the switch off and says why.
- AC-AUT-33: deleting an added default says it stays among the defaults, off; afterwards it is still listed under
  Defaults with its switch off and no Run now.
- AC-AUT-34: a person with read access sees the switches disabled.
- AC-AUT-35: when the shipped list cannot be read, there is no Defaults section and all the
  project's automations show under Custom, the agent-made one badged "Created by an agent".

## Test plan

- e2e (`e2e/tests/automations.e2e.ts`, mock server): the existing ready-made test rewritten for
  AC-AUT-31..33, a failed enable and a read-only check (AC-AUT-32, AC-AUT-34), and the shipped
  list failing to load (AC-AUT-35); the builder's Delete of a default and of a custom automation
  (AUT-035); a double press while enabling (AUT-034); `keepFocus` by label (unit, in the page). The mock catalog gains "Deploy dev every night"
  so it matches the shipped three, and one shipped automation created by an agent.
- No server change: the API (templates with `automation_id`, enable, update, delete) is unchanged
  and covered by `crates/yulius/tests/automations.rs`.

## Decisions

- Focus after a reload: `keepFocus` (dom.js, used by every loader) returns focus to the control of
  the same unique `aria-label` before falling back to the same place, so an added default's
  switch keeps focus although its card gains an open button before it, also when another reload
  overtakes the enable's. Positional restore stays for unlabelled or repeated labels.

- [ADR-0159](../../docs/decisions/0159-shipped-automations-are-the-automations-page-defaults.md):
  shipped automations are shown as the page's Defaults, switched on in place, recognised by name;
  their content stays an editable copy (ADR-0034), so "Default" does not mean read-only as for
  workflows, and delete is their reset.

## Evidence

- Test first: the two new e2e tests failed on the unchanged page (no Defaults heading), then
  passed. Review fixes were also test-first: focus after switching on failed (`toBeFocused`) and
  passed once fixed; the builder's Delete wording and `keepFocus` by label failed, then passed; the
  double-press assertion fails with the guard removed (mutation check) and passes with it.
- `cd e2e && npx e2e run tests/automations.e2e.ts`: 13 passed (13).
- `scripts/dev ci` on 3e38551b (origin/dev 3127a28b merged; the code of the delivered head): script
  tests 85 OK, nextest 2015 tests run, 2015 passed, 18 skipped; exit 0. Full e2e suite run after it
  on the same head, with nothing beside it: 598 passed, 1 skipped (59 files), exit 0. Earlier
  heads: 1975 passed (753b52c1, 56244b9b), 1992 (bc650894), 1996 (32b8bfb1).
- Full e2e suite (`npx e2e run`): 583 passed, 1 skipped on 753b52c1. On the merged head while
  the Rust gate ran beside it, 2 timed out under load (`queued-messages` "the request shows its
  files as placeholders", tracked by #525; `responsive` automation editor heading). Both files
  passed alone (27/27, 21/21). Run again with nothing beside it, the full suite on the merged
  head passed: 592 passed, 1 skipped (59 files), exit 0. The load timeouts are outside this change:
  they are page-load timeouts at a CPU the gate saturated, and `queued-messages` does not touch
  automations.
- `scripts/dev security` on 3e38551b (the vidoc-1 scan of ADR-0157): not run, exit 2. Vidoc
  refused this agent's key (HTTP 401), so the scan is reported as unrun, not passed. The CI
  `security` job scans the pull request head.
- Visual check: a screenshot of the mock Automations page (dark theme, 1440 px) shows Defaults
  (three shipped automations, the agent-made one badged "Default", the two not added off) above
  Custom.
- Reviews: round 1 (753b52c1) requested changes (fallback untested, focus lost, weak assertions,
  stale MCP text, SPEC-042 pointer), all fixed in 56244b9b. Round 2 approved 56244b9b with two
  minors (builder Delete, double-press test) and a focus nit, fixed in b5e6ad1d. Round 3 approved
  the merged head 92a00309 with one minor: the builder's Delete now looked up the default before
  asking, so a double click opened two dialogs. A test reproduced it (two dialogs), and a re-entry
  guard fixed it (no dialog once the builder is left). The double-press test now holds the enable
  (the mock's `holdpost=`), so it no longer depends on timing under load, and `keepFocus` gained
  the case of a label gone after the rebuild. Round 4 (1ed2068d) requested changes. The second dev
  merge had committed this worktree's leftover `tools/vidoc/node_modules`, after dev dropped its
  ignore line; it was removed in a new commit (ADR-0157). The left-builder case was untested; its
  test fails without the `isConnected` check and passes with it. Final review on the head below.
- Not testable here: the live instance's Automations page (it runs `main`; this reaches it at the
  next promotion).
