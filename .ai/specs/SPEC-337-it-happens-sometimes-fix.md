---
yulius-task: 9d81b781-1692-4b19-a2bb-a3b1b751313b
status: in-progress
---

# SPEC-337: A tab survives a sign-in in another tab (no stale CSRF token)

Status: in-progress
Date: 2026-10-27
Owner: Yulius task #321
Scope authorization: task #321 (`/goal`, "it happens sometimes, fix" with a screenshot of the
Superagent panel reading "Couldn't start the Superagent: missing or invalid CSRF token"), fixes
#933. Keeps SPEC-002's cookie-session CSRF rule (every state-changing cookie request carries the
session's token).

## Goal

A page that was open while the browser signed in again (another tab, a GitHub identity
confirmation) keeps working: its next write succeeds without a reload, as the same person, and
never as someone else.

## Context

- **Symptom.** The Superagent panel shows "Couldn't start the Superagent: missing or invalid CSRF
  token"; Try again fails the same way until the page is reloaded.
- **Code path.** `App::new_session` (`crates/yulius-app/src/auth.rs`) gives each web session its
  own random CSRF token. The page reads it once, in `signedIn` (`web/assets/app.js`,
  `api.setCsrf(me.csrf_token)`), and `createApi` (`web/assets/api.js`) sends it as
  `X-Yulius-CSRF` on every non-GET. The `Auth` extractor (`crates/yulius-server/src/auth.rs`)
  compares it with the token of the session the cookie names and refuses a mismatch with 403
  `forbidden`.
- **Root cause.** The session cookie is shared by every tab, but the token lives in each page's
  memory. A sign-in in any tab (GitHub sign-in, password sign-in after an expiry, the Secrets
  page's GitHub identity confirmation, which runs `github_callback` → `new_session`) replaces the
  cookie with a new session that has a new token. Older tabs keep sending the old token. Their GETs
  need none, so they look fine, but their first write is refused, and nothing reads the token
  again. Opening the Superagent is often a tab's first write (`POST /api/sandboxes/{id}/cockpit`),
  which is why it shows there. It happens only "sometimes" because it needs a second sign-in while
  a tab is open.
- **Why tests missed it.** Every test signs in once per page; none changes the session under an
  open page. The mock checks the token but answers the generic `forbidden` code.

## Requirements

- CSR-001: The server refuses a cookie request with a missing or wrong CSRF token with status 403
  and the error code `csrf_invalid` (message unchanged), distinct from other refusals.
- CSR-002: On a `csrf_invalid` refusal the page reads `GET /api/me`. If it names the user the
  page is signed in as, the page takes that response's CSRF token and sends the request again,
  once. Concurrent refusals share one read.
- CSR-003: If `/api/me` names another user, the page retries nothing and reloads, so it shows
  the person now signed in. If it answers 401, the usual "signed out" handling runs.
- CSR-004: A request refused again after the retry, or refused before any sign-in, fails as before
  (no loop). Bearer-token clients are unchanged.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Spec, ADR and issue for the stale CSRF token
- [ ] Regression tests showing a stale token refuses the Superagent
- [ ] Server tags the CSRF refusal; the page refreshes its token once and retries
- [ ] Docs, independent review and delivery

## Acceptance criteria

- AC-1 (CSR-001): an HTTP test posts with a valid cookie and a wrong token, and with none, and gets
  403 `csrf_invalid`; a correct token passes.
- AC-2 (CSR-002): in the mock, after the session's token is changed under the open page (what
  another tab's sign-in does), opening the Superagent shows its terminal, not the error. The test
  fails on dev's `api.js`.
- AC-3 (CSR-003): in the mock, after the token changes and `/api/me` names another user, a
  write does not go through and the page reloads.
- AC-4 (CSR-004): a request whose retry is refused again shows the refusal (one retry, no loop).

## Test plan

- Rust: `http_auth::password_login_rate_limit_and_csrf` asserts the `csrf_invalid` code (AC-1),
  written first and failing on `forbidden`.
- e2e (`e2e/tests/agent-panel.e2e.ts`): the stale-token Superagent test (AC-2), the other-user
  reload (AC-3) and the refused-retry case (AC-4). The mock answers `csrf_invalid` like the server.
  They wait on page elements, never on sleeps.
- The whole e2e suite and `scripts/dev ci` before delivery.

## Decisions

- [ADR-0228](../../docs/decisions/0228-a-page-rereads-its-csrf-token-when-the-session-changes.md):
  refresh the token from `/api/me` and retry once on `csrf_invalid`; reload when the user
  changed. Rejected: a CSRF token per user instead of per session, reading the token from a
  non-HttpOnly cookie (double submit), a `storage` event between tabs, and reloading on every
  refusal.
