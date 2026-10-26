# 0228. A page rereads its CSRF token when the session changes

## Status

Accepted. Specified in [SPEC-337](../../.ai/specs/SPEC-337-it-happens-sometimes-fix.md); task #321,
fixes #933. Keeps SPEC-002's cookie-session CSRF rule.

## Context

Each web session has its own CSRF token, which a page reads once when it signs in and sends on
every write. The session cookie is shared by every tab of the browser. A sign-in in any tab (GitHub
or password sign-in, or the Secrets page's GitHub identity confirmation) replaces the cookie with a
new session and a new token. Every other open tab then sends a stale token. Its reads still work,
and its writes are refused with "missing or invalid CSRF token" until the person reloads. The
Superagent showed it first because opening it is a write.

## Decision

- The server refuses a missing or wrong CSRF token with 403 and its own error code,
  `csrf_invalid` (`AppError::CsrfInvalid`), so a client can tell it from other refusals.
- On `csrf_invalid` the page reads `GET /api/auth/me`. That answer comes only to the page's own
  origin, and the server's site checks still apply. When it names the user the page is signed in
  as, the page takes its token and sends the request again, once. Concurrent refusals share that
  one read.
- When `/api/auth/me` names another user, the page retries nothing and reloads, so it never acts
  as someone else with what the previous person typed. A 401 runs the usual signed-out handling.

## Alternatives

- **One CSRF token per user instead of per session**: a new sign-in would keep the old token, but
  revoking one session would no longer change it, and every session of a user would share it.
- **Double-submit cookie** (a script-readable cookie holding the token): every tab would see the
  new token at once, but it adds a non-HttpOnly cookie that sibling hosts of the registrable domain
  could plant (SPEC-002 ADR-14 keeps the session cookie `__Host-`).
- **Tell the other tabs** (a `storage` event or `BroadcastChannel` at sign-in): misses sign-ins
  that end in a server redirect, such as GitHub's callback, unless every page load announces itself.
- **Reload on any CSRF refusal**: loses unsaved input in the page for a case that needs only a
  token.

## Consequences

- A tab open across another tab's sign-in keeps working without a reload.
- A request is sent at most twice. A write refused again after the retry fails as before.
- The mock answers `csrf_invalid` like the server, so the e2e suite can change the session under
  an open page.
