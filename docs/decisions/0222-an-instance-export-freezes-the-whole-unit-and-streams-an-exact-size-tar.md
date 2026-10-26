# 0222. An instance export freezes the whole unit and streams an exact-size tar

## Status

Accepted. Specified in [SPEC-329](../../.ai/specs/SPEC-329-add-a-way-to-export-the-whole-vm-s-state.md);
task #313. Extends [0131](0131-an-update-freezes-every-task-at-once.md)'s use of the
cgroup freezer from one run to the whole service; complements `yulius backup` (database only).

## Context

- An operator wants to copy a whole instance off its VM (to keep it, or to move to another VM),
  with nothing drifting while it is read, on a disk that may be 99 % full, knowing the size
  before the transfer starts, and to install the copy on a fresh VM from a file or straight from
  the old VM.
- Everything Yulius runs is in the `yulius.service` cgroup (`Delegate=yes`,
  `KillMode=process`): the server, every sandbox's holder, agents, terminals, browsers, CI jobs,
  deploys and Docker engines. Stopping the service leaves agents and terminals running.
- The database is SQLite in WAL mode with one in-process writer and no read-only switch; the
  server also writes Git checkouts, transcripts and run files outside the store.
- `sysroots/` holds files owned by the service user's subordinate ids, which only root (or a
  mapped namespace) reads in full; another host gives the service user other ids.

## Decision

- **Freeze the unit, server included**: `yulius instance export` runs as root and calls
  `systemctl freeze <unit>` (cgroup v2), waits until systemd reports it frozen (60 s, else it
  thaws and refuses), reads, then `systemctl thaw`, also on a broken pipe, SIGINT, SIGTERM and
  SIGHUP. While frozen no Yulius process runs, so the data directory is read-only by
  construction and the database's main file and WAL are crash-consistent, which SQLite recovers.
  It refuses a stopped unit whose cgroup still holds processes, a run from inside the unit's
  cgroup, and a running update helper. `--move` keeps the unit frozen and disables it after a
  successful export, so a reboot does not start the old instance beside the new one.
- **An uncompressed POSIX pax tar on stdout, sized first**: a first walk computes the exact
  length (headers are computed from metadata, data padded to 512 bytes) and a SHA-256 of every
  header; the manifest, the archive's first entry, padded to a fixed 8 KiB, carries the total, so
  both ends know it from the first kilobyte. The second walk streams; a header that differs from
  the plan aborts the export before the trailer. The archive ends with a SHA-256 of every byte
  before the trailer. Nothing is written on the exporting host. `system/` (the sandbox base),
  `runtime/`, `update/`, `yulius.db-shm`, sockets, FIFOs and devices other than overlay
  whiteouts are left out: the new host makes its own.
- **Pipes are the transport**: `export | ssh root@new yulius instance import`, or a file. No
  protocol, port or credential of our own.
- **The import trusts the archive no more than any input**: it reads with its own reader, opens
  every directory with `O_NOFOLLOW` from the data directory's handle, accepts five entry types,
  maps the old service ids and subordinate range to this host's (made with `host setup`'s own
  items) and gives anything else to the service user without setuid or setgid, restores only
  `user.*` and `trusted.overlay.*` attributes, never installs a unit, never overwrites an
  instance, and keeps a marker until the checksum and length matched. The copy goes to the same
  data directory path, since Git worktrees and the database hold absolute paths.

## Alternatives

- **A read-only server with only the sandboxes frozen**: the web UI would answer during the
  export, but a write gate in the store misses the server's Git and file writes, and every
  background loop would have to learn it. Rejected for this change.
- **`systemctl stop`**: leaves agents and terminals running (`KillMode=process`), so the copy
  would drift.
- **`VACUUM INTO` and `rsync`**: the database copy needs as much free disk as the database, and
  rsync's total is not known before it runs and needs rsync and SSH between the hosts.
- **A compressed stream (`zstd`)**: smaller, but its length is unknown until the end; a person can
  still pipe the exact-size tar through a compressor.
- **A web-UI export or download**: the server runs as the service user, cannot read every
  subordinate-owned file, and would be frozen itself; installing needs root on the new host
  anyway.
- **Installing the old unit file from the archive**: an archive's unit would run as root;
  `host setup` writes the unit instead and the import prints the public URL to give it.

## Consequences

- During an export the web UI and API do not answer and every agent, terminal and CI job is
  paused; after the thaw they continue (a Claude Code request cut by the pause is retried by
  Claude Code). The export prints how to thaw by hand: only SIGKILL leaves the unit frozen.
- The copy holds `master.key` with the database it opens: whoever has it has every secret. The
  command says so; the guide writes files with `umask 077`.
- Packages installed in sandboxes (overlay upper layers) sit on the new host's base; a different
  distribution release is warned about.
- File capabilities and ACLs in copied files are not restored; the import counts them.
