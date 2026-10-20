---
yulius-task: 42df7ea0-21bd-415b-8841-ceb7894a63f2
status: done
---

# SPEC-242: The superadmin password needs 8 characters, and 12 are recommended

Status: done
Date: 2026-10-20
Owner: task #228
Scope authorization: task #228. An early external user wanted the superadmin password `zaq1@WSX`
(8 characters) during first setup, but Yulius required 12. The task asks for a hard minimum of 8
(NIST SP 800-63B), a non-blocking hint when the password is short or weak ("at least 12
characters recommended"), and no blocking of common or keyboard-pattern passwords. Amends SPEC-001
ACC-001. Login rate limiting (ACC-013) and the recovery lockout (ACC-011) are
unchanged.

## Goal

A person setting up an instance, or resetting the superadmin password with the recovery code, can
choose any password of at least 8 characters. A 7-character one is refused with a message that
says the minimum. While the person types a password that is shorter than 12 characters or made of
only a few distinct characters, the form recommends a longer one, but it does not refuse it.

## Context

Confirmed against `dev` at 33410359.

- **Where passwords are set.** Only the superadmin has a password; every other account signs in
  with GitHub (SPEC-001 ACC-001..005). There is no signup, invite or password-change form, and the
  CLI sets no password (`crates/yulius/src` has none). A password is set in exactly two places:
  - setup, `App::complete_setup` (`crates/yulius-app/src/auth.rs`, `POST /api/setup`);
  - recovery, `App::recover_password` (`crates/yulius-app/src/recovery.rs`,
    `POST /api/auth/recover`).

  Both compare `chars().count()` with `MIN_PASSWORD_LEN = 12` and answer 422
  `use at least 12 characters`, each with its own copy of the match and length checks.
- **Web UI.** `pages/auth.js` has its own `MIN_PASSWORD = 12`. Its `passwordChecks` marks a short
  password invalid under its field and keeps the submit button disabled; the field hint says "At
  least 12 characters." for setup and for recovery. It counts `value.length` (UTF-16 units), so a
  password of astral characters (emoji) passes the UI's count but not the server's.
- **Mock.** `mock.js` `POST /api/setup` and `/api/auth/recover` check no length at all, and its
  list of public routes lacks `/api/auth/recovery` and `/api/auth/recover`, so "Forgot password?"
  signed out says "Couldn’t check recovery." in the mock (the server serves both signed out).
- **Docs.** No document, spec or ADR states the number 12. SPEC-001 ACC-001 and ACC-011 say a
  password is chosen without saying how long; `docs/api.md` lists the endpoints without their 422
  for a short password.
- **NIST SP 800-63B** (section 5.1.1.2, memorized secrets) sets 8 characters as the minimum for a
  secret chosen by the subscriber, asks verifiers not to impose composition rules, and counts each
  Unicode code point as one character.

## Requirements

| ID | Requirement |
| --- | --- |
| PWM-001 | A new superadmin password (setup, recovery) needs at least 8 characters, counted as Unicode code points. A shorter one is refused with 422 and the message `the password needs at least 8 characters`. One check in `auth.rs` serves both paths, with the match check before it as today. |
| PWM-002 | No other rule refuses a password: no composition rules, no blocklist of common or keyboard-pattern passwords. |
| PWM-003 | The setup and recovery forms say "At least 8 characters; 12 or more recommended." under the password field. Below 8 characters (counted as code points, as the server does) the field is invalid with "Use at least 8 characters." and the form cannot be sent. |
| PWM-004 | At 8 characters or more the form can be sent, and a non-blocking advice note replaces the field hint while the password is weak: shorter than 12 characters ("Short passwords are easier to guess: at least 12 characters recommended.") or, at 12 or more, made of fewer than 5 distinct characters ("Repeated characters are easy to guess: a phrase of several words is stronger."). The note is not an error: the field is not marked invalid. |
| PWM-005 | The mock refuses a password under 8 characters at setup and recovery with the server's message, and serves the recovery routes signed out as the server does, so the UI is tested against the same rule. |
| PWM-006 | Login rate limiting (ACC-013), the recovery-code lockout (ACC-011) and the password hashing are unchanged. |

## Plan

- [x] The server accepts an 8-character password at setup and recovery and refuses 7 with a clear message
- [x] The setup and recovery forms accept 8 characters and recommend 12 without blocking
- [x] SPEC-001, the API and operations docs, ADR-0183 and the changelog state the new minimum

## Acceptance criteria

- `POST /api/setup` with `zaq1@WSX` creates the superadmin, who then signs in with it.
- `POST /api/setup` and `POST /api/auth/recover` with a 7-character password answer 422 with
  `the password needs at least 8 characters`; a 7-code-point password of astral characters is
  also refused, and 8 of them are accepted.
- `POST /api/auth/recover` with an 8-character password resets the password.
- In the setup form, 7 characters show "Use at least 8 characters." and keep "Create superadmin"
  disabled; `zaq1@WSX` shows the 12-character recommendation, the field is not invalid, and setup
  completes. The recovery form behaves the same way.
- A 12-character password of 4 distinct characters shows the repeated-characters advice; a
  12-character varied one shows no advice.
- Rate limiting and lockout tests pass unchanged.

## Test plan

- Integration (`crates/yulius/tests/http_auth.rs`): setup refuses a 7-character password with the
  message, accepts `zaq1@WSX`, and the password signs in.
- Integration (`crates/yulius/tests/recovery.rs`): recovery refuses 7 characters with the message
  (without spending an attempt) and accepts 8.
- Unit (`auth.rs`): the check counts code points (7 and 8 astral characters) and refuses a mismatch
  first.
- End-to-end (`e2e/tests/password-setup.e2e.ts`, mock): the setup and recovery forms as in the
  acceptance criteria.
- Unchanged: `password_login_rate_limit_and_csrf` and the recovery lockout tests.

## Decisions

- **8 as the hard minimum, 12 as advice** (ADR-0183). NIST SP 800-63B's minimum for a chosen
  secret is 8; Argon2id hashing, the 5-per-minute login limit and the recovery lockout make online
  guessing slow. Keeping 12 blocks people with their own conventions on a self-hosted tool.
- **No blocklist or zxcvbn-style estimator.** The task says not to block common or keyboard-pattern
  passwords, and an estimator library (hundreds of KB of dictionaries) would be a new dependency
  for advice only. The advice uses two transparent rules (length, distinct characters).
- **One server check.** `validate_new_password` in `auth.rs` replaces the two copies so setup and
  recovery cannot drift.
- **Code points in the UI.** The UI counts `[...value].length`, as the server counts `chars()`,
  so the form never sends a password the server refuses for length.

## Evidence

- Red first: `http_auth::setup_accepts_eight_characters_and_refuses_seven_saying_the_minimum` and
  `recovery::recovery_accepts_eight_characters_and_refuses_seven_without_spending_an_attempt`
  failed with `left: "use at least 12 characters"`; `password-setup.e2e.ts` failed on the hint
  ("At least 12 characters.") and on "Couldn’t check recovery." (the mock's public routes).
  All pass after the change; the unit test
  `auth::tests::a_new_password_needs_eight_code_points_after_matching` passes.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^http_auth::/) |
  test(/^recovery::/) | test(/^accounts::/))'`: 27 passed, including the unchanged
  `password_login_rate_limit_and_csrf` and the recovery lockout test (PWM-006).
