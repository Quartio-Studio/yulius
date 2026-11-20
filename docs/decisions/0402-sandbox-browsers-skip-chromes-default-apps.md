# 0402. Sandbox browsers skip Chrome's default apps

## Status

Accepted for [SPEC-567](../../.ai/specs/SPEC-567-prevent-chrome-s-default-app-policy-dial.md).

## Context

Google Chrome installed Docs Offline in a fresh sandbox profile. Applying the host's wildcard extension blocklist while that profile was open displayed a native administrator dialog with an OK button. A viewer whose input was locked could not dismiss it. [ADR-0318](0318-viewers-input-reaches-a-sandbox-browser-only-under-the-hosts-managed-policy.md)

## Decision

Pass `--disable-default-apps` on sandbox browser launches. On virtual displays also pass `--disable-extensions` so an ordinary extension already present in an older profile stays unloaded when Chrome reads the host policy.

## Alternatives

With only `--disable-default-apps`, Chrome 157 still loaded Docs Offline in an older profile and displayed the dialog on policy reload. Deleting the profile would lose sign-ins. Allowlisting Docs Offline would weaken the wildcard blocklist.

## Consequences

The change takes effect on the browser's next launch. The virtual display keeps Chrome's built-in PDF viewer. Developers running headless browsers can still supply their own extensions. Host setup continues to install the policy specified in ADR-0318.
