# 0245. A sandbox's admins switch its Vidoc scans on its CI/CD page

## Status

Accepted. Specified in [SPEC-359](../../.ai/specs/SPEC-359-vidoc-can-be-turned-off-in-a-sandbox-s-o.md);
task #339. Amends [ADR-0229](0229-a-person-switches-a-projects-vidoc-scans-off-in-yulius.md) (the
project switch gets a place in the sandbox and a pointer from failing scans) and complements
[ADR-0242](0242-a-superadmin-switches-vidoc-off-for-the-whole-instance.md) and
[ADR-0107](0107-a-committed-switch-turns-the-vidoc-gate-on-or-off.md).

## Context

The Vidoc key ran out of quota (HTTP 429, 2026-10-22 to 2026-10-27) and has been refused since
2026-10-30 (HTTP 401). Each time every pull request's `security` job ended skipped or blocked, and
agents raised "Needs you" about it. ADR-0229's project switch could be flipped through
`PATCH /api/sandboxes/{sid}` by a sandbox admin, but its only screen was the superadmin's Instance
page; the person asked for the switch "per a single sandbox", without the superadmin. Nothing in
Yulius showed a repository's own committed `enabled = false`, and a failing scan named only the
Instance page.

## Decision

- **The switch lives on the sandbox's CI/CD page.** A "Security scan (Vidoc)" card after the CI
  configuration card of the CI tab (`/s/<slug>/ci`, `pages/cividoc.js`, read with the runs) shows the one switch ADR-0229 stores and changes
  it with the same PATCH; the Instance card and the Add sandbox dialog keep working on the same
  value, and the shared sandbox list keeps them in step.
- **Its admins flip it.** The PATCH keeps `Action::Administer` (GitHub admin on the repository, or a
  superadmin), as the delivery profile; writers and maintainers see the card disabled with who can
  change it. Each change stays audited as `delivery.vidoc.update` with who, when and on/off.
- **A read-only view says why a scan runs or not.** `GET /api/sandboxes/{sid}/vidoc` answers the
  project switch, the instance switch, whether a key reaches the project, the repository's
  `.config/vidoc.toml` at the integration head (`on`, `off`, `unreadable`; read by
  `yulius_domain::ci::vidoc::repository_vidoc`), whether a scan runs, whether the caller may
  change it, and the last change (`AuditStore::last_audit`). The repository's `enabled = false`
  wins: the card shows the switch off and disabled, says why, and offers no way on.
- **Failing keys point here.** Agent runs, terminals and CI jobs of a non-playground sandbox get
  `YULIUS_VIDOC_SWITCH=<public_url>/s/<slug>/ci`. On a refused key (401/403) or a used-up quota
  (402, or a 429 that says so) the scanner adds that a sandbox admin turns Vidoc off there (a
  superadmin also in Settings, Instance), so CI's job log and the agents' `needs_you` name it; the
  briefing and the onboarding prompt say so too.

## Alternatives

- **A new sandbox settings page.** A sandbox's settings live where they act (the delivery profile
  on the Git page, CI on the CI tab); a page for one switch would be a second home for settings.
- **The delivery profile dialog.** It is a form saved as a whole, admin-only and on the Git page,
  far from the CI jobs the switch decides; members could not see the state.
- **Letting writers flip it.** Turning the scan off loosens the merge gate, which only admins may
  do elsewhere (`require_checks`).
- **A second write endpoint (`PUT /api/sandboxes/{sid}/vidoc`).** Another path to the same store
  and permission; the PATCH already does it.
- **Refusing the PATCH while the repository is off.** The project's switch is Yulius's own choice;
  refusing it would break the Instance card's switch for that project and change nothing that
  scans. The card simply does not offer it.
- **The scanner guessing the address** (from `YULIUS_SANDBOX`). It does not know Yulius's public
  URL; Yulius sets it, as the `yulius/ci` commit status links to the same page.

## Consequences

- A sandbox's admins pause and resume Vidoc for their project without the instance admin; the card
  says who did it last.
- The repository's switch is read from the sandbox's clone at `origin/<integration>`; with no clone
  or branch yet, it counts as on, as CI reads `yulius.toml` there. A branch that changes the file
  is not reflected until it merges.
- The Instance card does not show each repository's file (it would read Git for every project).
- An older scanner copy ignores `YULIUS_VIDOC_SWITCH` and prints its message as before.
