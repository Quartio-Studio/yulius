---
yulius-task: 711a1e23-ce06-4ba0-b29d-5a3dc252d32c
status: done
---

# SPEC-040: secrets are a title, a value and notes; reading one needs your two-factor code

Status: done
Date: 2026-10-12
Scope authorization: user request (task #39, F 2026-10-12): "It should be only value title notes.
2FA should be per wallet and should be mandatory. The same for the agents but without 2FA for
agents ofc. But for human to read them, 2FA should also be mandatory (but they should be able to
read actually)", with a screenshot of the "Add team secret" dialog (Title, Address, User name,
Password, Notes, Two-factor seed (TOTP)).

## Goal

A secret in the vault (the "wallet") is a **title, a value and notes**, for people and for agents.
Two-factor authentication guards the vault itself instead of being a field of each secret:
every person reveals or copies a secret only after entering a current code from their own
authenticator app, and setting one up is mandatory before the first reveal. Agents keep using
agent secrets without any of this (through the egress proxy and browser filling, never seeing
them), and people can now read an agent secret's value too, behind the same two-factor code.

## Context

- The vault is SPEC-001 VLT-001..009 and ADR-0014 (`crates/yulius-app/src/vault/`, routes in
  `crates/yulius-server/src/routes/secrets.rs`, page `web/assets/pages/secrets.js`). It has two
  sections, shown as two tabs: **team secrets** (title, URL, user name, password, notes, TOTP
  seed; every member) and **agent secrets** (NAME, description, hosts, injection, user name,
  value, TOTP seed; write-only for everyone).
- Revealing a team secret needs a "fresh re-authentication" (VLT-002): the browser session signed
  in or confirmed the superadmin password in the last 10 minutes. A GitHub member confirms by
  signing in again. Neither is a second factor: whoever holds the session cookie, or a GitHub
  session, reads every team secret.
- A per-secret TOTP seed (VLT-003) made Yulius show a mailbox's current code. That is what the
  screenshot's last field is. The user wants two-factor on the wallet instead, and mandatory.
- Agent secret values are write-only (VLT-006): nobody, not even the person who set one, can
  read it back. The user wants people to be able to read them, behind two-factor.
- Payloads are sealed JSON (`TeamPayload`, `AgentPayload`). Existing rows must stay readable,
  and no value may be lost.

## Requirements

- R1. A team secret has a title, a value and notes. The API accepts `{title, value?, notes?}`;
  `url`, `username`, `password` and `totp` are refused (422, unknown field). The list shows the
  title, whether a value and notes are stored, and the value's age. Reveal returns
  `{value, notes}`; there is no per-secret code endpoint.
- R2. Existing team secrets keep every stored value. A stored password becomes the value. The
  address, user name and TOTP seed are added as lines in front of the notes (`Address: …`,
  `User name: …`, `Two-factor seed: …`) when the secret is next read, and the new form is sealed
  on its next change.
- R3. An agent secret is a NAME (its title), a value and notes (the description agents read),
  plus what makes it usable without being seen: its hosts, how it is added to requests, and a
  user name only for `Authorization: Basic`. The per-secret TOTP seed is gone from the API
  (`totp` refused). Secrets that already have one keep working for agents.
- R4. Two-factor is per person and mandatory. Revealing or copying a team secret, and revealing
  an agent secret, needs a **vault unlock**: a correct current code (RFC 6238, 6 digits, 30 s,
  ±1 step) from the person's own authenticator, entered in this browser session in the last 10
  minutes. Signing in and the password or GitHub confirmation no longer unlock the vault. API
  tokens never reveal.
- R5. Setting up two-factor: a person without it gets a new random seed (160 bits) as base32
  and an `otpauth://` link, and confirms it with a first code, which also unlocks the vault.
  Starting the setup needs the existing identity confirmation (sign-in or password in the last
  10 minutes), so a stolen session cookie alone cannot enroll its own authenticator. Once
  confirmed, the seed is never shown again and cannot be replaced from the account.
- R6. Codes are checked in constant time. A code is accepted once (a replayed or older code is
  refused), attempts (and setups) are limited to 5 a minute per person, and 10 wrong codes in a row
  lock codes (`two_factor = "locked"`) until the person confirms identity again (password or a new
  GitHub sign-in), so a stolen session cannot keep guessing. Every unlock, failed code, lock and
  setup is audited (`vault.2fa.setup`, `vault.2fa.confirm`, `vault.2fa.locked`, `vault.unlock`,
  `vault.unlock.failed`) without the code or the seed.
