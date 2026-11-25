# 0476. The creator of a project is made its admin on GitHub

## Status

Accepted for [SPEC-649](../../.ai/specs/SPEC-649-my-mate-wasnt-able-to-deploy-prod-even-i.md)
(#1749).

## Context

An instance administrator creates a project from a template (PRJ-004). In an organization
Yulius creates the repository with the GitHub App's installation token, so on GitHub the App is
its creator and nobody is made its admin. The person who asked has only what the organization
gives every member (its base permission: read by GitHub's default, often write), while an
organization owner is admin of every repository. Access mirrors GitHub (ACC-007) and instance
administration grants no repository access (ACC-009). So that person could not deploy the new
project's production ("deploying production needs administrator access to the project",
ADR-0110), remove it or change its delivery profile. Nobody noticed: the organization's owner,
admin everywhere, made the projects, and the creation test granted the creator admin before the
repository existed. It was found while investigating a report that an admin could not deploy
production. Which project and refusal that report met is not confirmed.

## Decision

- Right after GitHub creates an organization's repository for a project, Yulius gives the person
  who created it the `admin` role with the installation token (`PUT
  /repos/{owner}/{repo}/collaborators/{login}`). That is App permission Administration: write,
  which the App already needs to create repositories and rulesets. Creating the repository on
  GitHub would have made them its admin.
- The role goes to the login the creator's own GitHub token names now, and only while that token
  names the account they signed in as (its id). GitHub frees a renamed account's login for anyone
  (#653), so the login stored at sign-in could name a stranger. When the account cannot be
  confirmed, nobody is made admin.
- GitHub's 204 means they are its admin now. Its 201 means GitHub invited them (not a member of the
  organization), and they are its admin once they accept. A 4xx is a refusal, asked once. A 5xx, a
  timeout or the network is asked again, at most 3 times in all, with a short pause.
- Every outcome is audited as `sandbox.creator_admin` about `owner/name` when it happens, so an
  invitation is on record even if setting up the project fails afterwards. A refusal to someone
  who is its admin anyway (an organization owner) is audited as that, not as a failure.
- An invitation, a refusal or a failure never undoes the creation, since the repository exists by
  then. The creation's answer carries `admin_grant` (`invited`, `refused` or `failed`), and the page
  words it with what to do: accept the invitation, or ask an owner of the organization.
- No grant is asked for the superadmin without a GitHub identity (admin of every sandbox,
  ACC-012). None is asked in a personal owner account either: the owner's own token creates the
  repository and makes them its admin, and a personal repository has no other admin. Imports and
  playground moves are unchanged, since the person already has access on GitHub.

## Alternatives

- Treat instance administrators, or a sandbox's `created_by`, as admins in Yulius: refused. It
  breaks ACC-007 and ACC-009 for every Administer action. A production deploy also promotes as the
  person on GitHub, where a production ruleset lets only repository admins update the branch
  (ADR-0114), so the deploy would fail there instead.
- Create the repository with the creator's own token: an organization member often may not create
  repositories, the App's installation token is what PRJ-004 relies on, and the scaffold push and
  rulesets already use the App.
- Refuse the creation when the grant fails: the repository already exists on GitHub. Refusing would
  leave it orphaned without a sandbox, and an owner can grant the role later.
- Grant to the login stored at sign-in: refused, a renamed account's old login may belong to
  someone else by then.

## Consequences

- A person who creates a project of an organization can administer it and deploy its production,
  as an organization owner always could.
- The App uses its existing Administration: write permission for one more call per creation, and
  reads the creator's account once. An organization policy that forbids inviting outside
  collaborators shows as an audited refusal and the page's warning, not a failed creation. An
  outside collaborator invited as admin of a private repository takes a seat on paid plans.
- Projects created before this change keep their GitHub roles: an organization owner grants their
  creators admin on GitHub where needed.
