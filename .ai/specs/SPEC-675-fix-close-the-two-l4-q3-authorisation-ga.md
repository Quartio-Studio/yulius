---
yulius-task: 8ce90774-eb4a-4b3a-9da9-d30d8f595d33
status: draft
---

# SPEC-675: Close the two L4 Q3 authorisation gaps (git credential after disable, handoff answer)

## Goal

Two authorisation gaps from the November 2026 secrets and identity audit (L4, question Q3) get
closed. Task #653 confirmed both, and the owner decided they get fixed:

1. A task's agent keeps getting a fresh GitHub credential after its owner is disabled, or after
   the owner loses write access to the project.
2. A task's owner whose project access dropped to read can still answer its handoffs.

## Context

- `App::agent_git_credential` (`crates/yulius-app/src/hooks.rs`) issued the App's
  repository-narrowed agent token (ADR-0114) to any run that was not terminal.
  `App::agent_context` (`coordination.rs`) checks only the run.
- `App::update_user` (`admin.rs`) revokes a disabled person's web sessions, API tokens and cached
  access. It left their tasks' runs alone, and those runs could keep asking for credentials.
  Yulius's own pushes, pull requests and merges for such a task fall back to the App's
  installation token (`App::task_github_token`), so a run that kept going would still land work.
- The sibling `App::workspace_git_credential` (`identity.rs`), for a person's own processes,
  already refuses a disabled person or one without Work on the project.
- `App::answer_handoff` (`handoffs.rs`, SPEC-512 HOF-004) loaded the task with `load_task(View)`
  and then compared the caller to the owner directly. Every other task-control path asks
  `TaskAction::Control` (SPEC-490 OWN-001, ADR-0331). Control needs Work on the project and
  ownership.

## Requirements

- L4Q3-001: A run's git credential is issued only while the task's owner is enabled and holds
  Work on the task's project. `agent_git_credential` and `workspace_git_credential` ask one shared
  check (`App::git_credential_holder`), so both refuse the same way: a disabled person is
  `Unauthenticated`; a person without access is `NotFound`; one with read access is `Forbidden`.
  The superadmin keeps Admin on every project (ACC-012), so the superadmin's tasks are unchanged.
- L4Q3-002: Disabling a person stops every task they own that is not resting. That includes live
  runs and tasks that would start by themselves (queued, chained, prepared, parked, waiting for
  capacity). It uses the ordinary stop path, with an audit entry `task.stopped` per task naming
  the reason and a notice in the task's terminal. A stop that fails is logged. It does not undo
  the disable, and L4Q3-001 still refuses that run's credential. The decision is ADR-0481.
- L4Q3-003: `answer_handoff` loads the task with `TaskAction::Control`. An owner down to read
  access, and anyone but the owner, are refused with the same refusal as the other control paths.
  Approval still needs Work on the target project.

## Plan

- [ ] Spec and ADR-0481: disabling a person stops their tasks
- [ ] Failing integration tests for the three refusals
- [ ] Agent git credential asks the shared enabled-and-Work check
- [ ] Disabling a person stops the tasks they own
- [ ] Handoff answers ask the task Control rule
- [ ] Changes, architecture and API documents updated
- [ ] Touched test binaries pass in full
- [ ] Independent review of the change

## Acceptance criteria

- A run of a task whose owner was disabled through `PATCH /api/admin/users/{id}` is refused a git
  credential, and its task is stopped.
- A run of a task whose owner's access dropped to read is refused a git credential.
- The owner of a task whose access dropped to read gets `403` from
  `POST /api/tasks/{tid}/handoffs/{hid}`, with the control refusal, and the request stays
  `requested`.
- Every existing test passes unchanged, except one assertion that encoded gap 1 (see Decisions).

## Test plan

- `crates/yulius/tests/task_ownership.rs`: a disabled owner's run is refused a credential and
  the task rests; an owner down to read access is refused a credential. Both fail on `dev`.
- `crates/yulius/tests/handoffs.rs`: an owner down to read access cannot approve or decline a
  waiting handoff. Fails on `dev` (decline returned `200`).
- `cargo test -p yulius-app --lib` and `cargo test -p yulius --test main`, in full. Known failures
  before this change: `watches::run_bounds_are_enforced` (#1795) and
  `parking::a_staging_outcome_reaches_its_task_once` (#650).

## Decisions

- One shared check. `git_credential_holder(user, sandbox)` loads the person, refuses a disabled
  one, and requires `Action::Work` through `App::require`. Both credential paths call it, so there
  is no second copy. It asks `require` (access with the normal freshness bound), as the workspace
  path already did. A credential request after the access cache expired asks GitHub with the
  owner's token, and during a GitHub outage the stale-cache rule applies. That is the same cost
  and the same failure mode as a person's own git.
- Disabling stops the person's tasks: ADR-0481.
- One existing assertion changes. `agent_hooks::the_agents_git_never_holds_the_owners_token`
  asserted that a task owned by an unlinked non-superadmin, who has no access to the project, gets
  the agent token. That is gap 1 itself. The case now asserts the refusal, which is still never a
  person's token. The realistic unlinked owner, the superadmin (ACC-012), keeps getting the
  agent token.