- R7. People who manage an agent secret (write access to its sandbox, or instance administrators
  for one used in every sandbox) can reveal its value and notes after a vault unlock. The reveal
  is audited (`vault.agent.reveal`) and served with `Cache-Control: no-store`; a stored user
  name (Basic credentials, older website logins) and an older secret's TOTP seed lead the notes.
  Agents still never receive a value.
- R8. The seed is sealed in the vault like a secret (envelope encryption bound to the user), and
  no seed or code appears in logs, API errors or the audit trail.
- R9. Editing a team secret's notes starts from their current text: stored notes are read-only in
  the edit dialog until "Edit current notes" loads them (after a code), so the address, user name
  and seed folded into older notes are never replaced blindly.
  A pending setup can be finished with a code from the key added earlier, or restarted.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Team secrets hold a title, a value and notes, and older ones fold into that form
- [x] People set up two-factor and unlock the vault with a code
- [x] Every reveal needs a vault unlock, and people can read agent secrets they manage
- [x] Agent secrets drop the per-secret TOTP seed, and the user name serves Basic only
- [x] The Secrets page asks for the code, sets up two-factor and shows title, value and notes
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1. "Add team secret" has exactly Title, Value and Notes. A created secret lists with its
  title and no value. `POST /api/team-secrets` with `password` or `totp` gives 422.
- AC2. A team secret stored in the old form with URL, user name, password, notes and seed reveals
  `value` = the password and notes that contain the address, user name and seed lines followed by
  the old notes.
- AC3. Right after signing in, revealing gives 403 `reauth_required` with
  `reauth.two_factor = "missing"`. Setup works, the first correct code confirms it, and the
  reveal then succeeds. A wrong code gives 403 and is audited. A reused code gives 403. Eleven
  minutes later the reveal needs a new code. An API token is refused.
- AC4. Starting setup more than 10 minutes after sign-in, without a password confirmation, is
  refused. Setting up again after it was confirmed is refused (409).
- AC5. A sandbox writer reveals an agent secret's value after unlocking. A reader gets 403, and a
  member who cannot see the sandbox gets 404. The value never appears in the list or the MCP
  `secrets_list` output.
- AC6. `POST …/agent-secrets` with `totp` gives 422. The form shows Name, Value, Notes, Hosts and
  "Added to requests as", and asks for a user name only for Basic.
- AC7. The database file holds no seed or value in clear.

## Test plan

- Domain unit tests: base32 encoding round-trips through `base32_decode`; the `otpauth://` link
  parses back to the same seed; folding an old team payload keeps every field (AC2).
- Store unit tests: a two-factor row's accept moves `last_counter` forward only, and an unlock is
  stored per session.
- Integration (`crates/yulius/tests/secrets.rs`, the public API over the router with the test
  clock): AC1–AC5 and AC7, including wrong, replayed and late codes, the rate limit, API tokens,
  readers, outsiders and the audit trail, with no value in any non-reveal body.
- End-to-end (`e2e/tests/secrets.e2e.ts`, mock server): the add dialog's fields, the two-factor
  setup and code dialogs, revealing a team secret and an agent secret, no value in the DOM before
  a reveal, and no axe violations.

## Decisions

- Two-factor is **per person, guarding the whole vault** (ADR-0042). "Per wallet" is read as
  "on the wallet, not on each secret". A single seed shared by the team would make the audit
  trail meaningless (anyone who once saw it passes as anyone holding a session) and could never be
  revoked for one person. Each person enrolls their own authenticator, so one unlock opens both
  sections for 10 minutes in that browser session.
- Enrollment needs the existing identity confirmation, and a confirmed seed cannot be replaced
  from the account. Otherwise the second factor would only be as strong as the first. Recovery
  from a lost authenticator (an administrator reset) is a follow-up issue, not this change.
- Old payloads fold into the new form when read instead of through a migration. Payloads are
  sealed, so SQL cannot rewrite them, and folding keeps every value.
- Agent secrets keep hosts and injection. These are what lets an agent use a secret without
  seeing it, and they stop it from being sent anywhere else. The user name stays only as the
  user part of Basic credentials. The website-login user name and code are dropped as fields, as
  requested ("without 2FA for agents"); stored ones keep working.
- Readers of an agent secret are the people who can already change it. Anyone who can replace a
  value could capture it anyway. Read-only members still see only names.
