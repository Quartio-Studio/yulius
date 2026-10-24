---
yulius-task: d8437e67-a96a-41e2-bb69-1be7653a49df
status: in-progress
---

# SPEC-292: Feature and Fix workflows no longer tell agents to promote production

Status: in-progress
Date: 2026-10-24
Owner: task #278
Scope authorization: task #278 (issue #556, started by the nightly issue automation, task #277).

## Goal

No instruction Yulius ships to agents tells them to promote `dev` to `main` or to verify
production. The Feature and Fix workflows, the Feature workflow's description and the AGENTS.md
template Yulius generates for new projects all end delivery at a merge on green and cleanup, say
that dev is deployed only when the task asks, and leave production, with its promotion, to a person
(ADR-0110).

## Context

- Symptom (#556): `catalog/workflows/feature.md:33-34` and `catalog/workflows/fix.md:35-36` end
  with "deliver: merge on green, deploy and verify, promote, verify production, clean up
  (`git-hygiene`)". Every Feature and Fix task reads that in its briefing's "Workflow
  instructions", including this one.
- The same contradiction, inside this issue's scope: the Feature workflow's description
  (`workflows_list`) says "from spec to verified production", and `agents_md()` in
  `crates/yulius-domain/src/spec_format/templates.rs` (the AGENTS.md of scaffolded projects) says
  "deploy and verify `dev`, promotion to `main` by a merge commit, verify production".
- The rule they break: ADR-0110 (agents never promote; only a person deploys production), the
  `delivery-pipeline` skill (section 7: "Production, with its promotion, ... always a person's
  decision and action") and the session briefing ("`main` is a person's: never promote").
- Root cause: the workflows and the template were written in #148 (2026-10-11), when an agent could
  still promote with `delivery_promote` if its task asked. #487 (2026-10-15) accepted ADR-0110,
  removed the promotion tool and rewrote the `delivery-pipeline` skill and the nightly dev deploy
  automation, but left these three texts. No test ties the shipped agent instructions to the rule:
  `the_shipped_catalog_is_valid` checks that workflows parse, and `scaffold_contents_per_template`
  checks what AGENTS.md contains, never what it must not say.
- The other bundled workflows (chore, docs, infra-bootstrap, research) do not mention promotion.

## Requirements

- FIX-1: No bundled workflow (its whole file: description, steps and instructions) contains the verb
  "promote" (any form but the noun "promotion") or "verify production" in any form.
- FIX-2: The Feature and Fix instructions end with: merge on green, clean up (`git-hygiene`), deploy
  dev only when the task asks (`cd_deploy`), and production being a person's.
- FIX-3: The Feature description ends delivery at a merge on green, not at "verified production".
- FIX-4: The generated AGENTS.md's Delivery section ends the agent's path at merge on green and
  cleanup, deploys `dev` only when asked, and says production, with its promotion to `main`, is a
  person's decision and action; it never tells the agent to promote or verify production.
- FIX-5: Every workflow's instructions block stays within the briefing budget (1,444 bytes, #555).

## Plan

- [x] Root cause confirmed and fix spec written
- [ ] Regression tests that fail on the promote instructions
- [ ] Feature, Fix and AGENTS.md template tell agents not to promote
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, changes log updated

## Acceptance criteria

- `catalog/workflows/feature.md` and `fix.md` no longer tell the agent to promote or verify
  production; the Feature description no longer says "verified production".
- `no_workflow_tells_agents_to_promote` (next to `the_shipped_catalog_is_valid`) fails on dev and
  passes with the fix; it rejects "promote" and production verification in every bundled workflow
  and requires the Feature and Fix delivery clause to name `cd_deploy` and a person's production.
- `agents_md_leaves_production_to_a_person` fails on dev and passes with the fix.
- A `docs/changes.md` entry.

## Test plan

- Unit, `crates/yulius-app/src/workflows/catalog.rs`: `no_workflow_tells_agents_to_promote`
  scans every embedded workflow file (lowercased words), so a new workflow that tells agents to
  promote fails too. It allows the noun "promotion", since a sentence saying promotion is a
  person's job needs it.
- Unit, `crates/yulius-domain/src/spec_format/templates.rs`:
  `agents_md_leaves_production_to_a_person` renders AGENTS.md for every template.
- The existing budget assertion in `the_shipped_catalog_is_valid` covers FIX-5.
- Regression: both new tests are committed first and shown to fail on the unchanged texts.

## Decisions

- Reject the verb, allow the noun. Rejecting every occurrence of "promot" would forbid the sentence
  that states the rule; rejecting the verb forms and "verify/verified/verifies production" catches
  the instruction (its old wording and plausible rewordings) while the rule stays sayable.
- Scan whole workflow files, not only `instructions`: the description reaches agents through
  `workflows_list` and was wrong too.
- No ADR: this applies ADR-0110; it takes no new decision.
- What else the cause could break: AGENTS.md files already generated for projects keep the old
  Delivery line; Yulius does not rewrite a project's AGENTS.md, and the agent briefing and the
  `delivery-pipeline` skill (both current) say production is a person's. Out of scope.
