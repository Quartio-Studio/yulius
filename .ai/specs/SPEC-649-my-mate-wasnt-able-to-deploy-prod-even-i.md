---
yulius-task: 7be7a1a2-8398-48a1-b076-c86c2442d2cf
status: done
---

# SPEC-649: The person who creates a project is its admin on GitHub, so they can deploy its production

Status: done
Date: 2026-11-25
Owner: Yulius task #633
Scope authorization: the person's request of 2026-11-25 ("my mate wasnt able to deploy prod even
ih he was an admin, fix"). Issue [#1749](https://github.com/Quartio-Studio/yulius/issues/1749).

## Goal

An instance administrator who creates a project in Yulius (New project → Create, from a template)
in the instance owner's organization is an admin of the new GitHub repository, as GitHub makes the
creator of a repository its admin, so they can deploy its production on the CD page. When GitHub
only invites them, refuses or fails, the project is still created, the outcome is audited and the
page tells them which and what to do.

## Context

- Production deploys need administrator access to the project (ADR-0110, SPEC-134): `may_deploy`
  in `crates/yulius-domain/src/cd.rs` refuses a person without it ("deploying production needs
  administrator access to the project"), and the CD page disables "Deploy production…" with
  "Only an administrator deploys production" (`can_deploy_prod`).
- Access mirrors GitHub (ACC-007): `App::access_level` asks GitHub for the person's repository
  permissions with their own token; only the superadmin is an exception (ACC-012). Instance
  administration grants no repository access (ACC-009).
- The report names no project, and this task cannot read the instance's audit log or users. On
  `Quartio-Studio/yulius` both people of the team are admins on GitHub today
  (`GET /repos/Quartio-Studio/yulius/collaborators/<login>/permission` → `admin` for both); every
  production deploy on record was made by the organization's owner, and no promotion pull request
  or release branch of anyone else exists. A GitHub admin passes the administrator check, so on
  that project the mate met another refusal (the rollback gate, the staging-verified revision,
  secrets, an update under way) or was not admin at the time. Which one is not confirmed.
- What this task confirmed in the code is a defect that produces exactly the reported symptom for an
  instance administrator ("an admin" in Yulius's Users page) on a project they created:
  `create_repository` (`crates/yulius-app/src/projects.rs`) creates an organization repository
  with the GitHub App's installation token, so on GitHub the App is its creator, and grants no role
  to anyone. The person who created the project gets only the organization's base permission (read
  by GitHub's default, often write; none hides it), while an organization owner is admin of every
  repository. So an instance administrator who is not an owner creates a project they cannot
  administer or deploy.

## Root cause

Symptom: an instance administrator creates a project and cannot deploy its production (nor remove
it or change its delivery profile): Yulius rates them below admin.

Code path: `create_sandbox` → `create_repository` → `GitHubPort::create_repo(installation token,
owner, is_org = true, …)`; nothing grants the creator a role. Later `access_level` → `repo_access`
with the creator's token → GitHub's `permissions` without `admin` → `AccessLevel::Write` →
`can_deploy_prod: false` and `CdRefusal::ProductionNeedsAdministrator`.

Why the tests missed it: `create_from_template_pushes_a_validating_scaffold_with_rulesets` grants
the creator admin on the repository before it exists (`w.grant("alice", "new-shop", "admin")`), so
the fake GitHub answers as if the creation had made them admin.

What else the cause breaks: every Administer action of the new sandbox for its creator (remove it,
delivery profile, production protection, Vidoc switch on its CI/CD page, production rollback and
cancel); a creator whom the organization gives no base permission does not even see the project
they created. The "created but unreachable" advice to import the repository has the same gap for a
non-owner creator (#1751, outside this change).

## Requirements

- CRA-001: Creating a project in an organization owner account grants the creating person the
  `admin` role on the new repository right after GitHub creates it, with the installation token
  (`PUT /repos/{owner}/{repo}/collaborators/{login}` with `{"permission":"admin"}`, App permission
  Administration: write, which the App already has for repository creation and rulesets).
- CRA-002: GitHub's 204 means they are its admin now; 201 means GitHub invited them (not a member
  of the organization) and they are admin once they accept; any other status is not GitHub's
  documented answer. None of these, nor a refusal or a failure, stops the creation. Every outcome is
  audited as `sandbox.creator_admin` about `owner/name` when it happens (also when setting up the
  project fails afterwards), and a refusal or failure is logged.
- CRA-003: The creation's answer carries the creator's access as before (`access`) and, when they
  are not made its admin, `admin_grant`: `invited`, `refused` or `failed`. The page words each
  beside "Created": accept GitHub's invitation, or ask an owner of the organization to make them
  its admin to deploy production. A creator made admin, a refusal to someone who is its admin
  anyway (an organization owner) and a personal account carry no `admin_grant` and show no warning.
- CRA-004: Unchanged: the superadmin without a GitHub identity (admin through ACC-012, no grant
  asked), a personal owner account (the repository is created with the owner's own token, the owner
  is its only admin), imports and playground moves (the person already has access on GitHub).
- CRA-005: The role goes to the login the creator's own GitHub token names now, and only while that
  token names the account they signed in as (`github_id`): GitHub frees a renamed account's login
  for anyone (#653). An account that cannot be confirmed gets nobody the role (`failed`).
- CRA-006: A grant that fails in a way that may pass (a 5xx, a timeout, the network; the adapter's
  `map_grant` keeps those `Upstream`) is asked again, at most 3 times in all, with a short pause;
  a refusal (a 4xx) is asked once, and so is a rate limit (`Capacity`), which is waited out
  rather than asked into; both end the grant (`refused`, `failed`).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] The GitHub client grants a person a repository role, with its port and fakes
- [x] A project created in an organization makes its creator a repository admin; an invitation or refusal is audited and shown
- [x] Docs, ADR-0476 and the changes entry true to the fix
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- AC-1 (CRA-001): an instance administrator whom the organization gives write creates `new-shop`:
  GitHub received one grant of `admin` to them with the installation token, their `access` is
  `admin` with no `admin_grant`, the audit says "alice is an admin of Quartio-Studio/new-shop", the
  CD overview says `can_deploy_prod: true`, and their production deploy passes the administrator
  check (it stops at the promotion, which the fake GitHub cannot serve: 409 "branch 'dev' does not
  exist on GitHub").
- AC-2 (CRA-002, CRA-003, CRA-006): an invitation, a 403 refusal (asked once), three 5xx in a row
  (asked 3 times) each create the project with `access` `write`, `admin_grant` `invited`,
  `refused`, `failed` and the matching audit sentence; one 5xx then success makes them admin; a
  refusal to an organization owner is audited as "they are its admin already" with no
  `admin_grant`.
- AC-3 (CRA-004): a project created in a personal owner account asks for no grant and carries no
  `admin_grant`; the creator who is not its owner has what GitHub gives (`none`).
- AC-4 (CRA-001, CRA-002): the client sends `PUT /repos/{o}/{r}/collaborators/{login}` with the
  role, reads 204 as added and 201 as invited, gets 404 for a login GitHub does not know, refuses an
  invalid login or role before sending, and a token that cannot administer is refused.
- AC-5 (CRA-005): a creator renamed on GitHub since signing in is granted under the new login; a
  token naming another account gets nobody the role, `admin_grant` `failed` and an audit saying so.
- AC-6 (CRA-003): the page shows each outcome's warning (invited, refused, failed) and none for a
  creator made admin.

## Test plan

- `crates/yulius/tests/projects.rs`: the creation test no longer grants the creator before the
  repository exists; `the_creator_of_a_project_is_its_admin_and_may_deploy_production` (AC-1),
  `an_invited_refused_or_failed_creator_still_gets_the_project_and_the_reason` (AC-2),
  `the_admin_role_goes_to_the_creators_current_login_only` (AC-5), the personal account test
  without its pre-grant (AC-3). Run the whole `yulius::main` binary.
- `crates/yulius-github/tests/client.rs`: `a_collaborator_is_added_or_invited_with_a_role` (AC-4).
- `e2e/tests/home.e2e.ts`: the three warnings and the admin case without one (AC-6).

## Decisions

- The creator's admin role comes from GitHub, not from Yulius: ACC-007 and ACC-009 stay (instance
  administration grants no repository access), and a production deploy acts as the person on GitHub
  (the promotion merges as them), so a Yulius-only admin would fail there instead (ADR-0476).
- A refused, pending or failed grant never undoes the creation: the repository exists on GitHub by
  then, and an organization owner can grant the role later; the person is told instead.
- The outcome reaches the page as an optional `admin_grant` on the creation's `Sandbox` answer
  (absent everywhere else), rather than a new response type: one producer sets it, and readers that
  do not know it are unaffected.
- The reported case is not confirmed; the fix is delivered for the confirmed defect and says so in
  the changes entry and the pull request.

## Evidence

Round 0 (head `222aca159`):

- Reproduced on dev (`4299f0eac`, test only): `scripts/cargo nextest run -p yulius -E
  'binary_id(yulius::main) & test(/^projects::the_creator_of_a_project/)'` failed with
  `"access": String("write")` for the creator of `new-shop` (left: "write", right: "admin").
- `scripts/cargo nextest run -p yulius -p yulius-app --no-fail-fast`: 4223 passed, 26 skipped.
  Yulius CI run `9236c909` on `222aca1`: passed (the full suite runs on the merged revision).
- Mutation checks by hand: skipping the grant (`if false && is_org`) failed the regression test,
  the creation test and the invitation test; dropping the "already admin" check after a refusal
  failed the invitation test; removing the page's warning failed the e2e test. Each restored.

Review round 1 (findings answered in the pull request):

- The new tests failed on `ee478fb7f` against the round-0 code: no `admin_grant` (`left:
  (String("write"), Null)`), the audit on the sandbox instead of the repository, and the stale
  login granted (`left: [("Quartio-Studio/renamed", "alice", "inst-7")]`).
- With the round-1 fix: `projects::` 37 of 37; `scripts/cargo nextest run -p yulius -p yulius-app
  -p yulius-contracts -p yulius-server --no-fail-fast`: 4310 passed, 26 skipped;
  `scripts/cargo nextest run -p yulius-github --all-features`: 66 passed; e2e
  `npx e2e run tests/home.e2e.ts`: 6 of 6 (the three warnings, and none for a creator made admin);
  `scripts/cargo clippy --locked -p yulius-github -p yulius-app -p yulius -p yulius-contracts
  --all-targets --all-features -- -D warnings`: clean; `scripts/dev quick`: passed.
- Mutation checks by hand, each restored: dropping the account id check failed
  `the_admin_role_goes_to_the_creators_current_login_only`; granting the stored login instead of
  the current one failed it too; never asking again after a 5xx failed
  `an_invited_refused_or_failed_creator_still_gets_the_project_and_the_reason` (the `flaky` case).
- Vidoc: `yulius security-scan` answered "Security scan disabled: Vidoc is turned off in Yulius,
  for this project or the whole instance; nothing was scanned": not run, not passed.

Review round 2 (approved `b3368ec59`; its minors and nits fixed after the approval):

- The new tests (`6b8254e00`) failed as intended: `map_grant` missing (compile error in
  `wiring::github_client::tests`), then with the mapping alone the rate-limited case read
  `admin_grant` `refused` instead of `failed`. With the fix: `projects::` 38 of 38 and
  `wiring::github_client::tests` pass (41 tests), including
  `a_grant_is_audited_when_setup_fails_after_it`; clippy on `yulius-app` and `yulius` clean;
  e2e `home` and `playground` 15 of 15; `scripts/dev quick` passed.
- The non-owner creator in a personal account (pre-existing) is #1753.

Not testable here: GitHub's real answer to `PUT …/collaborators/{login}` for this organization (the
sandbox's installation token reaches only `Quartio-Studio/yulius`; GitHub documents 204 for an
organization member or collaborator and 201 for an invitation). The mate's existing project, if he
created it, keeps its GitHub roles: an owner of the organization grants him admin there (Settings →
Collaborators and teams).
