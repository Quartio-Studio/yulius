---
yulius-task: 7be7a1a2-8398-48a1-b076-c86c2442d2cf
status: in-progress
---

# SPEC-649: The person who creates a project is its admin on GitHub, so they can deploy its production

Status: in-progress
Date: 2026-11-25
Owner: Yulius task #633
Scope authorization: the person's request of 2026-11-25 ("my mate wasnt able to deploy prod even
ih he was an admin, fix"). Issue [#1749](https://github.com/Quartio-Studio/yulius/issues/1749).

## Goal

An instance administrator who creates a project in Yulius (New project → Create, from a template)
in the instance owner's organization is an admin of the new GitHub repository, as GitHub makes the
creator of a repository its admin, so they can deploy its production on the CD page. When GitHub
only invites them or refuses the grant, the project is still created, the refusal is audited and the
page tells them why they are not an admin yet and what to do.

## Context

- Production deploys need administrator access to the project (ADR-0110, SPEC-134): `may_deploy`
  in `crates/yulius-domain/src/cd.rs` refuses a person without it ("deploying production needs
  administrator access to the project"), and the CD page disables "Deploy production…" with
  "Only an administrator deploys production" (`can_deploy_prod`).
- Access mirrors GitHub (ACC-007): `App::access_level` asks GitHub for the person's repository
  permissions with their own token; only the superadmin is an exception (ACC-012). Instance
  administration grants no repository access (ACC-009).
- Both people of the team are admins of `Quartio-Studio/yulius` on GitHub (`GET
  /repos/Quartio-Studio/yulius/collaborators/<login>/permission` → `admin` for both), and every
  production deploy on record was made by the organization's owner. The mate's attempt left no
  promotion pull request and no release branch: it was refused before the promotion, at the
  administrator check.
- `create_repository` (`crates/yulius-app/src/projects.rs`) creates an organization repository
  with the GitHub App's installation token, so on GitHub the App is its creator, and grants no role
  to anyone. The person who created the project gets only the organization's base permission
  (write or read; none hides it), while an organization owner is admin of every repository. So an
  instance administrator who is not an owner creates a project they cannot administer.

## Root cause

Symptom: an instance administrator creates a project and cannot deploy its production (nor remove
it or change its delivery profile): Yulius rates them `write`.

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
they created.

## Requirements

- CRA-001: Creating a project in an organization owner account grants the creating person the
  `admin` role on the new repository right after GitHub creates it, with the installation token
  (`PUT /repos/{owner}/{repo}/collaborators/{login}` with `{"permission":"admin"}`, App permission
  Administration: write, which the App already has for repository creation and rulesets).
- CRA-002: GitHub's answer 204 means they are a collaborator now; 201 means GitHub invited them
  (not a member of the organization) and they are admin once they accept. Neither, nor a refusal,
  stops the creation: the project is created as before, and an invitation or a refusal is audited
  (`sandbox.creator_admin`, with GitHub's reason) and logged.
- CRA-003: The creation's answer carries the creator's access as before (`access`); when it is not
  `admin`, the page says, beside "Created", that GitHub gives them that access, not admin, and that
  they accept GitHub's invitation or ask an owner of the organization to make them its admin to
  deploy production.
- CRA-004: Unchanged: the superadmin without a GitHub identity (admin through ACC-012, no grant
  asked), a personal owner account (the repository is created with the owner's own token, the owner
  is its admin), imports and playground moves (the person already has access on GitHub).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] The GitHub client grants a person a repository role, with its port and fakes
- [x] A project created in an organization makes its creator a repository admin; an invitation or refusal is audited and shown
- [x] Docs, ADR-0476 and the changes entry true to the fix
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- AC-1 (CRA-001): an instance administrator who is not given the repository by anyone creates
  `new-shop` in the organization: GitHub received one grant of `admin` to them with the
  installation token, their `access` to the sandbox is `admin`, the CD overview says
  `can_deploy_prod: true` and their production deploy is not refused for administrator access.
- AC-2 (CRA-002): GitHub answers the grant with an invitation, or refuses it: the project is
  created, the creator's access is what GitHub gives, and the audit has `sandbox.creator_admin`
  with the reason.
- AC-3 (CRA-004): a project created in a personal owner account asks for no grant.
- AC-4 (CRA-001): the client sends `PUT /repos/{o}/{r}/collaborators/{login}` with the permission,
  reads 204 as added and 201 as invited, and refuses an invalid login before sending.

## Test plan

- `crates/yulius/tests/projects.rs`: the creation test no longer grants the creator before the
  repository exists; a regression test where the creator has no role until the grant asserts AC-1
  (access, `can_deploy_prod`), and variants for the invitation and the refusal (AC-2). The personal
  account test asserts no grant (AC-3). Run the whole `yulius::main` file `projects`, and `cd`.
- `crates/yulius-github/tests/client.rs`: the client's grant against the fake GitHub (AC-4).
- The page: the created toast's wording when `access` is not `admin` (e2e of the New project
  dialog if one covers creation; otherwise by reading the code path).

## Decisions

- The creator's admin role comes from GitHub, not from Yulius: ACC-007 and ACC-009 stay (instance
  administration grants no repository access), and a production deploy acts as the person on GitHub
  (the promotion merges as them), so a Yulius-only admin would fail there instead (ADR-0476).
- A refused or pending grant never undoes the creation: the repository exists on GitHub by then, and
  an organization owner can grant the role later; the person is told instead.

## Evidence

- Reproduced on dev (`4299f0eac`, test only): `scripts/cargo nextest run -p yulius -E
  'binary_id(yulius::main) & test(/^projects::the_creator_of_a_project/)'` failed with
  `"access": String("write")` for the creator of `new-shop` (left: "write", right: "admin").
- With the fix: `projects::` 36 of 36 passed (the regression test, the invitation and refusal
  test, the creation test without its pre-grant, the personal account test asserting no grant);
  `scripts/cargo nextest run -p yulius -p yulius-app --no-fail-fast`: 4223 passed, 26 skipped;
  `scripts/cargo nextest run -p yulius-github --all-features`: 66 passed, including
  `client::a_collaborator_is_added_or_invited_with_a_role` (AC-4).
- e2e: `npx e2e run tests/home.e2e.ts` 5 of 5 passed, including "a created project its creator is
  not admin of says why its production is not theirs yet" (CRA-003).
- Mutation checks by hand: skipping the grant (`if false && is_org`) failed
  `the_creator_of_a_project_is_its_admin_and_may_deploy_production`,
  `create_from_template_pushes_a_validating_scaffold_with_rulesets` and
  `an_invited_or_refused_creator_still_gets_the_project_and_the_reason`; dropping the "already
  admin" check after a refusal failed the last one ("an organization owner is its admin
  already"); removing the page's warning failed the e2e test. Each line restored.
- `scripts/cargo clippy --locked -p yulius-github -p yulius-app -p yulius --all-targets
  --all-features -- -D warnings`: clean. `scripts/dev quick`: passed.
- Vidoc: `yulius security-scan` answered "Security scan disabled: Vidoc is turned off in Yulius,
  for this project or the whole instance; nothing was scanned": not run, not passed.
- Not testable here: GitHub's real answer to `PUT …/collaborators/{login}` for this organization
  (the sandbox's installation token reaches only `Quartio-Studio/yulius`; GitHub documents 204 for
  an organization member and 201 for an invitation). The mate's existing project keeps its GitHub
  roles: an owner of the organization grants him admin there (Settings → Collaborators and teams).
