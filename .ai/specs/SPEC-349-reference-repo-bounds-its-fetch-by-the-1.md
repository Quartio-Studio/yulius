---
yulius-task: 89dccf9b-32bb-4e96-84cb-9ad2093c2c9d
status: in-progress
---

# SPEC-349: reference_repo bounds its fetch by the 1 GiB cap and the disk level

Status: in progress
Date: 2026-10-30
Owner: Yulius task #333
Scope authorization: task #333, fix GitHub issue #920 (found by task #314's review, SPEC-330).

## Goal

`reference_repo` (SPEC-267, SPEC-330) never lets the server's temporary copy of another repository
grow past the 1 GiB cap a task gets (`MAX_REFERENCE_BYTES`), and never starts a fetch the data disk
has no room for (SPEC-276's critical level), so one large repository cannot fill the disk that
also holds the database.

## Context

- Symptom (#920): `App::fetch_reference` (`crates/yulius-app/src/reference_repo.rs`) checks the
  bundle's size against `MAX_REFERENCE_BYTES` only after `bundle_repository` has fetched every
  branch and tag into `<data>/tmp/reference-*/repo.git` and written the bundle beside it. A
  repository of tens of GiB fills the data disk for the length of the fetch (bounded only by Git's
  30-minute network timeout), then is refused and removed.
- Root cause: nothing bounds the fetch. `GhRepo` carries no size, so no check runs before it; the
  Git runner (`crates/yulius-git/src/runner.rs`) bounds a command by time and captured output, not
  by what it writes to disk; and the disk-level guard of uploads (`App::check_declared`,
  `uploads.rs`) is not called here.
- Why tests missed it: every reference test fetches a small fixture, and the fake
  `bundle_repository` (`tests/support/repos.rs`) has no size or disk knob.
- What exists to reuse: GitHub's repository API reports `size` (KiB of the repository on
  GitHub); `App::data_disk` and `Level::of` (SPEC-276); the runner's poll loop, which already
  kills the process group on a timeout or an output overflow.

## Requirements

- **RFB-001** A repository whose size GitHub reports above `MAX_REFERENCE_BYTES` is refused
  before any concurrency slot, folder or Git command, and the answer says the size and the cap.
- **RFB-002** A fetch is refused before it starts when the data disk would reach its critical
  level with the fetch's peak use added: three times GitHub's size (the fetched copy, its bundle
  and the attached bundle exist at once), or nothing when GitHub reports no size. The check is the
  one uploads use (`App::check_declared`), shared, not copied. An unknown disk refuses nothing.
- **RFB-003** The Git fetch and the bundle it writes are each bounded by `MAX_REFERENCE_BYTES`: a
  command whose folder or file grows past the limit has its process group killed and reaped, and
  the call fails with a typed error (`GitError::TooLarge`). The tool answers that the repository
  is larger than the cap, and the fetch folder is removed as on every other end.

## Plan

- [x] Spec and ADR for a bounded reference fetch
- [ ] GitHub's reported size refuses an oversized repository before any fetch
- [ ] A full data disk refuses a fetch, sharing the uploads' check
- [ ] Git kills a command whose folder grows past a byte limit
- [ ] A reference fetch past the cap is stopped and its folder removed
- [ ] Docs, changes and every gate green
- [ ] Independent review answered and fixed
- [ ] Merged into dev on green CI

## Acceptance criteria

- **AC1 (RFB-001)** GitHub reports `acme/lib` at 2 GiB: `reference_repo` is refused naming the
  size and the 1 GiB cap; no Git call ran and `<data>/tmp` holds no fetch folder.
- **AC2 (RFB-002)** The data disk is 96 of 100 GB used: the tool is refused saying the disk has
  no room, before any Git call; at 50 of 100 GB the same call succeeds; a 10 GB disk at 80 % used
  refuses a repository GitHub reports at 600 MB (3 × 600 MB would pass 95 %).
- **AC3 (RFB-003)** A fetch whose work folder passes the limit is killed: the real Git runner
  stops a command writing past a byte limit with `GitError::TooLarge` and leaves no process; the
  tool, given a smaller limit by the fake, answers that the repository is larger than the cap and
  `<data>/tmp` is empty afterwards.

## Test plan

- `crates/yulius-github`: the wire type parses `size`; a missing one is `None`.
- `crates/yulius/tests/reference_repo.rs`: AC1 (the GitHub fake's size knob), AC2 (the harness's
  `SettableSystem` disk), AC3 at the tool level (the fake `bundle_repository` honours the limit it
  is given by failing past it, as the adapter does).
- `crates/yulius-git/tests/git_ops.rs`: a fetch from a local repository larger than a small limit
  fails with `TooLarge` and the folder stops growing; one under the limit succeeds.
- `crates/yulius/tests/reference_repo.rs` `the_git_adapter_bundles_every_branch_and_tag`: the real
  adapter with a tiny limit refuses the fixture.

## Decisions

- [ADR-0236](../../docs/decisions/0236-a-reference-fetch-is-bounded-by-the-cap-while-it-runs.md):
  GitHub's size refuses up front, the shared disk check refuses a fetch the disk has no room for,
  and the runner watches the fetch's folder and the bundle's file and kills Git past the cap.
- GitHub's `size` is an estimate GitHub updates lazily (a new or freshly pushed repository may
  report 0 or less than it holds), so it only refuses up front; the watcher is what bounds the
  disk.
- The watcher measures every 250 ms, not at the runner's 1-20 ms poll: walking a folder costs a
  few system calls per file. At 100 MB/s the copy may pass the cap by about 25 MB before the
  kill, which the critical level's 5 % margin absorbs.
