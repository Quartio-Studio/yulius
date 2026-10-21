# 0193. The Claude config dir is walked from above the home, and the main repository is read from the git layout

## Status

Accepted. Specified in [SPEC-253](../../.ai/specs/SPEC-253-claude-config-dir-links-and-commondir-ca.md);
task #239, fixes the rest of #634. Applies
[0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md) to the launch preparation
of SPEC-024, SPEC-106 and SPEC-210.

## Context

- Before every task run and Superagent launch the host prepares `<home>/.claude`
  (`ensure_claude_config`): it merges `.claude.json`, defaults `settings.json` and closes the diff
  plugin's pane in `plugins/store`. A task's home is `<sandbox>/home`, inside the read-write sandbox
  mount; a person's Superagent home is `<users>/<id>/home`, mounted as their `/home/<login>`.
- PR #641 made the reads open the last component without links or blocking, but the folders were
  still created with a recursive `DirBuilder` and written by path, so a linked `home`, `.claude`,
  `plugins` or `store` steered the reads and renames into any folder the service user can write.
- To trust a linked worktree's main repository (SPEC-106) the host resolved the agent-written
  `gitdir:` and `commondir` paths with `canonicalize`: an oracle for which host paths exist and
  where host links lead, written back into the agent's `.claude.json`.

## Decision

- **Anchor above the home.** `ensure_claude_config(<anchor>/<home>/<config>, …)` opens `<anchor>`
  by path (the sandbox directory, or the person's user folder: neither is inside a mount the agent
  writes) and the home and config dir with `O_DIRECTORY | O_NOFOLLOW`, making them 0700 when
  missing. A link or a file there refuses the launch, naming it. `plugins` and `store` are opened
  the same way through the config dir's handle, never created; anything but a folder leaves the
  pane alone.
- **Files through the handle.** Reads use `openat` with `O_NOFOLLOW | O_NONBLOCK` and only a
  regular file; replacements create a random `O_EXCL | O_NOFOLLOW` file (0600), sync it,
  `renameat` it within the handle and sync the folder.
- **Main repository from the layout, lexically.** The admin dir is the `gitdir:` path (against the
  worktree's real path) with `.` and `..` removed; it must be `<common>/worktrees/<name>`, and its
  `commondir` must lead lexically back to `<common>`. Nothing the agent names is resolved on the
  host. The worktree itself, the host's own path, keeps being resolved to its real path, which is
  how Claude Code keys its trust.

## Alternatives

- **Pass the anchor explicitly.** Both callers would derive it from the same home path; the
  derivation inside the function keeps one rule and one place.
- **Replace a linked config dir** (remove the link, create a folder). It deletes an entry the agent
  made, silently; a refusal names it, as ADR-0174 does elsewhere.
- **Walk `gitdir` with `O_NOFOLLOW` from `/`.** Refuses host links above the data directory (macOS
  `/var` → `/private/var`), which work today, and still touches paths the agent names.
- **Ask `git rev-parse --git-common-dir`.** A process per launch running in the agent's repository
  configuration, outside the containment.

## Consequences

- An agent that links its home, its config dir or a folder on the way refuses its own next launch
  with `… on the way to the Claude config dir is a link or not a folder: Yulius follows no links`.
- A linked worktree whose git files do not read like Git's own layout trusts the worktree alone;
  Claude Code may then ask for the main repository's trust as it would without Yulius.
- The two Vidoc ignores that pointed at #634 are gone; a new finding there is triaged afresh.
