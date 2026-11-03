# 0257. The config scrub and new names never go through a checked path

## Status

Accepted (2026-11-03, task #385, [SPEC-402](../../.ai/specs/SPEC-402-vidoc-triage-uploads-files-and-git-809-8.md)).
Applies [0174](0174-host-access-to-agent-owned-folders-goes-through-handles.md) to the clone
configuration scrub of SPEC-176 and to upload placement.

## Context

Every agent run of a sandbox mounts the sandbox directory read-write, the clones' `.git`
included. Three Vidoc findings named places where the server checked an entry there and then used
it by name:

- `scrub_transport_config` checked that `.git/config` and `config.worktree` were not links, then
  ran `git config --unset-all` by path. Git writes through `<file>.lock` and a rename after
  resolving a link at `<file>`, so a link swapped in after the check sent the write to the file it
  points at (#810). A test whose `git` swaps the file at its first `config` call reproduced it.
- `link_new_in`, where `linkat` fails (a file system without hard links), checked with `fstatat`
  and then renamed, replacing a name another writer took in between (#808).
- Folders were made with `mkdirat(…, 0o777)`, so their mode was the process umask's (#800).

## Decision

- **The scrub works through handles and Git's lock.** The repository folder and `.git` are opened
  with `O_DIRECTORY | O_NOFOLLOW`. Both files are checked first (missing or regular; a link or a
  FIFO refuses before anything is written). Each file is then scrubbed under `<file>.lock`,
  created `O_EXCL | O_NOFOLLOW` in the `.git` handle: read through an `O_NOFOLLOW | O_NONBLOCK`
  handle, copied to a 0700 folder of the service user, listed and unset there by Git
  (`--file`, so includes are not followed, as before), and written back into the lock with the
  original mode and renamed over the name through the handle. A file with nothing to remove is not
  rewritten. A link swapped in meanwhile is replaced; the file it points at is never read or
  written. Off Unix the scrub refuses (no contained agents run there).
- **New names are placed atomically or not at all.** The fallback after a failed `linkat` is
  `renameat2(RENAME_NOREPLACE)` (Linux, glibc); where it is unavailable the placement fails with the
  link's error.
- **Created folders ask for 0o755.** A umask can only narrow it. Under systemd's default umask
  (0022) the result is what it was.

## Alternatives

- **Check again after Git wrote**: the write already went through the link.
- **Parse and rewrite Git's configuration format in Rust**: no Git between the check and the
  write, but a second parser of a format Git owns (sections, subsections, quoting, multi-values);
  the copy keeps Git deciding what a key is and changes only where it writes.
- **Keep the check-then-rename fallback**: it is the race; failing an upload on a file system
  without hard links or `RENAME_NOREPLACE` is better than replacing an entry nobody asked to replace.
- **0o700 folders**: tighter, but a change of what production creates today (0755), which could
  hide folders from whoever reads the sandbox tree as another uid.

## Consequences

- The scrub leaves `.git/config` owned by the service user, as Git's own rewrite did.
- A Git writer holding `config.lock` makes the scrub fail rather than wait; its callers already
  handle a failed scrub (the playground move stops, a change clone logs it).
- An upload or a trash restore on a file system with neither hard links nor `RENAME_NOREPLACE`
  fails; none is used for the data directory.
- Other folders the server makes with `std::fs::create_dir_all`, and the harness's skill folders
  (`yulius-harness` `skills.rs`, `mkdirat(…, 0o777)`), still follow the process umask.
- Only a file that exists at the check is locked and scrubbed, so a stale `config.worktree.lock`
  does not refuse a clone without `config.worktree`.
