---
yulius-task: d0f188af-9197-4cec-94ce-366e767d9533
status: in-progress
---

# SPEC-423: A sandbox's admins turn the production-branch protection requirement off on the Git page

Status: in-progress
Date: 2026-11-05
Owner: Yulius task #405
Scope authorization: task #405 (`/goal`), with a screenshot of an onboarding agent's "Needs you"
card: "Yulius tried to protect the production branch main from agents with a GitHub ruleset;
GitHub refused (HTTP 403: rulesets need GitHub Pro or a public repository) … To do: Repo admin: add
that ruleset on main (needs GitHub Pro/Team or a public repo), or decide to accept the risk". The
request: "we want be able to explicitly disable this kind of protection for users with no access to
github pro. It should be done in Git section and on this kind of message user should be informed
how to do it exactly". Extends SPEC-141 (ADR-0114, the production ruleset) and SPEC-157
(ADR-0129, `production_protect`).

## Goal

A repository whose GitHub plan cannot have rulesets (a private repository without GitHub Pro, Team
or Enterprise) can still be used with Yulius without an unresolvable "Needs you": a sandbox admin
explicitly accepts that agents' token could write the production branch by switching the
requirement off on the sandbox's Git page, and every message that asks for the ruleset says exactly
where that switch is. While it is off, Yulius stops warning about, and asking agents to add, the
production ruleset; switching it on again restores today's behavior.

## Context

- ADR-0114: agents hold a GitHub token without Administration; a ruleset that only repository
  admins bypass keeps them off the production branch. `App::production_branch_warning` asks GitHub
  with the agents' token (`branch_guard`) and returns a warning when the branch is writable or the
  check failed. It is shown as `onboarding_status.warnings` (sandbox API, `yulius project onboard`,
  the import CLI) and prepended to the onboarding prompt with the instruction to call
  `production_protect` and, when refused, `needs_you`.
- ADR-0129: `production_protect` adds `yulius-people-only-<branch>` as the onboarding task's owner.
  GitHub answers 403 "Upgrade to GitHub Pro or make this repository public to enable this feature"
  for a private repository on a free plan; the tool then refuses with `production_ruleset_fix`
  (the ruleset to add) and the agent raises "Needs you". Nothing a person can do in Yulius answers
  it: the card offers only "add the ruleset (needs Pro) or decide to accept the risk", without
  saying how to record that decision.
- The Vidoc switch (SPEC-338, SPEC-359, ADR-0229, ADR-0245) is the precedent for a per-sandbox
  switch: stored as `"vidoc": false` in the sandbox's profile JSON, exposed as `profile.vidoc`,
  changed with `PATCH /api/sandboxes/{sid}` (`Action::Administer`), audited, kept by profile
  updates, and its failures name where it is.
- The Git page (`/s/<slug>/git`, `pages/git.js`) is where the delivery profile, including the
  production branch, is configured.

## Requirements

- **PPR-001 One stored switch.** `profile.protect_production` (contract `Profile`): `false` means a
  sandbox admin accepted that agents could write the production branch; absent or `true` keeps
  today's requirement. Responses always carry a value; stored only as `"protect_production": false`
  in the profile JSON, read on its own so an otherwise unreadable profile keeps it
  (`delivery::production_protection_required`). A profile update keeps the stored value.
- **PPR-002 Admins change it.** `PATCH /api/sandboxes/{sid}` `{"protect_production": bool}` with
  `Action::Administer` (GitHub admin on the repository, or a superadmin); a writer gets 403, a
  playground `Invalid` (it has no GitHub branch). It can be sent alone or with a profile, in one
  write. Each change is audited as `delivery.production_protection.update` (`on`/`off`).
- **PPR-003 Off means no requirement.** While off, `production_branch_warning` is `None` (no
  onboarding warning, nothing in the onboarding prompt about protecting production), and
  `production_protect` changes nothing on GitHub and answers `waived: true` with a message telling
  the agent to resolve a `needs_you` it raised about it. Switching it on again restores the
  warning, the prompt and the tool.
- **PPR-004 Every request for the ruleset says how to waive it.** `production_ruleset_fix`, and so
  every onboarding warning, the onboarding prompt and every `production_protect` refusal (including
  GitHub's 403 for the plan), ends with: without rulesets on the plan, a sandbox admin can turn the
  requirement off instead, at `<public_url>/s/<slug>/git#production-protection`, card "Production
  protection", switch "Require a GitHub ruleset on `<branch>`". The onboarding prompt tells the
  agent to quote it in `needs_you`.
- **PPR-005 The Git page card.** Below the board, a "Production protection" card
  (`id="production-protection"`) shows the switch "Require a GitHub ruleset on `<branch>`" with
  what on and off mean, a link to the repository's GitHub rulesets settings, and for non-admins
  that only the sandbox's admins (or a superadmin) can change it (the switch disabled). A playground
  has no card. Changing it sends the PATCH, toasts a failure and shows the stored value again.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] A sandbox's admins switch production protection off and on through the sandbox PATCH
- [ ] A switched-off sandbox gets no production warning or protect instruction, and the tool says it is waived
- [ ] Every ruleset request and refusal says exactly where to turn the protection off
- [ ] The Git page's Production protection card shows and flips the switch
- [ ] ADR, architecture, API, operations and product docs describe the switch
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1: an admin's PATCH `{"protect_production": false}` stores it, the sandbox view says
  `profile.protect_production: false`, an audit entry is written; a writer's PATCH is 403; a later
  profile update keeps it; `{"protect_production": true}` restores it.
- AC2: with the switch off and the branch unprotected, `onboarding_status.warnings` is empty and a
  started onboarding task's prompt does not mention `production_protect`; `production_protect`
  answers `waived: true` and creates no ruleset.
- AC3: with the switch on, the warning, the prompt and the tool's refusal for GitHub's 403 each
  contain the Git page address with `#production-protection` and the switch's label.
- AC4: on the Git page an admin sees the card with the switch on, turns it off, and the stored
  profile says `protect_production: false`; a writer sees it disabled with who can change it.

## Test plan

- Rust integration (`crates/yulius/tests/import_server.rs`, fake GitHub): AC1 through the HTTP
  PATCH and the store; AC2 and AC3 through `onboarding_status`, the onboarding prompt and the MCP
  tool with `refuse_rulesets`.
- Unit: `production_protection_required` reads `false` only; absent, `true` and unreadable JSON
  are on.
- e2e (`e2e/tests`): AC4 on the mock server, admin and writer.

## Decisions

- **A switch, not an automatic fallback.** Yulius could stop asking once GitHub says 403 for the
  plan; the request is to disable it *explicitly*, and accepting that agents can push to
  production is a person's decision (ADR-0114), so it stays an admin's audited act.
- **Stored with the profile, like Vidoc.** The same store, permission, audit and PATCH as the
  Vidoc switch; no migration. A separate endpoint or column would be a second path to the same
  kind of setting.
- **On the Git page as its own card, not in the Delivery profile dialog.** The request names the Git
  section; a card lets everyone see the state (the dialog is admin-only) and gives the message an
  anchor to point at.
- **Off silences, it does not hide the truth on demand.** The card does not call GitHub; it says
  what the switch means. Checking the branch live on every page load would add a GitHub call per
  visit for a fact onboarding already reports when the switch is on.
- **The tool answers `waived` rather than adding the ruleset anyway.** With the requirement off the
  agent was told nothing about it; an agent that still calls it (an older prompt) should stop asking
  the person, not retry a ruleset the plan refuses.
- Lasting decision recorded as ADR-0267.
