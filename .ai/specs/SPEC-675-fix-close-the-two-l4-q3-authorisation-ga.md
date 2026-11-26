---
yulius-task: 8ce90774-eb4a-4b3a-9da9-d30d8f595d33
status: in-progress
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
  Yulius's own pushes and pull requests for such a task fall back to the App's installation
  token (`App::task_github_token`), so a run that kept going would still publish work.
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
  An owner whose access cannot be settled now is refused too (`Unauthenticated`). The task's
  terminal says once per refusal episode why the agent gets no credential, and again after a
  hand-over; a credential issued again ends the episode.
- L4Q3-002: Disabling a person stops every task they own that is not resting. That includes live
  runs and tasks that would start by themselves (queued, chained, prepared, parked, waiting for
  capacity). It uses the ordinary stop path, waiting tasks first, on a task of its own after the
  disable and its `user.updated` audit are written. A task is read again before its stop and left
  when someone took control meanwhile. Each stop gets an audit entry `task.stopped` naming the
  reason and a notice in the task's terminal. A stop that fails is logged. It does not undo the
  disable, the others still stop, and L4Q3-001 still refuses that run's credential. The decision
  is ADR-0481. Automations of a disabled person are outside this change (#1824).
- L4Q3-003: `answer_handoff` loads the task with `TaskAction::Control`. An owner down to read
  access, and anyone but the owner, are refused with the same refusal as the other control paths.
  Approval still needs Work on the target project.

## Plan

- [x] Spec and ADR-0481: disabling a person stops their tasks
- [x] Failing integration tests for the three refusals
- [x] Agent git credential asks the shared enabled-and-Work check
- [x] Disabling a person stops the tasks they own
- [x] Handoff answers ask the task Control rule
- [x] Changes, architecture and API documents updated
- [x] Touched test binaries pass in full
- [x] Review round 1 answered: waiting statuses, failed stop and recovery tested, stop loop after the audit
- [x] Review round 2 answered: loop past a failed stop and notice reset tested
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

## Evidence

- Red on `dev` (commit 1d69c6256, tests only): `cargo test -p yulius --test main --
  a_disabled_owners_run an_owner_down_to_read_access` gave 3 failures. The disabled owner's run
  got `200 {"token":"agent-inst"}` where `401` was expected. The read-only owner's run got `200`
  where `403` was expected. The read-only owner's decline returned `200` and the handoff became
  `declined`.
- Green after the fixes: the same command passes 3/3, with
  `agent_hooks::the_agents_git_never_holds_the_owners_token`,
  `agent_hooks::git_credentials_are_scoped_to_the_project_repository`,
  `task_ownership::the_agents_gh_never_gets_the_new_owners_token`,
  `handoffs::ask_stores_a_request_the_owner_answers_in_a_browser` and
  `handoffs::two_approvals_at_once_create_one_task`.
- After merging `origin/dev` (0cd74ced5):
  - `cargo test -p yulius-app --lib`: 741 passed, 0 failed.
  - `cargo test -p yulius --test main`: 2834 passed, 1 failed, 26 ignored. The failure was
    `telegram::a_reply_to_a_runs_notice_answers_that_run`, which passed 3/3 alone. It is a test
    race in the Telegram brokered-command flow, outside this change, filed as #1820. The known
    `watches::run_bounds_are_enforced` (#1795) and `parking::a_staging_outcome_reaches_its_task_once`
    (#650) passed in this run.
  - `cargo clippy -p yulius-app -p yulius --all-targets -- -D warnings`: clean.
  - `scripts/dev quick`: passed.
- Review round 1 (Fable 5.2, head b703a94b8) requested changes. Answered in this order:
  - Tests now cover every waiting status (queued, chained, prepared, parked, waiting for capacity),
    the audit detail and the terminal notice, a stop that fails (a race hook moves the task in
    each of the stop's 4 rounds), and the recovery: Bob takes control, and the run gets the token
    again. Cutting `NOT_RESTING` down to `[Running]` fails
    `disabling_stops_every_waiting_task_of_the_owner_and_says_why`
    (`t-q was Queued, is Queued`).
  - The stop loop now runs on its own task after the `user.updated` audit, waiting tasks first,
    and re-reads each owner. It names the owner with `login_of`.
  - The agent's terminal says once why it gets no GitHub credential (test red first: 0 notices).
  - ADR-0481, `docs/api.md` and the disable dialog were corrected.
  - The pre-existing automation gap was filed as #1824.
- After round 1:
  - `cargo test -p yulius --test main`: 2837 passed, 0 failed, 26 ignored.
  - `cargo test -p yulius-app --lib`: 740 passed, 1 failed. The failure was
    `sweep::tests::over_its_cap_a_build_directory_loses_its_least_recently_used_incremental_units`
    (`busy: true`), which passed 3/3 alone. `sweep.rs` is untouched here; filed as #1826.
  - Clippy with `-D warnings`: clean. `cargo doc -p yulius-app --no-deps`: no warnings.
- Review round 2 (head 85e97d935) requested changes, answered as follows:
  - `t-after`, a live task handled after the failing one, now shows that the loop goes on past a
    failed stop. Replacing `continue` with `return` in the failed-stop branch fails the test
    (`t-after: Running`).
  - The notice's reset on a hand-over is tested: after Bob takes control and drops to read, he
    hears one notice naming him. Deleting the reset in `task_control.rs` fails the test (0
    notices).
  - A credential issued again now ends the notice's episode (test red first: 0 notices).
- After round 2: `cargo test -p yulius --test main` gave 2837 passed, 0 failed.
  `cargo test -p yulius-app --lib` gave 741 passed, 0 failed. Clippy `-D warnings` and
  `scripts/dev quick` passed.
- Security scan: Vidoc is turned off for this project (`yulius security-scan` reports it as
  disabled). Not run, not passed.
- Not tested: a real agent process being stopped on disable. The harness has no host to reap the
  seeded run, so the test accepts `Stopping`, as the existing stop tests do.
