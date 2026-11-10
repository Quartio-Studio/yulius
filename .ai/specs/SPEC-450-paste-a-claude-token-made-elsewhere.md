---
yulius-task: 49e3b24c-2853-4c54-9377-281144689c4f
status: done
---

# SPEC-450: Connect a Claude subscription by pasting a token made elsewhere

Status: implemented
Date: 2026-11-10
Owner: Yulius task #6 (studenciak00owl/yulius), proposed upstream
Scope authorization: the owner's request (2026-11-10): "Add to upstream Yulius a second way to
connect a Claude subscription: paste a long-lived token made with `claude setup-token` on another
machine (`sk-ant-oat01-…`, valid about a year), next to the existing login terminal. Use case: the
person (or a colleague with the subscription) runs `claude setup-token` on their laptop and pastes
the token; no terminal session on the server is needed." Extends SPEC-001 ROT-001/ROT-002 and
SPEC-243 (the login terminal stays as it is). ADR:
[0289](../../docs/decisions/0289-a-subscription-can-be-connected-by-pasting-a-setup-token.md).

## Goal

An instance administrator connects a Claude subscription, or repairs one, by pasting the token
`claude setup-token` printed on any computer, instead of finishing Claude's sign-in in a terminal
on the server. The subscription is usable at once, exactly as if the login terminal had captured
the same token.

## Context

Confirmed against upstream `dev` at 91a0cec5.

- **One way in today.** `POST /api/admin/accounts {"provider":"anthropic","label"}` creates the
  account in `pending_login` and starts `claude setup-token` in a server-owned PTY
  (`App::start_login`); the person opens the link and approves in the browser that holds the
  subscription. On exit (or 5 s after a complete token, SPEC-243 LGN-001) the token is taken from
  the masked output with `HarnessPort::extract_claude_token`, sealed with `Crypto::seal` and stored
  by `AccountStore::set_account_credential` with status `ok` (`App::store_credential`); then the
  pool is told (`capacity_changed`), the email is read (SPEC-244 SUB-003) and the usage asked for.
- **Why a second way.** The subscription's owner is often not the instance administrator, or not
  at that browser: today they must share a screen or the approval link. `claude setup-token` runs
  anywhere and prints the same long-lived token (`sk-ant-oat01-…`, about a year); the CLI on the
  server only exists to run it.
- **A cheap check exists.** The usage adapter's `account_email` (`GET /api/oauth/profile`) answers
  401 → `UsageFailureKind::Rejected` for a token Claude does not accept and 403 → `NotShared` for a
  `setup-token` (it has `user:inference` only, SPEC-002 ADR-17), which is the normal answer for a
  working pasted token. It is one request with the client's timeout, already used after every
  sign-in.
- **Audit actions** are documented beside each endpoint in `docs/api.md`.

## Requirements

