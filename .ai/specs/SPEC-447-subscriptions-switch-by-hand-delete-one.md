---
yulius-task: 21d6ddc4-37f2-4832-bdeb-07002470f502
status: in-progress
---

# SPEC-447: Move a task to another subscription, remove one in use, never reuse a removed one

Status: in progress
Date: 2026-11-10
Owner: Yulius task #429
Scope authorization: the owner, verbatim (2026-11-09): "i just canceled subscription and didnt pay
for it so the tier is free. However, yulius ux did bad - it should change the subscription
automatically. When i tried to do it manually, it turned out i can't. I also couldn't delete
subscription because task was on it. When i stopped the task and deleted, the task resumed on the
old subscription. Total mess. Fix it." General: every sandbox, no setting. Builds on PR #1138
(the automatic part: a refused subscription becomes `relogin` and its task moves on). Amends
SPEC-001 ROT-007 (no per-task choice in the UI, notices never name accounts) and ROT-009 and
SPEC-057 SUB-003 (removal refused while a task works on it). ADR:
[0288](../../docs/decisions/0288-a-person-moves-a-task-between-subscriptions-and-removing-one-moves-its-tasks.md).

## Goal

A person can always get a task off a subscription: they move it to the subscription they choose
from the task itself, at any time, or they remove the subscription and its tasks move on by
themselves. A removed subscription is never used again. Each task shows the subscription it runs
on, a refused subscription says why on its card, and the task's terminal says in one line when and
why a task moved, or that it needs a person because nothing is left to move to.

## Context

Confirmed against `dev` at 770c2412.

- **No manual switch in the UI, by design.** `POST /api/tasks/{tid}/switch {account_id}`
  (`App::switch_task`, rotation's `Trigger::Manual`) works for a running or waiting task, but
  ROT-007 (F 2026-10-10, "tasks are subscription-agnostic") removed every per-task choice from the
  UI: only the mock answers `/switch`, and Task details shows the subscription read-only. Members
  could not even name a subscription to the API: `GET /api/status` gives account ids to
  administrators only. This is why the owner "can't" switch by hand.
- **Removal refused while in use.** `App::delete_account` answers 409 "used by N running task(s);
  stop or switch them first" while a task that is not Done has a live run on it (ROT-009,
  SPEC-057 SUB-003), and the Subscriptions card disables Remove.
- **"Resumed on the old subscription".** Every launch picks its subscription through
  `choose_account`, from the stored accounts only, so a Continue after a removal starts on another
  subscription: the owner's sequence (stop, remove, continue) run on 770c2412 continues on the one
  left (`subscription_moves::a_continued_task_never_runs_on_its_deleted_subscription` passed before
  any change). The exact path the owner saw could not be reproduced from the code. What could keep
  a removed subscription in use was a run that outlived the removal: before this change a run on a
  removed subscription was never ended by it (a refused removal left it running; a held process of
  a waiting task kept its credential), and nothing showed which subscription a task ran on, so a
  resumed session showing the old refusal looked the same. This spec ends every run on a removed
  subscription and shows each task's subscription, so the question can no longer arise unseen.
- **Moves are silent.** A switch says "↻ Yulius continues this session on another Claude
  subscription…" (ROT-007: notices never name accounts), so nothing said that a subscription was
  refused or removed, or that a task waits because none is left.
- **A refused subscription's card** shows `relogin` with Sign in again, and the provider's text is
  in `last_error`, but the card shows no reason.

## Requirements

| ID | Requirement |
| --- | --- |
| MOV-001 | A person with write access to a task moves it to a subscription they choose from the task view: the task strip shows the subscription it runs on as a button, which opens "Run on…" listing every subscription with its usage; the current one is marked and one that cannot take the task is disabled with why (needs sign-in, at its limit). Choosing one moves a running or waiting task (`waiting_capacity` included, with or without a process) to it: the next turn runs there in the same session (the switch API, `Trigger::Manual`). A resting task offers "Continue on…" with the same list (`POST /continue {account_id}`). A task a person paused answers "Resume the task first" (PER-005, unchanged). |
| MOV-002 | A person's choice is honoured for any signed-in, healthy subscription of the task's provider that is not at 100 % of a window binding the task: the pool's 98 % threshold does not refuse it. The pool still applies its rules from the next turn boundary. A subscription that cannot take it is refused with one plain sentence naming it and why. A moved idle task gets nothing typed; a task cut mid-turn by a person's move is told to continue where it left off. |
| MOV-003 | Removing a subscription always works, also while tasks run or wait on it: it is deleted first (nothing can choose it from then on), its Done tasks' sessions are closed as before, and every other task with a run on it moves to the best other subscription, in the same session, or waits for one. Every run on it ends; no resumed, restarted, continued or reattached run, and no stored run's account, ever starts on it again: a launch that chose it just before checks again once its run is recorded and chooses again, and a move waits for a start or switch under way. A task its person paused is stopped, never resumed by the removal, and one whose move is refused is stopped too, each saying why. The request answers once Done sessions are closed; moves go on after it. |
| MOV-004 | Each task shows the subscription it runs on: the task view's strip, the Tasks page list ("… · on <subscription>" in its status cell) and the sidebar row's tooltip; `TaskSummary.account_id` names it for the UI. `GET /api/status` gives every signed-in person each subscription's id and status, so the list in MOV-001 works for members. |
| MOV-005 | A subscription marked `relogin` because Claude refused it says why on its Subscriptions card: "Claude refused it: <provider's text>. Sign in again." |
| MOV-006 | When a task moves because its subscription was refused or removed, its terminal says it in one plain line naming both ("↻ Claude refused <name> (…): this session continues on <other>." / "↻ <name> was removed: this session continues on <other>."). When nothing usable is left (every subscription removed or needing a sign-in), the task waits and its terminal says so in one line, and the task is under **Needs you** with "Sign a Claude subscription in again or add one in Subscriptions", which ends by itself when the task runs again. A wait only for capacity stays a plain pause (ROT-010). |

