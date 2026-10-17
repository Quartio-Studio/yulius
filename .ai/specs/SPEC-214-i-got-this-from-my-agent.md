---
yulius-task: 85082c48-c2db-498e-8518-e72973747852
status: in-progress
---

# SPEC-214: Yulius builds on macOS again, and the gate keeps it so

Status: in-progress
Date: 2026-10-17
Owner: task #201
Scope authorization: task #201 (requested by MrFishPL), quoting their agent: "The current Yulius
release doesn't build on macOS. The sandbox and app crates use Linux-only calls since #620, so the
full yulius CLI can't be installed on the Mac at all." — "fix, everything should build on mac as
well". Fixes #636.

## Goal

Every workspace target compiles on macOS, and Yulius CI, which runs on Linux only, fails when a
change stops it from doing so.

## Context

macOS is a supported development host ([development](../../docs/engineering/development.md)), but
Yulius CI runs the gate on Linux only and macOS was "checked by running `scripts/dev ci` on a Mac
before promotion" (SPEC-002 ADR-21 made it a manual check), which nobody did.

Reproduced on Linux by cross-checking for `aarch64-apple-darwin` (`rustup target add`, zig 0.17.0
as the C compiler of the C that `ring` and `libsqlite3-sys` vendor):

| Error | Site | Cause |
| --- | --- | --- |
| E0433 `rustix` not found (x3) | `yulius-sandbox/src/base.rs` `lock_builds` | #620 used `rustix::fs::{flock, OFlags}`, but `rustix` is a Linux-only dependency of the crate |
| E0599 no `OFlag::O_PATH` | `yulius-sandbox/src/engine_supervisor.rs` `connect_control` | `O_PATH` and `/proc/self/fd` are Linux-only; the function had no cfg gate |
| E0308 mismatched types | `yulius-app/src/vault/env_files.rs` `write_in` | `Mode::from_bits_truncate` takes `mode_t`, `u16` on macOS; the function took `u32` |

Once these compile, the rest of the workspace (`yulius-app`, `yulius-server`, `yulius`, tools) is
Clippy-clean for macOS with `--all-targets --all-features`. Why tests missed it: nothing in the
gate compiles for any target but the host's.

What else the cause could break: any new Linux-only call outside a `cfg(target_os = "linux")`
gate. The new gate stage catches all of them at compile time. Runtime behavior on macOS (which
paths work without Linux namespaces) is unchanged and out of scope: the sandbox's Docker engine,
namespaces and containment stay Linux-only, as before #620.

## Requirements

- **MAC-001** `cargo clippy --locked --workspace --all-targets --all-features --target
  aarch64-apple-darwin -- -D warnings` passes.
- **MAC-002** `scripts/dev macos` runs that check, and `scripts/dev ci` (the `quality` job of
  Yulius CI) runs it after `lint`; on Linux it compiles vendored C with the pinned zig through
  `scripts/zig_macos.py`, on a Mac with the Mac's own compiler.
- **MAC-003** Bootstrap installs the macOS Rust target (`rust-toolchain.toml` `targets`) and, on
  Linux, the zig tarball pinned by version and SHA-256 in `.config/tools.toml`, once per home
  cache; `scripts/dev doctor` checks its version.
- **MAC-004** Linux behavior is unchanged: the build lock is still an exclusive `flock` on a file
  opened with `O_NOFOLLOW` (now `std::fs::File::lock`), and the engine control socket still opens
  with `O_PATH | O_NOFOLLOW`; on other systems `connect_control` answers `Unsupported`.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- `scripts/dev macos` fails on `dev` before the fix with the three errors above and passes after.
- `scripts/dev ci` passes on Linux with the new stage.
- The wrapper's flag filtering and the gate's environment are unit-tested
  (`scripts/test_zig_macos.py`, `tools/xtask` tests).

## Test plan

- Regression: the `macos` gate stage itself, run on the parent commit's crates (fails) and on the
  fix (passes).
- Unit: `scripts/test_zig_macos.py` (flags dropped and kept, roles), xtask `macos_env` (zig
  wrappers off a Mac, nothing on a Mac).
- Full gate: `scripts/dev ci`.
- Not testable here: linking and running on a real Mac (no Apple SDK on Linux); the check stops at
  type-checking, which is where every reported failure was.

## Decisions

- [ADR-0169](../../docs/decisions/0169-the-gate-compiles-every-target-for-macos-on-linux.md): the
  gate cross-checks `aarch64-apple-darwin` on Linux with Clippy, the vendored C compiled by a
  pinned zig.
