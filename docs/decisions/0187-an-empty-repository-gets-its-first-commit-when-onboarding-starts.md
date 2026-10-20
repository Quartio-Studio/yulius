# 0187. An empty repository gets its first commit when onboarding starts

## Status

Accepted. Specified in [SPEC-241](../../.ai/specs/SPEC-241-onboarding-works-on-an-empty-github-repo.md);
task #227. Extends SPEC-051/SPEC-097 (onboarding) and PRJ-003..005 (create from a template).

## Context

- A new GitHub repository with no commits could be added but not onboarded: "Couldn't start
  onboarding: git: revision not found: origin/master". The onboarding task's worktree starts from
  `origin/<integration>` or `origin/<default branch>`, and an empty repository has neither.
- The recorded branch was also wrong. An empty clone has no `origin/HEAD`, and its `HEAD` names the
  server host's own `init.defaultBranch` (`master` when unset), which Git's answer preferred to
  GitHub's `default_branch`.
- The same user's earlier attempt to create the repository from Yulius failed with GitHub's bare
  "Resource not accessible by integration". GitHub needs the App's Administration (or, since
  2026-10-13, Repository creation) write permission to create a repository, and an installation on
  only selected repositories may not reach a repository it created. Its 403 names the accepted
  permissions in `X-Accepted-GitHub-Permissions`, which Yulius dropped.

## Decision

- Import records GitHub's reported default branch for a clone without branches, never the empty
  clone's `HEAD`.
- `start_onboarding` (every start: Add repository in the browser, Start onboarding, later starts)
  checks the sandbox's clone; when it lacks the recorded default branch, it fetches again (someone
  may have pushed since) under a lock that serializes these first commits, and asks GitHub for the
  repository's default branch with the installation token. When origin now has that branch and the
  record names another, the record is corrected. When origin still has no branch, Yulius commits `README.md` (`# <name>`)
  on GitHub's reported default branch (`main` when it reports none or a name Git refuses), authored
  by the person starting onboarding (else the sandbox's creator), pushes it with the installation
  token as a created project's scaffold is, fetches again and corrects the recorded branch. It is
  audited as `sandbox.repository.seeded`. A repository with any branch is never written to; a
  refused push fails the start, which stays `pending`.
- `GitHubError::Forbidden` carries `accepted_permissions` from the header. The `create_repo` port
  returns `Forbidden(accepted)` for a 403, and creating a project then answers 403 naming the
  permission in GitHub's words ("Administration: Read and write (or Repository creation: Read and
  write)"), the App's permission page and its installation page, and that nothing was created.
- A 403 without accepted permissions and without "Resource not accessible by integration" (an
  organization's policy) keeps GitHub's message.
- When the scaffold of a created repository cannot be cloned or pushed and the installation token
  cannot see the repository (404), the answer says that the repository exists, that the
  installation covers only selected repositories, where to add it, and to import it then.

## Alternatives

- **Seed at import.** Rejected. Sandboxes imported before the fix, with `master` recorded, would
  stay broken, and adding a repository would write to it before anyone chose to onboard it.
- **An empty commit.** Rejected. It needs a new Git operation, and GitHub's own "Add a README" is
  what a person expects to see first.
- **Push the seed as the person.** Rejected. The superadmin has no GitHub token. The commit is
  Yulius's setup, like a created project's scaffold, and it is authored by the person all the same.
- **Check the installation's repositories right after creating one.** Rejected. GitHub may list a
  new repository a moment later, so a working setup could be refused. The check runs only after
  the clone or push failed.
- **Ask for "All repositories" in the manifest.** Not possible. The installation's repository
  access is chosen by whoever installs the App, not by the manifest.

## Consequences

- An empty repository onboards without a manual step, and a sandbox stuck with `master` heals at
  its next Start onboarding, whether the repository is still empty or someone pushed to it since.
- A person who creates a project and lacks the permission sees the exact permission and the two
  pages to change it. A repository left behind by an unreachable installation can be imported.
- The empty repository's first commit is on its default branch before any protection exists; a
  rule that forbids creating the branch refuses it, and the start says so.
