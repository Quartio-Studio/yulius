# 0165. The Superagent bypasses approvals on contained servers, as tasks do

## Status

Accepted. Specified in [SPEC-208](../../.ai/specs/SPEC-208-tasks-and-the-superagent-should-run-in-d.md);
task #195. Amends [0020](0020-cockpit-as-a-plain-shell-session.md) ("no … approval bypass") and
SPEC-021 CKP-002.

## Context

Task runs of a contained server start with `--dangerously-skip-permissions` (SPEC-002 ADR-22). The
Superagent was kept "plain": no bypass, the person switching modes with Shift+Tab. Claude Code
2.1.292 now starts such a session in auto mode, whose classifier blocks commands the person asked
for and whose onboarding dialog ("Teach auto mode about your environment?") interrupts the panel.
The person asked that tasks and the Superagent run with the approval bypass instead.

A contained Superagent runs in its sandbox's bubblewrap as uid 0, where Claude Code accepts the
bypass only with `IS_SANDBOX=1`.

## Decision

- On a contained server the Superagent starts with `--dangerously-skip-permissions` and
  `IS_SANDBOX=1`, and its person's `.claude.json` records `bypassPermissionsModeAccepted`.
  `plan_cockpit` takes `bypass_permissions`, set from `config.contained`, as a task's launch is.
- On an uncontained server neither the Superagent nor a task bypasses (unchanged).
- The read-only `--allowedTools` list stays, for a session switched back to default mode.
- `COCKPIT_LAUNCH_REVISION` goes to 2, so running Superagents restart into the bypass once idle
  (SPEC-186 SAV-006).

## Alternatives

- **Keep the plain session and let people press Shift+Tab**: what the request is against; the mode
  resets at every start and auto mode keeps blocking.
- **`--permission-mode bypassPermissions` or `defaultMode` in settings**: same effect, but a second
  spelling of what task runs already do with the flag; the flag also needs the same
  `IS_SANDBOX` and acceptance handling.
- **Bypass on uncontained servers too**: the session would act on the host as the service user
  with no boundary.

## Consequences

- The Superagent no longer asks before running tools, including the Yulius management tools that
  create, stop or delete tasks; containment and the tools' own server-side checks (owner's access,
  audit) are its boundary, as for tasks.
- Claude Code's own checks that ask even under the bypass (SPEC-192) still ask; the person is
  there to answer.
- Running Superagents restart once after the update; their conversations continue.
