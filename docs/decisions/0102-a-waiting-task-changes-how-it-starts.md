# 0102. A waiting task changes how it starts in one step

## Status

Accepted. Specified in [SPEC-126](../../.ai/specs/SPEC-126-we-d-like-to-be-able-to-change-task-runn.md)
(SPEC-001 TSK-020); task #115.

## Context

A person asked to change a task's start "ofc not for already running", for example from "When I
decide" to running after another task. New task's start choice (Start now, Supertask, after a
task, a rule, When I decide, At a time…) existed only at creation. Two statuses wait before the
first run and hold no session: `chained` (a row in `task_chains`) and `prepared` (an optional row
in `task_schedules`, [0085](0085-a-prepared-task-waits-for-a-person-or-its-time.md)). The tick
and run ends evaluate chains, then release them with a compare-and-set on the status only, and a
rule's evaluation may ask GitHub for seconds. A rule judged only tasks created before the waiting
task.

## Decision

- One endpoint, `PUT /api/tasks/{tid}/start`, takes the creation's start-choice fields and checks
  them as creation does; none of them means Start now. Only `chained` and `prepared` tasks change.
- The store replaces the status, the chain row and the start time in one transaction, guarded by
  the status the change read. The domain names the moves (`ChainChosen`, `PreparedChosen`, or the
  existing releases for Start now).
- A release decided by evaluating a chain is a compare-and-set on that chain too (same
  predecessor, same rule), so an evaluation of the old chain never starts the task under a new
  choice. Start now is not guarded: the person asked for it.
- A rule judges the tasks that existed when it was written: the chain row's `created_at`, which
  equals the task's creation for rules given at creation.
- A wait may not wait for itself, directly or through chained tasks' predecessors and rules.
- The sandbox's bounds (32 chained, 32 prepared) count a move into the other wait, not a change
  within one.

## Alternatives

- **One endpoint per move** (re-chain, prepare, …). The composer's choices map one to one onto the
  creation fields; one endpoint shares its validation and the UI's menu.
- **Changing `queued` and `waiting_capacity` tasks too.** They are in line for a session and the
  queue drain may hand them a slot at any moment; changing them races the drain, and the request
  was for tasks not yet started. A queued task can still be cancelled.
- **Delete and recreate the task with the new choice.** It would lose the number, branch, steps,
  queued messages and audit trail.
- **Separate statements without the chain guard.** A tick holding the old chain could start the
  task under the new one.

## Consequences

- A blocked chained task ("Waiting for you") can be moved to another task or rule instead of only
  Start now or Cancel; the new chain starts unblocked.
- Agents cannot change a task's start through a tool yet (the request was the UI's).
- Rules given at creation keep their meaning; a rule written later may name tasks created after the
  waiting task.
