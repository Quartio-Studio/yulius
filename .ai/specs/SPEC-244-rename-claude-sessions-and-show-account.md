---
yulius-task: 99a9a048-da52-42c8-a233-daebe398f092
status: in-progress
---

# SPEC-244: Name a Claude subscription and see its account email

Status: in-progress
Date: 2026-10-20
Owner: task #230
Scope authorization: task #230. It carries feedback from Pawel, an early external user: "add
option to rename claude sessions + ideally see the session email". Changes SPEC-001 ROT-009, which
lists what a person manages on a subscription, and ROT-007, which defines what Task details shows.

## Goal

A person who manages the Claude subscriptions can give each one a name of their own, such as
"work Max" or "personal", by editing it in place on the Subscriptions page. Each subscription also
shows the email of the Claude account behind it, stored when Yulius signs it in. Every place that
names a subscription shows that name and email: the Subscriptions page, the Status panel, the
Dashboard's pool, Task details and the analytics. When no email is known, the place says
"email unknown".

## Context

Confirmed against `dev` at 33410359.

- **Terminology.** The UI calls a connected Claude account a *subscription* (#147); the API and
  the store call it an *account* (`accounts` table, `AccountView`). The feedback's "Claude
  session" means the same thing.
- **Names exist, renaming does not.** Every account has a `label` (1–80 printable characters)
  that the person types when adding it. `PATCH /api/admin/accounts/{id}` with `{"label"}` already
  renames an account for instance administrators (`App::update_account`, audited
  `account.updated`). The web UI never calls it: the Subscriptions page offers only Sign in again
  and Remove.
- **No email anywhere.** The `accounts` table has no email column. `accountUsage` in `layout.js`
  already renders an `email` field and avatar initials taken from it, but no API returns one.
- **Where the email can come from.** Yulius signs a subscription in with `claude setup-token` in a
  server-owned PTY (`accounts.rs`) and keeps only the token printed at the end. Claude Code
  2.1.292, the version in use, was checked as follows:
  - Its token exchange answer (`platform.claude.com/v1/oauth/token`) carries
    `account.email_address`, but in setup-token mode the CLI neither prints nor stores it. Only
    the token is shown, and no `oauthAccount` is written to the config directory.
  - Right after the exchange it calls `GET https://api.anthropic.com/api/oauth/profile` with the
    new token. That answer has `account.email` when the token may read the profile. The CLI
    ignores a failed call.
  - A setup-token sign-in has only the `user:inference` scope. Claude's usage endpoint
    therefore refuses it with HTTP 403 (`user:profile` missing, `wiring/usage.rs`). The profile
    endpoint may refuse it the same way, and nobody can check that without a real sign-in.
    Yulius has no such sign-in in its sandbox.
- **Who manages subscriptions.** Instance administrators and the superadmin (ACC-009,
  `require_instance_admin`). Everyone signed in sees a subscription's name, status and usage in
  the Status panel and the capacity view. Members of a sandbox see the subscription of its tasks
  in Task details.
- **Parallel work.** Task #229 changes how the sign-in terminal ends (`accounts.rs`, `admin.js`).
  This change stays clear of the terminal flow; it adds one call after a credential is stored.

## Requirements

| ID | Requirement |
| --- | --- |
| SUB-001 | On the Subscriptions page each subscription's name can be edited in place: a Rename button (or a click on the name) turns the name into a text field. Enter or leaving the field saves it, and Escape cancels. The name is trimmed, and an empty name or one over 80 characters is refused with the server's message while the field stays open. A saved name shows at once on the page and, through `account.updated`, in every open view. |
| SUB-002 | Only people who may manage subscriptions rename them: instance administrators and the superadmin. Anyone else gets 403 from `PATCH /api/admin/accounts/{id}`, and the page is not offered to them (unchanged ACC-009). Every rename is audited (`account.updated` with the new name). |
| SUB-003 | Each subscription stores the email of its Claude account. When a sign-in stores a credential, Yulius asks Claude's profile endpoint for the account's email, using that credential, and stores the answer. A sign-in whose email cannot be read (the token may not read the profile, Claude refused it, or no answer) stores no email. An email from an earlier sign-in is dropped then, because the new sign-in may be another Claude account. The sign-in itself succeeds or fails exactly as before; reading the email never blocks or fails it. |
| SUB-004 | Subscriptions signed in before this change get their email read once, in the background usage round, while their credential still works. A definite answer, an email or a refusal, ends the attempts for that sign-in. A temporary failure (no answer, throttling) is tried again on a later round. Without an email the subscription reads "email unknown" until its next sign-in. |
| SUB-005 | Wherever a subscription is named, its email is shown with its name, or "email unknown" when no email is stored: the Subscriptions page, the Status panel and the Dashboard pool (each account row), Task details (the current subscription and each run's), and the analytics' tokens by subscription. The API carries `email` (`string \| null`) next to `label` in `AccountView`, `StatusAccount` and `CapacityAccount`. It carries `account_email` next to `account_label` on a task (`TaskSummary`), a run (`AttemptView`) and a run's analytics row. Every viewer who sees the name sees the email. |
| SUB-006 | The email is personal data handled as such. It is read only from Claude's answer to Yulius's own request: never from the sign-in terminal, never guessed. It must look like an address (one `@`, printable characters, at most 254 characters), or it is not stored. It is never written to logs. |

## Plan

- [ ] Spec, ADR and plan for names and emails
- [ ] Accounts store an email, read at sign-in and backfilled
- [ ] The API returns the name and email wherever a subscription is named
- [ ] Subscriptions page renames in place and shows the email
- [ ] Status, Dashboard, Task details and analytics show name and email
- [ ] End-to-end proof of rename and email in the web UI
- [ ] Docs, product spec and changes entry match
- [ ] Independent review answered and gates green
- [ ] Merge into dev on green CI

## Acceptance criteria

- Renaming a subscription on the page persists: after a reload, the Status panel, the Dashboard
  pool and Task details show the new name (e2e `subscriptions` spec, a store test, and an API test
  of `PATCH`).
- A user without instance administration gets 403 when renaming (API test).
- A sign-in whose credential reads a profile with `account.email` stores that email. A sign-in
  whose profile read is refused stores none and drops an older one. A failing read leaves the
  sign-in succeeded (integration tests with a fake probe).
- An account without an email checked before is read once by the background round and not again
  after a definite answer; a temporary failure is retried (integration test).
- `AccountView`, `StatusAccount`, `CapacityAccount`, the task summary, the attempt view and the
  analytics row carry the email (`null` when unknown), and the UI shows "email unknown" for
  `null` (unit and e2e tests).
- The profile answer parser takes `account.email` or `account.email_address`. It refuses anything
  that is not an address and reads 403 as "may not read" (adapter unit tests).

## Test plan

- Store: migration 0038 adds `email` and `email_checked_at`; set and read them; a new sign-in
  clears an old email.
- Adapter (`wiring/usage.rs`): a local HTTP server answering the profile request with an email,
  without one, with 403, 401 and 500; the parser refuses a non-address.
- App/integration (`crates/yulius/tests/accounts.rs`, `account_usage.rs`): sign-in stores the email
  through a fake probe; refused read clears it; backfill round reads once and retries temporary
  failures; rename by a non-admin is 403; views carry the email.
- Web: the mock serves emails (one subscription with `null`). The e2e `subscriptions` spec renames
  in place, refuses an empty name, and sees the name and email in Status and Task details.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security`.

## Decisions

- **Source of the email: Claude's profile endpoint, asked by Yulius with the stored token.** This
  is the call Claude Code itself makes after a setup-token exchange. It goes through the existing
  usage adapter (same client, timeouts and plain failures), and the backfill can use it. Rejected:
  - Parsing the sign-in terminal. The CLI never prints the email.
  - Intercepting the token exchange answer through the egress proxy. That is the only place the
    email is certain to appear, but the proxy would have to terminate TLS for Claude's OAuth host
    during sign-ins and the sign-in would depend on the proxy. It is the obvious follow-up if the
    profile endpoint refuses setup-token sign-ins in production (see the ADR).
  - Asking the person to type the email. That is not read from the credentials, and a typo would
    be shown as fact.
  Recorded in [ADR-0185](../../docs/decisions/0185-a-subscription-email-comes-from-claudes-profile-at-sign-in.md).
- **Who sees the email: whoever sees the name.** Subscriptions are a shared resource of an
  instance whose users are all admitted by its superadmin. Task details must say which Claude
  account a task ran on, which is the point of the feedback. Showing the email only to
  administrators would hide it exactly where members need it.
- **Unknown is stored as `NULL` plus `email_checked_at`.** `email_checked_at` marks a definite
  answer for the current sign-in, so the backfill asks once per sign-in, not every round. A new
  sign-in resets both.
- **Renaming reuses `PATCH /api/admin/accounts/{id}`.** It is already validated, audited and
  limited to administrators. The UI is the missing part.
- **No picker exists to change.** Tasks are subscription-agnostic (ROT-007): the UI offers no
  per-task account choice. "Pickers" in the feedback is covered by the Subscriptions page, where
  subscriptions are listed and managed.
- Switch records (`switch_decisions.from_desc/to_desc`) keep the name a subscription had when the
  switch happened; they are history and are not rewritten by a rename.
