# 0223. A reference copy carries every branch and tag its owner can read

## Status

Accepted. Specified in [SPEC-330](../../.ai/specs/SPEC-330-fix-this-tasks-act-as-the-user-they-shou.md);
task #314, fixes #915. Amends [ADR-0197](0197-tasks-read-other-repositories-of-their-owner-through-a-bundle.md)
("fetches its default branch"); keeps [ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md).

## Context

A task of `pomerado-rust` needed Quartio v2's `dev` branch. `reference_repo` fetched only `main`, the
default branch, and the agent's own `gh` and `git` reach only its own repository, so it asked a
person to paste files or to make `dev` the default branch. The person's Superagent, running with
the person's own `gh`, cloned `dev` at once. The person: "Tasks act as the user. They should have
all things that the user has." GitHub grants read access per repository, not per branch, so the
default-branch-only copy gave the task less than its owner can read.

## Decision

- **The bundle carries everything readable in the repository.** Yulius fetches every branch and
  every tag with the owner's token and bundles them with `HEAD` on the default branch; the clone has
  every branch as `origin/<name>` and every tag.
- **The task picks the branch to check out.** `reference_repo` takes an optional `branch` (default:
  the default branch); a branch the repository lacks is invalid and the answer lists the branches,
  so the agent corrects itself without a person. The answer lists the branches it carries.
- **The rest of ADR-0197 stands**: same owner only, the agent never holds the person's token, Git
  runs on the host only in the server's own folder, one bundle per repository per task.

## Alternatives

- **Only an optional `branch`, still one branch per bundle**: fixes this case but a task porting
  from two branches (or reading a tag) calls again and replaces its copy each time; the whole
  repository is rarely much larger than its default branch's history.
- **Give tasks the person's own GitHub token, as the Superagent has**: would give "all things"
  literally, but that token can push to production and delete its ruleset (ADR-0114); reading is
  what the person needs here, and Yulius can give all of it without the token.
- **Lift the same-owner rule**: a different concern (prompt injection pulling another
  organisation's private code into a sandbox, ADR-0197); not part of this defect.

## Consequences

- A task reads any branch or tag of its owner's organisation repository without a person; nobody
  is asked to change a repository's default branch for a task.
- Bundles grow by the history only other branches and tags reach; the 1 GiB cap of ADR-0197
  bounds what a task receives, and its two concurrent fetches bound the work, but the cap is
  checked after the fetch, so the server's `<data>/tmp` briefly holds the whole repository
  (#920).
- The answer gives the clone and update commands with every word shell-quoted (`clone_command`,
  `update_command`): a branch name may hold `$`, `(` or `'`, and the update forces branches and
  tags so a re-pointed tag does not fail it.
- A `branch` the repository lacks is only known after the fetch, so a misspelling costs one fetch
  (bounded by the two concurrent fetches); the answer then lists the branches.
- A repository with very many branches answers the first 100 by name; all of them are in the
  bundle.
