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
  and the attached bundle exist at once), or three times the cap when GitHub reports no size. The
  check is the one uploads use (`App::disk_room_if_short`, called by `check_declared`), shared, not
  copied. An unknown disk refuses nothing.
- **RFB-003** The Git fetch and the bundle it writes are each bounded by `MAX_REFERENCE_BYTES`: a
  command whose folder or file (for the bundle, also Git's `<path>.lock`, which it renames at the
  end) grows past the limit, counted by the disk blocks a file takes when more than its length,
  has its process group killed and reaped, and
  the call fails with a typed error (`GitError::TooLarge`). The tool answers that the repository
  is larger than the cap, and the fetch folder is removed as on every other end.

## Plan

- [x] Spec and ADR for a bounded reference fetch
- [x] GitHub's reported size refuses an oversized repository before any fetch
- [x] A full data disk refuses a fetch, sharing the uploads' check
- [x] Git kills a command whose folder grows past a byte limit
- [x] A reference fetch past the cap is stopped and its folder removed
- [x] Docs, changes and every gate green
- [x] Independent review answered and fixed
- [ ] Round 2 review approves the final head with the gate green

## Acceptance criteria

- **AC1 (RFB-001)** GitHub reports `acme/lib` at 2 GiB: `reference_repo` is refused naming the
  size and the 1 GiB cap; no Git call ran and `<data>/tmp` holds no fetch folder.
- **AC2 (RFB-002)** The data disk is 96 of 100 GB used: the tool is refused saying the disk has
  no room, before any Git call; at 92 of 100 GB a repository GitHub gives no size for is refused
  (three times the cap); a 10 GB disk at 80 % used refuses a repository GitHub reports at 600 MB
  (3 × 600 MB would pass 95 %) and fetches one reported at 200 MB.
- **AC3 (RFB-003)** A fetch whose work folder passes the limit is killed: the real Git runner
  stops a command writing past a byte limit with `GitError::TooLarge` and leaves no process; the
  tool, given a smaller limit by the fake, answers that the repository is larger than the cap and
  `<data>/tmp` is empty afterwards.

## Test plan

- `crates/yulius-github`: the wire type parses `size`; a missing one is `None`.
- `crates/yulius/tests/reference_repo.rs`: AC1 (the GitHub fake's size knob), AC2 (the harness's
  `SettableSystem` disk), AC3 at the tool level (the fake `bundle_repository` honours the limit it
  is given by failing past it, as the adapter does).
- `crates/yulius-git/tests/git_ops.rs`: a script standing in for Git that grows its folder (or the
  bundle's `.lock`) past a small limit and sleeps is killed well before its sleep with `TooLarge`,
  its process group gone; one that ends past the limit fails and one under it succeeds; a real
  bundle past its limit fails. A unit test covers the folder walk (links not followed).
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

## Review

Round 1 (fresh session, read-only, at a3875c39): request changes. Every finding is own code.

- Medium: the bundle watch measured `<path>`, but Git writes `<path>.lock` and renames it at the
  end, so it only checked the finished bundle. Confirmed with Git 2.43 (`out.bundle.lock` while it
  runs). Fixed test-first: `a_bundle_growing_past_its_limit_is_killed_while_written` (30 s
  without the fix, killed at once with it); the watch takes several paths and the bundle adds
  its lock file.
- Low: files counted by length, not by disk use (a loose ref takes a 4 KiB block). Fixed: a file
  counts as the larger of its length and its blocks (`taken`, unit test).
- Low: the tool reads any capacity error of `bundle_repository` as "too large". Kept (the
  adapter's only capacity error is `GitError::TooLarge`), now commented at both ends.
- Low: "1.0 GB, above the 1 GB". Fixed test-first: `above_cap` rounds up to a tenth
  (`a_size_above_the_cap_is_rounded_up`).
- Low: GitHub's size counts pull request refs, so a repository just above it may be refused
  though its bundle would fit. Accepted, in ADR-0236's consequences.
- Low: the disk check reserves nothing, and an unknown size counted as 0. Unknown now counts the
  cap (test-first, AC2's 92 % case); no reservation, recorded in ADR-0236's consequences.
- Nits: the spec's shared-check name, AC2 and the test plan now match the code and tests; the
  stopped-fetch answer says "Yulius stopped Git when its copy grew", true for both steps.

