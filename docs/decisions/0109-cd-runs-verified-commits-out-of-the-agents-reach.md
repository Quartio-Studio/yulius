# 0109. CD runs the repository's scripts from protected branches, out of the agents' reach

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
- **Protected sources only, verified.** Dev reads its table and scripts at the integration
  branch head; production at the commit it deploys (the integration head decides whether it
  promotes; after a promotion the new production head is read and authorized again). The commit
  is the one GitHub reports for the branch. It is taken from the sandbox clone into a mirror only
  Yulius writes (`<data>/cd/<sandbox>/source.git`), with every object checked
  (`transfer.fsckObjects`) and replacement refs ignored, and read there. A task branch, a
  worktree, the sandbox clone's state or the database never changes what a deploy runs.
- **Run files out of the agents' reach.** A run's files (the commit's tree, its `HOME`, its log)
  live in its own run directory and its cache in `<data>/cd/<sandbox>/cache/<env>`, bound only into
  that run's mount namespace and removed with the run (the cache stays). Each run has its own
  PID namespace, so no agent reads its environment.
- **Dev in the sandbox, production beside it.** Dev runs inside the sandbox's containment: agents
  build and use dev, with the tools they installed. Production joins only the sandbox's network
  (no host loopback, as agents), on the host's read-only `/usr`, `/etc` and `/opt` with empty
  `/var`, `/root` and `/home`, the sandbox's resolver, and only the run's directory and its cache
  bound in: nothing an agent installed or wrote runs with production's secrets, and production
  reaches no more of the host than an agent does. The process host builds this itself from an
  explicit `host_system` mode of the launch spec's join (with the sandbox's resolver bound where
  the host's resolver file really is). Its scripts fetch the tools they need into
  `$YULIUS_CD_CACHE`. Where sandbox isolation does not run, both use the previous containment
  (production with the run's directory as its root), on the network agents have there.
- **Secrets per environment.** Dev runs get the secrets their table names that a person allowed
  outside the proxy (ADR-0105's allowance: dev is the agents' ground, as `.env` is). Production
  runs get only those a person allowed for production deploys, an allowance of its own. A
  production secret is exclusive: it cannot be allowed while agents can use it (`.env` copies,
  proxy hosts), it then refuses both, and dev never gets it. Stored output is redacted.
- **A person confirms what they saw.** The production dialog sends the commit it showed (the
  integration commit a promotion takes, else the production head); a deploy refuses when the
  branch moved since. Whether production already contains the integration commit, and whether
  a promotion reached it, is decided on the verified mirror, not on the agents' clone.
- **Bounded and never repeated.** Runs share the CI job timeout; one runs per environment at a
  time; a run a restart left behind has its process host terminated, its files removed and is
  recorded as `error`, never started again by itself (it has its own `cd_runs` table: a deploy
  result never counts as a test of its commit).
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
- **One allowance for every CD secret** (the first version, ADR-0105's). An allowance for `.env`
  lets any agent of the project use the value, so a production credential allowed that way could
  be used without a person's production deploy (independent review, H1).
- **Production inside the sandbox too.** Its system layer, cache and checkouts are writable by
  the project's agents, who could then change what a production deploy runs or read its secrets
  (independent review, C1).

## Consequences

- Agents improve CD by pull requests; what they merge into the integration branch reaches dev on
  the next dev deploy, and production only through a person's production deploy.
- Every project states its CD explicitly, even when it deploys nothing yet.
- Both deploy over the sandbox's network; production has only the host's base tools, which its
  scripts complete in `$YULIUS_CD_CACHE`.
- Agents can still block a deploy by breaking the sandbox clone (it is refused, never misled);
  and they still hold their task owner's GitHub token, which can write the production branch
  outside Yulius (issue #486).
- What the CD page shows is read from the clone as last fetched; what a deploy runs is verified
  again at the deploy.
- `[[delivery.environments]]` keep working for projects that use them; new projects use CD.
