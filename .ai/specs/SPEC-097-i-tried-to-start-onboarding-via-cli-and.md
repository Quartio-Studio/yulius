---
yulius-task: 8ef8d8e7-4134-46ea-877e-d569fd4eba81
status: done
---

# SPEC-097: a CLI upload creates no onboarding task until someone starts onboarding

Status: implemented
Date: 2026-10-14
Owner: Yulius task #82
Scope authorization: task request: "I tried to start onboarding via cli and I correctly got info
that I must do it from GUI. However, a closed task appeared, which is wrong because it shouldn't
have started at all."

## Goal

`yulius onboard` uploads a repository and leaves its sandbox `pending` (SPEC-051). Until a person
clicks **Start onboarding** in the browser, the sandbox has no onboarding task: its task list stays
empty instead of showing a "Closed · 0/6 steps" task that never ran.

## Context

- Symptom (task #82 screenshot): after `yulius onboard`, the task list shows one task, "Prepare this
  repository for Yulius", tagged Onboarding, "Closed · 0/6 steps", with no pull request.
- Root cause: `App::link_repository` (`crates/yulius-app/src/projects.rs`) creates the onboarding
  task on every import, through `App::create_onboarding_task`, which creates it without a run
  (`start: false`, status `closed`) and un-archives it so it stays in the active list. A browser
  import starts it at once; a token import (the CLI, SPEC-051 OBD-001/OBD-005) leaves it resting,
  so the never-run task shows as Closed until someone clicks Start onboarding.
- Why the tests missed it: SPEC-051 specified that a `pending` sandbox "stays `pending` with its
  resting task" (OBD-001, ADR-0038), and `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding`
  and `import_server::onboarding_starts_only_from_the_browser` asserted that the resting task exists.
- What already handles a `pending` sandbox without a task: `App::start_onboarding` creates the task
  when it is missing (Start onboarding), `App::onboarding_status` names "the preparation task", and
  deleting the onboarding task clears `onboarding_task_id` (`yulius-store` `tasks.rs`). Only
  `App::onboarding_wait` refuses (409) a new task in a `pending` sandbox without one.
- Parallel task #81 (SPEC-096, merged first as #402) made onboarding mandatory in the UI: a
  `pending` sandbox's page lists what onboarding will do from the onboarding task's own steps. With
  no task before Start onboarding, that plan needs its own read (OBT-004).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OBT-001 | Linking a repository with a bearer token (`yulius onboard`, `POST /api/sandboxes` with a token) creates no task: the sandbox is `pending` with no onboarding task, and its task list is empty. | Rust `import_server::onboarding_starts_only_from_the_browser` |
| OBT-002 | Start onboarding in the browser creates the onboarding task (full preparation, or the environment task for a repository that already validates) and starts it; the sandbox becomes `preparing`. Adding a repository in the browser still starts onboarding at once. | Rust `import_server::start_onboarding_creates_the_task_a_cli_upload_left_out`, `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding`; e2e `onboarding.e2e.ts` (the mock follows the server) |
| OBT-003 | A new task created in a `pending` sandbox without an onboarding task still waits after one (IMP-010, OBD-005): the onboarding task is created then, resting, and the new task is `chained` after it. Continuing that resting task with a token is still refused (OBD-002). Simultaneous requests create one onboarding task, and a skip made meanwhile is never undone (the creation and the skip share one lock; nothing is created once the sandbox is no longer `pending`). | Rust `import_server::onboarding_starts_only_from_the_browser`, `import_server::simultaneous_new_tasks_create_one_onboarding_task` |

| OBT-004 | What onboarding will do stays readable before its task exists: `GET /api/sandboxes/{sid}/onboarding/plan` (read access) returns the task's title and steps, or the ones it would be created with (`task_number: null`), and creates nothing. The mandatory-onboarding page (SPEC-096 OBG-002) shows that plan for a `pending` sandbox, naming the task once it exists. | Rust `import_server::onboarding_starts_only_from_the_browser`, `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding`; e2e `onboarding.e2e.ts` (both kinds) |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- AC-OBT-1: after a CLI upload of a new repository, `GET /api/sandboxes/{sid}/tasks` is empty and
  the sandbox is `pending` with no `onboarding_task_id`.
- AC-OBT-2: Start onboarding in the browser then creates and starts the onboarding task.
- AC-OBT-3: creating a task in that `pending` sandbox returns 201 `chained` after a new onboarding
  task, as before.

## Test plan

- Regression: `import_server::onboarding_starts_only_from_the_browser` asserts AC-OBT-1 right
  after the upload and after the refused CLI approve (fails before the fix: the task exists), then
  AC-OBT-3 and the token refusal of continue, then AC-OBT-2.
- `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding`: a token import of
  a validating repository has no task; Start onboarding in the browser creates the environment task.
- `import_server::simultaneous_new_tasks_create_one_onboarding_task`: two new tasks at once wait
  after the same, single onboarding task (fails without the lock: two are created).
- e2e `onboarding.e2e.ts`: the mock's pending sandbox lists no task until Start onboarding, and its
  page still shows the six-step (or, for a repository in the format, three-step) plan, read from
  `…/onboarding/plan`.
- The whole gate (`scripts/dev ci`).

## Decisions

- ADR-0077: the onboarding task of a `pending` sandbox is created when it is needed (Start
  onboarding, or a task that must wait after it), not at import. Alternatives considered: keep the
  resting task but archive or hide it (an archived Closed task still shows under archived tasks,
  and hiding it from lists spreads a special case over every list and count); refuse new tasks in a
  `pending` sandbox without one (changes the IMP-010 contract that new tasks wait).
- Sandboxes already `pending` with a resting onboarding task keep it: deleting it is the person's
  call, and Start onboarding starts it as before. No migration.
- Review findings left as they are (recorded): when a new task's later checks refuse it (for
  example `ensure_chain_room`), the onboarding task created for it stays, resting; it is the task
  the next request waits after, so nothing is lost. A request cancelled between creating the task
  and linking it to the sandbox can leave it unlinked, as import already could. A never-run task
  still reads "Closed" when a task waits after it: follow-up issue #399.

