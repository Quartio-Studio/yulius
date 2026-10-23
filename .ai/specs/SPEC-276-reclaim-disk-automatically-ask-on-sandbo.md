---
yulius-task: b702149f-23d1-482e-bb86-91397a049b4d
status: done
---

# SPEC-276: Yulius reclaims disk by itself, and removing a sandbox asks about its files

Status: done
Date: 2026-10-22
Owner: task #261
Scope authorization: task #261 (requested by MrFishPL): "Yulius must reclaim unnecessary disk
space by itself, and deleting a sandbox must ask the user whether to delete its files
permanently." Extends SPEC-215 (leftovers) and SPEC-008 (disk upkeep), generalises SPEC-171's
playground purge to every sandbox and amends PRJ-009 (SPEC-001). Decision: ADR-0205.

## Goal

Nothing rebuildable piles up until a person notices a full disk. Cargo build directories of
worktrees and CI checkouts are swept by last use and kept under a size cap, never while a build
holds them. When the server's disk runs low, Yulius sweeps harder, prunes idle caches and warns
everyone in the UI before the disk fills. When a person removes a sandbox, they choose: delete its
files permanently (gone as soon as nothing of it runs, no CLI step) or keep them (today's
behaviour), and kept files are listed with their size in Settings → Instance, where they can be
deleted later.

## Context

Incident of 2026-10-22 on the production instance: the root disk reached 96 % (16 GB free of
387 GB) and shrank about 1 GB a minute while agents built; it was freed by hand down to 48 %.

- **CI job build directories**: `ci/jobs/quality/target` 31 GB and `quality@1/target` 23 GB, of
  which about 41 GB nobody had read for over a day. Cargo never removes artifacts of old hashes.
  `disk_scan` (`crates/yulius-app/src/disk.rs`) prunes a CI checkout's cache only under the
  sandbox's quota pressure or above a quarter of the quota (`job_cache_cap`); this instance's
  sandboxes have no quota pressure, so nothing ran.
- **Worktree incremental caches**: about 93 GB in `target/debug/incremental` across 8 live
  worktrees (17–21 GB each). A task that works or idles keeps its caches whole (`owner_of`); only
  ended or done-and-quiet tasks lose them (SPEC-215 LFT-005). Measured on a sibling worktree of
  this sandbox: `target/` 22 GB, of it `debug/incremental` 15 GB in 208 crate directories (16 for
  `yulius` alone), `debug/deps` 4.5 GB, the macOS cross build 2.7 GB.
- **Removed sandboxes** kept 35 GB (PRJ-009: "kept unless a person deletes them"). Removing the
  studio sandbox in the UI freed nothing; its files could only go with `yulius leftovers delete`.
  Only playgrounds could ask for their code to be purged (SPEC-171).
- **Pressure is per sandbox quota only.** `bytes_to_free` reads the sandbox's quota; nothing reads
  the host's free space, and nothing warns anyone.
- **`yulius leftovers list` said "0 MB safe"** at 96 %: the automatic round (LFT-009: at start and
  hourly) did run, but what filled the disk was not a leftover by SPEC-215's rules. Its result was
  visible nowhere, so nobody could tell that it ran.

Cargo facts the sweep relies on (cargo 1.99, checked with a probe crate whose `build.rs` sleeps):
while Cargo compiles it holds exclusive `flock`s on `<target>/<profile>/.cargo-lock`,
`.cargo-build-lock` and `.cargo-artifact-lock` (also in a `--target` triple's
`<target>/<triple>/<profile>/`), and releases them when it is done compiling: test binaries,
`cargo test --doc`'s rustdoc runs and `cargo nextest` read `deps/` after that, without a lock
(review round 1). Only rustc reads `incremental/`, and only under the lock. A unit whose fingerprint (`.fingerprint/<name>-<hash>`)
is missing is rebuilt, and so is one whose output file is missing; rustc compiles incremental
sessions in `incremental/<crate>-<hash>/s-…-working` and renames them when done. The data disk is
mounted `relatime`: a read updates the access time when it is older than the modification time or
more than 24 h old, so a file whose access and modification times are both older than 48 h has not
been read for at least 24 h.

## Requirements

### Build caches

