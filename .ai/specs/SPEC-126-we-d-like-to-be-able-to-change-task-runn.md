---
yulius-task: d4504776-5edf-4bc2-b91a-5443c1c2e43e
status: in-progress
---

# SPEC-126: change how a waiting task starts

Status: in progress
Date: 2026-10-14
Owner: Yulius task #115
Scope authorization: user request (task #115): "we'd like to be able to change task running rule,
ofc not for already running but i'd like to be able to change 'When I decide' to chain etc".

## Goal

A task that has not started yet can change how it starts: a prepared task ("When I decide" or "At
a time…", SPEC-105) can be chained after a task or a rule, started now or as a supertask, and a
chained task (TSK-012, SPEC-017) can wait for another task or rule, be prepared, or be scheduled.
A task that has started keeps its start; nothing changes for it.

## Context

- The start choice exists only at creation: `CreateTask` takes `after_task_id`, `start_rule`,
  `supertask`, `prepared` and `start_at`; `App::create_task_as` / `create_task_held`
  (`yulius-app/src/lifecycle.rs`) validate them (one choice at most; a supertask or a prepared
  task alone; a live predecessor of the same sandbox; a rule naming existing tasks).
- Two statuses wait before the first run and hold no session: `chained` (a row in `task_chains`:
  predecessor or rule, `blocked_reason`, `created_at`; `chains.rs`) and `prepared` (an optional
  row in `task_schedules`; `prepared.rs`). Each sandbox holds at most 32 of each.
- After creation a person can only Start now (`POST …/start-now`), Cancel (`/stop`), and, for a
  prepared task, move or clear its time (`PUT …/schedule`).
- The tick (every 60 s) and run ends evaluate chains: `advance_after_chains` and
  `advance_rule_chains` read the chain, then `release_chained` moves `chained → queued` with a
  compare-and-set on the status only. A rule's facts may ask GitHub, so a pass can take seconds.
- A rule judges only tasks that existed when the waiting task was created
  (`evaluate_rule`, `unblock_rule_chains`: `named.created_at <= task.created_at`), so a number
  freed by deleting the newest task never answers for an older rule.
- The task view's attention bar (`taskview.js`) shows a chained or prepared task's start with
  Start now, Schedule…/Change time… and Cancel. The composer's start chip and menu
  (`startchoice.js`) offer every choice.
- `queued` and `waiting_capacity` tasks are in line for a session: the queue drain may hand them
  a slot at any moment.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CHS-001 | `PUT /api/tasks/{tid}/start` takes the creation's start choice (`after_task_id`, `start_rule`, `supertask`, `prepared`, `start_at`; none of them: Start now) with the same validation and messages (422), and replaces how a `chained` or `prepared` task starts. Any other status is 409 ("the task has started …"). It needs write access and ownership (SPEC-070, 403). It answers the updated `TaskSummary`. | int `start_change.rs` |
| CHS-002 | The new choice applies as at creation: after a task or a rule → `chained` with a fresh chain row (no blocked reason) and the predecessor's or rule's evaluation run at once (a predecessor already Done starts it); prepared → `prepared`, with or without a time (a due time releases it at once); Start now or Supertask → `queued` (with the `supertask` flag), then the normal start; a chained task keeps its chain row then, as Start now does, so it starts from its predecessor's branch with its briefing; while the sandbox's onboarding blocks tasks, Start now and Supertask wait chained after the preparation task as a new task does (a supertask keeps its flag). Otherwise the old chain row and start time are gone. The checks are New task's own (one shared validator). | int |
| CHS-003 | The change is one store transaction with a compare-and-set on the status read: a task that started, was cancelled or changed meanwhile is 409 and nothing changes. Changes run one at a time. Moving into `chained` or `prepared` from the other status respects the sandbox's bound of 32 (429), counted before the write as at creation (a creation at the same moment can pass it by one); staying in the same status needs no room. | int, store test |
| CHS-004 | A wait may not wait for itself: running after the task itself, a rule naming it, or a predecessor or rule that waits (through chained tasks' predecessors and rules, each rule naming the tasks it judges under CHS-006) for this task is 422 and changes nothing; two changes at once cannot both pass. | int |
| CHS-005 | A tick or run end that evaluated the old chain never releases or blocks the task under a new one: a release or a block from an evaluation applies only while the stored chain still has the same predecessor, rule and time (compare-and-set in the store). Start now is unaffected. | store test |
| CHS-006 | A rule written by a change judges the tasks that existed when it was written (the chain row's `created_at`), not when the task was created: a task created after the waiting task but before the change answers for it. Freed numbers still never answer for an older rule. | int |
| CHS-007 | The change is audited (`task.start_changed`, with the choice) and published; the old and new predecessors are published too (their "runs after it" lists). | int (links), review (audit, events) |
| CHS-008 | The task view's attention bar of a chained or prepared task the person may change shows a start chip ("Starts: When I decide", "Starts: After #12", "Starts: When #12 done", "Starts: At tomorrow 09:00") that opens the composer's start menu (Start now, Supertask, When I decide, At a time…, After a task is done, When a rule is true…) without the task itself among the tasks to run after or name in a rule; a pick calls `PUT …/start` and the bar, row and chip follow; an error is a toast and nothing changes. Readers and non-owners see no chip. No WCAG violations. | e2e `start-change.e2e.ts` |
| CHS-009 | SPEC-001 (TSK-020), the API doc, an ADR, the architecture document, the product summary and `docs/changes.md` describe it. | review |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] The store replaces a waiting task's start in one transaction and releases only the chain it evaluated
- [ ] The API changes how a prepared or chained task starts, refusing started tasks, cycles and full bounds
- [ ] A changed rule judges the tasks that existed when it was written
- [ ] The task view's start chip changes how a waiting task starts
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-CHS-1: a prepared task changed to "after #N" (N running) is `chained` after #N with no
  start time; changed to a rule it is `chained` on the rule; a scheduled task changed to When I
  decide (`prepared: true`) keeps waiting with `start_at: null`; a chained task changed to
  `start_at` an hour ahead is `prepared` with that time and has no chain.
- AC-CHS-2: a chained task changed to Start now is `queued` and starts; changed to Supertask it
  is `queued` with `supertask: true`; a prepared task changed to "after #N" where #N is already
  Done starts at once.
- AC-CHS-3: a running task, a cancelled task and a `queued` task answer 409; a reader and a writer
  who does not own the task answer 403; an invalid choice (after and rule together, prepared with
  a predecessor, a past time) answers 422; none of them changes anything.
- AC-CHS-4: the task itself as predecessor, a rule naming it, and a predecessor that is chained
  after it answer 422.
- AC-CHS-5: with 32 chained tasks in the sandbox, a prepared task changed to a chain answers 429;
  a chained task changed to another chain still succeeds.
- AC-CHS-6 (store): `release_chain` and `block_chain` act on a chained task only while its chain
  row is still the evaluated one (predecessor, rule and time); after a replacement they return
  false and the task stays chained and unblocked.
- AC-CHS-7: a task created before #M and changed to the rule `#M done` starts once #M is done.
- AC-CHS-8 (mock UI): a prepared task's bar shows "Starts: When I decide"; the menu offers the
  other choices without the task itself; picking a task to run after makes it "Queued after #N"
  with the chip "Starts: After #N"; picking When I decide on a chained task makes it Prepared; no
  WCAG violations; a non-owner sees no chip.

## Test plan

- Domain (`yulius-domain`): the transitions a change makes (`chained`/`prepared` → `chained`,
  `prepared`, `queued`), nothing from other statuses.
- Store (`yulius-store`): the replacing transaction (status compare-and-set, chain and schedule
  replaced, queued bookkeeping) and the chain-guarded release.
- Integration (`crates/yulius/tests/start_change.rs`, in-memory harness, injected clock):
  AC-CHS-1..5 and 7; the race of CHS-005 is shown at the store level (the evaluation's read and
  release are not separable through the API without hooks).
- End to end (`e2e/tests/start-change.e2e.ts`, mock server): AC-CHS-8.

## Decisions

- One endpoint taking the creation's start-choice fields, not one per transition: the composer's
  choices map one to one, and validation is shared with creation. ADR-0102.
- Only `chained` and `prepared` tasks change. A `queued` or `waiting_capacity` task is in line
  for a session and the queue drain may start it at any moment; changing it would race the drain.
  The person asked for tasks that have not started; a queued task can still be cancelled. ADR-0102.
- The change is one store transaction (status compare-and-set, chain and schedule replaced),
  and a release from an evaluation compares the chain it evaluated: no window where a stale
  evaluation starts the task under the new choice. ADR-0102.
- A rule judges tasks from the chain row's `created_at` (when the rule was written), which equals
  the task's creation for rules given at creation. ADR-0102.
- Agents and the Superagent do not get a tool for it in this change (the request is the UI's);
  a follow-up issue tracks it if wanted.
- Review round 1 (REQUEST CHANGES, fixed test-first): blocking is guarded like releasing and also
  compares the chain's time; a supertask held by onboarding keeps its flag; changes are serialized
  (an in-process lock: Yulius is one process) so two cannot form a cycle; a chained task started
  now keeps its chain as Start now does; creation and change share one validator; the cycle walk
  judges a rule's numbers from its time. The bounds stay counted before the write, as creation
  does (ADR-0085).
- The UI reuses the composer's start menu and rule editor; the task's own Start now, Change
  time… and Cancel buttons stay as they are.
