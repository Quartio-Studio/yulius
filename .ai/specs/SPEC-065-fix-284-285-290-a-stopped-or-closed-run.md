---
yulius-task: 91b2234a-ae68-46d7-82c8-169a23821653
status: in-progress
---

# SPEC-065: a stopped or closed run ends only when proven gone; deleting a Done task is consistent

Status: draft
Date: 2026-10-13
Owner: Yulius task #54
Scope authorization: task #54, part of task #47 ("fix all open actionable GitHub issues"): fix
GitHub issues #284, #285 and #290 by their acceptance criteria.

## Goal

A run that Yulius stops or closes is recorded as ended only once its host and agent are proven
gone, and deleting a Done task (SPEC-039) waits for the run's whole finalization, refuses at once
what it cannot close, and cannot be sent twice from the web UI.

## Context

- SPEC-039 / ADR-0031: `DELETE /api/tasks/{tid}` on a Done task calls `close_task_and_wait`, which
  closes the session (`close_task_now`) and waits, bounded by the close escalation plus
  `CLOSE_SETTLE_MS` (25 s), until the task rests with no live attempt.
- #284: after the escalation, `close_task_now`, `stop_task_now` (and `resolve_lost_connection`)
  call `session_host.terminate` and ignore its result, then `mark_lost`. `terminate_recorded` fails
  when a signal cannot be delivered or a process survives SIGKILL; the attempt is still recorded
  as lost, so deletion removes the worktree while the agent may still run. AGENTS.md: "A timeout is
  not proof a subprocess died. Reap children before releasing execution capacity."
- #285: `close_task_and_wait` returns once the store shows the run ended, while `finalize_attempt`
  or `mark_lost` still run `end_pause`, `hub.exit`, `publish_task`, `advance_chains` and
  `drain_queue`; `delete_task`'s `block_successors_deleted` and `advance_rule_chains` race with them.
- #290: (1) the 1 s re-check is a `timeout` around each `recv`, restarted by every event on the bus;
  (2) the 25 s bound can cancel `close_task_now` while it waits on the run lock or a blocked
  shutdown write, before the escalation is spawned; and a blocked write holds the run lock, which
  `finalize_attempt` and `mark_lost` also need; (3) a task that turned `switching` waits 25 s for
  "still closing"; (4) `deleteTask` has no in-flight guard and no progress; (5) a `stopping` task's
  Delete says "Still running: stop the task before deleting it".
- Checked on `dev` (328ea6e): none of the three is fixed.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| END-001 | When a stopped or closed run did not exit after the escalation, its host is terminated; only when `terminate` succeeds is the attempt marked lost. When it fails, the attempt stays live, the failure is logged, said in the task's terminal and (a running task) flags the task as needing attention. The same holds when the connection to a run whose host is alive is lost. | `task_delete.rs` |
| END-002 | Closing a task for deletion whose run could not be terminated answers 409 at once with "the task's session could not be ended: …" and deletes nothing. | `task_delete.rs` |
| END-003 | Each task counts the run endings being recorded (`finalize_attempt`, `mark_lost`): the count rises before the attempt is marked ended and falls when all of its work (chains, queue) is done. `close_task_and_wait` succeeds only when the task rests, has no live attempt and no ending is being recorded, within the same bound as before. | `task_done.rs` |
| END-004 | The wait looks again every second on a fixed interval that other events do not reset. | `task_delete.rs` |
| END-005 | The escalation is armed before the run lock is taken, and the shutdown write is bounded (2 s), so neither a cancelled wait nor a blocked writer leaves a stop or close without escalation, nor holds the run lock that ending the run needs. | `task_done.rs` |
| END-006 | When the close is refused because the task is not running, its status is read again: a task that is neither resting nor stopping is refused at once with "stop the task before deleting it". | `task_delete.rs` |
| END-007 | The web `deleteTask` sends one DELETE per task at a time and shows "Deleting #N…" while it runs; a `stopping` task's disabled Delete says "Stopping…". | e2e `task-done.e2e.ts` |
| END-008 | The escalation time is configuration (`AppConfig::stop_escalation_ms`, default interrupt + TERM + kill = 20 s) so tests can shorten it. | tests |

## Plan

- [x] Spec SPEC-065 written and validated
- [ ] A run whose host cannot be terminated stays live, is reported, and blocks deletion (#284)
- [ ] Closing for deletion waits for the run's finalization to complete (#285)
- [ ] Close re-check uses a fixed interval, the escalation is spawned first and the shutdown write is bounded (#290 1-2)
- [ ] A switching task's deletion is refused at once (#290 3)
- [ ] Web UI: one DELETE in flight with "Deleting #N…", a stopping task's Delete says "Stopping…" (#290 4-5)
- [ ] Independent review in a fresh session, findings fixed
- [ ] Docs, ADR, changes log and every gate green

## Acceptance criteria

- AC-END-1 (#284): with a session host whose `terminate` fails, a stopped task stays `stopping`
  with a live attempt and a terminal notice, and `DELETE` answers 409 and deletes nothing; once
  `terminate` succeeds, stopping again ends the run. A closed task's `close_task_and_wait`
  answers Conflict at once, its attempt stays live and it needs attention.
- AC-END-2 (#285): a task chained after a closed task whose run ending is held (the run lock is
  busy) has its outcome decided when `close_task_and_wait` returns, and deleting the predecessor
  then gives it the one outcome "#N was deleted".
- AC-END-3 (#290): the re-check fires while unrelated events keep arriving; with a session writer
  that blocks, the close still escalates and the run ends; a switching task is refused at once;
  e2e: a double confirm sends one DELETE, a stopping task's Delete says "Stopping…".
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- `task_delete.rs` (in-memory harness, seeded live attempt, a session host whose `terminate` fails
  or succeeds on demand, escalation shortened): stop path and close path of AC-END-1; the re-check
  under a stream of unrelated events (the run ends in the store with no update published); a
  switching task refused in well under a second.
- `task_done.rs` (real ptyhost and fake agent, a host whose writer's shutdown never completes):
  closing escalates and the agent process is gone; the chained successor's outcome is decided when
  the close returns and is "#N was deleted" after deletion.
- e2e `task-done.e2e.ts`: a held DELETE shows "Deleting #13…" and a second confirm sends no second
  request; a task touched to `stopping` has a Delete item that says "Stopping…".

## Decisions

- A failed terminate leaves the run live instead of retrying in a loop: the run's reader stays
  attached and finalizes it if its processes go later, and stopping again re-runs the escalation.
  Recorded in ADR-0048.
- Finalization is tracked as a per-task count in a `tokio::sync::watch` (raised before the attempt
  is marked ended, lowered by a guard at the end) rather than a one-shot "finalized" flag: a run
  that ended by itself just before the close, and two observers of the same end, need no
  generation bookkeeping, and the wait reads one value.
- The escalation is spawned before the run lock (the issue's second option) instead of spawning the
  whole close: after the intent is set nothing that matters can be cancelled, with one task less.
- The shutdown write is bounded by 2 s: it is one frame on a local socket; a write that takes
  longer is a stuck host, which the escalation handles.

## Evidence

Filled in when the work is done.
