# 0303. A move copies warm and freezes only for what its receipts do not cover

## Status

Accepted. Specified in
[SPEC-465](../../.ai/specs/SPEC-465-run-migration-warm-copy-while-the-instan.md); task #445
(slice B of #442). Extends [0297](0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md)
(the move) and [0222](0222-an-instance-export-freezes-the-whole-unit-and-streams-an-exact-size-tar.md)
(the stream).

## Context

- The move of ADR-0297 freezes the old instance for the whole copy: 54 minutes for production's
  275.8 GiB and 6.4 million entries.
- The move's proof compares metadata on both hosts (entries, bytes, a fingerprint of every
  entry's headers), not content. Whatever decides that an entry need not be sent again must
  therefore be right about its content by itself.
- Size and modification time are not enough: a file is rewritten in place within one second, or
  its time is set back.
- The inode's change time moves with every write, truncate, rename, link, owner, mode and
  attribute change of that inode, and cannot be set. Three cases still escape it: a change within
  the clock's tick of the last one; a write through a shared mapping, which stamps the inode only
  at the first write to a clean page; and a file that returns to its path unchanged because a
  directory above it was renamed away and back, after the path held another file.
- A pass over millions of small files takes an hour. A broken link or a killed command must not
  cost it again.

## Decision

- **The export's stream is the transport.** A pass of a move is an export whose manifest names
  the stage, the pass and a nonce. Warm passes are read while the instance runs: one walk,
  vanished paths left out, a file's header taken from the open file and its bytes padded when it
  shrank; their totals are estimates, their checksum is checked. The frozen pass is exact as
  before and ends with the digests of every path the old host holds.
- **A walk over a running instance never follows a name out of the data directory.** It runs
  as root while the sandboxes' processes write there. It holds every directory it is in open,
  opens each one in the directory that holds it without following a link, and names every entry
  through that open directory (`/proc/self/fd/<n>/<name>`), so a directory that is renamed away
  or replaced by a link between two looks leads it nowhere else. A file is reached through a
  handle that opens nothing, checked to be a regular file, and then opened plainly.
- **Receipts are a map from a path to the state the new host holds.** The state is a 16-byte
  digest of the entry's archive headers, its inode's change time in nanoseconds and its inode
  number. A pass skips an entry only when its path's receipt is that state. Sending a path
  revokes its receipt, so a path that held another file in between is sent again.
- **A receipt needs an entry older than its pass.** A pass begins by syncing the data directory's
  filesystem, which write-protects every mapped page. An entry earns a receipt only when its
  change time is at least 2 s before that, its stat did not move while it was read, and it lies
  on the synced filesystem. Any later change, through a mapping too, gets a later change time.
- **Receipts are believed only where they were earned.** An inode's times are not written with
  its data: after a crash a file rewritten in place may hold new content under the change time
  its receipt was earned with. And a clock that is set back by 2 s or more can stamp a later
  change with the time of what was copied. The receipts record the boot and the furthest the
  clock was ahead of the boot's own time when a pass began; another boot, or a clock 1.5 s or
  more behind that, drops them. The stage goes on and is filled again.
- **The new host acknowledges what it installed and synced, in its marker.** The marker
  `.yulius-import-incomplete` holds the stage id, the count of streams taken in, the last
  stream's nonce, how many of its entries were installed and the stream's digest there. The old
  host asks for the marker and adopts the receipts of exactly that prefix when the digest is its
  own; it revokes the paths it sent beyond it. A mismatch of id or count drops the receipts and
  the staged copy.
- **Nothing is known while a stream is open.** The marker names a stream before its first entry
  and says that the copy's state is unknown; the stream's end takes that back. A receive that
  dies, a pass that removed paths and then broke, and one whose directories' times could not be
  put back therefore cost the receipts, never the copy's truth: the stage goes on and
  everything is sent into it again.
- **A stream whose sender is gone is closed by the next run.** A link that breaks leaves the new
  host's end open until TCP gives up. The receive reads on a thread of its own and, without
  bytes, looks for a stop file beside its lock; the old host's next look at the marker writes
  that file and waits until the stream is closed and marked.
- **A warm pass never removes.** Stale paths stay until the frozen pass, whose kept paths decide
  what is deleted; an entry that would need a directory removed waits for it. After a frozen pass
  the receipts are exactly the entries it skipped. So a receipt is always true: the new host
  holds that path in that state.
- **Links of one file travel together**: when a file with several links is sent, its other links
  are sent in the same pass, so the copy never holds two files where the old host has one.
- **The freeze is estimated, and can be bounded.** After each delta pass the checklist shows the
  estimate; `--max-downtime` makes the move copy again while it is above the bound and stop
  without freezing after 5 delta passes. What a pass sent without earning a receipt, and what
  the new host could not install, is counted in full, since the frozen pass sends it whatever
  happens.

## Alternatives

- **`rsync` for the warm passes**: rejected in ADR-0297 (size and time, no id ranges, the
  import's checks bypassed). `--checksum` would read both copies whole in every pass.
- **A set of state digests** (the first sketch): a directory swapped away, replaced and swapped
  back returns its files to their old paths with their old change times; the old digests match
  although the new host holds the files of the pass in between.
- **A receipt for anything unchanged for 2 s when it is read** (the first sketch): a file read
  between two writes through an existing mapping keeps its change time. Relying on the kernel's
  writeback interval instead of a sync would depend on tunables.
- **Hashing content on both hosts in the freeze**: exact, but it reads the whole copy frozen,
  which is what the warm copy removes.
- **All-or-nothing passes**: simplest, but a cut after 50 minutes sends everything again, and the
  paths the cut stream already replaced on the new host would still have to be revoked.
- **An idle timeout on the receiving side** instead of the stop file: a pass that finds little
  to send writes nothing for minutes while it walks, so the timeout would end healthy streams.
- **Repairing a staged copy after a receive died** (a journal of directory times on the new
  host): more state to get wrong than sending everything again in that rare case.
- **Filesystem snapshots (LVM, btrfs) or `fanotify` marks**: exact change tracking, but they need
  a volume layout or a watcher the hosts do not have, and miss what happens before the watch.
- **Deleting in warm passes**: would make receipts of removed paths false unless every removal
  were reported back; the frozen pass deletes once, from the full list.
- **Receipts that outlive a restart**: they would need a digest of each file's content and a
  second read of the tree to check it after a crash, which is the copy again.
- **`openat2` with `RESOLVE_NO_SYMLINKS` for every look of a live walk**: equally safe, but it
  resolves the whole path again for each call and needs handles where the walk has paths.
  Holding the directories open resolves one name a look.

## Consequences

- The freeze holds two counts of the tree on the old host, the changed entries, one pass over the
  copy for deletions and one for the proof, and the start. For a tree of millions of entries the
  counts are most of it; the checklist says what it expects before it freezes.
- Entries changed during a pass are sent by the next one, so the passes shrink by the ratio of
  the change rate to the copy rate; a tree that changes faster than it is copied never gets under
  a bound, and the move then stops without freezing.
- Entries on another filesystem mounted inside the data directory get no receipt and are sent in
  every pass, the frozen one included.
- The old host keeps 32 bytes per entry in its state directory and in memory during a pass. A
  stream in flight adds 40 bytes per entry it sends on disk and 4 in memory; around a frozen
  pass the list of what it left alone and the log made of it come on top, 32 bytes per entry
  each. The new host holds 16 bytes per entry in memory while it deletes.
- The receipts rest on the inode change time, and on a sync that makes the next write through
  a mapping stamp its file again: true of filesystems that write back to a disk (ext4, xfs,
  btrfs), not of tmpfs, where a data directory would not outlive a restart anyway. The 2 s
  margin covers filesystems that keep whole seconds. A clock set back is found when a pass
  begins or ends (its lead over the boot's own
  time fell by 1.5 s or more); a clock set back and forward again between two such looks is
  not.
- A restart of the old host during the copy, or a clock set back, costs one full pass into the
  staged copy that is there.
- A live walk holds one open file for each level it is deep and names its entries through
  `/proc`. Every file that is sent, by the plain export too, is opened through `/proc`.
- A cut stream's prefix is acknowledged on the stream's own digest at the last entry, not on the
  trailer, which was never sent.
- A proof that fails still fails the move and thaws the old instance. The staged copy is not
  built on: the old host drops what it knew of it, and the next run removes it and copies
  everything again.
