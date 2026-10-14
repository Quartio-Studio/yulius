# 0085. A prepared task waits for a person or its time

## Status

Accepted. Specified in [SPEC-105](../../.ai/specs/SPEC-105-add-scheduling-when-i-decide-in-this-mod.md)
(SPEC-001 TSK-019); task #90.

## Context

A person asked for "scheduling + 'when I decide' (in this mode tasks have special status
'prepared' and can be sent after clicking a button)". New task's start menu already offers Start
now, Supertask, a task to run after and a start rule; every one starts the task by itself. Tasks
that wait before their first run are `chained`: a row in `task_chains` names a predecessor or a
rule, and a chain whose predecessor ended otherwise needs a person.

## Decision

- A new task status, `prepared`: the task is written and holds no session or worktree. Start now
  (`POST /api/tasks/{tid}/start-now`, the chained task's endpoint) and its time release it
  `prepared → queued` by compare-and-set, then it starts like any new task. Cancel (`/stop`) ends
  it `stopped(cancelled)`. It sits with queued tasks in the sidebar, never in Needs you.
- Its time lives in its own table, `task_schedules` (schema version 24), one row per scheduled
  task; "when I decide" has none. `PUT /api/tasks/{tid}/schedule` moves or clears it.
- The background tick, every minute, releases the due tasks; the tick already runs after a
  restart, so a time passed while Yulius was down starts the task then. A due task of a sandbox
  whose onboarding still blocks tasks waits for a later tick.
- A prepared task has one way to start: not together with a task to run after, a rule or a
  supertask. At most 32 wait per sandbox; a time is at most a minute past and 366 days ahead.

## Alternatives

- **A kind of `chained`** (a chain row with no predecessor and no rule). It would reuse Start
  now and the cancel path, but `chained` means "waits for other tasks" everywhere (the sidebar's
  labels, blocked reasons, the rule pass), and the request asked for a status of its own.
- **A `start_at` column on `tasks`.** Every `TaskRecord` would carry a field only prepared tasks
  use; `task_chains` set the precedent of a side table for how a task starts.
- **A timer per scheduled task.** Exact to the second, but timers must be rebuilt after a restart
  and cancelled on every change; the picker has minute precision, which the tick matches.
- **Scheduling through automations.** They repeat on a cron schedule; a one-off task at a time
  would be an automation that disables itself, and "when I decide" has no time at all.

## Consequences

- A scheduled task starts up to a minute after its time. The tick releases it in one statement
  that checks its status and its stored time together, so a time moved or cleared while the tick
  runs wins; a person's request starts only the task it is about.
- The 32-task bound is counted before the insert, as the chain bound is: concurrent creations can
  pass it by a few, which keeps it a bound on growth, not an exact limit.
- Code that lists waiting states (queued messages, the disk controller's idle owners,
  `tasks_list`'s active scope, chain candidates) names `prepared` too.
- Automations cannot create prepared tasks yet; editing a prepared task's prompt goes through
  queued messages (SPEC-049).
