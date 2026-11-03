# 0255. Git checks a worktree and the server removes it through handles

## Status

Accepted (2026-11-03, task #378, [SPEC-401](../../.ai/specs/SPEC-401-vidoc-triage-ci-disk-and-cd-848-844-795.md)).
Applies [0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md) to the removal of
task worktrees, which SPEC-263 left to Git.

## Context

Task worktrees live in `<sandbox>/worktrees/`. Every agent run and shell of the sandbox mounts that
folder read-write. The disk upkeep, the hygiene cleanup and snapshot imports removed a clean
worktree with `git worktree remove`, and task deletion with `git worktree remove --force --force`.
Yulius checked the path first (`canonicalize`, inside the owned root, registered, clean). Git then
resolved it again, and it deletes the folder through `opendir` on the path. So a worktree that a
contained agent swapped for a link after Yulius's check had the folder behind the link deleted by
the host: another sandbox's, or one of Yulius's own (#844, Vidoc). Task deletion does not check
`git status`, so there one swap is enough; the clean removal needs the swap during Git's own
checks. `git worktree prune` cannot replace Git's removal of the record: it would also drop the
records of worktrees that agents keep in places the server does not see (SPEC-215).

## Decision

- On Linux, Git only checks: inside the owned root, registered, not the main worktree, not locked,
  clean (`git status`), and no `modules/` in its record. A discard skips the cleanliness check,
  as before. Git also names the worktree's record (`rev-parse --absolute-git-dir`), which must lie
  in the repository's `worktrees/`.
- The server then removes the worktree folder below the parent of the owned root (the sandbox
  directory). After that, while no other worktree registration changes in this process, it
  removes the record below the parent of the clone. Both go through `leftovers::remove_below`:
  `O_NOFOLLOW` from the trusted folder down, then handle by handle. A worktree swapped for a link
  has only the link removed.
- `yulius-git` offers `worktree_remove_if_safe_below` and `worktree_discard_below`, which take the
  removal as a function (`RemoveBelow`). The Git adapter passes
  `leftovers::remove_worktree_part`, which holds the `WorktreeRemove` race point for tests.
- Off Linux (development hosts, whose runs are not contained) Git removes the worktree as before.

## Alternatives

- **Re-check right before Git (SPEC-263's DUL-003).** This narrows the window but cannot close it:
  Git resolves the path itself afterwards. Kept, as the first gate.
- **Remove the folder, then let Git drop the record (`git worktree remove` of a missing folder).**
  Git checks again whether the path exists and follows a link planted meanwhile. Rejected.
- **`git worktree prune` after the folder is gone.** This drops other worktrees' records too
  (SPEC-215). Rejected; a discard still prunes, as it did.
- **Move worktrees out of the sandbox directory (as ADR-0213 did for snapshots).** Agents work in
  their worktrees, so they must stay in the sandbox. Rejected.

## Consequences

- The server, not Git, deletes a worktree's files. Git's other side effects of a removal (an
  empty `worktrees/` folder removed) no longer happen; Git ignores an empty `worktrees/` folder.
- A worktree whose record holds submodules is not removed by the upkeep or the cleanup, as Git
  refused it before. A discard removes it anyway.
- The same handle walk serves caches, leftovers and worktrees, so there is one removal to keep
  right.