## Plan

- [x] Spec, ADR and the owner's sequence reproduced as tests
- [x] Removing a subscription in use moves its tasks and never reuses it
- [x] A person's choice is honoured below 100 % and moves running and waiting tasks
- [x] Moved and stranded tasks say so in one line, with a Needs you when nothing is left
- [x] Task view and Tasks list show and change the subscription
- [x] The Subscriptions card says why Claude refused one and removal is always offered
- [x] Docs, product spec and changes
- [x] Review round 1 fixes: removal races, paused tasks, Needs you
- [ ] Gates green and independent review approved

## Acceptance criteria

- AC-1: stop a task on A, remove A, Continue: the run starts on B; the summary names B.
- AC-2: remove A while a task runs on it: A is gone, the task runs on B in the same session, the
  terminal names both.
- AC-3: remove the only subscription while a task runs: the task waits with no run, its summary has
  `waits_for_account`, it is under Needs you, and its terminal says nothing is left.
- AC-4: a person moves a running task from A to B: the next run is on B, same session.
- AC-5: a person moves a task waiting for capacity (A at 99 %) onto A: it starts on A. A move to a
  subscription at 100 % or needing a sign-in is refused with a sentence naming it.
- AC-6: a refused subscription's card reads "Claude refused it: …".
- AC-7: the task strip and the Tasks list show the subscription; "Run on…" moves the task (e2e).

## Test plan

- `crates/yulius/tests/subscription_moves.rs` (real processes, fake agent): AC-1 to AC-5, the
  refusal's notice (with `--access-lost`, after #1138's test).
- `yulius-domain` `rotation` unit tests: a person's choice below 100 % honoured, at 100 % refused.
- `supervision.rs` removal tests updated from "refused" to "moves".
- e2e (`e2e/tests/subscriptions.e2e.ts`): the strip's subscription button and "Run on…", the Tasks
  list column, the refused card's reason, Remove enabled while in use.

## Decisions

- **Remove = delete first, then move.** Deleting the row first makes every later choice skip it
  without a "deleted" flag; the moves reuse the switch path, so the session continues natively.
  Rejected: a soft-delete column (a migration for what the missing row already says), refusing
  removal (the owner's complaint).
- **A person's choice ignores the 98 % threshold, not the limit.** Otherwise a task waiting for
  capacity could never be moved by hand: the pool already moved it to anything below 98 %.
- **Ids in `/api/status` for everyone.** An account id is an opaque key, not a secret; emails,
  errors and in-use counts stay administrators' (UI-022). Rejected: a per-task choices endpoint
  (a second view of the same list).
- **Review round 1 (independent reviewer, request changes), all fixed test-first:** a launch that
  chose the subscription just before its removal ran on it (race point `AccountChosen`; re-check
  after the attempt is recorded); a removal during a switch stopped the task (retry on `Conflict`
  until the task settles, bounded by the switch timeout); a removal resumed a person's pause (it
  stops the task); the Needs you could overwrite the agent's request or outlive a resume; a usage
  reading erased the refusal's text; the menu offered `limited` subscriptions and marked the current
  one by name (`TaskSummary.account_id` added); `DELETE` answered an error after deleting.
- **Review round 2 (request changes, two minor), fixed:** a person's move to a subscription
  removed after the switch's preflight stopped the task (the launch now lets the pool choose;
  test `a_move_to_a_subscription_removed_meanwhile_goes_where_the_pool_chooses`); a Done session
  whose close failed stayed on the removed subscription (it now moves or stops like any other run;
  no deterministic test, see Evidence); the retry's deadline uses `tokio::time::Instant`. Left as
  is: a person's pause arriving between the paused check and the move is ended by the move (a
  window of one store read; the pause path itself is unchanged).
- **Notices name the subscriptions** for refusals and removals only (amends ROT-007): the owner
  asked to be told; routine pool moves stay silent about names.

## Evidence

(Filled in at the end.)