- **RDA-001** Every Cargo build directory of a live sandbox, `worktrees/<name>/target` and
  `ci/jobs/<job>/target` (a worktree or checkout with `Cargo.toml`, reached from the sandbox
  directory with no link on the way), is swept: hourly, and on every disk round while the disk is
  low (RDA-006).
- **RDA-002** A sweep first takes, without waiting, an exclusive lock on every Cargo lock file
  (`.cargo-lock`, `.cargo-*-lock`) of the build directory's profiles (`<target>/<profile>` and
  `<target>/<triple>/<profile>`, the directories holding `.fingerprint`). One that is held (Cargo
  compiles) skips the whole directory this round. Holding them, no Cargo run can start in it until
  the sweep ends (it waits on the lock). A lock file that is a link or not a plain file (a FIFO) is
  opened without blocking and counts as held. Artifact units are swept only where nothing can run
  tests: a CI checkout while the sweep holds the sandbox's CI claim (a running CI run keeps it
  whole), a task's worktree while the task neither works nor runs background work (probed) and no
  Shell of the sandbox lives. Other worktrees (working tasks, worktrees no task owns, any worktree
  while a person's Shell is open) lose incremental units only.
- **RDA-003** The sweep deletes whole units, never parts of one: an artifact unit is every entry of
  `.fingerprint`, `build` and `deps` of one profile that carries the same 16-hex hash (its
  fingerprint goes with its outputs, so Cargo rebuilds it); an incremental unit is one
  `incremental/<crate>-<hash>` directory. A unit's last use is the newest access or modification
  time of any file in it. An incremental directory holding a `*-working` session is never
  touched. Files outside units (binaries at the profile root, `doc`, `nextest`, `tmp`) are left.
- **RDA-004** A unit unused for 48 h is deleted (relatime: unread for at least a day). On a
  `noatime` file system artifact units are not swept by age.
- **RDA-005** After that, a build directory whose units hold more than its cap loses its least
  recently used incremental units until they hold three quarters of the cap, never one used within
  the last hour (files outside units count neither; artifact units are never evicted for the cap,
  their access times can lag a day). A sweep cut short by its entry bound still takes the whole
  incremental units it read. The cap
  is 5 % of the data disk, at least 8 GB (19 GB on a 387 GB disk). Each sweep that deletes
  something is audited (`disk.sweep`: what, how much, why).

### Disk pressure

- **RDA-006** The upkeep reads the data disk's usage every minute. From 85 % used the disk is
  **low**: sweeps run every round with units unused for 6 h deleted and half the cap. From 95 % it
  is **critical**: units unused for an hour go, a quarter of the cap, no unit is spared for being
  recent, and the idle tasks' and idle CI checkouts' caches are pruned (largest first, as quota
  pressure does) until the disk is back at 85 %. A rise of the level is audited (`disk.pressure`)
  and starts a round at once; the safe leftovers round (SPEC-215) also runs at once, at most every
  10 minutes while the disk is low.
- **RDA-007** `GET /api/disk` (anyone signed in) returns the level (`ok`, `low`, `critical`), used
  and total bytes; an instance-wide `disk.updated` event says when the level changes. While the
  level is `low` or `critical`, every page shows a warning pill in the tab row ("Disk 91 % full")
  whose details say what Yulius is doing about it, with a link to Settings → Instance →
  Leftovers for the superadmin.

### Tasks and the scheduled leftovers round

- **RDA-008** A task's build output goes when it ends: closed, stopped or failed after the
  10-minute grace and done after 30 quiet minutes (SPEC-215 LFT-005, unchanged); **archived** with
  no grace, in a round started at once by the archiving; deleted together with its worktree
  (SPEC-001 TSK-009, unchanged).
- **RDA-009** The safe leftovers (SPEC-215) keep being deleted at start and hourly with nobody
  running the CLI. The leftovers view and `yulius leftovers list` show the last automatic round:
  when it ended, what it deleted and freed, and what failed.

### Removing a sandbox

- **RDA-010** Removing a sandbox (UI) asks what happens to its files on this server, with no
  default: **Delete permanently** or **Keep**. The remove button stays disabled until one is
  chosen. The dialog lists what the files are (clone, sessions and transcripts, export, CI
  directories, browser profile, system layer, runs) and that uncommitted or unpushed work in its
  worktrees is lost with them; the GitHub repository, task records and audit trail stay either
  way.
- **RDA-011** Delete permanently (`POST /api/sandboxes/{sid}/purge`, now for every sandbox, not
  only playgrounds): the sandbox is removed as before and every file of it is deleted as soon as
  nothing of it runs and its holder stopped: tried at once, then every 15 s for 10 minutes, then on
  every disk round. No 10-minute wait and no CLI step. The files are SPEC-215's removed-sandbox
  paths (worktrees, CI, browser, CD, displays, system layer, runtime state, run directories, the
  sandbox directory with its clone, home, sessions, attachments and imports, transcripts, a
  playground's origin, the export), deleted through the leftovers' guards and mapped removal, each
  audited `leftovers.delete`, then `sandbox.purged`. Failed paths are retried on the next round.
- **RDA-012** Keep: today's behaviour (the disposable parts go after 10 minutes, SPEC-215 LFT-001;
  the rest stays). Settings → Instance → Leftovers lists **Kept files of removed sandboxes**: each
  removed sandbox with files, its removal date, the size of everything of it, and **Delete
  files**, which confirms with the size and requests the purge (RDA-011). A sandbox whose purge
  waits says what it waits for. The superadmin may request the purge of any removed sandbox, an
  administrator of a live one when removing it.

## Plan

- [x] Spec and ADR-0205
- [x] Build caches are swept by last use under Cargo's own lock
- [x] A build cache over its cap loses its least recently used units
- [x] A low or critical disk sweeps harder, prunes idle caches and is audited
- [x] The UI warns while the server's disk is low
- [x] Archived tasks lose their caches at once; the last leftovers round is shown
- [x] Every sandbox's files can be purged once nothing of it runs
- [x] Removing a sandbox asks whether to delete or keep its files
- [x] Kept files of removed sandboxes are listed with their size and can be deleted
- [x] Architecture, operations, product, PRJ-009 and changes documented
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green on the reviewed head, spec evidence recorded
- [x] Full e2e suite green on the final head

## Acceptance criteria

- A build directory with a unit last used 3 days ago and one used an hour ago loses the first
  (its `.fingerprint`, `deps` and `build` entries together) and keeps the second.
- While a test process holds `.cargo-lock` of a profile, nothing of that build directory is
  deleted; once released, the next sweep deletes the old unit. A working task's worktree and a CI
  checkout whose CI run runs keep their artifact units.
- An `incremental/<crate>-<hash>` directory with an `s-…-working` session stays whatever its age.
- A build directory over its cap ends at or below three quarters of it, old units first, and a unit
  used within the hour stays (normal level).
- With the data disk at 96 %, a round prunes an idle task's `target/` that the normal level keeps,
  audits `disk.pressure`, and `GET /api/disk` answers `critical`; the pill shows on the page.
- An archived stopped task's `target/` goes in the round its archiving started, without waiting
  10 minutes.
- The leftovers view shows the last automatic round with what it deleted.
- Removing a GitHub sandbox with "Delete permanently" deletes its sandbox directory, system layer,
  runtime state, transcripts and export once nothing of it runs, without waiting 10 minutes; with an
  agent run still live it waits and says so, and the watch deletes them within seconds of its end. "Keep" deletes none of the kept files.
- The remove dialog cannot be confirmed before a choice is made.
- Settings → Instance lists a kept removed sandbox with its size; Delete files deletes them.

## Test plan

- `yulius-domain` unit tests (`sweep.rs`): unit keys of `.fingerprint`, `deps` (with `lib`
  prefixes and extensions) and `build` entries, incremental keys; the plan by age, by cap with the
  recent floor, `*-working` never chosen; levels at 85 % and 95 %; the policy per level; the cap.
- `yulius-app` unit tests (`sweep.rs`): a real temporary build directory with times set through
  `File::set_times`: old units go whole, recent ones stay; a held `.cargo-lock` (a `File::lock`
  in the test) skips everything; a lock file that is a link skips; `*-working` stays; cap eviction;
  a link in the way is never followed.
- `yulius` integration tests:
  - `disk.rs`: the round sweeps a worktree's and a CI checkout's build directory; a critical disk
    (the harness's settable disk) prunes an idle task's caches and audits `disk.pressure`; an
    archived task's caches go without the grace;
  - `leftovers.rs`: the purge of a GitHub sandbox deletes every kept path once nothing runs, waits
    while a shell runs, and goes without the 10-minute wait; "keep" leaves the kept paths; the
    view lists kept sandboxes with their size and the last automatic round;
  - `projects.rs`/`playground.rs`: the purge route accepts a GitHub sandbox; members are refused;
    playground purges keep working.
  - HTTP: `GET /api/disk` for a member; the `disk.updated` event on a level change.
- e2e (`e2e/tests/disk-files.e2e.ts`): the remove dialog's choice gates the button and each choice
  calls its route; the kept-files list and its Delete files; the disk pill at `low` and none at
  `ok`; no WCAG violations.

## Decisions

Lasting ones in ADR-0205. Summary:

- **Cargo's own lock, not process scanning, and artifacts only where nothing tests.** Every Cargo
  command that compiles into a build directory holds its locks; a non-blocking `flock` tells
  exactly whether one compiles, across the sandbox's namespaces, and holding it keeps a new one
  from starting mid-sweep. Test binaries, doctests and nextest read `deps/` after Cargo released
  them, so artifact units go only under the CI claim or for a resting task (review round 1).
  Scanning `/proc` for working directories needs permissions on the sandbox's processes and still
  races.
- **Whole units by hash, like cargo-sweep.** Deleting single files could leave a fresh fingerprint
  without its output or a build script's `out/` without its run, which breaks the next build.
  Deleting a unit's fingerprint with its outputs makes Cargo rebuild exactly that unit.
- **48 h, not 24 h.** With `relatime` an access time can lag a day; 48 h is the shortest age that
  proves a day without a read. Under pressure correctness still holds (the lock, and artifacts
  only where nothing tests), only rebuilds cost more. For the same reason the cap evicts
  incremental units only: their sessions are rewritten at every compile, so their modification
  times are exact.
- **A cap per build directory of 5 % of the disk (≥ 8 GB).** A full Yulius gate needs about 22 GB
  of which 15 GB is incremental state; a 19 GB cap trims the stale incremental directories (16
  generations of `yulius` alone) without evicting the current build, which the 1-hour floor
  protects. A fixed number would be wrong for small and large disks alike.
- **Host thresholds 85 % and 95 %.** Below 85 % the hourly sweep keeps up; at 1 GB a minute the
  last 5 % (19 GB) lasts about 20 minutes, which is when idle tasks' caches must go too. A
  per-sandbox quota keeps its own 90 % rule (SPEC-008).
- **The purge reuses the leftovers' paths, guards and mapped removal** instead of SPEC-171's
  in-process `delete_tree`, so the system layer and files of the sandbox's mapped ids go too and
  nothing is followed through a link. The `playground_purges` table records requests for every
  sandbox (no migration; the name is historical).
- **Two windows left open (review round 2).** Whether a task rests is probed before the sweep
  takes Cargo's locks: a task that resumes in between and finishes a no-op compile before the
  locks are taken could start its tests beside a sweep allowed to delete artifacts. The window is
  the probe plus a directory listing, and only units unused for the policy's age go, so it is
  accepted. An archive-triggered upkeep that finds the sandbox's upkeep already running returns at
  once; the archived task's caches then go in the next round (5 minutes), not "at once".
- **No default in the dialog.** Deleting is irreversible and keeping costs disk; the person who
  removes the sandbox knows which matters, so both are one click and neither is preselected.
- **One pull request.** Both halves came from one incident and share the disk round, the
  leftovers engine and the Leftovers card; the sweep, pressure and purge are separate commits.

## Evidence

Reviewed head `5cdc1423` (task branch `feat/261-reclaim-disk-automatically-ask-on-sandbo`, with
`dev` ab8ab732 merged in).

- **Measured here** (2026-10-22): a sibling worktree's `target/` held 22 GB: `debug/incremental`
  15 GB in 208 crate directories (16 generations of `yulius`), `debug/deps` 4.5 GB, the macOS
  cross build 2.7 GB. `/var/lib/yulius` inside a task is a tmpfs view of the sandbox, so the VM's
  own disk and its removed sandboxes could not be measured from the task.
- **Cargo's locks** (cargo 1.99, a probe crate whose `build.rs` sleeps 15 s, outside the
  worktree): during the build `flock -n` found `.cargo-lock`, `.cargo-build-lock` and
  `.cargo-artifact-lock` of `target/debug` held; after it, all three free.
- **Fails before, passes after** (each checked by breaking the fix locally):
  - `sweep::tests::a_held_cargo_lock_keeps_the_whole_build_directory` and
    `a_cross_target_profile_s_lock_counts_too` fail with the lock check disabled;
  - `sweep::tests::a_fifo_named_like_a_lock_counts_as_held_and_never_blocks` times out without
    `O_NONBLOCK`;
  - `disk::a_critical_data_disk_prunes_idle_caches_and_is_audited` fails without the host need;
  - `disk::an_archived_task_loses_its_caches_without_the_grace` failed before the rule ("archived:
    no grace");
  - `disk::archiving_a_task_through_the_api_removes_its_build_output` fails without the spawn;
  - `leftovers::a_purged_sandbox_loses_every_file_once_nothing_runs_and_a_kept_one_keeps_them`
    fails with the 10-minute wait kept for a requested purge;
  - `leftovers::the_leftovers_round_leaves_a_purged_sandbox_to_its_purge` fails without the filter;
  - `shells::a_live_shell_keeps_a_resting_task_s_build_artifacts` failed before the Shell check
    ("a Shell may be running its tests").
- **Suites:** `yulius-domain` sweep tests (5), `yulius-app` sweep tests (12) and `disk::tests`,
  the `disk::` (31), `leftovers::` (22), `playground::` and `shells::` integration tests pass.
- **`scripts/dev ci` at `5cdc1423`:** exit 0. 2532 tests passed, 17 skipped; lint, format, docs,
  deny, shear, repository contracts and the macOS cross-check passed. The run at `66ff1555`
  failed at `cargo doc` (public docs linking the purge's private constants), fixed in `615fd9cc`.
- **e2e:** `tests/disk-files.e2e.ts` (4: the pill at low and critical, for members, none at ok;
  the remove dialog's choice gates Remove, Delete purges, Keep keeps), `tests/leftovers.e2e.ts`
  (5, with the last automatic cleanup and the kept files' Delete files), `tests/playground.e2e.ts`
  (9) and `tests/places.e2e.ts` (21) pass. The full suite at `5cdc1423` (`cd e2e && npx e2e run`):
  65 files, 685 tests passed and 1 skipped, exit 0.
- **Security:** `scripts/vidoc_scan.py --base origin/dev` exited 2, not run: "Vidoc is out of
  quota for this key (HTTP 429: This API key has used its 50,000,000 tokens.)". As the person
  asked before the task started ("if you face vidoc problem, continue without vidoc atm"), the
  change goes on without it; it is not a pass. Yulius CI's `security` job reports the same state.
- **Independent review:** three rounds in a fresh read-only session. Round 1 asked for changes
  (four majors: a FIFO lock file could hang the upkeep; test binaries, doctests and nextest read
  `deps/` after Cargo unlocks; the export's deletion was not said in ADR-0139, SPEC-171 or the
  dialog; untested loop, watch and archive paths), all fixed test-first. Round 2 approved with two
  minors: the Shell case was fixed, the probe-to-lock window and the archive-vs-loop upkeep are
  accepted (Decisions). Round 3 approved `5cdc1423`. Filed for pre-existing code: #853 (a FIFO
  swapped in for `yulius.toml` blocks the existing upkeep).
- **Not testable here:** the real mapped removal of a purged sandbox's system layer (needs the
  contained service user and `bwrap`; the opt-in isolation suite), and the behaviour on the VM's
  real 387 GB disk. After promotion and update, check in Settings → Instance → Leftovers that the
  last automatic cleanup runs and that kept sandboxes are listed; watch `disk.sweep` and
  `disk.pressure` in the audit and `df -h /var/lib/yulius`.
