# 0054. A supertask holds its project at the one door every run starts through

## Status

Accepted. Specified in [SPEC-071](../../.ai/specs/SPEC-071-somewhere-here-the-user-should-be-able-t.md)
(task #67, SPEC-001 TSK-018).

## Context

People want to start broad work (security hardening, a large refactor) while the project stands
still: the running tasks finish, the broad task runs alone, and every other task of the project
waits for it, also tasks queued or chained before it. Tasks start in several ways: created, from
the queue at the session limit, released from a chain or a start rule, Start now, Continue, and
resumed by the capacity loop after a pause before they started. All of them end in
`App::start_run`. Live runs keep their slot; a Done task's idle session holds nothing (SES-101).

## Decision

- A task can be a **supertask** (`tasks.supertask`, chosen at creation in the composer's start
  menu, the API or `task_create`). It cannot also run after a task or a start rule.
- The hold applies in `start_run`, before the session limit, to every task without a live run:
  - any task waits while its project has a **pending** supertask (queued, starting, running and
    not Done, switching, or paused for capacity); a supertask waits only for one with a lower
    number;
  - a supertask also waits while its project has a **working** task (a live run that is not Done).
  A held task stays, or becomes, `queued`; nothing new in the state machine. Live runs are never
  stopped, paused or refused input.
- The queue offers free slots to supertasks first, then FIFO, one start after the other, so the
  first in that order takes a free slot. Starts were already serialized by the launch lock.
- The scope is the project (sandbox). Other projects keep running; they only share the session
  limit.
- A queued task's summary carries `held_by` (the numbers it waits for). The queue publishes a
  held task when that list changes.

## Alternatives

- **A new `held` status.** Clients, filters, counts and the state machine would all need to learn
  it, and every start path would need its own transition. `queued` with a reason says the same.
- **Pausing or stopping running tasks when a supertask arrives.** The request says running tasks
  keep running, and a forced stop loses a turn's work.
- **Instance-wide supertasks.** These would block other projects' unrelated code for no gain.
- **Supertask after a chain or a rule.** It could wait for a task its own hold keeps from
  starting, so the two would deadlock. It is refused instead.
- **Deriving the labels in the browser.** That duplicates the rule (Done, working, numbers) in
  JavaScript, which would drift.

## Consequences

- A supertask can wait as long as a running task that needs a person stays open: that is the
  intent ("the whole app should not change"), and the label names the tasks it waits for.
- The order among a project's supertasks is fixed by task number, so no cycle is possible: the
  earlier one never waits for a later one unless that one is already working.
- Each start of a task in a project that has supertasks reads the project's tasks and live
  attempts. That is cheap next to a launch. A project without supertasks stops after one read.
- A person typing into a Done task while a supertask runs makes it work again. That session was
  already running and is not held.
