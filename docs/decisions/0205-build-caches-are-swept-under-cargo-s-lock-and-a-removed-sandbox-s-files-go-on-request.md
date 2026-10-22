# 0205. Build caches are swept under Cargo's lock, and a removed sandbox's files go on request

## Status

Accepted. Specified in [SPEC-276](../../.ai/specs/SPEC-276-reclaim-disk-automatically-ask-on-sandbo.md);
task #261. Extends [0170](0170-leftovers-are-deleted-by-kind-rules-checked-again-at-deletion.md) (leftovers) and
SPEC-008's disk upkeep; generalises SPEC-171's playground purge; amends SPEC-001 PRJ-009.

## Context

On 2026-10-22 the production disk reached 96 % and shrank about 1 GB a minute. What filled it was
not a leftover: Cargo build directories of working or idle worktrees (93 GB of incremental state)
and of CI checkouts (54 GB, 41 GB of it unread for a day), which only quota pressure ever pruned,
and the files of removed sandboxes (35 GB), which PRJ-009 kept until a person ran
`yulius leftovers delete`. Nothing read the host's free space and nothing warned anyone.

## Decision

- **Sweep.** Every Cargo build directory of a live sandbox's worktrees and CI checkouts is swept
  hourly, and on every disk round while the disk is low. A sweep takes every profile's
  `.cargo-lock` with a non-blocking exclusive `flock`; one held skips the directory. Under the
  locks it deletes whole units: an artifact unit is a profile's `.fingerprint`, `deps` and `build`
  entries of one 16-hex hash, an incremental unit one `incremental/<crate>-<hash>` directory
  (never one holding a `*-working` session). A unit unused (access and modification time) for
  48 h goes; then a directory above its cap (5 % of the data disk, at least 8 GB) loses its least
  recently used units, never one used within the hour, down to three quarters of the cap.
- **Pressure.** The data disk is read every minute: from 85 % used it is low (6 h, half the cap,
  every round, the safe leftovers round at once at most every 10 minutes), from 95 % critical
  (1 h, a quarter of the cap, no recent floor, and idle tasks' and CI checkouts' caches pruned
  until the disk is back at 85 %). A rise is audited `disk.pressure`; `GET /api/disk` and the
  `disk.updated` event drive a warning pill on every page.
- **Removed sandboxes.** Removing a sandbox asks, with no default, whether to delete its files
  permanently or keep them. Delete requests a purge (any sandbox, not only playgrounds) that
  deletes every removed-sandbox path of SPEC-215 through the leftovers' guards and mapped removal
  as soon as nothing of the sandbox runs and its holder stopped, without the 10-minute wait: tried
  at once, every 15 s for 10 minutes, then every disk round. Keep is PRJ-009's behaviour, and the
  Leftovers card lists kept sandboxes with their size and a Delete files action.

## Alternatives

- **`cargo-sweep` or `cargo clean` as subprocesses.** Neither knows whether another Cargo run
  uses the directory, `cargo clean` drops the current build too, and a subprocess per worktree
  adds a dependency to every sandbox. The unit rule is cargo-sweep's, done in-process.
- **Detect builds by scanning `/proc`.** Needs permissions on the sandbox's processes, misses a
  build starting between the scan and the deletion, and cannot stop one from starting.
- **Delete single files by age.** A missing `out/` of a build script whose fingerprint stays fresh
  breaks the dependent crate's next compile; units keep Cargo's own invariants.
- **A fixed cap (e.g. 20 GB) or a cap per sandbox.** Wrong for small and large disks alike, and a
  sandbox-wide cap lets one worktree starve the others.
- **Delete removed sandboxes' files by default, or after a period.** Transcripts and sessions are
  history people may want; the person removing a sandbox decides, once.

## Consequences

- Build directories of active worktrees stay near their cap; a task continued after two days
  rebuilds what it had not used since. A Cargo run that starts during a sweep waits for it
  (seconds) on Cargo's own lock.
- Under a critical disk an idle task loses its caches and rebuilds when continued.
- Only Cargo build directories are swept by use; other tools' caches keep SPEC-008's rules.
- A purge cannot be undone; task records and the audit trail stay, so analytics keep working.
