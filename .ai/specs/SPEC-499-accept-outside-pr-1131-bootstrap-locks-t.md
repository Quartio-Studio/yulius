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
- BSL-003: `bootstrap.py`'s per-tool build locks and `zig_install.py`'s download lock use the same
  helper (one lock implementation in `scripts/`, same lock files and behavior as before).

## Plan

- [x] Spec SPEC-499 renumbered from the contributor's SPEC-445 and validated
- [x] The contributor's lock on current dev: home_lock with its tests, bootstrap holds it around the rustup install
- [x] A bootstrap test proves the toolchain install runs under the lock; per-tool locks share the helper
- [x] Changelog entry at the top of docs/changes.md, spec Evidence
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

What of #1131 was kept, changed or dropped (it cherry-picked cleanly onto c7348d044; the
contributor stays the author of that commit):

- Kept: `scripts/home_lock.py`, its tests, and the lock around the rustup installer and
  `rustup toolchain install` in `scripts/bootstrap.py`, unchanged.
- Changed: the spec, SPEC-445 on the fork, is renumbered to this task's SPEC-499 (the frontmatter
  names this task); the changelog entry moved from where the cherry-pick put it (under an older
  date) to the top. `test_home_lock`'s timing test now reads the clock before it starts its
  release timer, so a loaded host can only make the measured wait longer.
- Added: `scripts/test_bootstrap.py`, which proves the bootstrap holds the lock (the fork only
  tested the helper); the per-tool and zig locks now take the same helper (BSL-003).
- Dropped: nothing. `dev` still had no lock: every CI job of a sandbox runs with
  `HOME=<sandbox>/ci/home` and up to two lanes run at once.

Commands and results (2026-11-16, this sandbox):

- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 160 tests OK (the command
  `scripts/dev check` runs).
- `test_bootstrap` against `dev`'s `scripts/bootstrap.py` (copied over the file, then
  restored): fails with `AssertionError: 'free\n' != 'held\n'`; passes with the change.
- `python3 scripts/check_repository.py`: passed.
- The race itself, reproduced (the fork could not): `target/proof-481/race.sh` (not committed)
  ran two `python3 scripts/bootstrap.py --install-rustup` at once in one fresh `HOME` with
  `CI=true` and no rustup on `PATH`, the tools list emptied and zig unpinned so only the rustup
  part ran. With `dev`'s script one lane failed in 4 of 4 rounds: `OSError: [Errno 26] Text file
  busy: '<home>/.cargo/bin/rustup'` (rounds 1 and 4; one lane ran rustup while the other's
  installer was writing it) and `component download failed ... could not rename 'downloaded'
  file` in `~/.rustup/downloads`, after "recovering from a partially installed toolchain"
  (rounds 2 and 3). With this branch's script both lanes exited 0 in 5 of 5 rounds, the waiting
  one reporting `1.99.0-x86_64-unknown-linux-gnu unchanged`.
- `yulius security-scan`: exit 2, not run. Vidoc refused the instance's key (HTTP 401 "Missing
  or invalid API key"). The `security` job on the pull request reports the same until the key
  works.
- Not tested: Windows, where `home_lock` holds nothing (as the per-tool lock did before); the
  two lanes of a real Yulius CI starting at the same moment (the race proof above runs the same
  command in the same shared-home layout).
