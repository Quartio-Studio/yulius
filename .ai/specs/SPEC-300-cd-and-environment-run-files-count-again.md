---
yulius-task: 657ff438-a1c7-4805-bc78-74bbf58c3f4e
status: done
---

# SPEC-300: CD and environment run files count against a disk bound

Status: done
Date: 2026-10-24
Owner: task #286
Scope authorization: issue #664 (found in the review of task #205, SPEC-219), verified on `dev`
at e8cb3398 by task #283.

## Goal

A CD run (SPEC-134) or a delivery environment run (SPEC-219) cannot fill the data disk with its
files: what it writes into its own directory and its cache is measured while it runs, and a run
past its bound is stopped and recorded `error`, saying why.

## Context

- Symptom (#664): a run writes its commit's tree, `HOME` and log into its own directory
  (`<data>/runs/cd-<run>`, `<data>/runs/env-<run>`, or for production
  `<data>/cd/<sandbox>/runs/<name>`), and a CD run also writes its persistent cache
  `<data>/cd/<sandbox>/cache/<env>`. Those paths are bound writable into the run's containment,
  but they lie outside the sandbox directory, so the sandbox's measured usage
  (`yulius-sandbox/src/manager.rs`, sandbox dir, system layer, Docker storage) never sees them.
  The only bound while a run executes is its log (`MAX_LOG_FILE_BYTES`, 256 MiB) in the shared
  `ci_watch` tick (`ci/exec.rs`). A deploy command can write until the job timeout (30 min).
- Root cause: `ci_watch`, which CD runs (`cd.rs` `cd_process`) and environment runs
  (`environment.rs` `environment_launch`) share, bounds time, cancellation and output, but not the
  files of the run; CI-303 (SPEC-059) only refuses a CI job at start, over the sandbox's quota.
- Why tests missed it: no test writes files (only output) during a run.
- What else the cause could break: CI jobs write into the sandbox directory, which the quota
  measures, so they are not affected.

## Requirements

- RDB-001: while a CD run or an environment run executes, Yulius measures its files (the run's
  files directory, and for a CD run its cache) about every 5 s, off the watch loop, with the
  application's bounded `crate::disk::dir_size` (allocated blocks, links not followed, at most
  4 M entries; the application layer cannot use the sandbox adapter's `usage::measure`).
- RDB-002: a run whose files use more than the bound (`--cd-run-disk-gb`, default 20 GB, one for
  every run of the instance) is killed with its process group and recorded `error` with the
  reason "the run's files used X (run files A, cache B), past the Y disk bound for a run; it was
  stopped". An environment run records that reason in its output, as its other errors.
- RDB-003: a CD run stopped over the bound has its cache emptied, so the next run starts under
  it (a cache is rebuildable; out of agents' reach, nobody else could empty it): through handles
  that never follow a link and make read-only folders writable, else as the sandbox's mapped root
  for files a production run's containers wrote. The reason ends "; its cache was emptied", or
  says it could not be emptied and why.
- RDB-004: the bound is documented with the CD contract (SPEC-134 CDR-004) and in
  `docs/operations.md`.

## Plan

- [x] Spec, ADR-0216 and failing tests: a CD run and an environment run writing past the bound
- [x] Runs measured during ci_watch and stopped past the bound, recorded error
- [x] Configurable bound (--cd-run-disk-gb), cache emptied on a stop, docs (SPEC-134, operations, architecture, changes)
- [x] Independent review answered, gates green, merged into dev

## Acceptance criteria

- A CD run whose deploy writes past the bound into its cache is stopped before its timeout,
  recorded `error` with the reason, and its cache is empty afterwards; the next run passes.
- An environment run whose command writes past the bound is recorded `error` with the reason.
- Both tests fail on `dev` (the runs time out or pass instead).
- `scripts/dev ci` passes.

## Test plan

- `crates/yulius/tests/cd.rs`: `a_run_writing_past_its_disk_bound_is_stopped` (bound 8 MB, the
  deploy writes 32 MB into `$YULIUS_CD_CACHE` then sleeps).
- `crates/yulius/tests/delivery.rs`: `an_environment_run_writing_past_its_disk_bound_is_stopped`.
- Unit test of the reason text (`a_disk_bound_reason_names_each_directory_and_the_bound`).
- `yulius-sandbox` unit test: a CD cache is removed only by the walk as mapped root
  (`only_trees_below_sandboxes_skip_the_by_path_removal`).

## Decisions

- A fixed bound per run, not the sandbox's quota (ADR-0216): a production deploy must not die
  because agents filled their worktrees, measuring the whole sandbox every few seconds is too
  costly, and a run with containment off has no quota at all.
- The cache counts toward the run's bound and is emptied when a run is stopped over it.
- Measured every 10 ticks (5 s) in a blocking task polled from the watch loop: cancellation and
  the log keep being served while a large tree is walked. The bound is soft: a run can write for
  up to about 10 s plus a walk past it before it is stopped, and a run hiding files (a directory
  it made unreadable, over 4 M entries) is undercounted, as the sandbox quota is.
- A production run's files written by mapped ids are measured as the server can read them; an
  unreadable directory makes the measurement a lower bound (as for the sandbox quota).

## Evidence

- Red on `dev` (e8cb3398 plus the tests, commit ce3fe385):
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/a_run_writing_past_its_disk_bound/) | test(/an_environment_run_writing_past_its_disk_bound/))'`
  → both FAILED: the CD run ended "timed out after 20 s", the environment run `failed` with "the
  command exceeded 0 minutes" (its 20 s timeout).
- Green with the fix: the same command → 2 passed (5.4 s and 6.8 s).
- Review round 1 (major: cache emptying could fail silently on read-only folders or mapped-id
  files): `only_trees_below_sandboxes_skip_the_by_path_removal` red before ("cd/s1/cache/prod"),
  green after; `cd::*`, delivery environment tests, `leftovers::*`, `yulius-app ci::*`: 73 passed.
- Not testable here: the CD test's `chmod 555` cache folders cannot resist removal as root (tests
  run as uid 0 in this sandbox), and the "its cache could not be emptied" reason has no test, since
  this harness cannot make a cache resist the handle removal as root. The handle removal of
  read-only trees is covered by the existing `leftovers` tests (#680).
- `scripts/dev ci` at 0641272a: exit 0; nextest 2603 passed, 16 skipped; fmt, clippy, deps,
  macOS compile and docs passed.
- `scripts/dev security`: exit 2, not run: Vidoc answered HTTP 429 ("This API key has used its
  50,000,000 tokens"), so the 8 changed code files were not scanned. Unrun, not passed.
- Independent review (fresh session, read-only): round 1 request changes (1 major, 3 minor, 3
  nits; all answered), round 2 approve with 2 minors and 2 nits (fixed: random fill data for
  compressing filesystems, by-path removal only without `/proc`, removal off the runtime), round
  3 approve at 0641272a; the untested failure text is accepted as the limit above.
- Pre-existing, fixed on the way: run directories with read-only folders (a Go module cache in
  `HOME`) were never removed by `remove_run_dirs`; they now go through the same handle removal.
