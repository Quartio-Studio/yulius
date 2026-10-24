# 0213. A snapshot import checks out in a server-private folder

## Status

Accepted. Specified in [SPEC-296](../../.ai/specs/SPEC-296-snapshot-import-reserves-its-worktree-pa.md);
task #282, issue #824. Applies [ADR-0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md)
to a folder Git, not Yulius, writes into.

## Context

- Applying an import's snapshot (uncommitted changes) checks the client's head out in a temporary
  worktree, extracts the archive into it, commits and removes it. The worktree was
  `<sandbox>/worktrees/import-<job>[-n]`, its name picked with an existence check.
- Contained runs mount the sandbox directory read-write (ADR-0174), so an agent can plant a link
  at that name. `git worktree add` accepts an existing empty folder, checks it through the link,
  and checks the head out wherever the link points; the extraction then followed the same link.
- ADR-0174's rule (anchor, then handles) covers what Yulius writes itself. Git is a separate
  process that takes a path and resolves it again, link by link: whatever folder Yulius reserves
  in the sandbox, the agent can rename it away and plant a link before Git runs.

## Decision

- The snapshot's worktree is made in `<data>/import-worktrees/<sandbox>/`, a folder of the server
  (0700) outside every sandbox directory, like `project_change`'s `<data>/changes` (SPEC-247). Its
  name is reserved with a create-new `mkdir`: `import-<job>`, then `import-<job>-2`, … up to the
  snapshot bound; Git fills the empty folder.
- `extract_snapshot` refuses a root that is a link or not a folder (`symlink_metadata`) instead of
  canonicalizing it.
- Yulius never touches `<sandbox>/worktrees/import-…` any more; the hygiene round still leaves
  such checkouts of earlier imports to the import.

## Alternatives

- **Reserve the name in `worktrees/` through a handle (`mkdirat`, `EEXIST` loop) and re-check
  after Git with an `O_NOFOLLOW` open** (the issue's suggestion). It closes the existence-check
  race but not Git's: a swap between the reservation and `git worktree add` still makes Git write
  the checkout outside the sandbox before the re-check fails the import.
- **No worktree: build the commit with plumbing** (a temporary index, `hash-object`, `write-tree`,
  `commit-tree`). Nothing checked out at all, but a rewrite of the snapshot application, its
  link and deletion rules and its tests, for the same result.
- **Hand Git a handle path (`/proc/self/fd/N`).** Git records the worktree path it was given, and
  macOS has no such path (ADR-0169).

## Consequences

- An agent cannot steer the snapshot checkout or swap folders inside it while it is extracted;
  a link it plants at the old name is simply not used.
- `git worktree list` in the sandbox shows the server path while an import runs; the agent cannot
  open it, and pruning it only fails that import.
- A worktree left by a failed import stays in the server's folder, as it stayed in `worktrees/`
  before; a retry takes the next name.
- Task worktrees in `<sandbox>/worktrees/` are still created by Git by path, while other tasks'
  agents of the sandbox may run; they must stay where the agents work, so this decision does not
  apply to them (noted on #824).
