---
yulius-task: 72b41e13-c202-41cb-b8ce-8fcc06885f83
status: in-progress
---

# SPEC-382: one zig host name for bootstrap's and doctor's messages

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #364
Scope authorization: task #364, Fixes #645 (nits left at the review round limit of task #201,
SPEC-214, ADR-0169).

## Goal

Bootstrap's skip message and doctor's error name an unpinned Linux host the same way, from one
helper that normalizes the machine name.

## Context

- Symptom: on a host whose `platform.machine()` is `RISCV64`, `scripts/dev doctor` says
  "No pinned zig for RISCV64-linux" while bootstrap (`zig_install.install`) says
  "no pinned zig for riscv64-linux". Only unpinned architectures reach either message.
- Root cause: the normalization (`MACHINES.get(machine.lower(), machine.lower())`) lives inside
  `zig_install.host_key`, which returns only the pinned key or `None`. `install`'s skip branch
  copies the expression to build its message; `check_repository.doctor` has no copy and prints the
  raw machine name.
- Why the tests missed it: `scripts/test_zig_install.py` tests the skip message with a lower-case
  `riscv64` only, and `scripts/test_check_repository.py` has no test of doctor's zig check, which
  is inline in `doctor()` beside subprocess calls.
- What else the cause could break: nothing else reads the host name; the download URL and the pin
  lookup already use `host_key`'s normalized key.

## Requirements

- **ZHN-001** `zig_install.host_name(machine)` returns the normalized `<arch>-linux` name
  (lower-case, `amd64` → `x86_64`, `arm64` → `aarch64`); `host_key` is built on it and `install`'s
  skip message uses it, with no second copy of the normalization.
- **ZHN-002** Doctor's "No pinned zig for <host> in .config/tools.toml" names `host_name`'s result;
  the message is built by `check_repository.zig_pin_error(machine, pins)`, which returns `None` for
  a pinned host, so it is unit-tested without running doctor.

## Plan

- [x] Spec for one zig host name
- [ ] Regression tests for an upper-case machine name
- [ ] One host_name helper used by bootstrap and doctor
- [ ] Gates, review and delivery

## Acceptance criteria

- With machine `RISCV64`, both bootstrap's skip message and doctor's error name `riscv64-linux`
  (ZHN-001, ZHN-002); tests prove it and failed before the change.
- `install` no longer repeats the normalization (ZHN-001).

## Test plan

- `test_zig_install`: `host_name("RISCV64")` is `riscv64-linux`, `host_name("AMD64")` is
  `x86_64-linux`; the unpinned skip with `RISCV64` names `riscv64-linux` and not `RISCV64`.
- `test_check_repository`: `zig_pin_error("RISCV64", pins)` names `riscv64-linux` and not
  `RISCV64-linux`; `zig_pin_error("AMD64", pins)` with `x86_64-linux` pinned is `None`.
- `python3 -m unittest discover -s scripts -p 'test_*.py'`, then `scripts/dev ci`.

## Decisions

- **A separate `host_name(machine)` rather than `host_key` returning a pair.** Both of the issue's
  options work; a pair would change `host_key`'s callers (bootstrap, doctor, tests) to unpack a
  tuple, while a helper keeps `host_key`'s contract and gives each message a one-call name.
  Reversible, no ADR.
- **Doctor's check moves into `zig_pin_error`**, a pure function beside `doctor()`, because
  `doctor()` runs rustup and the pinned tools and cannot be called from a unit test.
