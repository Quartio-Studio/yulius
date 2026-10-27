---
yulius-task: 9d81b781-1692-4b19-a2bb-a3b1b751313b
status: done
---

# SPEC-337: A tab survives a sign-in in another tab (no stale CSRF token)

Status: done
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
  (no loop). A write sent before the page's own sign-in (`setCsrf`), or a reread answering after
  one, retries nothing: it was another session's, and a refusal after that sign-in asks for its
  own reread. A write cancelled during the reread rejects as `aborted` at once, without waiting
  for the reread. Bearer-token clients are unchanged.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec, ADR and issue for the stale CSRF token
- [x] Regression tests showing a stale token refuses the Superagent
- [x] Server tags the CSRF refusal; the page refreshes its token once and retries
- [x] Docs, independent review and delivery

## Acceptance criteria

- AC-1 (CSR-001): an HTTP test posts with a valid cookie and a wrong token, and with none, and gets
  403 `csrf_invalid`; a correct token passes.
- AC-2 (CSR-002): in the mock, after the session's token is changed under the open page (what
  another tab's sign-in does), opening the Superagent shows its terminal, not the error. The test
  fails on dev's `api.js`.
- AC-3 (CSR-003): in the mock, after the token changes and `/api/me` names another user, the
  page reloads, and the refused write was not sent again (counted across the reload).
- AC-4 (CSR-004): a request whose retry is refused again shows the refusal (one retry, no loop).
  In the API client, concurrent refusals share one reread, a write sent before the page's own
  sign-in is not retried, and a write cancelled during the reread rejects as `aborted`.

## Test plan

- Rust: `http_auth::password_login_rate_limit_and_csrf` asserts the `csrf_invalid` code (AC-1),
  written first and failing on `forbidden`.
- e2e (`e2e/tests/agent-panel.e2e.ts`): the stale-token Superagent test (AC-2), the other-user
  reload (AC-3) and the refused-retry case (AC-4). The mock answers `csrf_invalid` like the server.
  Two unit tests of `createApi` in the page with a stand-in transport (AC-4: shared reread, one
  retry, the page's own sign-in, a cancel).
  They wait on page elements, never on sleeps.
- The whole e2e suite and `scripts/dev ci` before delivery.

## Decisions

- [ADR-0228](../../docs/decisions/0228-a-page-rereads-its-csrf-token-when-the-session-changes.md):
  refresh the token from `/api/me` and retry once on `csrf_invalid`; reload when the user
  changed. Rejected: a CSRF token per user instead of per session, reading the token from a
  non-HttpOnly cookie (double submit), a `storage` event between tabs, and reloading on every
  refusal.

## Evidence

- Reproduced the cause in tests before the fix (commit b0dd7232):
  `http_auth::a_new_sign_in_refuses_the_old_csrf_token_and_me_gives_the_new_one` failed with
  `left: "forbidden"`, `right: "csrf_invalid"`; the three new agent-panel e2e tests failed
  (no terminal after the token changed, no reload, 1 refusal instead of 2).
- Fix (6bb7c5d7): `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^http_auth::/)'`
  21/21 passed; `npx e2e run tests/agent-panel.e2e.ts` 29/29 passed.
- `scripts/dev ci` after merging `origin/dev` (b13315c5): exit 0, nextest 2725 passed, 16 skipped;
  cargo-deny advisories, bans, licenses and sources ok.
- Full e2e suite (`npx e2e run`) at b13315c5: 742 passed, 1 skipped (67 files).
- Review round 1 (independent, fresh session): request changes. Medium: the reload test did not
  check that nothing was sent again. Low: a retry could cross the page's own sign-in, a cancel
  during the reread surfaced as `csrf_invalid`, the shared reread was untested, and `docs/api.md`
  lacked the code. Nits: SPEC-002 line wrap, this spec's Evidence. All fixed. The new unit test for
  the page's own sign-in and the cancel failed (hung on a retry) before da50cd04. After it,
  agent-panel e2e passed 31/31.
- Review round 2 (fresh session): approve at 6ad49664, with low findings in own code. A cancel
  waited for the reread. The reload test counted writes after the mock's latency, so a retry on
  its way could go uncounted. A refusal after the page's own sign-in could join an older reread.
  SPEC-002 had a short line. All fixed test-first in round 3. The extended unit test hung before
  the fix and passed after; agent-panel e2e passed 31/31.
- Review round 3 (fresh session): approve at 7e0df0c8, with low findings in own code. The guard
  that stops an older reread clearing a newer one was untested. A reread that the page's own
  sign-in had outdated could still reload the page. Fixed test-first: `onCsrfStale(isCurrent)`,
  and `currentCsrf` returns before deciding once `isCurrent()` is false. The test failed before
  e04006f8. A mutation check that removed the guard made it fail too. agent-panel e2e passed
  31/31.
- `scripts/dev security`: **not run** (exit 2). Vidoc answered HTTP 429, "This API key has used its
  50,000,000 tokens". The 7 changed code files were not scanned, so this is unrun, not passed.
- Not tested: a real browser with two tabs against `yulius serve`. The mock and the HTTP tests
  cover each side of it separately.
