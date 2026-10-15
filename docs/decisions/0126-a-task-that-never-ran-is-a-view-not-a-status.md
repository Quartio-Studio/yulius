# 0126. A task that never ran is a view of its stored status, and onboarding writes are compare-and-set

## Status

Accepted. Specified in [SPEC-153](../../.ai/specs/SPEC-153-fix-406-431-399-onboarding-writes-never.md)
(issues #399, #406). Builds on [ADR-0077](0077-onboarding-task-is-created-when-needed.md), which
creates the onboarding task resting when a task waits for it.

## Context

A task created without a run (`start: false`) is stored `closed`. Since ADR-0077 the onboarding
task of a `pending` sandbox is created that way when another task waits for it, and stays in the
task list: the UI read it "Closed · 0/N steps", as if it had run and ended, and the task list put
it in its Closed group, tab and count (#399).

The sandbox's onboarding state was written unconditionally. Start onboarding starts the task
(seconds: worktree, account, spawn, attach) and then wrote `preparing`; an admin's Skip landing
meanwhile was overwritten, the banner came back and new tasks waited again (#406).
`skip_onboarding` already took `onboarding_task_lock`; the start and the refresh did not.

## Decision

- The task summary (`TaskSummary`) says when a task never ran: `not_started` for a `closed` task
  with no attempt, and `awaits_start_onboarding` when that task is the onboarding task of its
  `pending` sandbox. The stored status stays `closed`. The web UI reads them as "Waits for Start
  onboarding" (with the waiting tasks, like a prepared task waiting for Start now) and "Not
  started" (with the stopped tasks), never as Closed.
- Every onboarding state change is a compare-and-set from the state it was decided on
  (`SandboxStore::transition_onboarding`); a write whose state moved meanwhile does nothing.
  Start onboarding writes `preparing` only from the `pending` it read before starting; a skip
  reads again and retries (bounded) when a start or a refresh moved the state between its read
  and its write.

## Alternatives

- A new task status (`not_started`), or storing such tasks as `prepared`: the state machine, every
  start path ("a fresh start of an ended task is skipped", Continue's resume), onboarding's checks
  of a resting task, the prepared scheduler and the schema would change for a question of wording.
- The UI deciding from the sandbox's onboarding state: the task rows read only the task, and the
  sandbox state they see may be stale; the server knows both.
- Holding `onboarding_task_lock` across Start onboarding: it would be held through `start_run`
  (which also waits for the launch lock), blocking every skip and onboarding task creation of every
  sandbox meanwhile.

## Consequences

- A `closed` summary costs one attempts read (and, for a never-run onboarding task, one sandbox
  read); other statuses cost nothing more.
- The sidebar still leaves `closed` tasks out, so neither kind shows there.
- An older server sends neither field; the UI then reads `closed` as before.
- A lost onboarding write is not audited: the audit log has only the changes that happened.
