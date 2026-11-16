---
yulius-task: 8831fd4a-2fd3-4d46-9bd7-a2733556fa41
status: in-progress
---

# SPEC-499: Bootstrap tolerates a concurrent run in the shared CI home

Status: in-progress
Date: 2026-11-16
Owner: Yulius task #481, from outside pull request #1131 by studenciak00owl (accepted by MrFishPL)
Scope authorization: task #481 brings #1131 onto current `dev`. This spec is the contributor's
SPEC-445 (written 2026-11-07 for their fork), renumbered to this task's number and adapted.

## Goal

Two CI runs that share one `$HOME` can both run `python3 scripts/bootstrap.py --install-rustup`
without breaking each other's rustup toolchain.

## Context

- A CI run (`ee7c4491`, on the contributor's fork) failed in `quality` after 17 s: two runs
  installed the pinned toolchain into the same `~/.rustup` at once ("recovering from a partially
  installed toolchain", "could not rename ... Directory not empty", "detected conflict").
- Still true on `dev` (c7348d044, 2026-11-16): every job of a sandbox's CI runs with
  `HOME=<sandbox>/ci/home` (`crates/yulius-app/src/ci/exec.rs`, only the build cache is per
  checkout), the dispatcher runs up to `CiSettings::lanes` runs per sandbox at once (default 2),
  and the `quality` job starts with `python3 scripts/bootstrap.py --install-rustup`
  (`yulius.toml`). `scripts/bootstrap.py` on `dev` installs rustup and the toolchain with no lock;
  only the pinned tool builds take an `flock` each in the tools cache.
- Task #479 changes CI admission in the Rust CI code; this change stays in `scripts/` and does not
  touch its files.

## Requirements

- BSL-001: the rustup installer and `rustup toolchain install` run under one exclusive lock in the
  tools cache (`$YULIUS_TOOLS_CACHE` or `~/.cache/yulius-tools`, `.rustup.lock`), so a second run
  waits and then finds the toolchain installed.
- BSL-002: the lock is a small module, `scripts/home_lock.py`, with regression tests: a second
  holder waits, and the lock is released when the holder fails.
- BSL-003: `bootstrap.py`'s per-tool build locks use the same helper (one lock implementation in
  the script, same files and behavior as before).

## Plan

- [x] Spec SPEC-499 renumbered from the contributor's SPEC-445 and validated
- [ ] The contributor's lock on current dev: home_lock with its tests, bootstrap holds it around the rustup install
- [ ] A bootstrap test proves the toolchain install runs under the lock; per-tool locks share the helper
- [ ] Changelog entry at the top of docs/changes.md, spec Evidence
- [ ] Independent review answered test-first

## Acceptance criteria

- A second process asking for the lock blocks until the first leaves it, also when the first fails.
- `bootstrap.py` run with a stand-in `rustup` finds `.rustup.lock` held by itself while
  `rustup toolchain install` runs, and free afterwards.
- `scripts/dev check` (it runs `unittest discover -s scripts`) and Yulius CI pass.

## Test plan

- `scripts/test_home_lock.py` (the contributor's): blocking and release on error.
- `scripts/test_bootstrap.py`: runs `scripts/bootstrap.py` in a temporary home with a stand-in
  `rustup` first on `PATH`; the stand-in tries a non-blocking `flock` on the lock file during
  `toolchain install` and records whether it was refused, then fails `rustup which` so the script
  stops before building tools. Fails on `dev` without the change (the lock is free).

## Decisions

- A file lock inside bootstrap, not `exclusive = true` on the job (the contributor's choice, kept):
  that would queue whole runs of the 10-minute job behind each other for a 1-minute install.
- The lock lives in the tools cache next to the per-tool locks (kept), not in `~/.rustup`: rustup
  owns that folder. Both lanes see the same cache because it is derived from the shared `HOME`.
- `rustup which` and the tool builds stay outside the rustup lock: they only read the installed
  toolchain, and the tool builds keep their own per-tool locks.
- No ADR: a local serialisation detail of one script, recorded here.

## Evidence

(filled in when the change is complete)
