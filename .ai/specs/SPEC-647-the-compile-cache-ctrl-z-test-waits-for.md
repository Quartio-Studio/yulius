---
yulius-task: d57842b3-f12b-4504-9fbe-86b936cae92c
status: in-progress
---

# SPEC-647: The compile-cache Ctrl+Z test waits for Cargo's stopped state instead of reading it once

## Goal

Make `scripts/test_compile_cache.py::CompileCache::test_ctrl_z_stops_a_running_background_build_brought_forward`
(and `test_terminal_suspends_and_resumes_the_disposable_build`, which shares `check_shell_job`)
deterministic on a busy host, while it still fails when Cargo really keeps running after
Ctrl+Z (#1710).

## Context

- The test failed once in the `quality` job of PR #1703 (run
  `3906b33d-4133-4b1a-a33a-e3c6782e3e48`, head `a6d543d48`) with
  `AssertionError: 'R' != 'T' : Cargo kept running while Bash reported its job suspended`;
  that pull request changed nothing under `scripts/`.
- `check_shell_job` starts `scripts/cargo --scratch-target run` in an interactive Bash on a
  pty. The launcher reaches `run_disposable` (`scripts/compile_cache.py`), which starts Cargo
  (the fake Cargo of `caller_fixture`) with `subprocess.Popen` in the same process group, so
  the terminal's SIGTSTP reaches both.
- After Ctrl+Z the test waits for Bash's `Stopped`, then reads `/proc/<cargo>/stat` once.

## Root cause

Bash prints `Stopped` when `waitpid` sees its own child, the launcher, stop. It never waits
for the grandchild. The fake Cargo is asleep in `open()` of a FIFO; SIGTSTP wakes it, and it
enters the stopped state (`T`) only when the scheduler next runs it and it handles the signal.
Until then `/proc/<pid>/stat` reads `R`. On an idle host that happens before the test's read;
when the CPU is held, the read comes first. The product is right: the kernel delivers the stop
to the whole group and nothing has to forward it (the comment in `run_disposable`).

## Requirements

- CZ-001: After Bash reports the job stopped, the test waits, bounded by 5 s like its other
  waits, until Cargo's `/proc/<pid>/stat` state reads `T`, and fails at the bound with the
  last state read.
- CZ-002: The wait is a condition poll; no fixed delay hides the race.
- CZ-003: A Cargo that does not stop (SIGTSTP ignored) still fails the test.

## Plan

- [x] Race reproduced under a held CPU share with the single read
- [x] Bounded wait for the stopped state, test file passes in full
- [x] By-hand mutation: SIGTSTP ignored still fails the test
- [ ] Independent review in a fresh session, findings answered

## Acceptance criteria

- With a probe that starves the fake Cargo (SCHED_IDLE on CPU 0 beside a busy loop pinned to
  CPU 0), the old single read fails every run with `'R' != 'T'`; the same probe without the
  busy loop passes every run.
- With the fix, the same probe passes every run.
- With the fixture ignoring SIGTSTP, the test fails at the bound naming the state it read.
- `python3 -m unittest scripts.test_compile_cache` passes.

## Test plan

- Probe (not committed, `target/probe-631/`): `apply_probe.py` makes the fixture's Cargo call
  `sched_setaffinity(0, [0])` and `sched_setscheduler(SCHED_IDLE)` when `PROBE_IDLE` is set;
  `run.sh N` runs the test N times beside `taskset -c 0 python3 -c 'while 1: pass'`.
- Before and after the fix: `run.sh 20` (and more after the fix).
- Mutation by hand: the fixture source starts with
  `signal.signal(signal.SIGTSTP, signal.SIG_IGN)`; the test must fail.
- The whole test file.

## Decisions

- Poll `/proc/<pid>/stat` rather than synchronise on the signal: the test is not Cargo's parent,
  so `waitpid(WUNTRACED)` cannot see its stop, and no other notification exists for a stop of
  a non-child short of ptrace, which would change the stop itself. The 5 ms poll matches the
  `fg` wait above it. No ADR: a test-only change.

## Evidence

- Probe before the fix (dev `90ec692d2`, single read; `target/probe-631/run.sh 20`, fake Cargo
  `SCHED_IDLE` on CPU 0 beside `taskset -c 0 python3 -c 'while 1: pass'`): `pass=0 fail=20`,
  each `AssertionError: 'R' != 'T'`.
- Control, the same probe without the busy loop: `pass=20 fail=0`.
- Probe after the fix (`5a708d998`, `run.sh 50`): `pass=50 fail=0`.
- Mutation by hand: the fixture's first line
  `import signal; signal.signal(signal.SIGTSTP, signal.SIG_IGN)`:
  `test_ctrl_z_stops_a_running_background_build_brought_forward` and
  `test_terminal_suspends_and_resumes_the_disposable_build` both fail after 5 s with
  `Cargo kept running (state S) while Bash reported its job suspended`; reverted.
- `python3 -m unittest discover -s scripts -p test_compile_cache.py`: `Ran 49 tests`,
  `OK (skipped=7)`.
- Not tested here: the CI host's own load; the probe stands in for it.
