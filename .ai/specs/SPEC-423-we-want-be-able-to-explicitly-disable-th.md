---
yulius-task: d0f188af-9197-4cec-94ce-366e767d9533
status: done
---

# SPEC-423: A sandbox's admins turn the production-branch protection requirement off on the Git page

Status: done
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
  playground `Invalid` (it has no GitHub branch). Turning it off needs a person (a browser session or
  an `operate` token); a `yulius login` token gets 403. It can be sent alone, with the Vidoc switch
  or with a profile, in one write; a refused profile changes nothing. Each change is audited as `delivery.production_protection.update` (`on`/`off`).
- **PPR-003 Off means no requirement.** While off, `production_branch_warning` is `None` (no
  onboarding warning, nothing in the onboarding prompt about protecting production), and
  `production_protect` changes nothing on GitHub and answers `waived: true` with a message telling
  the agent to resolve a `needs_you` it raised about it. Switching it on again restores the
  warning, the prompt and the tool.
- **PPR-004 Every request for the ruleset says how to waive it.** `production_ruleset_fix`, and so
  every onboarding warning, the onboarding prompt and every `production_protect` refusal (including
  GitHub's 403 for the plan), ends with: without rulesets on the plan, a sandbox admin can turn the
  requirement off instead, at `<public_url>/s/<slug>/git?show=production-protection`, card "Production
  protection", switch "Require a GitHub ruleset on `<branch>`". The onboarding prompt tells the
  agent to quote it in `needs_you`.
- **PPR-005 The Git page card.** Below the board, a "Production protection" card
  (`id="production-protection"`) shows the switch "Require a GitHub ruleset on `<branch>`" with
  what on and off mean, a link to the repository's GitHub rulesets settings, and for non-admins
  that only the sandbox's admins (or a superadmin) can change it (the switch disabled). A playground
  has no card. It is drawn apart from the delivery overview, so it is there when that fails to
  load, and `?show=production-protection` scrolls to it. Changing it sends the PATCH, toasts a
  failure and shows the stored value again at once, also when the page redrew meanwhile; the
  keyboard stays on the switch.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A sandbox's admins switch production protection off and on through the sandbox PATCH
- [x] A switched-off sandbox gets no production warning or protect instruction, and the tool says it is waived
- [x] Every ruleset request and refusal says exactly where to turn the protection off
- [x] The Git page's Production protection card shows and flips the switch
- [x] ADR, architecture, API, operations and product docs describe the switch
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1: an admin's PATCH `{"protect_production": false}` stores it, the sandbox view says
  `profile.protect_production: false`, an audit entry is written; a writer's PATCH is 403; a later
  profile update keeps it; `{"protect_production": true}` restores it.
- AC2: with the switch off and the branch unprotected, `onboarding_status.warnings` is empty and a
  started onboarding task's prompt does not mention `production_protect`; `production_protect`
  answers `waived: true` and creates no ruleset.
- AC3: with the switch on, the warning, the prompt and the tool's refusal for GitHub's 403 each
  contain the Git page address with `?show=production-protection` and the switch's label.
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

## Evidence

- Behavior tests, each seen failing before its change: `crates/yulius/tests/production_protection.rs`
  (AC1 PATCH, audit, profile updates keep it, writer 403, `yulius login` token 403 for turning it
  off, refused profile changes nothing, both switches in one write, playground 422; AC2 no warning
  while off; AC3 the warning names `?show=production-protection` and the switch label),
  `crates/yulius/tests/import_server.rs` (AC2 onboarding prompt without `production_protect` and the
  tool answering `waived` with no ruleset created; AC3 the prompt and GitHub's 403 refusal name the
  switch), unit `delivery::switch_tests` (stored-switch reading), and
  `e2e/tests/production-protection.e2e.ts` (AC4 admin keyboard toggle with focus kept, member
  disabled, refused change redrawn at once also after a mid-request redraw and after leaving and
  revisiting the page, card present when the overview fails, `?show=` scrolls to it). The card's
  failure tests were checked failing against the previous card code before the fixes.
- `scripts/dev ci` at `aed9a0e5`: exit 0 (repository check, script tests, check, fmt, clippy Linux
  and macOS, nextest 3131 passed / 16 skipped, doctests, deny, shear, doc). After it only web files,
  the e2e test and one doc comment changed; at `bdf9108a`: `cargo fmt --all -- --check` and
  `scripts/check_repository.py` pass, and `npx e2e run tests/production-protection.e2e.ts
  tests/git.e2e.ts` passes 12/12 twice. Yulius CI reruns the whole gate and e2e on the final head.
- Full e2e suite (`npx e2e run`): 828 passed, 1 skipped (run on the first card version; the files
  touched afterwards were rerun above).
- Security scan: `yulius security-scan` printed "Security scan disabled: Vidoc is turned off in
  Yulius"; disabled, not passed.
- Independent review: round 1 request changes (1 major, 6 minor, 3 nits, all own code), fixed in
  `aed9a0e5`; round 2 approve with 1 minor and 3 nits, fixed in `bdf9108a` (the README order nit is
  pre-existing: the index is not ordered around 0255-0267); round 3 on the final head.
- Not tested: a real GitHub plan refusing rulesets (the fake GitHub returns the same 403 text), and
  the Git page in a production build in a browser outside the mock.
