# 0040. Administrators reset a lost two-factor; a code confirms only the seed it was checked against

## Status

Accepted. Extends [ADR-0034](0034-personal-two-factor-guards-the-vault.md); specified in
[SPEC-061](../../.ai/specs/SPEC-061-fix-295-291-vault-two-factor-accept-matc.md) (issues #291 and
#295).

## Context

ADR-0034 makes a personal two-factor code mandatory for every reveal, and a confirmed seed cannot
be replaced from the account, so a stolen session cannot enroll its own authenticator. A person
who loses their authenticator can then never read a secret again, and nobody could clear their
seed (#291). Separately, accepting a code updated the person's two-factor row by user alone, so a
new pending seed set up while a code of the old one was being checked could be confirmed by that
code (#295).

## Decision

- Accepting a code names the sealed seed it was checked against (its ciphertext, which differs on
  every seal) and updates only that row. A seed replaced meanwhile refuses the code; that is not
  counted as a wrong code.
- Instance administrators and the superadmin reset another person's two-factor with
  `DELETE /api/users/{id}/two-factor`. It removes the seed and every vault unlock of that
  person's browser sessions, and is audited as `vault.2fa.reset` without any seed or code.
- The reset uses the reveal gate: a browser session (never an API token) with the
  administrator's own two-factor unlock in the last 10 minutes. Nobody resets their own
  two-factor: a confirmed seed is still never replaced from the account itself.
- The person sets two-factor up again on their next reveal, which still needs a recent sign-in or
  password confirmation, so a reset alone gives no one else their vault.
- Instance administrators may read the user list (`GET /api/admin/users`, now with each person's
  two-factor state) so they can reset from the Users page. Admitting, disabling and promoting
  users stay superadmin-only.

## Alternatives

- **Match the seed by `created_at`.** Two setups in the same millisecond share it. Rejected for
  the ciphertext.
- **Superadmin-only reset.** Simpler, but the issue asks for instance administrators too.
- **Self-service recovery codes.** Another secret to keep, and a stolen session that reads them
  enrolls its own authenticator. A good follow-up next to passkeys, not this change.
- **A separate two-factor list for administrators on the Secrets page.** A second list of the same
  people; the Users page is where people are managed.

## Consequences

- A lost authenticator costs an administrator's reset and a new setup, not the person's access.
- An administrator with a stolen session and their own fresh code can force someone to set up
  again, but cannot read that person's secrets or enroll for them; every reset is in the activity
  trail every member sees.
- The user list does not include the superadmin, so nobody can reset the superadmin's lost
  authenticator yet: the API allows it, but an instance administrator never sees the
  superadmin's id. That recovery (for example next to the superadmin recovery code) is a
  follow-up.
- An unlock counts only while the person has a confirmed seed, so an unlock stored by a request
  that raced a reset opens nothing.
