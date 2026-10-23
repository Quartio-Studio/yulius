# 0207. CD runs restore and save agent states under one hold

## Status

Accepted. Specified in [SPEC-278](../../.ai/specs/SPEC-278-cd-runs-use-agent-states-restore-before.md);
task #264. Amends [0200](0200-operator-state-is-a-sealed-agent-state-with-one-writer.md) (the
single-writer rule) and [0203](0203-project-variables-are-plain-per-project-values-in-task-runs.md)
(variables reach task runs only); extends
[0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md) and
[0195](0195-cd-environments-are-named-and-take-their-rules-from-a-stage.md).

## Context

A deploy tool keeps live state (an inventory database with pinned host keys, rollout folders,
receipts, locks). SPEC-271 moved that state into the vault as an agent state whose writer is the
copy a task restored into the sandbox home. A CD run cannot use it: it gets a fresh `HOME` and
only the secrets its table names, and a production run must not run on files an agent wrote.
The first user, Quartio-Studio/product, deploys dev to its own VM and production to every
customer VM from CD, with a tool that records failed upgrades and rollout locks in that state.

## Decision

- **A CD table names its states**: `states = ["NAME", …]` (or a space-separated string) in
  `[cd.<name>]`, at most 8. A deploy naming a state the vault does not hold is refused, nothing
  ran.
- **Restore before, from the vault, into the run**: before the deploy command the run takes the
  hold of all its states at once (waiting, with the output naming the holder, while another run
  holds one; bounded by the job timeout, ended by a cancel), then unpacks the vault's newest
  version of each into `<run>/work/states/<NAME>` and exports `YULIUS_STATE_<NAME>` with that
  absolute path. Both stages read the vault, never a task's copy; the folder is never the task's
  restore path and never inside `HOME`, so an agent-chosen path cannot shadow the run's dotfiles.
- **Save after, failed runs included**: once the deploy and the verify ended, however (passed,
  failed, timed out, cancelled), each folder is packed and stored as the next version with a
  compare-and-set on the version the run restored. An unchanged folder (same paths, modes and
  bytes) is not saved. A failed save ends the run as `error`, naming the state. Then the holds go.
- **One writer at a time**: the hold is in memory per sandbox and state. A task's
  `secret_state_save` and a person's import take it without waiting and are refused (409) while
  a run holds it. A restart ends a running deploy as `error` (0109) and its hold with the process.
- **Tasks write only a current copy**: a CD save makes a task's restored copy stale; its save is
  refused until `secret_state_restore` replaces the folder with the newest version, and
  `secrets_list` marks it `stale` and `held_by` a running deploy. A laptop import over a state a
  CD run wrote last is refused like one over a restored copy, unless `replace`.
- **Audit**: each restore and save by a run is `vault.state.restore` / `vault.state.save` with
  the actor `cd:<environment>` and the run in the detail; the state's `updated_by` is
  `cd:<environment>`.
- **Variables**: CD runs get the project's variables first in their environment (Yulius's own
  variables and the secrets win): a dev-stage run all of them, a production-stage run only those
  its verified table names (`variables = ["NAME"]`, at most 32); a named one the project lacks
  refuses the deploy. The others are listed as left out in the output.
- **Docker and SSH**: dev runs keep the sandbox's engine. Production runs have no engine yet; one
  agents cannot reach is task #265. CD runs reach any Internet host over TCP (the sandbox's
  network, host loopback off), port 22 included; Yulius keeps no host list, and a project names
  its hosts and pins their keys in its own state or repository.

## Alternatives

- **CD runs use the task's restored copy in the sandbox home**: production would run on files an
  agent wrote, and a task and a run would write one folder at once.
- **Only CD runs write a state a table names**: forbids the operator's manual fixes through a
  task; restoring fresh before writing gives the same single writer.
- **A persisted lock row**: outlives the process that held it and needs a cleanup; a restart
  already ends the run.
- **Restore into the task's restore path inside `HOME`**: an agent chooses that path; inside a
  production run `~/.ssh` would become the agent's.
- **Every variable in production**: a person with write access could give an administrator's
  deploy `PYTHONPATH`, `SHELLOPTS`/`PS4` or `OPENSSL_CONF` and run code with production's secrets.
- **A deny list of code-loading names in production** (the first version): every tool has its own
  hooks (`OPENSSL_CONF`, `CLASSPATH`, `AWS_CONFIG_FILE`, `TF_CLI_CONFIG_FILE`, …), so a list is
  never complete (review of SPEC-278, M3). The table is reviewed on a protected branch.
- **Filter SSH hosts per project**: needs a per-run network namespace and filter; not asked for,
  and the tool pins host keys.

## Consequences

- A deploy tool keeps its state between CD runs, and failed upgrades and locks are kept too.
- A task that changed its copy without saving loses those changes when it restores after a CD
  save; the instructions say to save after each change.
- A restart during a run loses what that run wrote to its states (the run ends as `error`).
- Production restores the newest version whoever wrote it: a dev run (running the integration
  branch's code) or a task's save included. The vault's state is the one exception to "nothing an
  agent wrote runs with production's secrets", as data: a deploy tool must never run code from it.
- A run that waited for the hold can last up to twice the job timeout (the wait has its own bound).
- After a timeout or a cancel the folder is saved once the process host ended the run; contained
  runs have their own PID namespace, so none of the run's own processes writes after that (a
  container a dev run started on the sandbox's engine is outside it).
- A task restoring the state three times while a run saves it makes that save fail (the run is an
  `error`, its writes are lost); the save retries the compare-and-set twice.
- Production deploys that need Docker wait for task #265.
