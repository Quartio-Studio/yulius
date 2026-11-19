---
yulius-task: b8068c9c-ed44-4278-992a-7a75bf36f268
status: in-progress
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
- [ ] Isolate executable writes and verify twenty loaded crate runs
- [ ] Obtain independent approval and deliver on green CI

## Acceptance criteria

- **AC-1 (GTF-001)** A fork probe overlaps fixture construction and execution. The fixed helper executes the expected body on the first attempt even while a child retains a writer.
- **AC-2 (GTF-001, GTF-002)** Twenty consecutive invocations of `cargo test -p yulius-git` succeed alongside approximately fifteen runnable CPU workers.

## Test plan

Exercise the shared helper on Linux, demonstrate the regression against the original implementation, and run the whole crate twenty consecutive times under load. Stop the series on its first failure. Check all executable fixture call sites and the repository quick checks.

## Decisions

Only a subprocess opens the inode that will be executed for writing. This follows the test-fixture precedent in SPEC-494. Renaming a parent-written executable preserves the inherited writer, and close-on-exec still leaves a window before the child executes. This change is local to the Git integration tests.
