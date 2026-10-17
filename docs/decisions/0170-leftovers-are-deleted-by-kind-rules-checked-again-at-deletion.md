# 0170. Leftovers are deleted by kind rules checked again at deletion

## Status

Accepted. Specified in [SPEC-215](../../.ai/specs/SPEC-215-leftovers-are-found-and-cleaned-accurate.md).
Amends PRJ-009 (SPEC-001: what a removed sandbox keeps), SPEC-008's upkeep of finished tasks and
SPEC-171's playground purge (the system layer, runtime state and CD directory now go too).

## Context

The VM's disk reached 81 % (312 of 386 GiB). A good part of that was files Yulius never deleted:

- a removed sandbox's worktrees and CI checkouts, because the purge loop read only live sandboxes
  (#625);
- its system layer `sysroots/<sid>` with its Docker images, which no code removed at all (#582);
- its holder state, CD sources and browser profile;
- the 11–23 GB `target/` of each task that finished ("done") but was not closed.

Each of these had its own partial cleanup, or none, and no person could see what was there
without SSH.

## Decision

- **An inventory with a rule per kind.** Every path Yulius writes under its data directory has an
  owner record and a rule for when it becomes garbage (the table in SPEC-215). The `leftovers`
  module scans by these rules. Each item has a kind, a path, a size, an owner, a reason and whether
  it is **safe automatically**. Only a path no record owns, or one its owner's state says is
  disposable, is ever an item.
- **Checked again at deletion.** Right before deleting an item, its rule is evaluated again on
  fresh state: the store, live shells, the browser, CI lanes (claimed for the deletion), live CD
  runs, the holder and the processes in its namespaces. If the rule no longer holds, the item is
  kept and reported as kept. A missing path counts as deleted, so a deletion is idempotent and an
  interrupted round resumes on the next one. Each deletion is audited as `leftovers.delete` (path,
  size, reason, actor) and each failure as `leftovers.failed`.
- **What a removed sandbox loses by itself**, 10 minutes after the removal once nothing of it
  runs: its worktrees, CI checkouts, browser profile and display, CD directory and agent run
  directories. Once its holder has stopped, it also loses its system layer and runtime state,
  through the isolation's mapped removal for files owned by the sandbox's ids. Its clone, home
  (sessions), attachments, transcripts, playground origin and export stay (PRJ-009). These are
  listed as one manual item, "everything of the removed sandbox", which only the superadmin
  deletes, after a confirmation naming the size. The records stay.
- **Finished tasks.** A running task whose activity is **done** loses its rebuildable caches
  (`target`, `node_modules`, declared caches) after 30 quiet minutes. A waiting task keeps the 4 h
  rule, and ended tasks keep their 10 minute grace. The worktree, uncommitted work and the branch
  always stay. Continuing the task rebuilds the caches.
- **Git metadata.** A clone's `.git/worktrees/<name>` goes when its worktree lay inside the
  sandbox directory, is gone, and the record is not locked. Only that record goes, never through
  `git worktree prune`, which would also drop the records of worktrees an agent keeps in its own
  `/tmp` (the host does not see them).
- **No link is ever followed.** A sandbox's agents and shells can write anywhere in their sandbox
  directory, so they could swap a directory for a link. An item reached through a link is never
  listed. The removal opens every directory from the data directory down with `O_NOFOLLOW` and
  removes through the opened directory (`/proc/self/fd/<fd>/<name>`), so a link that appears
  after the last check fails the removal instead of steering it. Below that, std's
  `remove_dir_all` removes links as links, and a read-only tree is made writable through
  directory handles (`O_NOFOLLOW`, `fchmod` to a fixed 0o700), never by path. Only the isolation's
  mapped removal goes by path: a removed sandbox's system trees, and a stale smoke test's or an
  unowned sandbox's trees under `sandboxes/`. No run of those is left.
- **Run directories.** `runs/<name>` stays while an attempt record owns it (an ended task's last
  run holds its terminal's final screen), a run is live, or a CI job or deploy of it runs. One no
  record owns goes a day after it was last touched.
- **Manual only**: worktrees no task owns (they may hold uncommitted work), directories no
  sandbox record owns (except a smoke test's whose process is gone), and everything of a removed
  sandbox. The host's `/var/lib/docker` and `/var/lib/containerd` are reported, never deleted:
  they are the host's.
- **When.** The automatic round runs at every server start, which is when an update lands
  (including a hand-copied binary that leaves no record), and then every hour. The quota-driven
  upkeep goes on every 5 minutes, as before.
- **Where people see it.** Settings → Instance → Leftovers (superadmin), from
  `GET/POST /api/admin/leftovers*`. Deletions run in the background with progress. `yulius
  leftovers list|delete` runs over the loopback listener with the key the server writes to
  `<data>/leftovers-cli.key` (0600: root and the service user only), so the CLI applies the
  server's rules and live state.

## Alternatives

- **Fix only the purge loop (#625) and add `sysroots` to it.** The leftovers would stay invisible,
  partly-cleaned states would fail, and done tasks would still hold 70 GB.
- **Delete a removed sandbox's directory whole, automatically.** Breaks PRJ-009: the person
  expects transcripts and the clone to survive a removal.
- **Keep a removed sandbox's parts for a day** (the old purge). Nothing can use them once its runs
  have ended, and re-adding the repository makes a new sandbox. A day only delayed 100+ GB.
- **Run only on the first start of a new version.** That needs a stored "last cleaned build", and
  a plain restart is no less safe. A restart costs one scan.
- **A CLI that works on the files itself.** It cannot see the server's live state (open shells, CI
  lanes, leases), so it could delete something in use.
- **Treat a done task as closed and remove its worktree.** Its uncommitted work and local state
  would go. Caches alone are where the bytes are.

## Consequences

- A removed sandbox frees nearly all its space about 10 minutes after the removal. What PRJ-009
  keeps costs only its clone, home and transcripts.
- Done tasks keep about 50–100 MB each instead of 10–20 GB. Continuing one costs a rebuild.
- An item that turned live between the list and the click is kept, never deleted. The person sees
  "kept" and why.
- A part of the data directory that is a link (moved to another disk) hides its leftovers: the
  server logs it once per scan. Bind-mount such a directory instead.
- The isolation port gains `holder_live`, `remove_owned_tree` and `unused_bases`. Adapters without
  isolation keep the defaults: no holder, plain removal, no bases.