| ID | Requirement |
| --- | --- |
| TKP-001 | `POST /api/admin/accounts {"provider":"anthropic","label","token"}` (instance administrator) creates the subscription signed in with that token and answers 201 `{"account": Account}` with `status: "ok"` and no `login_id`; no login terminal, process or temporary directory is started. Without `token` (or `token: null`) the request behaves as today (`pending_login` and a `login_id`). |
| TKP-002 | `PUT /api/admin/accounts/{id}/token {"token"}` (instance administrator) replaces the subscription's credential with the token and answers 200 `Account` with `status: "ok"`, `last_error: null` and the email read again; a running sign-in of that subscription is cancelled and waited for (at most 15 s) first, and one that has not ended by then is 409 with nothing changed, so a token it captured can never overwrite the pasted one. 404 for an unknown subscription. It repairs a `relogin`, `error` or `pending_login` subscription and re-signs a working one. |
| TKP-003 | The token is accepted only in the shape `claude setup-token` prints: after trimming surrounding whitespace, `sk-ant-oat01-` followed by 20 or more of `A-Z a-z 0-9 - _`, at most 512 characters in all. Anything else is 422 `invalid` with one sentence saying what a token looks like and how to make one; nothing is created or changed. |
| TKP-004 | Before anything is stored, Yulius asks Claude's profile once with the token. A refusal (HTTP 401) is 422 `invalid` "Claude did not accept this token…" and nothing is created or changed. Every other answer accepts the token: an email is stored as the subscription's email, a refusal to read the profile (HTTP 403, normal for a `setup-token`) is stored as "no email", and no answer for now leaves the email to the background round (SPEC-244 SUB-004). |
| TKP-005 | The token is sealed and stored exactly like a token the login terminal captured (`Crypto::seal`; status `ok`; `last_error`, email and its check cleared), the pool is told at once so waiting tasks may continue on it (ROT-010), its usage is read in the background, and `account.updated` is published. |
| TKP-006 | The token never leaves the request in clear: it is never in a response, an error message, an event, the audit log or a log line, the request types' `Debug` prints it redacted, and the plaintext copy is zeroed once sealed. |
| TKP-007 | Each paste is audited without the token: a new subscription as `account.created` with detail `provider=anthropic label=<label> via=token`, a replaced token as `account.token_replaced` (no detail). A refused paste is not audited. |
| TKP-008 | The Subscriptions page's Add subscription dialog offers a choice of **Sign in here** (today's terminal, the default) and **Paste token**. Paste token shows a Token field and one hint: run `claude setup-token` on any computer signed in to the subscription and paste what it prints; do not send it through chats or email in clear text. An empty or malformed token, or one the server refuses, is shown under the field (`invalid()`); success closes the dialog and a toast says the subscription is connected. |
| TKP-009 | Every subscription card has **Paste a new token**, a dialog with the same field and hint that calls TKP-002; success says so in a toast, a refusal under the field. Both dialogs pass the e2e accessibility (axe) check. |
| TKP-010 | A subscription connected by paste is used like any other: a new task starts on it, and its run gets the pasted token as `CLAUDE_CODE_OAUTH_TOKEN`. |

## Plan

- [x] Spec SPEC-450 and ADR-0289 on upstream dev
- [x] API: a pasted token creates a subscription that is ok at once
- [x] API: an existing subscription takes a new pasted token
- [x] A task starts on a pasted-token subscription (fake agent)
- [x] UI: Add subscription offers Sign in here or Paste token
- [x] UI: the subscription card pastes a new token, e2e and axe
- [x] Docs: api.md, product.md, architecture, changes
- [x] Gates green, independent review, one squashed commit pushed

## Acceptance criteria

- AC-1 (TKP-001, TKP-005, TKP-007): an administrator posts a well-formed token: 201, the account is
  `ok` with no `login_id`, `GET /api/admin/accounts` lists it `ok`, its stored credential opens to
  the token, the audit row is `account.created` with `via=token` and holds no token.
- AC-2 (TKP-003, TKP-006): a token without the prefix, with a short body, with a space or other
  character inside, longer than 512 characters, or empty is 422; nothing is created; no response
  body holds the token.
- AC-3 (TKP-004): a token whose profile answers 401 is 422 and nothing is created; a token whose
  profile answers 403 is accepted with no email; one whose profile has an email is accepted and
  shows that email.
- AC-4 (TKP-002): `PUT …/token` on a `relogin` subscription makes it `ok` with the new credential,
  audited `account.token_replaced`; a running sign-in of it ends `cancelled`; an unknown id is
  404; a malformed or refused token changes nothing; a member is 403.
- AC-5 (TKP-010): with the real process host and the fake agent, a new task on an instance whose
  only subscription was pasted starts on it and its run has `CLAUDE_CODE_OAUTH_TOKEN` of the
  pasted token's length.
- AC-6 (TKP-008, TKP-009): in the mock UI, Paste token connects a subscription, a malformed token
  is refused under the field, Paste a new token repairs a card, and axe finds no violations in
  either dialog.

## Test plan

- Unit (`yulius-app::accounts`): the token shape (prefix, body length and charset, whitespace,
  the 512 bound) and the redacted `Debug` of the request types (`yulius-contracts`).
