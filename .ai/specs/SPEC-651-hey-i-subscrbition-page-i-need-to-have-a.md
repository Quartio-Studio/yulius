---
yulius-task: 96417217-47ba-402b-807a-8c67f4647150
status: in-progress
---

# SPEC-651: Each subscription card says when it next charges the card

Status: in-progress
Date: 2026-11-25
Owner: Yulius task #635
Scope authorization: task #635 (`/goal`, with a screenshot of the Subscriptions page: three cards
with their usage windows and the tokens Yulius observed): "I need to have also info when the subs
will charge my card so I can keep track of this … ex charge: 24.11 etc please analyze deeply how
to do it and do it the best way". Amends SPEC-001 ROT-009 (the Subscriptions page) and SPEC-244
(what a subscription card shows).

## Goal

Every card on the Subscriptions page can say when its subscription charges the card next, for
example "Next charge Thu 24 Dec 2026 · in 29 days", with how often it is charged, so a person
keeps track of the charges of every Claude, Codex and Antigravity subscription in one place.

## Context

Confirmed against `dev` at 1770c1b30.

- **The page.** `mountAccountsPage` in `web/assets/pages/admin.js` draws one card per
  subscription: its head and usage windows (`accountUsage` in `layout.js`), the tokens Yulius
  observed (`tokenTotals`) and the buttons. Nothing on it is about money a person pays: the
  dollar figures are what the observed tokens would cost at API list prices.
