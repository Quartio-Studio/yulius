# 0154. Paused is an overlay on a live run, set by an update or a person

## Status

Accepted. Specified in [SPEC-191](../../.ai/specs/SPEC-191-tasks-list-doesnt-refresh-automatically.md);
task #176. Extends [0131](0131-an-update-freezes-every-task-at-once.md).

## Context

An update freezes every live task (ADR-0131) and changes no task state, so the lists showed a
frozen task as Working. People also asked to pause a task themselves and resume it later. The only
"Paused" was `waiting_capacity`, a status of its own that the capacity controller owns (ROT-010).

## Decision

- **An overlay, not a status.** A paused task keeps `running` (or `switching`); its summary gains
  `pause: {by, user, since, pending}`: `update` while an update holds it and has not released it,
  `person` while someone's pause holds its live run (a person's pause wins). `pending` is true
  until the pause took hold: the run is frozen, or its hook was held in the last 15 s. The lists
  show the `st-paused` icon and "Paused", with why. An update publishes `task.updated` when its
  hold goes on and off, when a run is confirmed frozen, when a hook is first held and when a task
  is released.
- **A person's pause freezes the run as an update does.** `POST /api/tasks/{tid}/pause` (its owner,
  a running task) stores who, when and the live attempt (`tasks.paused_by`, `paused_at`,
  `paused_attempt`, migration 0033) and freezes the run under the update's freeze lock. A freeze
  the kernel does not confirm is asked again every 2 s, 5 times, then thawed; a run that cannot be
  frozen is held at its turn boundary (its `Stop` and `UserPromptSubmit` hooks, after the update's
  hold and before any rotation). `POST /api/tasks/{tid}/resume` clears it and thaws the run, except
  while an update holds the task: the update thaws it with the others.
- **Nothing else wakes it.** Every update thaw and the sweeps (the end of an update, every start)
  spare the runs people paused (`SessionHost::thaw_all` takes the runs to keep; nothing is swept
  when the pauses cannot be read), and a starting server freezes them again. A pause acts only on
  its attempt, so a later run is never paused by an old one.
- **Ending thaws first.** Stop, close and a switch thaw a paused run and clear the pause before they
  interrupt the agent; a manual switch of a paused task is refused ("Resume the task first").

## Alternatives

- **A `paused` `TaskStatus`.** Every reader of `running` (hooks, activity, chains, capacity,
  rotation, the supervisor's watches) and the state machine would have to learn it, for nothing
  the overlay lacks: the process lives, frozen.
- **Holding the hooks only.** A `/goal` turn lasts hours, so Pause would do nothing for hours.
- **Interrupting the agent.** It notices and loses its turn.
- **Reusing the capacity `pauses` table.** Its resume loop would resume the task and the Capacity
  card would count a person's pause as missing capacity.

## Consequences

- A paused run keeps its session slot and memory. There is no limit on how long it stays paused.
- Wall-clock timers run on while frozen: a tool call in flight longer than its timeout fails after
  the resume, and the agent handles it as any timeout. The provider stream open at the freeze is
  cut by the egress proxy and retried after the thaw (ADR-0131).
- What a person types into a paused task's terminal waits until it resumes.
- A run that cannot be frozen counts as paused once its agent is idle or asks a person (the
  update's pause point): if the person answers a permission prompt in the terminal, its turn goes
  on until its next `Stop` hook, which is held.
- Rollback: a binary without this change ignores the three columns (they stay, null or stale, and
  a later pause clears a stale one), and its start-up sweep thaws every paused run, so a revert
  resumes every paused task.
