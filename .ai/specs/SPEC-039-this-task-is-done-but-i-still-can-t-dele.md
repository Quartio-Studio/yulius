---
yulius-task: 019e228c-f94f-45cb-af47-0a4f111d1ab4
status: done
---

# SPEC-039: a Done task can be deleted; its session is closed first

Status: implemented
Date: 2026-10-12
Owner: Yulius task #37
Scope authorization: user request (F 2026-10-12, task #37): "This task is done but I still can't
delete it. I see 2 ways to handle that: done tasks still need to be stopped/closed. Then, it should
be clearly communicated. Or, done tasks = closed and then we should be able to delete it. Choose
what way is better and implement", with screenshots of task #27 in the sidebar's Done group (PR #263
merged, "Goal achieved") and its menu, where "Delete permanently…" is disabled with "Stop the task
before deleting it".

## Goal

A task that is Done (SPEC-001 TSK-014) can be deleted permanently from its menu, like any ended task:
its idle session is closed (`closed`, not stopped) as part of the deletion. Only a task that is still
at work, or waits for the user, has to be stopped first, and the menu says so in those words.

## Context

- TSK-014 keeps a Done task's Claude Code session attached and idle (typing in it makes the task
  Working again), so a Done task is still `running` on the server.
- TSK-009 deletion refuses live tasks (`DELETE /api/tasks/{tid}` → 409 "stop the task before deleting
  it"); the menu (`taskActions` in `status.js`) disables "Delete permanently…" for every live task with
  the tooltip "Stop the task before deleting it". The menu has a Stop item, but nothing tells the user
  that a Done task is still "running", and stopping a finished task records it as stopped (cancelled),
  which is not what happened.
- Archive already solved the same problem (F 2026-10-12, task #11): `archive_task` closes a Done
  task's session (`close_task_now`, `EndIntent::Close`, the task ends `closed`) as it archives it; the
  menu enables Archive for Done tasks by the sidebar's one rule (`taskInfo(...).group === 'done'`,
  server side `task_is_done`).
- `close_task_now` is asynchronous: it asks the session to shut down (interrupt, then TERM, then
  kill, at most `STOP_INTERRUPT_GRACE_MS + STOP_TERM_GRACE_MS + STOP_KILL_WAIT_MS` = 20 s, after which
  the host is terminated and the run marked lost). Deletion removes the worktree, run directories and
  records, so it must wait until the run has really ended (AGENTS.md: reap before releasing; a
  timeout is not proof a subprocess died).

Option chosen: **Done = closed, so it can be deleted** (the request's second option). A Done task has
nothing left to stop; asking the user to stop it adds a step, records a misleading "stopped" outcome,
and differs from Archive. The first option (keep the refusal and explain it) keeps the friction the
user reported.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DDL-001 | `DELETE /api/tasks/{tid}` on a running task that is Done (`task_is_done`, the sidebar's rule) closes its session (`EndIntent::Close`), waits until the run has ended and is reaped, then deletes the task as for a resting one (TSK-009): 204. | `task_done.rs` |
| DDL-002 | The close and the wait are bounded together: the wait ends when the task rests with no live attempt, observed through the task's update events (looking again every second in case an update is never published), and at most the close escalation plus a margin (25 s). A run that already ended by itself is not a conflict. If the run is still there at the bound, the answer is 409 "the task's session is still closing; try again" and nothing is deleted. | `task_done.rs` (closed well under the escalation), `task_delete.rs` (already ended) |
| DDL-003 | A running task that is not Done (working, waiting for the user, background shell still running) is still refused with 409 and nothing changes; read users still get 403. | `task_delete.rs` |
| DDL-004 | The task menu enables "Delete permanently…" for a Done task (the same rule as Archive). Its confirmation says the finished session is closed first. For a live task that is not Done, the disabled item says "Still running: stop the task before deleting it". The request may take the close's time (timeout above the server bound). | e2e `task-done.e2e.ts` |
| DDL-005 | SPEC-001 TSK-009 and TSK-014, `docs/api.md`, an ADR and `docs/changes.md` say that a Done task is closed as it is deleted. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Deleting a Done task closes its session, then deletes it (server and API)
- [x] Task menu offers Delete for Done tasks and says why otherwise
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-DDL-1: with the fake agent, a task whose goal is achieved and that has no shell left is Done;
  `delete_task` returns Ok, the task record is gone, its run has ended (`closed`, no live attempt) and
  its run directory is removed.
- AC-DDL-2: a running task that is not Done is refused with Conflict and stays.
- AC-DDL-3: in the UI, a Done task's menu has "Delete permanently…" enabled; confirming it deletes
  the task; a working task's item is disabled with "Still running: stop the task before deleting it".
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Integration (`crates/yulius/tests/task_done.rs`, real fake-agent sessions, like
  `a_done_task_can_be_archived`): "a Done task can be deleted: its session is closed first" — red
  before (Conflict), green after.
- `task_delete.rs`: the existing live-task refusal stays (DDL-003).
- e2e: the task menu for a Done task enables Delete and the confirmation calls `DELETE`; a working
  task shows the new tooltip.

## Decisions

- Done tasks are closed and deleted in one action, not stopped first: see the Context and
  [ADR-0031](../../docs/decisions/0031-deleting-a-done-task-closes-it.md).
- The server does close-then-delete in one request, waiting on the task's update events, rather than
  the UI closing and then deleting (two requests that a closed tab or a second viewer can interrupt,
  and an API that would need a separate close endpoint), or a persisted "delete when closed" flag
  (new state that must survive restarts, for a wait of a few seconds).
- The wait reuses the existing close escalation (20 s) plus 5 s for finalization; the UI request
  timeout for delete is raised to 40 s so the client does not give up first.
- The Done rule is `task_is_done`, the same one Archive and the sidebar use; no second rule.

## Evidence

- Red first: `a_done_task_can_be_deleted_its_session_is_closed_first` (`task_done.rs`) failed with
  `Conflict("stop the task before deleting it")` before the change and passes after; it asserts
  the task was announced `closed`, the delete took well under the 20 s escalation, the record and
  run directory are gone and the agent process no longer exists.
- Review fix, red first: `closing_a_task_whose_session_already_ended_is_not_a_conflict`
  (`task_delete.rs`) failed with a Conflict ("task is closed") and passes after.
- e2e `task-done.e2e.ts`: "a Done task can be deleted from its menu; a working one says it has to be
  stopped first" failed before the UI change (old tooltip, Delete disabled) and passes after.
- `scripts/dev ci` exit 0 on the rebased head: 1186 tests run, 1186 passed, 16 skipped; check,
  fmt, clippy, doctests, deny, machete, doc green.
- e2e full suite: 159 passed, 1 skipped (the live-model test).
- Independent review (fresh read-only session), round 1: APPROVE with minor findings. Fixed: the
  close is inside the bound, a run that ended by itself is not a conflict, a 1 s re-check, a
  stronger test, doc wording. Filed as follow-ups (already true for stop then delete): #284 (a failed
  terminate counts as a dead process), #285 (deletion races with the end of finalization for chains).
- Not covered: the 409 at the 25 s bound (needs a session host that never ends a run, see #284);
  a deployed instance was not exercised.
