---
yulius-task: 3269ff66-bd71-4ab4-9331-c2e839e6eef3
status: in-progress
---

# SPEC-033: the product's integration tests build one binary, not fifty

Status: in progress
Date: 2026-10-12
Owner: Yulius task #31
Scope authorization: user request (F 2026-10-12): "recently I got dozens of CI errors coming from
disk problems. Identify and fix them. I dont thing simple CI needs that much disk"

## Goal

A build of the gate stops filling a sandbox: the `yulius` crate's integration tests link into
one test executable instead of one per file, so a worktree's or CI checkout's `target/` holds
about one copy of the product instead of fifty, and the 100 GB soft quota is no longer passed by
a few working tasks.

## Context

Symptom: dozens of Yulius CI runs ended in about 1 s with `quality: this sandbox uses 114.7 GB of
its 100.0 GB disk quota; free space (old worktrees, caches, installed packages) before starting
new sessions`. CI did not fail on its own disk use: `Manager::prepare`
(`crates/yulius-sandbox/src/manager.rs`) refuses every start, CI jobs included, while the
sandbox as a whole is over its quota.

Root cause, measured in this sandbox's CI checkout (`ci/jobs/quality/target`, one gate run):

- `target/debug/deps` held 11 GB, of which 9.2 GB were 51 test executables.
- 50 of them are `crates/yulius/tests/*.rs`: Cargo builds each file at the root of `tests/` as its
  own crate, and each links the whole product statically (web UI assets, every adapter, Tokio,
  Axum…): about 190 MB each (34 MB code, 24 MB read-only data, 62 MB line tables, 53 MB symbols).
  The other crates' test executables are 10 to 45 MB.
- One copy is about 9.5 GB, and a worktree keeps several: the gate builds with `--all-features`,
  an agent's `cargo test -p yulius --test X` without it, and any dependency or toolchain change
  gives new hashes; Cargo never removes the old files. Task transcripts show 21, 38, 42, 46, 48
  and 49 GB `target/` directories (tasks #4, #5, #8, #22; SPEC-008 incident).
- SPEC-008 and SPEC-019 prune caches of tasks that are not working; a working task's caches are
  never pruned (by design), so three or four working tasks reach the quota on their own and every
  CI run is refused until one ends.

The existing tests missed it because nothing checks how many test executables a crate builds.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NFR-010 | The composition crate (`yulius`, the one linking the whole product) builds exactly one integration test executable: `crates/yulius/tests/main.rs`, whose modules are the test files (`autotests = false`). The repository check fails when such a crate has more than one integration test target, or when a `.rs` file next to its `main.rs` is not declared as a module of it (it would silently not run). | `scripts/test_check_repository.py`, `scripts/dev check` |

## Plan

- [ ] Root cause measured: 50 test executables of about 190 MB each
- [ ] Spec and a repository check that fails on the current tree
- [ ] The yulius integration tests build as one binary, every gate green
- [ ] ADR, development docs and changes log describe the single test binary
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

1. `cargo metadata` lists one test target for `yulius` (`main`); the repository check passes.
2. The check fails with a named error for a second integration test target and for an undeclared
   test file.
3. `scripts/dev ci` runs every test it ran before (same test count) and is green.
4. A gate build's `target/debug/deps` holds one product test executable instead of 50.

## Test plan

- `scripts/test_check_repository.py`: the rule against synthetic packages (one target passes; two
  fail; a stray file fails; a non-composition crate with several targets passes) and against the
  real tree.
- `scripts/dev ci` before and after: same number of tests, all passing.
- Measure `target/debug/deps` test executables after the gate.
- Not automated: a real 100 GB sandbox with several working tasks.

## Decisions

- One binary for the composition crate only: its executables are 190 MB because they link
  everything; the other crates' are small, so their per-file targets stay (recorded in ADR-0025).
- Files stay where they are (`tests/<name>.rs`) and become modules of `tests/main.rs`; paths in
  earlier specs and `include_str!` stay valid. A test is now selected with
  `cargo test -p yulius --test main <file>::` (nextest: `-E 'binary(main)'`).
- Reducing debug info (`strip`, `split-debuginfo`) was rejected as the fix: it saves about a third
  of each executable but keeps fifty copies of the code and symbols, and it costs line numbers
  in backtraces.
- Pruning working tasks' caches was rejected: deleting a running build's `target/` breaks the
  agent's next build and SPEC-008 keeps working tasks untouched on purpose.
