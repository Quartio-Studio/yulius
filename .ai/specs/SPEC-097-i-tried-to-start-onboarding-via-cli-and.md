---
yulius-task: 8ef8d8e7-4134-46ea-877e-d569fd4eba81
status: in-progress
---

# SPEC-097: a CLI upload creates no onboarding task until someone starts onboarding

Status: in progress
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
- Parallel task #81 (SPEC-096) hides the task input in the web UI until onboarding finishes; this
  fix is the server side and does not touch the UI.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OBT-001 | Linking a repository with a bearer token (`yulius onboard`, `POST /api/sandboxes` with a token) creates no task: the sandbox is `pending` with no onboarding task, and its task list is empty. | Rust `import_server::onboarding_starts_only_from_the_browser` |
| OBT-002 | Start onboarding in the browser creates the onboarding task (full preparation, or the environment task for a repository that already validates) and starts it; the sandbox becomes `preparing`. Adding a repository in the browser still starts onboarding at once. | Rust `import_server::onboarding_starts_only_from_the_browser`, `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding` |
| OBT-003 | A new task created in a `pending` sandbox without an onboarding task still waits after one (IMP-010, OBD-005): the onboarding task is created then, resting, and the new task is `chained` after it. Continuing that resting task with a token is still refused (OBD-002). | Rust `import_server::onboarding_starts_only_from_the_browser` |

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

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
- The whole gate (`scripts/dev ci`).

## Decisions

- ADR-0077: the onboarding task of a `pending` sandbox is created when it is needed (Start
  onboarding, or a task that must wait after it), not at import. Alternatives considered: keep the
  resting task but archive or hide it (an archived Closed task still shows under archived tasks,
  and hiding it from lists spreads a special case over every list and count); refuse new tasks in a
  `pending` sandbox without one (changes the IMP-010 contract that new tasks wait).
- Sandboxes already `pending` with a resting onboarding task keep it: deleting it is the person's
  call, and Start onboarding starts it as before. No migration.
