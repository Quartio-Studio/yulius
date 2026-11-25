# 0478. A person gives each subscription's charge date

## Status

Accepted (2026-11-25). Specified in
[SPEC-651](../../.ai/specs/SPEC-651-hey-i-subscrbition-page-i-need-to-have-a.md); task #635 (the
owner, with a screenshot of the Subscriptions page: "I need to have also info when the subs will
charge my card so I can keep track of this … ex charge: 24.11"). Amends SPEC-001 ROT-009 (what
the Subscriptions page shows).

## Context

- The Subscriptions page shows each subscription's usage windows and the tokens Yulius observed,
  with their cost at API list prices. Nothing on it says what the person pays, or when.
- No provider tells Yulius the billing date:
  - Claude: Yulius signs subscriptions in with `claude setup-token`, whose token has only the
    `user:inference` scope; Claude answers `/api/oauth/usage` and `/api/oauth/profile` with 403
    ([ADR-0185](0185-a-subscription-email-comes-from-claudes-profile-at-sign-in.md), and every
    production card reads "email unknown"). Claude Code's own profile read knows
    `organization.subscription_created_at`, the day the subscription started, which is not its
    billing day after an upgrade, a pause or a failed payment.
  - Codex: the `auth.json`'s `id_token` names the plan. The Codex CLI reads no renewal claim, and
    no sign-in at hand shows that ChatGPT puts one there.
  - Antigravity: the CLI's token file is opaque to Yulius; a Gemini API key is billed by use.
- The person knows the date: it is on their receipt and their bank statement. Monthly plans
  charge on the same day each month; Claude Pro can also be paid yearly.

## Decision

- **An administrator gives one charge date and how often it repeats** (`month` or `year`) for a
  subscription, on its card (a dialog opened by "Set charge date" or the pencil). The date may be
  the last charge or the next one: the cycle runs both ways from it.
- **Yulius counts the next charge**, in the domain (`yulius_domain::billing`), from today's UTC
  date: the anchor's day each month (each year on the anchor's day and month), the month's last
  day in a month without that day, and a charge due today is today. Every `Account` view carries
  `billing: {anchor, period, next_charge_on}`, or `null`, and the card shows the date, how far it
  is and how it repeats.
- **`PUT` and `DELETE /api/admin/accounts/{id}/billing`**, instance administrators only, audited
  `account.billing_set` and `account.billing_removed`.
- **Its own table** (`account_billing`, removed with its subscription): a new sign-in, a token
  paste, an import or a rename never touches it. It belongs to what the person pays, not to the
  credential.

## Alternatives

- **Read the date from the provider.** Not possible today (Context). If a provider ever gives
  Yulius's credential its renewal date, it can fill the same cycle and the card can say where the
  date came from.
- **Guess it** from the day the subscription was added to Yulius, or Claude's
  `subscription_created_at`. Rejected: a guess would be shown as fact and is wrong after any
  upgrade or late payment.
- **A day of the month only** ("the 24th"). Rejected: it cannot say "every year on 24 Nov", and a
  date copied from a receipt needs no arithmetic from the person.
- **Count the next charge in the browser**, in the person's own day. Rejected: one count in the
  domain serves the page, the API and the Superagent alike. The page clamps "in N days" at
  "today", so the few hours around midnight where the UTC day differs never show a past date.
- **A column on `accounts`.** Rejected: `AccountRecord` is built by hand in about 40 test files,
  and a credential write must not be able to touch the cycle.
- **A reminder before each charge** (a notice or a Telegram message). Not built: the request is to
  see the date. It can read the same cycle later.

## Consequences

- Each card says when its subscription charges next once a person entered one date; the date
  moves on by itself every period.
- A date the person typed wrong is shown as they typed it, like a typed email
  ([ADR-0300](0300-an-administrator-can-type-a-subscriptions-email.md)); the dialog says where to
  find it.
- The Status panel, the Dashboard and task views do not show it: it is for the people who manage
  subscriptions, where they manage them.
