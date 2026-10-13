---
yulius-task: 9d43863b-8bb0-4bd7-b920-4549a73d5c16
status: in-progress
---

# SPEC-061: a two-factor code confirms only the seed it was checked against; administrators reset a lost two-factor

Status: in progress
Date: 2026-10-13
Scope authorization: task #52 (part of task #47, "fix all open actionable GitHub issues"): fix
GitHub issues #295 and #291 following their acceptance criteria. Extends
[SPEC-040](SPEC-040-it-should-be-only-value-title-notes-2fa.md) and
[ADR-0034](../../docs/decisions/0034-personal-two-factor-guards-the-vault.md).

## Goal

1. (#295) A two-factor code confirms or unlocks only the seed it was checked against. A setup
   of a new pending seed that lands while a code from the old one is being checked can no longer
   be confirmed by that code.
2. (#291) A person who lost their authenticator can read secrets again: an instance
   administrator or the superadmin resets that person's two-factor from the Users page, after
   unlocking the vault with their own code. The person then sets two-factor up again on their
   next reveal.

## Context

- `App::unlock_vault` (`crates/yulius-app/src/vault/two_factor.rs`) reads the person's
  `vault_two_factor` row, checks the code against its seed, then calls
  `vault_accept_two_factor(user_id, counter, at)`, which updates by `user_id` alone. A pending
  seed is replaced by `vault_put_two_factor` (a new setup), so a setup between the read and the
  update lets a code from seed A confirm seed B (#295). Only the person's own concurrent
  requests can do this, but the result is a confirmed seed whose key they may never have added.
- A confirmed seed cannot be replaced from the account (SPEC-040 R5), so a lost authenticator
  means no reveal ever again (#291). Nobody can clear another person's `vault_two_factor` row.
- Unlocks are stored per browser session (`vault_unlock.session_hash`, which is
  `web_sessions.id_hash`); a person's sessions are `web_sessions` rows with their `user_id`.
- The Users page (`/admin/users`, `GET /api/admin/users`) is the people page and is
  superadmin-only today. Instance administrators manage the subscriptions, MCP servers and
  skills for every sandbox, and the activity trail of the vault is shown to every member.
- Reveals already have the gate this needs: `fresh_or_status` refuses API tokens (403) and
  answers `403 reauth_required` with the reauth status when the session has no unlock in the
  last 10 minutes, and the web UI's `withReauth` turns that into the code dialog and a retry.

## Requirements

- R1 (#295). `vault_accept_two_factor` takes the sealed seed (its ciphertext) that the code was
  checked against and updates only when the stored row still holds that ciphertext. Every seal
  uses a fresh data key and nonce, so a new setup always changes the ciphertext.
- R2 (#295). When the seed changed meanwhile, the code is refused (403, audited as
  `vault.unlock.failed`) with the message "two-factor was set up again meanwhile: enter a code
  for the new key". It does not count as a wrong code, and the new seed stays pending.
- R3 (#291). `DELETE /api/users/{id}/two-factor` removes the person's two-factor seed and every
  vault unlock of their browser sessions. It answers 204, also when the person had no
  two-factor (nothing is removed and nothing is audited then).
- R4 (#291). Who: instance administrators and the superadmin. Members get 403. API tokens get
  403 (the reset is done from a browser session). The administrator's own session needs a vault
  unlock in the last 10 minutes; otherwise `403 reauth_required` with the reauth status, exactly
  like a reveal. An unknown user is 404. Resetting your own two-factor is refused (403): a
  confirmed seed is still never replaced from the account itself.
- R5 (#291). The reset is audited as `vault.2fa.reset` by the administrator, with the person's
  login (or `superadmin`) as the detail and no seed or code. It is in the vault activity trail.
- R6 (#291). After a reset the person's two-factor is `missing`: their next reveal leads to the
  setup, which still needs a recent sign-in or password confirmation, and a code from the old
  seed is refused.
- R7 (#291). `GET /api/admin/users` is readable by instance administrators too, and each
  `AdminUser` carries `two_factor`: `missing`, `pending` or `on`. Admitting users, disabling them
  and changing the administrator flag stay superadmin-only.
- R8 (#291). The Users page is shown to instance administrators. It has a Two-factor column with
  the person's state and, unless it is off, a "Reset" button that asks for confirmation
  ("Reset two-factor of @login?") and then for the administrator's own code when the vault is
  locked. The recovery code card, "Admit user" and the toggles stay superadmin-only.

## Plan

- [x] Write and validate the spec, the ADR and the plan
- [ ] A two-factor code confirms only the seed it was checked against (#295)
- [ ] Administrators reset a person's two-factor over the API, audited (#291)
- [ ] Instance administrators see the people and their two-factor state (#291)
- [ ] The Users page offers the reset with a confirmation (#291)
- [ ] The API reference, architecture and changes log describe the reset
- [ ] Independent review approves the head, findings fixed test-first
- [ ] Every gate green and merged into dev

## Acceptance criteria

- AC1 (#295). Store: an accept with the ciphertext of a replaced pending seed is refused and
  leaves the new seed pending; an accept with the current ciphertext confirms it.
- AC2 (#295). Integration: the person sets up seed A; while a code from A is being checked, a
  setup of seed B lands (the interleaving is forced between the check and the accept); the code
  is refused, two-factor stays `pending`, and a code from B then confirms B.
- AC3 (#291). Integration: an instance administrator and the superadmin, each after their own
  unlock, reset a member's confirmed two-factor (204). The member's reauth status is `missing`
  and not fresh, a code from the old seed is refused (422, set up first), and setting up again
  works. The activity trail has `vault.2fa.reset` naming the member and holds no seed.
- AC4 (#291). Integration: a member gets 403; an API token of an administrator gets 403; an
  administrator without a fresh unlock gets `403 reauth_required`; an unknown user is 404; the
  administrator's own id is 403. None of these change the member's two-factor.
- AC5 (#291). `GET /api/admin/users` as an instance administrator lists users with
  `two_factor`; `POST`/`PATCH` stay 403 for them.
- AC6 (#291). E2E (mock): the Users page shows the Two-factor column; "Reset" asks for
  confirmation, then for the code, and the person's two-factor shows Off; no axe violations.

## Test plan

- Store unit test (`yulius-store` `vault::tests`): AC1; the reset removes the seed and the
  person's unlocks, keeps other people's, and reports whether a seed was removed.
- Integration (`crates/yulius/tests/secrets.rs`): AC2 with a crypto decorator in the test
  harness that runs a second setup through the API exactly when the unlock opens the seed (a
  multi-threaded test runtime, no sleeps); AC3, AC4 and AC5 through the public router.
- E2E (`e2e/tests/secrets.e2e.ts`, mock server): AC6.

## Decisions

- Match the seed by its ciphertext rather than `created_at`: two setups in the same millisecond
  would share `created_at`, while every seal draws a new data key and nonce.
- A changed seed is not a wrong code: it does not count toward the lockout, since only the
  person's own concurrent setup can cause it.
- The reset lives at `DELETE /api/users/{id}/two-factor` (the issue's path) next to the other
  vault routes, and it reuses the reveal gate (`fresh_or_status`), so the web UI's existing
  unlock-and-retry flow works unchanged.
- Self-reset is refused. Otherwise a stolen session that saw one code could enroll its own
  authenticator, which SPEC-040 R5 forbids. A superadmin who lost their authenticator needs an
  instance administrator; one without any is a follow-up noted in the summary.
- The reset is idempotent (204 when nothing was set up), so a double click or two administrators
  at once give no error; only a real removal is audited.
- Instance administrators read the user list (ADR-0040): the issue asks for them to reset from
  the people page, and the list holds only logins, names and flags they already act on. Changing
  users stays superadmin-only.
- One pull request for both issues: Yulius delivers one pull request per task, and both touch
  the same two-factor row and tests.

## Evidence

To be filled in when the work is done.
