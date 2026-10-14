# 0109. CD runs the repository's scripts from protected branches, inside the sandbox

## Status

Accepted. Specified in [SPEC-134](../../.ai/specs/SPEC-134-implement-cd-it-should-write-cd-bash-scr.md);
task #113. Builds on [0015](0015-yulius-ci.md) (Yulius CI) and [0105](0105-agents-copy-allowed-secrets-into-an-ignored-env.md)
(allowed secrets); leaves [0010](0010-delivery-profiles.md)'s delivery environments as they are.

## Context

The person asked for CD "at the same level as CI", "a separate mechanism like CI", not GitHub, with
bash scripts that deploy, dev and production "explicitly defined but both can be empty", agents
that "can build and improve the infra but can't cheat it", and no dev deploy after every merge.
Deployments differ by project: for Yulius, dev is a machine not chosen yet and production is a push
to `main` that instances install by themselves; for others, dev is a roll-out on the team's machine
and production is the customers' platform.

Yulius already had deploy environments (`[[delivery.environments]]`): checkpoints of each task's
delivery, with commands in the database profile that only administrators edit, run with the
server's privileges in the shared clone, and no secrets. They are tied to every merge, agents cannot
build them, and a command that runs a repository script would run agent-written code as the server.

## Decision

- **Configuration.** `yulius.toml` has `[cd.dev]` and `[cd.prod]`, each with optional `deploy` and
  `verify` commands, `secrets` (names) and `about`; production also takes `promote`. A missing table
  is "not defined" (a gap), a table with nothing to run is "empty" (a valid, explicit state).
- **Scripts in Git.** Deploy and verify scripts are committed (`scripts/cd/` by convention) and go
  through review and CI like code. Machines, addresses and credentials are agent secrets. Run
  records and logs live in Yulius.
- **Protected sources only.** Dev reads its table and scripts at the integration branch head;
  production at the commit it deploys (the integration head it promotes, or the production head).
  A task branch, a worktree or the database never changes what a deploy runs.
- **Run like CI, not on the server.** A CD run checks out that exact commit into the environment's
  own checkout and runs `deploy` then `verify` through the process host inside the sandbox's
  containment, bounded by the CI job timeout. It has its own `cd_runs` table: a deploy result never
  counts as a test of its commit, and a run left by a restart is recorded as `error`, never run again
  by itself.
- **Secrets.** A run gets the real values of the secrets its table names, only when a person allowed
  each one outside the proxy (ADR-0105's allowance); its home is made for the run and removed after
  it; its stored output is redacted.
- **Separate from merges.** A merge never deploys. A person (CD page), an agent (`cd_deploy`) or an
  automation's task deploys dev when it is wanted.

## Alternatives

- **Extend `[[delivery.environments]]`.** Their commands are database rows run unsandboxed and tied
  to each task's delivery stage: agents could not build them, and making them repository scripts
  would run agent code with the server's privileges.
- **CD runs as CI runs with a new reference kind.** The CI planner tests a commit once, whatever
  reference carries it, and the merge gate reads runs by commit: a deploy result would count as a
  test, and a restart would run a deploy again.
- **Read the configuration from the task's worktree.** An agent could run anything as a deploy
  without review.
- **A second, CD-only allowance for secrets.** The exposure is the one ADR-0105 already asks a
  person about: the value leaves the proxy for code the project runs.

## Consequences

- Agents improve CD by pull requests; what they merge into the integration branch reaches dev on
  the next dev deploy, and production only through a person's production deploy.
- Every project states its CD explicitly, even when it deploys nothing yet.
- Deploys use the sandbox's network: SSH and HTTPS to machines work, the host's loopback does not.
- `[[delivery.environments]]` keep working for projects that use them; new projects use CD.
