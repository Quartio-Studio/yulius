---
yulius-task: 7883fa00-dceb-4dac-a7b9-8bf425e35271
status: in-progress
---

# SPEC-465: Run migration: warm copy while the instance runs, a freeze that sends only the changes

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #445 (slice B of #442)
Scope authorization: task #445, asked by the project's owner: "Make the freeze of `yulius instance
migrate` short: copy the data while the old instance still runs, then freeze and send only what
changed." It implements SPEC-460's requirement MIG-032.

## Goal

`sudo yulius instance migrate root@new-host` copies the data directory while the old instance
runs, in one or more passes into a staged copy on the new host, and freezes the old instance only
for the entries that changed since they were last sent, the removal of what no longer exists, the
proof (MIG-031) and the start. The estimated freeze is shown before it begins, `--max-downtime`
bounds it, and a pass that was cut is continued by the same command.

## Context

- SPEC-460 (task #442, ADR-0297) delivered the command with a frozen full copy. Production's move
  on 2026-11-11 was frozen for 54 minutes (275.8 GiB, 6,388,083 entries, bound by the old disk's
  IOPS on small files). MIG-032 names the warm copy as slice B.
- The transport is the export's own archive (SPEC-329, ADR-0222): `walk.rs` (archive order,
  hard links to their first path), `archive.rs` (pax stream with a SHA-256 trailer), `export.rs`
  (plan the exact length, then stream), `extract.rs` (O_NOFOLLOW handles, id remapping),
  `import.rs` (checks, the marker `.yulius-import-incomplete`). `rsync` stays rejected
  (ADR-0297): its quick check compares size and time.
- The move's proof is the inventory (`inventory.rs`): entries, bytes and a fingerprint of every
  entry's headers per top-level folder, counted on both hosts. It compares metadata, not content,
  so a warm copy must never skip an entry whose content the new host does not hold.
- Task #442 sketched the design the request repeats: receipts on the old host (a digest of an
  entry's archive headers, its inode change time and number), bound to a stage id in the new
  host's marker. This spec uses it and changes two points, each for a case the sketch misses
  (Decisions): receipts are a map from a path to its last state, and a receipt needs a change time
  older than the pass's own start.
- Since #442's commit `2866a890` the sandbox base is built while preparing (`Part::BeforeStart`),
  before anything is copied or frozen; the request's "after the first pass" was written before
  that commit.
- Tasks #446, #447 and #450 change `migrate.rs` and its tests in parallel.

## Requirements

Passes

- WCP-001 **Warm passes.** After Prepare the move enters the phase `Copy`: it streams the data
  directory to the new host while the old instance runs. The first pass of a staged copy sends
  every entry; a later pass sends only entries the new host does not hold in their current state.
  A run always ends its warm passes with a pass that began with receipts (a delta pass), so the
  freeze never follows a full pass directly.
- WCP-002 **Live reads.** A warm pass walks once. A path that vanished is left out (with what is
  below it). A file's header comes from the open file's own stat, exactly that many bytes follow,
  padded with zeros when the file shrank while it was read. What replaced a file between the walk
  and the open (a link, a FIFO, a device) is never opened blocking or followed. The manifest says
  the pass was read live; its totals are estimates, so the importer checks the stream's checksum
  and not its length.
- WCP-003 **Receipts.** The old host keeps, for every path it sent, the state the new host holds:
  a 16-byte digest of the entry's archive headers, its inode's change time in nanoseconds and its
  inode number. A pass skips an entry only when the receipt of its path is that digest. An entry
  earns a receipt only when (a) its stat was the same before and after it was read, (b) its change
  time is at least 2 s older than the start of the pass, which begins by syncing the data
  directory's filesystem (so a later write through a mapping is stamped too), (c) it lies on that
  filesystem, and (d) the new host acknowledged it after syncing its own filesystem. Sending a
  path revokes the receipt it had. When a file with several links is sent, every other link of it
  is sent in the same pass.
- WCP-004 **Changes are never found by size and time.** A file rewritten in place within the same
  second, a file whose modification time was set back, and a path that holds another file again
  after its directory was swapped away and back are sent again.
- WCP-005 **The staged copy.** The new host's marker `.yulius-import-incomplete` holds the stage
  id, the count of acknowledged passes and what the last stream installed. A pass is accepted only
  into an empty data directory (it creates the stage) or into the stage whose id and pass count it
  names. A marker without that stage id is an unfinished copy and is removed before the first
  pass, as today. `Request::Mark` removes the marker once the copy is proven, so a copy that was
  started is never a staged copy.
- WCP-006 **Binding.** Receipts live in `<state>/copy/` on the old host with the stage id and the
  count of acknowledged passes. Before every pass the old host asks the new host for its marker:
  equal id and count continue; a count one ahead with the pending stream's nonce is adopted
  (WCP-011); anything else (no marker, another id, another count) drops the receipts and the
  staged copy, and the next pass is a full one.

The frozen pass

- WCP-007 **Exact delta.** The frozen pass is planned and streamed as today (exact length, the
  stream must match the plan, checksum and length checked by the importer). It sends the entries
  without a matching receipt and then the digests of every path the old host holds.
- WCP-008 **Deletions.** After the checksum matched, the new host removes everything in the staged
  copy whose path is not among those digests, except the names a copy never carries
  (`walk::LEFT_OUT`) and its marker. A warm pass removes nothing.
- WCP-009 **Extraction onto a staged copy.** An entry replaces what is at its path: a file by a
  directory, and in the frozen pass a directory (with everything below) by a file, link or
  whiteout. In a warm pass an entry that would need a directory removed is not installed and gets
  no receipt (it waits for the frozen pass). A hard link may name a file of an earlier pass. A
  directory that is sent again loses the restorable attributes the entry no longer carries. A
  directory that is not sent keeps its time when an entry below it is written or removed.
- WCP-010 **Proof.** MIG-031's proof runs over the whole staged copy after the frozen pass,
  unchanged: the new instance starts only when both inventories are equal.

Resume and bound

- WCP-011 **Resumable.** A pass cut by a broken link or a killed command leaves the staged copy
  and the receipts usable: the new host syncs and records how many entries of the stream it
  installed, with the stream's nonce and the stream's digest at that entry; the old host adopts
  the receipts of exactly those entries when the digest matches its own record, and revokes the
  receipts of the paths it sent after them. Running the same command again continues: no
  acknowledged entry is sent again. A pending record written before the old host was restarted is
  not adopted.
- WCP-012 **Estimate.** After each delta pass the checklist shows the estimated freeze (item
  `downtime`) before anything freezes: four counts of the tree at the measured rate (two on each
  host), the time to send what will have changed since the pass began (the last pass's sending
  time, scaled by how long changes gathered for it), and the start. The plan shows what is known
  then: the first pass's size and rate, and the bound.
- WCP-013 **`--max-downtime <minutes>`.** While the estimate is above the bound the move copies
  again instead of freezing; after 5 delta passes in a run it stops without freezing, says so in
  a failed line (exit 1) and leaves the staged copy for the next run. Without the option it
  copies again while the changes alone would take more than 60 s to send (same limit of passes),
  then freezes.
- WCP-014 **Before the freeze.** The sandbox base is built and the new host proven ready while
  preparing, before the first pass (as since SPEC-460's `2866a890`); the freeze holds the delta,
  the deletions, the proof and the start.

## Design

- **Stream.** A pass is an export stream whose manifest carries `pass`: the stage id, the pass's
  number (the count of acknowledged passes it builds on), a nonce, whether it was read live and,
  for the frozen pass, the count of kept paths. A frozen pass carries the entry
  `yulius-export.kept` (16 bytes per path: the first half of the SHA-256 of its archive path)
  after the last data entry. `yulius instance import` refuses a stream with `pass`.
- **Receipts** (`instance/receipts.rs`). `<state>/copy/stage.json` holds the stage id, the count,
  the valid length of the log and the pending stream; `receipts` is a log of 32-byte records
  (path digest, state digest; a zero state revokes); `pending` is the journal of the stream in
  flight, 40 bytes per sent entry in stream order (the stream's digest before the entry, path,
  state), written before the entry is sent and completed after it; `pending.kept` lists what a
  frozen pass left alone. A frozen pass that completed replaces the log by `pending.kept`: after
  its deletions the receipts are exactly the entries that were skipped.
- **Acknowledgment.** The importer counts installed entries and remembers the stream's digest at
  the last two entry boundaries. At the end of a stream, complete or not, it sets the directories'
  times, syncs the filesystem and rewrites the marker atomically. The old host never trusts the
  reply alone: it asks for the marker (`Request::Stage`, which waits for a receive still running
  there) and reconciles.
- **Flow.** Plan (the marker is read with the facts, receipts are reconciled) → Prepare (the
  data directory keeps this move's stage) → Copy (warm passes, estimate, bound) → Cutover (freeze,
  frozen pass, proof, mark, start) → Finish. A signal during the warm passes ends the command;
  nothing is frozen yet.
- **States.** A staged copy is `staged` (owner: the move; exit: proven and marked, or dropped on a
  mismatch); a pass is `pending` on the old host until the marker names it, then `acknowledged`
  or `revoked`.

## Plan

- [x] Spec SPEC-465 and ADR-0303 for the warm copy
- [x] Receipts on the old host: the map of what the new host holds, the journal of a stream, adoption
- [x] Passes in the export: a live walk that tolerates change, an exact delta with the kept paths
- [ ] Extraction onto a staged copy: replace, link to earlier files, keep directory times, defer or delete
- [ ] The import of a pass: stage marker, acknowledged prefix, deletions, sync
- [ ] The move: warm passes, estimate and --max-downtime, the frozen delta, resume
- [ ] Operations guide §7, architecture, product summary, changes, SPEC-460's pointer
- [ ] A real move between two hosts (QEMU guests) with changes during the copy
- [ ] Independent review answered, gates green

## Acceptance criteria

- Between two fake hosts with the real export, import and inventory: a move with files changed,
  added, deleted and replaced (file to directory and back, a broken hard link) between the passes
  ends with exit 0, both inventories equal, the copy's content equal to the old tree, and the
  events in this order: `before_start` < first pass < `downtime` line < freeze < frozen pass <
  proof < mark < start.
- A file whose content changes with the same size and the same modification time is sent again;
  an unchanged file is not (the second pass's stream does not hold it).
- A directory swapped away and back, whose path held another file in the pass between, is sent
  again.
- A file that shrinks while a warm pass reads it is installed padded, gets no receipt and is sent
  again; a path that vanishes during the walk does not fail the pass.
- A pass cut after some entries: the next run sends none of the acknowledged ones again and
  completes; with the marker removed on the new host the next pass is a full one into a new stage.
- With `--max-downtime` below the estimate the move copies again, then ends with exit 1 and a
  failed `downtime` line; the old unit was never frozen and the staged copy is kept. With a bound
  above the estimate it freezes after the first delta pass.
- After a rolled-back move (the new instance did not come up) the copy on the new host has no
  marker and the next run refuses it, as today.
- A plain `yulius instance export | import` is unchanged (its tests pass untouched), and `import`
  refuses a pass.

## Test plan

- Unit tests: `instance::receipts` (state digest, held, revoke, adoption of a prefix, a foreign
  nonce, another boot, a completed frozen pass replacing the log, reload from disk);
  `instance::walk` (change time and inode seen, a vanished path tolerated, the first path of a
  link forgotten); `instance::archive` (the kept entry, digests at entry boundaries, a padded
  live file); `instance::export` (each case of WCP-002..004 and WCP-007 against an in-memory
  ledger); `instance::extract` (each case of WCP-009, the deletions of WCP-008);
  `instance::import` (stage marker, refusal of another stage or count, a cut stream's
  acknowledgment, the plain import refusing a pass).
- Flow tests (`instance::migrate::tests::flow`): the acceptance criteria, with the existing moves
  updated to the new order of events.
- CLI test: `--max-downtime` is parsed and refused when 0.
- Gates: `scripts/dev ci`; `yulius security-scan` (Vidoc is off for this project: disabled, not
  passed).
- Real hosts: two QEMU guests as in SPEC-460's Evidence, with files written during the passes.
  Not testable here: production's size (the walk rate at 6.4 million entries is measured on a
  synthetic tree), a link that breaks by itself, two physical machines.

## Decisions

- ADR-0303 records the lasting ones: the warm copy extends the export's stream instead of adding
  another transport; receipts are a map from path to state and are granted only for entries older
  than the pass; a cut stream is acknowledged by its installed prefix; a warm pass never removes;
  the frozen pass carries the kept paths.
- **A map, not a set of digests** (differs from #442's sketch). With a set, a directory that is
  swapped away, replaced and swapped back returns its files to their old paths with their old
  change times: the old digests match although the new host now holds the files of the pass in
  between. Keyed by path, a receipt is revoked when its path is sent again.
- **Older than the pass, not than the read** (differs from the sketch's "2 s before it was
  read"). A write through a shared mapping stamps the inode only at the first write to a clean
  page, so a file read between two such writes keeps its change time. The pass syncs the
  filesystem first, which write-protects every mapped page; a write after that is stamped after
  the pass's start, and the receipt's rule sees it. The price is that entries changed during a
  pass are sent again by the next one.
- **Two walks while frozen, as today.** One walk that keeps the delta in memory would save one
  count of the tree, but needs a second code path when the delta is large. Left for a follow-up
  if a measured freeze asks for it.
- **The base stays in Prepare** (WCP-014): a host that cannot run sandboxes fails before hours of
  copying, as SPEC-460's acceptance criterion asks.
- **A bound that cannot be met fails** (exit 1) rather than ending as `manual`: a script that
  asked for a bounded move must not read exit 0 as "moved".
- **One pull request.** The change is one concern (MIG-032) whose parts are not useful alone; it
  is built in the plan's slices, each with its tests.
