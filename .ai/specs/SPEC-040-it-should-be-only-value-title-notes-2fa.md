---
yulius-task: 711a1e23-ce06-4ba0-b29d-5a3dc252d32c
status: in-progress
---

# SPEC-040: secrets are a title, a value and notes; reading one needs your two-factor code

Status: in-progress
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
  refused), attempts are limited to 5 a minute per person, and every unlock, failed code and
  setup is audited (`vault.2fa.setup`, `vault.2fa.confirm`, `vault.unlock`, `vault.unlock.failed`)
  without the code or the seed.
- R7. People who manage an agent secret (write access to its sandbox, or instance administrators
  for one used in every sandbox) can reveal its value and notes after a vault unlock. The reveal
  is audited (`vault.agent.reveal`) and served with `Cache-Control: no-store`. Agents still never
  receive a value.
- R8. The seed is sealed in the vault like a secret (envelope encryption bound to the user), and
  no seed or code appears in logs, API errors or the audit trail.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Team secrets hold a title, a value and notes, and older ones fold into that form
- [x] People set up two-factor and unlock the vault with a code
- [x] Every reveal needs a vault unlock, and people can read agent secrets they manage
- [x] Agent secrets drop the per-secret TOTP seed, and the user name serves Basic only
- [x] The Secrets page asks for the code, sets up two-factor and shows title, value and notes
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

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

- Two-factor is **per person, guarding the whole vault** (ADR-0029). "Per wallet" is read as
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
- One pull request (more than 400 lines). The fields, the unlock and the reveal of agent secrets
  are one access-model change of the vault: shipping any part alone would leave the page in a
  state nobody asked for.
