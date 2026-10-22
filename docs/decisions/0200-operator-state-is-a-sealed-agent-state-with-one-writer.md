# 0200. Operator state is a sealed agent state with one writer

## Status

Accepted. Specified in [SPEC-271](../../.ai/specs/SPEC-271-laptop-onboarding-move-a-project-s-opera.md);
task #257. Extends [0194](0194-an-operate-token-confirms-for-its-person-s-laptop-onboarding.md)
(laptop onboarding) and [0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md)
(host writes into agent-owned folders go through handles).

## Context

A project that deploys from the laptop keeps live operator state there: a folder holding a SQLite
inventory (deployments, pinned host keys, an upgrade journal, rollout locks), rollout folders and
configs. Laptop onboarding moved the keys as agent secrets, but an agent secret is one value of at
most 16 KiB (`--from-file`: 64 KiB of one-line text or a key), and nothing places files into a
sandbox's home. The state changes on every rollout, so after the move exactly one copy may be
written to, or the two copies fork and a rollout runs against a stale inventory.

## Decision

- **An agent state** is a project's named folder, stored in the vault database as one gzip tar
  (≤ 64 MiB packed, ≤ 512 MiB and 20 000 entries unpacked, only files and folders, relative paths)
  sealed as an envelope bound to its row and version, like an agent secret. Names are agent secret
  names, unique per project. Every import, replace, restore, save and download is audited
  (`vault.state.*`) as the person or the agent acting for them, without content.
- **Sent from the laptop** with `yulius secrets import --to agent --state NAME=DIR` (packed by the
  CLI, or an existing `.tar.gz`), under ADR-0194's rules: the typed `yes`, or `--yes` with the
  operate token only; names, file counts and sizes are shown, never content.
- **Restored by a task** with `secret_state_restore {name, path}` into a `~/…` folder of the
  sandbox home (it outlives tasks), through folder handles: unpacked into a fresh sibling folder,
  then renamed into place; permission bits kept, setuid, setgid and sticky dropped.
- **One writer.** Before a restore the laptop is the writer and a new import replaces the stored
  copy. Once restored, the sandbox's copy is the writer: a new import is refused (409, naming the
  copy) unless `replace=true` (`--replace-state`), which makes the sandbox's copy stale until a
  restore replaces it. `secret_state_save` stores the restored folder as the next version with a
  compare-and-set on the version, so it never overwrites a replace it has not restored. The CLI
  says after each send that the laptop folder stops being the writer once restored.
- **Back to the laptop** with `yulius secrets state-get NAME --out FILE` (the newest saved
  version), which leaves the sandbox's copy the writer.

## Alternatives

- **Many agent secrets** holding pieces of the state: 16 KiB values, text only, no file modes, no
  folder layout; rejected.
- **Another repository** carrying the state: the inventory and pinned host keys would sit in Git
  history outside the vault and its audit; rejected.
- **Sealed files beside the database**: avoids large rows, but two stores must then stay
  consistent through crashes; a 64 MiB row is well within SQLite's limits and is backed up with
  the vault. Rejected for now.
- **Telling the laptop** it is no longer the writer (a marker in its folder): the laptop cannot be
  reached after the CLI exits, and writing into the person's operator folder could break their
  tools. The server-side refusal is what holds; the CLI's message is the notice.
- **Two-way sync**: two writers by design; rejected.

## Consequences

- The keys stay agent secrets; configs that name key files by a laptop path are adapted by the
  onboarding task (keys go to files through `secret_copy_to_env`).
- The restored files are plain files in the sandbox home: an agent can read them, so the tool
  descriptions and briefing tell agents never to print them, and the transcript redaction covers
  key material and agent secret values as for any file.
- The server holds at most two states in memory at once (64 MiB each, packed); more concurrent
  operations wait as `Capacity`.
- Any task of the project may restore and save its states, as any task uses the project's agent
  secrets; downloading one to a computer needs a person with write access and a session or an
  operate token (a read-only token is refused).
- The single-writer rule is a compare-and-set on the version and the restored copy as read, so an
  import racing a restore, or two restores, never both win; a copy placed by the losing restore is
  removed.
- A restore interrupted by a server stop leaves a `.yulius-state-*` folder beside its target in
  the home; nothing sweeps it yet.
- A state is removed with its project; there is no delete of one state yet.
