---
yulius-task: d4f29be7-73f4-4811-b191-7ca28a4149b3
status: done
---

# SPEC-078: an automation starts its tasks like New task

Status: implemented
Date: 2026-10-13
Owner: Yulius task #70
Scope authorization: user request (task #70), with a screenshot of the automation builder ("Keep
the docs up to date": The task, Workflow, When, Advanced: cron): "We'd like to be able to set the
automation as a normal task: queue it, 'run if rule', 'run after' and run as a supertask.
Implement."

## Goal

The tasks an automation creates start the way a person can start a task from New task: queued to
start as soon as a session slot is free (today's only behavior), after another task is done, when
a start rule is true, or as a supertask that runs alone in its project. The choice is made once on
the automation and applies to every task it creates.

## Context

- New task's start chip (`afterChip` in `web/assets/taskview.js`) offers exactly these four:
  **Start now** (the task is `queued` and starts when a slot is free), **Supertask** (SPEC-071,
  TSK-018), **After a task** (`chained` until that task is Done, TSK-012) and **When a rule is
  true…** (`chained` until a sentence like `#12 done and #13 merged` is true, SPEC-017, edited in
  the rule editor of SPEC-022). The API carries them as `CreateTask.after_task_id`,
  `CreateTask.start_rule` and `CreateTask.supertask`; `create_task_as` refuses a supertask with
  either of the others, and `create_task_ruled` refuses after and rule together.
- An automation (`yulius-app/src/automations.rs`, SPEC-001 AUT-001..007, SPEC-042) creates its
  task in `create_automation_task` with `start: true` and `supertask: false`, so every created
  task is simply queued. A run is skipped (`skipped_previous_active`) while the task of the
  previous run is still active (`is_active_status`: not closed, stopped or failed), which
  includes a task waiting `chained` or held `queued`.
- `chain_predecessor` refuses an ended predecessor ("#N has ended; start the new task now
  instead") and checks the per-sandbox chain bound (`MAX_CHAINED_PER_SANDBOX`); `rule_anchor`
  accepts any existing task named by the rule (ended ones too) and checks the same bound. A rule
  already true starts the task at once; a rule that can no longer be true leaves the task
  `chained` with a reason and a person decides (Start now or Cancel).
- Agents manage automations with `automation_create` / `automation_update` (SPEC-066), and tasks
  with `task_create`, which takes `after_task` (a number), `start_rule` and `supertask`.
- Automations are stored in the `automations` table (schema version 18 today).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| AUT-013 | An automation has a **start choice** for the tasks it creates, the same four as New task: start now (the default), supertask, after a task, or a start rule. The API's `Automation` carries it as `supertask` (default false), `after_task_id` (a task id) and `start_rule` (rule text, stored canonically), with the read-only `after_task_number`; existing automations and clients that send none of them keep "start now". It is stored with the automation (schema version 19). | int `automation_start.rs` |
| AUT-014 | Create and update refuse (422) an invalid start choice and store nothing: a supertask with an after-task or a rule; an after-task together with a rule; an after-task that is not a task of the automation's project; a rule that does not parse or names a task number the project does not have. An update that sends the stored choice back unchanged keeps it unchecked (its task may have been deleted since). | int `automation_start.rs` |
| AUT-015 | Every run of the automation (scheduled, catch-up after a restart, Run now) creates its task with the start choice: start now → `queued`; supertask → a supertask (SPEC-071); rule → `chained` until the rule is true (at once when it already is); after a task → `chained` after it while it has not ended, and once it has ended, `chained` on the rule `#N done`, so a run after a predecessor that was Done (TSK-014; a closed task: plan complete and pull request merged, or open and green) starts at once and one after a predecessor that ended without being Done waits for a person, as a rule that can no longer be true does. A predecessor that no longer exists, or a full chain bound, makes the run `failed` with the reason. | int `automation_start.rs` |
| AUT-016 | The previous-active rule is unchanged: a created task waiting `chained` or `queued` is active, so the next scheduled runs are skipped until it ends; waiting tasks never pile up. | int `automation_start.rs` |
| AUT-017 | Agents set the start choice with `automation_create` and `automation_update` (`after_task` as a task number, `start_rule`, `supertask`), with the same checks; in an update, giving any of the three replaces the start choice (the ones not given are cleared). `automations_list` returns it. | int `automation_start.rs` |
| AUT-018 | The automation builder has a **Start** row under the workflow with the chips Start now, Supertask, After a task… (the project's tasks that have not ended, as New task offers them) and When a rule is true… (the rule editor of New task); the picked task or rule shows on its chip. The header sentence names a choice other than Start now ("· supertask", "· after #12", "· when #12 done"). | e2e `automations.e2e.ts` |
| AUT-019 | SPEC-001 (AUT-013 row), the API doc, an ADR, the architecture document, the product summary and `docs/changes.md` describe the start choice. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] An automation stores a start choice through the API, validated like New task
- [x] Each run creates its task with the automation's start choice
- [x] Agents set the start choice with the automation tools
- [x] The automation builder offers Start now, Supertask, After a task and a rule
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-AUT-13: `POST /api/sandboxes/{sid}/automations` with `supertask: true` returns it with
  `supertask: true`; with `after_task_id` of task #1 it returns `after_task_id` and
  `after_task_number: 1`; with `start_rule: "#1 done"` it returns that rule; with none of them
  the three are empty/false. `PUT` changes and clears them.
- AC-AUT-14: supertask + after, supertask + rule, after + rule, an unknown or foreign after-task,
  an unparseable rule and a rule naming an unknown number each answer 422 and leave the stored
  automation unchanged.
- AC-AUT-15: with a controllable clock, a due run of a supertask automation creates a task with
  `supertask = true`; of a rule automation a `chained` task whose chain carries the rule; of an
  after-task automation whose predecessor is live a `chained` task after it; once that predecessor
  is closed, Run now creates a `chained` task on the rule `#N done` (its chain names the rule);
  a predecessor deleted since makes the run `failed` with "the task to run after does not exist".
- AC-AUT-16: while the chained task of a run waits, the next due slot is
  `skipped_previous_active`.
- AC-AUT-17: `automation_create` with `supertask: true` and `automation_update` with
  `after_task: <n>` store the choice; `automation_update` with `start_rule` clears the after-task;
  invalid combinations answer with an error and change nothing.
- AC-AUT-18 (mock UI): the builder shows the Start row with Start now picked for an existing
  automation without a choice; picking Supertask and saving sends `supertask: true` and no
  `after_task_id`/`start_rule`; picking a task under After a task… sends its id; picking a rule in
  the rule editor sends the rule text; the header sentence names the choice.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- `crates/yulius/tests/automation_start.rs` (HTTP router, controllable clock, sessions capped at 0 so
  created tasks stay waiting): AC-AUT-13 .. AC-AUT-17, each red before its slice.
- Store round trip of the new columns through the same API tests (create, read back, update,
  clear); the migration is applied by every store open (`migrates_and_reopens`).
- Unit tests in `yulius-app/src/automations.rs` for the pure start-choice validation (the
  combinations of AUT-014 that need no store).
- e2e `e2e/tests/automations.e2e.ts` (mock server): AC-AUT-18.

## Decisions

- **"Queue it" is New task's Start now.** The composer's start chip is the only place a person
  "queues" a task, and its first choice reads "the task starts as soon as a session slot is free":
  that is what an automation already does. The builder names it **Start now** like New task, so
  the two read the same; it stays the default and existing automations keep it.
- **One start choice per automation, not per run.** The request is to set the automation "as a
  normal task"; the choice is part of the recipe, like the workflow. A per-run override (Run now
  asking how to start) adds a dialog for no reported need.
- **A start choice as one enum in the app, three nullable columns in the store, three fields on
  the wire.** The wire mirrors `CreateTask` exactly so the API reads the same for tasks and
  automations; the app's `AutomationStart` makes two choices at once unrepresentable; the store
  keeps plain columns with defaults so old rows read as Start now with no data migration.
- **After a task that has ended becomes the rule `#N done`.** An automation recurs, and its
  predecessor ends after the first runs. New task's chain refuses an ended predecessor, which
  would make every later run fail. The rule language already says what "after #N is done" means
  for an ended task: true when it was done (the task starts at once), never true when it ended
  otherwise (a person decides). The task's chain then reads "Queued by a rule: when #N done", which
  is what it waits for.
- **Validated at save and again at each run.** Saving catches typos (a missing task, a bad rule)
  where the person is; a run re-checks because tasks are deleted and chain bounds fill between
  runs, and records the reason on a `failed` run, as other task-creation failures are.
- **The previous-active skip stays.** A recurring automation whose task waits for a rule or a
  supertask hold would otherwise stack a new waiting task every slot, up to the chain bound.
- **Review round 1** (fresh-session review, REQUEST CHANGES):
  - Fixed:
    - A stored choice sent back unchanged is kept without a re-check, so an automation whose
      task was deleted can still be paused or renamed.
    - "Was done" means Done as the rule language defines it (a closed task: plan complete and
      pull request merged, or open and green), now stated in the ADR and the builder, with tests
      for both outcomes.
    - Tests added for a rule already true, the chain bound, the blocked state and a choice
      surviving invalid updates; unit tests for `start_input`.
    - The Start row follows a task list that loads late, without re-rendering the prompt.
    - The rule chip says why it cannot open.
    - The mock checks and canonicalises rules and counts a chained task as active.
    - The `after_task` argument parsing is shared with `task_create`.
  - Accepted and recorded in ADR-0062: a rule's number reused after the newest task is
    deleted, and a predecessor ending mid-run failing that one run.
  - An old client's PUT without the fields clears the choice, as for `workflow_id`.
- **Review round 2** (APPROVE): its minors fixed in 78562aa: a store compatibility test (an
  automation of the previous schema reads back as start now), mock parity for an unchanged
  choice and the seeded rows, `startPhrase` moved to `format.js`, and the ADR states the
  scheduler's added wait.
- **Review round 3** (APPROVE of 78562aa, one minor and three nits, all in the mock or the
  wording): left as they are, because the review loop ends after three rounds and none of them
  changes what users get. The mock's template-enable route omits the new fields, which the UI
  reads as start now. After the reviews only `dev` was merged in: migration and ADR numbers
  were renumbered to 0019 and 0062 because `dev` took 0018 and 0055. That is evidence-only, with
  no logic change.
- Recorded as ADR-0062.

## Evidence

- `scripts/dev ci` on the head merged with `dev` (cc74a6a): exit 0. Repository contracts, fmt,
  clippy, deny, shear, nextest **1403 tests passed** (16 skipped), doctests. Before the merge
  (78562aa): exit 0, 1375 passed.
- Full e2e suite (`npx e2e run`) on cc74a6a: 270 passed, 1 skipped (41 files).
- New tests, each red before its slice (the review's coverage tests pinned behaviour that was
  already correct):
  - `crates/yulius/tests/automation_start.rs`, 5 tests:
    - the choice is stored, changed and cleared, and invalid choices are refused with 422 and
      change nothing (AC-AUT-13/14);
    - each run's task starts with its choice, and a waiting task skips the next run
      (AC-AUT-15/16);
    - a Done-and-closed predecessor and a true rule start the task at once, and a full chain
      bound fails the run;
    - a stored choice naming a deleted task still saves (red before the round-1 fix);
    - the agent tools set and replace the choice (AC-AUT-17).
  - `crates/yulius-app/src/automations.rs` unit test `a_start_choice_is_one_of_four`.
  - `crates/yulius-store/src/lib.rs` `version_eighteen_automations_start_their_tasks_now`.
  - `e2e/tests/automations.e2e.ts`: the builder's Start row (AC-AUT-18, no WCAG violations) and
    `ruleFromText` (unit, in the page).
- Checked by eye in the Yulius browser: the Start row under the workflow on the mock builder.
- Independent review in fresh sessions:
  - round 1: REQUEST CHANGES, fixed in 31cd455;
  - round 2: APPROVE, its minors fixed in 78562aa;
  - round 3: APPROVE of 78562aa.
- `spec_check` valid; `python3 scripts/check_repository.py` passed.
- Not testable here: real Claude sessions and a live GitHub. Runs use the in-memory harness,
  where tasks wait queued with live sessions capped at 0, and Done comes from a seeded merged
  delivery.
