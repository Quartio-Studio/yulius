# 0267. A sandbox's admins waive the production ruleset on its Git page

## Status

Accepted. Specified in [SPEC-423](../../.ai/specs/SPEC-423-we-want-be-able-to-explicitly-disable-th.md);
task #405. Amends [ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md) (the
ruleset is required unless a sandbox admin waives it) and
[ADR-0129](0129-onboarding-agents-protect-production-as-the-onboarding-person.md)
(`production_protect` answers `waived`); follows the switch pattern of
[ADR-0245](0245-a-sandboxs-admins-switch-its-vidoc-scans-on-its-ci-cd-page.md).

## Context

ADR-0114 keeps agents off the production branch with a GitHub ruleset only repository admins
bypass; onboarding warns while it is missing and its agent adds it with `production_protect`.
GitHub refuses rulesets on a private repository without GitHub Pro, Team or Enterprise (HTTP 403
"Upgrade to GitHub Pro or make this repository public"). The agent then raised "Needs you" with
"add that ruleset (needs GitHub Pro/Team or a public repo), or decide to accept the risk", and
nothing in Yulius recorded that decision: the warning and the request came back. The request
(task #405): explicitly disable this protection for users without GitHub Pro, in the Git section,
and tell the person exactly how in that message.

## Decision

- **One switch per sandbox, stored with the profile.** `profile.protect_production`, stored only as
  `"protect_production": false` in the sandbox's profile JSON and read on its own
  (`delivery::production_protection_required`), always a boolean in responses, kept by profile
  updates. `PATCH /api/sandboxes/{sid}` `{"protect_production": bool}` changes it with
  `Action::Administer` (sandbox admin or superadmin), audited as
  `delivery.production_protection.update`; a playground (no GitHub branch) is refused. Turning it
  off is a person's decision: a browser session or an `operate` token (`acts_for_person`), never a
  `yulius login` token, which an agent may hold; any of them may turn it on again. Sent with the
  Vidoc switch or a profile, everything is one write of the stored profile, or nothing on a refusal.
- **Off means Yulius no longer requires the ruleset.** `production_branch_warning` answers nothing,
  so onboarding neither warns nor tells its agent to protect production, and `production_protect`
  changes nothing on GitHub and answers `waived: true`, telling the agent to resolve its
  `needs_you`. GitHub's own protection is untouched either way.
- **Every request for the ruleset names the switch.** `production_ruleset_fix` (the warning, the
  onboarding prompt and every `production_protect` refusal, GitHub's 403 included) ends with: without
  GitHub Pro, Team or Enterprise a sandbox admin can accept the risk at
  `<public_url>/s/<slug>/git?show=production-protection`, card "Production protection", switch "Require a
  GitHub ruleset on `<branch>`". The prompt tells the agent to quote it in `needs_you`.
- **The Git page shows it.** A "Production protection" card under the activity, drawn apart from
  the delivery overview (also when that cannot be read), visible to every member, switchable by
  admins, with what on and off mean and a link to the repository's rulesets;
  the page scrolls to it when opened with `?show=production-protection` (a query, which in-app links and the sign-in redirect keep).

## Alternatives

- **Stop asking automatically after GitHub's 403.** The request is an explicit choice, and letting
  agents write production is a person's risk to accept (ADR-0114); an automatic fallback would also
  hide a plan that later gains rulesets.
- **A field of the delivery profile dialog.** Admin-only and saved as a whole; members could not see
  the state, and a message could not link to it.
- **A column or its own endpoint.** A second store and permission path for the same kind of switch
  the Vidoc one already uses.
- **Checking GitHub live on the card.** One more GitHub call per Git page visit for what onboarding
  already reports while the switch is on.
- **Letting `production_protect` still try when waived.** The plan refuses it, and the agent would
  ask the person again for what they just declined.

## Consequences

- A project on a free plan's private repository finishes onboarding without an unanswerable
  "Needs you"; the person sees and can undo the choice on the Git page, and the audit log says who
  made it.
- While off, agents' token can move the production branch on GitHub; Yulius's own rules (only a
  person promotes, ADR-0110) still apply to what agents do through Yulius, but GitHub no longer
  enforces them.
- Switching it on again restores the warning and `production_protect` at the next onboarding
  status or task.
