---
yulius-task: 8dcb47b3-4dfd-49ae-b9bd-7b1410e844d5
status: in-progress
---

# SPEC-197: The shipped automations are the Automations page's defaults

Status: in-progress
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
  switch on. Turning an added default off or on is the ordinary update. Without write access the
  switch is disabled. A failed enable leaves the switch off and says why.
- **AUT-035 Deleting a default resets it.** Deleting an added default keeps the one verb behind
  the trash icon (Delete, #147), and its dialog says it stays among the defaults, off, and that
  turning it on again starts from the current shipped version. Afterwards the card shows the
  not-added default.
- **AUT-036 Custom's empty state.** Without custom automations the Custom section shows "No custom
  automations yet" with New automation (write access). When the shipped list cannot be read, the
  page shows all automations under Custom as before, so the page still works.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] Defaults and Custom sections with the Default badge on the Automations page
- [ ] A default is switched on in place and deleting it resets it
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-AUT-31: in the mock studio project, "Defaults" lists the three shipped automations and
  "Custom" the two others; a shipped automation created by an agent shows "Default", not "Created
  by an agent"; "Ready-made automations" is gone; no WCAG violations.
- AC-AUT-32: turning on "Keep the docs up to date" (not added) creates it enabled: the toast says it
  is on and its switch stays on in the Defaults section, now with Run now; turning it off pauses it.
- AC-AUT-33: deleting an added default says it stays among the defaults, off; afterwards it is still listed under
  Defaults with its switch off and no Run now.
- AC-AUT-34: a person with read access sees the switches disabled.

## Test plan

- e2e (`e2e/tests/automations.e2e.ts`, mock server): the existing ready-made test rewritten for
  AC-AUT-31..33, plus a read-only check (AC-AUT-34). The mock catalog gains "Deploy dev every night"
  so it matches the shipped three, and one shipped automation created by an agent.
- No server change: the API (templates with `automation_id`, enable, update, delete) is unchanged
  and covered by `crates/yulius/tests/automations.rs`.

## Decisions

- [ADR-0159](../../docs/decisions/0159-shipped-automations-are-the-automations-page-defaults.md):
  shipped automations are shown as the page's Defaults, switched on in place, recognised by name;
  their content stays an editable copy (ADR-0034), so "Default" does not mean read-only as for
  workflows, and delete is their reset.
