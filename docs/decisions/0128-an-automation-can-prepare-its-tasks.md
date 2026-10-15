# 0128. An automation can prepare its tasks ("When I decide")

## Status

Accepted. Specified in [SPEC-155](../../.ai/specs/SPEC-155-fix-416-automations-can-create-prepared.md)
(task #142, issue #416). Extends [decision 0062](0062-an-automation-starts-its-tasks-like-new-task.md)
with the prepared status of [decision 0085](0085-a-prepared-task-waits-for-a-person-or-its-time.md).

## Context

New task can prepare a task: it waits `prepared`, holding no session and no worktree, until a
person presses Start now (SPEC-105). An automation's start choice (ADR-0062) offered start now, a
supertask, after a task and a rule, but not this. A person who wants the day's task written each
morning, and sent when they are ready, had no way to say so.

## Decision

- The automation's start choice gets a fifth value, **When I decide** (`AutomationStart::Prepared`).
  On the wire it is the flag `prepared` on `Automation` and on Run now's `RunAutomation`, as on
  `CreateTask`; in the store it is the column `automations.prepared` (migration 0031, default 0),
  beside the three columns of migration 0019.
- It stands alone: with `supertask`, `after_task_id` or `start_rule` it is refused (422), with the
  same reason New task gives.
- Each run creates its task through `create_task_held` with no start time, so the task is
  `prepared` and waits for Start now, with SPEC-105's bound of 32 prepared tasks per sandbox (a
  full sandbox fails the run with the reason).
- The previous-active rule is unchanged: a prepared task is active, so a run while the previous
  run's task still waits is `skipped_previous_active`. One waiting task at a time per automation.
- The editor's start menu offers When I decide without "At a time…"; the agent tools take
  `prepared` like the other start arguments.

## Alternatives

- **Also "At a time…" relative to the run.** The cron schedule already is the automation's time;
  a second time per task is a second schedule nobody asked for.
- **A prepared task per run even while one waits.** A daily automation left alone for a week
  would stack seven unstarted tasks toward the bound of 32; the person needs only the latest.
- **One `start` enum on the wire.** It would read unlike `CreateTask` and `ChangeStart`, which
  speak in flags, and break clients that send the three existing fields.

## Consequences

- An automation can write its task on schedule and leave sending it to a person; the sidebar
  shows it in the in-line group like any prepared task, with Start now and Cancel.
- Clients that never send `prepared` keep their stored choice only when they send it back: a PUT
  without it is start now, as for the other start fields (ADR-0062).
- A prepared automation task skips the onboarding wait when created and waits for onboarding when
  started, as New task's prepared tasks do.
