---
yulius-task: 7ebbad1e-501f-45e7-b961-19cf7c7850ac
status: done
---

# SPEC-329: Export the whole instance as one stream and install it on another VM

Status: done
Date: 2026-10-26
Owner: task #313
Scope authorization: task #313: "Add a way to export the whole VM's state. On exporting,
everything should be paused and read-only to avoid drifts. Export should stream to avoid
increasing disk on the vm (it should be possible even if 99% of the VM's disk is occupied) but at
the same time total num of GB should be known from the beginning. We want to be able to install
from the copy or stream to different VM directly, without the private disk. Figure out how to do
the UX and things."

## Goal

An operator copies a whole Yulius instance (database, vault key, sandboxes with their clones,
worktrees, homes and Claude sessions, installed packages and Docker data, transcripts, runs) off
its VM with one root command, without writing anything on that VM's disk, knowing the exact size
before the first byte, while every Yulius process is paused so the copy cannot drift. The same
stream installs the instance on a fresh VM, from a file or piped straight from the old VM.

```sh
# straight from the old VM to a new one (nothing written on either side but the result)
sudo yulius instance export | ssh root@new-vm yulius instance import
# or keep a copy elsewhere and install it later
ssh root@old-vm yulius instance export > yulius-2026-10-26.tar
ssh root@new-vm yulius instance import < yulius-2026-10-26.tar
```

## Context

Confirmed against `dev` at 84b45934.

- Today `yulius backup` writes a `VACUUM INTO` copy of the database only, to a file on the same
  host, and the operations guide (§7) tells the operator to copy `master.key` and, "if wanted",
  `sandboxes/` by hand. Nothing copies `sysroots/` (installed packages, Docker images and
  volumes, owned by the service user's subordinate ids), keeps the pieces consistent with each
  other, or installs a copy on another host. The update helper copies `yulius.db` and its WAL raw
  while the service is stopped (`update_helper.rs` `backup`).
- Processes: everything Yulius runs lives in the `yulius.service` cgroup (`Delegate=yes`,
  `DelegateSubgroup=server`, `KillMode=process`): the server in `<unit>/server`, each sandbox's
  holder, agents, CI jobs and Docker engine in `<unit>/sandboxes/<sb>/{system,agents,ci,engine}`
  (`yulius-sandbox/src/cgroup.rs`), leftovers of older starts in `<unit>/legacy`. Stopping the
  service leaves agents and terminals running (KillMode=process), so a stop does not stop drift.
  Per-run freezing already uses the cgroup v2 freezer (ADR-0131, `freeze.rs`); a frozen ancestor
  freezes every descendant and a thaw restores each child's own state.
- The database is one rusqlite connection on one thread, WAL mode, `synchronous=NORMAL`, with no
  read-only switch (`yulius-store/src/lib.rs`). SQLite recovers a database from its main file and
  WAL as they stand at any instant (crash consistency); the `-shm` index is rebuilt.
- Data directory: `yulius.db`, `master.key`, key files, `sandboxes/`, `sysroots/` (mapped ids),
  `system/` (the sandbox base, a copy of the host system built by `host setup`; base files are
  re-owned to mapped ids), `runtime/` (holders' and shims' runtime files), `runs/`,
  `transcripts/`, `exports/`, `shells/`, `logins/`, `users/`, `cd/`, `displays/`, `changes/`,
  `import-worktrees/`, `playgrounds/`, `skills/`, `tmp/`, `update/` (the server's request to the
  root update helper, which a path unit watches).
- `yulius host setup` (SPEC-203) brings a fresh Ubuntu host to a runnable state: packages, the
  service user (owner of the data directory, else `--user`, created with the data directory as
  its home), a subordinate id range, the unit, the base; `Run` items are methods behind the
  `Host` trait (testable with a fake host).
- Nothing in Yulius streams an archive; `tar` 0.4.46 (no default features) and `rustix` 1.1
  (`fs`: `openat`, `*xattr`, `mknodat`, `statvfs`, `utimensat`) are workspace dependencies.

## Requirements

- **EXP-001 Freeze.** `yulius instance export` runs as root on the host. Before it reads
  anything it freezes the service unit (`systemctl freeze yulius.service`, cgroup v2 freezer):
  the server, every agent, terminal, browser, CI job, deploy and sandbox Docker engine stops
  between two instructions, so no Yulius process writes while the copy is read (read-only by
  construction). It checks that systemd reports the unit `frozen` (up to 60 s, else it thaws
  and refuses: a process may hang in the kernel) and prints, before the first byte, the command
  that thaws it by hand (`sudo systemctl thaw yulius.service`). After the stream ends it thaws
  the unit, also when the export fails, the reader goes away (a broken pipe) or the command gets
  SIGINT, SIGTERM or SIGHUP (a dropped SSH session), answered on a thread of their own so a
  thaw happens while the export is blocked on a stalled pipe; only SIGKILL leaves it frozen.
  - `--move`: a successful export leaves the instance frozen and disables the unit, so neither
    a thaw by mistake nor a reboot runs the old instance beside the new one (same agents, same
    repositories); it prints how to undo that (`systemctl thaw` and `systemctl enable`). A unit
    that was already frozen before the export is left frozen.
  - A stopped unit whose cgroup still holds processes (KillMode=process keeps agents) is refused:
    they would write during the copy. A stopped unit with an empty cgroup needs no freeze.
  - An export started from inside the unit's cgroup (a Yulius terminal) is refused: it would
    freeze itself.
  - A running update helper (`yulius-update.service` active) is refused, before the freeze, once
    the unit is frozen (a request written just before it may start the helper: thawed and
    refused) and before the checksum is written (the copy is left without it, so incomplete).
  - `--service NAME` names another unit; `--data-dir` (`YULIUS_DATA`) the data directory.
- **EXP-002 Exact size first.** With everything frozen the command plans the archive: one walk
  of the data directory computing the exact byte length of the stream. It prints the size and
  entry count to stderr ("Exporting 123.4 GB (1,234,567 entries) of /var/lib/yulius") before the
  first byte, and the first archive entry, the manifest, carries the same `total_bytes`, so the
  receiver knows the total from its first kilobyte.
- **EXP-003 Stream.** The archive is written to stdout only, as it is read: nothing is written to
  any disk of the exporting host (an export works on a full disk). Standard output that is a
  terminal is refused ("pipe it to a file or to ssh"). Progress goes to stderr about once a
  second: done/total, percent, rate, time left. The stream is exactly the announced length; a
  file whose size changed under the freeze aborts the export (exit 1) rather than send a copy
  that differs from the plan.
- **EXP-004 Format.** A POSIX pax tar (readable by GNU tar): an entry gets a pax header only when
  a field does not fit ustar (`path`, `linkpath`, `uid`, `gid`, `size`, `mtime`) or it has
  extended attributes (`SCHILY.xattr.*`), then a ustar header and the data padded to 512 bytes. An
  entry whose pax header passes the reader's 1 MiB bound fails the plan, before the first byte. Order:
  `yulius-export.json` (manifest), `data` and `data/...` (the data directory, parents before
  children, names sorted bytewise), then `yulius-export.sha256`, the SHA-256 of every byte before that entry's header, and the two
  zero blocks. Entries keep numeric owners, mode bits (with setuid/setgid/sticky), mtimes,
  extended attributes, symbolic links as links, hard links as links to the first path, and
  character devices 0:0 (overlay whiteouts). Uncompressed: compression would hide the exact size
  (a person may pipe through `zstd` themselves).
- **EXP-005 Manifest.** `{format: 1, total_bytes, entries, created_at, yulius_build, schema,
  data_dir, public_url, os_release, service: {unit, user, uid, gid}, subuid: {start, count},
  subgid: {start, count}}`, padded with spaces to a length fixed before the total is known;
  `schema` is the number of database migrations the exporting binary knows, `public_url` the
  unit's `YULIUS_PUBLIC_URL`, `os_release` the host's `ID VERSION_ID`. The unit file itself is
  not carried: the new host's unit is written by `host setup`, never taken from the archive.
- **EXP-007 Secrets.** The stream holds `master.key` with the database it opens, sign-ins and
  homes: whoever has it has every secret. The command says so on stderr before the first byte
  and the guide writes copies with `umask 077`; transport is the person's (`ssh`).
- **EXP-006 What is left out**, because the new host makes its own: `system/` (the sandbox base,
  a copy of the host system; `host setup` builds one for the new host), `runtime/` (live
  processes' sockets and files), `update/` (a pending request would start an update on the new
  host), `yulius.db-shm`, sockets, FIFOs and block or character devices other than 0:0. Nothing
  else is left out: caches included (the copy is the whole state; "the size" is the size).
- **IMP-001 Import.** `yulius instance import [FILE]` (stdin without FILE or with `-`) runs as root
  on the new host with the `yulius` binary installed. The target is `--data-dir` (`YULIUS_DATA`,
  default `/var/lib/yulius`), never the manifest's alone. It reads the manifest first and
  refuses, before writing anything:
  - a format it does not know, or a `schema` above its own ("install a Yulius at least as new as
    the exported one, build X");
  - a target that is not a plain absolute path (no `.` or `..`), or not the manifest's
    `data_dir`: Git worktrees, database rows and sandbox files hold absolute paths under it;
  - the marker of an import that did not finish (`.yulius-import-incomplete`, even a link:
    "remove the data directory and retry");
  - an existing target that is not a directory, holds anything but `lost+found` and dotfiles
    (what `useradd --create-home` copies), "already holds an instance" when `yulius.db` or
    `sandboxes` is there, or belongs to another user than the service user, or to root (a new
    disk: the refusal prints the `useradd`, `chown` and `chmod` to run);
  - fewer free inodes than the manifest's `entries` (where the filesystem counts inodes);
  - a free space below `total_bytes` on the data directory's filesystem ("needs 123.4 GB, 80.2 GB
    free");
  - a non-root caller.
- **IMP-002 Host first.** It then makes the service user (the manifest's name unless `--user`)
  and its subordinate id range with `host setup`'s own items (a range on this host smaller than
  the exported one is refused then, before anything of the copy is written), creates the marker
  (create-new: a link is never followed), and extracts `data/` into the data directory as it
  reads, with progress to stderr against `total_bytes`. It writes the
  incomplete marker first and removes it last. A different `os_release` is a warning (copied
  package layers sit on this host's base).
- **IMP-003 Owners.** Owners are remapped: the old service uid/gid → the new ones; an id inside
  the old subordinate range → the same offset in the new range; any other id (root included)
  → the service user with setuid and setgid removed, counted in a warning. The import never
  creates a file owned by host root or by an id outside the service's. Order per entry: owner,
  then mode, then extended attributes (a chown clears setuid bits and capabilities).
  Extended attributes restored: `user.*` and `trusted.overlay.*` (overlay layers); others
  (`security.capability`, ACLs, which hold host ids) are skipped and counted in a warning.
- **IMP-004 Safe extraction.** Paths are relative, start with `data/`, and have no `..`, `.` or
  empty component. Every directory on the way is opened with `O_NOFOLLOW` from the data
  directory's handle, so a link in the archive (stored as a link) is never followed by a later
  entry; a hard link must name an earlier regular file of the same archive (a set of the
  written files' path hashes). An existing file at
  an entry's place is replaced; an existing directory is kept and given the entry's metadata.
- **IMP-005 Integrity.** The import hashes what it reads and compares it with
  `yulius-export.sha256`, and the length read with `total_bytes`; a missing trailer, a short
  stream, a mismatch or an entry type other than directory, file, symlink, hard link and
  character device 0:0 fails the import
  (exit 1) and says the data directory holds an incomplete copy to remove before a retry.
- **IMP-006 After.** The command prints the next steps: `sudo yulius host setup --public-url
  <the manifest's>` (writes the unit, builds the base for this host, checks Docker, starts
  Yulius), copy other `Environment=` lines changed on the old unit, point the reverse proxy and
  DNS here, and keep the old instance frozen or stopped (an export with `--move`): both would
  run the same agents against the same repositories.
- **UX-001** Both commands print one summary line at the end (bytes, entries, time) and exit 0
  on success, 1 on failure, 2 when refused before anything was done. `yulius instance --help`
  shows the two pipe recipes.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Exact-size streaming archive: pax tar writer with manifest and checksum trailer
- [x] Data directory plan: walk with exclusions, hard links, xattrs and an exact total
- [x] Freeze guard: the service frozen during the export, thawed or kept frozen after, refusals
- [x] `yulius instance export` streams to stdout with progress
- [x] Safe extraction: no link followed, owners remapped, xattrs, hard links, checksum checked
- [x] `yulius instance import` prepares the host, checks disk and version, makes the service user and its ids
- [x] Docs: operations guide, architecture, ADR, product summary
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1: For a tree with files, empty files, directories, symlinks, hard links, long paths, large
  ids and extended attributes, the bytes written equal the planned total, and GNU tar lists it.
- AC-2: `system/`, `runtime/`, `update/`, `yulius.db-shm` and sockets are not in the archive.
- AC-3: The export freezes the unit before planning and thaws it after a success, after a failure
  when the reader closes the pipe and on SIGTERM; `--move` does not thaw after a success and
  disables the unit; a unit that never reports frozen is thawed and refused. A stopped
  unit with processes, a run from inside the unit's cgroup, a running update helper and a
  terminal stdout are refused with exit 2 and nothing frozen.
- AC-4: Export then import of a tree reproduces contents, modes, mtimes, symlinks, hard links and
  user extended attributes; owners are remapped by the manifest's ranges.
- AC-5: An archive with `../`, an absolute path, an entry under a link of the archive, or a hard
  link to a path not in it is refused without writing outside the data directory; foreign
  owners become the service user without setuid; only `user.*` and `trusted.overlay.*`
  attributes are restored.
- AC-6: A truncated archive or one whose checksum differs fails the import; a too-new schema, an
  existing or incomplete instance, another data directory, too little free space or inodes and
  a smaller id range are refused before writing.

## Test plan

- Unit tests (`crates/yulius/src/instance/tests.rs`): writer size equals plan (AC-1), exclusions
  (AC-2), freeze guard over a fake `systemctl` (AC-3), round trip into a temporary directory as
  the test user with ownership mapping tested as a pure function (AC-4), hostile archives built
  by hand (AC-5), truncation, corrupted byte, schema and free-space refusals (AC-6).
- A test that GNU `tar -tvf` reads an exported archive (skipped when `tar` is missing).
- Not testable in CI: `systemctl freeze` of a real unit, root-only ownership changes and
  `mknod` of whiteouts, a real two-VM move. The guard is tested through its command runner;
  root-only steps are covered where the test runs as root and otherwise skipped with a message.

## Decisions

- **Freeze the whole unit, server included, with systemd's freezer** (ADR-0222): one atomic,
  reversible operation pauses every Yulius process and makes the data directory read-only by
  construction; the database's main file and WAL are then crash-consistent, which SQLite is
  built to recover. A read-only server with frozen sandboxes was rejected: the server's
  background loops write Git and files outside the store, so a write gate in the store is not
  enough, and it would touch every writer.
- **The export runs as root on the host, not from the web UI**: it reads files owned by
  subordinate ids and freezes the server that would serve a web request; installing needs root
  on the new host anyway. The web UI is unreachable while frozen; the export is an operator's
  maintenance step like an update.
- **Uncompressed pax tar on stdout**: the only format whose length is known before writing;
  pipes give file, `ssh` and VM-to-VM transfer without a protocol of our own.
- assumed (reversible): "without the private disk" means no intermediate disk anywhere (neither
  on the old VM nor a staging disk): the stream goes from the old data directory to the new one,
  because the request pairs it with "stream to different VM directly".
- assumed (reversible): after a successful export the instance is thawed by default (a backup
  keeps working); a move uses `--move` (frozen and disabled, undoable), because thawing is the
  less surprising default for a command that does not say "move", and a move must survive a
  reboot of the old VM without starting it.
- **The import trusts the archive no more than any input** (spec review): it never installs a
  unit, never creates root-owned or foreign-owned files, restores only overlay and user
  attributes, and refuses every entry type but five.
- **One pull request for export and import**: the archive format is one contract; its writer and
  reader are reviewed and tested together (round trip), and the export alone is not a way back.
- assumed (reversible): no streaming encryption (`age`) now: `ssh` carries the move, and a copy
  at rest is the person's to encrypt (`| age -r …`); the guide says so.
- assumed (reversible): the walk crosses into other filesystems under the data directory, since
  the guide recommends bind mounts for moving parts of it to another disk; sandbox mounts live
  in their own namespaces and are not seen on the host.
- assumed (reversible): rebuildable caches are exported too; a `--skip-caches` option can come
  later, because the request asks for the whole state.
- assumed (reversible): a Settings → Instance card is not part of this change: the work is root
  CLI on both hosts and the operations guide documents it.

## Evidence

Head b1cccd89 (merged with `origin/dev` at 056e061c).

- `scripts/dev ci`: exit 0. Repository contracts, Python tests, `cargo check`, `fmt --check`,
  clippy `-D warnings` on Linux and for `aarch64-apple-darwin`, `cargo nextest`: 2696 tests run,
  2696 passed (16 skipped), doctests, `cargo deny`, `cargo shear`, `cargo doc`.
- The feature's own tests (`instance::` and `host_setup::`, 97, all passing, run as root):
  - AC-1: `archive::tests` (written length equals the plan for files, links, hard links, long
    and non-ASCII paths, large ids, binary attributes, negative times; GNU `tar -tvf` and the
    `tar` crate read it). `export::tests` checks the stream against the announced total.
  - AC-2: `walk::tests` (system/, runtime/, update/, yulius.db-shm, FIFO and socket left out).
  - AC-3: `freeze::tests` (freeze then thaw, thaw on drop, `--move` disables, a unit that never
    freezes is thawed and refused, inside-the-unit, a running or newly started update helper,
    a stopped unit with processes, signals: thaw only a held freeze, claim of the end).
  - AC-4: `extract::tests::an_exported_tree_installs_as_it_was`, the import round trip.
  - AC-5: `extract::tests` (paths, links of the copy and already there, hard links outside
    the copy, foreign owners lose setuid, only user/overlay attributes).
  - AC-6: `import::tests` (cut, damaged, wrong length, schema, format, free space, an
    existing or non-empty target, another owner, `..`, a marker or link at the marker,
    another data directory than the exported one).
  - Mutation checks: each test added for a review finding fails with its fix removed.
- Binary smoke (debug build, root in a sandbox, a stand-in `systemctl` on PATH): `yulius
  instance export --data-dir <tree> > copy.tar` called `show`, `is-active`, `freeze`, `show`,
  `thaw` in that order; the file was 3,015,168 bytes, the manifest's `total_bytes`; GNU tar
  listed it without `system/`. `yulius instance import --user ubuntu < copy.tar` into the
  exported path made the directory for the existing `ubuntu`, remapped owners, printed the next
  steps; a second import was refused (exit 2); a cut copy failed (exit 1) and left the
  incomplete marker. `/etc/passwd` and `/etc/subuid` were unchanged.
- Independent review (fresh session, `om-code-review`), four rounds: round 1 request changes
  (High: the manifest chose where root writes; Medium: signals blocked by a stalled write, the
  update helper starting under the freeze; Lows: btrfs inodes, marker through a link, hard
  links to files not in the copy, oversized pax headers at plan time, messages, umask recipe,
  spec wording); round 2 approve with two Lows (a signal racing the `--move` end, exit 1 instead
  of 2 on a signal during a refused freeze) and a nit (hard-link digests); round 3 one
  Medium (a signal right after the export took its end); round 4 approve on b1cccd89, no
  findings left. All findings were own code and fixed test-first; none pre-existing.
- PR #923's first CI run (f04234e5) failed one e2e test that came in with the `dev` merge,
  `agent-panel.e2e.ts` "a hidden Superagent or Shell terminal keeps its session's size"
  (SPEC-331): its `fitted` took the mock Shell session's 100×30 from before the page's hello
  reached it, so under load the later check compared the page's 50×39 with it. Reproduced by
  holding the Shell's hello (`__yuliusHoldShell`) until after `fitted` ("expected 50x39 to be
  100x30", the CI message); fixed in the test by waiting for the hello first; the file passes
  with a 1 s hold and without it. Outside this feature; fixed here because it blocked the merge.
- Security scan (`yulius security-scan`, beside the gate): **not run**, exit 2: Vidoc answered
  HTTP 429 "This API key has used its 50,000,000 tokens" (the instance key's quota). The PR's CI
  `security` job is the live scan.
- Not tested: a real `systemctl freeze` of `yulius.service` with live agents, a real two-VM
  move over `ssh`, a terabyte-scale copy, and a restore of `trusted.overlay.*` and whiteouts into
  real overlay layers followed by a sandbox start on the new host (no systemd or second VM in
  the sandbox). The freeze is tested through its command runner; extraction ran as root in the
  sandbox (ownership, setuid) but not with overlay mounts.

