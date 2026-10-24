---
yulius-task: 360e1ba7-ab73-465a-acb2-c931079d7931
status: done
---

# SPEC-288: Disk upkeep reads a worktree's files without waiting on a FIFO

Status: done
Date: 2026-10-24
Owner: task #274
Scope authorization: task #274: "Solve GitHub issue #853 … in one pull request into `dev`".
Issue #853 (found by the independent review of SPEC-276, task #261). Extends SPEC-008 and
SPEC-263; follows the handle rule of ADR-0174 (#657).

## Goal

Nothing an agent puts in its sandbox stops the disk upkeep or the leftovers round: the small
files they read from a sandbox (a worktree's `yulius.toml`, a worktree record's `gitdir`) are
read through one handle that never follows a link and never waits on a FIFO, and anything other
than a small regular file counts as absent.

## Context

- Symptom (#853): `declared_caches` (`crates/yulius-app/src/disk.rs`) checks
  `<worktree>/yulius.toml` with `symlink_metadata` (regular file, at most `MAX_TOML_BYTES`), then
  reads it again by path with `std::fs::read_to_string`. An agent can swap the file for a FIFO
  between the two calls; opening a FIFO for reading blocks until a writer appears.
- Reached from `find_caches` (inside `spawn_blocking`: the round never ends, its `UpkeepMark`
  stays taken, so every later round of that sandbox is skipped: no pruning, no pressure handling)
  and from `worktree_only_caches`, which calls it straight on a tokio worker (from
  `remove_ended_worktree` and the hygiene cleanup, `ci/hygiene.rs`): a FIFO there blocks the
  worker itself. A FIFO in place before the check is refused by `is_file()`; only the swap
  races, but an agent can retry it at will.
- Same pattern (acceptance item 3): `gone_worktree` (`crates/yulius-app/src/leftovers.rs`) reads
  `.git/worktrees/<name>/gitdir` of the sandbox repository by path with `read_to_string`: no
  check at all, so a FIFO there blocks the leftovers round (and the disk round's leftovers
  check) every time, a link is followed, and the file has no size bound. The agent can write
  that directory (it is its repository's Git directory).
- Other reads checked: `root_entries` (`read_dir`, which opens with `O_DIRECTORY`, so a FIFO
  fails), `real_dir_from`/`dir_size`/`exists` (metadata only), and the Cargo sweep's
  `.cargo-lock` (`sweep.rs`, already opened with `O_NONBLOCK` and checked through its handle by
  SPEC-276). The rest of `sweep.rs`'s opens are in its tests.
- Why the tests missed it: every test wrote a regular `yulius.toml`; none put a FIFO, and the
  leftovers tests never made `gitdir` anything but a regular file.

## Requirements

- FIF-001: a worktree's `yulius.toml` is opened once with `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`;
  its handle must be a regular file of at most `MAX_TOML_BYTES` (64 KiB), and at most one byte
  more is read from it (a file grown past the limit after the check is refused). A FIFO, a link, a directory, an oversized file or anything unreadable
  declares no cache.
- FIF-002: a worktree record's `gitdir` is read the same way, bounded by `MAX_GITDIR_BYTES`
  (4 KiB, a path and its newline); anything else means the record names no gone worktree (it is
  kept, as an unreadable record was before).
- FIF-003: `worktree_only_caches` reads the declared caches on a blocking thread, not on a
  tokio worker. With FIF-001 the read cannot block, so no test can tell the two apart; this is
  checked by reading the code (defence in depth for file I/O on async workers).
- FIF-004: a disk round over a worktree whose `yulius.toml` is a FIFO ends, prunes its
  recognised caches (`target`) and keeps what only that file declared.

## Plan

- [x] Spec SPEC-288 written and validated
- [x] Failing regression tests for a FIFO, a link and an oversized yulius.toml and gitdir
- [x] yulius.toml and gitdir read through one non-blocking no-follow handle
- [x] Declared caches of an ended worktree read off the async workers
- [x] Independent review answered and gates green
- [x] Pull request into dev says Fixes #853 and #853 has the progress

## Acceptance criteria

- A test with a FIFO named `yulius.toml` in a worktree ends `disk_upkeep` within a bound and
  treats the worktree as having no declared caches (FIF-001, FIF-004).
- A link or an oversized `yulius.toml` still declares nothing; a regular one still does (FIF-001).
- A FIFO, a link or an oversized `gitdir` names no gone worktree; a regular one still does
  (FIF-002).
- The ended-worktree path (`worktree_only_caches`) ends within the bound with a FIFO in place
  (FIF-003, FIF-004).

## Test plan

- Unit (`disk.rs`): `declared_caches` on a regular file, a link to a valid file, a file one byte
  over the limit, a directory and a FIFO, each on its own thread answering within a bound
  (`recv_timeout`; a blocked thread is left behind and the test fails).
- Unit (`disk.rs`), the race of #853: a thread swaps `yulius.toml` between a regular file and a
  FIFO (`rename`) while another reads it 20 000 times; the reads must end within 60 s. A FIFO
  already in place was refused before the fix too; only the swap after the type check blocked,
  so this is the test that fails without the fix.
- Unit (`leftovers.rs`): `gone_worktree` with `gitdir` as a regular file naming a gone worktree
  (found), a link to such a file, an oversized file and a FIFO (none), each within a bound.
- Integration (`crates/yulius/tests/disk.rs`): a crashed task (the `find_caches` path) and a
  closed task (the `worktree_only_caches` path) whose `yulius.toml` is a FIFO; `disk_upkeep`
  ends within 60 s, `target` goes and the declared `out` stays. On the timeout the test opens the
  FIFOs' write ends to release a blocked reader, so a regression fails instead of hanging the
  runtime's shutdown. It guards FIF-004 end to end; it is not the race's regression test.

## Decisions

- One helper, `read_small_file(path, max)` in `leftovers.rs` (beside `no_link_between`, which
  `disk.rs` already uses), serves both reads, instead of two copies of the open-and-fstat code.
  It reads at most `max + 1` bytes through `take`, so a file growing after the `fstat` is still
  refused. No new ADR: this applies ADR-0174's rule (the handle, not the path, is checked and
  read) to two more reads.
- The unit tests bound each read with a thread and `recv_timeout` (a blocked thread is left
  behind; the process still ends). The integration test runs the round as a spawned task on a
  multi-threaded runtime under `tokio::time::timeout` and, on the timeout, opens the FIFOs' write
  ends: a reader blocked in `spawn_blocking` would otherwise hold the runtime's shutdown.
- Links higher up the path (a worktree directory itself swapped for a link) are outside this
  change: `O_NOFOLLOW` covers the last component only, as before; the removal still checks the
  worktree with `real_dir_from`, and a link there can at most change the list of cache names read.

## Evidence

- Before the fix (commit `de64f2ab`, tests only):
  `cargo nextest run -p yulius-app -E 'test(/yulius_toml|gitdir/)'` failed 2 of 4:
  `a_yulius_toml_swapped_for_a_fifo_is_never_waited_on` (60.0 s, "a read waited on the FIFO")
  and `only_a_small_regular_gitdir_names_a_gone_worktree` ("a link": the old read followed it).
  The integration test passed there too (a FIFO in place was refused by the old type check).
- After the fix: the same run, plus `disk::`/`leftovers::` unit tests, 12 passed; the race test
  takes 0.33 s for 20 000 reads. `disk::`, `leftovers::` and `hygiene` integration tests: 58
  passed.
- Mutation (removing `O_NONBLOCK` from `read_small_file`): the integration test fails after 60 s
  with "the round waited on a FIFO" and does not hang; restored afterwards.
- `scripts/dev ci` at `2d90883b`: exit 0, 2576 tests passed, 17 skipped.
- `scripts/dev security`: exit 2, not run: Vidoc is out of quota for this key (HTTP 429, "has
  used its 50,000,000 tokens"); unrun, not passed. No ignore entry was added.
- Independent review (fresh session, `om-code-review` and `yulius-review`) at `09ab6977`:
  APPROVE, 4 minor and 2 nits, all on this change's own code. Answered in `2d90883b`: the
  integration test releases a blocked reader on timeout (1); FIF-003 is checked by reading the
  code, and the spec says the integration test guards FIF-004 rather than the race (2); Plan and
  Evidence (3); a directory case (4); FIF-001 wording (5). Links higher up the path (6) are
  pre-existing and unchanged; see Decisions.
- Acceptance: FIFO `yulius.toml` ends `disk_upkeep` with no declared caches
  (`a_fifo_named_yulius_toml_declares_nothing_and_never_stops_the_round`); a link, an oversized
  file and a directory still declare nothing (`only_a_small_regular_yulius_toml_declares_caches`);
  other reads checked (Context), `gitdir` fixed (`only_a_small_regular_gitdir_names_a_gone_worktree`).
