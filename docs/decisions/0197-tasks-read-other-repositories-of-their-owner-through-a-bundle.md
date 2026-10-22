# 0197. Tasks read other repositories of their owner through a bundle Yulius fetches as that person

## Status

Accepted. Specified in [SPEC-267](../../.ai/specs/SPEC-267-i-got-this-such-things-should-usually-ha.md);
task #253, fixes #803. Amends SPEC-001 PRJ-002 ("Checking out another repository inside a sandbox
is not supported"); keeps [ADR-0114](0114-agents-hold-a-token-github-refuses-on-production.md)'s
agent token and follows [ADR-0129](0129-onboarding-agents-protect-production-as-the-onboarding-person.md)'s
pattern of a tool acting as the task's owner.

## Context

A task of Quartio-Studio/quartio-v2 had to port code from Quartio-Studio/product. Its agent's token
is restricted to the sandbox's own repository (ADR-0114), so GitHub answered 404 and the agent asked
a person to give the GitHub App read access there. The person asked their Superagent, which runs
with the person's own `gh` sign-in and simply cloned the repository into the sandbox: the person had
nothing to do. They asked for such needs to be settled on onboarding and for the needless "Needs
you" to go away.

## Decision

- **A task tool, `reference_repo {repo}`, fetches as the task's owner.** Yulius checks with the
  owner's GitHub user token that they can read the repository, fetches its default branch with that
  token into a fresh folder of its own (`<data>/tmp`, never inside a sandbox), writes a Git bundle of
  that branch and places it among the task's attachments through the attachment path's handles
  (ADR-0174). The agent clones the bundle itself, in its containment, into
  `<sandbox>/reference/<owner>/<name>`, outside every worktree. Each fetch is audited.
- **Same owner only.** The repository must belong to the sandbox repository's GitHub owner and must
  not be the sandbox's own; the owner's account must be enabled and signed in to GitHub; a
  playground is refused. Otherwise, or when the person cannot read it, the tool refuses and the agent
  asks a person with `needs_you`.
- **Onboarding fetches first.** The onboarding prompts ask the agent to fetch the repositories the
  project refers to and to name them in AGENTS.md; `needs_you`'s description and the briefing send
  agents to `reference_repo` before a person.

## Alternatives

- **Widen the GitHub App's repository access** (what the agent asked for): a person's action for
  every repository, and agents' installation tokens are per repository anyway (ADR-0114), so it
  would still need a token change; it also grants the App write access there.
- **Give the agent a read token for the repository.** A person's user token is never an agent's
  (ADR-0114); a read-only installation token needs the App installed there, which was the problem.
- **Clone or update a checkout inside the sandbox from the server.** Git on the host in a folder an
  agent can write runs that agent's `.git/config` and hooks; the bundle keeps the server's Git in
  its own folder and its only write to the sandbox is one file through a handle.
- **A cached mirror per sandbox.** Saves downloads but needs a lock per mirror and cleanup after a
  sandbox's removal; reference fetches are rare.
- **Any repository the person can read.** Would let a prompt injected into a task pull the person's
  repositories of other organisations into a sandbox; the organisation boundary covers the need.

## Consequences

- A task needs no person to read another repository of its organisation that its owner can read;
  the copy is visible to everyone who can open the sandbox, as files the person adds are.
- Each call downloads the branch's whole history once (bundles are capped at 1 GiB, at most 2
  fetches run at once); a task keeps one bundle per repository, replaced by each call and removed
  with its attachments. The fetch runs to its end even when the agent's request goes away, and a
  fetch folder a restart left is swept by the next fetch.
- The owner GitHub answers with decides, so a repository moved to another organisation is that
  organisation's.
- A repository of another owner, or one the task's owner cannot read, still needs a person.
