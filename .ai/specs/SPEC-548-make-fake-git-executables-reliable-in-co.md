---
yulius-task: b8068c9c-ed44-4278-992a-7a75bf36f268
status: done
---

# SPEC-548 Make fake Git executables reliable in concurrent tests

## Goal

Every fake Git and hook in `git_ops.rs` must run on its first attempt when other test threads fork during fixture setup.

## Context

[Issue #1332](https://github.com/Quartio-Studio/yulius/issues/1332) reports intermittent fetch failures on dev. Both fetch tests use `write_hook`. Three other fake Git writers also open the executable for writing inside the multithreaded test process. A fork can retain that writer until the child executes or exits. A Linux probe reproduced errno 26 even after the parent closed its writer and renamed the file.

## Requirements

- **GTF-001** Every generated executable in `git_ops.rs` must support immediate execution while other test threads fork. Preserve script contents, arguments, exit status and executable permissions.
- **GTF-002** Fixture creation must report write or installation failure. Do not retry execution.

## Plan

- [x] Reproduce the inherited writer during executable setup
- [x] Prove the race with a fork regression
- [x] Isolate executable writes and verify twenty loaded crate runs
- [x] Obtain independent approval for delivery on green CI

## Acceptance criteria

- **AC-1 (GTF-001)** A fork probe overlaps fixture construction and execution. The fixed helper executes the expected body on the first attempt even while a child retains a writer.
- **AC-2 (GTF-001, GTF-002)** Twenty consecutive invocations of `cargo test -p yulius-git` succeed alongside approximately fifteen runnable CPU workers.

## Test plan

Exercise the shared helper on Linux, demonstrate the regression against the original implementation, and run the whole crate twenty consecutive times under load. Stop the series on its first failure. Check all executable fixture call sites and the repository quick checks.

## Decisions

Only a subprocess opens the inode that will be executed for writing. This follows the test-fixture precedent in SPEC-494. Renaming a parent-written executable preserves the inherited writer, and close-on-exec still leaves a window before the child executes. This change is local to the Git integration tests.

## Evidence

- The Linux test `fake_git_runs_while_a_child_holds_an_inherited_writer` failed at 4cc394832 with errno 26. With the fix, all eight first attempts returned the expected body output while the child still held the old writer.
- A Linux probe forked once per iteration while the parent had the script body open, blocked the child on a pipe through execution, then released and reaped it. All 250 direct writes failed to execute with errno 26. All 250 runs using a separate source plus `install -m 755` succeeded.
- A syscall trace of `a_fetch_that_ends_past_its_limit_fails_and_one_under_it_succeeds` showed that only the install process opened the new `quick-git` for writing. Both fetch invocations executed it successfully.
- `scripts/dev quick` and `cargo clippy --locked -p yulius-git --tests -- -D warnings` passed.
- The independent reviewer approved 280dba28f. The reviewer ran the inherited-writer regression, five fetch tests and five hook, wrapper and configuration tests.
- `scripts/dev mutants` reported “No mutant in the Rust lines changed against origin/dev.” `scripts/dev security` reported that Vidoc is disabled for this project.
- Twenty consecutive `cargo test -p yulius-git` commands passed with fifteen CPU workers active throughout. Each ran 49 unit tests, eight credential-helper tests, 57 Git integration tests and two doctests. The shared Linux host’s one-minute load at the start of each run ranged from 241.77 to 447.90.