- `scripts/dev ci` (at d14e307e; later commits touch only the spec, docs and the e2e test): exit
  0; nextest 2272 passed, 17 skipped; lint, deps, docs and macOS stages green.
- `scripts/dev security` (at 6a72ded9): exit 0, "no open critical or high finding, and no open
  medium one in the change". One medium finding outside the change (`auth.rs:380`, revoked API
  tokens in `authenticate_bearer`): code this branch did not touch; Yulius CI files it as an issue.
- `cd e2e && npx e2e run` (full suite): 635 passed, 1 skipped, 1 failed:
  `secrets.e2e.ts` "a failed copy shows no check…" (axe target-size on a Close button), the known
  flake tracked by open issue #640 (about 1 run in 5, unrelated to the password forms); the file
  rerun alone passed 20/20. `password-setup.e2e.ts` 3/3, including an axe check with the advice
  shown.
- Independent review (fresh session, read-only, om-code-review): APPROVE at d14e307e, no blocker
  or major; its minors and nits on own code fixed in 6a72ded9 (spec amends ACC-001 only,
  operations step wording and wrap, axe check, recovery not-invalid assertion). Not changed: the
  notes inside the wrapping `<label>` join the input's name (pre-existing pattern of every field
  error; WCAG 3.3.1 applies to errors and is met).
- Not tested: a real browser against `yulius serve` (the e2e suite runs the UI on the mock; the
  server rule is covered by the integration tests).