- **No provider tells Yulius the billing date.**
  - Claude: Yulius signs subscriptions in with `claude setup-token`, whose token has only the
    `user:inference` scope. `/api/oauth/usage` and `/api/oauth/profile` answer 403 for it
    (`wiring/usage.rs`, ADR-0185, SPEC-463 Context: every production card reads "email
    unknown"). Claude Code's own profile read knows `organization.subscription_created_at`
    (seen in the CLI's code), the day the subscription started, not its billing day after an
    upgrade, a pause or a failed payment, and Yulius cannot read it anyway.
  - Codex: the `auth.json`'s `id_token` names the plan (`chatgpt_plan_type`, read by
    `codex_pro_500`). The Codex CLI reads no renewal claim (none in its binary), and no
    `auth.json` with one is at hand to prove that ChatGPT puts one there.
  - Antigravity: the CLI's token file is opaque to Yulius (SPEC-505); a Gemini API key is billed
    by use, with no charge date.
- **So the person tells Yulius the date**, as they told it the email (SPEC-463): they know it
  from their receipt or their bank, and the request says it in that form ("charge: 24.11").
  Monthly plans charge on the same day each month (Claude Pro, Max; ChatGPT Plus, Pro); Claude
  Pro can also be paid yearly.
- **Dates exist in the domain.** `yulius_domain::cron::CivilDate` (Hinnant's civil-date
  arithmetic, `days_in_month`) is exact and tested; it is reused.
- **`AccountRecord` is built by hand in 39 test files.** A new field on it would touch all of
  them and collide with parallel work (memory: new struct field vs parallel PRs). The charge date
  is kept in its own table instead, and read where a view is made.

## Requirements

| ID | Requirement |
| --- | --- |
| BIL-001 | A subscription may have one billing cycle: a charge date the person knows (`anchor`, a calendar date, the last charge or the next one) and how often it repeats (`period`: `month` or `year`). None is set by default. |
| BIL-002 | The next charge is the first date on or after today (UTC) that is the anchor moved by a whole number of periods, forwards or backwards. A monthly cycle keeps the anchor's day of month; in a month without that day it is the month's last day (anchor 31 Jan → 28 or 29 Feb, 31 Mar, 30 Apr). A yearly cycle on 29 Feb charges on 28 Feb in other years. A charge due today is today, not next period. |
| BIL-003 | `PUT /api/admin/accounts/{id}/billing` with `{"anchor": "YYYY-MM-DD", "period": "month"\|"year"}` sets the cycle and answers the `Account`; `DELETE` on the same path removes it and answers the `Account` (also when none was set). An anchor that is not a real calendar date in that exact form, a year before 2000 or after 2100, another period, a missing or unknown field is 422 `invalid`, nothing written. Instance administrators only (403 otherwise); 404 for an unknown subscription. Audited `account.billing_set` with `anchor=… period=…`, or `account.billing_removed`. |
| BIL-004 | Every `Account` the server gives (the list, every answer of an account route, the `account.updated` event) carries `billing`: `null` without a cycle, else `{"anchor", "period", "next_charge_on"}` (dates `YYYY-MM-DD`), so an update never drops it. |
| BIL-005 | Removing a subscription removes its cycle. A cycle stays across sign-ins, token pastes, imports and renames: it belongs to the subscription the person pays, not to its credential. |
| BIL-006 | Each Subscriptions card has a "Next charge" line under the observed tokens. Set: the date with its weekday in the browser's locale and how far it is ("today", "tomorrow", "in 29 days"), and under it how it repeats ("Every month on the 24th", "Every month on the 31st, or the month's last day", "Every year on 24 Nov", "Every year on 29 Feb, or 28 Feb in other years"), with a pencil button "Change the charge date of *name*". Not set: "Not set" and a button "Set charge date". |
| BIL-007 | The pencil or the button opens a dialog "Charge date · *name*": a date field "A charge date" (prefilled with the anchor, empty when none), with the hint "The day this subscription charges your card: the last charge or the next one, from your receipt or bank statement. Yulius counts the next charges from it."; a choice "Charged" of Every month (the default) or Every year; Cancel, Save, and Remove when a cycle is set. Save without a date is refused under the field ("Pick the date of a charge.") and sends nothing. A saved or removed cycle shows on the card at once, with a toast ("Next charge of *name*: *date*." / "Charge date of *name* removed."). A refusal from the server stays in the dialog. |
| BIL-008 | The Status panel, the Dashboard and the task views do not show the charge date: it is for the people who manage the subscriptions (instance administrators), where they manage them. |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A billing cycle counts a subscription's next charge
- [x] The store keeps each subscription's charge date and drops it with the subscription
- [x] PUT and DELETE /api/admin/accounts/{id}/billing, and the next charge in every account view
- [x] Each Subscriptions card shows its next charge and sets it in a dialog
- [x] ADR-0478, API reference, architecture and product say where the charge date comes from
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A monthly cycle anchored on 2026-11-24 gives 2026-11-24 on that day, 2026-12-24 from
  2026-11-25, and 2026-11-24 from any day of 2026-10-25..2026-11-24 (an anchor in the future
  counts backwards too) (domain unit tests).
- Anchor 2026-01-31 monthly gives 2026-02-28, 2026-03-31, 2026-04-30; 2028-02-29 (leap);
  yearly anchor 2024-02-29 gives 2025-02-28 and 2028-02-29 (domain unit tests).
- The store keeps, replaces and removes a cycle; deleting the subscription deletes its cycle; a
  new credential keeps it (store tests).
- PUT sets the cycle and the answer carries `next_charge_on`; the list and the `account.updated`
  event carry it; DELETE removes it; a bad date (`2026-02-30`, `24.11.2026`, `1999-12-31`), a bad
  period, an unknown field are 422 with nothing written; a member is 403; an unknown id is 404;
  both are audited (API tests).
- On the Subscriptions page, a card without a cycle says "Not set"; setting 24 Nov 2026 monthly in
  the dialog shows "Next charge" with the date and "Every month on the 24th"; reopening shows the
  stored date; Remove brings "Not set" back; Save without a date is refused with nothing sent
  (e2e against the mock).

## Test plan

- `yulius-domain`: unit tests of `BillingCycle::next_charge` (BIL-002 cases above) and of the
  anchor's parsing.
- `yulius-store`: the cycle's round trip, its removal, the cascade on a deleted subscription, and
  that a credential change keeps it (`tests/store.rs`).
- `yulius` integration: `tests/account_billing.rs` drives the routes over HTTP (BIL-003, BIL-004)
  with the audit and the published event.
- e2e: `e2e/tests/subscription-billing.e2e.ts` against the mock (BIL-006, BIL-007); the mock
  computes `next_charge_on` like the server for the cases the test uses.
- Mutation checks by hand: undo the month-end clamp and the "today counts" comparison, watch the
  domain tests fail.

## Decisions

- **The person enters the date; Yulius does not guess it** (ADR-0478). No provider gives Yulius a
  billing date it can read (Context). A guessed date (the sign-in day, the subscription's start)
  would be shown as fact and be wrong after any upgrade; a typed one is what the person's receipt
  says. If a provider ever exposes the renewal to Yulius's credential, it can fill the same cycle.
- **An anchor date plus a period, not a day of month.** "The 24th" cannot say "yearly on 24 Nov",
  and a date the person copies from a receipt (last charge or next) needs no arithmetic from them.
  The anchor's day is kept, so a cycle anchored on the 31st returns to the 31st after February.
- **The server counts the next charge**, once, in the domain, against today's UTC date: every
  view and API client get the same answer. Around midnight a person far from UTC may see a charge
  as "today" a few hours longer or shorter; the page clamps the day count at "today", never
  negative.
- **Its own table** (`account_billing`, `ON DELETE CASCADE`) instead of a column on `accounts`:
  `AccountRecord` stays as it is (39 test files build it), a credential change cannot touch the
  cycle, and the removal cleans up by itself.
- **Its own route** (`PUT`/`DELETE …/billing`) instead of a `PATCH` field: the cycle is set in a
  dialog, not in place, and `null` versus absent in `PATCH` would need a third state there.
- **A dialog, not an in-place field.** Two inputs (a date and a period) do not fit one line of the
  card, and the in-place name and email editing took five review rounds of focus rules
  (SPEC-463); a dialog uses `openModal`'s settled behaviour.
- **No reminder before a charge** (a notice or a Telegram message a day before). The request is to
  see the date; a reminder is a later feature if asked for.

## Review

Round 1 (yulius-reviewer, head `1be35c8b1`): request changes. Each finding and its answer:

1. Major, own code: the e2e file did not hold the shown date, the toast's date, the day count or
   the forward count; three mutations survived. Fixed test-first: the page's clock is held at
   noon on 10 Feb 2027 and the tests assert whole dates (formatted by the browser's own locale)
   and exact counts for today, tomorrow, yesterday's charge (next month), a 31st (28 Feb), a
   yearly 29 Feb (28 Feb 2027), a yearly charge 303 days ahead, and the toast's date. They failed
   first because the page counted from the wall clock (`new Date()`), not its `Date.now`, which
   the page now reads. The reviewer's three mutations (a fixed date, a count 3 days off, no move
   to the next period in the mock) each fail the file now.
2. Minor, own code: no test for a server refusal kept in the dialog, focus after Save and
   Remove, or nothing sent on an empty date. Added to the first e2e test (a `1999-12-31` refusal,
   `aria-label` of the focused control after Save, "Set charge date" focused after Remove, the
   mock's `sent` count).
3. Minor, own code: BIL-004 unheld at `publish_account_id` and the usage, token, relogin and
   import answers. New test `every_update_and_answer_about_a_subscription_carries_its_charge_date`
   holds the background round's email update (`publish_account_id`), the usage refresh's answer,
   a pasted token's answer, and the `null` published after a removal; the 422 bodies now assert
   the `invalid` code. Reverting `publish_account_id` or the usage refresh to `account_view`
   fails it.
4. Minor, own code: ADR-0478's "never show a past date" was inexact. Reworded: east of UTC a card
   shows yesterday's charge for as many hours as the offset, its count clamped at "today"; west of
   UTC the next period's date shows that many hours early.
5. Minor, pre-existing: a list redraw under an open dialog sends the keyboard to the page heading
   on close; "Paste a new token" does the same on `dev`. Filed as #1758.
6. Nits, own code: `change_billing` now reads the subscription again after the write; the mock no
   longer leaks `billing_cycle` and all its `account.updated` events carry `billing`; a yearly
   29 Feb reads "Every year on 29 Feb, or 28 Feb in other years"; the Goal's weekday is Thursday.
   One unreadable `account_billing` row failing every account view is kept: it is consistent with
   how every other store read treats a bad column (`bad_column`), and the API cannot write one
   (`BillingCycle::parse` checks every value, the table's `CHECK` the period).
