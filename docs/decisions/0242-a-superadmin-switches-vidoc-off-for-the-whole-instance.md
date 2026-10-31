# 0242. A superadmin switches Vidoc off for the whole instance

## Status

Accepted. Specified in [SPEC-356](../../.ai/specs/SPEC-356-vidoc-security-should-be-optional-and-su.md);
task #338. Amends [ADR-0229](0229-a-person-switches-a-projects-vidoc-scans-off-in-yulius.md), which
rejected an instance-wide pause; the per-project switch stays.

## Context

The person asked that Vidoc security be optional and that a superadmin be able to disable it. The
only instance-wide ways to stop Vidoc were removing the instance key, which must be pasted again
later and cannot be done at all in the UI for a key from `YULIUS_VIDOC_API_KEY_FILE`, or switching
every project off one by one, which new projects do not follow. ADR-0229 assumed removing the key
stops every project; a project with its own `VIDOC_API_KEY` secret keeps scanning. The key's quota
and its refusals (HTTP 429 and 401, 2026-10-22 to 2026-10-30) kept stopping every task's scan.

## Decision

- **One instance switch, on by default.** `vidoc_off_at` and `vidoc_off_by` on the `instance` row
  (migration 0045): off while set. `PUT /api/admin/vidoc/enabled` `{"enabled": bool}`, superadmin
  only, changes it at once (cached in `VidocServices`, read at start) and audits `vidoc.disabled`
  or `vidoc.enabled` on `instance:vidoc`; `GET /api/admin/vidoc` shows `enabled`, `disabled_at`
  and `disabled_by`. The key is kept and can still be saved, replaced, removed and tested.
- **Off, every project is off.** `App::vidoc_scans_on(sandbox)` is the instance switch and the
  project's own (`delivery::vidoc_switched_on`); every place that ran a scan for a project uses
  it: `YULIUS_VIDOC=off` for agent runs, terminals and CI jobs, no instance key as the virtual
  `VIDOC_API_KEY`, no built-in `security` job. A project's own key and own job stop too, as with
  the project switch. `me.instance.vidoc` is false, so the Add sandbox dialog offers no Vidoc box.
- **Sessions already running send nothing either.** An agent run, terminal or Superagent that
  started before the switch went off has no `YULIUS_VIDOC=off` and an older briefing; the egress
  proxy therefore intercepts `model.vidocsecurity.com:443` for a project whose scans are off (the
  instance's or its own) and refuses each request with 403 "Yulius: Vidoc is turned off in Yulius
  …; nothing was sent", whatever key it carries. The scanner reports that as not run, saying
  Vidoc is off, not as a refused key. A switch that cannot be read counts as off; at the proxy it
  refuses with 503 instead, so the scan ends incomplete (a CI error that blocks the merge), never
  "off" (Skipped).
- **The same variable.** The scanner keeps reading `YULIUS_VIDOC=off`, and its disabled line says
  "turned off in Yulius, for this project or the whole instance". The briefing tells agents which
  (`VidocBriefing::OffForInstance`), and the onboarding prompt adds no scan rule.
- **Projects keep their choice.** The project view and its PATCH keep showing and setting the
  project's own switch; the Vidoc card shows them disabled while the instance is off, and turning
  the instance on restores each project's choice.

## Alternatives

- **Removing the key.** It loses the key, cannot remove a configuration-file key, and leaves
  projects with their own key scanning.
- **Switching every project off.** Many writes, new projects start on, and turning back on loses
  which projects a person had switched off on purpose.
- **Writing the instance state into each project's switch.** The same loss of each project's
  choice, and the project view would claim a choice nobody made.
- **A new `YULIUS_VIDOC` value for the instance.** An older scanner copy (a repository's own
  `scripts/vidoc_scan.py`) honours only `off` and would scan with a project's own key.
- **A configuration flag** (environment or file). Needs a restart and server access, and is
  invisible on the Instance page.

## Consequences

- While off, every project's pull requests merge unscanned, visibly: the Vidoc card says since when
  and by whom, CI has no scan job or its own job reads Skipped, and agents report "disabled".
- Switching off takes effect at once for every request toward Vidoc; a running session's own
  environment and briefing still say "on" until its next run, and its scan ends "not run" with the
  reason. Interception needs the proxy's CA in the session (as for any bound secret): without it
  the TLS handshake fails, nothing is sent, and the scan ends incomplete. Turning it on takes effect at the next run or CI job; nothing is rescanned by itself.
- A store write that finds no instance row fails (409), so the switch is never cached without being
  stored.
