---
yulius-task: 5cf2374c-26ae-468b-be74-b7c1ccc4efe3
status: in-progress
---

# SPEC-155: automations can create prepared tasks ("When I decide")

Status: in progress
Date: 2026-10-15
Owner: Yulius task #142 (part of task #118, SPEC-138)
Scope authorization: GitHub issue #416, a follow-up of SPEC-105 (prepared tasks) and SPEC-078
(automations): "The automation editor's Start row, the API and the tools offer 'When I decide':
each run creates its task `prepared` (SPEC-105), which waits for Start now. The overlap check
(`skipped_previous_active`) counts a prepared task of the automation as active."

## Goal

An automation can prepare its task instead of starting it: each run (scheduled, catch-up or Run
now) creates the task `prepared`, and it waits until a person presses Start now. A morning
automation can then write the day's task for the person to send when they are ready.

## Context

- SPEC-078 gave an automation one start choice for every task it creates (`AutomationStart`:
  Now, Supertask, After, Rule; AUT-013..019). It is stored in three columns of `automations`
  (`supertask`, `after_task_id`, `start_rule`, migration 0019) and carried on the wire as the same
  three fields as `CreateTask`. SPEC-093 moved the choice into the editor's action bar as New
  task's start chip, and Run now sends the choice the chip shows (`RunAutomation`, AUT-023).
- SPEC-105 added prepared tasks: `CreateTask.prepared: true` ("When I decide") or `start_at`
  creates a task `prepared` that holds no session and no worktree until Start now or its time.
  `App::create_task_held(req, after, rule, Some(None))` creates one; it refuses a predecessor, a
  rule or a supertask with it (`prepared_alone`) and a 33rd prepared task of the sandbox
  (`ensure_prepared_room`, 429).
- An automation run creates its task in `create_automation_task` through `create_task_ruled`; a
  run is `skipped_previous_active` while the previous run's task `is_active_status` (not closed,
  stopped or failed), which already includes `prepared`.
- The shared start menu (`openStartMenu` in `web/assets/startchoice.js`) offers "When I decide"
  and "At a time…" only together, when the caller passes `onTime` (New task and a waiting task's
  view); the editor passes neither. `startPhrase` (`format.js`) names the choice in the editor's
  sentence and the list row.
- Agents use `automation_create` / `automation_update` with `after_task`, `start_rule` and
  `supertask` (AUT-017).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| AUT-026 | An automation's start choice can be **When I decide** (`AutomationStart::Prepared`). The API's `Automation` and `RunAutomation` carry it as `prepared` (default false), as `CreateTask` does; it is stored with the automation (migration 0031, column `automations.prepared`, default 0), so existing automations and clients that do not send it keep their choice. Create, update and Run now refuse (422) `prepared` together with `supertask`, `after_task_id` or `start_rule`, and store nothing. | int `automation_start.rs` |
| AUT-027 | Every run of a prepared automation (scheduled, catch-up after a restart, Run now with the stored choice or with `prepared: true` in the body) creates its task `prepared` with no start time: it starts no run and waits for Start now (SPEC-105 PRP-004). A sandbox already holding 32 prepared tasks makes the run `failed` with the reason. | int `automation_start.rs` |
| AUT-028 | While a prepared task of the automation's previous run still waits, the next run is `skipped_previous_active` ("the previous task #N is still prepared"); once it is started, cancelled or deleted the next run creates a new one. | int `automation_start.rs` |
| AUT-029 | `automation_create` and `automation_update` take `prepared` (boolean) like the other start arguments, with the same checks; in an update, giving it replaces the start choice. `automations_list` and both tools return `prepared`. | int `automation_start.rs` |
| AUT-030 | The editor's start chip offers **When I decide** (no "At a time…": the schedule is the automation's time) and reads "When I decide" when picked; its tooltip says each task waits, prepared, until Start now. The header sentence and the Automations list row name it ("· when I decide"). Save and Run now send `prepared: true`. | e2e `automations.e2e.ts` |
| AUT-031 | SPEC-078, `docs/api.md`, the tool descriptions, an ADR, the architecture document, the product summary and `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] An automation stores the prepared choice through the API, validated like New task
- [x] Each run and Run now create the automation's task prepared, and a waiting one skips the next run
- [x] Agents set the prepared choice with the automation tools
- [x] The automation editor, list row and sentence offer and name When I decide
- [x] Docs, ADR and the changes log describe it
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, evidence recorded

## Acceptance criteria

- AC-AUT-26: `POST /api/sandboxes/{sid}/automations` with `prepared: true` answers with
  `prepared: true` and the other three empty; a `GET` returns it; `PUT` without it clears it
  (start now); `prepared` with `supertask`, `after_task_id` or `start_rule` is 422 on create,
  update and Run now and leaves the automation unchanged.
- AC-AUT-27: with a controllable clock, a due run of a prepared automation creates a task with
  `status: "prepared"` and `start_at: null` that stays prepared across background ticks; Run now
  with `{"prepared": true}` on a start-now automation creates a prepared task and does not store
  the choice; with 32 prepared tasks in the sandbox the run is `failed`.
- AC-AUT-28: while that prepared task waits, the next due slot is `skipped_previous_active`;
  after Start now released it and it ended (cancelled), the following slot creates a new
  prepared task.
- AC-AUT-29: `automation_create` with `prepared: true` stores it and returns `prepared: true`;
  `automation_update` with `supertask: true` replaces it; `prepared: true` with `start_rule` is
  an error and changes nothing.
- AC-AUT-30 (mock UI): the editor's start menu offers When I decide and no At a time…; picking it
  shows "When I decide" on the chip and "when I decide" in the sentence; Save sends
  `prepared: true` with no other choice; Run now sends `prepared: true` and creates a prepared
  task; the list row reads "· when I decide"; no WCAG violations.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Integration, `crates/yulius/tests/automation_start.rs` (HTTP router, controllable clock):
  AC-AUT-26 .. AC-AUT-29, each written first and red before its slice.
- Unit, `yulius-app/src/automations.rs`: `start_input` refuses prepared with the other choices.
- Store: the new column round-trips through the API tests; the version-18 compatibility test
  keeps reading old rows as start now.
- e2e, `e2e/tests/automations.e2e.ts` (mock server): AC-AUT-30.

## Decisions

- **`prepared` is a fourth wire flag, not a `start` enum on the wire.** `CreateTask`,
  `ChangeStart` and the automation already speak in flags (`supertask`, `after_task_id`,
  `start_rule`, `prepared`); the automation mirrors `CreateTask` so the editor sends the same
  body shape for both, and the app's `AutomationStart` still makes two choices unrepresentable.
- **No "At a time…" for automations.** The cron schedule is already the automation's time; a
  per-task start time relative to the run would be a second schedule nobody asked for. The menu
  gets a `prepare` option that offers When I decide alone.
- **A column, not a reuse of `supertask`/`start_rule`.** A boolean column with default 0 keeps
  old rows as they are and needs no data migration, as migration 0019 did.
- **The overlap rule stays as it is.** A prepared task is already active (`is_active_status`),
  so a daily automation never stacks unstarted tasks: the person sees one waiting task, and the
  next run after they started or cancelled it prepares the next one. This is what #416 asks.
- **A prepared automation task skips the onboarding wait at creation,** as New task's prepared
  task does (PRP-003): it waits for onboarding when it is started.
- Recorded as ADR-0128.
