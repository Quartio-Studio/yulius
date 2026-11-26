---
yulius-task: 7341e60c-41be-421d-b9c0-a62785782ad0
status: in-progress
---

# SPEC-671: Cleanup of the wiring, the main.rs arguments and the self-update overlap (audit L6)

Status: in-progress
Date: 2026-11-26
Owner: Yulius task #655
Scope authorization: the repository owner's cleanup chore of 2026-11-25 (audit L6, findings F4,
F5 and F7, under the cleanup charter). The charter asks for no spec; this one exists only because
Yulius's delivery needs one, and records nothing beyond the task.

## Goal

The same behaviour in less code: the findings F4 (wiring), F5 (subcommand arguments declared once)
and F7 (the self-update overlap, only where update and rollback stay identical) of the L6 audit,
one pull request each.

## Context

The audit report L6 "composition and operations" of 2026-11-25 names the copies: a dead
`wiring/github.rs`, test-only adapters in `wiring/pending.rs`, a hand-rolled base64, seven copies
of the reqwest client setup and eight bounded body reads, two npm tarball installers, four
wall-clock readers, and `main.rs` rebuilding every option struct field by field.

## Requirements

- R1: Behaviour stays the same; every existing test passes unchanged.
- R2: A new helper only where it replaces at least two copies.
- R3: F7 changes nothing observable in an update or a rollback; otherwise it is reported, not done.

## Plan

- [x] F4: delete dead github.rs and move pending.rs to tests
- [x] F4: base64 crate and Timestamp::now()
- [x] F4: one HTTP client builder and one bounded reader
- [x] F4: one shared npm tarball module
- [x] F4: independent review answered
- [x] F5: clap Args on the option structs in main.rs
- [x] F5: independent review answered
- [x] F7: compare the update helper and staging CD install/rollback and the two freezes
- [x] F7: simplify where behaviour stays identical, or report
- [ ] F7: independent review answered

## Acceptance criteria

- The `yulius` test binary and the touched crates' unit tests pass with no assertion changed.
- `git diff --shortstat origin/dev` shows fewer product lines for each pull request.

## Test plan

Run the whole `yulius` test binary and the touched crates' unit tests; clippy and `cargo fmt
--check`; Yulius CI on each head.

## Decisions

- The wall-clock reader is `wiring::clock::now()`, not `Timestamp::now()` in yulius-domain: RUST-005
  keeps clocks out of the domain crate.
- F7 unifies nothing across the update helper and staging CD, nor across the two freezes: every
  unification would change an observable step (R3). The differences are listed under Evidence.

## Evidence

- F4, PR #1776, merged into dev at `205e16d88`: product Rust −168 lines (+112 moved to the tests).
  `scripts/cargo nextest run -p yulius`: 3537 passed, 26 skipped. Review approved `9da9799d8`; its
  minor (no test held the git token header) was fixed with `the_token_header_is_padded_standard_base64`.
- F5, PR #1787, merged into dev at `14d8a89ae`: product Rust −247 lines, `main.rs` 1,230 → 841.
  Every `--help` page (66) byte-identical to dev's binary, and 37 refused or early-ending
  invocations identical in exit code, stdout and stderr (the reviewer added 95 more: identical).
  `scripts/cargo nextest run -p yulius`: 3539 passed, 26 skipped. Review approved `95ea82deb`;
  its minor (`Options::action()` untested) was fixed in the parse test.
- F7: the update helper (`update_helper.rs`, production) and staging CD (`scripts/cd/remote.sh`)
  differ in what a person or the server sees at these steps:
  1. binary: built from source on the host with root's cargo and ancestor checks, against built in
     Docker, uploaded and checked by SHA-256;
  2. before the stop: the restart guard (sessions in the unit's cgroup), against a migration check
     on a copy of the live store with per-table row counts;
  3. what is kept: one backup of the binary and the database with its WAL per request
     (`KEEP_BACKUPS`), against each release's binary and the store it left, the last 10 kept;
  4. health: the reported commit at the health URL that stays up 2 s, against the commit at the
     internal listener within `start_wait`;
  5. failure: restore and wait for the previous commit, reported `rolled_back` in `status.json`,
     against a put back, a `hold` file and a rollback barrier when the put back fails, and a trap for
     an interrupted session;
  6. after the start: `host check --json` into `host-check.json` (Settings reads it), against a
     separate verify (doctor, smoke test);
  7. progress: the steps of `status.json` the Settings page shows, against `vm:` lines;
  8. rollback to an older kept release: CD only.
  The update drain freezes each run's process group through the process host, skips runs a
  person paused and falls back to the turn-boundary hold; the move freezes the whole server and
  sessions units with the cgroup freezer. What they freeze and when they thaw differ.
  Done instead, behaviour identical: in `updates.rs` the drain's freeze pass uses `host_freeze`,
  one `known_freeze` lookup and one `thaw_runs` loop replace two copies each.
  `scripts/cargo nextest run -p yulius-app`: 740 passed; `-p yulius`: 3539 passed, 26 skipped.
- Security scan: Vidoc is switched off for this project (disabled, not passed).
