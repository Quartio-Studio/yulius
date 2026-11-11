# 0300. An administrator can type a subscription's email

## Status

Accepted (2026-11-11). Specified in
[SPEC-463](../../.ai/specs/SPEC-463-hey-i-want-to-do-a-quick-change.md); task #448 (the owner,
with a screenshot of the Subscriptions page: "we need to have a possibility to change address
email and the main text … only this small UI fix (those 2 editable lines)"). Amends
[ADR-0185](0185-a-subscription-email-comes-from-claudes-profile-at-sign-in.md), which rejected
typing the email, and SPEC-001 ROT-009 and UI-022. The rest of ADR-0185 stands.

## Context

- ADR-0185 reads a subscription's email from Claude's profile with the subscription's own
  credential, and stores nothing else. It named the risk: a `claude setup-token` sign-in has only
  the `user:inference` scope, so Claude may refuse it the profile, and then "every subscription
  reads 'email unknown'".
- That is what the production instance shows. People wrote the address into the name, which left
  the name nowhere to go.
- ADR-0185 rejected letting a person type the email: "that is a second name, not the account
  behind the credential, and a typo would be shown as fact". Its follow-up for this case, reading
  the email from Claude's token exchange through the egress proxy, was not built: the proxy would
  terminate TLS for Claude's OAuth host during sign-ins.
- The email is read in six places: the Subscriptions page, the Status panel, the Dashboard pool,
  Task details, the analytics, and the toast after a sign-in ("*name* is connected as *email*",
  ADR-0184). All read `accounts.email`.

## Decision

- **An administrator types the email in place**, under the name, on the Subscriptions page. It is
  saved by `PATCH /api/admin/accounts/{id}`, which now takes `email` beside an optional `label`:
  an address by the rule Claude's own answer passes (`plausible_email`), or an empty string to
  remove it. Instance administrators and the superadmin only; audited as `account.updated` with
  `email=set` or `email=removed`, never the address.
- **A typed email is the person's, like the name.** `accounts.email_typed` (a new column) marks
  it. A new sign-in keeps it, and Claude's answer never replaces it: beside a typed email only the
  time of the answer is stored. Removing it clears the email and that time, so the background
  round asks Claude once more and shows what it answers.
- **Each of the store's three writes settles a typed email in one SQL statement**
  (`set_account_email`, `set_account_credential`, `update_account`), so an answer from Claude
  that arrives while a person types cannot replace what they typed.
- **Yulius never says Claude confirmed a typed email.** `AccountView` carries `email_typed`, and
  the toast after a sign-in names the email only when Claude gave it.
- **Removing takes Enter.** Leaving a field saves what was typed, as for the name, but an emptied
  field that is left removes nothing.

## Alternatives

- **Keep "email unknown" until the token exchange is intercepted.** Rejected: that needs the
  proxy to open Claude's OAuth traffic and its own security review, for a line the owner can fill
  in a second. It stays the way to know the address for certain.
- **Store the typed email without a marker.** Rejected: the next sign-in would clear it
  (ADR-0185 drops the email at every new credential), and a late answer from Claude would
  replace it.
- **A second column, shown in preference to Claude's.** Rejected: every reader (four views, the
  analytics' SQL, the toast) would have to choose between two columns. One column with a marker
  keeps them as they are.
- **Tell a typed email apart in every view.** Rejected for now: only the sign-in toast says where
  an email comes from. The other views show an address under a name an administrator also typed.

## Consequences

- A card reads as its administrator wants: "Agent TEAM" over the team's address.
- A typed email is not verified. If a subscription is signed in again as another Claude account,
  the card still shows what was typed until someone changes it. The audit trail says who set it
  and when, not the address.
- A subscription with a typed email costs one profile request per sign-in whose answer is not
  stored.
- Removing an email that Claude gave makes the background round read it again, so it comes back
  within minutes: the line shows what the administrator typed, else what Claude shares.
- Schema version 51 adds `accounts.email_typed`; every email stored before stays Claude's. An
  older build refuses the newer database, as for every schema change (ADR-0264).
- The Subscriptions page no longer redraws its list while a name or an email field is open, also
  when the list was already being read as the field opened. Before, saving one line and going
  straight on to the other lost the second field, and its blur saved what was half typed.
