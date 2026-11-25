# 0476. The creator of a project is made its admin on GitHub

## Status

Accepted for [SPEC-649](../../.ai/specs/SPEC-649-my-mate-wasnt-able-to-deploy-prod-even-i.md)
(#1749).

## Context

An instance administrator creates a project from a template (PRJ-004). In an organization
Yulius creates the repository with the GitHub App's installation token, so on GitHub the App is
its creator and nobody is made its admin: the person who asked has only what the organization
gives every member (its base permission, typically write), while an organization owner is admin
of every repository. Access mirrors GitHub (ACC-007) and instance administration grants no
repository access (ACC-009), so that person could not deploy the new project's production
("deploying production needs administrator access to the project", ADR-0110), remove it or change
its delivery profile. It went unnoticed because the owner of the organization, admin everywhere,
made the projects, and the creation test granted the creator admin before the repository existed.

## Decision

- Right after GitHub creates an organization's repository for a project, Yulius gives the person
  who created it the `admin` role with the installation token (`PUT
  /repos/{owner}/{repo}/collaborators/{login}`, App permission Administration: write, which the
  App already needs to create repositories and rulesets), as creating the repository on GitHub
  would have made them its admin.
- GitHub's 204 means they are its admin now. Its 201 means GitHub invited them (not a member of the
  organization): its admin once they accept. Neither an invitation nor a refusal undoes the
  creation, since the repository exists by then: the project is created, the outcome is audited as
  `sandbox.creator_admin` with GitHub's reason (nothing for a refused grant to someone who is its
  admin anyway, an organization owner), and the page says, beside "Created", that GitHub gives
  them less than admin and what to do (accept the invitation or ask an owner of the organization).
- No grant is asked for the superadmin without a GitHub identity (admin of every sandbox,
  ACC-012), in a personal owner account (the owner's own token creates it and makes them its
  admin), for an import or a playground move (the person already has access on GitHub).

## Alternatives

- Treat instance administrators, or a sandbox's `created_by`, as admins in Yulius: refused. It
  breaks ACC-007 and ACC-009 for every Administer action, and a production deploy promotes as the
  person on GitHub, where a production ruleset lets only repository admins update the branch
  (ADR-0114): the deploy would fail there instead.
- Create the repository with the creator's own token: an organization member often may not create
  repositories, the App's installation token is what PRJ-004 relies on, and the scaffold push and
  rulesets already use the App.
- Refuse the creation when the grant fails: the repository already exists on GitHub; refusing would
  leave it orphaned without a sandbox, and an owner can grant the role later.

## Consequences

- A person who creates a project can administer it and deploy its production, as an organization
  owner always could.
- The App's existing Administration: write permission is used for one more call per creation; an
  organization policy that forbids inviting outside collaborators shows as an audited refusal and
  the page's warning, not a failed creation.
- Projects created before this change keep their GitHub roles: an organization owner grants their
  creators admin on GitHub where needed.
