---
yulius-task: e21572b4-5fb2-4b4f-befc-86979fd86619
status: in-progress
---

# SPEC-295: The login terminal socket closes when its session or token is revoked

Status: in-progress
Date: 2026-10-24
Owner: task #281
Scope authorization: task #281 (nightly issue automation, task #277), issue #759 (opened by task
#233's independent review).

## Goal

An open provider sign-in terminal (`WS /api/admin/accounts/login/{login_id}/terminal`) ends like
the other sockets once the sign-in that opened it no longer holds: within one re-check interval of
a sign-out, a revoked API token, an expired session, a disabled or deleted user, or the loss of
instance administration, it closes with 4003.

## Context

- Symptom: an instance administrator's open login terminal keeps streaming output and accepting
  input after their session or token is revoked, until the sign-in ends (at most 15 minutes).
- Root cause: `login_terminal` in `crates/yulius-server/src/routes/accounts.rs` checks access once,
  in `App::login_terminal` (`require_instance_admin`), then hands only the `LoginTerminal` to
  `pump`/`serve`. The select loop has arms for the socket and the hub, no re-check, and the file
  never calls `App::still_signed_in`. The task terminal (`routes/terminal.rs`), shells
  (`routes/shells.rs`), events (`routes/events.rs`) and browser (`routes/browser.rs`) sockets all
  re-check every `viewer_recheck_ms` (60 s by default) and close with 4003.
- Why tests missed it: `accounts.rs`'s
  `login_terminal_websocket_streams_masked_output_and_accepts_input` covers streaming, input,
  refusals at the handshake and the in-band refusal of an ended sign-in; no test revoked a sign-in
  while a login terminal was open. The #66 and SPEC-247 RMT-001 tests revoke sign-ins on the event
  stream and through `still_signed_in` directly.
- What else the cause touches: nothing else. Every other long-lived socket already re-checks; the
  login terminal is the only socket in `routes/` without a `recheck` interval (SPEC-286 found the
  same and reported it on #722).

## Requirements

- LTR-001: the login terminal re-checks the sign-in every `viewer_recheck_ms` (at least 100 ms),
  with `MissedTickBehavior::Delay`, in a spawned task beside the loop so output and input keep
  flowing while it waits for the store (as SPEC-118 KEY-005 does for terminals and shells).
- LTR-002: a re-check that finds the sign-in gone (`App::signed_in_user` is `None`: a signed-out,
  revoked or expired session, a revoked token, a disabled or deleted user) closes the socket with
  4003, reason `signed out`.
- LTR-003: a re-check that finds the person no longer an instance administrator (neither
  `is_superadmin` nor `is_instance_admin` in the current record) closes with 4003, reason
  `access revoked`.
- LTR-004: a re-check that itself fails (the spawned task panics) closes with 4003, as the shells
  do: a failed check proves nothing.
- LTR-005: a sign-in that still holds leaves the socket open; one person's revocation does not
  close another person's terminal on the same login.

## Plan

- [x] Spec written and validated
- [ ] Regression test fails on dev for session, token and admin loss
- [ ] Login terminal re-checks sign-in and administration and closes with 4003
- [ ] docs/api.md, SPEC-002 and changes.md describe the rule
- [ ] Independent review answered
- [ ] Gates green and pull request delivered

## Acceptance criteria

- With `viewer_recheck_ms = 200`, a login terminal opened with a cookie session closes with 4003
  after `POST /api/auth/logout` of that session (regression test).
- A login terminal opened with an operate API token closes with 4003 after
  `DELETE /api/me/tokens/{id}` (regression test).
- A login terminal of an administrator whose `is_instance_admin` the superadmin removes closes with
  4003 (regression test).
- Terminals of sign-ins that still hold stay open and keep answering (`hello` → `state`) after
  another one closed.
- The existing login terminal test still passes.

## Test plan

- `crates/yulius/tests/accounts.rs`, new test
  `a_login_terminal_closes_once_its_sign_in_or_administration_ends`: the fake process host runs a
  login that never exits; a harness with the fake host and `viewer_recheck_ms = 200`; alice
  (instance admin, cookie session and an operate token) and bob (instance admin, cookie session)
  open three terminals on the same login. Each step revokes one sign-in (alice's logout, then the
  token's deletion, then bob's demotion), waits by condition (bounded at 10 s) for that socket's
  close frame and asserts 4003, then sends `hello` on each socket still open and waits for its
  `state` answer.
- Fails on dev: the first wait times out with no close frame.

## Decisions

- Re-check instance administration too, not only the sign-in (the issue left it open). The socket
  is admin-only at the handshake, and an administrator can type into the provider's CLI (paste a
  code) and read its masked output; a demoted person keeping that for up to 15 minutes is the same
  gap as a signed-out one. It costs nothing: `App::signed_in_user` already returns the current
  record (SPEC-286), and the events stream reads the same flags at its re-check (#722). Closing
  rather than downgrading to read-only, because the socket has no read-only mode and the handshake
  refuses non-administrators outright.
- The verdict lives in the app layer (`App::login_terminal_recheck`, like `App::shell_recheck`),
  so the route only schedules it and closes; the rule that decides admin access stays next to
  `require_instance_admin`.
- Close code 4003 (the existing `CLOSE_GONE`, `CLOSE_REVOKED` in termsocket.js): the browser does
  not reconnect, which is right since a reconnect would be refused anyway. No client change.
- This follows the existing re-check pattern of the other sockets, so no new ADR.

## Evidence

(Filled in when the work is done.)