- Lockout after 10 wrong codes in a row (review finding): with 5 attempts a minute alone, a stolen
  30-day session could guess a code with about even odds. A confirmed identity, not time, lifts it,
  because only the person can confirm. Expired `vault_unlock` rows are not pruned, like
  `vault_reauth` rows already; both are a few bytes per browser session.
- The page's locked and pending flows are covered by the API integration tests, not by the e2e
  mock (which starts signed in with no seed); the mock flow covers setup, unlock and reveals.
- An older agent secret's TOTP seed cannot be cleared on its own (the API no longer takes `totp`);
  replacing the secret removes it. Agents keep using it for website codes until then.
- Not in this change: a QR code in the setup dialog (the key and the `otpauth://` link work in
  every authenticator) and multi-line values (put them in the notes).
- A correct code clears the wrong-code count (NIST counts failures "in a row"): between two of
  the owner's unlocks a stolen session gets at most 9 guesses, each about 3 in 10^6.
- Not in this change: clearing a stored value or notes from the page (the API takes `""`), and
  zeroizing the request and response strings of the code and the new seed (served `no-store`,
  like reveals).
- Review rounds: three fresh-session rounds ran. The third found no code defect; it asked for the
  rebase over #281 (ADR renumbered, then 0034, now 0042 after #317) and minor fixes, which were made
  test-first. A last confirmation review covers exactly that delta on the final head, because
  merging needs an approval of the exact head.
- One pull request (more than 400 lines). The fields, the unlock and the reveal of agent secrets
  are one access-model change of the vault: shipping any part alone would leave the page in a
  state nobody asked for.

## Evidence

- `scripts/dev ci` (repository check, fmt, clippy, cargo-deny, machete, nextest, docs): exit 0,
  1204 tests passed, 16 skipped (2026-10-13, on the final head rebased over #281).
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets::/)'`: 13 passed,
  among them `team_secrets_are_shared_and_reveals_need_a_two_factor_code` (AC1, AC3),
  `older_team_secrets_fold_into_a_value_and_notes` (AC2, and a later edit keeps the notes),
  `two_factor_is_set_up_once_and_unlocks_the_vault` (AC3: wrong, reused and late codes, the rate
  limit, API tokens, `no-store`, the audit trail),
  `setting_up_two_factor_needs_a_recent_sign_in` (AC4),
  `ten_wrong_codes_lock_two_factor_until_identity_is_confirmed` (R6 lockout),
  `agent_secrets_follow_sandbox_access_and_reveal_only_to_managers` (AC5, AC6),
  `older_agent_secrets_reveal_their_user_name_and_seed` (R7) and `nothing_is_stored_in_clear`
  (AC7: no value or two-factor seed in the database file, its WAL or a backup).
- Store unit tests (`yulius-store` `vault::tests`): accept, replay, pending replacement, the
  confirmed seed kept, wrong-code counts and clears. Domain unit tests: base32 round trips and
  `otpauth://` links parse back.
- `cd e2e && npx e2e run` on the final head: 158 passed, 1 skipped, 1 failed:
  `task-done.e2e.ts` "a finished task whose pull request is tested…", unrelated to this change. It
  passed 3 of 3 alone and is filed as flaky (#294). Before the rebase the whole suite passed (159); `tests/secrets.e2e.ts` 6 passed
  after the last change (add dialog fields, setup with a wrong then right code, reveals of team
  and agent secrets, editing current notes, axe on the page and the setup dialog).
- Visual check in the sandbox browser against the mock: the "Set up two-factor" dialog (key,
  Copy, authenticator link, code field) and "Add team secret" with Title, Value and Notes only.
- Independent reviews in fresh sessions: round 1 requested changes (a stolen session could keep
  guessing codes; editing older notes could drop folded fields; minor items), all fixed test-first;
  round 2 approved `8017484` with minor items, fixed test-first (the locked message, older agent
  secrets' user name and seed readable); round 3 found no code defect and asked for the rebase over
  #281 (the decision renumbered past dev's; it was ADR-0034 since #287 and #306 took 0030 and 0033,
  and is ADR-0042 since #317) and minor fixes (stored notes read-only until loaded, agent tool
  texts, SPEC-002 statements), made test-first; a confirmation review approves the final head.
- Not tested here: a real authenticator app scanning the `otpauth://` link (the link format is
  unit-tested to parse back to the same seed); the deployed instance.
- Follow-up: #291 (an administrator resets a lost authenticator).

