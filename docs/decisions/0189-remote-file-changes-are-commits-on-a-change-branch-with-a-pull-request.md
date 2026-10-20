# 0189. Remote file changes are commits on a change branch with a pull request

## Status

Accepted. Specified in [SPEC-247](../../.ai/specs/SPEC-247-drive-yulius-fully-from-a-local-agent-cl.md)
RMT-024..026; task #233. Applies ADR-0109 (CD runs verified commits out of the agents' reach)
and ADR-0175 (environment runs read a mirror of origin, never the sandbox's clone).

## Context

- Onboarding a repository from a laptop means adjusting its files: above all `yulius.toml`'s
  `[[ci.jobs]]`, which set up Yulius CI. The person's agent has no checkout of the server's
  copy, and the web UI's editor writes into a live sandbox.
- Writing into a sandbox's checkout or a task's worktree behind its agent's back loses work,
  confuses the agent and changes code nobody reviewed or tested.
- Agents can write into the sandbox's clone (its `.git` is shared with their worktrees): its refs
  and objects, even `git replace` entries, are not trustworthy as the base of a commit pushed in
  a person's name.
- Yulius CI already tests every open pull request into the integration branch, including one that
  adds the CI itself (the bootstrap path of `ci_sync_sandbox`).

## Decision

- **Reads** (`project_files`, `project_file_read`) come from the sandbox's clone after a fetch, at
  the integration branch (the default branch while it does not exist yet), a branch of origin or
  a commit, as the web UI's Git views read: informational, at most 256 KiB of text per file, 1000
  entries per directory.
- **Changes** (`project_change`) are whole files or deletions, at most 50, 256 KiB each, at plain
  repository-relative paths (no `..`, no absolute path, nothing in `.git`, no backslash or control
  character). Yulius clones the repository from GitHub afresh with the person's token into
  `<data>/changes/<id>/`, a directory only the server's user reads (0700) and outside every
  sandbox: agents can write anywhere under their sandbox, so a clone there could have its
  configuration changed between the clone and the push (sending the person's token through a
  proxy) or its files changed before the commit. At most 2 changes are made at once; a third is
  refused with "try again" rather than queued. A change runs to its end even when its caller goes
  away (it is spawned), so its scratch clone is removed and its pull request recorded. Yulius adds a
  worktree on a new branch `yulius/change-<id>-<slug>` from
  `origin/<integration>`, writes the files (refusing any path through a symbolic link of the
  repository), commits them as one commit authored by the person, pushes the branch and opens a
  pull request into the integration branch as the person. The scratch directory is removed on
  every path. A change that changes nothing is refused before anything is pushed. Audited
  `project.change`.
- **Merging** (`change_merge`) takes only a pull request `project_change` opened, as its
  `project.change` audit entry records it (pull request, branch and commit), still open, into the
  integration branch, from the repository itself, with its head still the commit Yulius made: a
  branch someone else named `yulius/change-…`, or one pushed to since, is refused. After a fetch,
  it decides as delivery does: Yulius CI's run at the exact head alone when the integration branch
  or the pull request itself has Yulius CI (pending or failed refuses, with the state), else
  GitHub's checks, and a project with no checks at all merges unchecked. It merges that head with
  the profile's merge method as the person and deletes the branch. Audited
  `project.change_merged`. A task's pull request is never merged this way: its task delivers it.
- **Work that needs judgment** is a task (`task_create`): the agent edits, tests, gets reviewed
  and delivers through the one delivery path.

## Alternatives

- **Write into the sandbox's checkout.** Rejected: running tasks branch from it and agents read
  it; nothing would test or review the change.
- **Commit from the sandbox's clone.** Rejected: agents can plant refs and replacement objects in
  it, which a commit made there would carry to GitHub in the person's name.
- **Every change through a task.** Kept for anything needing judgment, rejected as the only way:
  a whole agent run, with its cost and nondeterminism, to write a file the person already wrote.
- **Push straight to the integration branch.** Rejected: it skips CI and the protected-branch
  rules that keep agents (and their tokens) off `dev` and `main`.
- **A patch (diff) instead of whole files.** Rejected for now: whole files are unambiguous for an
  agent to produce and for Yulius to check; a patch that no longer applies would need a third
  outcome.

## Consequences

- A person's agent sets up CI from a laptop: read `yulius.toml`, `project_change` it, follow
  `ci_runs`, `change_merge` once green.
- Each change costs a fresh clone; large repositories take longer, bounded by the request's time.
- The `project.change` audit entry is the record `change_merge` trusts; when it cannot be
  written, the result says so and the pull request is merged on GitHub instead.
- A server killed in the middle of a change leaves its scratch clone in `<data>/changes/`, which
  the next change does not reuse; it is removed by hand (it holds no credential once the clone's
  configuration is scrubbed, as it is before every push).
- Change branches are ordinary branches: `repo_hygiene` reports one left unmerged like any other.
- Reads can be stale by a fetch and can be influenced by an agent writing into the sandbox's
  clone; they are informational, and every change starts from GitHub's own branch.
