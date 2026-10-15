---
yulius-task: d77e8c62-5100-42ca-a9f4-808194f0028c
status: in-progress
---

# SPEC-153: onboarding writes never undo a Skip; archive test and wording; a task that never ran is not Closed

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #126 (part of task #118, SPEC-138 lanes)
Scope authorization: the person's request (task #126): fix GitHub issues #406, #431 and #399, all
about the onboarding task's lifecycle. "Make the writes conditional (`UPDATE … WHERE
onboarding_state = 'pending'`) or take `onboarding_task_lock`"; "add a test that archives a live,
Done, onboarding-origin task […]. The 409 a task changing state mid-archive gets reads 'stop the
task first' […]; document that 409 on `archive_task`. Reflow the 163-character line in SPEC-002";
"a task with no attempts is not shown as Closed in the list, its tabs or its counts, and the
onboarding task of a `pending` sandbox says it waits for Start onboarding. Rust and e2e tests."

## Goal

- A Skip is never undone: no onboarding state write lands on a state it did not read. Start
  onboarding that was running while an admin pressed Skip leaves the sandbox `skipped` (#406).
- Archiving a live, Done onboarding task is covered by a test, and the rare "still changing"
  409 names the action generically and is documented (#431).
- A task that never ran does not read Closed: the onboarding task of a `pending` sandbox reads
  "Waits for Start onboarding", any other task without a run reads "Not started", and neither
  sits in the Closed group, tab or count of the task list (#399).

## Context

- `App::start_onboarding` (`crates/yulius-app/src/onboarding.rs`) starts the onboarding task
  (`start_run`, which prepares the worktree and the account and attaches the run), re-reads the
  sandbox and writes `preparing` through `set_onboarding_state`, which writes unconditionally
  (`SandboxStore::set_onboarding`). `skip_onboarding` writes `skipped` under
  `onboarding_task_lock`; `start_onboarding` and `refresh_onboarding` do not take it. A Start that
  runs while an admin presses Skip writes `preparing` over `skipped`: the banner comes back and
  new tasks wait again (#406, found in the review of SPEC-097).
- The review nit of #406 (the web mock creates its preparation task only after its own 422 checks,
  and not with a task to run after or a rule) was already done on `dev` by #405 (the mock's
  `preparation()` is called lazily after every 422 check, and `before || preparation()` skips it
  with a predecessor). On the server, `create_task_held` calls `onboarding_wait` (which creates
  the onboarding task) before its own prompt check, but every request path validates the prompt,
  mode, harness, workflow and predecessor first (`create_task_as`), so a refused request already
  creates nothing; a regression test now says so.
- `App::archive_task` (`tasks.rs`) closes a running Done onboarding task with
  `close_task_and_wait` so that `ensure_archive_allowed` sees the run's end (SPEC-106 OTR-004).
  No test covers it. `close_task_and_wait` (`supervisor.rs`) returns "stop the task before
  deleting it" when the task changes state mid-close, also for an archive; `archive_task`'s
  `# Errors` does not list the 409s of the close (#431).
