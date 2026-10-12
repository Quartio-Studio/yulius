# 0029. A personal two-factor code guards the vault; a secret is a title, a value and notes

## Status

Accepted. Changes the reveal rule of [ADR-0014](0014-secrets-vault.md) (SPEC-002 ADR-20);
specified in [SPEC-040](../../.ai/specs/SPEC-040-it-should-be-only-value-title-notes-2fa.md).

## Context

Team secrets had an address, a user name, a password, notes and a TOTP seed, and Yulius showed a
shared mailbox's current code. Revealing one needed only a "fresh re-authentication": a sign-in or
the superadmin's password in the last 10 minutes. Neither is a second factor: whoever held a
session cookie read every team secret. Agent secret values were write-only for everyone. Task
#39 asked for "only value title notes", for two-factor "per wallet" and mandatory, the same for
agent secrets ("without 2FA for agents"), and for people to be able to read agent secrets behind
two-factor.

## Decision

- A secret is a **title, a value and notes**, for people and for agents. For agents, the title is
  the NAME and the notes are the description agents read. The hosts and injection stay, because
  they let an agent use a value without seeing it, and a user name stays only for Basic
  credentials. Older payloads fold in when read: the password becomes the value, and the
  address, user name and seed lead the notes.
- Two-factor guards the **vault**, and each person has their own: an RFC 6238 seed (160 random
  bits, sealed like a secret and bound to the user) set up once and confirmed by a first code.
  Every reveal or copy, of a team secret or of an agent secret, needs a code entered in that
  browser session in the last 10 minutes. Codes are checked in constant time, within one time
  step either way. Each works once (`last_counter`), attempts are limited to 5 a minute, and
  everything is audited. Ten wrong codes in a row lock codes until the person confirms identity
  again (password or a new GitHub sign-in), so a stolen session cannot keep guessing.
- Setting up needs a sign-in or password confirmation in the last 10 minutes, and a confirmed
  seed cannot be replaced from the account. A stolen cookie alone cannot enroll its own
  authenticator.
- The people who manage an agent secret (write access to its sandbox, instance administrators
  for one used in every sandbox) can read it back. Agents never do.

## Alternatives

- **One seed per wallet, shared by the team.** This is the literal reading of "per wallet".
  Everyone who once saw the seed passes as anyone with a session, the audit trail could not say
  who unlocked, and one leaver forces everyone to re-enroll. Rejected.
- **Keep the per-secret seed and add two-factor on top.** The request asked for title, value and
  notes only. Rejected.
- **Passkeys (WebAuthn).** Stronger, but they need a relying-party setup, a JavaScript ceremony
  and hardware or platform support on every device. A good follow-up, not the smallest change.
- **Let read-only members read agent secrets.** Anyone who can replace a value can capture it
  anyway, so managers reading it adds no power. Readers reading it would. Rejected.

## Consequences

- People enroll once from the Secrets page. A lost authenticator locks that person out of
  reveals until an administrator reset exists (follow-up issue).
- A sign-in no longer reveals anything by itself. The superadmin password and GitHub sign-in
  remain the identity confirmation that allows setup.
- Agent website logins put the user name in the notes. Older secrets keep their stored user
  name and seed for `browser_fill_secret`.
