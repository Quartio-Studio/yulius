# 0288. A person moves a task between subscriptions, and removing one moves its tasks

## Status

Accepted. Specified in [SPEC-447](../../.ai/specs/SPEC-447-subscriptions-switch-by-hand-delete-one.md)
(task #429). Amends SPEC-001 ROT-007 (no per-task choice in the UI, notices never name accounts) and
ROT-009, and [ADR-0040](0040-done-tasks-release-their-subscription.md)'s refusal to remove a
subscription a working task uses. Builds on PR #1138, which marks a subscription Claude refuses as
`relogin` and moves its task on.

## Context

The owner cancelled a Claude subscription (2026-11-09). Claude Code answered every turn with "Your
organization has disabled Claude subscription access for Claude Code"; #1138 now recognises that.
The rest of what the owner met is this decision's subject:

- **No way to switch by hand.** `POST /api/tasks/{tid}/switch {account_id}` worked, but ROT-007
  (F 2026-10-10, "tasks are subscription-agnostic") had removed every per-task choice from the UI,
  and members did not even get account ids from `GET /api/status`.
- **No way to remove the subscription.** Removal answered 409 while a working task used it
  (ADR-0040), which is exactly when a broken subscription must go.
- **Nothing said what happened.** A switch said "continues this session on another Claude
  subscription…" without names, the task view did not show its subscription, and a subscription's
  card showed no reason for its state.

## Decision

- **A person's choice.** The task view's strip shows the task's subscription as a button; "Run #N
  on…" lists every subscription with its use (its own marked, one needing a sign-in or at 100 %
  disabled with why). A running or waiting task (`waiting_capacity`, with or without a process)
  moves through the switch path, `App::move_task`, which goes on as the task stands: past a limit
  (SPEC-034), the first message again (SPEC-068), a held prompt typed again, a turn cut mid-way
  told to go on, nothing typed into an idle one. An ended task continues on the choice. The
  rotation's `Trigger::Manual` honours any healthy subscription below 100 % of every window
  binding the task: the pool's 98 % threshold no longer refuses a person, the pool's rules apply
  again from the next turn boundary. A refusal is one sentence naming the subscription.
- **Removing always works.** `DELETE /api/admin/accounts/{id}` deletes the row first, so no later
  choice can pick it, then closes Done tasks' idle sessions on it (ADR-0040) and moves every other
  task with a run on it to the pool's choice, in the same session, or lets it wait. Every run on
  it ends: a launch that chose it before the deletion checks again once its run is recorded and
  chooses again; a move waits for a start or switch under way; a task its person paused (an
  administrator never resumes it) or whose move is refused (ROT-014) is stopped, saying why. The
  request answers once Done sessions are closed; the moves go on after it.
- **Said in one line.** A task that moved because its subscription was refused or removed says so
  in its terminal, naming both ("↻ Claude a was removed: this session continues on Claude b.").
  When no usable subscription is left, the line says so and the task gets a Needs you ("Sign a
  Claude subscription in again or add one in Subscriptions"), ended by Yulius when its pause ends.
  A wait only for capacity stays a plain pause (ROT-010).
- **Visible.** The Tasks list and the sidebar's tooltip name each task's subscription; a `relogin`
  card reads "Claude refused it: <Claude's words>. Sign in again."; `GET /api/status` gives every
  signed-in person each subscription's id (emails, errors and counts stay administrators').

## Alternatives

- **A soft-deleted subscription** (a `deleted_at` column every choice filters): a migration for
  what the missing row already says, and one more filter to forget.
- **Keep refusing removal and only add the manual switch**: the owner removed the subscription
  precisely because it was broken; moving the tasks is what they asked for.
- **Honour a person's choice only under the 98 % threshold**: a task waiting for capacity could
  then never be moved by hand, since the pool already uses every subscription below it.
- **A per-task endpoint listing the choices**: a second view of the subscription list; the status
  view already lists them for everyone.

## Consequences

A person is never stuck on a subscription: they move the task or remove the subscription. The
owner's sequence (stop, remove, continue) runs on the subscription left, as before, and now also
the paths around it end every run on a removed subscription. A task moved by hand onto a nearly
full subscription may pause again at its next turn boundary by the pool's rule. Account ids reach
members (and `TaskSummary.account_id`); they are opaque keys, not credentials. The request to
remove a subscription answers before its tasks have moved; each says so in its terminal.
