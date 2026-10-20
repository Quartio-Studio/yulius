# 0183. The superadmin password needs 8 characters, and 12 are advised

## Status

Accepted. Specified in [SPEC-242](../../.ai/specs/SPEC-242-password-minimum-8-chars-with-strength-w.md);
task #228. Amends SPEC-001 ACC-001.

## Context

- Only the superadmin has a password; everyone else signs in with GitHub. A password is set at
  setup and at recovery with the recovery code. Both required 12 characters
  (`MIN_PASSWORD_LEN`), and the web UI did too.
- An early external user wanted `zaq1@WSX` (8 characters) during first setup and could not use it.
  On a self-hosted tool, people bring their own password conventions.
- NIST SP 800-63B (5.1.1.2) sets 8 characters as the minimum for a secret the subscriber chooses,
  asks verifiers not to impose composition rules, and counts each Unicode code point as one
  character.
- Online guessing is already bounded: password sign-in allows 5 attempts per client per minute
  (ACC-013), the hash is Argon2id, and three wrong recovery codes lock recovery (ACC-011).

## Decision

- A new superadmin password needs at least 8 code points (`MIN_PASSWORD_LEN = 8`). One function,
  `validate_new_password` in `auth.rs`, checks setup and recovery: the repeat must match, then the
  length. A shorter password is refused with 422 `the password needs at least 8 characters`.
- Nothing else refuses a password: no composition rules, no list of common or keyboard-pattern
  passwords.
- The setup and recovery forms say "At least 8 characters; 12 or more recommended." While an
  accepted password is shorter than 12 characters, or has 12 or more made of fewer than 5 distinct
  characters, advice replaces that hint. The advice does not mark the field invalid and does not
  block the form. The forms count code points, as the server does.
- Login rate limiting, the recovery lockout and the hashing are unchanged.

## Alternatives

- **Keep 12.** Rejected: it blocks people with their own conventions for no gain the rate limit
  and Argon2id do not already give.
- **A strength estimator (zxcvbn) that blocks a low score.** Rejected: the task forbids blocking
  common or keyboard-pattern passwords, and the library ships hundreds of kilobytes of
  dictionaries.
- **The same estimator for advice only.** Rejected for now: a new dependency for advice. Two plain
  rules (length, distinct characters) are transparent and testable; an estimator can replace them
  later without changing the contract.
- **A configurable minimum.** Rejected: a setting for one password, and an operator who wants
  longer can choose longer.

## Consequences

- An 8-character password works at setup and recovery; existing passwords are unaffected (the
  minimum applies only when one is set).
- An 8-to-11-character password is easier to guess offline than a 12-character one if the
  database leaks. The advice and Argon2id soften this; the choice is the operator's.
- The web UI's `MIN_PASSWORD` and the mock's check must stay equal to `MIN_PASSWORD_LEN`; the
  e2e test `password-setup.e2e.ts` and the integration tests pin all three at 8.