- `create_task_held` (`lifecycle.rs`) stores a task created without a run (`start: false`) as
  `closed` (archived, except the onboarding task, which is listed). The web UI (`status.js`
  `statusInfo`) reads every `closed` task as "Closed" and the task list (`pages/tasks.js`
  `groupOf`) puts it in the Closed group and tab. Since SPEC-097 the onboarding task is created
  resting when a task is created in a `pending` sandbox: it reads "Closed · 0/N steps" (#399).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OLC-001 | Every onboarding state change is a compare-and-set from the state it was decided on (`SandboxStore::transition_onboarding`: `UPDATE … WHERE onboarding_state = <read>`); a change whose state moved meanwhile writes nothing and is not audited. `start_onboarding` writes `preparing` only from the `pending` it read before starting the task; `refresh_onboarding` returns the current state when its write lost. A skip is retried from the state it finds (at most 4 reads; states only move forward while it holds the lock), so a start or refresh landing between its read and its write never makes it fail. | Rust `task_done::a_skip_during_start_onboarding_stays_skipped`; store unit test `onboarding_transitions_only_from_the_state_read` |
| OLC-002 | A new task request refused for its prompt (empty or over 64 KiB) creates no onboarding task in a `pending` sandbox. Already true on `dev` (validated before the gate); kept by a regression test. | Rust `import_server::a_refused_new_task_creates_no_onboarding_task` |
| OLC-003 | Archiving a live, Done onboarding-origin task with a plan step open, in a `preparing` sandbox whose project validates, answers 200: the task is closed and archived and the sandbox `ready`. | Rust `task_done::a_done_onboarding_task_is_archived_and_the_sandbox_is_ready` (fails with `close_task_now`: the plan is open, so only the run's end makes the sandbox ready) |
| OLC-004 | The 409 for a task that is neither closed, resting nor stopping when an archive or a deletion closes it reads "the task is changing state; stop the task first" for both. `archive_task` documents its 409s: a live task that is not Done, a session still closing, a session that could not be ended, and the onboarding task of a blocking sandbox. | review of the text; `cargo doc` |
| OLC-005 | The task summary says whether a resting task never ran: `not_started` is true for a `closed` task with no attempt; `awaits_start_onboarding` is true when that task is the onboarding task of its `pending` sandbox. Both default to false (older servers). | Rust `import_server::onboarding_starts_only_from_the_browser` (pending: both true; after Start: both false), `import_server::a_skipped_onboarding_task_reads_not_started` |
| OLC-006 | The web UI reads such a task as "Waits for Start onboarding" (the Working group, like a prepared task waiting for a person) or "Not started" (with the stopped ones), never "Closed": not in the task list's Closed group, tab or count, and its time says "waiting" or "created", not "closed". The sidebar leaves `closed` tasks out, as before. | e2e `onboarding.e2e.ts` `the onboarding task a new task waits for reads Waits for Start onboarding, not Closed` |
| OLC-007 | SPEC-002's 163-character line (launch preparation, diff sidebar) is reflowed under 100 characters. | the diff |

## Plan

- [x] Spec and plan for #406, #431 and #399
- [x] Onboarding state writes are compare-and-set, so a Skip survives Start onboarding
- [x] A refused new task creates no onboarding task
- [x] A live Done onboarding task's archive is tested; the close 409 reads stop the task first
- [x] The task summary says a task never ran or waits for Start onboarding
- [x] The web UI shows Not started and Waits for Start onboarding outside Closed
- [x] Docs, ADR and changes
- [ ] Independent review and fixes
- [ ] Gates green, evidence and delivery

## Acceptance criteria

- With Start onboarding held inside `start_run` and an admin's Skip landing meanwhile, the sandbox
  ends `skipped` and the audit log has no `skipped -> preparing`. Fails without OLC-001.
- `transition_onboarding` from a state the sandbox no longer has changes nothing and says so.
- An empty prompt in a `pending` sandbox answers 422 and leaves the task list empty.
- OLC-003's test passes, and fails when `archive_task` uses `close_task_now` for it.
- The task list API of a `pending` sandbox with a waiting task shows the onboarding task with
  `not_started` and `awaits_start_onboarding`; after Skip, `not_started` alone; once started,
  neither.
- In the UI (mock), that task reads "Waits for Start onboarding" in the Working group, the
  Closed tab counts 0 and has no such row; after Skip it reads "Not started" under Stopped.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Integration (`task_done.rs`, real ptyhost behind `GatedHost`, fake agent): a `pending`
  sandbox; Start onboarding (browser context) is held at the run's attach; Skip runs and the
  sandbox reads `skipped`; the attach is released; Start onboarding answers; the sandbox is still
  `skipped` and no audit entry wrote `preparing` after the skip.
- Store unit test (`yulius-store` sandboxes): `transition_onboarding` from the current state
  changes it and returns true; from another state returns false and leaves it.
- Integration (`import_server.rs`): an empty prompt in a `pending` sandbox: 422, no task.
- Integration (`task_done.rs`): a goal task of origin onboarding, two plan steps open, the
  sandbox `preparing` with it as its onboarding task and the format scaffold in its clone; the
  fake agent achieves the goal (Done); `archive_task` returns, the task is closed and archived,
  the sandbox `ready`.
- Integration (`import_server.rs`): the existing pending-sandbox flow checks the summary fields
  before and after Start onboarding; a new test skips and reads `not_started` alone.
- e2e (`onboarding.e2e.ts`, mock): import a repository (mock leaves it `pending`), create a task;
  the Tasks page shows the onboarding task "Waits for Start onboarding" under Working, the Closed
  tab's count is 0; Skip; it reads "Not started" under Stopped. axe on the list.

## Decisions

- **Compare-and-set over the lock (OLC-001).** Taking `onboarding_task_lock` across
  `start_onboarding` would hold it across `start_run` (worktree, account, spawn and attach, which
  can take seconds and waits on the launch lock), blocking every skip and onboarding creation of
  every sandbox meanwhile. A conditional `UPDATE` makes every writer (start, refresh, skip) safe
  without new waits, and is checked by SQLite atomically. `set_onboarding_state` compares with
  the state of the record it was given, so callers keep reading once and deciding.
- **"Never ran" is a view of the stored task, not a new status (ADR-0126, OLC-005).** A new
  `TaskStatus` (or storing such tasks as `prepared`) would touch the state machine, the start
  paths ("a fresh start of an ended task is skipped", Continue's resume), onboarding's
  `is_resting` checks, the prepared scheduler and the schema. The summary already reads a task's
  run state; `not_started` costs one attempts read for `closed` tasks only, and
  `awaits_start_onboarding` one sandbox read for a never-run onboarding task only.
- **Where the UI puts them.** "Waits for Start onboarding" waits for a person's start, like a
  prepared task "When I decide" (SPEC-105), which sits under Working and is never "Needs you";
  the sandbox's onboarding page already asks for Start onboarding. "Not started" (an onboarding
  task after Skip, a continued session whose start failed) will not start by itself and has ended
  without running, so it sits with the stopped tasks, not under Closed.
- **The server, not the UI, decides `awaits_start_onboarding`.** The task rows (sidebar, list,
  task page) read only the task; the sandbox state there may be stale. The server knows both.
- **The mock nit of #406** was already done by #405. The server's matching order was checked
  (OLC-002): it holds, so only a regression test is added.
