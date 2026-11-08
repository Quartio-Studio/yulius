---
yulius-task: 31ca8240-2be9-4bd9-8c11-8f0567af3c9b
status: done
---

# SPEC-445: Bootstrap tolerates a concurrent run in the shared CI home

Status: done
Date: 2026-11-07
Owner: studenciak00owl
Scope authorization: found while onboarding a fork of this repository; a job's installs in the shared
CI home must tolerate a concurrent run.

## Goal

Two CI runs that share one `$HOME` can both run `python3 scripts/bootstrap.py --install-rustup`
without breaking each other's rustup toolchain.

## Context

- A CI run (`ee7c4491`, on a fork) failed in `quality` after 17 s: two runs installed the pinned toolchain
  into the same `~/.rustup` at once ("recovering from a partially installed toolchain", "could not
  rename ... Directory not empty", "detected conflict"). Both CI lanes of a sandbox share the CI home.
- `bootstrap.py` already serialises the tool builds with an `flock` per tool in the tools cache; the
  rustup installer and `rustup toolchain install` are not.

## Requirements

- BSL-001: the rustup installer and `rustup toolchain install` run under one exclusive lock in the
  home (`$YULIUS_TOOLS_CACHE` or `~/.cache/yulius-tools`), so a second run waits and then finds the
  toolchain installed.
- BSL-002: the lock is a small module, `scripts/home_lock.py`, with a regression test.

## Plan

- [x] Test `home_lock.exclusive` (second holder waits; released on error), then add it.
- [x] Use it around the rustup install in `scripts/bootstrap.py`.
- [x] Gates and fresh-home proof.

## Acceptance criteria

- A second process asking for the lock blocks until the first leaves it, also when the first fails.
- `scripts/dev ci` and the fresh-home proof of `quality` pass.

## Test plan

`scripts/test_home_lock.py` (unittest, run by `scripts/dev check`): blocking and release on error.

## Decisions

- A file lock inside bootstrap, not `exclusive = true` on the job: that would queue whole runs of
  the 10-minute job behind each other for a 1-minute install.

## Evidence

- `scripts/dev ci` (quality job command): exit 0. Fresh-home proof of the quality command before this
  change: exit 0, 3199 tests passed. e2e: every file passes alone; under CPU load a handful of
  tests time out (different ones per run), none reproduced in isolation.
- `python3 -m unittest discover -s scripts`: 154 tests OK, including `test_home_lock`.
- Not testable here: the concurrent-install race itself (needs two CI lanes); the lock is covered by
  the unit test. `security` job needs Vidoc's key and runs on pull request heads.
