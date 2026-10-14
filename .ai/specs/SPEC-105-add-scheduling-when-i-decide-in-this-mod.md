---
yulius-task: f44e18c7-f992-4815-adc3-2018aae81a4d
status: done
---

# SPEC-105: prepared tasks start when you decide, or at a time

Status: implemented
Date: 2026-10-14
Owner: Yulius task #90
Scope authorization: user request (task #90), with a screenshot of the composer's start menu
(Start now · Supertask · After a task is done · When a rule is true…): "add scheduling + 'when I
decide' (in this mode tasks have special status 'prepared' and can be sent after clicking a
button or something".

## Goal

A person can write a task now and start it later: either **when they decide** (it waits, prepared,
until they press Start now) or **at a time** they pick (it starts by itself then). Until it starts
the task is `prepared`: it holds no session and no worktree, and can be started, rescheduled or
cancelled.

## Context

- New task's start chip (`afterChip`, `web/assets/taskview.js`) offers Start now, Supertask, a
  task to run after (TSK-012) and a start rule (SPEC-017). Each maps onto `CreateTask`
  (`after_task_id`, `start_rule`, `supertask`); the MCP tool `task_create` takes the same choices.
- A task that waits before its first run is `chained` (`yulius-app/src/chains.rs`): its row in
  `task_chains` names a predecessor or a rule; the background tick (every 60 s) and run ends
  re-evaluate it; `POST /api/tasks/{tid}/start-now` releases it (`chained → queued`, then
  `start_run`), `/stop` cancels it (`stopped_cancelled`). At most 32 wait per sandbox.
- A waiting task (`queued`, `chained`, `waiting_capacity`) takes queued messages into its prompt
  (SPEC-049 QMS-001), is idle for the disk controller and is listed as active by `tasks_list`.
- The sidebar groups (`AttentionFacts::group`, mirrored by `status.js`) put queued and chained
  tasks in Working's "in line" part (`TaskGroup::Queued`).
- Onboarding is mandatory: while a sandbox is prepared, new tasks wait chained after the
  onboarding task (U-08, `App::onboarding_wait`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PRP-001 | A task can be created **prepared**: `CreateTask.prepared: true` ("when I decide"), or `CreateTask.start_at` (RFC 3339, implies prepared). It gets the status `prepared` (domain `TaskStatus::Prepared`, wire `prepared`) and starts no run. `start_at` must be at most 60 s in the past and at most 366 days ahead, else 422. Prepared is refused together with `after_task_id`, `start_rule` or `supertask` (422). At most 32 prepared tasks wait per sandbox (429). The MCP tool `task_create` takes `prepared` and `start_at`. | unit `task.rs`, int `prepared.rs` |
| PRP-002 | `TaskSummary.start_at` is the scheduled time (RFC 3339) while the task is `prepared` and has one, null otherwise. The time is stored in `task_schedules` (schema version 24). | int `prepared.rs`, store test |
| PRP-003 | A prepared task never starts by itself without a time. A prepared task with a time starts by itself on the first background tick at or after it (at most 60 s late; a time passed while Yulius was down starts it at the first tick after the restart): `prepared → queued`, compare-and-set, then the normal start (session limit, supertask hold, capacity). While its sandbox's onboarding blocks tasks it stays prepared and starts on a tick after onboarding. | int `prepared.rs` |
| PRP-004 | **Start now** (`POST /api/tasks/{tid}/start-now`) starts a prepared task at once whatever its time (409 while onboarding blocks). **Cancel** (`/stop`) makes it `stopped_cancelled`. **Delete** removes it. `PUT /api/tasks/{tid}/schedule {"start_at": "<RFC 3339>"|null}` sets or clears its time under PRP-001's window (409 when the task is not prepared). Each needs write access and ownership (SPEC-070). | int `prepared.rs` |
| PRP-005 | A prepared task is a waiting task: it takes queued messages into its prompt (QMS-001), counts as idle for the disk controller, is active in `tasks_list`, may be run after (TSK-012) and sits in the sidebar's in-line part (`TaskGroup::Queued`), never in Needs you. | unit `task.rs`, `disk.rs`, int |
| PRP-006 | The composer's start menu offers **When I decide** ("The task waits, prepared, until you press Start now") and **At a time…** (a dialog with a date and time, default the next full hour; refused in the past). The chip reads "When I decide" or "At <time>", the send button "Prepare task" or "Schedule task". The task's row reads "Prepared" or "Scheduled · <time>"; its view says how it starts, with Start now, Schedule…/Change time… and Cancel; its menu offers Start now and Cancel. | e2e `prepared.e2e.ts` |
| PRP-007 | SPEC-001 (TSK-019), SPEC-002 §6.1, the API doc, an ADR, the architecture document, the product summary and `docs/changes.md` describe prepared tasks. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The domain knows the prepared status and the store keeps a task's start time
- [x] A task is created prepared or scheduled through the API and the task_create tool
- [x] Prepared tasks start now, on their time, or are rescheduled and cancelled
- [x] The composer prepares or schedules a task and the task view starts it
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-PRP-1: `POST /api/sandboxes/{sid}/tasks` with `prepared: true` answers 201 with
  `status: "prepared"`, `start_at: null`, and no run starts; with `start_at` an hour ahead it is
  prepared with that time; with a time 2 minutes ago, 367 days ahead, or together with
  `after_task_id`, `start_rule` or `supertask` it is 422 and creates nothing; the 33rd prepared
  task of a sandbox is 429.
- AC-PRP-2: Start now on a prepared task releases it to the normal start (`prepared → queued`,
  then `start_run`, as Start now on a chained task does); `/stop` on another makes it
  `stopped_cancelled`; Start now on a task that left `prepared` is 409; a reader, and a writer who
  does not own the task (SPEC-070), change nothing (403). Creating or rescheduling one task never
  starts another due one: that is the tick's.
- AC-PRP-3: a task scheduled at T stays prepared before T and is running after the tick past T
  (injected clock); a prepared task without a time stays prepared across ticks.
- AC-PRP-4: `PUT …/schedule` moves the time, `null` clears it (`start_at: null`); on a running
  task it is 409; a time outside the window is 422.
- AC-PRP-5: a prompt queued to a prepared task joins its prompt; `task_create` with
  `prepared: true` creates a prepared task.
- AC-PRP-6 (mock UI): the start menu offers When I decide and At a time…; When I decide sends
  `prepared: true` and the task view offers Start now, which starts it; At a time… sends the
  picked time as `start_at` and the row reads "Scheduled · …"; no WCAG violations.

## Test plan

- Unit (`yulius-domain`): `Prepared` transitions (released → queued, cancel → stopped, nothing
  else), wire name round trip, group `Queued`, disk owner idle.
- Store: schedule set, read, cleared, due list joined with `prepared`.
- Integration (`crates/yulius/tests/prepared.rs`, in-memory harness, injected clock): AC-PRP-1..5.
  Real processes after `queued` are the existing start path's (`supervision.rs`).
- End to end (`e2e/tests/prepared.e2e.ts`, mock server): AC-PRP-6.

## Decisions

- A new status, not a kind of `chained`: a chained task waits for other tasks and can become
  blocked; a prepared one waits for a person or a clock and never blocks. ADR-0085.
- The time lives in its own table (`task_schedules`), like `task_chains`, rather than a column
  every `TaskRecord` carries. ADR-0085.
- The minute tick releases due tasks: minute precision matches the picker, and the tick already
  survives restarts; no per-task timers to cancel. ADR-0085.
- Start now and Cancel reuse the chained task's endpoints; only rescheduling is new.
- Automations do not get "when I decide" in this change (their own start choice is being
  reworked by task #78): issue #416.
- Editing a prepared task's prompt is out of scope: queued messages already add to it.

## Evidence

Commands run on this branch (head a73f649c, before the final review nits; the nit commit reran
the touched tests and clippy):

- `scripts/dev lint`: rustfmt and Clippy (`-D warnings`) clean.
- `scripts/dev check`: repository contracts passed (16 crates, 7 skills, links, unique decision
  numbers, one product test binary); 39 script tests OK; `cargo check` of every target.
- `scripts/dev test`: 1543 tests passed, 16 skipped (nextest), doc tests OK. Among them
  `prepared::` (6: AC-PRP-1..5, the bound, a request releasing only its own task, a non-owner
  writer refused), `queued_messages::` (a prepared task takes a message), the store tests
  `prepared_task_schedules_persist_and_come_due` and
  `a_scheduled_release_checks_the_time_it_releases_on`, the domain test
  `prepared_tasks_wait_for_a_person_or_their_time` and the start-time window unit test.
- `scripts/dev deps`: advisories, bans, licenses and sources ok; cargo-shear clean.
- `cd e2e && npx e2e run`: 43 files passed, 1 skipped (307 tests passed, 1 skipped), including
  `prepared.e2e.ts` (3 tests, AC-PRP-6, no WCAG violations), `times.e2e.ts` ("prepared 3h"),
  and `menus.e2e.ts` (its keyboard and wheel tests count the two new start choices).
- After the final nits (`release_if_due` called directly, the domain transition checked before
  the store's conditional release): `prepared::`, `queued_messages::`, `intake::`,
  `supertask::` (26 passed) and workspace Clippy clean.
- Mutation check: without the onboarding guard in the tick,
  `a_scheduled_task_starts_by_itself_at_its_time` fails.
- Independent review in a fresh session: round 1 approved with minors (fixed test-first: the tick's
  race with a reschedule, requests releasing other tasks, the row's time word, a passed draft time,
  owner coverage); round 2 approved with nits (addressed).

Not tested here: a real Claude Code run started by a released prepared task (the in-memory
harness stops at `start_run`; the start path after `queued` is the existing one, covered by
`supervision.rs`), and the tick on a live server across a restart.
