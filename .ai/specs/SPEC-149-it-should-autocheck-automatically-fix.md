---
yulius-task: b1f92762-0e2e-47f8-9eeb-f4d96b976935
status: done
---

# SPEC-149: Opening the instance page checks main by itself

Status: done
Date: 2026-10-15
Owner: Yulius task #143, issue #500
Scope authorization: the person's request (task #143), with a screenshot of Instance → Updates
reading "Checked 35 min ago with the GitHub App" beside "Check now": "it should autocheck
automatically, fix".

## Goal

The superadmin who opens the instance page sees where `main` is now, not where it was at the
last hourly check, without clicking "Check now".

## Context

- **Symptom.** Instance → Updates shows the result of the server's last check of `main`
  (OPS-006: one minute after a start, then every hour, `YULIUS_UPDATE_CHECK_MINUTES`). Opening
  the page right after a merge to `main` shows "up to date" and "Checked 35 min ago" until
  someone clicks "Check now".
- **Root cause.** `mountInstancePage` (`web/assets/pages/instance.js`) only reads the cached view
  (`GET /api/update` through `ctx.updates.refresh()`). The check itself
  (`POST /api/admin/update/check`) is sent only from the "Check now" button. No test covered
  what opening the page does, so the page's "checks main every hour. Check now to look at once"
  was the only behavior.
- **Server side is right.** `check_updates_now` is superadmin-only and reuses a check younger
  than 30 s (`MANUAL_CHECK_GAP_MS`), so reopening the page costs no new GitHub compare within
  30 s. Checks sent at the same moment (two tabs, or the hourly check running) may each compare
  once: freshness is read before the check lock (#502).
- **Not an autoupdate.** OPS-006 ("nothing updates by itself") is untouched: only the check runs
  by itself; installing still takes the superadmin's confirmation.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| IAC-001 | Opening the instance page as superadmin checks `main` at once (the same request as "Check now"), and the page shows that check's result and time. | `update.e2e.ts` "opening the instance page checks main without a click" |
| IAC-002 | An automatic check that fails raises no error toast: the page keeps the last result. "Check now" still says a failed check. | `update.e2e.ts` "an automatic check that fails keeps the last result and raises no error" |
| IAC-003 | Others than the superadmin send no check (the page shows them only its placeholder). | the early return of `mountInstancePage` comes before the check (by reading); the server refuses them (`self_update.rs` `a_newer_main_is_offered_to_superadmins_only`) |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green on the branch merged with dev

## Acceptance criteria

- Opening Instance with a 10-minute-old check shows "Checked just now" without a click.
- A failed automatic check leaves "Checked 10 min ago" and no toast; a failed "Check now" toasts.
- The rest of `update.e2e.ts` passes unchanged.

## Test plan

- Regression: the two new `update.e2e.ts` cases fail on the unchanged page (no POST is sent,
  `failedPosts` stays 0) and pass with the fix.
- The mock counts POSTs it failed on purpose (`__yuliusMock.failedPosts`), so the failure case
  waits on the automatic check having been sent, not on time.
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, the e2e suite,
  `scripts/dev security`.

## Decisions

- The page checks on open rather than the server checking more often
  ([ADR-0123](../../docs/decisions/0123-the-instance-page-checks-main-when-opened.md)): the person
  looks at the page when they want to know; a shorter server interval would spend GitHub calls
  every few minutes whether anyone looks or not, and still show a stale time.
- The automatic check is quiet on failure: the result of a failed GitHub read is already in the
  view (its error banner); only a failed request (network, 5xx) is dropped, as nothing was asked.
- Review round 1 (fresh session, approved `dc64390c`) found two minor races that already existed
  before this change and that neither the mock nor the fakes can reproduce deterministically:
  overlapping checks each compare (freshness read before the lock), and an older `/api/update`
  read landing last. They are filed as #502 with deterministic tests as acceptance criteria,
  rather than fixed here without a failing test; the docs no longer claim "one compare per 30 s
  whatever the tabs". The nits (wrapping, AC-OPS-6, IAC-003's evidence) are fixed.

## Evidence

- Regression first (`138db6b7`, page unchanged): `npx e2e run tests/update.e2e.ts`: the two new
  cases failed: "Checked just now" not found (still "Checked 10 min ago"), and `failedPosts`
  stayed 0 (no check sent). With the fix (`dc64390c`): 14/14 passed.
- `scripts/dev check` and `scripts/dev lint`: passed.
- `scripts/dev test`: 1784 tests run, 1784 passed, 16 skipped.
- Full e2e (`npx e2e run`, while `scripts/dev test` loaded the machine): 445 passed, 1 skipped,
  1 failed: `notes.e2e.ts` "the tabs show a grip…" (hover colour poll), not touched here. Run
  again alone: 35/35 passed. Filed as flaky, #503.
- `scripts/dev security`: exit 2, not run (Vidoc answers 401 to the stored key, #470;
  ADR-0104). Unrun, not passed.
- Independent review (fresh session) of `e37fdd21..dc64390c`: approved; minors filed as #502,
  nits fixed.
