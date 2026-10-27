# 0229. A person switches a project's Vidoc scans off in Yulius

## Status

Accepted. Specified in [SPEC-338](../../.ai/specs/SPEC-338-in-vidoc-related-problems-we-want-to-be.md);
task #322. Amends [ADR-0157](0157-vidoc-is-an-optional-instance-key-and-the-scan-runs-on-vidoc-1.md)
(where the key applies) and [ADR-0107](0107-a-committed-switch-turns-the-vidoc-gate-on-or-off.md)
(the committed switch stays; this one is Yulius's, per project).

## Context

The instance's Vidoc key ran out of quota (HTTP 429, "used its 50,000,000 tokens"; 2026-10-22 to
2026-10-27). Every task's scan then ended "not run", CI's `security` job ended skipped, and agents
asked people with `needs_you` whether to pay for more. The person answered "skip vidoc for now" and
asked to be able to switch Vidoc off for a project until the quota is back, with Vidoc optional in
general and managed at onboarding and in the admin panel.

Until now Vidoc was on wherever a `VIDOC_API_KEY` reached a run: the instance key or a project's own
secret. Turning it off meant removing the instance key (every project, pasted again later),
deleting a project's secret, or a reviewed `enabled = false` commit in the project's repository.

## Decision

- **A per-project switch in Yulius.** `vidoc` in the sandbox's stored profile JSON: absent or
  `true` is on, `false` off. `PATCH /api/sandboxes/{sid}` `{"vidoc": bool}` (sandbox admin, as the
  delivery profile; superadmins pass) sets it on its own, a profile update keeps it, a playground
  refuses it, and each change is audited as `delivery.vidoc.update`. The view always shows it.
- **Off scans nothing, whatever key the project has.** Yulius gives the project's agent runs,
  terminals and CI jobs `YULIUS_VIDOC=off`; the scanner then prints "Security scan disabled: Vidoc is turned off for
  this project in Yulius; nothing was scanned." and exits 0, before it reads a key or Git (with
  `--ci`, the skip code: Skipped, never Passed). The instance key is not applied as the project's
  virtual `VIDOC_API_KEY`, and Yulius CI adds no built-in `security` job.
- **Agents are told.** The briefing's `## Security scan` section says Vidoc is off for the project,
  that the scan reports "disabled", never a pass, and not to ask anyone about the key or its quota;
  the onboarding prompt adds no scan rule and says why.
- **Managed where people look.** The Instance page's Vidoc card lists the projects with a switch
  each; the Add sandbox dialog offers "Scan pull requests with Vidoc" (checked) when the instance
  has a key (`me.instance.vidoc`); an unchecked box sends `"vidoc": false` with the create request,
  and the server switches the new project off before its onboarding starts. Profile writes are
  serialized, and a request whose profile is refused changes nothing. The setup wizard's Vidoc key step stays optional.

## Alternatives

- **An instance-wide pause.** Removing the key already stops every project that uses it, and it
  would not stop a project's own key, which was this repository's case.
- **A new sandbox column.** It needs a migration and every `SandboxRecord` literal (28) changed;
  the profile JSON is already the per-project settings store, read directly as `gate` is.
- **Only dropping the key.** A project's own key, and its own committed `security` job (this
  repository's), would keep scanning and keep failing on the quota.
- **Only the committed `enabled = false`.** It needs a pull request in each repository, is not
  visible in Yulius, and takes effect only for branches that merged it.
- **An agent-set environment variable** (rejected by ADR-0107). Here only Yulius sets it, from a
  person's setting; an agent setting it itself only skips its own local run, while CI follows the
  setting.

## Consequences

- A switched-off project's pull requests merge unscanned, visibly: its CI has no scan job, or its
  own job reads Skipped with the reason, and the Vidoc card shows the switch off.
- Turning a project back on takes effect at the next run or CI job; nothing is rescanned by itself.
- The switch is per Yulius instance, not in the repository: another instance managing the same
  repository decides for itself.
