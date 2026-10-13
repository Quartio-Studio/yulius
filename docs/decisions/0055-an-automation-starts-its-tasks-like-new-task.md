# 0055. An automation starts its tasks like New task

## Status

Accepted. Specified in [SPEC-078](../../.ai/specs/SPEC-078-we-d-like-to-be-able-to-set-the-automati.md)
(task #70, SPEC-001 AUT-013).

## Context

People want an automation's tasks to start the way a task from New task can: queued as soon as a
session slot is free, after another task is done, when a start rule is true, or as a supertask.
Until now every task an automation created was simply queued. An automation recurs, while a
chain's predecessor ends once, and New task's chain refuses a predecessor that has ended.

## Decision

- An automation stores **one start choice** for every task it creates: start now (the default),
  supertask, after a task, or a start rule. In the app it is the enum `AutomationStart`, in the
  store three nullable columns with defaults (`supertask`, `after_task_id`, `start_rule`, schema
  version 18), and on the wire the same three fields as `CreateTask`, plus the read-only
  `after_task_number`. Old rows and clients read as start now.
- Create and update check it like New task (one choice at most; the task and the rule's task
  numbers belong to the project) and answer 422 otherwise. Each run checks it again, because
  tasks are deleted and chain bounds fill between runs; a failure ends the run `failed` with the
  reason.
- An update that sends the stored choice back unchanged keeps it without checking it again, so
  an automation whose task was deleted since can still be paused or renamed; a new choice is
  checked.
- A run creates its task through `create_task_ruled` with that choice. **After a task that has
  ended** becomes the rule `#N done`, with Done as the rule language defines it (TSK-014: for a
  closed task, a complete plan and its pull request merged, or open and green). For such a
  predecessor the rule is true, and the task starts at once. Otherwise it can never be true: a
  task that ended some other way, or a goal reached without a pull request and then closed.
  The task then waits for a person, as any rule that can no longer be true does.
- The previous-active skip is kept: a task waiting `chained` or `queued` is active, so a waiting
  task never piles up behind another one.
- Agents set it with `automation_create` / `automation_update` (`after_task` as a number,
  `start_rule`, `supertask`). In an update, giving any of them replaces the choice.

## Alternatives

- **A choice per run (Run now asks).** This adds a dialog nobody asked for. The request is to set
  the automation itself "as a normal task".
- **Keep after-task as a chain only.** Every run after the predecessor ended would fail with
  "#N has ended", and a daily automation would break after its first day.
- **Drop the skip for waiting tasks.** A daily automation whose rule stays false would stack a
  waiting task every day, up to the chain bound of 32.
- **One `start` text column.** The wire would need a new shape unlike `CreateTask`, and the store
  would need parsing. Three columns map directly and default cleanly.

## Consequences

- The builder and the agent tools offer the four New task choices. The list row and the sentence
  name the choice ("· supertask", "· after #12", "· when #12 done").
- A PUT replaces the start choice like every other editable field, so clients send it back. The
  Automations page's switch does.
- A rule written by an agent that the editor cannot show (parentheses, mixed and/or) is kept as
  it is. Opening it in the builder starts the rule editor empty, and saving replaces it.
- Known limits:
  - A rule names tasks by number. Deleting the newest task frees its number, and a stored rule
    then names the next task created with it. New task's creation-time guard does not apply,
    because the run's task is newer.
  - A predecessor that ends between a run's read and the chain's own check fails that one run
    with "#N has ended". The next run takes the rule path.