- Integration (`crates/yulius/tests/account_token.rs`, HTTP router with a scripted profile probe):
  AC-1 to AC-4, including the audit rows and that no response or audit detail holds the token.
- Integration (same file, real `PtySessionHost` and fake agent, the `pool_supervision` builders):
  AC-5.
- e2e (`e2e/tests/subscriptions.e2e.ts`, mock mode): AC-6.

## Decisions

- **Same endpoint, optional `token`; a separate `PUT …/token` for a replacement.** Creation with a
  token is still "add a subscription", so it stays `POST /api/admin/accounts`; the answer keeps its
  shape with `login_id` absent. A replacement is a different act from `POST …/relogin` (which
  starts a terminal and answers a `login_id`), so it gets its own idempotent `PUT`. Rejected: a
  `token` body on `/relogin` (one endpoint with two answers), a separate `POST
  /api/admin/accounts/token` (a second way to create the same record).
- **Check with the profile request, refuse only on 401.** It is the one request Yulius already makes
  with a fresh credential, so it costs nothing new; 403 is the normal answer for a `setup-token`
  and anything temporary must not block a person. Rejected: no check (a typo would make a
  subscription that fails its first task), a test inference request (spends the subscription,
  slower, a new adapter).
- **Shape bound of 512 characters.** Real tokens are about 100 characters; the bound keeps a
  mistaken paste (a whole terminal screen) out and is far above any plausible token growth.
- **No migration.** The account row is the same; only how its credential arrived differs, and
  that lives in the audit log.

## Evidence

Run on `feat/claude-token-paste` from upstream `dev` at 91a0cec5 (2026-11-10). Builds used
`CARGO_INCREMENTAL=0 -j 1` (the machine's memory).

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`: clean.
- `cargo test -p yulius-contracts -p yulius-app -p yulius-server`: 474 + 50 + 25 + 2 passed,
  among them `accounts::tests::pasted_tokens_must_have_the_setup_token_shape` (TKP-003) and
  `api::admin::tests::pasted_tokens_are_redacted_in_debug` (TKP-006).
- `cargo test -p yulius --test main -- account_token::`: 6 passed (AC-1 to AC-5 and
  `a_paste_wins_over_a_token_the_terminal_captured`); with `accounts::`, `account_email::`,
  `claude_gate::`, `account_usage::`, `ui_contract::` and `subscription_pool::`: 52 passed.
- `npx e2e run` (Node 24.18.0) of `subscriptions`, `claude-connect`, `consistency`,
  `onboarding-flow`, `subscription-names` and `loading-shapes`: 69 passed, axe clean in both new
  dialogs (AC-6).
- `python3 scripts/check_repository.py`: passed; `python3 -m unittest discover -s scripts -p
  'test_*.py'`: 152 passed.
- The tests were written with the code, not before it; instead each was shown to fail on the
  defect it guards by a mutation, then the code restored: Claude's 401 accepted and a 1-character
  token body accepted made `a_malformed_token_is_refused_and_creates_nothing`,
  `a_token_claude_refuses_creates_nothing` and `a_new_token_signs_a_subscription_in_again` fail;
  removing the cancel of a running sign-in made `a_new_token_signs_a_subscription_in_again` and
  `a_paste_wins_over_a_token_the_terminal_captured` fail.
- Independent review (fresh session, `om-code-review`): round 1 request changes (one major: the
  cancel assertion could not fail; minor: zeroing on early returns, an ignored wait timeout,
  missing cases), all fixed; round 2 approve.

Not verified here:

- `scripts/dev security` (Vidoc): no key in this environment; upstream CI runs the `security` job
  on the pull request.
- The 409 when a running sign-in has not stopped within 15 s: the fakes' sign-ins always end
  within the 10 s stop wait, so the branch has no test.
- A task waiting for capacity resuming on a pasted subscription: the paste calls the same
  `capacity_changed()` as the terminal's sign-in, covered by the ROT-010 tests; no test removes
  that call here.
- A real `claude setup-token` token against Claude's real profile endpoint (synthetic tokens and
  a scripted profile only).
