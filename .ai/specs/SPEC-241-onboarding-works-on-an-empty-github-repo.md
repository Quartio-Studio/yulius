---
yulius-task: 9789f9e0-021a-474f-9d62-afc1f0e0837e
status: in-progress
---

# SPEC-241: Onboarding works on an empty GitHub repository

Status: in-progress
Date: 2026-10-20
Owner: task #227
Scope authorization: task #227, feedback from an early external user on a fresh install: (1) a new
GitHub repository with no commits, onboarded into Yulius, failed with "Couldn't start onboarding:
git: revision not found: origin/master"; (2) before that, creating the repository from Yulius
failed with "some permissions" error. Extends SPEC-051/SPEC-097 (onboarding) and PRJ-003..005
(create from a template, SPEC-001).

## Goal

A brand-new, empty GitHub repository (no commits, no branch yet) can be added to Yulius and
onboarded without any manual step. Creating a repository from Yulius either works or fails with a
message that names the GitHub permission or setting that is missing and where to change it.

## Context

Confirmed against `dev` at 33410359.

- **Empty repository, wrong branch.** `link_repository` (`crates/yulius-app/src/projects.rs`)
  clones the repository and asks Git for its default branch first
  (`GitCli::default_branch`, `crates/yulius-git/src/repo.rs`): `refs/remotes/origin/HEAD`, else
  the branch the clone's `HEAD` names. An empty clone has no `origin/HEAD`, and its `HEAD` names
  the host's own `init.defaultBranch` (`master` when unset), not GitHub's. Only an empty answer
  falls back to GitHub's `default_branch`. So the sandbox records `master`.
- **Empty repository, no base.** The onboarding task's worktree is created from
  `origin/<integration>` when that branch exists, else `origin/<default_branch>`
  (`Supervisor::ensure_worktree`, `crates/yulius-app/src/supervisor.rs`). With no commits neither
  exists, so `add_worktree` fails with `RevisionNotFound("origin/master")`. `approve_onboarding`
  returns it and the web UI shows "Couldn't start onboarding: git: revision not found:
  origin/master". Whatever branch name were recorded, an empty repository has nothing to branch
  from.
- **Creating a repository.** `create_repository` creates it on GitHub (an organization: the App's
  installation token, `POST /orgs/{org}/repos`; a personal account: the owner's sign-in token,
  `POST /user/repos`), then clones it and pushes the scaffold with the installation token. GitHub
  documents both creation endpoints as needing the App's **Administration: write** (or, since
  2026-10-13, **Repository creation: write**) permission, for installation and user access tokens.
  A refusal is a 403 "Resource not accessible by integration" with the accepted permissions in the
  `X-Accepted-GitHub-Permissions` header. Yulius drops the header and shows
  "GitHub: GitHub refused the request (HTTP 403): Resource not accessible by integration".
- **The second permission failure.** An App installed on **Only select repositories** may create a
  repository that its installation does not then include (reported for user access tokens; GitHub
  adds a blank repository it creates in most cases, not always). The scaffold's clone or push with
  the installation token then fails with "Repository not found" or "Permission denied", and the
  GitHub repository stays, empty. That empty repository is what the user then onboarded.
- **Not reproducible here.** This instance has no throwaway GitHub account to create repositories
  in. The refusals are reproduced with the test doubles GitHub's documentation describes (a 403
  with `X-Accepted-GitHub-Permissions`; a created repository missing from the installation).

## Requirements

- **EMP-001** Adding a repository whose clone has no branches records GitHub's reported default
  branch (`main` when GitHub reports none), never the name the empty clone's `HEAD` happens to
  hold.
- **EMP-002** Starting onboarding (the browser's Add repository, Start onboarding, or any later
  start of the onboarding task) on a sandbox whose remote has no branches first creates one commit
  on the default branch and pushes it. The branch is GitHub's reported default branch, `main`
  when GitHub reports none; the sandbox's record is corrected when it differs. The commit adds
  only `README.md` with the repository's name as a heading, as GitHub's "Add a README" does; it is
  authored by the person who starts onboarding (else the sandbox's creator) and pushed with the
  installation token, as a created project's scaffold is. Onboarding then continues as for any
  repository with one branch.
- **EMP-004** When the clone lacks the recorded default branch but GitHub's reported default
  branch exists on origin (someone pushed since the import), starting onboarding corrects the
  record and writes nothing to the repository. Concurrent starts of one instance seed one at a time.
- **EMP-003** A repository that has branches is never written to by EMP-002. A push GitHub refuses
  (someone pushed first, a rule) fails the start with the reason, and the sandbox stays `pending`
  so that it can be started again.
- **CRT-001** When GitHub refuses to create the repository for a missing App permission (403), the
  error names the permission in GitHub's words (for example "Administration: Read and write"),
  using the `X-Accepted-GitHub-Permissions` header when GitHub sends it, the App's permission page
  and the installation page where the owner accepts the change, and that nothing was created.
- **CRT-002** When the repository was created but the App's installation cannot reach it (the
  installation token gets 404 for it), the error says that the repository now exists on
  GitHub, that the installation covers only selected repositories, where to add it (the
  installation page), and that it can then be added with Import, which EMP-002 sets up.
- **CRT-003** Other failures keep their current messages, including a 403 that names no
  permission and does not say "Resource not accessible by integration" (an organization's policy).

## Plan

- [ ] Empty repository records GitHub's default branch on import
- [ ] Onboarding seeds an empty repository with a first commit and starts
- [ ] GitHub's accepted permissions reach the create-repository refusal
- [ ] Create names the missing permission or the installation's repository access
- [ ] ADR, operations guide, architecture and changes record the empty-repository path
- [ ] Independent review approved
- [ ] Repository gate and security scan pass
- [ ] Merged into dev on green CI

## Acceptance criteria

- An integration test imports a local bare repository with no commits in the browser: the import
  answers 201 with `default_branch` `main`, onboarding is `preparing`, the onboarding task has a
  worktree on `origin/main`, and the remote's `main` holds one commit with `README.md` only.
- The same with a token import and Start onboarding later, for a sandbox recorded with `master`:
  the record is corrected to GitHub's branch and the push goes to it.
- A repository with commits is not pushed to when onboarding starts.
- A create GitHub refuses with 403 and `administration=write` answers 403 naming "Administration:
  Read and write", the App's permission page and the installation page; nothing is cloned.
- A create whose new repository the installation cannot reach answers with a message naming the
  installation page, Import and the repository; the local directory is removed.
- The adapter test reads `X-Accepted-GitHub-Permissions` from a 403 into the error.

## Test plan

- `crates/yulius/tests/projects.rs`: empty remote (bare repository with no commits) for both start
  paths; repository with commits untouched; the two create refusals with the test GitHub double.
- `crates/yulius-github/tests`: 403 with `X-Accepted-GitHub-Permissions` from the fake GitHub.
- Unit tests for the permission wording (`administration=write; repository_creation=write`).
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **Seed at onboarding start, not at import.** Every start path, including sandboxes already
  imported with the wrong branch, goes through `start_onboarding`, so the seed there repairs them
  too; import stays read-only on GitHub. ADR-0187.
- **A README commit, not an empty commit.** It needs no new Git operation (the scaffold's
  commit-and-push path is reused) and is what GitHub itself offers for a new repository.
- **Installation token for the seed.** Like a created project's scaffold: the commit is Yulius's
  setup, not a task's work, and the person may not have a sign-in token (the superadmin).
- **Diagnose after the failure, not before.** Checking the installation's repositories right after
  creating one could race GitHub's own update and refuse a working setup; the check runs only once
  the clone or push has failed.

## Evidence

Pending.
