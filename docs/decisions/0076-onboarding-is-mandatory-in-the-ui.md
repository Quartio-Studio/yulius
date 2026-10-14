# 0076. Onboarding is mandatory in the UI; the skip endpoint is an operator's recovery path

## Status

Accepted. Specified in [SPEC-096](../../.ai/specs/SPEC-096-onboarding-should-be-mandatory-and-shoul.md).
It narrows [ADR-0038](0038-onboarding-starts-in-the-browser.md): onboarding still starts only in the browser.

## Context

A sandbox that needs onboarding showed a banner with **Start onboarding** and **Skip for now**
above the whole new-task composer. People started tasks or skipped onboarding before the sandbox
had its environment and the Yulius documents, so those tasks met missing toolchains and docs. The
request: onboarding is mandatory, and the page shows no task input until it has finished, only a
centred message saying what will happen and its button.

## Decision

- While a sandbox is `pending` or `preparing`, its new-task page shows a centred onboarding region
  instead of the composer. The region says what onboarding will do and offers **Start onboarding**
  (`pending`, write access) or **Follow onboarding** (`preparing`). The composer is hidden, not
  destroyed, and returns by itself when the sandbox is ready.
- The web UI offers no way to skip onboarding.
- `POST /api/sandboxes/{sid}/onboarding/skip` stays, admin only, as an operator's recovery path for
  an onboarding that cannot finish.
- The server keeps accepting tasks during onboarding and holds them until it lands (U-08).

## Alternatives

- **Remove the skip endpoint.** This would enforce the rule everywhere. But an onboarding that
  fails beyond repair would then leave the sandbox blocked for good, with only database edits as a
  way out.
- **Refuse new tasks on the server while onboarding.** Automations, the CLI and agents create
  tasks that already wait correctly; refusing them would turn a scheduling wait into errors.
- **Keep the composer, disable it.** A disabled task box still invites input, and it contradicts
  "show no task input".

## Consequences

- People can no longer start or skip work from the page before onboarding. Tasks that arrive
  through the API still queue behind it.
- An admin who must unblock a broken sandbox calls the endpoint (for example with `curl` and a
  token). `docs/api.md` names this as recovery.
- The superadmin's recovery-code reminder on this page waits until the sandbox is ready.
