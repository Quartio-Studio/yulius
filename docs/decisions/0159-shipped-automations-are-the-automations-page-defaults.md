# 0159. Shipped automations are the Automations page's defaults, switched on in place

## Status

Accepted. Specified in
[SPEC-197](../../.ai/specs/SPEC-197-those-2-automations-should-be-defaults-l.md) (task #183);
amends [0034](0034-ready-made-automations-catalog.md) (how the shipped automations are shown).

## Context

The automations Yulius ships (`catalog/automations/*.md`) were offered below the project's list as
"Ready-made automations" with an Enable button; once added, one looked like any other automation.
On Yulius's own instance an agent created two of them by name (SPEC-042 OPS-010), so the page
showed them as "Created by an agent" and the third as a suggestion below. People expect them to be
defaults, as the default workflows are: listed first, marked "Default", part of every project.

## Decision

- The Automations page has two sections, as the Workflows page: **Defaults** ("ship with Yulius,
  on or off per project"), every shipped automation in catalog order, then **Custom** ("yours to
  change"), the project's other automations, with its own empty state.
- A shipped automation is a default whether or not the project has added it. Added means the
  project has an automation of its name (0034's link, unchanged); its card is that automation's,
  badged "Default" (never "Created by an agent") with the shipped description. Not added, it shows
  its schedule, "Off" and a switch that is off; switching it on calls the existing enable endpoint.
- Deleting an added default is its reset: it stays among the defaults, off, and switching it on
  again starts from the version shipped then. The dialog says so; the verb stays Delete (#147).
- No API or store change: `GET …/automation-templates` (with `automation_id`), enable, update and
  delete already carry everything.

## Alternatives

- **Read-only defaults kept current, as default workflows.** The run would take its prompt from
  the catalog, customised by duplicating. An automation is per-project state people edit (schedule,
  start choice, prompt), and nothing but the name links a row to the catalog, so a person's own
  automation of that name would silently lose its prompt. Rejected; 0034's copy-once stays and
  Delete gives the current version back.
- **Seed the defaults into every project as disabled rows.** Rejected for 0034's reasons (a creator
  per row, a backfill, deleted rows coming back).
- **Only drop the "Created by an agent" badge for shipped names.** Fixes the screenshot but leaves
  defaults split across the list and a suggestion section. Rejected.

## Consequences

- Every project shows all shipped automations; a new one shipped in a release appears there, off.
- A renamed default leaves the Defaults section (it becomes Custom) and the shipped one shows off
  again, as 0034 already accepted.
- `created_via` is still stored and shown for custom automations; for defaults the page does not
  show who added them.
