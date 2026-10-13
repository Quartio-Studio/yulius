---
yulius-task: 17980e20-fdf3-44d3-836c-18b6525738b7
status: in-progress
---

# SPEC-071: supertasks run alone in their project

Status: draft
Date: 2026-10-13
Owner: Yulius task #67
Scope authorization: user request (task #67), with a screenshot of the new-task composer's
choices (Auto mode · Auto workflow · Start now): "Somewhere here the user should be able to set
'supertask' status of the task. When such task is submitted: (1) already running tasks keep
running and the supertask is queued after them (2) every task that is already queued or chained
will run after this supertask, supertask has a priority (3) after running tasks are finished this
supertask can launch and all other waits for him. It means running supertask can intentionally
only block the work of other people. It is for broad tasks like security hardening etc when we
prefer the whole app to not change. Implement."

## Goal

A person can start a broad task (security hardening, a large refactor) as a **supertask**: it
waits until the work already running in its project has finished, then runs alone. While it is
pending or running, no other task of the project starts, so the code it changes stays still.

## Context

- Tasks start through one door, `App::start_run` (`yulius-app/src/supervisor.rs`): a new task is
  `queued` and `start_run` launches it, or leaves it queued when the instance is at its session
  limit (`max_live_sessions`, SES-101). `drain_queue` starts queued tasks FIFO on every run end,
  every Done (TSK-014), every tick and after a restart. Continue (a resting task), the capacity
  loop (a task paused before it started, ROT-010) and released chains (TSK-012, SPEC-017) also
  go through `start_run`; switches (`switching`) do too but keep a live session.
- A slot is held by a working attempt: a live attempt of a task that is not Done
  (`working_attempts`, SES-101). Done tasks keep their idle session but hold nothing.
- Chained tasks (`chained`) wait for a predecessor or a rule, then become `queued` and start
  through `start_run`. Start now (`POST /api/tasks/{tid}/start-now`) releases one at once.
- The composer's third chip (`afterChip`, `web/assets/taskview.js`) is the start choice: Start
  now, After a task, When a rule is true. The task row and header read their state from
  `statusInfo`/`taskInfo` (`web/assets/status.js`).
- Agents create tasks with the MCP tool `task_create` (SPEC-066), which maps onto `CreateTask`.
- Projects (sandboxes) are independent: one sandbox's work never blocks another's except through
  the shared session limit.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| SUP-001 | A task can be created as a **supertask**: `CreateTask.supertask` (API, default false), the MCP tool `task_create` (`supertask: true`) and the composer's start chip ("Supertask"). It is stored with the task (`tasks.supertask`, schema version 17) and returned as `TaskSummary.supertask`. A supertask starts after the running work, so it cannot also run after a task or a rule: `supertask` with `after_task_id` or `start_rule` is refused (422). | int `supertask.rs`, e2e |
| SUP-002 | While a supertask of a project is **pending** — `queued`, `starting`, `running` (not Done), `switching`, `waiting_capacity`, or `stopping` until its process is reaped — no other task of that project starts a run: a new task, a released chain, and a continued task wait `queued`; a task paused for capacity before it ran keeps its pause (with its kind and continuation) until the hold ends. Runs already live are never stopped or paused: a Done task typed into again keeps working. | int `supertask.rs` |
| SUP-003 | A pending supertask starts only when no other task of its project is **working** (a live run that is not Done, SES-101) and no earlier pending supertask (lower number) is waiting; it then starts like any queued task (session limit, capacity). Without a working task it starts at once. | int `supertask.rs` |
| SUP-004 | Priority: when the queue is drained, queued supertasks are offered a free session slot before other queued tasks (instance-wide), then FIFO. One queue pass runs at a time (a call during a pass makes it go once more), and a task is started from the queue only while it is still queued under the launch lock. | int `supertask.rs` |
| SUP-005 | The hold ends when the supertask is Done, closed, stopped, failed, archived or deleted: the project's waiting tasks then start in their usual order (chains after the supertask start when it is Done). A continued supertask is pending again. | int `supertask.rs` |
| SUP-006 | Tasks say why they wait: `TaskSummary.held_by` lists, while a task is `queued` because of a supertask, the task numbers it waits for (a supertask: the earlier supertasks and the working tasks; another task: the pending supertasks). The row and header read "Supertask · waits for #3 and #5" / "Queued after supertask #7"; a supertask shows a "Supertask" mark in its header. The labels follow the change without a reload: every queue pass refreshes each queued task's hold, also at the session limit, and publishes the task when it changed; creating, stopping, deleting or failing to start a supertask runs a pass. | int `supertask.rs`, e2e `supertask.e2e.ts` |
| SUP-007 | SPEC-001 (TSK-018), the API doc, an ADR, the architecture document, the product summary and `docs/changes.md` describe supertasks. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A task is created and stored as a supertask through the API and the task_create tool
- [x] A pending supertask holds its project's starts and waits for the running tasks
- [x] The composer offers Supertask and waiting tasks say what they wait for
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-SUP-1: `POST /api/sandboxes/{sid}/tasks` with `supertask: true` returns `supertask: true`;
  with `after_task_id` or `start_rule` too it answers 422 and creates nothing; `task_create` with
  `supertask: true` creates a supertask.
- AC-SUP-2 (fake agent, real processes): task A runs; supertask S is created → S stays `queued`
  with `held_by = [A]`; task B is created → B stays `queued` with `held_by = [S]`; A is stopped →
  S starts while B stays queued; S is stopped → B starts.
- AC-SUP-3: without a working task a supertask starts at once; a Done task does not hold it.
- AC-SUP-4: a task of another project starts while a supertask holds its own project.
- AC-SUP-5: with the session limit reached, a supertask queued after other queued tasks gets the
  next free slot first.
- AC-SUP-6 (mock UI): the start chip's menu has "Supertask"; picking it shows the chip
  "Supertask" and the created task's request carries `supertask: true` and no `after_task_id`;
  a queued supertask reads "Supertask · waits for #N" and a task held by it "Queued after
  supertask #N".
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- `crates/yulius/tests/supertask.rs` (fake agent, real ptyhost, like `supervision.rs`):
  AC-SUP-2 .. AC-SUP-5, each red before the scheduler slice.
- API tests (`crates/yulius/tests/supertask.rs` through the HTTP router support): AC-SUP-1,
  including the 400s and the MCP tool.
- Unit tests of the pure hold rule in `yulius-app/src/supertask.rs` (who holds whom, ordering of
  supertasks, Done and resting tasks not holding).
- e2e `e2e/tests/supertask.e2e.ts` (mock): AC-SUP-6.
- Store migration covered by the store tests (round trip of the new column).

## Decisions

- **Scope is the project (sandbox), not the instance.** "We prefer the whole app to not change"
  is about the repository being worked on; other projects have their own code and keep running.
  The shared session limit still applies; SUP-004 gives the supertask the next slot.
- **Gate inside `start_run`, not a new status.** Every new run already goes through it, so one
  check covers new tasks, released chains, Continue and capacity resumes. A held task simply stays
  (or becomes) `queued`; no state-machine change, no new wire status for clients to learn.
- **Live runs are never touched.** "Already running tasks keep running": the supertask waits for
  them to be Done or ended, and nothing interrupts a session (also a Done task typed into again).
- **Supertask excludes After/Rule.** A supertask waiting for a chain could wait for a task its own
  hold keeps from starting (a deadlock); its meaning is "after the running work" anyway.
- **Order among supertasks by task number.** An earlier pending supertask goes first; a later one
  holds no earlier one unless it is already working, so no cycle is possible.
- **Review round 1** (fresh-session review, REQUEST CHANGES): labels and starts now follow a
  hold change at once (a queue pass refreshes every queued task's hold before the session
  limit, and a created, stopped, deleted or failed supertask runs one); the queue pass is single
  flight; a queued start is re-checked under the launch lock; a second start of a running task is
  a no-op (it raced and told the terminal "Could not start"); a held capacity resume keeps its
  pause instead of losing its continuation; `stopping` counts as pending; 422, not 400.
- **Labels from the server.** `held_by` is computed where the rule lives and republished when it
  changes during a queue pass, so the client never re-derives the rule.
- Recorded as ADR-0054.