## Evidence

- Root cause reproduced: `import_server::onboarding_starts_only_from_the_browser` failed on the
  test commit c4b3b8f2 (`left: Some(TaskId(..)) right: None`), as did
  `start_onboarding_creates_the_task_a_cli_upload_left_out`. Reported as issue #397.
- `simultaneous_new_tasks_create_one_onboarding_task` failed 3 out of 3 runs with the lock
  commented out (two onboarding tasks, numbers 1 and 2), and passes with it.
- e2e `npx e2e run tests/onboarding.e2e.ts`: fails against the previous mock ("expect.toBeVisible
  failed" on the empty task list), passes (2/2) with the updated mock.
- `scripts/dev ci` at 2f84fbbf: exit 0, 1521 tests passed, 16 skipped.
- After the review fixes, `scripts/dev ci`: 1520 of 1522 passed; the two failures are
  `task_done::a_background_shell_keeps_the_task_working_until_it_ends` ("timed out waiting for one
  background shell") and `task_done::a_done_task_can_be_archived` (90 s), at host load average
  about 31. Both use a `not_needed` sandbox this change does not reach, and pass alone 3 runs out of
  3 (31/31). Filed as flaky issue #404, not retried into green. The scoped onboarding, import,
  projects, automations and sessions tests pass (30/30); e2e `npx e2e run`: 281 passed, 1 skipped.
- Independent review in a fresh session (om-code-review) of e6bbeeef: APPROVE, no blocker or major;
  minor findings 1-4 fixed (skip lock and `pending` guard, concurrency test, mock and its e2e,
  SPEC-051 and ADR-0038 text), 5 and nits recorded above.
- Round 2 review of c27f4d22: APPROVE; its pre-existing minor (Start onboarding vs. a concurrent
  Skip) filed as #406; the mock nit fixed (the mock creates its preparation task only once a new
  task's request is valid and waits for it).
- Merging #402 (SPEC-096) conflicted: its page read the plan from the resting task. OBT-004 adds
  the plan read; the tests failed first (404 on `…/onboarding/plan`; e2e: the two pending-page
  tests fail with the previous page) and pass after.
- At 10d01392 (after merging #402 and adding OBT-004): `scripts/dev ci` exit 0, 1534 tests passed,
  16 skipped (the #404 tests passed this time, load average about 28); `npx e2e run`: 291 passed,
  1 skipped.
