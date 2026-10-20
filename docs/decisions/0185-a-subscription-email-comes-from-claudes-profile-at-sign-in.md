# 0185. A subscription's email comes from Claude's profile at sign-in

## Status

Accepted. Specified in [SPEC-244](../../.ai/specs/SPEC-244-rename-claude-sessions-and-show-account.md);
task #230 (feedback from an early external user: "add option to rename claude sessions + ideally
see the session email"). Amends SPEC-001 ROT-007 and ROT-009.

## Context

- People connect several Claude subscriptions and could not tell them apart. The name typed when
  adding one could not be changed in the UI, and nothing showed which Claude account was behind
  it.
- Yulius signs a subscription in with `claude setup-token` and keeps only the printed token
  (SPEC-002 ADR-05). Claude Code 2.1.292 receives the account's email in its token exchange
  (`account.email_address`), but in setup-token mode it neither prints nor stores it.
- Right after the exchange, Claude Code asks `GET https://api.anthropic.com/api/oauth/profile`
  with the new token. When the token may read the profile, the answer's `account.email` is the
  address. A setup-token sign-in has only `user:inference`, and Claude's usage endpoint refuses it
  for lack of `user:profile` (HTTP 403). The profile endpoint may refuse it the same way, and
  nobody can check that without a real sign-in.

## Decision

- An account stores `email` (nullable) and `email_checked_at` (schema version 38).
- When a sign-in stores a credential, the app asks the usage adapter for the account's email
  (`UsageProbe::account_email`, the profile request above). The answer is handled as follows:
  - An address is stored, and so is the time of the answer.
  - A refusal (403 or 401) stores no email and marks the time, so nobody asks again.
  - A temporary failure stores no email and leaves the time unset.

  An older email is always dropped first: the new sign-in may be another Claude account. The
  sign-in's outcome never depends on this read.
- The background usage round reads the email once for every signed-in account whose email was
  never answered. These are the subscriptions connected before this change, and temporary
  failures from earlier rounds.
- Only an answer to Yulius's own request is stored: it must be one `@`, printable, at most 254
  characters. It is never logged.
- The email is for administrators, as SPEC-001 UI-022 says ("sign-in email (administrators)").
  They see it wherever a subscription is named: the Subscriptions page, the Status panel, the
  Dashboard pool, Task details and the analytics. None is stored for an administrator to see, so
  the page reads "email unknown". Everyone else sees the name only. Task summaries, which go to
  every viewer of a task, never carry it.

## Alternatives

- **Intercept the token exchange through the egress proxy.** The email is certain to be there.
  But the proxy would terminate TLS for Claude's OAuth host during sign-ins, the sign-in would
  depend on the proxy, and the exchange answer (with the token) would pass through new code.
  Rejected for now. If production shows that the profile endpoint refuses setup-token sign-ins,
  this is the follow-up, with its own security review.
- **Read the sign-in terminal.** Rejected: the CLI never prints the email.
- **Let the person type the email.** Rejected: that is a second name, not the account behind the
  credential, and a typo would be shown as fact.
- **Show the email to everyone who sees the name.** Rejected: UI-022 limits the sign-in email to
  administrators, and a person's address would reach every member. The editable name is what
  members see.

## Consequences

- If Claude refuses the profile to setup-token sign-ins, every subscription reads "email unknown".
  The rename still lets people tell subscriptions apart. The follow-up above is then needed, and
  the background round costs one refused request per sign-in, not one per round.
- Renaming stays `PATCH /api/admin/accounts/{id}` (instance administrators, audited). The
  Subscriptions page now edits the name in place.
