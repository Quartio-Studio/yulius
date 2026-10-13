# 0033. Ready-made automations ship as a catalog, enabled in one click and matched by name

## Status

Accepted. Specified in [SPEC-042](../../.ai/specs/SPEC-042-create-2-default-automations-that-can-be.md).
Follows the default-workflow catalog of AUT-006 (`catalog/workflows/`).

## Context

People asked for two automations every project should be able to switch on easily: keep the docs
in line with the day's changes, and work through the core team's open issues, both nightly at
03:00 CEST. Automations are per-project rows attributed to a person; there were no defaults. Two
parallel branches add migration 0013, so a schema change would collide.

## Decision

- Ready-made automations are Markdown files in `catalog/automations/<id>.md` (frontmatter `name`,
  `description`, `cron`, `utc_offset_minutes`, optional `workflow`, `order`; the body is the
  prompt), embedded by `crates/yulius-app/build.rs` and parsed by
  `yulius_domain::automation_format`. A unit test keeps the shipped files valid.
- They never run by themselves. The Automations page lists those not added yet; Enable
  (`POST /api/sandboxes/{sid}/automation-templates/{id}/enable`, write access) creates an
  ordinary automation, enabled and attributed to the person who clicked.
- A project's automation with the template's name (case-insensitive) counts as the template's.
  Enable on it re-enables it and keeps its content; nothing links the two in the store.
- The schedule is a fixed offset (+120 for CEST), as for every automation.

## Alternatives

- **Seed disabled automations into every project.** Needs a creator for each row, a backfill for
  existing projects, and a record of deleted ones so they are not re-seeded; an unchanged seed
  would also keep stale prompt text forever. Rejected.
- **Store a template id on the automation.** Exact, but needs a migration now and would not
  recognise automations created by agents through `automation_create` with the same content.
  Rejected for now; a rename makes the suggestion reappear, which is harmless.
- **Update enabled automations when a release changes a prompt.** Would overwrite people's
  edits. Rejected: enabling copies the content once.

## Consequences

- A release changes what new Enables get, never automations already enabled.
- A renamed automation lets its template be offered again; enabling it then creates a second one.
- After the switch to CET the ready-made automations run at 02:00 local time unless people change
  the offset.
- Agents cannot yet enable a template by id (issue #293); they create the same automation by
  name, cron and prompt, which the page then recognises.
