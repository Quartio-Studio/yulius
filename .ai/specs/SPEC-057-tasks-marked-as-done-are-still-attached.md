---
yulius-task: 4903d493-783f-4f20-8128-f7b46d451379
status: draft
---

# SPEC-057: Done tasks no longer hold their subscription

Status: draft
Date: 2026-10-13
Owner: Yulius task #48
Scope authorization: user request: "Tasks marked as done are still attached to subscribtion. Fix."
(screenshot of the Accounts page: cards saying "5 tasks on this subscription", "6 tasks on this
subscription" with **Remove** disabled, while those tasks are Done).

## Goal

A task that is Done (TSK-014) no longer counts on its subscription: the subscription cards say
"No tasks right now" when only Done tasks used it, the pool spreads new work as if those idle
sessions were not there, and the subscription can be removed.

## Context

- **Symptom.** Accounts page (`crates/yulius-server/web/assets/layout.js` `accountUsage`,
  `pages/admin.js`): "N tasks on this subscription" counts `in_use_by`, and **Remove** is disabled
  while it is above zero. `DELETE /api/admin/accounts/{id}` refuses with 409 the same way.
- **Root cause.** `in_use_by` is `accounts::in_use_count` over `store.live_attempts()`: every
  attempt in `spawning`/`live`. A Done task keeps its session attached and idle by design
  (TSK-014, so a follow-up prompt continues it), so its attempt stays `live` forever and the
  subscription stays "in use" until someone stops or archives the task. The same raw list feeds
  the Status panel (`status.rs`), the capacity view (`capacity.rs`), the account delete guard and
  the rotation's per-account load (`App::account_views`, `rotation::score`).
- **Precedent.** SPEC-050 SES-101 already stopped counting Done tasks against the session limit
  with `App::sessions_in_use`, using `task_is_done_locally` (no GitHub call). The subscription
  counts were not changed with it, which is why the existing tests missed this: they only insert
  attempts of working tasks (`accounts::account_crud_permissions_usage_and_in_use_guard`).
- **Closing a Done session** is already defined for archive and delete (TSK-009, TSK-014,
  SPEC-039): `close_task_and_wait` closes it (`closed`, not stopped) and waits for the run to end.

## Requirements

| ID | Requirement | Test |
| --- | --- | --- |
| SUB-001 | A subscription's `in_use_by` (Accounts page and API, account events, Status panel, capacity view) counts the live attempts of tasks that are not Done, by the session limit's rule (`task_is_done_locally`, SES-101). A Done task typed into again is Working and counts again. | `supervision::a_done_task_lets_its_subscription_go` |
| SUB-002 | The pool's per-subscription load (`AccountView::in_use` used by `rotation::score`) counts the same tasks, so idle Done sessions do not push new work away from a subscription. | covered by the shared helper of SUB-001 |
| SUB-003 | Removing a subscription is refused (409) while a task that is not Done uses it, as before. When only Done tasks use it, their sessions are closed first (`closed`, as archiving does, TSK-014) and the subscription is then deleted; a Done task whose session does not end in time keeps the subscription (409). | `supervision::a_done_task_lets_its_subscription_go` |

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Subscription cards and rotation count only working tasks
- [ ] Removing a subscription closes its Done tasks' sessions
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Docs, gates and delivery into dev

## Acceptance criteria

- A subscription whose only task is Done shows "No tasks right now" and an enabled **Remove**;
  `GET /api/admin/accounts`, `GET /api/status` and `GET /api/capacity` report `in_use_by: 0` for it.
- `DELETE /api/admin/accounts/{id}` with a working task on it answers 409 and keeps the account;
  with only a Done task it closes that task (`closed`, no live attempt) and deletes the account.

## Test plan

- Regression test `supervision::a_done_task_lets_its_subscription_go` (real fake agent): a working
  task counts and blocks removal; after its activity becomes Done the Accounts list, the Status
  panel and the capacity view say 0, and removal closes the task and deletes the subscription.
  It fails on `dev` (in_use_by stays 1, removal is refused).
- Existing `accounts::account_crud_permissions_usage_and_in_use_guard` keeps the working-task guard.
- Full gate: `scripts/dev ci`.

## Decisions

- **Count by the session limit's rule, not a new one.** A second notion of "busy" would drift from
  SES-101; reusing `task_is_done_locally` keeps the sidebar's Done, the session limit and the
  subscription cards in agreement. Recorded in ADR-0040.
- **Close Done sessions on removal rather than leaving them on a deleted subscription.** A session
  left running would keep using a credential Yulius no longer has and report usage for an account
  that no longer exists; closing is what archiving a Done task already does, and **Continue**
  starts it again on another subscription. Alternative rejected: refuse removal while Done tasks
  use it (the reported bug).
- **Not detaching Done sessions automatically.** TSK-014 keeps a Done session attached for
  follow-ups; closing every Done task would change that contract and lose the idle session.
