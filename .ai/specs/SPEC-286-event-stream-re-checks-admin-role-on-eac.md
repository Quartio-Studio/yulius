---
yulius-task: 059cd6ce-a821-448c-86e1-ec99770d5027
status: in-progress
---

# SPEC-286: The event stream re-checks the admin role on each re-check

Status: in-progress
Date: 2026-10-24
Owner: task #272
Scope authorization: task #272, issue #722 (opened by task #230's independent review).

## Goal

An open `WS /api/events` stream follows the person's current instance administration: within one
re-check interval of the superadmin removing it, admin-only events (`account.updated`, which
carries subscription state, last error and the account email since SPEC-244) stop; within one
interval of granting it, they start, without a reconnect.

## Context

- Symptom: a person whose `is_instance_admin` flag is removed keeps receiving admin-only events on
  an open page until it reconnects.
- Root cause: `pump` in `crates/yulius-server/src/routes/events.rs` computes
  `is_admin = ctx.user.is_superadmin || ctx.user.is_instance_admin` once, from the `AuthContext`
  of the upgrade. The `recheck` arm calls `App::still_signed_in`, which loads the current user but
  only answers a bool, and clears the sandbox caches; the stale flag (and the stale `UserRecord`
  given to `can_view`/`can_work_in`) stays for the life of the socket.
- `PATCH /api/admin/users/{id}` (`App::update_user`) changes the flag without revoking sessions
  (only disabling does), so nothing else ends the stream.
- Why the tests missed it: the #66 test (`http_auth.rs`,
  `event_streams_follow_revoked_access_and_close_for_disabled_users`) covers revoked sandbox
  access and disabling, never a role change; no test published an admin-only event after a
  role change.
- What else the cause touches: only the event stream filters on the admin flag of a long-lived
  socket. The terminal, shell and browser sockets re-check sandbox access through
  `access_level`, which reads `is_superadmin` (the superadmin cannot be changed through the API).

## Requirements

- EVS-001: each re-check of `/api/events` reloads the person from the store; admin-only events
  are filtered with the reloaded `is_superadmin || is_instance_admin`, and sandbox visibility is
  decided with the reloaded record.
- EVS-002: losing the flag stops admin-only events after the next re-check; gaining it starts
  them after the next re-check; the stream stays open in both cases.
- EVS-003: a re-check that finds the person signed out or disabled still closes with 4003, as
  before (#66).

## Plan

- [x] Spec written and validated
- [x] Regression tests for losing and gaining the flag fail on dev
- [x] The re-check reloads the person and the stream filters with it
- [x] docs/api.md, SPEC-002 §8 and changes.md describe the rule
- [ ] Independent review answered
- [ ] Gates green and pull request delivered

## Acceptance criteria

- An instance admin's open stream gets no `admin_only` event once a re-check has run after the
  flag was removed (regression test).
- A non-admin's open stream gets admin-only events once a re-check has run after the flag was
  granted (regression test).
- The #66 test still passes (disabled users' streams close with 4003).
- docs/api.md and SPEC-002 §8 state the rule.

## Test plan

- `crates/yulius/tests/http_auth.rs`, with `viewer_recheck_ms = 150`: rounds of an admin-only
  `account.updated` followed by an everyone-visible marker event. Before the change the admin's
  stream delivers both; after `PATCH /api/admin/users/{id}` `{"is_instance_admin": false}` the test
  waits by condition (the first round whose marker arrives without the admin-only event, bounded
  by a deadline) and then asserts further rounds never deliver it. The reverse test grants the
  flag and waits for the first round that delivers it.
- Both fail on dev: the losing test times out with the admin event still arriving, the gaining
  test never sees it.

## Decisions

- Filter in place rather than closing the stream on a role change: it matches the #66 sandbox
  visibility re-check, needs no client change and loses no events of the reconnect gap. A close
  would make the page refetch everything for a change that only widens or narrows one event kind.
  This follows the existing re-check pattern, so no new ADR.
- `App::signed_in_user(&ctx) -> Option<UserRecord>` returns the reloaded user when the context
  still holds; `still_signed_in` becomes `signed_in_user(..).is_some()`, so the other socket
  re-checks keep their call unchanged.
